use super::super::*;

const BIG: Rect = Rect { x: 0.0, y: 0.0, w: 2000.0, h: 1000.0 };

fn old_json(tools: &str) -> String {
    format!(
        r#"{{"layout":{{"tabs":[{{"name":"1","root":{{"Split":{{"axis":"Row","ratio":0.2,"a":{{"Split":{{"axis":"Column","ratio":0.35,"a":{{"Pane":8}},"b":{{"Pane":9}}}}}},"b":{{"Pane":1}}}}}},"focus":8,"zoomed":null}}],"active":0}},"sessions":{{"1":"abc"}},"tools":{tools},"seeded":true}}"#
    )
}

fn restore(json: &str) -> Tiling {
    let saved: SavedLayout = serde_json::from_str(json).expect("old layouts load");
    let mut next = 500;
    Tiling::restore(&saved, &mut next, |_| "claude".into()).expect("restores")
}

fn trees(t: &Tiling) -> usize {
    t.panes().into_iter().filter(|p| t.content(*p) == Some(Content::Tree)).count()
}

#[test]
fn both_old_panes_open_as_one_tree_in_the_sessions_place() {
    let t = restore(&old_json(r#"{"8":"Workspaces","9":"Sessions"}"#));
    assert_eq!(trees(&t), 1);
    assert_eq!(t.panes().len(), 2, "the agent and the tree");
    let tree = t.panes_of(Content::Tree)[0];
    let r = t.rects(BIG);
    assert!(r.pane(tree).unwrap().w < 0.25 * BIG.w);
    assert!(r.pane(tree).unwrap().h > 0.99 * BIG.h, "the full height");
    let agent = t.panes().into_iter().find(|p| *p != tree).unwrap();
    assert_eq!(t.content(agent), Some(Content::ClaudeChat));
    assert_eq!(t.agent(agent).unwrap().session_id.as_deref(), Some("abc"));
}

#[test]
fn legacy_timeline_is_dropped_and_legacy_agent_terminal_becomes_agent_cli() {
    let json = r#"{
        "layout":{"tabs":[{"name":"1","root":{"Split":{"axis":"Row","ratio":0.5,"a":{"Pane":1},"b":{"Pane":2}}},"focus":1,"zoomed":null}],"active":0},
        "sessions":{"1":"s1"},
        "tools":{"2":{"Terminal":{"agent":"codex","resume":"cx-9"}}},
        "seeded":true
    }"#;
    let t = restore(json);
    assert_eq!(t.agent_panes().len(), 2);
    let codex = t.agent_panes().into_iter().find(|(_, a)| a.provider.as_deref() == Some("codex")).unwrap().1;
    assert_eq!(codex.session_id.as_deref(), Some("cx-9"));
}

#[test]
fn legacy_multi_tab_layout_keeps_all_agents_and_deduplicates_tree() {
    let json = r#"{
        "layout":{
            "tabs":[
                {"name":"1","root":{"Split":{"axis":"Row","ratio":0.2,"a":{"Pane":1},"b":{"Pane":2}}},"focus":2,"zoomed":null},
                {"name":"2","root":{"Split":{"axis":"Row","ratio":0.2,"a":{"Pane":3},"b":{"Pane":4}}},"focus":4,"zoomed":null}
            ],
            "active":0
        },
        "agents":{
            "2":{"provider":"claude","session_id":"s1"},
            "4":{"provider":"agy","session_id":"s2"}
        },
        "tools":{"1":"Tree","3":"Tree"},
        "seeded":true
    }"#;
    let t = restore(json);
    assert_eq!(trees(&t), 1);
    assert_eq!(t.agent_panes().len(), 2);
    assert!(t.pane_of_session("s1").is_some());
    assert!(t.pane_of_session("s2").is_some());
}

#[test]
fn the_toggle_adds_and_removes_the_one_pane() {
    let mut t = Tiling::new(1, AgentCliPane::pending());
    assert!(!t.has_side_panes());
    assert!(t.add_side_panes(|| 9, 0.2, BIG));
    assert!(t.has_side_panes());
    assert_eq!(t.panes_of(Content::Tree), [9]);
    assert!(!t.add_side_panes(|| 10, 0.2, BIG), "one per workspace");
    assert_eq!(t.remove_side_panes(), 1);
    assert!(!t.has_side_panes());
    assert_eq!(t.panes(), [1]);
}
