//! A session fed the CLI's lines: turns, tools, prompts, usage.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::*;

#[test]
fn a_steer_the_cli_answers_in_a_turn_of_its_own_keeps_the_session_busy() {
    let mut s = Session::new(Kind::Code, std::env::temp_dir());
    // Not running: a steer is an ordinary message.
    assert!(!s.can_steer());
    // Taken in during the turn: one result, and the session is idle after it.
    s.busy = true;
    s.steered = true;
    s.handle(json!({ "type": "result", "duration_ms": 1, "total_cost_usd": 0.0 }));
    assert!(!s.busy && s.late_steer);
    // Too late for that turn: the CLI starts another for it, which is busy too.
    s.handle(json!({ "type": "stream_event", "parent_tool_use_id": null,
        "event": { "type": "message_start" } }));
    assert!(s.busy && !s.late_steer && !s.finished);
    s.handle(json!({ "type": "result", "duration_ms": 1, "total_cost_usd": 0.0 }));
    assert!(!s.busy && !s.late_steer);
    // Without a steer, output after a result (a background task's turn) doesn't.
    s.handle(json!({ "type": "stream_event", "parent_tool_use_id": null,
        "event": { "type": "message_start" } }));
    assert!(!s.busy);
}

#[test]
fn result_recaps_the_turn() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    s.items.push(Item::User { text: "old".into(), images: Vec::new() });
    s.handle(json!({ "type": "assistant", "message": { "content": [
        { "type": "tool_use", "name": "Edit", "input": { "file_path": "C:/x/old.rs" } }
    ] } }));
    s.items.push(Item::User { text: "go".into(), images: Vec::new() });
    s.handle(json!({ "type": "assistant", "message": { "content": [
        { "type": "tool_use", "name": "Read", "input": { "file_path": "C:/x/a.rs" } },
        { "type": "tool_use", "name": "Edit", "input": { "file_path": "C:/x/a.rs" } },
        { "type": "tool_use", "name": "Write", "input": { "file_path": "/y/b.md" } },
        { "type": "tool_use", "name": "Edit", "input": { "file_path": "C:/x/a.rs" } }
    ] } }));
    s.busy = true;
    s.handle(json!({ "type": "result", "duration_ms": 5000, "total_cost_usd": 0.1 }));
    assert!(!s.busy && s.finished);
    let Some(Item::Result { tools, files, .. }) = s.items.last() else { panic!("no result") };
    assert_eq!(*tools, 4);
    assert_eq!(files, &["C:/x/a.rs", "/y/b.md"]);
}

#[test]
fn cost_of_a_restarted_process_adds_to_the_earlier_one() {
    let mut s = Session::new(Kind::Code, std::env::temp_dir());
    s.handle(json!({ "type": "result", "total_cost_usd": 0.5 }));
    s.handle(json!({ "type": "result", "total_cost_usd": 0.75 }));
    assert!((s.total_cost - 0.75).abs() < 1e-9);
    // A new process: its counter starts again at 0.
    s.carried_cost += std::mem::take(&mut s.run_cost);
    s.handle(json!({ "type": "result", "total_cost_usd": 0.25 }));
    assert!((s.total_cost - 1.0).abs() < 1e-9);
}

#[test]
fn restarting_an_mcp_server_reports_back() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    s.mcp_requests = vec!["r1".into(), "r2".into()];
    s.handle(json!({ "type": "control_response",
        "response": { "request_id": "r1", "subtype": "error", "error": "Server not found: x" } }));
    assert!(matches!(s.items.last(), Some(Item::Error(e)) if e.ends_with("Server not found: x")));
    s.handle(json!({ "type": "control_response", "response": { "request_id": "r2", "subtype": "success",
        "response": { "mcpServers": [{ "name": "pw", "status": "connected" }] } } }));
    assert_eq!(s.mcp.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["pw"]);
    assert!(s.mcp_requests.is_empty());
}

#[test]
fn the_initialize_answer_ends_starting() {
    use crate::claude::pulse::Activity;
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    s.items.push(Item::User { text: "hi".into(), images: Vec::new() });
    s.busy = true;
    assert_eq!(s.pulse().map(|p| p.activity), Some(Activity::Starting));
    s.handle(json!({ "type": "control_response", "response": { "request_id": INIT_ID, "response": {} } }));
    assert_eq!(s.pulse().map(|p| p.activity), Some(Activity::Waiting));
    s.handle(json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "hello" }] } }));
    assert_eq!(s.pulse().map(|p| p.activity), Some(Activity::Working));
}

#[test]
fn idle_sessions_fall_asleep_and_working_ones_dont() {
    let after = Duration::from_secs(60);
    let later = Instant::now() + after * 2;
    // A stand-in process: this test binary, listing its tests.
    let running = || {
        let mut s = Session::new(Kind::Chat, std::env::temp_dir());
        let exe = std::env::current_exe().expect("test binary");
        s.child = Some(Command::new(exe).arg("--list").stdout(Stdio::null()).spawn().expect("spawn"));
        s
    };

    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    assert!(!s.sleep_if_idle(after, later), "no process to stop");

    let mut s = running();
    assert!(!s.sleep_if_idle(after, Instant::now()), "not idle long enough");
    s.finished = true;
    assert!(s.sleep_if_idle(after, later));
    assert!(!s.is_running());
    assert!(s.finished, "a reply you haven't seen stays unseen");

    let mut s = running();
    s.warm();
    assert!(!s.sleep_if_idle(after, Instant::now() + after / 2), "warming counts as activity");

    for keep in [
        |s: &mut Session| s.busy = true,
        |s: &mut Session| s.browser = true,
        |s: &mut Session| s.queue.push("later".into(), Vec::new()),
    ] {
        let mut s = running();
        keep(&mut s);
        assert!(!s.sleep_if_idle(after, later));
        s.kill();
    }
}

#[test]
fn calls_are_timed_from_start_to_result() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    // From a log (no process): no timing.
    s.handle(json!({ "type": "assistant", "message": { "content": [
        { "type": "tool_use", "id": "a", "name": "Bash", "input": { "command": "make", "timeout": 120000 } }
    ] } }));
    let Some(Item::ToolUse { timing, input, .. }) = s.items.last() else { panic!("no call") };
    assert_eq!((*timing, timeout_ms(input)), (Timing::default(), Some(120_000)));

    // Watched live: it runs until its result comes.
    s.items.push(Item::User { text: "go".into(), images: Vec::new() });
    let started = Some(Instant::now() - Duration::from_secs(5));
    let input = r#"{ "command": "make", "timeout": 120000 }"#.to_string();
    s.items.push(Item::ToolUse {
        id: "b".into(),
        name: "Bash".into(),
        summary: String::new(),
        input,
        timing: Timing { started, took: None },
    });
    s.busy = true;
    assert!(matches!(s.items.last(), Some(Item::ToolUse { timing, .. })
        if timing.is_running() && timing.elapsed().unwrap().as_secs() >= 5));
    s.handle(json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "b", "content": "ok" }] } }));
    let Some(Item::ToolUse { timing, .. }) = s.items.iter().rev().find(|i| matches!(i, Item::ToolUse { .. })) else {
        panic!()
    };
    let took = timing.took.expect("stopped");
    assert!(took.as_secs() >= 5 && timing.elapsed() == Some(took));
}

#[test]
fn thinking_is_timed_while_streamed_and_kept_once_done() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    let stream = |e: Value| json!({ "type": "stream_event", "event": e });
    s.handle(stream(json!({ "type": "content_block_start", "content_block": { "type": "thinking" } })));
    s.handle(stream(
        json!({ "type": "content_block_delta", "delta": { "type": "thinking_delta", "thinking": "hm, " } }),
    ));
    s.handle(stream(
        json!({ "type": "content_block_delta", "delta": { "type": "thinking_delta", "thinking": "yes" } }),
    ));
    let Some(Item::Thinking(t)) = s.items.last() else { panic!("no thinking") };
    assert!(t.is_live() && t.label().starts_with("Thinking…"));
    s.handle(stream(json!({ "type": "content_block_stop" })));
    // The whole message comes after; it doesn't add the block again.
    s.handle(json!({ "type": "assistant", "message": { "content": [{ "type": "thinking", "thinking": "hm, yes" }] } }));
    assert_eq!(s.items.len(), 1);
    let Some(Item::Thinking(t)) = s.items.last() else { panic!("no thinking") };
    assert!(!t.is_live() && t.label().starts_with("Thought for "), "{}", t.label());
    assert_eq!(t.text, "hm, yes");
}

#[test]
fn thinking_from_a_log_or_cut_short() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    s.handle(json!({ "type": "assistant", "message": { "content": [{ "type": "thinking", "thinking": "plan" }] } }));
    let Some(Item::Thinking(t)) = s.items.last() else { panic!("no thinking") };
    assert_eq!((t.label().as_str(), t.text.as_str()), ("Thought", "plan"));
    // Stopped mid-thought: the turn's end stops the clock.
    s.handle(json!({ "type": "stream_event", "event": { "type": "content_block_start", "content_block": { "type": "thinking" } } }));
    s.busy = true;
    s.handle(json!({ "type": "result", "duration_ms": 10 }));
    assert!(s.items.iter().all(|i| !matches!(i, Item::Thinking(t) if t.is_live())));
}

#[test]
fn the_pulse_covers_the_gaps_nothing_else_shows() {
    use crate::claude::pulse::Activity;
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    let stream = |e: Value| json!({ "type": "stream_event", "event": e });
    let activity = |s: &Session| s.pulse().map(|p| p.activity);
    assert_eq!(activity(&s), None, "idle");
    s.items.push(Item::User { text: "hi".into(), images: Vec::new() });
    s.busy = true;
    assert_eq!(activity(&s), Some(Activity::Starting));
    s.process_up();
    assert_eq!(activity(&s), Some(Activity::Waiting));
    // A thinking block shows itself; the gap after it doesn't.
    s.handle(stream(json!({ "type": "content_block_start", "content_block": { "type": "thinking" } })));
    assert_eq!(activity(&s), None);
    s.handle(stream(json!({ "type": "content_block_stop" })));
    assert_eq!(activity(&s), Some(Activity::Working));
    // A tool call shows from its first byte, and while it runs.
    s.handle(stream(json!({ "type": "content_block_start",
        "content_block": { "type": "tool_use", "id": "t1", "name": "Write", "input": {} } })));
    assert_eq!(activity(&s), None);
    s.handle(stream(json!({ "type": "content_block_stop" })));
    assert_eq!(activity(&s), None, "running");
    s.handle(json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }] } }));
    assert_eq!(activity(&s), Some(Activity::Working));
    // Redacted thinking has nothing to show: the pulse stays.
    s.handle(stream(json!({ "type": "content_block_start", "content_block": { "type": "redacted_thinking" } })));
    assert_eq!(activity(&s), Some(Activity::Working));
    s.handle(json!({ "type": "result", "duration_ms": 10 }));
    assert_eq!(activity(&s), None, "the turn ended");
}

#[test]
fn a_tool_call_shows_while_its_input_streams() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    let stream = |e: Value| json!({ "type": "stream_event", "event": e });
    let delta = |p: &str| {
        stream(json!({ "type": "content_block_delta", "delta": { "type": "input_json_delta", "partial_json": p } }))
    };
    s.items.push(Item::User { text: "write it".into(), images: Vec::new() });
    s.handle(stream(json!({ "type": "content_block_start",
        "content_block": { "type": "tool_use", "id": "w", "name": "Write", "input": {} } })));
    let call = |s: &Session| match s.items.last() {
        Some(Item::ToolUse { name, summary, timing, .. }) => (name.clone(), summary.clone(), timing.is_running()),
        _ => panic!("no call"),
    };
    assert_eq!(call(&s), ("Write".into(), String::new(), true));
    let before = s.revision();
    s.handle(delta(r#"{"file_path": "src/ma"#));
    assert_eq!(call(&s).1, "");
    s.handle(delta(r#"in.rs", "content": "fn main() {"#));
    assert_eq!(call(&s).1, "src/main.rs");
    assert!(s.revision() > before, "the run's summary is laid out again");
    s.handle(delta(r#"}"}"#));
    s.handle(stream(json!({ "type": "content_block_stop" })));
    // The whole message fills the same call in instead of adding one.
    s.handle(json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "id": "w", "name": "Write",
        "input": { "file_path": "src/main.rs", "content": "fn main() {}" } }] } }));
    assert_eq!(s.items.iter().filter(|i| matches!(i, Item::ToolUse { .. })).count(), 1);
    let Some(Item::ToolUse { input, .. }) = s.items.last() else { panic!() };
    assert!(input.contains("fn main() {}"));
}

#[test]
fn wants_you_after_a_turn_or_on_a_prompt() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    assert!(!s.wants_you());
    s.busy = true;
    s.handle(json!({ "type": "result", "duration_ms": 10, "total_cost_usd": 0.0 }));
    assert!(s.wants_you());
    s.finished = false;
    s.items.push(Item::Permission {
        request_id: "r".into(),
        tool: "Bash".into(),
        input: Value::Null,
        suggestions: Value::Null,
        description: String::new(),
        why: String::new(),
        state: PermState::Pending,
    });
    assert!(s.wants_you());
}

#[test]
fn prompts_are_cancelled_when_their_request_or_process_goes() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    let ask = |id: &str| {
        json!({ "type": "control_request", "request_id": id,
            "request": { "subtype": "can_use_tool", "tool_name": "Bash", "input": {} } })
    };
    s.handle(ask("a"));
    s.handle(ask("b"));
    s.handle(json!({ "type": "control_cancel_request", "request_id": "a" }));
    let states = |s: &Session| -> Vec<PermState> {
        s.items
            .iter()
            .filter_map(|i| match i {
                Item::Permission { state, .. } => Some(state.clone()),
                _ => None,
            })
            .collect()
    };
    assert!(states(&s) == [PermState::Denied("cancelled".into()), PermState::Pending]);
    assert!(s.waiting());
    s.kill();
    assert!(!s.waiting());
    // An answer after the fact changes nothing.
    assert!(s.answer_permission(1, Answer::Once).is_empty());
    assert!(states(&s)[1] == PermState::Denied("cancelled".into()));
}

#[test]
fn a_comment_rides_along_with_a_yes() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    let ask = |id: &str| {
        json!({ "type": "control_request", "request_id": id,
            "request": { "subtype": "can_use_tool", "tool_name": "Bash", "input": {} } })
    };
    s.handle(ask("a"));
    s.handle(ask("b"));
    s.answer_permission_with(0, Answer::Once, "  use the staging config ");
    let Some(Item::Permission { state, .. }) = s.items.first() else { panic!() };
    assert!(*state == PermState::Allowed("allowed · “use the staging config”".into()));
    // Blank comments add nothing.
    s.answer_permission_with(1, Answer::Once, "  ");
    let Some(Item::Permission { state, .. }) = s.items.get(1) else { panic!() };
    assert!(*state == PermState::Allowed("allowed".into()));
    // A second answer changes nothing.
    s.answer_permission_with(0, Answer::Once, "again");
    let Some(Item::Permission { state, .. }) = s.items.first() else { panic!() };
    assert!(*state == PermState::Allowed("allowed · “use the staging config”".into()));
}

#[test]
fn mcp_servers_and_server_wide_allow() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    s.handle(json!({ "type": "system", "subtype": "init",
        "mcp_servers": [{ "name": "playwright-edge", "status": "failed" }] }));
    assert_eq!(s.mcp[0].status, crate::claude::mcp::Status::Failed);
    s.handle(json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "id": "t",
        "name": "mcp__playwright-edge__browser_click", "input": { "element": "Sign in" } }] } }));
    let Some(Item::ToolUse { summary, .. }) = s.items.last() else { panic!("no call") };
    assert_eq!(summary, "\u{201c}Sign in\u{201d}");
    s.handle(json!({ "type": "control_request", "request_id": "r", "request": {
        "subtype": "can_use_tool", "tool_name": "mcp__playwright-edge__browser_click", "input": {} } }));
    s.answer_permission(1, Answer::Server);
    let Some(Item::Permission { state, .. }) = s.items.last() else { panic!("no prompt") };
    assert!(*state == PermState::Allowed("all Edge actions allowed this session".into()));
}

#[test]
fn settings_go_live_or_mark_the_process_stale() {
    let mut s = Session::new(Kind::Code, std::env::temp_dir());
    let perms = project::Permissions::default();
    let claude = project::ClaudeSettings { effort: "high".into(), ..Default::default() };
    // Not running: the next start reads them.
    s.apply_settings(&perms, &claude);
    assert!(!s.stale && s.applying.is_empty());
    s.launched = Some(project::Launched::new(&perms, &project::ClaudeSettings::default()));
    s.apply_settings(&perms, &claude);
    assert_eq!(s.applying, ["kynex-1"]);
    // A CLI that refuses the patch starts again on the next idle moment.
    s.handle(json!({ "type": "control_response", "response": { "subtype": "error", "request_id": "kynex-1",
        "error": "unknown subtype" } }));
    assert!(s.stale && s.applying.is_empty());
    s.kill();
    assert!(!s.stale && s.launched.is_none());
    // Effort back to the default can't go live.
    s.launched = Some(project::Launched::new(&perms, &claude));
    s.apply_settings(&perms, &project::ClaudeSettings::default());
    assert!(s.stale);
}

#[test]
fn keeps_what_the_process_reports_at_start() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    // Other control responses (an interrupt's, say) aren't the handshake.
    s.handle(json!({ "type": "control_response", "response": { "request_id": "kynex-1", "response": {} } }));
    assert!(s.take_init().is_none());
    s.handle(json!({ "type": "control_response", "response": { "subtype": "success", "request_id": INIT_ID,
        "response": {
            "commands": [{ "name": "compact", "description": "Free up context" }],
            "models": [{ "value": "opus", "displayName": "Opus", "supportedEffortLevels": ["high"] }],
        } } }));
    let init = s.take_init().unwrap();
    assert_eq!(init.commands[0].name, "compact");
    assert_eq!(init.models[0].value, "opus");
    assert_eq!(init.models[0].efforts, ["high"]);
    assert!(s.take_init().is_none(), "taken once");
    assert!(s.items.is_empty(), "nothing in the transcript");
}

#[test]
fn keeps_the_last_usage_limits_until_taken() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    let event = |used: f64| {
        json!({ "type": "rate_limit_event", "rate_limit_info": { "unifiedWindows": {
            "five_hour": { "utilization": used, "resetsAt": 1, "status": "allowed" } } } })
    };
    s.handle(event(0.1));
    s.handle(event(0.2));
    assert_eq!(s.take_limits().unwrap().five_hour.unwrap().used, 0.2);
    assert!(s.take_limits().is_none(), "taken once");
    assert!(s.items.is_empty(), "nothing in the transcript");
}

#[test]
fn context_use_follows_main_thread_replies() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    let reply = |parent: Value, input: u64| {
        json!({ "type": "assistant", "parent_tool_use_id": parent, "message": {
            "model": "test-model-ctx", "content": [],
            "usage": { "input_tokens": input, "cache_creation_input_tokens": 1000, "cache_read_input_tokens": 9000 }
        } })
    };
    s.handle(reply(Value::Null, 0));
    assert_eq!(s.context, ContextUse { tokens: 10_000, window: None });
    // A subagent's reply doesn't move the meter.
    s.handle(reply(json!("toolu_1"), 500_000));
    assert_eq!(s.context.tokens, 10_000);
    s.handle(json!({ "type": "result", "modelUsage": { "test-model-ctx": { "contextWindow": 200_000 } } }));
    assert_eq!(s.context.window, Some(200_000));
    assert_eq!(s.context.fraction(), Some(0.05));
    // Another session on the same model knows the window from its first reply.
    let mut other = Session::new(Kind::Chat, std::env::temp_dir());
    other.handle(reply(Value::Null, 10_000));
    assert_eq!(other.context, ContextUse { tokens: 20_000, window: Some(200_000) });
    other.handle(json!({ "type": "system", "subtype": "compact_boundary",
        "compact_metadata": { "pre_tokens": 20_000, "post_tokens": 3_000 } }));
    assert_eq!(other.context.tokens, 3_000);
    // A synthetic reply leaves the meter alone; a compaction without a size blanks it.
    other.handle(json!({ "type": "assistant", "parent_tool_use_id": null, "message": {
        "model": "<synthetic>", "content": [], "usage": { "input_tokens": 0 } } }));
    assert_eq!(other.context, ContextUse { tokens: 3_000, window: Some(200_000) });
    other.handle(json!({ "type": "system", "subtype": "compact_boundary" }));
    assert_eq!(other.context.tokens, 0);
    // An unknown reply model takes the biggest window the turn reported.
    let mut third = Session::new(Kind::Chat, std::env::temp_dir());
    third.handle(json!({ "type": "result", "modelUsage": {
        "side-model-ctx": { "contextWindow": 100_000 }, "main-model-ctx[1m]": { "contextWindow": 1_000_000 } } }));
    assert_eq!(third.context.window, Some(1_000_000));
}

#[test]
fn a_background_task_ending_wants_you() {
    let mut s = Session::new(Kind::Code, std::env::temp_dir());
    s.handle(json!({ "type": "system", "subtype": "task_started", "task_id": "b1", "tool_use_id": "toolu_1",
        "description": "Watch CI", "task_type": "local_bash" }));
    s.handle(json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "toolu_1",
        "content": "Command running in background with ID: b1. Output is being written to: /tmp/b1.output. You will be notified when it completes." }] } }));
    assert!(s.has_tasks() && !s.wants_you());
    assert_eq!(s.tasks.running[0].output_file, Some(PathBuf::from("/tmp/b1.output")));
    s.handle(json!({ "type": "system", "subtype": "task_notification", "task_id": "b1", "tool_use_id": "toolu_1",
        "status": "completed", "output_file": "/tmp/b1.output", "summary": "Background command \"Watch CI\" completed (exit code 0)" }));
    assert!(!s.has_tasks() && s.wants_you());
    assert!(matches!(s.items.last(), Some(Item::Task(n)) if n.status == tasks::Status::Completed));
}

#[test]
fn stopping_a_task_waits_for_the_cli() {
    let mut s = Session::new(Kind::Code, std::env::temp_dir());
    s.handle(json!({ "type": "system", "subtype": "task_started", "task_id": "b1", "tool_use_id": "toolu_1",
        "description": "Watch CI", "task_type": "local_bash" }));
    // Refused: it runs on, and says why.
    s.stop_task("b1");
    assert!(s.tasks.running[0].stopping);
    s.handle(json!({ "type": "control_response", "response": { "subtype": "error", "request_id": "kynex-1",
        "error": "no such task" } }));
    assert!(!s.tasks.running[0].stopping);
    assert!(matches!(s.items.last(), Some(Item::Error(e)) if e.contains("no such task")));
    // Accepted: it stays, stopping, until the CLI says it stopped.
    s.stop_task("b1");
    s.stop_task("b1");
    s.handle(json!({ "type": "control_response", "response": { "subtype": "success", "request_id": "kynex-2" } }));
    assert!(s.tasks.running[0].stopping && s.stopping.is_empty());
    s.handle(json!({ "type": "system", "subtype": "task_notification", "task_id": "b1", "tool_use_id": "toolu_1",
        "status": "stopped", "summary": "Background command \"Watch CI\" was stopped" }));
    assert!(!s.has_tasks());
    assert!(matches!(s.items.last(), Some(Item::Task(n)) if n.status == tasks::Status::Stopped));
}

#[test]
fn a_prompt_carries_why_the_cli_asked() {
    let mut s = Session::new(Kind::Code, std::env::temp_dir());
    s.handle(json!({ "type": "control_request", "request_id": "r1", "request": {
        "subtype": "can_use_tool", "tool_name": "Bash", "input": { "command": "git push" },
        "decision_reason_type": "rule", "decision_reason_rule_source": "projectSettings" } }));
    s.handle(json!({ "type": "control_request", "request_id": "r2", "request": {
        "subtype": "can_use_tool", "tool_name": "Bash", "input": {} } }));
    let whys: Vec<_> = s
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Permission { why, .. } => Some(why.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(whys, ["An ask rule in this project's .claude/settings.json matches this.", ""]);
}
