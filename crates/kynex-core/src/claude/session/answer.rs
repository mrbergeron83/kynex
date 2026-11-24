//! Answering permission prompts and questions.

use serde_json::{Value, json};

use super::*;

impl Session {
    /// Answers a permission prompt. For [`Answer::Project`] it returns the rules
    /// to save in the project's settings; the live process gets them right away
    /// as session rules, and later processes get them from the saved settings.
    pub fn answer_permission(&mut self, index: usize, answer: Answer) -> Vec<String> {
        self.answer_permission_with(index, answer, "")
    }

    /// [`Self::answer_permission`] with a comment on a yes ("yes, but use the
    /// staging config"). The CLI's allow response has no field for one, so it
    /// goes as a user message right behind the answer; the CLI takes messages
    /// that arrive mid-turn in at the next step, so Claude reads it before it
    /// carries on. A comment on a no is [`Answer::Deny`]'s feedback.
    pub fn answer_permission_with(&mut self, index: usize, answer: Answer, comment: &str) -> Vec<String> {
        let comment = comment.trim();
        let pending = matches!(self.items.get(index), Some(Item::Permission { state: PermState::Pending, .. }));
        let allows = pending && !matches!(answer, Answer::Deny(_));
        let rules = self.answer_inner(index, answer);
        if allows
            && !comment.is_empty()
            && let Some(Item::Permission { state: PermState::Allowed(note), .. }) = self.items.get_mut(index)
        {
            note.push_str(&format!(" · “{comment}”"));
            self.write(json!({ "type": "user", "message": { "role": "user", "content": [
                { "type": "text", "text": comment }
            ] } }));
        }
        rules
    }

    pub(super) fn answer_inner(&mut self, index: usize, answer: Answer) -> Vec<String> {
        self.perm_gen += 1;
        let Some(Item::Permission { request_id, tool, input, suggestions, state, .. }) = self.items.get_mut(index)
        else {
            return Vec::new();
        };
        if *state != PermState::Pending {
            return Vec::new();
        }
        let rules = if answer == Answer::Project { project::suggested_rules(suggestions) } else { Vec::new() };
        let mut plan_mode = None;
        let (response, new_state) = match answer {
            Answer::Deny(feedback) if feedback.trim().is_empty() => (
                json!({ "behavior": "deny", "message": "The user doesn't want to proceed with this tool use.", "interrupt": true }),
                PermState::Denied("denied".into()),
            ),
            Answer::Deny(feedback) => (
                json!({
                    "behavior": "deny",
                    "message": format!("The user doesn't want to proceed with this tool use. They said: {}", feedback.trim()),
                }),
                PermState::Denied(format!("denied: {}", feedback.trim())),
            ),
            Answer::Once => {
                (json!({ "behavior": "allow", "updatedInput": input }), PermState::Allowed("allowed".into()))
            }
            Answer::Session | Answer::Project => {
                // Kynex keeps project rules itself, so the CLI never writes them to its settings files.
                let mut updates = suggestions.clone();
                for u in updates.as_array_mut().into_iter().flatten() {
                    u["destination"] = json!("session");
                }
                let note = if rules.is_empty() { "allowed for this session" } else { "always allowed in this project" };
                (
                    json!({ "behavior": "allow", "updatedInput": input, "updatedPermissions": updates }),
                    PermState::Allowed(note.into()),
                )
            }
            Answer::Server => {
                // `mcp__<server>` with no tool covers all of that server's tools.
                let rule = crate::claude::mcp::server_rule(tool).unwrap_or_else(|| tool.clone());
                let updates = json!([{
                    "type": "addRules",
                    "rules": [{ "toolName": rule }],
                    "behavior": "allow",
                    "destination": "session",
                }]);
                let label = crate::claude::mcp::split(tool)
                    .map_or_else(|| tool.clone(), |(s, _)| crate::claude::mcp::server_label(s));
                (
                    json!({ "behavior": "allow", "updatedInput": input, "updatedPermissions": updates }),
                    PermState::Allowed(format!("all {label} actions allowed this session")),
                )
            }
            Answer::Plan(mode) => {
                plan_mode = Some(mode);
                (
                    json!({ "behavior": "allow", "updatedInput": input }),
                    PermState::Allowed(format!("plan approved · {}", mode.label())),
                )
            }
            Answer::Questions(answers) => {
                let note = answers.values().filter_map(Value::as_str).collect::<Vec<_>>().join(" · ");
                let mut updated = input.clone();
                updated["answers"] = Value::Object(answers);
                (
                    json!({ "behavior": "allow", "updatedInput": updated }),
                    PermState::Allowed(format!("answered: {note}")),
                )
            }
        };
        let approves_plan =
            matches!(new_state, PermState::Allowed(_)) && tool.replace(' ', "").eq_ignore_ascii_case("ExitPlanMode");
        *state = new_state;
        let msg = json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id.clone(), "response": response }
        });
        self.write(msg);
        // An approved plan means "go build it": in the mode picked with it, else
        // leave plan mode for the project's default, or Auto if that's Plan itself.
        if let Some(mode) = plan_mode {
            self.set_mode(mode);
        } else if approves_plan && self.mode == project::Mode::Plan {
            let default = match self.kind {
                Kind::Code => project::ClaudeSettings::load(self.settings_folder()).mode,
                Kind::Chat => project::Mode::default(),
            };
            self.set_mode(if default == project::Mode::Plan { project::Mode::Auto } else { default });
        }
        rules
    }
}
