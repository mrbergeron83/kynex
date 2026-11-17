//! Which session started which: a session made or forked at another session's request (or
//! forked by hand) is that session's child. The links are kept per project, since a child
//! may live in another workspace of it (whose `sessions.json` is a different file), and are
//! keyed by CLI session id, which outlives a run. A child only gets its id once its process
//! reports one; the app holds the link by uid until then.
//!
//! The tree nests a child under its parent ([`Lineage::arrange`]); a child's turn's answer
//! goes back to its parent as a message ([`route`], [`notice`]). Caps on depth and on open
//! children ([`Lineage::can_add`]) keep the tree, and the answers going up it, bounded.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session::Item;

/// How deep sessions may nest: a child of a child of … this many levels below a top one.
pub const MAX_DEPTH: usize = 4;

/// How many open children a session may have at once.
pub const MAX_CHILDREN: usize = 5;

/// A child's parent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// The parent's CLI session id.
    pub parent: String,
    /// The parent's workspace folder.
    pub parent_folder: PathBuf,
    /// The child's own workspace folder.
    pub folder: PathBuf,
    /// Its answers go back to the parent: a session asked for it. A fork made by hand only
    /// nests (it is another way to go, not work handed out).
    #[serde(default)]
    pub reports: bool,
}

/// A project's parent links, and which parents are folded in the tree.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lineage {
    /// Child id to its parent.
    pub links: BTreeMap<String, Link>,
    /// Parents whose children the tree folds away.
    pub folded: BTreeSet<String>,
}

/// Where a key landed when [`Lineage::arrange`] nested a list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placed {
    /// How far it is nested under a parent in the same list.
    pub depth: usize,
    /// Its children nested under it in this list (drawn, or folded away).
    pub children: usize,
    /// Its descendants folded away under it, for its lamp.
    pub hidden: Vec<String>,
    /// Its children that aren't in this list: another workspace, or another section.
    pub elsewhere: usize,
    /// Its parent, when that isn't the row it is nested under.
    pub parent_away: Option<String>,
}

impl Lineage {
    const FILE: &str = "parents.json";

    /// The links of `project` (blocks: reads a file).
    pub fn load(project: &Path) -> Self {
        crate::project::load(project, Self::FILE)
    }

    /// Writes them (blocks).
    pub fn save(&self, project: &Path) -> Result<(), String> {
        crate::project::save(project, Self::FILE, self)
    }

    /// Notes that `child` was started by `parent`. A link that would make a loop is refused.
    pub fn link(&mut self, child: &str, link: Link) -> bool {
        if child == link.parent || self.ancestors(&link.parent).iter().any(|a| a == child) {
            return false;
        }
        self.links.insert(child.to_string(), link);
        true
    }

    pub fn parent_of(&self, child: &str) -> Option<&Link> {
        self.links.get(child)
    }

    /// The direct children of `id`.
    pub fn children_of(&self, id: &str) -> Vec<&str> {
        self.links.iter().filter(|(_, l)| l.parent == id).map(|(c, _)| c.as_str()).collect()
    }

    /// Its parent, its parent's parent, … nearest first; a loop in the links ends it.
    pub fn ancestors(&self, id: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut at = id;
        while let Some(l) = self.links.get(at) {
            if l.parent == id || out.contains(&l.parent) {
                break;
            }
            out.push(l.parent.clone());
            at = &l.parent;
        }
        out
    }

    /// How many levels below a top session `id` is.
    pub fn depth(&self, id: &str) -> usize {
        self.ancestors(id).len()
    }

    /// Everything under `id`: its children, theirs, and so on.
    pub fn descendants(&self, id: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut todo = vec![id.to_string()];
        while let Some(at) = todo.pop() {
            for c in self.children_of(&at) {
                if c != id && !out.iter().any(|o| o == c) {
                    out.push(c.to_string());
                    todo.push(c.to_string());
                }
            }
        }
        out
    }

    /// Whether `parent` may start one more child: not past [`MAX_DEPTH`], and fewer than
    /// [`MAX_CHILDREN`] open ones (`open_children`: those open now, linked or not yet).
    pub fn can_add(&self, parent: &str, open_children: usize) -> Result<(), String> {
        if self.depth(parent) + 1 > MAX_DEPTH {
            return Err(format!(
                "sessions nest at most {MAX_DEPTH} deep, and this one is {} below a top session",
                self.depth(parent)
            ));
        }
        if open_children >= MAX_CHILDREN {
            return Err(format!("a session may have at most {MAX_CHILDREN} children open at once; close one first"));
        }
        Ok(())
    }

    /// `id` is gone (deleted): its children move up a level, to its parent or to the top.
    /// Returns whether anything changed.
    pub fn remove(&mut self, id: &str) -> bool {
        let up = self.links.remove(id);
        let mut changed = up.is_some() | self.folded.remove(id);
        for (_, l) in self.links.iter_mut().filter(|(_, l)| l.parent == id) {
            changed = true;
            match &up {
                Some(u) => {
                    l.parent = u.parent.clone();
                    l.parent_folder = u.parent_folder.clone();
                }
                None => l.parent.clear(),
            }
        }
        self.links.retain(|_, l| !l.parent.is_empty());
        changed
    }

    /// Folds or unfolds `id`'s children in the tree.
    pub fn toggle_fold(&mut self, id: &str) {
        if !self.folded.remove(id) {
            self.folded.insert(id.to_string());
        }
    }

    /// Orders one list of the tree so each child follows its parent (when both are in it),
    /// and drops the descendants of folded parents. Keys that aren't linked (a new session's
    /// `new:` key) stay where they are. Returns where each kept key landed.
    pub fn arrange(&self, keys: &mut Vec<String>) -> HashMap<String, Placed> {
        let here: BTreeSet<&str> = keys.iter().map(String::as_str).collect();
        // A key is nested when its parent is in the list (and the chain up has no loop).
        let nested = |k: &str| self.links.get(k).is_some_and(|l| here.contains(l.parent.as_str()));
        let mut out: Vec<String> = Vec::new();
        let mut placed: HashMap<String, Placed> = HashMap::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let children_here = |id: &str| -> Vec<String> {
            // In the list's own order.
            keys.iter().filter(|c| self.links.get(*c).is_some_and(|l| l.parent == id)).cloned().collect()
        };
        #[allow(clippy::too_many_arguments)]
        fn walk(
            me: &Lineage,
            k: &str,
            depth: usize,
            hidden_under: Option<&str>,
            children_here: &dyn Fn(&str) -> Vec<String>,
            out: &mut Vec<String>,
            placed: &mut HashMap<String, Placed>,
            seen: &mut BTreeSet<String>,
        ) {
            if !seen.insert(k.to_string()) {
                return;
            }
            let kids = children_here(k);
            if let Some(top) = hidden_under {
                if let Some(p) = placed.get_mut(top) {
                    p.hidden.push(k.to_string());
                }
            } else {
                out.push(k.to_string());
                placed.insert(
                    k.to_string(),
                    Placed {
                        depth,
                        children: kids.len(),
                        elsewhere: me.children_of(k).len().saturating_sub(kids.len()),
                        ..Placed::default()
                    },
                );
            }
            let hide = hidden_under.or_else(|| me.folded.contains(k).then_some(k));
            for c in kids {
                walk(me, &c, depth + 1, hide, children_here, out, placed, seen);
            }
        }
        for k in keys.iter().filter(|k| !nested(k)) {
            walk(self, k, 0, None, &children_here, &mut out, &mut placed, &mut seen);
        }
        // Keys whose parents name each other in a loop: on their own.
        for k in keys.iter() {
            walk(self, k, 0, None, &children_here, &mut out, &mut placed, &mut seen);
        }
        for (k, p) in placed.iter_mut() {
            if p.depth == 0 {
                p.parent_away = self.links.get(k).map(|l| l.parent.clone());
            }
        }
        *keys = out;
        placed
    }
}

/// Where a child's answer (or a request's outcome) goes, by how its parent stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// The parent is idle: send it now, starting a turn.
    SendNow,
    /// The parent is working: queue it, sent with the rest when its turn ends.
    Queue,
    /// The parent is closed: nothing is sent.
    Drop,
}

/// How to deliver to a parent that is open and `Some(busy)`, or closed (`None`).
pub fn route(parent_busy: Option<bool>) -> Route {
    match parent_busy {
        None => Route::Drop,
        Some(true) => Route::Queue,
        Some(false) => Route::SendNow,
    }
}

/// What the turn that just ended in `items` answered: what was said after its last tool call,
/// and whether it ended in an error. `None` if the last item isn't a turn's end.
pub fn turn_answer(items: &[Item]) -> Option<(String, bool)> {
    let (end, Item::Result { is_error, .. }) = (items.len().checked_sub(1)?, items.last()?) else {
        return None;
    };
    let start = items[..end]
        .iter()
        .rposition(|i| matches!(i, Item::User { .. } | Item::Task(_) | Item::Result { .. }))
        .map_or(0, |i| i + 1);
    let turn = &items[start..end];
    let after = turn.iter().rposition(|i| matches!(i, Item::ToolUse { .. })).map_or(0, |i| i + 1);
    let said: Vec<&str> = turn[after..]
        .iter()
        .filter_map(|i| match i {
            Item::Assistant(t) if !t.trim().is_empty() => Some(t.trim()),
            Item::Error(e) => Some(e.trim()),
            _ => None,
        })
        .collect();
    Some((said.join("\n\n"), *is_error))
}

/// The tag a message from another session is wrapped in, so the transcript (and a replay of
/// the log) shows it as a notice, not as something you said.
const TAG: &str = "session-notification";

/// How a message carrying a notice starts it: a cheap test before [`split_notices`].
pub const NOTICE_OPEN: &str = "<session-notification ";

/// A message from another session: who it's from, and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// The sending session's CLI id, or uid before it has one.
    pub from: String,
    pub title: String,
    pub text: String,
}

/// The message a session gets from `from` (`title`): `text`, wrapped so it reads as a notice.
pub fn notice(n: &Notice) -> String {
    let clean = |s: &str| s.replace(['"', '<', '>', '\n'], " ");
    // No closing tag inside, not even one a removal would make.
    let close = format!("</{TAG}>");
    let mut text = n.text.trim().to_string();
    while text.contains(&close) {
        text = text.replace(&close, "");
    }
    format!("<{TAG} from=\"{}\" title=\"{}\">\n{text}\n{close}", clean(&n.from), clean(&n.title))
}

/// Reads back a message written by [`notice`]; `None` for anything else.
pub fn parse_notice(text: &str) -> Option<Notice> {
    let rest = text.trim().strip_prefix(&format!("<{TAG} "))?;
    let (head, body) = rest.split_once('>')?;
    let body = body.strip_suffix(&format!("</{TAG}>"))?;
    let attr = |name: &str| -> Option<String> {
        let at = head.find(&format!("{name}=\""))? + name.len() + 2;
        let end = head[at..].find('"')?;
        Some(head[at..at + end].to_string())
    };
    Some(Notice { from: attr("from")?, title: attr("title").unwrap_or_default(), text: body.trim().to_string() })
}

/// A message as the transcript draws it: the notices from other sessions in it (several
/// queued answers go out as one message), and what's left, which you wrote.
pub fn split_notices(text: &str) -> (Vec<Notice>, String) {
    let open = format!("<{TAG} ");
    let close = format!("</{TAG}>");
    let (mut notices, mut rest) = (Vec::new(), String::new());
    let mut at = text;
    while let Some(start) = at.find(&open) {
        let Some(len) = at[start..].find(&close).map(|e| e + close.len()) else { break };
        let Some(n) = parse_notice(&at[start..start + len]) else { break };
        rest.push_str(&at[..start]);
        notices.push(n);
        at = &at[start + len..];
    }
    rest.push_str(at);
    (notices, rest.trim().to_string())
}

/// The first message of a session another one started: what it was asked, then who started
/// it and why (after, so the session's title is still what it was asked).
pub fn first_prompt(parent_title: &str, parent: &str, why: Option<&str>, prompt: &str) -> String {
    let why = why.map(str::trim).filter(|w| !w.is_empty()).map(|w| format!(": {w}")).unwrap_or_default();
    format!(
        "{prompt}\n\n(Started from Kynex by session \"{parent_title}\" ({parent}){why}. When your turn \
         ends, your answer goes back to it.)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(parent: &str, folder: &str) -> Link {
        Link { parent: parent.into(), parent_folder: "/p".into(), folder: folder.into(), reports: true }
    }

    fn keys(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn family() -> Lineage {
        let mut l = Lineage::default();
        l.link("b", link("a", "/p"));
        l.link("c", link("b", "/p"));
        l.link("d", link("a", "/p/wt"));
        l
    }

    #[test]
    fn links_answer_up_and_down() {
        let l = family();
        assert_eq!(l.parent_of("c").map(|p| p.parent.as_str()), Some("b"));
        assert_eq!(l.ancestors("c"), ["b", "a"]);
        assert_eq!(l.depth("c"), 2);
        assert_eq!(l.depth("a"), 0);
        let mut kids = l.children_of("a");
        kids.sort();
        assert_eq!(kids, ["b", "d"]);
        let mut all = l.descendants("a");
        all.sort();
        assert_eq!(all, ["b", "c", "d"]);
    }

    #[test]
    fn a_loop_is_refused_and_a_hand_made_one_ends() {
        let mut l = family();
        assert!(!l.link("a", link("c", "/p")), "a under its own grandchild");
        assert!(!l.link("a", link("a", "/p")));
        // A file edited by hand into a loop still answers.
        l.links.insert("a".into(), link("c", "/p"));
        assert_eq!(l.ancestors("c").len(), 2);
        assert!(l.descendants("a").len() <= 3);
        let mut list = keys(&["a", "b", "c"]);
        let placed = l.arrange(&mut list);
        assert_eq!(list.len(), 3, "each once: {list:?}");
        assert_eq!(placed.len(), 3);
    }

    #[test]
    fn caps_on_depth_and_open_children() {
        let mut l = Lineage::default();
        for i in 1..=MAX_DEPTH {
            l.link(&format!("s{i}"), link(&format!("s{}", i - 1), "/p"));
        }
        assert!(l.can_add(&format!("s{}", MAX_DEPTH - 1), 0).is_ok());
        let deep = l.can_add(&format!("s{MAX_DEPTH}"), 0).unwrap_err();
        assert!(deep.contains("deep"), "{deep}");
        assert!(l.can_add("s0", MAX_CHILDREN - 1).is_ok());
        assert!(l.can_add("s0", MAX_CHILDREN).unwrap_err().contains("children"));
    }

    #[test]
    fn deleting_a_parent_moves_its_children_up() {
        let mut l = family();
        assert!(l.remove("b"));
        assert_eq!(l.parent_of("c").map(|p| p.parent.as_str()), Some("a"), "c moves up to a");
        assert!(l.parent_of("b").is_none());
        assert!(l.remove("a"));
        assert!(l.links.is_empty(), "a was on top: its children are now top ones");
        assert!(!l.remove("nobody"));
    }

    #[test]
    fn archiving_takes_the_nest() {
        let l = family();
        let mut nest = l.descendants("b");
        nest.sort();
        assert_eq!(nest, ["c"]);
        // Each in its own folder.
        assert_eq!(l.parent_of("d").map(|p| p.folder.clone()), Some(PathBuf::from("/p/wt")));
    }

    #[test]
    fn children_follow_their_parent_in_a_list() {
        let l = family();
        let mut list = keys(&["c", "x", "a", "new:5", "b"]);
        let placed = l.arrange(&mut list);
        assert_eq!(list, ["x", "a", "b", "c", "new:5"]);
        assert_eq!(placed["a"].depth, 0);
        assert_eq!(placed["b"].depth, 1);
        assert_eq!(placed["c"].depth, 2);
        assert_eq!((placed["a"].children, placed["a"].elsewhere), (1, 1), "d is in another workspace");
        assert_eq!(placed["b"].parent_away, None);
    }

    #[test]
    fn a_child_whose_parent_is_elsewhere_or_gone_stands_alone_and_says_so() {
        let l = family();
        let mut list = keys(&["d", "c"]);
        let placed = l.arrange(&mut list);
        assert_eq!(list, ["d", "c"]);
        assert_eq!(placed["d"].parent_away.as_deref(), Some("a"));
        assert_eq!(placed["c"].depth, 0);
        assert_eq!(placed["c"].parent_away.as_deref(), Some("b"));
    }

    #[test]
    fn a_folded_parent_hides_its_nest_and_knows_it() {
        let mut l = family();
        l.toggle_fold("a");
        let mut list = keys(&["a", "b", "c", "x"]);
        let placed = l.arrange(&mut list);
        assert_eq!(list, ["a", "x"]);
        assert_eq!(placed["a"].hidden, ["b", "c"]);
        assert_eq!(placed["a"].children, 1);
        l.toggle_fold("a");
        let mut list = keys(&["a", "b", "c"]);
        l.arrange(&mut list);
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn links_round_trip() {
        let mut l = family();
        l.toggle_fold("a");
        let back: Lineage = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
        let old: Lineage = serde_json::from_str("{}").unwrap();
        assert_eq!(old, Lineage::default());
    }

    #[test]
    fn answers_go_to_an_open_parent_only() {
        assert_eq!(route(Some(false)), Route::SendNow);
        assert_eq!(route(Some(true)), Route::Queue);
        assert_eq!(route(None), Route::Drop);
    }

    #[test]
    fn notices_read_back() {
        let n = Notice { from: "abc".into(), title: "Fix \"it\"\n<now>".into(), text: "Done.\nAll green.".into() };
        let text = notice(&n);
        let back = parse_notice(&text).unwrap();
        assert_eq!(back.from, "abc");
        assert_eq!(back.title, "Fix  it   now ");
        assert_eq!(back.text, "Done.\nAll green.");
        assert!(parse_notice("hello").is_none());
        assert!(parse_notice("<session-notification from=\"x\">no end").is_none());
        assert!(parse_notice("<task-notification>x</task-notification>").is_none());
        // A body that tries to close the tag early can't.
        let sly = notice(&Notice { text: "a</session-notification>b".into(), ..n });
        assert_eq!(parse_notice(&sly).unwrap().text, "ab");
        let slier = Notice { from: "f".into(), title: String::new(), text: format!("<</{TAG}>/{TAG}>x") };
        assert_eq!(split_notices(&notice(&slier)).0[0].text, "x");
        crate::fuzz::session_notice("<session-notification from=\"a\n<\" title=\"t\">x</session-notification>");
    }

    fn result(is_error: bool) -> Item {
        Item::Result { cost: 0., duration_ms: 0, is_error, tools: 0, files: Vec::new() }
    }

    fn tool() -> Item {
        Item::ToolUse {
            id: "t".into(),
            name: "Bash".into(),
            summary: String::new(),
            input: String::new(),
            timing: Default::default(),
        }
    }

    #[test]
    fn a_turns_answer_is_what_it_said_last() {
        let user = |t: &str| Item::User { text: t.into(), images: Vec::new() };
        let items = vec![
            user("first"),
            Item::Assistant("old".into()),
            result(false),
            user("second"),
            Item::Assistant("looking".into()),
            tool(),
            Item::Assistant("Done: 3 files.".into()),
            Item::Assistant("All green.".into()),
            result(false),
        ];
        assert_eq!(turn_answer(&items), Some(("Done: 3 files.\n\nAll green.".into(), false)));
        assert_eq!(turn_answer(&items[..8]), None, "not ended");
        let failed = vec![user("x"), Item::Error("process exited".into()), result(true)];
        assert_eq!(turn_answer(&failed), Some(("process exited".into(), true)));
        assert_eq!(turn_answer(&[user("x"), tool(), result(false)]).map(|a| a.0), Some(String::new()));
    }

    #[test]
    fn notices_are_taken_out_of_a_message() {
        let a = notice(&Notice { from: "a".into(), title: "A".into(), text: "one".into() });
        let b = notice(&Notice { from: "b".into(), title: "B".into(), text: "two".into() });
        let (ns, rest) = split_notices(&format!("look at this\n\n{a}\n\n{b}"));
        assert_eq!(ns.iter().map(|n| n.from.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(rest, "look at this");
        let (ns, rest) = split_notices("plain <session-notification from=\"x\"> unclosed");
        assert!(ns.is_empty());
        assert!(rest.ends_with("unclosed"));
        assert_eq!(split_notices(&a).1, "");
    }

    #[test]
    fn a_childs_first_prompt_says_who_and_why() {
        let p = first_prompt("Refactor", "abc", Some(" split the parser "), "Do the tests");
        assert!(p.contains("\"Refactor\" (abc): split the parser."), "{p}");
        assert!(p.starts_with("Do the tests\n\n"));
        assert!(!first_prompt("R", "abc", None, "x").contains(": "));
    }
}
