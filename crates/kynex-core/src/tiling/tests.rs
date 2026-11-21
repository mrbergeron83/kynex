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
