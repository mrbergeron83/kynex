use super::*;

const BIG: Rect = Rect { x: 0.0, y: 0.0, w: 2000.0, h: 1000.0 };

fn agent(p: &str, sid: Option<&str>) -> AgentCliPane {
    AgentCliPane::with_provider(p, None, sid.map(str::to_string))
}

fn three() -> Tiling {
    let mut t = Tiling::new(1, agent("claude", Some("s1")));
    assert!(t.split_agent(Axis::Row, 2, agent("grok", Some("s2")), BIG));
    assert!(t.split_agent(Axis::Column, 3, agent("agy", Some("s3")), BIG));
    t
}

#[test]
fn splits_and_finds_agent_panes() {
    let t = three();
    assert_eq!(t.focus(), Some(3));
    assert_eq!(t.agent_panes().len(), 3);
    assert_eq!(t.pane_of_session("s2"), Some(2));
    assert_eq!(t.pane_of_session("missing"), None);
}

#[test]
fn closing_the_focused_pane_keeps_the_last_agent_pane() {
    let mut t = three();
    assert_eq!(t.close_focused(), Some(3));
    assert_eq!(t.close_focused(), Some(2));
    assert_eq!(t.close_focused(), None);
    assert_eq!(t.panes(), [1]);
}

#[test]
fn save_and_restore_round_trips_agent_cli_panes_and_tools() {
    let mut t = three();
    assert!(t.add_terminal(4, 0.7, BIG));
    assert!(t.open_git(5, BIG));
    let saved = t.save();
    let json = serde_json::to_string(&saved).unwrap();
    let mut next = 100;
    let back = Tiling::restore(&serde_json::from_str(&json).unwrap(), &mut next, |_| "claude".into()).unwrap();
    assert_eq!(back.agent_panes().len(), 3);
    assert_eq!(back.terminals().len(), 1);
    assert_eq!(back.panes_of(Content::Git).len(), 1);
    assert!(back.pane_of_session("s1").is_some());
    assert!(back.pane_of_session("s2").is_some());
    assert!(back.pane_of_session("s3").is_some());
}

#[test]
fn drop_side_picks_edges_and_middle() {
    let r = Rect::new(0.0, 0.0, 100.0, 100.0);
    assert_eq!(drop_side(r, 10.0, 50.0), Some(Direction::Left));
    assert_eq!(drop_side(r, 90.0, 50.0), Some(Direction::Right));
    assert_eq!(drop_side(r, 50.0, 10.0), Some(Direction::Up));
    assert_eq!(drop_side(r, 50.0, 90.0), Some(Direction::Down));
    assert_eq!(drop_side(r, 50.0, 50.0), None);
}

#[test]
fn claude_providers_report_claude_chat_content() {
    let mut t = Tiling::new(1, agent("claude", Some("c1")));
    assert!(t.split_agent(Axis::Row, 2, agent("claude-openrouter", Some("c2")), BIG));
    assert!(t.split_agent(Axis::Column, 3, agent("agy", Some("a1")), BIG));
    assert!(t.split_agent(Axis::Column, 4, agent("grok", None), BIG));
    assert!(t.split_agent(Axis::Row, 5, AgentCliPane::pending(), BIG));

    assert_eq!(t.content(1), Some(Content::ClaudeChat));
    assert_eq!(t.content(2), Some(Content::ClaudeChat));
    assert_eq!(t.content(3), Some(Content::AgentCli));
    assert_eq!(t.content(4), Some(Content::AgentCli));
    assert_eq!(t.content(5), Some(Content::AgentCli));
    assert_eq!(t.panes_of(Content::ClaudeChat), [1, 2]);
    assert_eq!(t.panes_of(Content::AgentCli), [3, 4, 5]);
}

#[test]
fn save_and_restore_migrates_claude_providers_to_claude_chat() {
    let mut t = Tiling::new(1, agent("claude", Some("s-claude")));
    assert!(t.split_agent(Axis::Row, 2, agent("claude-openrouter", Some("s-or")), BIG));
    assert!(t.split_agent(Axis::Column, 3, agent("agy", Some("s-agy")), BIG));

    let saved = t.save();
    let json = serde_json::to_string(&saved).unwrap();
    let mut next = 200;
    let back = Tiling::restore(&serde_json::from_str(&json).unwrap(), &mut next, |_| "claude".into()).unwrap();

    let p_claude = back.pane_of_session("s-claude").unwrap();
    let p_or = back.pane_of_session("s-or").unwrap();
    let p_agy = back.pane_of_session("s-agy").unwrap();
    assert_eq!(back.content(p_claude), Some(Content::ClaudeChat));
    assert_eq!(back.content(p_or), Some(Content::ClaudeChat));
    assert_eq!(back.content(p_agy), Some(Content::AgentCli));

    // Legacy provider names ("anthropic", "openrouter") also normalize to ClaudeChat.
    let legacy = r#"{
        "layout":{"tabs":[{"name":"1","root":{"Split":{"axis":"Row","ratio":0.5,"a":{"Pane":1},"b":{"Pane":2}}},"focus":1,"zoomed":null}],"active":0},
        "agents":{"1":{"provider":"anthropic","session_id":"leg-1"},"2":{"provider":"openrouter","session_id":"leg-2"}},
        "seeded":true
    }"#;
    let mut next = 300;
    let back = Tiling::restore(&serde_json::from_str(legacy).unwrap(), &mut next, |_| "claude".into()).unwrap();
    assert_eq!(back.content(back.pane_of_session("leg-1").unwrap()), Some(Content::ClaudeChat));
    assert_eq!(back.content(back.pane_of_session("leg-2").unwrap()), Some(Content::ClaudeChat));
}

#[test]
fn last_agent_close_protection_applies_to_claude_chat() {
    let mut t = Tiling::new(1, agent("claude", Some("s1")));
    assert!(t.add_terminal(2, 0.7, BIG));

    // Closing the sole ClaudeChat pane is refused even when a terminal pane exists.
    assert_eq!(t.close_pane(1), None);
    assert_eq!(t.panes().len(), 2);

    // Splitting a second agent pane allows closing one and keeps protecting the last ClaudeChat pane.
    assert!(t.split_agent(Axis::Row, 3, agent("agy", None), BIG));
    assert_eq!(t.close_pane(3), Some(Content::AgentCli));
    assert_eq!(t.close_pane(1), None);
    assert_eq!(t.close_pane(2), Some(Content::Terminal));
    assert_eq!(t.panes(), [1]);
}
