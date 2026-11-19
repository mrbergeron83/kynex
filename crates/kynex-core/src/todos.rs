//! Claude's todo list, for the strip over the composer: the CLI shows it above
//! its input. Two kinds of tools keep it, and a session may use either:
//!
//! - `TodoWrite` sends the whole list each time (`{"todos": [{content, status,
//!   activeForm}]}`); the latest call is the list.
//! - The task tools change it one item at a time: `TaskCreate` (`{subject,
//!   activeForm}`, its result says `Task #N created…`) and `TaskUpdate`
//!   (`{taskId, status, subject?, activeForm?}`, a status of `deleted` drops it).
//!
//! Read from the session's items, so a log read back has its list too.

use serde_json::Value;

use crate::session::Item;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    InProgress,
    Done,
}

impl Status {
    fn parse(s: &str) -> Self {
        match s {
            "in_progress" => Self::InProgress,
            "completed" => Self::Done,
            _ => Self::Pending,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Todo {
    /// The task tools' id; empty for `TodoWrite`'s.
    pub id: String,
    /// What's to do: "Run the tests".
    pub content: String,
    /// The same while it's being done: "Running the tests".
    pub active_form: String,
    pub status: Status,
}

impl Todo {
    /// What to show for it: the "-ing" form while it's being done.
    pub fn label(&self) -> &str {
        if self.status == Status::InProgress && !self.active_form.trim().is_empty() {
            &self.active_form
        } else {
            &self.content
        }
    }
}

/// The list as it stands after `items`; empty when Claude keeps none.
pub fn of(items: &[Item]) -> Vec<Todo> {
    let mut list: Vec<Todo> = Vec::new();
    // Task tools' ids: from each `TaskCreate`'s result once in, else counted.
    let mut next_id = 1u64;
    for (ix, item) in items.iter().enumerate() {
        let Item::ToolUse { id: call, name, input, .. } = item else { continue };
        match name.as_str() {
            "TodoWrite" => {
                let Ok(v) = serde_json::from_str::<Value>(input) else { continue };
                list = v["todos"].as_array().into_iter().flatten().map(from_todo_write).collect();
            }
            "TaskCreate" => {
                let Ok(v) = serde_json::from_str::<Value>(input) else { continue };
                let id = created_id(&items[ix + 1..], call).unwrap_or(next_id);
                next_id = id + 1;
                list.push(Todo {
                    id: id.to_string(),
                    content: text(&v["subject"]),
                    active_form: text(&v["activeForm"]),
                    status: Status::Pending,
                });
            }
            "TaskUpdate" => {
                let Ok(v) = serde_json::from_str::<Value>(input) else { continue };
                let id = match &v["taskId"] {
                    Value::Number(n) => n.to_string(),
                    other => text(other),
                };
                let Some(at) = list.iter().position(|t| t.id == id) else { continue };
                match v["status"].as_str() {
                    Some("deleted") => {
                        list.remove(at);
                        continue;
                    }
                    Some(s) => list[at].status = Status::parse(s),
                    None => {}
                }
                if let Some(s) = v["subject"].as_str() {
                    list[at].content = s.to_string();
                }
                if let Some(s) = v["activeForm"].as_str() {
                    list[at].active_form = s.to_string();
                }
            }
            _ => {}
        }
    }
    list
}

/// How far along the list is: (done, all).
pub fn progress(list: &[Todo]) -> (usize, usize) {
    (list.iter().filter(|t| t.status == Status::Done).count(), list.len())
}

/// The header's one line for a `TodoWrite` call: "3 / 7 done".
pub fn summary(input: &Value) -> String {
    let list: Vec<Todo> = input["todos"].as_array().into_iter().flatten().map(from_todo_write).collect();
    let (done, all) = progress(&list);
    format!("{done} / {all} done")
}

fn from_todo_write(t: &Value) -> Todo {
    Todo {
        id: String::new(),
        content: text(&t["content"]),
        active_form: text(&t["activeForm"]),
        status: Status::parse(t["status"].as_str().unwrap_or("")),
    }
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").trim().to_string()
}

/// The id a `TaskCreate` call's result gives its task: `Task #12 created…`.
fn created_id(after: &[Item], call: &str) -> Option<u64> {
    let content = after.iter().find_map(|i| match i {
        Item::ToolResult { id, content, is_error: false, .. } if id == call => Some(content),
        _ => None,
    })?;
    let rest = &content[content.find('#')? + 1..];
    rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Timing;

    fn call(id: &str, name: &str, input: &str) -> Item {
        Item::ToolUse {
            id: id.into(),
            name: name.into(),
            summary: String::new(),
            input: input.into(),
            timing: Timing::default(),
        }
    }

    fn result(id: &str, content: &str) -> Item {
        Item::ToolResult { id: id.into(), content: content.into(), images: Vec::new(), is_error: false }
    }

    #[test]
    fn todo_write_replaces_the_whole_list() {
        let first = r#"{"todos":[{"content":"Read","activeForm":"Reading","status":"completed"},
            {"content":"Fix","activeForm":"Fixing","status":"in_progress"}]}"#;
        let second = r#"{"todos":[{"content":"Fix","activeForm":"Fixing","status":"completed"},
            {"content":"Test","activeForm":"Testing","status":"pending"}]}"#;
        let items = [call("a", "TodoWrite", first)];
        let list = of(&items);
        assert_eq!(list.len(), 2);
        assert_eq!((list[1].label(), list[0].label()), ("Fixing", "Read"));
        assert_eq!(progress(&list), (1, 2));
        let items = [call("a", "TodoWrite", first), call("b", "TodoWrite", second)];
        let list = of(&items);
        assert_eq!(list.iter().map(|t| t.content.as_str()).collect::<Vec<_>>(), ["Fix", "Test"]);
        assert_eq!(summary(&serde_json::from_str(second).unwrap()), "1 / 2 done");
    }

    #[test]
    fn task_tools_change_it_one_item_at_a_time() {
        let items = [
            call("c1", "TaskCreate", r#"{"subject":"Write it","activeForm":"Writing it"}"#),
            result("c1", "Task #4 created successfully: Write it"),
            // No result yet: counted on from the last one.
            call("c2", "TaskCreate", r#"{"subject":"Test it"}"#),
            call("c3", "TaskCreate", r#"{"subject":"Drop me"}"#),
            call("u1", "TaskUpdate", r#"{"taskId":"4","status":"in_progress"}"#),
            call("u2", "TaskUpdate", r#"{"taskId":5,"status":"completed","subject":"Tested it"}"#),
            call("u3", "TaskUpdate", r#"{"taskId":"6","status":"deleted"}"#),
            call("u4", "TaskUpdate", r#"{"taskId":"99","status":"completed"}"#),
        ];
        let list = of(&items);
        let got: Vec<_> = list.iter().map(|t| (t.id.as_str(), t.label(), t.status)).collect();
        assert_eq!(got, [("4", "Writing it", Status::InProgress), ("5", "Tested it", Status::Done)]);
    }

    #[test]
    fn no_list_without_its_tools_or_with_broken_input() {
        assert!(of(&[]).is_empty());
        assert!(of(&[call("a", "Bash", "{}"), call("b", "TodoWrite", "{not json")]).is_empty());
        // A streamed call's input isn't in yet.
        assert!(of(&[call("a", "TodoWrite", "")]).is_empty());
    }
}
