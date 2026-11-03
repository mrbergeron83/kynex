//! A session's browser server, run by Kynex so it outlives the session's
//! `claude` processes.
//!
//! `@playwright/mcp --extension` drives your own browser through its extension.
//! Each server is one client of the extension, with its own tab group and
//! colour. Started by `claude` as a stdio server it lives and dies with that
//! process, so every restart pairs with the browser again: a new connect tab,
//! and the session's tabs fall out of their group. So for each `--extension`
//! server configured for a folder, Kynex runs the same command over HTTP on
//! localhost (`--port`, `--shared-browser-context`) and hands it to each
//! `claude` of the session under the same name: `--mcp-config` replaces the
//! configured entry. One per session, so sessions keep to their own groups.
//!
//! A process that starts before its server answers still gets it, never the
//! stdio one, which would be one more client; the session has it reconnect
//! once the server is up (#258).
//!
//! The server lets go of the browser when its last client leaves, which a
//! restart is. So once the session has used the browser, Kynex connects as a
//! client of its own and stays ([`BrowserServer::hold`]); the pairing and the
//! tabs carry over to the next process.
//!
//! A watcher thread keeps it that way (#267, #268): a server that dies or stops
//! answering starts again on the same port, which the running `claude` still
//! has, and the session has it reconnect. While the session is using the
//! browser, Kynex's client lists the tabs every [`KEEPALIVE`]: that reads each
//! tab's title through the extension, and the traffic keeps Edge from stopping
//! the extension's idle service worker, which drops every connection.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::extensions::{self, Approval, Server, Source, Transport};
use crate::process::Tree;
use crate::session::Wake;

/// How often a held browser is touched; under the ~30 s an extension's service
/// worker may sit idle.
const KEEPALIVE: Duration = Duration::from_secs(20);
/// How long after the session last used the browser the keepalive goes on.
/// After that the connection may drop; the next use connects again.
const KEEP_FOR: Duration = Duration::from_secs(15 * 60);
/// Checks in a row a started server may refuse its port before it counts as hung.
const HUNG_AFTER: u32 = 5;
/// The longest wait between restarts of a server that keeps failing.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// An `--extension` browser server configured for a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The name sessions know it by, which Kynex's server takes over.
    pub name: String,
    launch: Launch,
}

/// What a server runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Launch {
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
                launch: Launch {
                    command: command.clone(),
                    args: args.clone(),
                    env: extensions::server_env(global, mcp_json, cwd, s),
                },
            })
        })
        .collect()
}

/// The configured command line, serving over HTTP on `port` with one browser
/// context for every client.
fn serve_args(launch: &Launch, port: u16) -> Vec<String> {
    let mut args = launch.args.clone();
    args.extend(["--host", "127.0.0.1", "--port", &port.to_string(), "--shared-browser-context"].map(Into::into));
    args
}

/// `--mcp-config` for `servers`: each under its configured name, over HTTP.
pub fn mcp_config<'a>(servers: impl IntoIterator<Item = &'a BrowserServer>) -> String {
    config_for(servers.into_iter().map(|s| (s.name.as_str(), s.port)))
}

fn config_for<'a>(servers: impl IntoIterator<Item = (&'a str, u16)>) -> String {
    let map: serde_json::Map<String, Value> = servers
        .into_iter()
        .map(|(name, port)| (name.to_string(), json!({ "type": "http", "url": url(port) })))
        .collect();
    json!({ "mcpServers": map }).to_string()
}

/// `localhost`, not the address it's bound to: the server turns away any
/// other `Host`.
fn url(port: u16) -> String {
    format!("http://localhost:{port}/mcp")
}

/// A session's running browser server for one of its entries.
pub struct BrowserServer {
    pub name: String,
    launch: Launch,
    port: u16,
    shared: Arc<Shared>,
}

/// What the session and the watcher share.
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// The server was dropped: the watcher ends.
    stop: AtomicBool,
}

#[derive(Default)]
struct State {
    tree: Option<Tree>,
    /// Counts starts, so a hold or keepalive that outlived its server is ignored.
    generation: u64,
    /// Kynex's own client session on it, once it holds the browser.
    held: Option<String>,
    /// A hold is under way or done.
    holding: bool,
    /// The keepalive is under way.
    pinging: bool,
    /// When the session last drove the browser.
    used_at: Option<Instant>,
    /// It started again and the session hasn't had its process reconnect yet.
    restarted: bool,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl BrowserServer {
    /// Starts `entry`'s command as an HTTP server on a free port, and its
    /// watcher, which rings `wake` when it starts it again. Doesn't wait for it
    /// to answer; see [`Self::ready`].
    fn start(entry: &Entry, wake: Wake) -> Result<Self, String> {
        let port = free_port()?;
        let tree = spawn(&entry.launch, port, &entry.name)?;
        let shared = Arc::new(Shared::default());
        shared.state().tree = Some(tree);
        let server = Self { name: entry.name.clone(), launch: entry.launch.clone(), port, shared };
        let (shared, launch, name) = (server.shared.clone(), server.launch.clone(), server.name.clone());
        std::thread::spawn(move || watch(&shared, &launch, port, &name, &wake));
        Ok(server)
    }

    /// It answers on its port. Doesn't block: a closed port refuses at once.
    pub fn ready(&self) -> bool {
        answers(self.port)
    }

    /// Kynex holds its browser, so it stays paired through restarts.
    pub fn held(&self) -> bool {
        self.shared.state().held.is_some()
    }

    /// The session drove the browser: keeps the connection up for a while
    /// ([`KEEP_FOR`]), and connects Kynex as a client that stays and has it
    /// look at the tabs: the browser connection opens on a client's first tool
    /// call and closes with its last client. In the background; again only if
    /// the last hold failed or its server started again.
    pub fn hold(&mut self) {
        let mut st = self.shared.state();
        st.used_at = Some(Instant::now());
        if st.holding {
            return;
        }
        st.holding = true;
        let generation = st.generation;
        drop(st);
        let (port, shared, name) = (self.port, self.shared.clone(), self.name.clone());
        std::thread::spawn(move || {
            let got = open_session(port);
            let mut st = shared.state();
            if st.generation != generation {
                return;
            }
            match got {
                Ok(id) => st.held = Some(id),
                Err(e) => {
                    st.holding = false;
                    eprintln!("kynex: couldn't hold the browser of {name}: {e}");
                }
            }
        });
    }

    /// It started again and answers now: once, so the session's process
    /// reconnects to it.
    pub fn take_restarted(&self) -> bool {
        if !self.shared.state().restarted || !self.ready() {
            return false;
        }
        self.shared.state().restarted = false;
        true
    }
}

impl Drop for BrowserServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        // Dropping the tree stops the server.
        let tree = self.shared.state().tree.take();
        drop(tree);
    }
}

/// Starts `launch` serving on `port`.
fn spawn(launch: &Launch, port: u16, name: &str) -> Result<Tree, String> {
    let mut cmd = Command::new(program(&launch.command));
    cmd.args(serve_args(launch, port))
        .envs(launch.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    Tree::spawn(&mut cmd).map_err(|e| format!("couldn't start the browser server {name}: {e}"))
}

/// What the watcher does about a server it just looked at.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Fine,
    /// Gone, or started and then stopped answering: start it again.
    Restart,
}

/// The watcher's memory between looks at one server.
#[derive(Debug)]
struct Health {
    answered: bool,
    refused: u32,
    backoff: Duration,
    next_start: Instant,
}

impl Health {
    fn new(now: Instant) -> Self {
        Self { answered: false, refused: 0, backoff: Duration::from_secs(2), next_start: now }
    }

    /// One look: whether the process runs and whether its port answers.
    fn look(&mut self, now: Instant, alive: bool, answering: bool) -> Step {
        if alive && answering {
            // Up: the next failure restarts it soon again.
            (self.answered, self.refused, self.backoff) = (true, 0, Duration::from_secs(2));
            return Step::Fine;
        }
        if alive {
            // Still starting, or hung once it had answered.
            if !self.answered {
                return Step::Fine;
            }
            self.refused += 1;
            if self.refused < HUNG_AFTER {
                return Step::Fine;
            }
        }
        if now < self.next_start {
            return Step::Fine;
        }
        // Starts that fail at once back off, up to a minute apart.
        self.next_start = now + self.backoff;
        self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
        (self.answered, self.refused) = (false, 0);
        Step::Restart
    }
}

/// The watcher: restarts the server when it's gone and keeps a held browser
/// busy. Ends when the server is dropped.
fn watch(shared: &Arc<Shared>, launch: &Launch, port: u16, name: &str, wake: &Wake) {
    let mut health = Health::new(Instant::now());
    let mut last_ping = Instant::now();
    // The start the session was last told about.
    let mut rung = 0;
    while !shared.stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_secs(1));
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        let alive = shared.state().tree.as_ref().is_some_and(Tree::alive);
        let answering = alive && answers(port);
        if health.look(Instant::now(), alive, answering) == Step::Restart {
            // The old one goes first: it may still hold the port.
            let old = shared.state().tree.take();
            drop(old);
            let tree = spawn(launch, port, name).map_err(|e| eprintln!("kynex: {e}")).ok();
            let mut st = shared.state();
            if shared.stop.load(Ordering::Relaxed) {
                return;
            }
            st.tree = tree;
            st.generation += 1;
            (st.held, st.holding, st.pinging, st.restarted) = (None, false, false, true);
            continue;
        }
        let mut st = shared.state();
        if answering && st.restarted && rung != st.generation {
            // Up again: the session has its process reconnect.
            rung = st.generation;
            drop(st);
            wake();
            continue;
        }
        let in_use = st.used_at.is_some_and(|t| t.elapsed() < KEEP_FOR);
        if answering
            && in_use
            && !st.pinging
            && last_ping.elapsed() >= KEEPALIVE
            && let Some(id) = st.held.clone()
        {
            st.pinging = true;
            last_ping = Instant::now();
            let (shared, generation) = (shared.clone(), st.generation);
            std::thread::spawn(move || keepalive(&shared, generation, port, &id));
        }
    }
}

/// Lists the tabs as Kynex's client, which reads each one's title through the
/// extension. A session the server no longer knows is let go, so the next
/// use holds the browser again.
fn keepalive(shared: &Shared, generation: u64, port: u16, id: &str) {
    let got = call_tool(port, id, "browser_tabs", json!({ "action": "list" }), Duration::from_secs(30));
    let mut st = shared.state();
    if st.generation != generation {
        return;
    }
    st.pinging = false;
    if let Err(e) = got
        && e.contains("answered 4")
    {
        (st.held, st.holding) = (None, false);
    }
}

/// Brings a session's `servers` in line with `wanted`: keeps the ones still
/// configured (their watchers restart them), starts the missing ones, drops
/// (and so stops) the rest. Each watcher rings `wake` when it restarts one.
pub fn reconcile(servers: &mut Vec<BrowserServer>, wanted: &[Entry], wake: &Wake) {
    servers.retain(|s| wanted.iter().any(|e| e.name == s.name && e.launch == s.launch));
    for e in wanted {
        if !servers.iter().any(|s| s.name == e.name) {
            match BrowserServer::start(e, wake.clone()) {
                Ok(s) => servers.push(s),
                Err(e) => eprintln!("kynex: {e}"),
            }
        }
    }
}

/// Something answers on `port`. Doesn't block: a closed port refuses at once.
fn answers(port: u16) -> bool {
    TcpStream::connect_timeout(&addr(port), Duration::from_millis(50)).is_ok()
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
    // Waits on the browser, which waits on you if the extension asks first.
    call_tool(port, &id, "browser_tabs", json!({ "action": "list" }), Duration::from_secs(600))?;
    Ok(id)
}

/// Calls `tool` in MCP session `id` on the server at `port`. Blocks up to `wait`.
fn call_tool(port: u16, id: &str, tool: &str, args: Value, wait: Duration) -> Result<Option<Value>, String> {
    let call = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    });
    let reply = post(port, Some(id), &call, wait)?;
    match reply.message {
        Some(m) if m.get("error").is_some() => Err(format!("the browser server refused: {}", m["error"])),
        m => Ok(m),
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
        assert_eq!(got[0].launch.env, [("PLAYWRIGHT_MCP_EXTENSION_TOKEN".to_string(), "t".to_string())]);
        assert_eq!(
            serve_args(&got[0].launch, 4321),
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
        let v: Value = serde_json::from_str(&config_for([("playwright-edge", 4321), ("chrome", 4322)])).unwrap();
        assert_eq!(
            v,
            json!({ "mcpServers": {
                "playwright-edge": { "type": "http", "url": "http://localhost:4321/mcp" },
                "chrome": { "type": "http", "url": "http://localhost:4322/mcp" },
            } })
        );
    }

    /// Each session runs its own server, which is its own client of the
    /// extension and so its own tab group.
    #[test]
    fn each_session_runs_its_own_server() {
        // A stand-in that stays up; never asked anything.
        let launch = |tag: &str| Launch {
            command: if cfg!(windows) { "cmd" } else { "sh" }.into(),
            args: if cfg!(windows) {
                vec!["/c".into(), format!("ping -n 30 127.0.0.1 >nul & rem {tag}")]
            } else {
                vec!["-c".into(), format!("sleep 30; true {tag}")]
            },
            env: vec![],
        };
        let entry = |name: &str, tag: &str| Entry { name: name.into(), launch: launch(tag) };
        let wake: Wake = Arc::new(|| {});
        let (mut a, mut b) = (Vec::new(), Vec::new());
        reconcile(&mut a, &[entry("pw", "one")], &wake);
        reconcile(&mut b, &[entry("pw", "one")], &wake);
        assert_ne!(a[0].port, b[0].port, "a server, and so a tab group, per session");

        // The same configuration keeps its server; a new token gets a new one.
        let port = a[0].port;
        reconcile(&mut a, &[entry("pw", "one")], &wake);
        assert_eq!(a[0].port, port);
        reconcile(&mut a, &[entry("pw", "two")], &wake);
        assert_eq!(a.len(), 1);
        assert_ne!(a[0].port, port);
        reconcile(&mut a, &[], &wake);
        assert!(a.is_empty());
    }

    #[test]
    fn a_server_that_dies_starts_again_on_its_port() {
        // A stand-in that ends at once, as a stopped server does.
        let launch = Launch {
            command: if cfg!(windows) { "cmd" } else { "sh" }.into(),
            args: if cfg!(windows) { vec!["/c".into(), "exit".into()] } else { vec!["-c".into(), "exit".into()] },
            env: vec![],
        };
        let rang = Arc::new(AtomicBool::new(false));
        let r = rang.clone();
        let wake: Wake = Arc::new(move || r.store(true, Ordering::Relaxed));
        let mut servers = Vec::new();
        reconcile(&mut servers, &[Entry { name: "pw".into(), launch }], &wake);
        let (port, shared) = (servers[0].port, servers[0].shared.clone());
        let start = Instant::now();
        while shared.state().generation < 2 && start.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(shared.state().generation >= 2, "started again, twice");
        assert_eq!(servers[0].port, port, "the running claude still has the old URL");
        assert!(shared.state().restarted);
        // It never answers, so the session isn't told to reconnect yet.
        assert!(!servers[0].take_restarted());
        assert!(!rang.load(Ordering::Relaxed));
        drop(servers);
        assert!(shared.stop.load(Ordering::Relaxed));
        assert!(shared.state().tree.is_none(), "dropping the server stops it");
    }

    #[test]
    fn health_restarts_the_dead_and_the_hung_with_backoff() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut h = Health::new(t0);
        // Starting: alive but not answering yet is fine, however long.
        for i in 0..20 {
            assert_eq!(h.look(t0 + s(i), true, false), Step::Fine);
        }
        assert_eq!(h.look(t0 + s(20), true, true), Step::Fine);
        // Hung once it had answered: restarted after HUNG_AFTER refusals.
        for i in 1..HUNG_AFTER {
            assert_eq!(h.look(t0 + s(20 + u64::from(i)), true, false), Step::Fine);
        }
        assert_eq!(h.look(t0 + s(30), true, false), Step::Restart);
        // Dying at once backs off: 2 s, then 4 s, …
        assert_eq!(h.look(t0 + s(31), false, false), Step::Fine);
        assert_eq!(h.look(t0 + s(32), false, false), Step::Restart);
        assert_eq!(h.look(t0 + s(35), false, false), Step::Fine);
        assert_eq!(h.look(t0 + s(36), false, false), Step::Restart);
        for _ in 0..10 {
            h.look(t0 + s(10_000), false, false);
            h.next_start = t0;
        }
        assert_eq!(h.backoff, MAX_BACKOFF);
        // Up again: the backoff starts over.
        h.look(t0 + s(10_001), true, true);
        assert_eq!(h.backoff, s(2));
    }

    /// Against a real `@playwright/mcp --extension` and your browser: the
    /// server is killed under a held browser, starts again on its port, and
    /// drives the browser again.
    #[test]
    #[ignore = "needs npx, a browser with the Playwright extension, and its token; opens example.com in it"]
    fn the_browser_comes_back_after_its_server_dies() {
        let wait = |what: &str, done: &mut dyn FnMut() -> bool| {
            let start = Instant::now();
            while !done() {
                assert!(start.elapsed() < Duration::from_secs(120), "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(200));
            }
        };
        let entry = entries(&std::env::current_dir().unwrap()).into_iter().next().expect("an --extension server");
        let mut servers = Vec::new();
        reconcile(&mut servers, &[entry], &(Arc::new(|| {}) as Wake));
        let mut server = servers.pop().expect("the server started");
        let port = server.port;
        wait("the server", &mut || server.ready());
        let first = open_session(port).unwrap();
        call_tool(port, &first, "browser_navigate", json!({ "url": "https://example.com" }), Duration::from_secs(120))
            .unwrap();
        server.hold();
        wait("Kynex to hold the browser", &mut || server.held());

        // Killed, as `Stop-Process` on every Playwright process did.
        let tree = server.shared.state().tree.take();
        drop(tree);
        wait("the server to start again", &mut || server.take_restarted());
        assert_eq!(server.port, port);
        assert!(!server.held(), "the old hold went with the old server");

        let next = open_session(port).unwrap();
        let got = call_tool(
            port,
            &next,
            "browser_navigate",
            json!({ "url": "https://example.com" }),
            Duration::from_secs(120),
        )
        .unwrap()
        .unwrap()
        .to_string();
        assert!(got.contains("example.com"), "{got}");
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
        let mut servers = Vec::new();
        reconcile(&mut servers, &[entry], &(Arc::new(|| {}) as Wake));
        let mut server = servers.pop().expect("the server started");
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
        let tabs = call(port, &next, "browser_tabs", json!({ "action": "list" }));
        drop(server);
        assert!(tabs.contains("example.com"));
    }
}
