//! The economy flow for chief workers: Opus plans, Haiku executes. The
//! worker's own session is a cheap coordinator that hands a task to three
//! subagents (`--agents`): a `planner` that turns it into an exact plan, an
//! `executor` that carries the plan out, and a `reviewer` that checks the
//! diff. Planning and review are short and read-only, so they take the strong,
//! pricey model; execution, the long loop of reading, editing and rerunning
//! that most of a session's tokens go to, takes the fast, cheap one.

use serde_json::json;

/// The model of the worker's own session, which only coordinates.
pub const COORDINATOR_MODEL: &str = "haiku";
/// The model that plans and reviews.
pub const THINKER_MODEL: &str = "opus";
/// The model that executes.
pub const DOER_MODEL: &str = "haiku";

/// The subagents, as `claude --agents` takes them.
pub fn agents() -> String {
    json!({
        "planner": {
            "description": "Turns a task into an exact, step-by-step plan. Reads the code, never changes it. \
                            Use it before any change bigger than a small, obvious one.",
            "prompt": "You plan work for a fast, literal coder. You never change anything.\n\
                       Read what you need, and no more than that. Then answer with:\n\
                       GOAL: the task in a sentence.\n\
                       STEPS: numbered; each names the file and the exact change (the function, the lines, the \
                       code itself when short) and how to check it worked.\n\
                       RISKS: what could go wrong or is unknown.\n\
                       DONE MEANS: how to tell it's finished.\n\
                       Be exact enough that the coder never has to use judgment. If only the user can decide \
                       something, say QUESTION: and what you need decided.",
            "tools": ["Read", "Grep", "Glob"],
            "model": THINKER_MODEL,
        },
        "executor": {
            "description": "Carries out a plan exactly: makes the changes, runs the checks, reports. Fast and cheap. \
                            Give it the plan word for word.",
            "prompt": "You carry out the plan you're given, step by step, exactly.\n\
                       Make the changes, run the checks the plan names, and report briefly: what you did for each \
                       step, what the checks said, and anything that didn't match the plan. If something doesn't \
                       match, stop and say what's different; don't improvise around it.",
            "model": DOER_MODEL,
        },
        "reviewer": {
            "description": "Reviews a finished change against the task and the plan. Reads, never changes. \
                            Use it once the work is done.",
            "prompt": "You review a finished change. Look at what changed with read-only git commands (git status, \
                       git diff), read the changed code, and check it against the task and the plan.\n\
                       Answer OK, or a numbered list of problems with the file and line of each. Never edit.",
            "tools": ["Read", "Grep", "Glob", "Bash"],
            "model": THINKER_MODEL,
        },
    })
    .to_string()
}

/// How the coordinator uses them, added to its instructions. Each planner or
/// reviewer call has a fixed cost (it starts a strong model on the project), so
/// they're for work big enough to repay it.
pub const COORDINATOR_PROMPT: &str = "How you work: you coordinate. You have three subagents and you don't do \
the work yourself.
- planner: a strong, pricey model that reads and plans. executor: a fast, cheap model that does the work. \
reviewer: a strong, pricey model that checks the result.
1. A small or clear task (one or two files, and you can say exactly what to change): give it straight to \
the executor, with what it needs. Don't call the planner or the reviewer for it.
2. A task that spans several files, needs a design decision, or isn't clear: ask the planner for a plan \
(the goal, the constraints, where to look), then hand the plan to the executor word for word, a step or a \
few at a time. Each call starts fresh, so include what it needs.
3. If the executor reports something that didn't match, send it to the planner to change that step for a \
big task, or fix the instruction yourself for a small one. Don't guess.
4. Ask the reviewer only for a large or risky change, and send any problems to the executor to fix. \
Planner and reviewer calls cost much more than the executor's, so use them where they pay.
5. Then end your turn with the report described above. Keep what you hold small: the plan, the results \
and your decisions.";
#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn the_agents_are_valid_and_split_by_price() {
        let a: Value = serde_json::from_str(&agents()).unwrap();
        for name in ["planner", "executor", "reviewer"] {
            assert!(a[name]["description"].as_str().is_some_and(|s| !s.is_empty()), "{name}");
            assert!(a[name]["prompt"].as_str().is_some_and(|s| !s.is_empty()), "{name}");
        }
        assert_eq!(a["planner"]["model"], THINKER_MODEL);
        assert_eq!(a["reviewer"]["model"], THINKER_MODEL);
        assert_eq!(a["executor"]["model"], DOER_MODEL);
        // The pricey ones can't change anything; the executor has every tool.
        let tools = |n: &str| -> Vec<String> {
            a[n]["tools"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(String::from)).collect()
        };
        assert!(!tools("planner").iter().any(|t| ["Edit", "Write", "Bash"].contains(&t.as_str())));
        assert!(!tools("reviewer").iter().any(|t| ["Edit", "Write"].contains(&t.as_str())));
        assert!(a["executor"]["tools"].is_null());
    }

    /// The flow end to end: Haiku coordinates, Opus plans, Haiku executes, and
    /// the turn's usage shows both models. Costs a few cents of Opus. Run with
    /// `cargo test -- --ignored the_flow_uses_both_models --nocapture`.
    #[test]
    #[ignore = "needs the claude CLI and uses some quota, Opus included"]
    fn the_flow_uses_both_models() {
        let dir = std::env::temp_dir().join(format!("kynex-economy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = std::process::Command::new(crate::cli::program())
            .args([
                "-p",
                "Write calc.py with add, sub, mul and div functions (div raises ValueError on zero) and \
                 test_calc.py with a test for each. It's a task with several steps: use your planner first, \
                 then your executor.",
                "--model",
                COORDINATOR_MODEL,
                "--output-format",
                "json",
                "--permission-mode",
                "acceptEdits",
                "--agents",
                &agents(),
                "--append-system-prompt",
                COORDINATOR_PROMPT,
            ])
            .current_dir(&dir)
            .output()
            .unwrap();
        let result: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!("{e}: {} {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
        });
        let usage = result["modelUsage"].as_object().expect("modelUsage");
        let cost = |family: &str| -> f64 {
            usage.iter().filter(|(m, _)| m.contains(family)).map(|(_, u)| u["costUSD"].as_f64().unwrap_or(0.0)).sum()
        };
        println!("models: {:?}", usage.keys().collect::<Vec<_>>());
        println!("opus ${:.4}, haiku ${:.4}, total ${:.4}", cost("opus"), cost("haiku"), result["total_cost_usd"]);
        assert!(cost("opus") > 0.0, "the planner never ran on Opus");
        assert!(cost("haiku") > 0.0);
        assert!(dir.join("calc.py").exists(), "{}", result["result"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The CLI takes the agents. Run with `cargo test -- --ignored the_cli_takes_the_agents`.
    #[test]
    #[ignore = "needs the claude CLI and uses a little quota"]
    fn the_cli_takes_the_agents() {
        let out = std::process::Command::new(crate::cli::program())
            .args(["-p", "Reply with the single word OK.", "--model", COORDINATOR_MODEL, "--agents", &agents()])
            .current_dir(std::env::temp_dir())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success() && text.contains("OK"), "{text} {}", String::from_utf8_lossy(&out.stderr));
    }
}
