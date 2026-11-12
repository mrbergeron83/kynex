use super::super::*;

const BIG: Rect = Rect { x: 0.0, y: 0.0, w: 2000.0, h: 1000.0 };

/// What the release with the Sessions and Workspaces panes wrote: Workspaces (8) above
/// Sessions (9) on the left, the session (1) on the right, the focus on Workspaces.
fn old_json(tools: &str) -> String {
    format!(
        r#"{{"layout":{{"tabs":[{{"name":"1","root":{{"Split":{{"axis":"Row","ratio":0.2,"a":{{"Split":{{"axis":"Column","ratio":0.35,"a":{{"Pane":8}},"b":{{"Pane":9}}}}}},"b":{{"Pane":1}}}}}},"focus":8,"zoomed":null}}],"active":0}},"sessions":{{"1":"abc"}},"tools":{tools},"seeded":true}}"#
    )
}

fn restore(json: &str) -> Tiling {
    let saved: SavedLayout = serde_json::from_str(json).expect("old layouts load");
    let mut next = 500;
    Tiling::restore(&saved, &mut next, None, |_| Some(10)).expect("restores")
}

fn trees(t: &Tiling) -> usize {
    t.panes().into_iter().filter(|p| t.content(*p) == Some(Content::Tree)).count()
}

#[test]
fn both_old_panes_open_as_one_tree_in_the_sessions_place() {
    let t = restore(&old_json(r#"{"8":"Workspaces","9":"Sessions"}"#));
    assert_eq!(trees(&t), 1);
    assert_eq!(t.panes().len(), 2, "the session and the tree");
    let tree = t.panes_of(Content::Tree)[0];
    let r = t.rects(BIG);
    // The tree has the whole left column (20%): the room of the closed pane came back.
    assert!(r.pane(tree).unwrap().w < 0.25 * BIG.w);
    assert!(r.pane(tree).unwrap().h > 0.99 * BIG.h, "the full height");
    let session = t.panes().into_iter().find(|p| *p != tree).unwrap();
    assert_eq!(t.content(session), Some(Content::Session(10)));
    assert!((r.pane(session).unwrap().w - 0.8 * BIG.w).abs() < 20.0, "sizes kept");
    // The focus was on the dropped pane: it is on something that exists, a session.
    assert_eq!(t.focus(), Some(session));
}

#[test]
fn an_old_sessions_pane_alone_opens_as_the_tree() {
    let json = old_json(r#"{"9":"Sessions"}"#)
        .replace(r#""a":{"Split":{"axis":"Column","ratio":0.35,"a":{"Pane":8},"b":{"Pane":9}}}"#, r#""a":{"Pane":9}"#);
    let t = restore(&json.replace(r#""focus":8"#, r#""focus":9"#));
    assert_eq!(trees(&t), 1);
    assert_eq!(t.panes().len(), 2);
}

#[test]
fn an_old_workspaces_pane_alone_opens_as_the_tree() {
    let json = old_json(r#"{"8":"Workspaces"}"#)
        .replace(r#""a":{"Split":{"axis":"Column","ratio":0.35,"a":{"Pane":8},"b":{"Pane":9}}}"#, r#""a":{"Pane":8}"#);
    let t = restore(&json);
    assert_eq!(trees(&t), 1);
    assert_eq!(t.panes().len(), 2);
    assert_eq!(t.content(t.focus().unwrap()), Some(Content::Session(10)));
}

#[test]
fn a_layout_with_neither_gets_none() {
    let json = old_json("{}")
        .replace(r#""a":{"Split":{"axis":"Column","ratio":0.35,"a":{"Pane":8},"b":{"Pane":9}}}"#, r#""a":{"Pane":9}"#);
    let t = restore(&json.replace(r#""tools":{}"#, r#""tools":{"9":"Git"}"#).replace(r#""focus":8"#, r#""focus":9"#));
    assert_eq!(trees(&t), 0);
    assert_eq!(t.tab_panes_of(Content::Git).len(), 1);
}

#[test]
fn each_tab_keeps_its_own_tree() {
    let mut t = Tiling::new(1, 10);
    assert!(t.new_tab(2, 20));
    let mut next = 100;
    t.seed_side_panes(
        || {
            next += 1;
            next
        },
        0.2,
        BIG,
    );
    let saved = t.save(|k| Some(k.to_string()));
    let text = serde_json::to_string(&saved).unwrap();
    let mut n = 500;
    let mut back =
        Tiling::restore(&serde_json::from_str(&text).unwrap(), &mut n, None, |id| id.and_then(|i| i.parse().ok()))
            .unwrap();
    for i in 0..2 {
        assert!(back.select_tab(i) && back.has_side_panes(), "tab {i}");
    }
}

#[test]
fn the_toggle_adds_and_removes_the_one_pane() {
    let mut t = Tiling::new(1, 10);
    assert!(!t.has_side_panes());
    assert!(t.add_side_panes(|| 9, 0.2, BIG));
    assert!(t.has_side_panes());
    assert_eq!(t.tab_panes_of(Content::Tree), [9]);
    assert!(!t.add_side_panes(|| 10, 0.2, BIG), "one per tab");
    assert_eq!(t.remove_side_panes(), 1);
    assert!(!t.has_side_panes());
    assert_eq!(t.panes(), [1]);
}

#[test]
fn new_layouts_write_tree() {
    let mut t = Tiling::new(1, 10);
    t.add_side_panes(|| 9, 0.2, BIG);
    let text = serde_json::to_string(&t.save(|_| Some("a".into()))).unwrap();
    assert!(text.contains(r#""9":"Tree""#), "{text}");
}
