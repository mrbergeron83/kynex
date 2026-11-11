use super::*;

const BIG: Rect = Rect { x: 0.0, y: 0.0, w: 2000.0, h: 1000.0 };

fn three() -> Tiling {
    let mut t = Tiling::new(1, 10);
    assert!(t.split(Axis::Row, 2, 20, BIG));
    assert!(t.split(Axis::Column, 3, 30, BIG));
    t
}

#[test]
fn the_sessions_are_those_of_every_pane() {
    assert_eq!(three().sessions(), [10, 20, 30]);
}

#[test]
fn a_split_shows_a_new_session_in_the_new_focused_pane() {
    let t = three();
    assert_eq!(t.focus(), Some(3));
    assert_eq!(t.visible(), [(1, 10), (2, 20), (3, 30)]);
}

#[test]
fn a_session_already_on_show_cannot_be_split_in_again() {
    let mut t = Tiling::new(1, 10);
    assert!(!t.split(Axis::Row, 2, 10, BIG));
    assert_eq!(t.panes(), [1]);
}

#[test]
fn picking_a_session_that_is_on_show_focuses_its_pane() {
    let mut t = three();
    assert!(t.reconcile(&[10, 20, 30], 10).is_empty());
    assert_eq!(t.focus(), Some(1));
    assert_eq!(t.visible().len(), 3, "nothing was replaced");
}

#[test]
fn picking_a_session_that_is_not_on_show_replaces_the_focused_pane() {
    let mut t = three();
    t.focus_pane(2);
    assert!(t.reconcile(&[10, 20, 30, 40], 40).is_empty());
    assert_eq!(t.visible(), [(1, 10), (2, 40), (3, 30)]);
    assert_eq!(t.focus(), Some(2));
}

#[test]
fn a_pane_whose_session_closed_goes() {
    let mut t = three();
    // 30 closed; 20 is where the window is.
    assert_eq!(t.reconcile(&[10, 20], 20), [3]);
    assert_eq!(t.visible(), [(1, 10), (2, 20)]);
    assert_eq!(t.focus(), Some(2));
}

#[test]
fn the_last_pane_stays_and_takes_the_current_session() {
    let mut t = Tiling::new(1, 10);
    assert!(t.reconcile(&[11], 11).is_empty());
    assert_eq!(t.visible(), [(1, 11)]);
}

#[test]
fn closing_the_focused_pane_keeps_the_last_one() {
    let mut t = three();
    assert_eq!(t.close_focused(), Some(3));
    assert_eq!(t.close_focused(), Some(2));
    assert_eq!(t.close_focused(), None);
    assert_eq!(t.visible(), [(1, 10)]);
}

#[test]
fn a_new_tab_is_numbered_and_active() {
    let mut t = three();
    assert!(t.new_tab(4, 40));
    let tabs = t.tabs();
    assert_eq!(tabs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["1", "2"]);
    assert_eq!(tabs[0].sessions, [10, 20, 30]);
    assert_eq!((tabs[1].sessions.clone(), tabs[1].active), (vec![40], true));
    assert_eq!(t.visible(), [(4, 40)]);
    // A session that is on show somewhere can't open a second time.
    assert!(!t.new_tab(5, 10));
    // Numbers aren't reused while a higher one is around.
    t.rename_tab(1, "review");
    assert!(t.new_tab(5, 50));
    assert_eq!(t.tabs().iter().map(|t| t.name.clone()).collect::<Vec<_>>(), ["1", "review", "3"]);
}

#[test]
fn selecting_a_tab_shows_its_panes_and_picking_a_session_brings_its_tab_up() {
    let mut t = three();
    t.new_tab(4, 40);
    assert!(t.select_tab(0));
    assert_eq!(t.visible().len(), 3);
    assert!(!t.select_tab(7));
    // 40 becomes the session the window talks to: its tab comes up.
    t.reconcile(&[10, 20, 30, 40], 40);
    assert_eq!(t.visible(), [(4, 40)]);
}

#[test]
fn closing_a_tab_gives_back_its_panes_and_keeps_the_last_tab() {
    let mut t = three();
    t.new_tab(4, 40);
    assert_eq!(t.close_tab(0), [(1, 10), (2, 20), (3, 30)]);
    assert_eq!(t.tabs().len(), 1);
    assert_eq!(t.visible(), [(4, 40)]);
    assert!(t.close_tab(0).is_empty(), "the only tab stays");
}

#[test]
fn tabs_are_renamed_and_moved() {
    let mut t = three();
    t.new_tab(4, 40);
    t.rename_tab(0, "  agents ");
    t.rename_tab(1, "   ");
    assert_eq!(t.tabs().iter().map(|t| t.name.clone()).collect::<Vec<_>>(), ["agents", "2"]);
    t.move_tab(1, 0);
    let tabs = t.tabs();
    assert_eq!(tabs.iter().map(|t| t.name.clone()).collect::<Vec<_>>(), ["2", "agents"]);
    assert!(tabs[0].active, "the active tab is still the same tab");
}

#[test]
fn a_pane_goes_at_any_side_of_another() {
    let mut t = Tiling::new(1, 10);
    assert!(t.split_at(1, Direction::Left, 2, 20, BIG));
    assert_eq!(t.visible(), [(2, 20), (1, 10)], "left of it comes first");
    assert_eq!(t.focus(), Some(2));
    assert!(t.split_at(1, Direction::Down, 3, 30, BIG));
    assert_eq!(t.visible(), [(2, 20), (1, 10), (3, 30)]);
    assert!(t.split_at(1, Direction::Up, 4, 40, BIG));
    assert_eq!(t.visible(), [(2, 20), (4, 40), (1, 10), (3, 30)]);
    // Not a session that's on show, nor beside a pane that isn't there.
    assert!(!t.split_at(1, Direction::Right, 5, 20, BIG));
    assert!(!t.split_at(9, Direction::Right, 5, 50, BIG));
    assert_eq!(t.pane_of(40), Some(4));
}

#[test]
fn a_session_takes_a_panes_place() {
    let mut t = three();
    assert!(t.show_in(2, 40));
    assert_eq!(t.visible(), [(1, 10), (2, 40), (3, 30)]);
    assert!(!t.show_in(2, 10), "10 is in another pane");
    assert!(t.show_in(2, 40), "showing what it shows is fine");
    assert!(!t.show_in(9, 50));
}

#[test]
fn focus_and_swap_go_by_direction() {
    let mut t = three();
    assert_eq!(t.focus(), Some(3));
    assert!(t.focus_dir(Direction::Up, BIG));
    assert_eq!(t.focus(), Some(2));
    assert!(!t.focus_dir(Direction::Up, BIG));
    assert!(t.focus_dir(Direction::Left, BIG));
    assert_eq!(t.focus(), Some(1));
    let before = t.rects(BIG).pane(1).unwrap();
    assert!(t.swap_dir(Direction::Right, BIG));
    assert_eq!(t.focus(), Some(1));
    assert_ne!(t.rects(BIG).pane(1).unwrap(), before);
    assert!(!t.swap_dir(Direction::Up, BIG), "pane 1 is on the top edge now");
}

#[test]
fn the_focused_pane_grows_towards_a_direction() {
    let mut t = three();
    assert!(t.focus_pane(1));
    let w = t.rects(BIG).pane(1).unwrap().w;
    assert!(t.resize_focused(Direction::Right, 40.0, BIG));
    assert!((t.rects(BIG).pane(1).unwrap().w - w - 40.0).abs() < 0.5);
    assert!(!t.resize_focused(Direction::Left, 40.0, BIG));
}

#[test]
fn zoom_shows_one_pane_and_back() {
    let mut t = three();
    t.toggle_zoom();
    assert_eq!(t.zoomed(), Some(3));
    assert_eq!(t.visible(), [(3, 30)]);
    t.toggle_zoom();
    assert_eq!((t.zoomed(), t.visible().len()), (None, 3));
}

#[test]
fn any_pane_closes_but_the_last() {
    let mut t = three();
    assert_eq!(t.close_pane(1), Some(Content::Session(10)));
    assert_eq!(t.close_pane(1), None);
    assert_eq!(t.close_pane(2), Some(Content::Session(20)));
    assert_eq!(t.close_pane(3), None, "the last pane stays");
}

#[test]
fn panes_move_between_tabs() {
    let mut t = three();
    assert!(t.move_to_new_tab(3));
    assert_eq!(t.tabs().iter().map(|t| t.sessions.clone()).collect::<Vec<_>>(), [vec![10, 20], vec![30]]);
    assert!(!t.move_to_new_tab(3), "alone in its tab already");
    assert!(t.move_to_tab(2, 1, BIG));
    assert_eq!(t.tabs().iter().map(|t| t.sessions.clone()).collect::<Vec<_>>(), [vec![10], vec![30, 20]]);
    assert!(!t.move_to_tab(2, 1, BIG));
}

/// Session ids as the app would know them: key 10 is "s10".
fn id_of(key: Key) -> Option<String> {
    Some(format!("s{key}"))
}

fn key_of_id(id: Option<&str>) -> Option<Key> {
    id.map(|i| i[1..].parse().unwrap())
}

#[test]
fn a_saved_layout_comes_back_with_new_pane_ids() {
    let mut t = three();
    t.new_tab(4, 40);
    t.select_tab(0);
    t.focus_pane(2);
    let saved = t.save(id_of);
    assert_eq!(saved.sessions.len(), 4);
    let saved: SavedLayout = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
    let mut next = 100;
    let back = Tiling::restore(&saved, &mut next, None, key_of_id).unwrap();
    assert_eq!(next, 104, "four panes took four ids");
    assert_eq!(back.panes(), [100, 101, 102, 103]);
    assert_eq!(back.sessions(), [10, 20, 30, 40]);
    assert_eq!(back.tabs().iter().map(|t| t.sessions.clone()).collect::<Vec<_>>(), [vec![10, 20, 30], vec![40]]);
    assert_eq!(back.focus(), Some(101), "the focus came along");
    assert_eq!(back.key_of(back.focus().unwrap()), Some(20));
}

#[test]
fn a_pane_without_an_id_is_a_new_session() {
    let mut t = three();
    t.show_in(2, 99);
    let saved = t.save(|k| (k != 99).then(|| format!("s{k}")));
    assert_eq!(saved.sessions.len(), 2);
    let mut next = 1;
    let back =
        Tiling::restore(&saved, &mut next, None, |id| Some(id.map_or(500, |i| i[1..].parse().unwrap()))).unwrap();
    assert_eq!(back.sessions(), [10, 500, 30]);
}

#[test]
fn a_missing_session_closes_its_pane() {
    let saved = three().save(id_of);
    let mut next = 1;
    let back = Tiling::restore(&saved, &mut next, None, |id| key_of_id(id).filter(|k| *k != 20)).unwrap();
    assert_eq!(back.sessions(), [10, 30]);
    assert_eq!(back.panes().len(), 2);
    assert!(back.layout().is_valid());
    assert!(back.focus().is_some());
}

#[test]
fn no_session_left_is_no_tiling() {
    let saved = three().save(id_of);
    assert!(Tiling::restore(&saved, &mut 1, None, |_| None).is_none());
}

#[test]
fn a_session_in_two_panes_keeps_the_first() {
    let mut saved = three().save(id_of);
    saved.sessions.insert(2, "s10".into());
    let mut asked = Vec::new();
    let back = Tiling::restore(&saved, &mut 1, None, |id| {
        asked.push(id.map(str::to_string));
        key_of_id(id)
    })
    .unwrap();
    assert_eq!(back.sessions(), [10, 30]);
    assert_eq!(asked.len(), 2, "the repeat wasn't asked for twice");
}

#[test]
fn a_corrupt_layout_is_not_restored() {
    let saved = three().save(id_of);
    let mut next = 7;
    let json = serde_json::to_string(&saved).unwrap().replace("\"Pane\":3", "\"Pane\":1");
    let bad: SavedLayout = serde_json::from_str(&json).unwrap();
    assert!(Tiling::restore(&bad, &mut next, None, key_of_id).is_none());
    assert_eq!(next, 7, "nothing was taken");
    assert!(Tiling::restore(&SavedLayout::default(), &mut next, None, key_of_id).is_none());
}

#[test]
fn the_focused_pane_can_keep_an_id_and_reconcile_keeps_every_restored_session() {
    let mut t = three();
    t.focus_pane(2);
    let saved = t.save(id_of);
    let mut next = 50;
    let back = Tiling::restore(&saved, &mut next, Some(7), key_of_id);
    let mut back = back.unwrap();
    assert_eq!(back.focus(), Some(7));
    assert_eq!(back.key_of(7), Some(20));
    assert_eq!(back.panes().len(), 3);
    assert_eq!(next, 53, "every pane took a fresh id before the focused one was renamed");
    assert!(back.panes().iter().filter(|p| **p != 7).all(|p| (50..53).contains(p)));
    // The focused pane's session is gone: the pane that is focused after that keeps the id.
    let gone = Tiling::restore(&saved, &mut 50, Some(7), |id| key_of_id(id).filter(|k| *k != 20)).unwrap();
    assert_eq!(gone.panes().len(), 2);
    assert!(gone.focus() == Some(7) && gone.key_of(7).is_some_and(|k| k != 20));
    assert_eq!(gone.sessions().len(), 2);
    // The window talks to the focused pane's session: nothing is replaced.
    assert!(back.reconcile(&[10, 20, 30], 20).is_empty());
    assert_eq!(back.sessions(), [10, 20, 30]);
}

#[test]
fn renumbered_panes_do_not_collide_with_ones_in_use() {
    let saved = three().save(id_of);
    let mut next = 1;
    let a = Tiling::restore(&saved, &mut next, None, key_of_id).unwrap();
    let b = Tiling::restore(&saved, &mut next, None, key_of_id).unwrap();
    assert!(a.panes().iter().all(|p| !b.panes().contains(p)));
}

/// Session 10 over terminal 2, with terminal 3 beside it.
fn with_terminals() -> Tiling {
    let mut t = Tiling::new(1, 10);
    // The window has been talking to 10 before the terminals open, as in the app.
    t.reconcile(&[10], 10);
    assert!(t.add_terminal(2, TERMINAL_RATIO, BIG));
    assert!(t.split_terminal(2, Direction::Right, 3, BIG));
    t
}

#[test]
fn terminals_sit_side_by_side_under_a_session() {
    let t = with_terminals();
    assert_eq!(t.visible_panes(), [1, 2, 3]);
    assert_eq!(t.visible(), [(1, 10)], "sessions only");
    assert_eq!(t.terminals(), [2, 3]);
    assert_eq!((t.content(2), t.content(1)), (Some(Content::Terminal), Some(Content::Session(10))));
    assert_eq!(t.focus(), Some(3));
    let r = t.rects(BIG);
    let (s, a, b) = (r.pane(1).unwrap(), r.pane(2).unwrap(), r.pane(3).unwrap());
    assert!(s.w > 1999.0 && a.y > s.y && (a.y - b.y).abs() < 0.5 && a.x < b.x);
}

#[test]
fn a_terminal_keeps_the_focus_until_another_session_is_picked() {
    let mut t = with_terminals();
    t.reconcile(&[10], 10);
    assert_eq!(t.focus(), Some(3), "the session on show didn't change");
    assert_eq!(t.session_focus(), Some(1));
    // Another session: it goes into the session pane, not the terminal, and is focused.
    t.reconcile(&[10, 11], 11);
    assert_eq!(t.visible(), [(1, 11)]);
    assert_eq!(t.focus(), Some(1));
    assert!(t.is_terminal(3));
}

#[test]
fn the_last_session_pane_does_not_close_but_terminals_do() {
    let mut t = with_terminals();
    assert_eq!(t.close_pane(1), None);
    assert_eq!(t.close_pane(3), Some(Content::Terminal));
    assert_eq!(t.terminals(), [2]);
    // A session that closed doesn't take the last session pane with it either.
    assert!(t.reconcile(&[12], 12).is_empty());
    assert_eq!(t.visible(), [(1, 12)]);
}

#[test]
fn a_session_does_not_take_a_terminals_place() {
    let mut t = with_terminals();
    assert!(!t.show_in(2, 40));
}

#[test]
fn terminals_are_saved_and_come_back() {
    let mut t = with_terminals();
    t.focus_pane(1);
    let saved = t.save(id_of);
    assert_eq!(saved.tools.as_ref().map(|t| t.len()), Some(2));
    let saved: SavedLayout = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
    let back = Tiling::restore(&saved, &mut 100, None, key_of_id).unwrap();
    assert_eq!(back.terminals(), [101, 102]);
    assert_eq!(back.sessions(), [10]);
    assert_eq!(back.focus(), Some(100));
}

#[test]
fn a_terminal_focused_when_saved_gives_the_focus_to_a_session() {
    let saved = with_terminals().save(id_of);
    let back = Tiling::restore(&saved, &mut 1, Some(50), key_of_id).unwrap();
    assert_eq!(back.focus(), Some(50));
    assert_eq!(back.key_of(50), Some(10));
}

#[test]
fn terminals_without_a_session_are_not_restored() {
    let saved = with_terminals().save(id_of);
    assert!(Tiling::restore(&saved, &mut 1, None, |_| None).is_none());
}

#[test]
fn a_layout_from_before_terminal_panes_reads_as_having_none_said() {
    let json = serde_json::to_string(&three().save(id_of)).unwrap();
    let old = json.replace(",\"tools\":{}", "");
    let saved: SavedLayout = serde_json::from_str(&old).unwrap();
    assert_eq!(saved.tools, None);
    assert_eq!(Tiling::restore(&saved, &mut 1, None, key_of_id).unwrap().sessions(), [10, 20, 30]);
}

#[test]
fn a_tab_holding_the_only_sessions_does_not_close() {
    let mut t = Tiling::new(1, 10);
    assert!(t.add_terminal(2, TERMINAL_RATIO, BIG));
    assert!(t.move_to_new_tab(2));
    assert!(t.close_tab(0).is_empty(), "tab 0 has the only session");
    assert!(t.close_tab(1).is_empty(), "a terminal has no session to give back");
    assert_eq!(t.tabs().len(), 1);
    assert!(t.terminals().is_empty());
}

#[test]
fn git_and_the_timeline_sit_beside_a_session_and_come_back() {
    let mut t = Tiling::new(1, 10);
    t.reconcile(&[10], 10);
    assert!(t.split_tool(1, Direction::Right, 2, Tool::Timeline, 0.8, BIG));
    assert!(t.split_tool(1, Direction::Left, 9, Tool::Sessions, 0.3, BIG));
    assert!(t.split_tool(9, Direction::Down, 10, Tool::Workspaces, 0.4, BIG));
    assert_eq!((t.content(9), t.content(10)), (Some(Content::Sessions), Some(Content::Workspaces)));
    assert!(t.is_tool(9) && t.is_tool(10));
    assert_eq!(t.close_pane(9), Some(Content::Sessions));
    assert_eq!(t.close_pane(10), Some(Content::Workspaces));
    assert!(t.split_tool(1, Direction::Down, 3, Tool::Git, 0.5, BIG));
    assert_eq!((t.content(2), t.content(3)), (Some(Content::Timeline), Some(Content::Git)));
    assert_eq!(t.tab_panes_of(Content::Git), [3]);
    let r = t.rects(BIG);
    assert!(r.pane(2).unwrap().w < 0.25 * BIG.w, "the timeline got the narrow side");
    // A session doesn't land in them, and the focus stays on one while nothing changes.
    t.reconcile(&[10], 10);
    assert_eq!(t.focus(), Some(3));
    assert!(!t.show_in(2, 40));
    t.reconcile(&[10, 40], 40);
    assert_eq!(t.visible(), [(1, 40)]);
    let saved = t.save(id_of);
    let back = Tiling::restore(&saved, &mut 10, None, key_of_id).unwrap();
    assert_eq!(back.panes_of(Content::Timeline).len(), 1);
    assert_eq!(back.panes_of(Content::Git).len(), 1);
    assert_eq!(back.sessions(), [40]);
}

#[test]
fn an_agent_pane_is_no_shell_and_keeps_its_resume_id() {
    let mut t = Tiling::new(1, 10);
    t.reconcile(&[10], 10);
    let agent = TermPane { agent: Some("codex".into()), ..TermPane::default() };
    assert!(t.split_tool(1, Direction::Right, 2, Tool::Terminal(agent), 0.5, BIG));
    assert!(t.tab_terminals().is_empty(), "Ctrl+` doesn't mean the agent");
    assert_eq!(t.agents(), [(2, "codex".to_string())]);
    assert!(t.set_resume(2, "abc".into()));
    assert!(!t.set_resume(2, "abc".into()), "no change");
    assert!(!t.set_resume(1, "abc".into()), "a session has none");
    let back = Tiling::restore(&t.save(id_of), &mut 5, None, key_of_id).unwrap();
    let p = back.agents()[0].0;
    assert_eq!(back.term_pane(p).and_then(|t| t.resume.clone()).as_deref(), Some("abc"));
}

#[test]
fn a_drop_near_an_edge_is_for_that_side() {
    let r = Rect::new(100.0, 50.0, 400.0, 200.0);
    assert_eq!(drop_side(r, 120.0, 150.0), Some(Direction::Left));
    assert_eq!(drop_side(r, 480.0, 150.0), Some(Direction::Right));
    assert_eq!(drop_side(r, 300.0, 60.0), Some(Direction::Up));
    assert_eq!(drop_side(r, 300.0, 240.0), Some(Direction::Down));
    assert_eq!(drop_side(r, 300.0, 150.0), None, "the middle is the pane itself");
    assert_eq!(drop_side(Rect::new(0.0, 0.0, 0.0, 0.0), 1.0, 1.0), None);
}

#[test]
fn a_narrow_window_is_too_thin_for_a_split_but_not_for_one_pane() {
    let mut two = Tiling::new(1, 10);
    assert!(two.split(Axis::Row, 2, 20, BIG));
    let narrow = Rect::new(0.0, 0.0, 500.0, 800.0);
    assert!(two.too_narrow(narrow, 360.0));
    assert!(!two.too_narrow(BIG, 360.0), "wide enough: the split stays");
    assert!(!Tiling::new(1, 10).too_narrow(narrow, 360.0), "one pane is never too thin");
    two.toggle_zoom();
    assert!(!two.too_narrow(narrow, 360.0), "a zoomed pane already fills the tab");
}

#[test]
fn side_panes_go_on_the_left_of_every_tab_once() {
    let mut t = Tiling::new(1, 10);
    assert!(t.new_tab(2, 20));
    let mut next = 100;
    let mut ids = || {
        next += 1;
        next
    };
    t.seed_side_panes(&mut ids, 0.2, BIG);
    assert!(t.seeded());
    // Each tab: the sessions list, the workspaces above it, and the session.
    for i in 0..2 {
        assert!(t.select_tab(i));
        assert!(t.has_side_panes(), "tab {i}");
        assert_eq!(t.tab_panes_of(Content::Sessions).len(), 1);
        assert_eq!(t.tab_panes_of(Content::Workspaces).len(), 1);
        assert_eq!(t.visible_panes().len(), 3);
    }
    // The tab that was active still is, with a session focused.
    assert!(t.session_focus().is_some());
    // Not again where they are.
    assert!(!t.add_side_panes(&mut ids, 0.2, BIG));
    // They come back as saved, and `seeded` with them.
    let saved = t.save(|_| None);
    assert!(saved.seeded);
    let mut next_pane = 500;
    let back = Tiling::restore(&saved, &mut next_pane, None, |_| Some(10)).unwrap();
    assert!(back.seeded());
}

#[test]
fn closing_the_side_panes_sticks_and_they_can_come_back() {
    let mut t = Tiling::new(1, 10);
    let mut next = 100;
    t.seed_side_panes(
        || {
            next += 1;
            next
        },
        0.2,
        BIG,
    );
    assert_eq!(t.remove_side_panes(), 2);
    assert!(!t.has_side_panes());
    assert!(t.seeded(), "closed on purpose: no seeding again");
    assert!(t.add_side_panes(|| 900, 0.2, BIG) || t.add_side_panes(|| 901, 0.2, BIG));
}

#[test]
fn side_panes_may_be_thinner_than_a_conversation() {
    let mut t = Tiling::new(1, 10);
    let mut next = 100;
    t.seed_side_panes(
        || {
            next += 1;
            next
        },
        0.2,
        BIG,
    );
    // 1500 wide: the sessions column is 300, the conversation 1200. Fine.
    assert!(!t.too_narrow(Rect::new(0.0, 0.0, 1500.0, 900.0), 360.0));
    // 500 wide: the conversation is under what one needs.
    assert!(t.too_narrow(Rect::new(0.0, 0.0, 500.0, 900.0), 360.0));
}
