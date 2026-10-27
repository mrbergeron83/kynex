//! Answering Claude's questions (`AskUserQuestion`) one at a time, like Claude
//! Code's own prompt: a tab per question, a cursor over its options with "Type
//! something" as the last one, and, with several questions, a Submit tab that
//! sums up the answers. This is the state and its rules; the app draws it.

use serde_json::{Map, Value};

pub struct Choice {
    pub label: String,
    pub description: String,
}

pub struct Question {
    /// A short tag for the tab, e.g. "Auth method".
    pub header: String,
    pub question: String,
    pub options: Vec<Choice>,
    pub multi: bool,
}

/// What a key or click did, for the app to follow up on.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// The cursor or tab moved, or a pick changed.
    Moved,
    /// The cursor is on "Type something": put the caret in its field.
    Type,
    /// Every question is answered: send [`Form::answers`].
    Submit,
    /// Don't answer; Claude stops.
    Cancel,
}

pub struct Form {
    pub questions: Vec<Question>,
    /// The tab shown: a question, or `questions.len()` for the Submit tab.
    pub tab: usize,
    /// The focused row in the tab: an option, or the "Type something" row
    /// just after them. On the Submit tab, 0 submits and 1 cancels.
    pub cursor: usize,
    picks: Vec<Vec<usize>>,
    other: Vec<String>,
}

impl Form {
    /// Reads the tool call's `questions`.
    pub fn parse(input: &Value) -> Self {
        let s = |v: &Value| v.as_str().unwrap_or("").to_string();
        let questions: Vec<Question> = input["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|q| Question {
                header: s(&q["header"]),
                question: s(&q["question"]),
                options: q["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|o| Choice { label: s(&o["label"]), description: s(&o["description"]) })
                    .collect(),
                multi: q["multiSelect"] == true,
            })
            .collect();
        let n = questions.len();
        Self { questions, tab: 0, cursor: 0, picks: vec![Vec::new(); n], other: vec![String::new(); n] }
    }

    /// Several questions get tabs and a Submit tab; a single one answers directly.
    pub fn has_tabs(&self) -> bool {
        self.questions.len() > 1
    }

    pub fn on_submit_tab(&self) -> bool {
        self.tab >= self.questions.len()
    }

    /// The question shown, unless it's the Submit tab.
    pub fn current(&self) -> Option<&Question> {
        self.questions.get(self.tab)
    }

    /// The "Type something" row of question `q`.
    pub fn other_row(&self, q: usize) -> usize {
        self.questions.get(q).map_or(0, |q| q.options.len())
    }

    pub fn on_other_row(&self) -> bool {
        !self.on_submit_tab() && self.cursor == self.other_row(self.tab)
    }

    fn rows(&self) -> usize {
        if self.on_submit_tab() { 2 } else { self.other_row(self.tab) + 1 }
    }

    pub fn picked(&self, q: usize, n: usize) -> bool {
        self.picks.get(q).is_some_and(|p| p.contains(&n))
    }

    pub fn other(&self, q: usize) -> &str {
        self.other.get(q).map_or("", String::as_str)
    }

    /// The text typed for question `q`. For a single-choice question it
    /// replaces any option picked.
    pub fn set_other(&mut self, q: usize, text: String) {
        let Some(slot) = self.other.get_mut(q) else { return };
        *slot = text;
        if !slot.trim().is_empty() && !self.questions[q].multi {
            self.picks[q].clear();
        }
    }

    /// The answer to question `q` as Claude gets it: the picked labels, joined,
    /// and the typed text.
    pub fn answer(&self, q: usize) -> Option<String> {
        let question = self.questions.get(q)?;
        let mut parts: Vec<&str> =
            self.picks[q].iter().filter_map(|&n| question.options.get(n)).map(|c| c.label.as_str()).collect();
        let other = self.other[q].trim();
        if !other.is_empty() {
            parts.push(other);
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }

    pub fn complete(&self) -> bool {
        (0..self.questions.len()).all(|q| self.answer(q).is_some())
    }

    /// The answers by question text, as `AskUserQuestion` takes them.
    pub fn answers(&self) -> Map<String, Value> {
        self.questions
            .iter()
            .enumerate()
            .filter_map(|(q, question)| Some((question.question.clone(), Value::String(self.answer(q)?))))
            .collect()
    }

    pub fn move_cursor(&mut self, down: bool) -> Step {
        let last = self.rows() - 1;
        self.cursor = if down { (self.cursor + 1).min(last) } else { self.cursor.saturating_sub(1) };
        self.landed()
    }

    /// Shows tab `tab`, with the cursor on its first pick (or its typed text).
    pub fn go_to(&mut self, tab: usize) -> Step {
        let tabs = self.questions.len() + usize::from(self.has_tabs());
        self.tab = tab.min(tabs - 1);
        self.cursor = match self.picks.get(self.tab) {
            Some(p) if !self.other[self.tab].trim().is_empty() && p.is_empty() => self.other_row(self.tab),
            Some(p) => p.first().copied().unwrap_or(0),
            None => 0,
        };
        self.landed()
    }

    pub fn next_tab(&mut self) -> Step {
        self.go_to(self.tab + 1)
    }

    pub fn prev_tab(&mut self) -> Step {
        self.go_to(self.tab.saturating_sub(1))
    }

    fn landed(&self) -> Step {
        if self.on_other_row() { Step::Type } else { Step::Moved }
    }

    /// Picks row `n` of the shown tab (a click, or its number key). A
    /// single-choice pick moves on by itself; multi-select toggles.
    pub fn choose(&mut self, n: usize) -> Step {
        if self.on_submit_tab() {
            return match n {
                0 => self.submit(),
                1 => Step::Cancel,
                _ => Step::Moved,
            };
        }
        let q = self.tab;
        if n > self.other_row(q) {
            return Step::Moved;
        }
        self.cursor = n;
        if n == self.other_row(q) {
            return Step::Type;
        }
        if self.questions[q].multi {
            match self.picks[q].iter().position(|&p| p == n) {
                Some(at) => {
                    self.picks[q].remove(at);
                }
                None => self.picks[q].push(n),
            }
            return Step::Moved;
        }
        self.picks[q] = vec![n];
        self.other[q].clear();
        self.advance()
    }

    /// Enter: picks the row under the cursor, or for multi-select and typed
    /// text, takes the question as answered and moves on.
    pub fn confirm(&mut self) -> Step {
        if self.on_submit_tab() {
            return self.choose(self.cursor);
        }
        let q = self.tab;
        if self.questions[q].multi || self.on_other_row() {
            return if self.answer(q).is_some() { self.advance() } else { self.landed() };
        }
        self.choose(self.cursor)
    }

    /// After answering the shown question: submit a lone question, else go to
    /// the next tab (the last one leads to Submit).
    fn advance(&mut self) -> Step {
        if !self.has_tabs() { self.submit() } else { self.next_tab() }
    }

    /// Submits if everything is answered, else shows the first open question.
    fn submit(&mut self) -> Step {
        match (0..self.questions.len()).find(|&q| self.answer(q).is_none()) {
            None => Step::Submit,
            Some(q) => self.go_to(q),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn form(multi_second: bool) -> Form {
        Form::parse(&json!({ "questions": [
            { "header": "Auth", "question": "Which auth?", "multiSelect": false,
              "options": [{ "label": "OAuth", "description": "via a provider" }, { "label": "Password" }] },
            { "header": "Extras", "question": "What else?", "multiSelect": multi_second,
              "options": [{ "label": "Logs" }, { "label": "Metrics" }] }
        ] }))
    }

    #[test]
    fn parses_questions() {
        let f = form(true);
        assert_eq!(f.questions.len(), 2);
        assert_eq!(f.questions[0].options[0].description, "via a provider");
        assert!(f.questions[1].multi && !f.questions[0].multi);
        assert!(f.has_tabs());
    }

    #[test]
    fn picking_moves_on_and_the_last_tab_submits() {
        let mut f = form(false);
        assert_eq!(f.choose(1), Step::Moved);
        assert_eq!(f.tab, 1);
        f.choose(0);
        assert!(f.on_submit_tab());
        assert_eq!(f.choose(0), Step::Submit);
        let answers = f.answers();
        assert_eq!(answers["Which auth?"], "Password");
        assert_eq!(answers["What else?"], "Logs");
    }

    #[test]
    fn picking_again_replaces_a_single_pick() {
        let mut f = form(false);
        f.choose(0);
        assert!(f.picked(0, 0));
        assert!(!f.complete(), "the second question is still open");
        f.go_to(0);
        f.choose(1);
        assert!(f.picked(0, 1) && !f.picked(0, 0));
        assert_eq!(f.answer(0).as_deref(), Some("Password"));
        f.choose(1);
        assert!(f.complete());
    }

    #[test]
    fn submitting_early_shows_the_open_question() {
        let mut f = form(false);
        f.go_to(2);
        assert_eq!(f.choose(0), Step::Moved);
        assert_eq!(f.tab, 0);
        f.go_to(2);
        assert_eq!(f.choose(1), Step::Cancel);
    }

    #[test]
    fn multi_select_toggles_and_enter_moves_on() {
        let mut f = form(true);
        f.go_to(1);
        assert_eq!(f.confirm(), Step::Moved, "nothing picked yet: stays");
        assert_eq!(f.tab, 1);
        f.choose(0);
        f.choose(1);
        f.choose(0);
        assert_eq!(f.answer(1).as_deref(), Some("Metrics"));
        f.confirm();
        assert!(f.on_submit_tab());
    }

    #[test]
    fn typed_text_replaces_a_single_pick() {
        let mut f = form(false);
        f.choose(0);
        f.go_to(0);
        assert_eq!(f.cursor, 0, "the cursor starts on the pick");
        assert_eq!(f.move_cursor(true), Step::Moved);
        assert_eq!(f.move_cursor(true), Step::Type);
        assert_eq!(f.move_cursor(true), Step::Type, "stays on the last row");
        f.set_other(0, "SSO".into());
        assert!(!f.picked(0, 0));
        assert_eq!(f.answer(0).as_deref(), Some("SSO"));
        assert_eq!(f.confirm(), Step::Moved);
        assert_eq!(f.tab, 1);
        assert_eq!(f.go_to(0), Step::Type, "back on a typed answer: the field");
    }

    #[test]
    fn a_lone_question_submits_straight_away() {
        let mut f = Form::parse(&json!({ "questions": [
            { "question": "Go?", "options": [{ "label": "Yes" }, { "label": "No" }] }
        ] }));
        assert!(!f.has_tabs());
        f.move_cursor(true);
        assert_eq!(f.confirm(), Step::Submit);
        assert_eq!(f.answers()["Go?"], "No");
        assert_eq!(f.go_to(5), Step::Moved, "no Submit tab to go to");
        assert_eq!(f.tab, 0);
    }
}
