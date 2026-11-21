//! The client end: what `knx` subcommands and `knx hook` use to talk to the running app.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use interprocess::local_socket::{GenericFilePath, Stream, ToFsName, prelude::*};

use super::Request;
use super::agent;
use super::protocol::request_line_from;

const NOT_RUNNING: &str = "Kynex isn't running";

fn address_in(dir: &Path) -> Result<String, String> {
    let addr = std::fs::read_to_string(super::addr_file(dir)).map_err(|_| NOT_RUNNING.to_string())?;
    Ok(addr.trim().to_string())
}

fn connect(addr: &str) -> Result<Stream, String> {
    let name = PathBuf::from(addr).to_fs_name::<GenericFilePath>().map_err(|e| e.to_string())?;
    Stream::connect(name).map_err(|_| NOT_RUNNING.to_string())
}

fn open(dir: &Path) -> Result<Stream, String> {
    connect(&address_in(dir)?)
}

pub fn running() -> bool {
    running_in(&crate::paths::data_dir())
}

pub fn running_in(dir: &Path) -> bool {
    open(dir).is_ok()
}

pub fn call(request: &Request, on_line: impl FnMut(&str)) -> Result<(), String> {
    let addr = match agent::handed_address() {
        Some(addr) => addr,
        None => address_in(&crate::paths::data_dir())?,
    };
    send(&addr, request, agent::caller(), patience(request), on_line)
}

fn patience(request: &Request) -> Option<Duration> {
    match request {
        Request::Hook { .. } => Some(Duration::from_millis(250)),
        Request::Events
        | Request::PaneWait { .. }
        | Request::PaneRun { wait: true, .. }
        | Request::PanePrompt { wait: true, .. } => None,
        _ => Some(Duration::from_secs(15)),
    }
}

pub fn call_in(dir: &Path, request: &Request, on_line: impl FnMut(&str)) -> Result<(), String> {
    call_within(dir, request, patience(request), on_line)
}

pub fn call_within(
    dir: &Path,
    request: &Request,
    first: Option<Duration>,
    on_line: impl FnMut(&str),
) -> Result<(), String> {
    send(&address_in(dir)?, request, None, first, on_line)
}

fn send(
    addr: &str,
    request: &Request,
    caller: Option<u64>,
    first: Option<Duration>,
    mut on_line: impl FnMut(&str),
) -> Result<(), String> {
    let stream = connect(addr)?;
    let mut send = &stream;
    writeln!(send, "{}", request_line_from(1, request, caller)).map_err(|e| e.to_string())?;
    send.flush().map_err(|e| e.to_string())?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::Builder::new()
        .name("control-client".into())
        .spawn(move || {
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        if tx.send(line.trim_end().to_string()).is_err() {
                            return;
                        }
                    }
                }
            }
        })
        .map_err(|e| e.to_string())?;
    let mut got = false;
    loop {
        let next = match (got, first) {
            (false, Some(wait)) => rx.recv_timeout(wait).map_err(|e| match e {
                std::sync::mpsc::RecvTimeoutError::Timeout => Some(()),
                std::sync::mpsc::RecvTimeoutError::Disconnected => None,
            }),
            _ => rx.recv().map_err(|_| None),
        };
        match next {
            Ok(line) => {
                got = true;
                on_line(&line);
                if *request != Request::Events {
                    break;
                }
            }
            Err(Some(())) => return Err("Kynex didn't answer (its window may be stuck)".into()),
            Err(None) => break,
        }
    }
    if got { Ok(()) } else { Err("Kynex closed the connection without answering".into()) }
}

/// Fast, fire-and-forget handler for `knx hook <provider> [event]`: reads JSON from stdin,
/// posts `Request::Hook` to the running Kynex with a 250ms budget, prints any required JSON
/// stdout response (e.g. `{}` for `agy`), and ALWAYS returns exit code 0 so hooks never break CLIs.
fn run_hook(provider: &str, event: Option<&str>) -> i32 {
    let mut buf = String::new();
    let _ = std::io::stdin().take(256 * 1024).read_to_string(&mut buf);
    let report = crate::hooks::parse_payload(provider, event, &buf);
    if let Some(pane) = agent::hook_target() {
        let req = Request::Hook {
            pane,
            provider: provider.to_string(),
            state: report.state,
            session_id: report.session_id,
            detail: report.detail,
        };
        let _ = call(&req, |_| {});
    }
    if let Some(reply) = crate::hooks::stdout_response(provider) {
        println!("{reply}");
    }
    0
}

pub fn run(args: &[String]) -> i32 {
    if let Some((provider, event)) = super::args::wants_hook(args) {
        return run_hook(provider, event);
    }
    if super::args::wants_schema(args) {
        println!("{}", serde_json::to_string_pretty(&super::schema::schema()).unwrap_or_default());
        return 0;
    }
    let request = match super::args::parse(args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let mut failed = false;
    let sent = call(&request, |line| {
        println!("{line}");
        failed |= serde_json::from_str::<serde_json::Value>(line).is_ok_and(|v| v["ok"] == false);
    });
    match sent {
        Ok(()) => i32::from(failed),
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

pub fn attach_console() {
    #[cfg(windows)]
    {
        use std::os::windows::io::IntoRawHandle;
        use windows_sys::Win32::System::Console::{
            ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
        };
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                return;
            }
            for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
                let current = GetStdHandle(which);
                if (current.is_null() || current as isize == -1)
                    && let Ok(console) = std::fs::OpenOptions::new().write(true).open("CONOUT$")
                {
                    SetStdHandle(which, console.into_raw_handle() as _);
                }
            }
        }
    }
}

pub fn open_folder(folder: &str) -> bool {
    let request = Request::ProjectOpen { path: folder.to_string() };
    let mut answered_ok = false;
    let sent = call(&request, |line| {
        answered_ok = serde_json::from_str::<serde_json::Value>(line).is_ok_and(|v| v["ok"] == true);
    });
    sent.is_ok() && answered_ok
}
