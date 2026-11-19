//! The browser Kynex gives every Claude session: `@playwright/mcp` in its own
//! Edge or Chrome window, picked in Settings. One server, shared by every
//! session over HTTP ([`shared`]), handed to each `claude` with
//! `--mcp-config` under the name the old presets used (`playwright-edge`), so it
//! takes the place of such an entry in Claude's own config instead of adding a
//! second browser. Playwright keeps a profile per folder, so logins made in its
//! window stay.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::json;

/// A browser Playwright can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Browser {
    Edge,
    Chrome,
}

impl Browser {
    pub const ALL: [Self; 2] = [Self::Edge, Self::Chrome];

    pub fn label(self) -> &'static str {
        match self {
            Self::Edge => "Edge",
            Self::Chrome => "Chrome",
        }
    }

    /// `@playwright/mcp`'s `--browser` value.
    fn channel(self) -> &'static str {
        match self {
            Self::Edge => "msedge",
            Self::Chrome => "chrome",
        }
    }

    /// The MCP server's name, which its tools carry (`mcp__playwright-edge__…`).
    pub fn server(self) -> String {
        format!("playwright-{}", self.label().to_ascii_lowercase())
    }

    /// Whether it's installed, by where its installers put it. Blocks on a few
    /// file checks.
    pub fn installed(self) -> bool {
        self.places().iter().any(|p| p.exists())
    }

    #[cfg(windows)]
    fn places(self) -> Vec<PathBuf> {
        let rel = match self {
            Self::Edge => ["Microsoft", "Edge", "Application", "msedge.exe"],
            Self::Chrome => ["Google", "Chrome", "Application", "chrome.exe"],
        };
        ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|root| rel.iter().fold(PathBuf::from(root), |p, part| p.join(part)))
            .collect()
    }

    #[cfg(target_os = "macos")]
    fn places(self) -> Vec<PathBuf> {
        let app = match self {
            Self::Edge => "Microsoft Edge.app",
            Self::Chrome => "Google Chrome.app",
        };
        let mut out = vec![std::path::Path::new("/Applications").join(app)];
        out.extend(dirs::home_dir().map(|h| h.join("Applications").join(app)));
        out
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    fn places(self) -> Vec<PathBuf> {
        let names: &[&str] = match self {
            Self::Edge => &["microsoft-edge", "microsoft-edge-stable"],
            Self::Chrome => &["google-chrome", "google-chrome-stable"],
        };
        names.iter().filter_map(|n| crate::mcp_config::on_path(n)).collect()
    }
}

/// The Settings choice: which browser sessions get, if any.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pick {
    Off,
    #[default]
    Edge,
    Chrome,
}

impl Pick {
    pub const ALL: [Self; 3] = [Self::Edge, Self::Chrome, Self::Off];

    pub fn browser(self) -> Option<Browser> {
        match self {
            Self::Off => None,
            Self::Edge => Some(Browser::Edge),
            Self::Chrome => Some(Browser::Chrome),
        }
    }

    pub fn label(self) -> &'static str {
        self.browser().map_or("Off", Browser::label)
    }

    fn code(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Edge => 1,
            Self::Chrome => 2,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Off,
            2 => Self::Chrome,
            _ => Self::Edge,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(1);

/// Sets the app-wide choice; sessions started from now on follow it.
pub fn set(pick: Pick) {
    CURRENT.store(pick.code(), Ordering::Relaxed);
    shared::stop_unless(pick.browser());
}

pub fn current() -> Pick {
    Pick::from_code(CURRENT.load(Ordering::Relaxed))
}

/// `--mcp-config` for `browser`: the shared server's URL, or, if it won't start,
/// a server of the session's own as before. Blocks while the shared one starts.
pub fn mcp_config(browser: Browser) -> String {
    match shared::port(browser) {
        Ok(port) => http_config(browser, port),
        Err(_) => stdio_config(browser),
    }
}

fn http_config(browser: Browser, port: u16) -> String {
    json!({ "mcpServers": { browser.server(): {
        "type": "http",
        "url": format!("http://127.0.0.1:{port}/mcp"),
    } } })
    .to_string()
}

fn stdio_config(browser: Browser) -> String {
    json!({ "mcpServers": { browser.server(): {
        "type": "stdio",
        "command": "npx",
        "args": ["@playwright/mcp@latest", "--browser", browser.channel()],
    } } })
    .to_string()
}

/// One `@playwright/mcp` for every session, over HTTP, instead of an `npx` and
/// a `node` per session. Each session that connects still gets a browser of its
/// own, with the profile of its folder, as with a server of its own.
mod shared {
    use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
    use std::process::{Command, Stdio};
    use std::sync::mpsc::{Sender, channel};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::Browser;
    use crate::process::Tree;

    /// A first run downloads the package, which can take a while.
    const READY_WITHIN: Duration = Duration::from_secs(90);

    struct Server {
        browser: Browser,
        port: u16,
        tree: Arc<Tree>,
        /// Dropping it ends the thread that holds the process, which ends it.
        _stop: Sender<()>,
    }

    static SERVER: Mutex<Option<Server>> = Mutex::new(None);

    /// The port of the running server for `browser`, starting it (and ending
    /// one for the other browser) if needed. Blocks until it takes connections;
    /// sessions starting meanwhile wait for the same one.
    pub(super) fn port(browser: Browser) -> Result<u16, String> {
        let mut slot = SERVER.lock().map_err(|_| "browser server lock poisoned".to_string())?;
        if let Some(s) = slot.as_ref()
            && s.browser == browser
            && s.tree.alive()
        {
            return Ok(s.port);
        }
        *slot = None;
        let server = start(browser)?;
        let port = server.port;
        *slot = Some(server);
        Ok(port)
    }

    /// Ends the server unless it's for `keep`. Doesn't wait for one that is
    /// starting: the next session's start replaces that if it's the wrong one.
    pub(super) fn stop_unless(keep: Option<Browser>) {
        if let Ok(mut slot) = SERVER.try_lock()
            && slot.as_ref().is_some_and(|s| Some(s.browser) != keep)
        {
            *slot = None;
        }
    }

    fn start(browser: Browser) -> Result<Server, String> {
        let port = free_port()?;
        let mut cmd = command(browser, port);
        // On its own thread for as long as it runs: on Linux a child dies with
        // the thread that spawned it, and a session's start thread is brief.
        let (tree_tx, tree_rx) = channel();
        let (stop, stop_rx) = channel::<()>();
        std::thread::spawn(move || match Tree::spawn(&mut cmd) {
            Ok(tree) => {
                let tree = Arc::new(tree);
                let _ = tree_tx.send(Ok(tree.clone()));
                // Until the server is dropped (or replaced).
                let _ = stop_rx.recv();
            }
            Err(e) => {
                let _ = tree_tx.send(Err(format!("failed to start the browser server: {e}")));
            }
        });
        let tree = tree_rx.recv().map_err(|_| "the browser server's thread ended".to_string())??;
        let server = Server { browser, port, tree, _stop: stop };
        let deadline = Instant::now() + READY_WITHIN;
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        loop {
            if TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok() {
                return Ok(server);
            }
            if !server.tree.alive() {
                return Err("the browser server exited at start".into());
            }
            if Instant::now() > deadline {
                return Err("the browser server didn't start in time".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// A port nobody listens on right now.
    fn free_port() -> Result<u16, String> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| format!("no free port: {e}"))?;
        listener.local_addr().map(|a| a.port()).map_err(|e| format!("no free port: {e}"))
    }

    fn command(browser: Browser, port: u16) -> Command {
        let npx = if cfg!(windows) { "npx.cmd" } else { "npx" };
        let mut cmd = Command::new(crate::mcp_config::on_path(npx).unwrap_or_else(|| npx.into()));
        cmd.args(args(browser, port));
        cmd.envs(crate::fresh_env::vars().iter().map(|(n, v)| (n, v)));
        // stdin stays open while Kynex holds it; the server ends when it closes (macOS).
        cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd
    }

    pub(super) fn args(browser: Browser, port: u16) -> Vec<String> {
        let port = port.to_string();
        ["@playwright/mcp@latest", "--browser", browser.channel(), "--host", "127.0.0.1", "--port", &port]
            .map(String::from)
            .to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn config_names_the_server_by_browser() {
        let v: Value = serde_json::from_str(&stdio_config(Browser::Edge)).unwrap();
        assert_eq!(
            v,
            json!({ "mcpServers": { "playwright-edge": {
                "type": "stdio", "command": "npx", "args": ["@playwright/mcp@latest", "--browser", "msedge"],
            } } })
        );
        let v: Value = serde_json::from_str(&http_config(Browser::Chrome, 4242)).unwrap();
        assert_eq!(
            v,
            json!({ "mcpServers": { "playwright-chrome": { "type": "http", "url": "http://127.0.0.1:4242/mcp" } } })
        );
        assert_eq!(Browser::Chrome.server(), "playwright-chrome");
    }

    #[test]
    fn shared_server_listens_on_loopback() {
        let args = shared::args(Browser::Edge, 4242);
        assert_eq!(args, ["@playwright/mcp@latest", "--browser", "msedge", "--host", "127.0.0.1", "--port", "4242"]);
    }

    #[test]
    fn picks_round_trip() {
        for p in Pick::ALL {
            assert_eq!(Pick::from_code(p.code()), p);
        }
        assert_eq!(Pick::default(), Pick::Edge);
        assert_eq!(Pick::Off.label(), "Off");
        assert_eq!(Pick::Chrome.browser(), Some(Browser::Chrome));
    }
}
