//! A session's browser server, run by Kynex so it outlives the session's
//! `claude` processes.
//!
//! `@playwright/mcp --extension` drives your own browser through its extension.
//! Started by `claude` as a stdio server it lives and dies with that process,
//! so every restart pairs with the browser again: a new connect tab, and the
//! session's tabs fall out of their group. So for each `--extension` server
//! configured for a folder, Kynex runs the same command over HTTP on localhost
//! (`--port`, `--shared-browser-context`) and hands it to each `claude` of the
//! session under the same name: `--mcp-config` replaces the configured entry.
//!
//! The server lets go of the browser when its last client leaves, which a
//! restart is. So once the session has used the browser, Kynex connects as a
//! client of its own and stays ([`BrowserServer::hold`]); the pairing and the
//! tabs carry over to the next process.
//!
//! `npx` takes seconds to start, so a spare server per configuration waits
//! ready ([`take`]). A process that starts before its server answers gets the
//! configured stdio server, as without Kynex.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};

use crate::extensions::{self, Approval, Server, Source, Transport};

/// An `--extension` browser server configured for a folder.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Entry {
    /// The name sessions know it by, which Kynex's server takes over.
    pub name: String,
    command: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

/// The `--extension` browser servers sessions in `cwd` get. Blocks reading the
/// CLI's config files.
pub fn entries(cwd: &Path) -> Vec<Entry> {
    let Some(paths) = extensions::Paths::detect() else { return Vec::new() };
    let global = extensions::read_json(&paths.global).unwrap_or(Value::Null);
    let mcp_json = extensions::read_json(&cwd.join(".mcp.json")).unwrap_or(Value::Null);
    let config = extensions::load_from(&paths, cwd);
    entries_from(&config.servers, &global, &mcp_json, cwd)
}

/// [`entries`] from the configured `servers` and the files they came from.
fn entries_from(servers: &[Server], global: &Value, mcp_json: &Value, cwd: &Path) -> Vec<Entry> {
    let disabled = extensions::disabled_servers(global, cwd);
    servers
        .iter()
        .filter(|s| s.approval == Approval::Approved && !disabled.contains(&s.name))
        .filter(|s| matches!(s.source, Source::Local | Source::Project | Source::User))
        .filter_map(|s| {
            let Transport::Stdio { command, args } = &s.transport else { return None };
            let playwright = args.iter().any(|a| a.starts_with("@playwright/mcp"));
            let extension = args.iter().any(|a| a == "--extension");
            // Already set up to serve over HTTP: it's the user's to run.
            let served = args.iter().any(|a| a.starts_with("--port") || a.starts_with("--host"));
            (playwright && extension && !served).then(|| Entry {
                name: s.name.clone(),
                command: command.clone(),
                args: args.clone(),
                env: extensions::server_env(global, mcp_json, cwd, s),
            })
        })
        .collect()
}

/// The configured command line, serving over HTTP on `port` with one browser
/// context for every client.
fn serve_args(entry: &Entry, port: u16) -> Vec<String> {
    let mut args = entry.args.clone();
    args.extend(["--host", "127.0.0.1", "--port", &port.to_string(), "--shared-browser-context"].map(Into::into));
    args
}

/// `--mcp-config` for `servers`: each under its configured name, over HTTP.
pub fn mcp_config<'a>(servers: impl IntoIterator<Item = &'a BrowserServer>) -> String {
    let map: serde_json::Map<String, Value> =
        servers.into_iter().map(|s| (s.name.clone(), json!({ "type": "http", "url": s.url() }))).collect();
    json!({ "mcpServers": map }).to_string()
}

/// A running browser server.
pub struct BrowserServer {
    pub name: String,
    entry: Entry,
    port: u16,
    child: Child,
    /// Kynex's own client session on it, once it holds the browser.
    held: Arc<Mutex<Option<String>>>,
    holding: bool,
}

impl BrowserServer {
    /// Starts `entry`'s command as an HTTP server on a free port. Doesn't wait
    /// for it to answer; see [`Self::ready`].
    fn start(entry: &Entry) -> Result<Self, String> {
        let port = free_port()?;
        let mut cmd = Command::new(program(&entry.command));
        cmd.args(serve_args(entry, port))
            .envs(entry.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        crate::process::own_group(&mut cmd);
        let child = crate::process::dies_with_parent(&mut cmd)
            .spawn()
            .map_err(|e| format!("couldn't start the browser server {}: {e}", entry.name))?;
        Ok(Self { name: entry.name.clone(), entry: entry.clone(), port, child, held: Arc::default(), holding: false })
    }

    /// `localhost`, not the address it's bound to: the server turns away any
    /// other `Host`.
    fn url(&self) -> String {
        format!("http://localhost:{}/mcp", self.port)
    }

    /// It answers on its port. Doesn't block: a closed port refuses at once.
    pub fn ready(&self) -> bool {
        TcpStream::connect_timeout(&addr(self.port), Duration::from_millis(50)).is_ok()
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Kynex holds its browser, so it stays paired through restarts.
    pub fn held(&self) -> bool {
        self.held.lock().is_ok_and(|h| h.is_some())
    }

    /// Connects Kynex as a client that stays, and has it look at the tabs:
    /// the browser connection opens on a client's first tool call and closes
    /// with its last client. In the background; once per server.
    pub fn hold(&mut self) {
        if self.holding {
            return;
        }
        self.holding = true;
        let (port, held, name) = (self.port, self.held.clone(), self.name.clone());
        std::thread::spawn(move || match open_session(port) {
            Ok(id) => {
                if let Ok(mut h) = held.lock() {
                    *h = Some(id);
                }
            }
            Err(e) => eprintln!("kynex: couldn't hold the browser of {name}: {e}"),
        });
    }
}

impl Drop for BrowserServer {
    fn drop(&mut self) {
        crate::process::kill_tree(&mut self.child);
    }
}

/// Spare servers, started ahead so a session's first process can have one.
fn spares() -> &'static Mutex<HashMap<Entry, BrowserServer>> {
    static SPARES: OnceLock<Mutex<HashMap<Entry, BrowserServer>>> = OnceLock::new();
    SPARES.get_or_init(Mutex::default)
}

/// A server for `entry`: the spare if there is one, else one started now.
/// Starts the next spare either way.
pub fn take(entry: &Entry) -> Option<BrowserServer> {
    let mut spares = spares().lock().ok()?;
    let spare = spares.remove(entry).and_then(|mut s| s.alive().then_some(s));
    let server = spare.or_else(|| BrowserServer::start(entry).map_err(|e| eprintln!("kynex: {e}")).ok());
    // An older configuration of the same server (a new token) is done with.
    spares.retain(|e, _| e.name != entry.name);
    if let Ok(next) = BrowserServer::start(entry) {
        spares.insert(entry.clone(), next);
    }
    server
}

/// Stops the spare servers. Kynex's job object does this when it exits; a
/// test, which has none, does it itself.
pub fn stop_spares() {
    if let Ok(mut spares) = spares().lock() {
        spares.clear();
    }
}

/// Brings `servers` in line with `wanted`: keeps the ones still configured and
/// running, starts the missing ones, drops the rest.
pub fn reconcile(servers: &mut Vec<BrowserServer>, wanted: &[Entry]) {
    servers.retain_mut(|s| wanted.contains(&s.entry) && s.alive());
    for e in wanted {
        if !servers.iter().any(|s| &s.entry == e)
            && let Some(s) = take(e)
        {
            servers.push(s);
        }
    }
}

/// `npx` as Windows needs it spelled to start it: `npx.cmd`. (Node also puts
/// an extensionless `npx`, a shell script, beside it.)
fn program(command: &str) -> PathBuf {
    if cfg!(windows) && Path::new(command).extension().is_none() {
        for ext in ["exe", "cmd", "bat"] {
            if let Some(p) = crate::mcp_config::on_path(&format!("{command}.{ext}")) {
                return p;
            }
        }
    }
    PathBuf::from(command)
}

fn addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

fn free_port() -> Result<u16, String> {
    let listener = TcpListener::bind(addr(0)).map_err(|e| format!("no free port for the browser server: {e}"))?;
    listener.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

/// Opens an MCP session on the server at `port` and makes one tool call, which
/// connects the browser (or joins the connection a session already made).
/// Blocks until the browser answers.
fn open_session(port: u16) -> Result<String, String> {
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "Kynex", "version": "1" } },
    });
    let reply = post(port, None, &init, Duration::from_secs(30))?;
    let id = reply.session.ok_or("the browser server gave no session")?;
    post(
        port,
        Some(&id),
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        Duration::from_secs(10),
    )?;
    let call = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "browser_tabs", "arguments": { "action": "list" } },
    });
    // Waits on the browser, which waits on you if the extension asks first.
    let reply = post(port, Some(&id), &call, Duration::from_secs(600))?;
    match reply.message {
        Some(m) if m.get("error").is_some() => Err(format!("the browser server refused: {}", m["error"])),
        _ => Ok(id),
    }
}

/// An HTTP reply, as much of it as an MCP client needs.
#[derive(Debug, Default, PartialEq)]
struct Reply {
    status: u16,
    session: Option<String>,
    /// The JSON-RPC answer, from a JSON body or the first one in an event stream.
    message: Option<Value>,
}

/// POSTs `body` to the MCP endpoint at `port` and reads the whole reply.
fn post(port: u16, session: Option<&str>, body: &Value, wait: Duration) -> Result<Reply, String> {
    let body = body.to_string();
    let mut request = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(s) = session {
        request.push_str(&format!("Mcp-Session-Id: {s}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&body);
    let err = |e: std::io::Error| format!("the browser server didn't answer: {e}");
    let mut stream = TcpStream::connect_timeout(&addr(port), Duration::from_secs(2)).map_err(err)?;
    stream.set_read_timeout(Some(wait)).map_err(err)?;
    stream.write_all(request.as_bytes()).map_err(err)?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(err)?;
    let reply = parse_reply(&raw).ok_or("the browser server's answer couldn't be read")?;
    if !(200..300).contains(&reply.status) {
        return Err(format!("the browser server answered {}", reply.status));
    }
    Ok(reply)
}

/// An HTTP/1.1 response: status, `Mcp-Session-Id`, and the JSON-RPC message in
/// a JSON or `text/event-stream` body, plain or chunked.
fn parse_reply(raw: &[u8]) -> Option<Reply> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.split("\r\n");
    let status = lines.next()?.split_whitespace().nth(1)?.parse().ok()?;
    let (mut session, mut chunked) = (None, false);
    for line in lines {
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        if k.eq_ignore_ascii_case("mcp-session-id") {
            session = Some(v.to_string());
        } else if k.eq_ignore_ascii_case("transfer-encoding") && v.eq_ignore_ascii_case("chunked") {
            chunked = true;
        }
    }
    let body = &raw[split + 4..];
    let body = if chunked { dechunk(body) } else { body.to_vec() };
    let text = String::from_utf8_lossy(&body);
    let is_answer = |v: &Value| v.get("result").is_some() || v.get("error").is_some();
    let message = match serde_json::from_str::<Value>(text.trim()) {
        Ok(v) => Some(v),
        Err(_) => text
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .find(is_answer),
    };
    Some(Reply { status, session, message })
}

/// A chunked body joined back up; what can be read of it if it's cut short.
fn dechunk(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(eol) = body.windows(2).position(|w| w == b"\r\n") {
        let size = String::from_utf8_lossy(&body[..eol]);
        let Ok(size) = usize::from_str_radix(size.split(';').next().unwrap_or("").trim(), 16) else { break };
        let start = eol + 2;
        if size == 0 || start > body.len() {
            break;
        }
        let end = start.saturating_add(size).min(body.len());
        out.extend_from_slice(&body[start..end]);
        body = body.get(end + 2..).unwrap_or(&[]);
    }
    out
}

/// For the fuzz target: a reply from a browser server, whatever it holds.
pub(crate) fn fuzz_reply(raw: &[u8]) {
    let _ = parse_reply(raw);
    let _ = dechunk(raw);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(command: &str, args: &[&str]) -> Transport {
        Transport::Stdio { command: command.into(), args: args.iter().map(|a| a.to_string()).collect() }
    }

    #[test]
    fn only_extension_playwright_servers_are_taken_over() {
        let cwd = Path::new("/work/app");
        let global = json!({
            "mcpServers": {
                "playwright-edge": { "command": "npx", "args": ["@playwright/mcp@latest", "--browser", "msedge", "--extension"],
                    "env": { "PLAYWRIGHT_MCP_EXTENSION_TOKEN": "t" } },
                "playwright": { "command": "npx", "args": ["@playwright/mcp@latest"] },
                "served": { "command": "npx", "args": ["@playwright/mcp", "--extension", "--port", "9000"] },
                "off": { "command": "npx", "args": ["@playwright/mcp", "--extension"] },
                "web": { "type": "http", "url": "http://localhost:1/mcp" },
            },
            "projects": { "/work/app": { "disabledMcpServers": ["off"] } },
        });
        let servers = extensions::servers(&global, &Value::Null, &[], cwd);
        let got = entries_from(&servers, &global, &Value::Null, cwd);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "playwright-edge");
        assert_eq!(got[0].env, [("PLAYWRIGHT_MCP_EXTENSION_TOKEN".to_string(), "t".to_string())]);
        assert_eq!(
            serve_args(&got[0], 4321),
            [
                "@playwright/mcp@latest",
                "--browser",
                "msedge",
                "--extension",
                "--host",
                "127.0.0.1",
                "--port",
                "4321",
                "--shared-browser-context"
            ]
        );
    }

    #[test]
    fn project_servers_only_once_allowed() {
        let cwd = Path::new("/w");
        let server = |approval| Server {
            name: "pw".into(),
            source: Source::Project,
            transport: stdio("npx", &["@playwright/mcp", "--extension"]),
            approval,
        };
        let got = |a| entries_from(&[server(a)], &Value::Null, &Value::Null, cwd).len();
        assert_eq!((got(Approval::Approved), got(Approval::Pending), got(Approval::Rejected)), (1, 0, 0));
    }

    #[test]
    fn reads_json_and_event_stream_replies() {
        let sse = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nmcp-session-id: abc\r\n\r\n\
            event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/x\"}\r\n\r\n\
            data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\r\n\r\n";
        let r = parse_reply(sse).unwrap();
        assert_eq!((r.status, r.session.as_deref()), (200, Some("abc")));
        assert_eq!(r.message.unwrap()["result"]["ok"], true);

        let plain = b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\n\r\n{\"error\":{\"code\":1}}";
        let r = parse_reply(plain).unwrap();
        assert_eq!(r.status, 400);
        assert_eq!(r.message.unwrap()["error"]["code"], 1);

        let accepted = b"HTTP/1.1 202 Accepted\r\n\r\n";
        assert_eq!(parse_reply(accepted).unwrap(), Reply { status: 202, ..Reply::default() });
        assert_eq!(parse_reply(b"HTTP/1.1 200 OK\r\n"), None);
        assert_eq!(parse_reply(b"nonsense\r\n\r\n"), None);
    }

    #[test]
    fn joins_chunked_bodies() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
            7\r\ndata: {\r\n14;ext=1\r\n\"id\":1,\"result\":{}}\n\r\n0\r\n\r\n";
        let r = parse_reply(raw).unwrap();
        assert_eq!(r.message.unwrap()["id"], 1);
        assert_eq!(dechunk(b"3\r\nabc\r\n2\r\nde"), b"abcde");
        assert_eq!(dechunk(b"ff\r\nshort"), b"short");
        assert_eq!(dechunk(b"zz\r\nabc"), b"");
    }

    #[test]
    fn config_names_each_server_by_its_own_name() {
        let entry = Entry { name: "playwright-edge".into(), command: "x".into(), args: vec![], env: vec![] };
        let server = BrowserServer {
            name: entry.name.clone(),
            entry,
            port: 4321,
            // Something to own that ends at once; never started as a server.
            child: Command::new(if cfg!(windows) { "cmd" } else { "true" })
                .args(if cfg!(windows) { &["/c", "exit"][..] } else { &[][..] })
                .spawn()
                .unwrap(),
            held: Arc::default(),
            holding: false,
        };
        let v: Value = serde_json::from_str(&mcp_config([&server])).unwrap();
        assert_eq!(
            v,
            json!({ "mcpServers": { "playwright-edge": { "type": "http", "url": "http://localhost:4321/mcp" } } })
        );
        assert!(!server.held());
    }

    /// Against a real `@playwright/mcp --extension` and your browser, as a
    /// session goes: a first `claude` opens a page, Kynex holds the browser,
    /// the first `claude` goes, and the next one still finds the page.
    #[test]
    #[ignore = "needs npx, a browser with the Playwright extension, and its token; opens example.com in it"]
    fn holds_the_browser_across_clients() {
        let wait = |what: &str, done: &mut dyn FnMut() -> bool| {
            for _ in 0..600 {
                if done() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            panic!("timed out waiting for {what}");
        };
        let call = |port: u16, id: &str, tool: &str, args: Value| {
            let call = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": tool, "arguments": args } });
            post(port, Some(id), &call, Duration::from_secs(120)).unwrap().message.unwrap().to_string()
        };
        let entry = entries(&std::env::current_dir().unwrap()).into_iter().next().expect("an --extension server");
        let mut server = BrowserServer::start(&entry).unwrap();
        let port = server.port;
        wait("the server", &mut || server.ready());

        let first = open_session(port).unwrap();
        call(port, &first, "browser_navigate", json!({ "url": "https://example.com" }));
        server.hold();
        wait("Kynex to hold the browser", &mut || server.held());
        let close = format!(
            "DELETE /mcp HTTP/1.1\r\nHost: localhost:{port}\r\nMcp-Session-Id: {first}\r\nConnection: close\r\n\r\n"
        );
        let mut s = TcpStream::connect(addr(port)).unwrap();
        s.write_all(close.as_bytes()).unwrap();
        let _ = s.read_to_end(&mut Vec::new());

        let next = open_session(port).unwrap();
        assert!(call(port, &next, "browser_tabs", json!({ "action": "list" })).contains("example.com"));
    }
}
