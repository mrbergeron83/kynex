//! The client end: what `knx` subcommands use to ask the running app, and how a second
//! `knx <folder>` finds the first instance.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use interprocess::local_socket::{GenericFilePath, Stream, ToFsName, prelude::*};

use super::Request;
use super::protocol::request_line;

const NOT_RUNNING: &str = "Kynex isn't running";

/// Connects to the running app's control socket. Blocks only as long as connecting does.
fn open(dir: &Path) -> Result<Stream, String> {
    let addr = std::fs::read_to_string(super::addr_file(dir)).map_err(|_| NOT_RUNNING.to_string())?;
    let name = PathBuf::from(addr.trim()).to_fs_name::<GenericFilePath>().map_err(|e| e.to_string())?;
    Stream::connect(name).map_err(|_| NOT_RUNNING.to_string())
}

/// Whether an app is answering on the control socket.
pub fn running() -> bool {
    running_in(&crate::paths::data_dir())
}

/// [`running`] for the app whose data dir is `dir`.
pub fn running_in(dir: &Path) -> bool {
    open(dir).is_ok()
}

/// Sends `request` to the running app and hands each line it answers with to `on_line`: one
/// answer, or for [`Request::Events`] every event until the app closes the connection.
/// Blocks until then (a `wait` can take as long as a turn). `Err` if no app is running or the
/// connection broke before an answer.
pub fn call(request: &Request, on_line: impl FnMut(&str)) -> Result<(), String> {
    call_in(&crate::paths::data_dir(), request, on_line)
}

/// [`call`] to the app whose data dir is `dir`.
pub fn call_in(dir: &Path, request: &Request, mut on_line: impl FnMut(&str)) -> Result<(), String> {
    let stream = open(dir)?;
    let mut send = &stream;
    writeln!(send, "{}", request_line(1, request)).map_err(|e| e.to_string())?;
    send.flush().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    let mut got = false;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                got = true;
                on_line(line.trim_end());
                if *request != Request::Events {
                    break;
                }
            }
        }
    }
    if got { Ok(()) } else { Err("Kynex closed the connection without answering".into()) }
}

/// The `knx` command line (without the program name): sends it to the running app and
/// prints the answers, one JSON line each. The exit code: 0, or 1 with the reason on stderr
/// (also when the app answered with an error).
pub fn run(args: &[String]) -> i32 {
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

/// Windows: the app has no console of its own (it is a window), so a `knx` subcommand takes
/// the terminal it was started from to print to. Does nothing elsewhere.
pub fn attach_console() {
    #[cfg(windows)]
    {
        use std::os::windows::io::IntoRawHandle;
        use windows_sys::Win32::System::Console::{
            ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
        };
        // SAFETY: plain Win32 calls with valid arguments; the handles opened here live as long as the process.
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                return;
            }
            for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
                // Left alone when the output is redirected to a file or a pipe.
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

/// Asks the running app, if there is one, to open `folder` as a project. Whether one did.
pub fn open_folder(folder: &str) -> bool {
    let request = Request::ContextOpen { path: folder.to_string() };
    let mut answered_ok = false;
    let sent = call(&request, |line| {
        answered_ok = serde_json::from_str::<serde_json::Value>(line).is_ok_and(|v| v["ok"] == true);
    });
    sent.is_ok() && answered_ok
}
