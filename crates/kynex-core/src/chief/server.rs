//! Kynex's MCP server for the chief: JSON-RPC over HTTP on 127.0.0.1, behind
//! a token made for each run. Every call is answered as the reply to its POST;
//! there's no server-to-client stream.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};

use super::{Call, NAME, SYSTEM_PROMPT, tools, worker_tools};
use crate::session::Wake;

/// How long a tool call waits for the app, except a worker's question, which waits for the user.
const ANSWER_WAIT: Duration = Duration::from_secs(30);

/// Who is calling: the chief, or the worker of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Role {
    Chief,
    Worker(PathBuf),
}

/// A tool call waiting for the app's answer.
pub struct Request {
    pub call: Call,
    reply: Sender<Result<String, String>>,
}

impl Request {
    pub fn answer(self, result: Result<String, String>) {
        let _ = self.reply.send(result);
    }
}

/// The server. Stops when dropped.
pub struct Server {
    port: u16,
    token: String,
    rx: Receiver<Request>,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// Listens on a free port on 127.0.0.1; `wake` rings when a call comes in.
    pub fn start(wake: Wake) -> Result<Self, String> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .map_err(|e| format!("couldn't start the chief's tools: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token = token();
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let (auth, stopped) = (format!("Bearer {token}"), stop.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let (tx, wake, auth) = (tx.clone(), wake.clone(), auth.clone());
                std::thread::spawn(move || serve(stream, &auth, &tx, &wake));
            }
        });
        set_worker_endpoint(Some((port, token.clone())));
        Ok(Self { port, token, rx, stop })
    }

    /// The calls that came in since last asked.
    pub fn requests(&self) -> Vec<Request> {
        self.rx.try_iter().collect()
    }

    /// The `claude` arguments that give a session the chief's role and tools,
    /// and only those.
    pub fn launch_args(&self) -> Vec<String> {
        let config = json!({ "mcpServers": { NAME: {
            "type": "http",
            "url": format!("http://127.0.0.1:{}/mcp", self.port),
            "headers": { "Authorization": format!("Bearer {}", self.token) },
        } } });
        [
            "--mcp-config",
            &config.to_string(),
            "--strict-mcp-config",
            "--allowedTools",
            &format!("mcp__{NAME}"),
            "--append-system-prompt",
            SYSTEM_PROMPT,
        ]
        .map(String::from)
        .to_vec()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        set_worker_endpoint(None);
        // Wakes the accept loop so it sees the flag.
        let _ = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], self.port)), Duration::from_millis(200));
    }
}

/// The chief's launch arguments for sessions in [`crate::paths::chief_dir`],
/// set by the app once its server runs.
fn launch() -> &'static Mutex<Vec<String>> {
    static LAUNCH: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    LAUNCH.get_or_init(Mutex::default)
}

pub fn set_launch_args(args: Vec<String>) {
    if let Ok(mut l) = launch().lock() {
        *l = args;
    }
}

/// Where the server listens, for the workers' `ask_user`, while it runs.
fn worker_endpoint() -> &'static Mutex<Option<(u16, String)>> {
    static ENDPOINT: OnceLock<Mutex<Option<(u16, String)>>> = OnceLock::new();
    ENDPOINT.get_or_init(Mutex::default)
}

fn set_worker_endpoint(endpoint: Option<(u16, String)>) {
    if let Ok(mut e) = worker_endpoint().lock() {
        *e = endpoint;
    }
}

/// The ``claude`` arguments that give a worker in ``project`` Kynex's ``ask_user``,
/// next to the project's own MCP servers; none while the server isn't running.
pub fn worker_launch_args(project: &Path) -> Vec<String> {
    let Some((port, token)) = worker_endpoint().lock().ok().and_then(|e| e.clone()) else {
        return Vec::new();
    };
    let config = json!({ "mcpServers": { NAME: {
        "type": "http",
        "url": format!("http://127.0.0.1:{port}/worker/{}", hex(&project.to_string_lossy())),
        "headers": { "Authorization": format!("Bearer {token}") },
    } } });
    vec!["--mcp-config".into(), config.to_string()]
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<String> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> =
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect();
    String::from_utf8(bytes?).ok()
}

/// Who a request's path is for: ``/mcp`` the chief, ``/worker/<hex path>`` a worker.
fn role_for(target: &str) -> Option<Role> {
    let path = target.split('?').next().unwrap_or("");
    if path == "/mcp" {
        return Some(Role::Chief);
    }
    path.strip_prefix("/worker/").and_then(unhex).filter(|p| !p.is_empty()).map(|p| Role::Worker(PathBuf::from(p)))
}

/// The extra ``claude`` arguments for a chat session in ``cwd``: the chief's, in its folder.
pub fn launch_args_for(cwd: &Path) -> Vec<String> {
    if cwd != crate::paths::chief_dir() {
        return Vec::new();
    }
    launch().lock().map(|l| l.clone()).unwrap_or_default()
}

/// A token nobody else on the machine can guess, for this run.
fn token() -> String {
    let part = || {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        h.write_u32(std::process::id());
        h.finish()
    };
    format!("{:016x}{:016x}", part(), part())
}

/// Answers one HTTP connection.
fn serve(stream: TcpStream, auth: &str, tx: &Sender<Request>, wake: &Wake) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(mut out) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    let Some(req) = read_request(&mut reader) else { return };
    let response = if req.authorization.as_deref() != Some(auth) {
        http(401, "")
    } else if req.method != "POST" {
        http(405, "")
    } else if let Some(role) = role_for(&req.target) {
        let message = serde_json::from_slice::<Value>(&req.body).unwrap_or(Value::Null);
        match handle(&message, &role, |call| ask(call, tx, wake)) {
            Some(answer) => http(200, &answer.to_string()),
            None => http(202, ""),
        }
    } else {
        http(404, "")
    };
    let _ = out.write_all(response.as_bytes());
    // Closing with bytes unread resets the connection on Windows, losing the
    // answer: say we're done, then let the client close first.
    let _ = out.shutdown(std::net::Shutdown::Write);
    let _ = reader.get_mut().set_read_timeout(Some(Duration::from_secs(1)));
    let _ = std::io::copy(&mut reader, &mut std::io::sink());
}

/// Passes `call` to the app and waits for its answer.
fn ask(call: Call, tx: &Sender<Request>, wake: &Wake) -> Result<String, String> {
    // A worker's question waits as long as the user takes; the rest, a moment.
    let patient = matches!(call, Call::Ask { .. });
    let (reply, answer) = channel();
    tx.send(Request { call, reply }).map_err(|_| "Kynex is closing".to_string())?;
    wake();
    if patient {
        return answer.recv().map_err(|_| "The question was withdrawn: Kynex stopped this worker.".to_string())?;
    }
    answer.recv_timeout(ANSWER_WAIT).map_err(|_| "Kynex didn't answer in time".to_string())?
}

struct HttpRequest {
    method: String,
    /// The request target, e.g. `/mcp`.
    target: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

fn read_request(r: &mut impl BufRead) -> Option<HttpRequest> {
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut first = line.split_whitespace();
    let method = first.next()?.to_string();
    let target = first.next().unwrap_or("").to_string();
    let (mut length, mut authorization) = (0usize, None);
    loop {
        line.clear();
        if r.read_line(&mut line).ok()? == 0 {
            break;
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        let Some((k, v)) = l.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        if k.eq_ignore_ascii_case("content-length") {
            length = v.parse().ok()?;
        } else if k.eq_ignore_ascii_case("authorization") {
            authorization = Some(v.to_string());
        }
    }
    // Calls are a few kilobytes; anything near this isn't one.
    if length > 4 << 20 {
        return None;
    }
    let mut body = vec![0; length];
    r.read_exact(&mut body).ok()?;
    Some(HttpRequest { method, target, authorization, body })
}

fn http(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Method Not Allowed",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The JSON-RPC answer to `message`, `None` for a notification. Tool calls
/// go to `call`.
fn handle(message: &Value, role: &Role, call: impl FnOnce(Call) -> Result<String, String>) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message["method"].as_str().unwrap_or("");
    let result = match method {
        "initialize" => json!({
            "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": NAME, "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": if *role == Role::Chief { tools() } else { worker_tools() } }),
        "tools/call" => {
            let params = &message["params"];
            let name = params["name"].as_str().unwrap_or("");
            let parsed = match role {
                Role::Chief => Call::parse(name, &params["arguments"]),
                Role::Worker(project) => Call::parse_worker(name, &params["arguments"], project.clone()),
            };
            let outcome = parsed.and_then(call);
            let (text, is_error) = match outcome {
                Ok(t) => (t, false),
                Err(e) => (e, true),
            };
            json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
        }
        _ => {
            return Some(json!({ "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("unknown method {method}") } }));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;
    use crate::chief::DEFAULT_MODEL;

    #[test]
    fn answers_the_protocol() {
        let init = handle(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-03-26" } }),
            &Role::Chief,
            |_| unreachable!(),
        )
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        assert_eq!(
            handle(
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
                &Role::Chief,
                |_| unreachable!()
            ),
            None
        );
        let list =
            handle(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }), &Role::Chief, |_| unreachable!())
                .unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), tools().as_array().unwrap().len());
        let call = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "result", "arguments": { "project": "kynex" } } });
        let ok = handle(&call, &Role::Chief, |c| {
            assert_eq!(c, Call::Result { project: "kynex".into() });
            Ok("done".into())
        })
        .unwrap();
        assert_eq!(ok["result"]["content"][0]["text"], "done");
        assert_eq!(ok["result"]["isError"], false);
        let err = handle(&call, &Role::Chief, |_| Err("no".into())).unwrap();
        assert_eq!(err["result"]["isError"], true);
        let unknown =
            handle(&json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/list" }), &Role::Chief, |_| unreachable!());
        assert_eq!(unknown.unwrap()["error"]["code"], -32601);
    }

    #[test]
    fn a_worker_gets_ask_user_and_nothing_else() {
        let project = PathBuf::from("/r/kynex");
        let role = Role::Worker(project.clone());
        let list =
            handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }), &role, |_| unreachable!()).unwrap();
        let names: Vec<&str> =
            list["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["ask_user"]);
        // The chief's tools aren't a worker's.
        let dispatch = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "dispatch", "arguments": { "project": "x", "prompt": "y" } } });
        assert_eq!(handle(&dispatch, &role, |_| unreachable!()).unwrap()["result"]["isError"], true);
        let ask = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "ask_user",
            "arguments": { "question": "Which DB?", "header": "Database", "options": ["Postgres", { "label": "SQLite", "description": "one file" }] } } });
        let answered = handle(&ask, &role, |c| {
            let Call::Ask { project: p, input } = c else { panic!("{c:?}") };
            assert_eq!(p, project);
            assert_eq!(input["questions"][0]["question"], "Which DB?");
            assert_eq!(input["questions"][0]["options"][1]["description"], "one file");
            assert_eq!(input["questions"][0]["multiSelect"], false);
            Ok("The user answered: SQLite".into())
        })
        .unwrap();
        assert_eq!(answered["result"]["content"][0]["text"], "The user answered: SQLite");
        // Too few choices is the worker's mistake to fix.
        let thin = json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "ask_user", "arguments": { "question": "?", "options": ["only"] } } });
        assert_eq!(handle(&thin, &role, |_| unreachable!()).unwrap()["result"]["isError"], true);
    }

    #[test]
    fn routes_by_path_and_round_trips_a_project_path() {
        assert_eq!(role_for("/mcp"), Some(Role::Chief));
        let path = r"C:\Users\me\repos\ünï code";
        assert_eq!(role_for(&format!("/worker/{}", hex(path))), Some(Role::Worker(PathBuf::from(path))));
        assert_eq!(role_for("/worker/zz"), None);
        assert_eq!(role_for("/worker/"), None);
        assert_eq!(role_for("/elsewhere"), None);
    }

    #[test]
    fn answers_are_told_in_words() {
        let mut answers = serde_json::Map::new();
        assert!(crate::chief::answer_text(&answers).starts_with("The user didn't answer"));
        answers.insert("Which DB?".into(), json!("SQLite"));
        assert_eq!(crate::chief::answer_text(&answers), "The user answered:\nWhich DB? → SQLite");
    }

    /// A real worker asks through `ask_user` and gets the answer back. Run with
    /// `cargo test -- --ignored a_worker_asks`.
    #[test]
    #[ignore = "needs the claude CLI and uses a little quota"]
    fn a_worker_asks_through_ask_user() {
        let server = Server::start(Arc::new(|| {})).unwrap();
        let project = std::env::temp_dir();
        let mut cmd = std::process::Command::new(crate::cli::program());
        cmd.args([
            "-p",
            "Use the ask_user tool now to ask which colour I prefer, with the options Red and Blue. \
             Then repeat the answer you got, word for word.",
            "--model",
            DEFAULT_MODEL,
            "--allowedTools",
            &format!("mcp__{NAME}"),
        ])
        .args(worker_launch_args(&project))
        .current_dir(&project);
        let run = std::thread::spawn(move || cmd.output().unwrap());
        let mut asked = None;
        while !run.is_finished() {
            for r in server.requests() {
                if let Call::Ask { project: p, input } = &r.call {
                    assert_eq!(*p, std::env::temp_dir());
                    asked = Some(input.clone());
                }
                let mut answers = serde_json::Map::new();
                answers.insert("which".into(), json!("Blue"));
                r.answer(Ok(crate::chief::answer_text(&answers)));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let out = run.join().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let input = asked
            .unwrap_or_else(|| panic!("no question; stdout: {text}; stderr: {}", String::from_utf8_lossy(&out.stderr)));
        let labels: Vec<&str> =
            input["questions"][0]["options"].as_array().unwrap().iter().filter_map(|o| o["label"].as_str()).collect();
        assert_eq!(labels, ["Red", "Blue"]);
        assert!(text.contains("Blue"), "{text}");
    }

    #[test]
    #[ignore = "needs the claude CLI and uses a little quota"]
    fn the_cli_calls_kynex_tools() {
        let server = Server::start(Arc::new(|| {})).unwrap();
        let mut cmd = std::process::Command::new(crate::cli::program());
        cmd.args(["-p", "Call the status tool, then repeat its answer word for word.", "--tools", ""])
            .args(["--model", DEFAULT_MODEL])
            .args(server.launch_args())
            .current_dir(std::env::temp_dir());
        let run = std::thread::spawn(move || cmd.output().unwrap());
        let mut called = false;
        while !run.is_finished() {
            for r in server.requests() {
                called = true;
                r.answer(Ok("Workers: pelican-42".into()));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let out = run.join().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(called, "no tool call; stdout: {text}; stderr: {}", String::from_utf8_lossy(&out.stderr));
        assert!(text.contains("pelican-42"), "{text}");
    }

    #[test]
    fn serves_over_http_with_the_token() {
        let wake: Wake = Arc::new(|| {});
        let server = Server::start(wake).unwrap();
        let args = server.launch_args();
        let config: Value = serde_json::from_str(&args[1]).unwrap();
        let url = config["mcpServers"]["kynex"]["url"].as_str().unwrap().to_string();
        let auth = config["mcpServers"]["kynex"]["headers"]["Authorization"].as_str().unwrap().to_string();
        let port: u16 = url.trim_start_matches("http://127.0.0.1:").trim_end_matches("/mcp").parse().unwrap();
        let post = |auth: &str, body: &Value| {
            let body = body.to_string();
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let request = format!(
                "POST /mcp HTTP/1.1\r\nHost: x\r\nAuthorization: {auth}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            s.write_all(request.as_bytes()).unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        };
        let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
        assert!(post("Bearer nope", &list).starts_with("HTTP/1.1 401"));
        assert!(post(&auth, &list).contains("\"dispatch\""));
        // A tool call waits for the app's answer.
        let answering = std::thread::spawn(move || {
            loop {
                if let Some(r) = server.requests().pop() {
                    assert_eq!(r.call, Call::Status);
                    r.answer(Ok("all quiet".into()));
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let call = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "status" } });
        assert!(post(&auth, &call).contains("all quiet"));
        answering.join().unwrap();
    }
}
