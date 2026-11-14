//! The app's end of the control socket. Connections are served on threads of their own; what
//! they ask is queued for the app, which answers each [`Call`] when it can (a prompt that waits
//! for the turn answers much later).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use interprocess::local_socket::{GenericFilePath, ListenerOptions, Stream, ToFsName, prelude::*};
use serde_json::{Value, json};

use super::protocol::{Incoming, MAX_LINE, Request, answer, event_line, parse_incoming};

/// One request waiting for the app. `reply` takes the answer, once.
pub struct Call {
    pub request: Request,
    /// The session it says it comes from (a `knx` run inside one). Not proof: see `agent`.
    pub caller: Option<u64>,
    pub reply: Sender<Result<Value, String>>,
}

type Calls = Arc<Mutex<VecDeque<Call>>>;
type Subscribers = Arc<Mutex<Vec<Sender<String>>>>;

/// The socket being served. Dropping it takes its address file away.
pub struct Server {
    dir: PathBuf,
    calls: Calls,
    subscribers: Subscribers,
}

impl Server {
    /// Starts serving. `wake` is called, from another thread, whenever there is a call to
    /// [`take`](Self::take). `Err` (said in words) if another Kynex already serves the socket
    /// or it can't be made; the app then runs without it.
    pub fn start(wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        Self::start_in(&crate::paths::data_dir(), wake)
    }

    /// [`start`](Self::start) with the socket's files in `dir`.
    pub fn start_in(dir: &Path, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        if super::client::running_in(dir) {
            return Err("another Kynex is already serving the control socket".into());
        }
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        // The socket is made under the umask; the folder round it keeps others out meanwhile.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let address = address(dir);
        // A socket file a crashed Kynex left behind would stop the new one from binding.
        #[cfg(unix)]
        let _ = std::fs::remove_file(&address);
        let name = PathBuf::from(&address).to_fs_name::<GenericFilePath>().map_err(|e| e.to_string())?;
        let listener = ListenerOptions::new().name(name).create_sync().map_err(|e| e.to_string())?;
        // Only the user: the socket file is theirs alone.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&address, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        }
        std::fs::write(super::addr_file(dir), &address).map_err(|e| e.to_string())?;
        let calls: Calls = Arc::default();
        let subscribers: Subscribers = Arc::default();
        let (c, s) = (calls.clone(), subscribers.clone());
        std::thread::Builder::new()
            .name("control-accept".into())
            .spawn(move || {
                for conn in listener.incoming().flatten() {
                    let (c, s, wake) = (c.clone(), s.clone(), wake.clone());
                    let _ =
                        std::thread::Builder::new().name("control-conn".into()).spawn(move || serve(conn, c, s, wake));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { dir: dir.to_path_buf(), calls, subscribers })
    }

    /// The requests that came in since the last call.
    pub fn take(&self) -> Vec<Call> {
        self.calls.lock().map(|mut q| q.drain(..).collect()).unwrap_or_default()
    }

    /// Tells everyone listening to `events` about `name`; `data` is an object whose fields go
    /// into the line.
    pub fn broadcast(&self, name: &str, data: Value) {
        let line = event_line(name, data);
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.retain(|s| s.send(line.clone()).is_ok());
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(super::addr_file(&self.dir));
    }
}

/// Where this run listens: the socket file in the data dir, or on Windows a pipe with a name
/// nobody can guess without reading `control.addr`.
fn address(dir: &Path) -> String {
    if cfg!(windows) {
        use std::hash::{BuildHasher, Hasher};
        let random = std::collections::hash_map::RandomState::new().build_hasher().finish();
        format!(r"\\.\pipe\kynex-{random:016x}")
    } else {
        dir.join("control.sock").to_string_lossy().into_owned()
    }
}

/// What a read from a client found.
#[derive(Debug, PartialEq)]
enum Line {
    /// One line, at most [`MAX_LINE`] bytes.
    Text(String),
    /// The client left, or sent something that isn't text.
    Gone,
    /// It went on for more than [`MAX_LINE`] bytes without ending.
    TooLong,
}

fn read_line(reader: &mut impl BufRead) -> Line {
    let mut line = String::new();
    match reader.by_ref().take(MAX_LINE as u64 + 1).read_line(&mut line) {
        Ok(0) | Err(_) => Line::Gone,
        Ok(_) if line.len() > MAX_LINE => Line::TooLong,
        Ok(_) => Line::Text(line),
    }
}

fn serve(stream: Stream, calls: Calls, subscribers: Subscribers, wake: Arc<dyn Fn() + Send + Sync>) {
    let mut send = &stream;
    let mut reader = BufReader::new(&stream);
    let mut write = move |line: &str| writeln!(send, "{line}").and_then(|()| send.flush()).is_ok();
    loop {
        let line = match read_line(&mut reader) {
            Line::Text(line) => line,
            Line::Gone => return,
            Line::TooLong => {
                let e = format!("a request can't be longer than {} KiB", MAX_LINE / 1024);
                write(&answer(&None, Err(e)));
                return;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let Incoming { id, caller, request } = match parse_incoming(&line) {
            Ok(r) => r,
            Err(e) => {
                if !write(&answer(&None, Err(e))) {
                    return;
                }
                continue;
            }
        };
        if request == Request::Events {
            let (tx, rx) = channel::<String>();
            if let Ok(mut subs) = subscribers.lock() {
                subs.push(tx);
            }
            if !write(&answer(&id, Ok(json!({ "subscribed": true })))) {
                return;
            }
            // Until the client goes away: a line that can't be written ends it.
            for event in rx {
                if !write(&event) {
                    return;
                }
            }
            return;
        }
        let (reply, answered) = channel();
        if let Ok(mut q) = calls.lock() {
            q.push_back(Call { request, caller, reply });
        }
        wake();
        let result = answered.recv().unwrap_or_else(|_| Err("Kynex closed".to_string()));
        if !write(&answer(&id, result)) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A real socket in a scratch folder: a request goes in, the app side answers, and an
    /// event reaches a listener.
    #[test]
    fn a_client_talks_to_a_server_over_the_socket() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::mpsc::channel as chan;
        let dir = std::env::temp_dir().join(format!("kynex-control-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rang = Arc::new(AtomicUsize::new(0));
        let r = rang.clone();
        let server = Arc::new(
            Server::start_in(
                &dir,
                Arc::new(move || {
                    r.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .unwrap(),
        );
        assert!(super::super::client::running_in(&dir));
        assert!(Server::start_in(&dir, Arc::new(|| {})).is_err(), "one app serves it");

        // The app side: answers every call it finds.
        let app = server.clone();
        std::thread::spawn(move || {
            loop {
                for call in app.take() {
                    let _ = call.reply.send(Ok(json!({ "got": format!("{:?}", call.request) })));
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        });
        // A line that never ends gets an answer saying so.
        let mut long = Vec::new();
        let huge = format!("{}\n", "a".repeat(MAX_LINE + 5));
        let mut raw = interprocess::local_socket::Stream::connect(
            PathBuf::from(std::fs::read_to_string(dir.join("control.addr")).unwrap())
                .to_fs_name::<GenericFilePath>()
                .unwrap(),
        )
        .unwrap();
        let _ = raw.write_all(huge.as_bytes());
        let _ = BufReader::new(&raw).read_line(&mut String::new()).map(|_| long.push(1));
        assert_eq!(long.len(), 1, "the client was answered");

        let mut lines = Vec::new();
        super::super::client::call_in(&dir, &Request::PaneList, |l| lines.push(l.to_string())).unwrap();
        let v: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["got"], "PaneList");
        assert!(rang.load(Ordering::SeqCst) >= 1);

        // Events: the first line says subscribed, then what is broadcast.
        let (tx, rx) = chan();
        let d = dir.clone();
        std::thread::spawn(move || {
            let _ = super::super::client::call_in(&d, &Request::Events, |l| {
                let _ = tx.send(l.to_string());
            });
        });
        assert!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap().contains("subscribed"));
        server.broadcast("turn_end", json!({ "session": 4 }));
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), json!({ "event": "turn_end", "session": 4 }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An app that takes the connection but never answers: the client gives up.
    #[test]
    fn a_client_gives_up_on_a_stuck_app() {
        let dir = std::env::temp_dir().join(format!("kynex-control-stuck-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _server = Server::start_in(&dir, Arc::new(|| {})).unwrap();
        let started = std::time::Instant::now();
        let e = super::super::client::call_within(
            &dir,
            &Request::PaneList,
            Some(std::time::Duration::from_millis(200)),
            |_| {},
        )
        .unwrap_err();
        assert!(e.contains("didn't answer"), "{e}");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_is_read_up_to_its_end() {
        let mut r = Cursor::new(b"{\"cmd\":\"events\"}\nnext\n".to_vec());
        assert_eq!(read_line(&mut r), Line::Text("{\"cmd\":\"events\"}\n".into()));
        assert_eq!(read_line(&mut r), Line::Text("next\n".into()));
        assert_eq!(read_line(&mut r), Line::Gone);
    }

    #[test]
    fn a_line_that_goes_on_too_long_ends_the_connection() {
        let mut r = Cursor::new(vec![b'a'; MAX_LINE + 10]);
        assert_eq!(read_line(&mut r), Line::TooLong);
        let mut ok = Cursor::new([vec![b'a'; MAX_LINE - 1], b"\n".to_vec()].concat());
        assert!(matches!(read_line(&mut ok), Line::Text(_)));
    }

    #[test]
    fn text_that_isnt_utf8_ends_it_too() {
        let mut r = Cursor::new(vec![0xff, 0xfe, b'\n']);
        assert_eq!(read_line(&mut r), Line::Gone);
    }
}
