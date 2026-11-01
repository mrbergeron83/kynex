//! The OpenRouter chat harness: Kynex run as `kynex --openrouter-chat`, in
//! place of `claude`, for the "OpenRouter chat" provider. It speaks the CLI's
//! stream-json on stdin / stdout, so a [`crate::session::Session`] drives it
//! like any `claude` process, and logs in Claude Code's format, so its sessions
//! list, reopen and rewind like the rest. Each message goes to OpenRouter's
//! chat completions API with the conversation so far, the reply streaming
//! back; no tools and no system prompt, so any model works and is itself.
//! HTTP goes through `curl`, like the rest of Kynex; the key reaches it on
//! stdin (`--config -`), never on a command line.

mod log;
pub mod stream;

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use serde_json::{Value, json};

use crate::provider::{DEFAULT_MAX_OUTPUT_TOKENS, DEFAULT_OPENROUTER_MODEL, OpenRouterSettings};

/// The argument that starts Kynex as the harness instead of the app.
pub const FLAG: &str = "--openrouter-chat";
/// The key, in the harness's environment.
const KEY_ENV: &str = "KYNEX_OPENROUTER_KEY";

/// The harness for a session in `cwd` on `settings` with `key`, continuing
/// session `resume` (from its entry `at`) if given: Kynex's own executable.
pub(crate) fn command(
    cwd: &Path,
    key: &str,
    settings: &OpenRouterSettings,
    resume: Option<&str>,
    at: Option<&str>,
) -> Result<Command, String> {
    let exe = std::env::current_exe().map_err(|e| format!("couldn't find Kynex to start the chat: {e}"))?;
    let mut cmd = Command::new(exe);
    cmd.current_dir(cwd).arg(FLAG).env(KEY_ENV, key.trim());
    let model = settings.model.trim();
    cmd.args(["--model", if model.is_empty() { DEFAULT_OPENROUTER_MODEL } else { model }]);
    let cap = if settings.max_output_tokens == 0 { DEFAULT_MAX_OUTPUT_TOKENS } else { settings.max_output_tokens };
    cmd.args(["--max-tokens", &cap.to_string()]);
    if let Some(id) = resume {
        cmd.args(["--resume", id]);
    }
    if let Some(at) = at {
        cmd.args(["--resume-session-at", at]);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    Ok(cmd)
}

/// The API messages in a session log's text, as a resumed chat sends them.
pub fn history(log: &str) -> Vec<Value> {
    log::messages(&crate::rewind::branch(log))
}

/// What the harness was started with.
#[derive(Debug, Default, PartialEq)]
struct Args {
    model: String,
    max_tokens: u32,
    resume: Option<String>,
    at: Option<String>,
}

impl Args {
    fn parse(args: impl IntoIterator<Item = String>) -> Args {
        let mut out =
            Args { model: DEFAULT_OPENROUTER_MODEL.into(), max_tokens: DEFAULT_MAX_OUTPUT_TOKENS, ..Args::default() };
        let mut args = args.into_iter();
        while let Some(a) = args.next() {
            let mut value = || args.next().unwrap_or_default();
            match a.as_str() {
                "--model" => out.model = value(),
                "--max-tokens" => out.max_tokens = value().parse().unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS),
                "--resume" => out.resume = Some(value()),
                "--resume-session-at" => out.at = Some(value()),
                _ => {}
            }
        }
        out
    }
}

/// Runs the harness if Kynex was started as one: call first thing in `main`,
/// and exit with the code it returns.
pub fn main_if_asked() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(FLAG) {
        return None;
    }
    Some(run(Args::parse(args)))
}

/// Everything the main loop hears of.
enum Msg {
    Line(Value),
    Eof,
    Done(Reply),
}

/// stdout, shared by the main loop and the turn streaming.
#[derive(Clone)]
struct Out(Arc<Mutex<Box<dyn Write + Send>>>);

impl Out {
    fn send(&self, v: Value) {
        if let Ok(mut out) = self.0.lock() {
            let _ = writeln!(out, "{v}").and_then(|_| out.flush());
        }
    }
}

/// The request in flight, so an interrupt can stop it.
#[derive(Default)]
struct Stop {
    curl: Mutex<Option<Child>>,
    asked: AtomicBool,
}

impl Stop {
    fn now(&self) {
        self.asked.store(true, Ordering::SeqCst);
        if let Ok(mut curl) = self.curl.lock()
            && let Some(c) = curl.as_mut()
        {
            let _ = c.kill();
        }
    }
}

fn run(args: Args) -> i32 {
    let Some(key) = std::env::var(KEY_ENV).ok().filter(|k| !k.trim().is_empty()) else {
        eprintln!("No OpenRouter key is saved. Add one in Settings → Providers.");
        return 2;
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let (mut log, mut history) = log::Log::open(&cwd, args.resume.as_deref(), args.at.as_deref());
    let out = Out(Arc::new(Mutex::new(Box::new(std::io::stdout()))));
    let stop = Arc::new(Stop::default());
    let (tx, rx) = channel();
    let input = tx.clone();
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                let _ = input.send(Msg::Line(v));
            }
        }
        let _ = input.send(Msg::Eof);
    });

    let mut pending: VecDeque<Value> = VecDeque::new();
    let mut busy = false;
    let mut said_init = false;
    let mut total_cost = 0.0;
    let start = |content: Value, log: &mut log::Log, history: &mut Vec<Value>, said_init: &mut bool| {
        if let Err(e) = log.user(&content) {
            eprintln!("{e}");
        }
        if let Some(c) = stream::user_content(&content) {
            history.push(json!({ "role": "user", "content": c }));
        }
        if !*said_init {
            *said_init = true;
            out.send(json!({ "type": "system", "subtype": "init", "session_id": log.session_id, "model": args.model,
                             "tools": [], "mcp_servers": [] }));
        }
        stop.asked.store(false, Ordering::SeqCst);
        let turn = Turn {
            key: key.clone(),
            model: args.model.clone(),
            max_tokens: args.max_tokens,
            messages: history.clone(),
            session_id: log.session_id.clone(),
            out: out.clone(),
            stop: stop.clone(),
        };
        let done = tx.clone();
        thread::spawn(move || {
            let _ = done.send(Msg::Done(turn.run()));
        });
    };

    for msg in rx {
        match msg {
            Msg::Line(v) => match v["type"].as_str().unwrap_or("") {
                "user" => {
                    let content = v["message"]["content"].clone();
                    if busy {
                        pending.push_back(content);
                    } else {
                        busy = true;
                        start(content, &mut log, &mut history, &mut said_init);
                    }
                }
                "control_request" => {
                    let request = &v["request"];
                    let id = v["request_id"].clone();
                    match request["subtype"].as_str() {
                        // Nothing to report: no slash commands of its own, and its
                        // models are the user's OpenRouter models.
                        Some("initialize") => continue,
                        Some("interrupt") => {
                            pending.clear();
                            stop.now();
                        }
                        _ => {}
                    }
                    out.send(json!({ "type": "control_response",
                                     "response": { "subtype": "success", "request_id": id, "response": {} } }));
                }
                _ => {}
            },
            Msg::Done(reply) => {
                total_cost += reply.usage.cost;
                if let Err(e) = reply.log(&mut log, &args.model) {
                    eprintln!("{e}");
                }
                if !reply.text.is_empty() {
                    history.push(json!({ "role": "assistant", "content": reply.text }));
                }
                out.send(reply.result(&log.session_id, &args.model, total_cost));
                match pending.pop_front() {
                    Some(next) => start(next, &mut log, &mut history, &mut said_init),
                    None => busy = false,
                }
            }
            Msg::Eof => {
                stop.now();
                break;
            }
        }
    }
    0
}

/// One request and its streamed reply.
struct Turn {
    key: String,
    model: String,
    max_tokens: u32,
    messages: Vec<Value>,
    session_id: String,
    out: Out,
    stop: Arc<Stop>,
}

/// What a turn came to.
#[derive(Debug, Default)]
struct Reply {
    id: String,
    model: Option<String>,
    text: String,
    reasoning: String,
    usage: stream::Usage,
    error: Option<String>,
    interrupted: bool,
    took_ms: u64,
}

/// The block being streamed out.
#[derive(Clone, Copy, PartialEq)]
enum Block {
    None,
    Thinking,
    Text,
}

impl Turn {
    fn run(self) -> Reply {
        let began = Instant::now();
        let mut reply = Reply { id: format!("msg_{}", log::new_id().replace('-', "")), ..Reply::default() };
        let mut block = Block::None;
        let mut index = 0;
        match self.request() {
            Err(e) => reply.error = Some(e),
            Ok(stdout) => {
                let mut status = None;
                let mut other = String::new();
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    match stream::event(&line) {
                        stream::Event::Data(d) => {
                            if let Some(id) = d.id {
                                reply.id = id;
                            }
                            reply.model = d.model.or(reply.model.take());
                            if !d.reasoning.is_empty() {
                                self.switch(&mut block, Block::Thinking, &mut index, &reply);
                                self.delta(index, json!({ "type": "thinking_delta", "thinking": d.reasoning }));
                                reply.reasoning += &d.reasoning;
                            }
                            if !d.text.is_empty() {
                                self.switch(&mut block, Block::Text, &mut index, &reply);
                                self.delta(index, json!({ "type": "text_delta", "text": d.text }));
                                reply.text += &d.text;
                            }
                            if let Some(u) = d.usage {
                                reply.usage = u;
                            }
                            if d.error.is_some() {
                                reply.error = d.error;
                            }
                        }
                        stream::Event::Status(s) => status = Some(s),
                        stream::Event::Other(l) => {
                            other.push_str(&l);
                            other.push('\n');
                        }
                        stream::Event::Done | stream::Event::Nothing => {}
                    }
                }
                let (ended, curl_said) = self.finish_curl();
                reply.interrupted = self.stop.asked.load(Ordering::SeqCst);
                let failed =
                    status.is_some_and(|s| s != 200) || (status.is_none() && !ended.is_some_and(|e| e.success()));
                if reply.error.is_none() && !reply.interrupted && failed {
                    reply.error = Some(stream::failure(status, &(other + &curl_said)));
                }
            }
        }
        self.switch(&mut block, Block::None, &mut index, &reply);
        // A failure shows as the CLI shows one: a reply that says so.
        if let Some(e) = &reply.error {
            self.out.send(json!({ "type": "assistant", "session_id": self.session_id, "parent_tool_use_id": null,
                "message": { "id": reply.id, "role": "assistant", "model": "<synthetic>",
                             "content": [{ "type": "text", "text": e }] } }));
        }
        reply.took_ms = began.elapsed().as_millis() as u64;
        reply
    }

    /// Starts curl on this turn's request; its stdout streams the reply.
    fn request(&self) -> Result<std::process::ChildStdout, String> {
        let body = stream::body(&self.model, self.max_tokens, &self.messages);
        let config = crate::openrouter::curl_config(&self.key, stream::URL, Some(&body))
            .ok_or("The OpenRouter key has a character curl can't take. Set it again in Settings → Providers.")?;
        let write_out = format!("\n{}%{{http_code}}\n", stream::STATUS_MARK);
        let mut cmd = crate::cli::command("curl");
        cmd.args(["-sS", "-N", "--connect-timeout", "30", "--speed-limit", "1", "--speed-time", "300"])
            .args(["--config", "-", "-w", &write_out])
            .stdin(Stdio::piped());
        let mut curl = cmd.spawn().map_err(|e| format!("failed to start `curl`: {e}"))?;
        if let Some(mut stdin) = curl.stdin.take() {
            // Dropped at the end of this block, so curl sees the end of its config.
            stdin.write_all(config.as_bytes()).map_err(|e| format!("curl: {e}"))?;
        }
        let stdout = curl.stdout.take().ok_or("curl has no output")?;
        // Kept where an interrupt finds it; the turn takes it back once the reply ends.
        let mut slot = self.stop.curl.lock().map_err(|_| "the chat harness broke")?;
        // Stopped before curl even started: it doesn't get to run.
        if self.stop.asked.load(Ordering::SeqCst) {
            let _ = curl.kill();
        }
        *slot = Some(curl);
        Ok(stdout)
    }

    /// Takes curl back once its output has ended, and waits for it.
    fn finish_curl(&self) -> (Option<std::process::ExitStatus>, String) {
        let curl = self.stop.curl.lock().ok().and_then(|mut slot| slot.take());
        let Some(mut curl) = curl else { return (None, String::new()) };
        let mut err = String::new();
        if let Some(mut stderr) = curl.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut stderr, &mut err);
        }
        (curl.wait().ok(), err)
    }

    fn stream_event(&self, event: Value) {
        self.out.send(json!({ "type": "stream_event", "event": event, "session_id": self.session_id,
                              "parent_tool_use_id": null }));
    }

    fn delta(&self, index: usize, delta: Value) {
        self.stream_event(json!({ "type": "content_block_delta", "index": index, "delta": delta }));
    }

    /// Ends the block being streamed (sending it whole, as the CLI does) and
    /// starts `to`, unless it's already the one.
    fn switch(&self, block: &mut Block, to: Block, index: &mut usize, reply: &Reply) {
        if *block == to {
            return;
        }
        if *block != Block::None {
            self.stream_event(json!({ "type": "content_block_stop", "index": *index }));
            let (content, usage) = match block {
                Block::Thinking => (json!({ "type": "thinking", "thinking": reply.reasoning }), Value::Null),
                _ => (
                    json!({ "type": "text", "text": reply.text }),
                    json!({ "input_tokens": reply.usage.input, "output_tokens": reply.usage.output }),
                ),
            };
            let model = reply.model.clone().unwrap_or_else(|| self.model.clone());
            self.out.send(json!({ "type": "assistant", "session_id": self.session_id, "parent_tool_use_id": null,
                "message": { "id": reply.id, "role": "assistant", "model": model, "content": [content], "usage": usage } }));
            *index += 1;
        }
        let start = match to {
            Block::None => None,
            Block::Thinking => Some(json!({ "type": "thinking", "thinking": "" })),
            Block::Text => Some(json!({ "type": "text", "text": "" })),
        };
        if let Some(content_block) = start {
            self.stream_event(
                json!({ "type": "content_block_start", "index": *index, "content_block": content_block }),
            );
        }
        *block = to;
    }
}

impl Reply {
    /// Logs what came, even cut short; a failure alone isn't logged.
    fn log(&self, log: &mut log::Log, model: &str) -> Result<(), String> {
        let mut content = Vec::new();
        if !self.reasoning.is_empty() {
            content.push(json!({ "type": "thinking", "thinking": self.reasoning }));
        }
        if !self.text.is_empty() {
            content.push(json!({ "type": "text", "text": self.text }));
        }
        if content.is_empty() {
            return Ok(());
        }
        log.assistant(&self.id, self.model.as_deref().unwrap_or(model), content, &self.usage)
    }

    /// The turn's `result` line, as the CLI ends a turn.
    fn result(&self, session_id: &str, model: &str, total_cost: f64) -> Value {
        let model = self.model.as_deref().unwrap_or(model);
        let error = self.error.is_some();
        json!({
            "type": "result",
            "subtype": if error { "error_during_execution" } else { "success" },
            "is_error": error,
            "duration_ms": self.took_ms,
            "num_turns": 1,
            "result": self.text,
            "session_id": session_id,
            "total_cost_usd": total_cost,
            "usage": { "input_tokens": self.usage.input, "output_tokens": self.usage.output },
            "model_usage_since_last_result": { model: {
                "inputTokens": self.usage.input, "outputTokens": self.usage.output, "costUSD": self.usage.cost,
            } },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_parse_with_defaults() {
        let a = Args::parse(
            ["--model", "m/x", "--max-tokens", "100", "--resume", "s", "--resume-session-at", "u"].map(String::from),
        );
        assert_eq!(a, Args { model: "m/x".into(), max_tokens: 100, resume: Some("s".into()), at: Some("u".into()) });
        let d = Args::parse(Vec::new());
        assert_eq!(
            (d.model.as_str(), d.max_tokens, d.resume),
            (DEFAULT_OPENROUTER_MODEL, DEFAULT_MAX_OUTPUT_TOKENS, None)
        );
    }

    #[test]
    fn the_command_carries_the_key_in_its_environment_only() {
        let settings = OpenRouterSettings { model: "m/x".into(), max_output_tokens: 0 };
        let cmd = command(Path::new("."), " sk-or ", &settings, Some("s"), None).unwrap();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            [FLAG, "--model", "m/x", "--max-tokens", &DEFAULT_MAX_OUTPUT_TOKENS.to_string(), "--resume", "s"]
        );
        assert!(!args.iter().any(|a| a.contains("sk-or")));
        let key = cmd.get_envs().find(|(k, _)| *k == KEY_ENV).and_then(|(_, v)| v);
        assert_eq!(key.map(|v| v.to_string_lossy().into_owned()).as_deref(), Some("sk-or"));
    }

    /// Lines written to a buffer, for a turn's output.
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    #[ignore = "needs curl and the OpenRouter key in the keychain; spends a fraction of a cent"]
    fn a_reply_streams_from_openrouter_into_a_session() {
        let keychain = crate::secrets::Keychain::os().unwrap();
        let key = keychain.get(crate::secrets::Secret::OpenRouterKey).unwrap().expect("an OpenRouter key");
        let buffer = Buffer::default();
        let turn = Turn {
            key,
            model: std::env::var("KYNEX_TEST_MODEL")
                .unwrap_or_else(|_| "cognitivecomputations/dolphin-mistral-24b-venice-edition".into()),
            max_tokens: 50,
            messages: vec![json!({ "role": "user", "content": "Reply with the single word: pineapple" })],
            session_id: "s".into(),
            out: Out(Arc::new(Mutex::new(Box::new(buffer.clone())))),
            stop: Arc::new(Stop::default()),
        };
        let reply = turn.run();
        assert_eq!(reply.error, None);
        assert!(reply.text.to_lowercase().contains("pineapple"), "{reply:?}");
        // What it streamed, a session shows as it would the CLI's.
        let mut s = crate::session::Session::new(crate::session::Kind::Chat, std::env::temp_dir());
        let out = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        for line in out.lines() {
            s.handle(serde_json::from_str(line).unwrap());
        }
        s.handle(reply.result("s", "m", reply.usage.cost));
        let said: String = s
            .items
            .iter()
            .filter_map(|i| match i {
                crate::session::Item::Assistant(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(said, reply.text);
        eprintln!("{} said {:?}; usage {:?}", reply.model.unwrap_or_default(), reply.text, reply.usage);
    }

    /// One turn through a running harness: sends `text`, returns its lines up to the result.
    fn converse(child: &mut Child, lines: &mut impl Iterator<Item = String>, text: &str) -> Vec<Value> {
        let stdin = child.stdin.as_mut().unwrap();
        let msg =
            json!({ "type": "user", "message": { "role": "user", "content": [{ "type": "text", "text": text }] } });
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
        let mut got = Vec::new();
        for line in lines.by_ref() {
            let v: Value = serde_json::from_str(&line).unwrap();
            let end = v["type"] == "result";
            got.push(v);
            if end {
                break;
            }
        }
        got
    }

    #[test]
    #[ignore = "needs a built Kynex in KYNEX_BIN, curl and the OpenRouter key in the keychain; spends a fraction of a cent"]
    fn the_harness_converses_logs_and_resumes() {
        let bin = std::env::var("KYNEX_BIN").expect("KYNEX_BIN: a built kynex executable");
        let keychain = crate::secrets::Keychain::os().unwrap();
        let key = keychain.get(crate::secrets::Secret::OpenRouterKey).unwrap().expect("an OpenRouter key");
        let cwd = std::env::temp_dir().join(format!("kynex-chat-e2e-{}", log::new_id()));
        std::fs::create_dir_all(&cwd).unwrap();
        let start = |resume: Option<&str>| {
            let mut cmd = Command::new(&bin);
            cmd.current_dir(&cwd).arg(FLAG).env(KEY_ENV, &key);
            cmd.args(["--model", "cognitivecomputations/dolphin-mistral-24b-venice-edition", "--max-tokens", "40"]);
            if let Some(id) = resume {
                cmd.args(["--resume", id]);
            }
            let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
            let lines = BufReader::new(child.stdout.take().unwrap()).lines().map_while(Result::ok);
            (child, lines)
        };
        let (mut child, mut lines) = start(None);
        let first = converse(&mut child, &mut lines, "Remember the word pineapple. Reply with OK only.");
        let id = first[0]["session_id"].as_str().unwrap().to_string();
        assert_eq!(first.last().unwrap()["is_error"], false, "{first:?}");
        drop(child.stdin.take());
        child.wait().unwrap();

        let log = crate::session::find_transcript(&cwd, &id).expect("the conversation is logged");
        let listed = crate::session::list_sessions(&cwd);
        assert!(listed.iter().any(|s| s.id == id), "listed like any session");

        let (mut child, mut lines) = start(Some(&id));
        let second = converse(&mut child, &mut lines, "Which word did I ask you to remember? Reply with it only.");
        drop(child.stdin.take());
        child.wait().unwrap();
        let said = second.last().unwrap()["result"].as_str().unwrap_or("").to_lowercase();
        let _ = std::fs::remove_dir_all(log.parent().unwrap());
        let _ = std::fs::remove_dir_all(&cwd);
        assert!(said.contains("pineapple"), "{second:?}");
    }

    #[test]
    fn a_turn_ends_like_the_clis() {
        let reply = Reply {
            model: Some("m/x".into()),
            text: "hi".into(),
            usage: stream::Usage { input: 5, output: 2, cost: 0.001 },
            took_ms: 40,
            ..Reply::default()
        };
        let r = reply.result("s", "m/y", 0.003);
        assert_eq!((r["type"].as_str(), r["is_error"].as_bool()), (Some("result"), Some(false)));
        assert_eq!(r["total_cost_usd"], 0.003);
        assert_eq!(r["model_usage_since_last_result"]["m/x"]["costUSD"], 0.001);
        // A session takes it as any turn's end.
        let mut s = crate::session::Session::new(crate::session::Kind::Chat, std::env::temp_dir());
        s.handle(r);
        assert!(s.finished && !s.busy);
    }
}
