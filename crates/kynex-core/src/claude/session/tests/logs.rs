//! Sessions read back from their logs, forks, and starting processes and providers.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::replay::{Loaded, Loader, Replayed};
use super::super::*;

#[test]
fn a_session_in_a_workspace_runs_under_its_projects_settings() {
    let mut s = Session::new(Kind::Code, PathBuf::from("/work/app-feat"));
    assert_eq!(s.settings_folder(), std::path::Path::new("/work/app-feat"), "its own folder by default");
    s.settings_dir = Some(PathBuf::from("/work/app"));
    assert_eq!(s.settings_folder(), std::path::Path::new("/work/app"));
    // Its transcripts stay under the folder it runs in.
    assert_eq!(s.cwd, PathBuf::from("/work/app-feat"));
}

#[test]
fn replay_leaves_out_what_the_cli_wrote() {
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    let note = "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n</task-notification>";
    s.replay_entry(json!({ "type": "user", "message": { "role": "user", "content": "check CI" } }));
    s.replay_entry(json!({ "type": "user", "promptSource": "system",
        "origin": { "kind": "task-notification" }, "message": { "role": "user", "content": note } }));
    // Older logs have no `promptSource` or `origin`.
    s.replay_entry(json!({ "type": "user", "message": { "role": "user", "content": note } }));
    let said: Vec<_> = s
        .items
        .iter()
        .filter_map(|i| if let Item::User { text, .. } = i { Some(text.as_str()) } else { None })
        .collect();
    assert_eq!(said, ["check CI"]);
    // Each notification is a row of its own instead.
    assert_eq!(s.items.iter().filter(|i| matches!(i, Item::Task(n) if n.task_id == "b1")).count(), 2);
}

#[test]
fn images_in_logs_and_tool_results_get_previews() {
    // Previews here say what they were made from.
    set_preview_maker(|mt, bytes| Some(Arc::new(format!("{mt} {}", bytes.len()))));
    let said = |p: &Preview| p.clone().downcast::<String>().unwrap().to_string();
    let image = |data: &str| json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": data } });
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    // "AAAA" is three bytes; "!" isn't base64, so that one is only counted.
    s.replay_user(&json!([image("AAAA"), image("!"), { "type": "text", "text": "see" }]));
    let Some(Item::User { text, images }) = s.items.last() else { panic!() };
    assert_eq!(text, "[1 image(s)] see");
    assert_eq!(images.iter().map(said).collect::<Vec<_>>(), ["image/png 3"]);

    s.handle(json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t",
        "content": [{ "type": "text", "text": "shot" }, image("AAAAAAAA")] }] } }));
    let Some(Item::ToolResult { content, images, .. }) = s.items.last() else { panic!() };
    assert_eq!(content, "shot");
    assert_eq!(images.iter().map(said).collect::<Vec<_>>(), ["image/png 6"]);
}

/// Entries as if read from no log.
fn unlogged<const N: usize>(entries: [Value; N]) -> [(Span, Value); N] {
    entries.map(|v| (Span { offset: 0, len: 0 }, v))
}

#[test]
fn logged_screenshots_stay_in_the_log_until_loaded() {
    use base64::Engine;
    let mut bytes = Vec::new();
    image::RgbaImage::new(4, 3).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let image = json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": data } });
    let sent = json!({ "type": "user", "uuid": "a", "message": { "role": "user", "content": [image, { "type": "text", "text": "see" }] } });
    let shot = json!({ "type": "user", "uuid": "b", "parentUuid": "a", "message": { "content": [
        { "type": "tool_result", "tool_use_id": "t", "content": "x" },
        { "type": "tool_result", "tool_use_id": "u", "content": [{ "type": "text", "text": "shot" }, image] }
    ] } });
    let first = format!("{sent}\n");
    let path = std::env::temp_dir().join(format!("kynex-logged-{}.jsonl", std::process::id()));
    std::fs::write(&path, format!("{first}{shot}\n")).unwrap();
    let spans =
        [Span { offset: 0, len: first.len() - 1 }, Span { offset: first.len() as u64, len: shot.to_string().len() }];
    let path = Arc::new(path);
    let r = Replayed::of(Some(&path), [(spans[0], sent), (spans[1], shot)]);
    let logged: Vec<&LoggedImage> = r
        .items
        .iter()
        .flat_map(|i| match i {
            Item::User { images, .. } | Item::ToolResult { images, .. } => images.as_slice(),
            _ => &[],
        })
        .map(|p| p.downcast_ref::<LoggedImage>().expect("kept as where it is"))
        .collect();
    let pointers: Vec<_> = logged.iter().map(|i| (i.span, i.pointer.as_str(), i.size)).collect();
    assert_eq!(
        pointers,
        [(spans[0], "/message/content/0", Some((4, 3))), (spans[1], "/message/content/1/content/1", Some((4, 3)))]
    );
    for i in logged {
        assert_eq!(i.load().unwrap(), bytes);
    }
    let _ = std::fs::remove_file(&*path);
}

#[test]
fn history_comes_newest_first_and_older_goes_in_front() {
    let text = |t: &str| json!({ "type": "assistant", "message": { "role": "assistant", "content": [{ "type": "text", "text": t }] } });
    let said = |s: &Session| {
        s.items
            .iter()
            .filter_map(|i| if let Item::Assistant(t) = i { Some(t.clone()) } else { None })
            .collect::<Vec<_>>()
    };
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    let (tx, rx) = channel();
    s.loader = Some(Loader { rx, shown: false });
    assert!(s.loading());
    assert_eq!(s.take_history(), None);
    // A message sent meanwhile waits for the history.
    s.send("later".into(), Vec::new());
    assert_eq!(s.queue.items().len(), 1);
    assert!(s.items.is_empty());
    s.queue.remove(0);

    tx.send(Loaded::Recent(Replayed::of(None, unlogged([text("c"), text("d")])))).unwrap();
    assert_eq!(s.take_history(), Some(2));
    assert!(!s.loading());
    tx.send(Loaded::Older(Replayed::of(None, unlogged([text("a"), text("b")])).items)).unwrap();
    drop(tx);
    assert_eq!(s.take_history(), Some(2));
    assert_eq!(said(&s), ["a", "b", "c", "d"]);
    assert!(s.loader.is_none());

    // No log at all: the session says so, and carries on.
    let mut s = Session::new(Kind::Chat, std::env::temp_dir());
    let (tx, rx) = channel::<Loaded>();
    s.loader = Some(Loader { rx, shown: false });
    drop(tx);
    assert_eq!(s.take_history(), Some(1));
    assert!(matches!(s.items[0], Item::Error(_)) && !s.loading());
}

#[test]
fn a_failed_start_shows_once_a_message_waits_and_warming_backs_off() {
    let finish = |s: &mut Session| {
        let until = Instant::now() + Duration::from_secs(30);
        while s.starting() && Instant::now() < until {
            s.poll();
            thread::sleep(Duration::from_millis(10));
        }
    };
    // No such folder: the process can't start there.
    let mut s = Session::new(Kind::Chat, std::env::temp_dir().join("kynex-no-such-folder").join("x"));
    s.warm();
    assert!(s.starting() && s.is_running(), "warming returns at once");
    finish(&mut s);
    assert!(!s.is_running() && s.items.is_empty(), "a warm-up's failure waits for a message");
    s.warm();
    assert!(!s.starting(), "and isn't retried on every keystroke");
    s.send("hi".into(), Vec::new());
    assert!(s.starting() && s.busy);
    finish(&mut s);
    assert!(matches!(s.items.last(), Some(Item::Error(_))), "a message's start reports its failure");
    assert!(!s.busy && !s.is_running());
}

#[test]
fn an_openrouter_model_switch_reaches_a_session_in_use() {
    use crate::provider::{Launch, OpenRouterSettings};
    let running = |provider: Launch| {
        let mut s = Session::new(Kind::Chat, std::env::temp_dir());
        s.set_provider(provider).unwrap();
        s.model = "old".into();
        let exe = std::env::current_exe().expect("test binary");
        s.child = Some(Command::new(exe).arg("--list").stdout(Stdio::null()).spawn().expect("spawn"));
        s.talked = true;
        s
    };
    let openrouter = Launch::OpenRouter { key: "k".into(), settings: OpenRouterSettings::default() };

    // Anthropic: the process in use keeps its model until it next starts.
    let mut s = running(Launch::Anthropic);
    s.use_model("new".into());
    assert!(s.child.is_some() && s.model == "old");
    s.kill();

    // OpenRouter, idle: it stops, and the next message resumes it on the new model.
    let mut s = running(openrouter.clone());
    s.use_model("new".into());
    assert!(s.child.is_none() && s.model == "new");

    // OpenRouter, mid-turn: left alone.
    let mut s = running(openrouter);
    s.busy = true;
    s.use_model("new".into());
    assert!(s.child.is_some() && s.model == "old");
    s.kill();
}

#[test]
fn a_warmed_up_id_is_not_resumed() {
    // A process warmed up early reported an id, then stopped (sleep, a
    // settings restart): no log was written, so the next one starts fresh.
    let mut s = Session::new(Kind::Code, std::env::temp_dir());
    s.handle(json!({ "type": "system", "subtype": "init", "session_id": "early" }));
    assert_eq!(s.resume_id(), None);
    // Once a message went out, the conversation is resumed.
    s.conversation = true;
    assert_eq!(s.resume_id(), Some("early"));
    // A reopened one always is.
    let resumed = Session::resume(Kind::Code, std::env::temp_dir(), "abc".into(), Arc::new(|| {}));
    assert_eq!(resumed.resume_id(), Some("abc"));
}

#[test]
fn a_fork_continues_its_original_until_it_has_its_own_id() {
    let mut s = Session::fork(Kind::Code, std::env::temp_dir(), "orig".into(), Arc::new(|| {}));
    // No id of its own, so it never stands in for the original.
    assert_eq!((s.session_id.as_deref(), s.forked_from()), (None, Some("orig")));
    assert_eq!(s.resume_id(), Some("orig"));
    // A warmed-up process reports the new id; until a message goes out the
    // next process still forks the original.
    s.handle(json!({ "type": "system", "subtype": "init", "session_id": "new" }));
    assert_eq!(s.resume_id(), Some("orig"));
    assert!(!s.is_new());
    // Once the message went out, the new session is the one resumed.
    s.fork_from = None;
    s.conversation = true;
    assert_eq!(s.resume_id(), Some("new"));
}

#[test]
fn only_a_new_session_switches_provider() {
    use crate::provider::{Launch, OpenRouterSettings, Provider};
    let openrouter = || Launch::OpenRouter { key: "k".into(), settings: OpenRouterSettings::default() };
    let mut s = Session::new(Kind::Code, std::env::temp_dir());
    assert_eq!(s.provider().provider(), Provider::Anthropic);
    s.set_provider(openrouter()).unwrap();
    s.set_provider(Launch::Anthropic).unwrap();

    // Warmed up early, with the id its process reported: still new; it starts again.
    let exe = std::env::current_exe().expect("test binary");
    s.child = Some(Command::new(exe).arg("--list").stdout(Stdio::null()).spawn().expect("spawn"));
    s.session_id = Some("early".into());
    assert!(s.is_new());
    s.set_provider(openrouter()).unwrap();
    assert!(s.child.is_none() && s.session_id.is_none());
    s.set_provider(Launch::Anthropic).unwrap();

    s.items.push(Item::User { text: "hi".into(), images: Vec::new() });
    let err = s.set_provider(openrouter()).unwrap_err();
    assert!(err.contains("new session"), "{err}");
    assert_eq!(s.provider(), &Launch::Anthropic);
    // Picking the one it has is fine.
    s.set_provider(Launch::Anthropic).unwrap();

    let mut resumed = Session::new(Kind::Code, std::env::temp_dir());
    resumed.session_id = Some("abc".into());
    resumed.reopened = true;
    assert!(resumed.set_provider(openrouter()).is_err());
    assert!(resumed.awaits_restore());
    // Reopened, it goes back to the provider it ran on, until it starts here:
    // also after a fallback to Anthropic (the key not loaded yet).
    resumed.restore_provider(Launch::Anthropic).unwrap();
    resumed.restore_provider(openrouter()).unwrap();
    assert_eq!(resumed.provider().provider(), Provider::OpenRouter);
    resumed.talked = true;
    assert!(!resumed.awaits_restore());
    assert!(resumed.restore_provider(Launch::Anthropic).is_err());
}

/// Replays every real log under `KYNEX_LOGS_DIR` (read only) as a session
/// opens, and reads back each screenshot it found. Run with
/// `cargo test --release -- --ignored --nocapture real_logged_images`.
#[test]
#[ignore = "reads your real Claude Code logs"]
fn real_logged_images_load() {
    use crate::claude::session_log::{Branch, read_span};
    let dir = PathBuf::from(std::env::var_os("KYNEX_LOGS_DIR").expect("KYNEX_LOGS_DIR"));
    let (mut images, mut bytes) = (0, 0);
    for e in std::fs::read_dir(dir).unwrap().filter_map(Result::ok) {
        let path = Arc::new(e.path());
        if path.extension().is_none_or(|x| x != "jsonl") {
            continue;
        }
        let mut file = std::fs::File::open(&*path).unwrap();
        let mut recent = Vec::new();
        let w = Branch { recent: 60 }
            .walk(&mut file, None, |v| {
                recent = v;
                true
            })
            .unwrap()
            .unwrap();
        let mut buf = Vec::new();
        let older: Vec<_> =
            w.older.iter().rev().filter_map(|s| Some((*s, read_span(&mut file, *s, &mut buf)?))).collect();
        let r = Replayed::of(Some(&path), older.into_iter().chain(recent.into_iter().rev()));
        for i in &r.items {
            let (Item::User { images: shown, .. } | Item::ToolResult { images: shown, .. }) = i else { continue };
            for p in shown {
                let logged = p.downcast_ref::<LoggedImage>().expect("logged");
                let got = logged.load().unwrap_or_else(|e| panic!("{e}: {logged:?}"));
                assert_eq!(
                    crate::claude::attach::dimensions(&got),
                    logged.size.or(crate::claude::attach::dimensions(&got))
                );
                images += 1;
                bytes += got.len();
            }
        }
    }
    println!("{images} screenshots read back, {} MB no longer kept in memory", bytes / 1_000_000);
}
