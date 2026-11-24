//! Round trips with the real `claude` CLI; `#[ignore]`d, as they use quota.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;

use super::super::*;

/// Talks to the real `claude` CLI, so it uses your login and a little quota.
/// Run with `cargo test -- --ignored`.
#[test]
#[ignore]
fn chat_round_trip() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    s.model = "haiku".into();
    s.send("Reply with exactly: pong".into(), Vec::new());
    let start = Instant::now();
    while s.busy && start.elapsed() < Duration::from_secs(90) {
        s.poll();
        std::thread::sleep(Duration::from_millis(50));
    }
    let text: String =
        s.items.iter().filter_map(|i| if let Item::Assistant(t) = i { Some(t.as_str()) } else { None }).collect();
    assert!(text.to_lowercase().contains("pong"), "got: {text:?}");
    assert!(s.items.iter().any(|i| matches!(i, Item::Result { is_error: false, .. })));
    assert!(s.session_id.is_some());
    // The process answered the `initialize` it starts with, before the reply.
    let init = s.take_init().expect("no initialize response");
    assert!(init.commands.iter().any(|c| c.name == "compact"));
    assert!(!init.models.is_empty());
}

/// Two turns, the second writing a file; rewinding to it restores the file
/// and the conversation, and the next message carries on from the first
/// turn. Run with `cargo test -- --ignored`.
#[test]
#[ignore]
fn rewind_live() {
    let root = std::env::temp_dir().join(format!("kynex-rewind-{}", std::process::id()));
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    unsafe { std::env::set_var("KYNEX_DATA_DIR", root.join("data")) };
    let mut s = Session::new(Kind::Code, project.clone());
    s.model = "haiku".into();
    s.mode = project::Mode::AcceptEdits;
    let turn = |s: &mut Session, text: &str| {
        s.send(text.into(), Vec::new());
        let start = Instant::now();
        while s.busy && start.elapsed() < Duration::from_secs(120) {
            s.poll();
            std::thread::sleep(Duration::from_millis(50));
        }
        s.items
            .iter()
            .rev()
            .find_map(|i| if let Item::Assistant(t) = i { Some(t.clone()) } else { None })
            .unwrap_or_default()
    };
    turn(&mut s, "Remember the word APPLE. Reply only OK.");
    turn(&mut s, "Also remember BANANA, and write BANANA to fruit.txt with the Write tool. Reply only OK.");
    assert!(project.join("fruit.txt").is_file(), "the second turn wrote no file");

    let id = s.session_id.clone().unwrap();
    let points = crate::claude::rewind::points(&project, &id).unwrap();
    assert_eq!(points.len(), 2);
    let second = &points[0];
    assert!(second.checkpoint && second.changed == ["fruit.txt"], "unexpected rewind checkpoint metadata");
    s.kill();
    crate::claude::rewind::restore_files(&project, &id, second, s.provider()).unwrap();
    assert!(!project.join("fruit.txt").exists(), "the file is still there");
    s.rewind_to(second);
    assert_eq!(s.items.iter().filter(|i| matches!(i, Item::User { .. })).count(), 1);

    let reply = turn(&mut s, "List every word I asked you to remember, nothing else.");
    assert!(reply.contains("APPLE") && !reply.contains("BANANA"), "got: {reply:?}");
    assert_eq!(s.session_id.as_deref(), Some(id.as_str()));
    let _ = std::fs::remove_dir_all(&root);
}

fn live_session(name: &str) -> (PathBuf, Session) {
    let root = std::env::temp_dir().join(format!("kynex-{name}-{}", std::process::id()));
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    unsafe { std::env::set_var("KYNEX_DATA_DIR", root.join("data")) };
    let mut s = Session::new(Kind::Code, project);
    s.model = "haiku".into();
    s.mode = project::Mode::Bypass;
    s.bypass = true;
    (root, s)
}

fn wait_idle(s: &mut Session) {
    let start = Instant::now();
    while (s.busy || s.loading()) && start.elapsed() < Duration::from_secs(120) {
        // As the app does: the log read in the background comes in here.
        s.take_history();
        s.poll();
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn last_reply(s: &Session) -> String {
    s.items
        .iter()
        .rev()
        .find_map(|i| if let Item::Assistant(t) = i { Some(t.clone()) } else { None })
        .unwrap_or_default()
}

/// A message written mid-turn is read in that turn: one result, and the reply
/// follows it. Run with `cargo test -- --ignored`.
#[test]
#[ignore]
fn steer_live() {
    let (root, mut s) = live_session("steer");
    s.send(
        "Use the Bash tool to run: sleep 6 && echo first. Then run: sleep 6 && echo second. Then reply in one line."
            .into(),
        Vec::new(),
    );
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(4) {
        s.poll();
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(s.can_steer(), "the turn ended before the steer");
    s.steer("Change of plan: end your final reply with the word BANANA.".into(), Vec::new());
    wait_idle(&mut s);
    let results = s.items.iter().filter(|i| matches!(i, Item::Result { .. })).count();
    let reply = last_reply(&s);
    assert!(reply.contains("BANANA"), "got: {reply:?}");
    assert!(results <= 2 && !s.busy, "{results} results");
    s.kill();
    let _ = std::fs::remove_dir_all(&root);
}

/// `/fork` continues a session as a new one, and the original's log is left as it was.
#[test]
#[ignore]
fn fork_live() {
    let (root, mut s) = live_session("fork");
    s.send("Remember the word APPLE. Reply only OK.".into(), Vec::new());
    wait_idle(&mut s);
    let orig = s.session_id.clone().unwrap();
    let cwd = s.cwd.clone();
    s.kill();
    let mut f = Session::fork(Kind::Code, cwd.clone(), orig.clone(), Arc::new(|| {}));
    f.model = "haiku".into();
    wait_idle(&mut f);
    f.send("Which word did I ask you to remember? Reply with just it.".into(), Vec::new());
    wait_idle(&mut f);
    assert!(last_reply(&f).contains("APPLE"), "got: {:?}", last_reply(&f));
    let new = f.session_id.clone().unwrap();
    assert_ne!(new, orig);
    assert_eq!(f.resume_id(), Some(new.as_str()));
    f.kill();
    let users = |id: &str| {
        let log = std::fs::read_to_string(crate::sessions::find_claude_transcript(&cwd, id).unwrap_or_default())
            .unwrap_or_default();
        log.lines().filter(|l| l.contains(r#""type":"user""#) && !l.contains("tool_result")).count()
    };
    assert_eq!(users(&orig), 1, "the original grew");
    assert!(users(&new) >= 2);
    let _ = std::fs::remove_dir_all(&root);
}

/// A project session starts in Auto (no prompt for a harmless mkdir), then a
/// live switch to Manual makes the next one ask. Run with `cargo test -- --ignored`.
#[test]
#[ignore]
fn permission_modes_live() {
    let root = std::env::temp_dir().join(format!("kynex-modes-{}", std::process::id()));
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    // Keep the project's Kynex settings out of the real data dir.
    unsafe { std::env::set_var("KYNEX_DATA_DIR", root.join("data")) };

    let mut s = Session::new(Kind::Code, project.clone());
    assert_eq!(s.mode, project::Mode::Auto);
    s.model = "haiku".into();
    let run = |s: &mut Session, dir: &str| {
        s.send(format!("Run this exact command with the Bash tool: mkdir {dir} . Then reply OK."), Vec::new());
        let start = Instant::now();
        while s.busy && start.elapsed() < Duration::from_secs(120) {
            s.poll();
            if let Some(i) =
                s.items.iter().position(|i| matches!(i, Item::Permission { state: PermState::Pending, .. }))
            {
                s.answer_permission(i, Answer::Deny(String::new()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let asked = |s: &Session| s.items.iter().filter(|i| matches!(i, Item::Permission { .. })).count();

    run(&mut s, "made1");
    assert!(project.join("made1").is_dir(), "auto mode should run mkdir");
    assert_eq!(asked(&s), 0, "auto mode shouldn't ask");

    s.set_mode(project::Mode::Manual);
    run(&mut s, "made2");
    assert_eq!(s.mode, project::Mode::Manual);
    assert!(asked(&s) > 0, "manual mode should ask");
    assert!(!project.join("made2").exists(), "denied mkdir shouldn't run");

    s.kill();
    let _ = std::fs::remove_dir_all(&root);
}

/// A comment on a yes reaches Claude within the same turn. Run with `cargo test -- --ignored`.
#[test]
#[ignore]
fn comment_on_a_yes_live() {
    let root = std::env::temp_dir().join(format!("kynex-comment-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    unsafe { std::env::set_var("KYNEX_DATA_DIR", root.join("data")) };
    let mut s = Session::new(Kind::Code, root.clone());
    s.model = "haiku".into();
    s.mode = project::Mode::Manual;
    s.send(
        "Run this exact command with the Bash tool: mkdir made . Then reply with one short sentence.".into(),
        Vec::new(),
    );
    let start = Instant::now();
    while s.busy && start.elapsed() < Duration::from_secs(120) {
        s.poll();
        if let Some(i) = s.items.iter().position(|i| matches!(i, Item::Permission { state: PermState::Pending, .. })) {
            s.answer_permission_with(i, Answer::Once, "Also end your reply with the word BANANA.");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let results = s.items.iter().filter(|i| matches!(i, Item::Result { .. })).count();
    let text: String =
        s.items.iter().filter_map(|i| if let Item::Assistant(t) = i { Some(t.as_str()) } else { None }).collect();
    assert!(root.join("made").is_dir(), "allowed mkdir should run");
    assert_eq!(results, 1, "the comment should join the running turn");
    assert!(text.contains("BANANA"), "got: {text:?}");
    s.kill();
    let _ = std::fs::remove_dir_all(&root);
}

/// An `AskUserQuestion` answered from Kynex reaches Claude. Run with `cargo test -- --ignored`.
#[test]
#[ignore]
fn answers_questions() {
    let mut s = Session::new(Kind::Code, std::env::temp_dir());
    s.model = "haiku".into();
    s.send(
        "Use the AskUserQuestion tool to ask me which color I like, with options Red and Blue. \
         Then reply with exactly the color I picked, in capitals."
            .into(),
        Vec::new(),
    );
    let start = Instant::now();
    while s.busy && start.elapsed() < Duration::from_secs(120) {
        s.poll();
        let pending = s.items.iter().enumerate().find_map(|(i, it)| match it {
            Item::Permission { tool, input, state: PermState::Pending, .. } if tool == "AskUserQuestion" => {
                Some((i, input["questions"][0]["question"].as_str().unwrap_or("").to_string()))
            }
            _ => None,
        });
        if let Some((i, question)) = pending {
            let mut answers = serde_json::Map::new();
            answers.insert(question, json!("Blue"));
            s.answer_permission(i, Answer::Questions(answers));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let text: String =
        s.items.iter().filter_map(|i| if let Item::Assistant(t) = i { Some(t.as_str()) } else { None }).collect();
    assert!(s.items.iter().any(|i| matches!(i, Item::Permission { state: PermState::Allowed(_), .. })), "never asked");
    assert!(text.contains("BLUE"), "got: {text:?}");
    s.kill();
}
