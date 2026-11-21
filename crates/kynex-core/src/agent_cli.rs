//! Pure interactive CLI providers for `agent_cli` panes.
//!
//! Every provider runs its own native interactive CLI directly inside Kynex's GPU terminal
//! (preserving each CLI's TUI, permissions, slash commands, MCP servers, and auto-updater).
//! OpenRouter overrides are supported specifically for **Claude Code** (`claude-openrouter`)
//! and **Antigravity** (`agy-openrouter`), using the API key stored in the OS keychain.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::agent_panes::{self, AgentRule};
use crate::control::agent as control_agent;
use crate::hooks;
use crate::layout::PaneId;
use crate::provider::{DEFAULT_ANTIGRAVITY_MODEL, DEFAULT_OPENROUTER_MODEL, Launch, OpenRouterSettings};
use crate::terminal::{ShellKind, ShellSpec};
use crate::user_commands;

pub const CLAUDE: &str = "claude";
pub const CLAUDE_OPENROUTER: &str = "claude-openrouter";
pub const AGY: &str = "agy";
pub const AGY_OPENROUTER: &str = "agy-openrouter";
pub const GROK: &str = "grok";

/// One selectable provider shown in the New Session provider picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderEntry {
    pub id: String,
    pub label: String,
    pub short_badge: String,
    pub description: String,
    pub uses_openrouter: bool,
    pub installed: bool,
    pub builtin: bool,
}

/// Parameters for launching an interactive CLI inside an `agent_cli` pane.
pub struct LaunchRequest<'a> {
    pub pane_id: PaneId,
    /// Stable live-agent instance id (`KYNEX_SESSION`), survives swapping panes.
    pub instance_id: u64,
    pub provider: &'a str,
    pub cwd: &'a Path,
    pub resume_id: Option<&'a str>,
    pub model: Option<&'a str>,
    pub openrouter_key: Option<&'a str>,
    pub openrouter_cap: u32,
    pub control_addr: Option<&'a str>,
    pub rules: &'a [AgentRule],
}

/// Normalizes legacy provider names (from older `sessions.json` files) to canonical provider IDs.
pub fn normalize_provider_id(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "anthropic" | "claude" => CLAUDE.into(),
        "openrouter" | "openrouter-chat" | "claude-openrouter" => CLAUDE_OPENROUTER.into(),
        "antigravity" | "agy-openrouter" => AGY_OPENROUTER.into(),
        "agy" => AGY.into(),
        "grok" => GROK.into(),
        other => other.to_string(),
    }
}

/// Whether `provider` uses the saved OpenRouter key (Claude or Antigravity override).
pub fn uses_openrouter(provider: &str) -> bool {
    matches!(provider, CLAUDE_OPENROUTER | AGY_OPENROUTER)
}

/// Human-readable name for a provider ID.
pub fn label(provider: &str, rules: &[AgentRule]) -> String {
    match provider {
        CLAUDE => "Claude Code".into(),
        CLAUDE_OPENROUTER => "Claude Code (OpenRouter)".into(),
        AGY => "Antigravity".into(),
        AGY_OPENROUTER => "Antigravity (OpenRouter)".into(),
        GROK => "Grok Build".into(),
        other => rules.iter().find(|r| r.id == other).map_or_else(|| other.to_string(), |r| r.name.clone()),
    }
}

/// Short badge text for session rows and pane headers.
pub fn badge(provider: &str, rules: &[AgentRule]) -> String {
    match provider {
        CLAUDE => "Claude".into(),
        CLAUDE_OPENROUTER => "Claude · OR".into(),
        AGY => "Agy".into(),
        AGY_OPENROUTER => "Agy · OR".into(),
        GROK => "Grok".into(),
        other => rules.iter().find(|r| r.id == other).map_or_else(|| other.to_string(), |r| r.name.clone()),
    }
}

/// The `agent_rules` ID used for screen-pattern tracking of `provider`.
pub fn rule_id(provider: &str) -> &str {
    match provider {
        CLAUDE | CLAUDE_OPENROUTER => CLAUDE,
        AGY | AGY_OPENROUTER => AGY,
        other => other,
    }
}

/// Lists all built-in providers plus any additional `agent_rules` CLIs (Codex, Copilot,
/// Cursor, opencode, custom user rules). Checks installation status on disk/PATH; call off
/// the UI thread.
pub fn catalog(rules: &[AgentRule], has_openrouter_key: bool) -> Vec<ProviderEntry> {
    let claude_ok = crate::cli::program().is_file();
    let agy_ok = crate::antigravity::installed();
    let grok_ok = crate::grok::installed();

    let mut out = vec![
        ProviderEntry {
            id: CLAUDE.into(),
            label: "Claude Code".into(),
            short_badge: "Claude".into(),
            description: "Pure `claude` CLI with your Anthropic login".into(),
            uses_openrouter: false,
            installed: claude_ok,
            builtin: true,
        },
        ProviderEntry {
            id: CLAUDE_OPENROUTER.into(),
            label: "Claude Code (OpenRouter)".into(),
            short_badge: "Claude · OR".into(),
            description: if has_openrouter_key {
                "`claude` CLI routed through your OpenRouter key".into()
            } else {
                "Add an OpenRouter key in Settings → Providers".into()
            },
            uses_openrouter: true,
            installed: claude_ok,
            builtin: true,
        },
        ProviderEntry {
            id: AGY.into(),
            label: "Antigravity".into(),
            short_badge: "Agy".into(),
            description: "Pure `agy` CLI with your Google account".into(),
            uses_openrouter: false,
            installed: agy_ok,
            builtin: true,
        },
        ProviderEntry {
            id: AGY_OPENROUTER.into(),
            label: "Antigravity (OpenRouter)".into(),
            short_badge: "Agy · OR".into(),
            description: if has_openrouter_key {
                "`agy` CLI routed through your OpenRouter key".into()
            } else {
                "Add an OpenRouter key in Settings → Providers".into()
            },
            uses_openrouter: true,
            installed: agy_ok,
            builtin: true,
        },
        ProviderEntry {
            id: GROK.into(),
            label: "Grok Build".into(),
            short_badge: "Grok".into(),
            description: "Pure `grok` CLI with your xAI login".into(),
            uses_openrouter: false,
            installed: grok_ok,
            builtin: true,
        },
    ];

    for r in rules {
        if matches!(r.id.as_str(), CLAUDE | GROK | AGY) {
            continue;
        }
        let installed = r.argv.first().is_some_and(|p| user_commands::find_program(p, Path::new(".")).is_ok());
        out.push(ProviderEntry {
            id: r.id.clone(),
            label: r.name.clone(),
            short_badge: r.name.clone(),
            description: format!("`{}` CLI in terminal", r.argv.join(" ")),
            uses_openrouter: false,
            installed,
            builtin: false,
        });
    }
    out
}

/// Builds a [`ShellSpec`] to launch the requested provider CLI inside a PTY. Blocks on PATH
/// lookup and writing cached hook config files; call off the UI thread.
pub fn build_spec(req: &LaunchRequest<'_>) -> Result<ShellSpec, String> {
    let provider = normalize_provider_id(req.provider);
    let mut env = base_kynex_env(req.pane_id, req.instance_id, &provider, req.cwd, req.control_addr);
    let mut env_remove: Vec<String> = Vec::new();
    let title = label(&provider, req.rules);

    let (program, args) = match provider.as_str() {
        CLAUDE => {
            let program = resolve_claude()?;
            let launch = Launch::Anthropic;
            let diff = launch.env(std::env::vars_os());
            env.extend(diff.set.into_iter().map(|(k, v)| (k.to_string(), v)));
            env_remove.extend(os_strings(diff.remove));
            let mut args = Vec::new();
            if let Some(settings_path) = hooks::write_claude_settings(CLAUDE, None) {
                args.push("--settings".into());
                args.push(settings_path.to_string_lossy().into_owned());
            }
            if let Some(id) = req.resume_id.filter(|s| !s.trim().is_empty()) {
                args.push("--resume".into());
                args.push(id.to_string());
            }
            (program, args)
        }
        CLAUDE_OPENROUTER => {
            let key = req
                .openrouter_key
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .ok_or("No OpenRouter key is saved. Add one in Settings → Providers.")?;
            let program = resolve_claude()?;
            let model = req.model.map(str::trim).filter(|m| !m.is_empty()).unwrap_or(DEFAULT_OPENROUTER_MODEL);
            let launch = Launch::OpenRouter {
                key: key.to_string(),
                settings: OpenRouterSettings { model: model.to_string(), max_output_tokens: req.openrouter_cap },
            };
            let diff = launch.env(std::env::vars_os());
            env.extend(diff.set.into_iter().map(|(k, v)| (k.to_string(), v)));
            env_remove.extend(os_strings(diff.remove));
            let mut args = Vec::new();
            if let Some(settings_path) = hooks::write_claude_settings(CLAUDE_OPENROUTER, launch.settings_env()) {
                args.push("--settings".into());
                args.push(settings_path.to_string_lossy().into_owned());
            }
            if let Some(id) = req.resume_id.filter(|s| !s.trim().is_empty()) {
                args.push("--resume".into());
                args.push(id.to_string());
            }
            (program, args)
        }
        AGY => {
            let program = resolve_agy()?;
            hooks::ensure_agy_hooks();
            // Ensure pure `agy` uses Google login rather than any leftover gateway vars.
            env_remove.extend([
                "AGY_LLM_GATEWAY_URL".into(),
                "AGY_LLM_GATEWAY_WIRE_PROTOCOL".into(),
                "AGY_LLM_GATEWAY_API_KEY".into(),
                "AGY_ACP_ENABLE_GATEWAY_AUTH".into(),
            ]);
            let mut args = Vec::new();
            if let Some(id) = req.resume_id.filter(|s| !s.trim().is_empty()) {
                args.push("--conversation".into());
                args.push(id.to_string());
            }
            (program, args)
        }
        AGY_OPENROUTER => {
            let key = req
                .openrouter_key
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .ok_or("Antigravity (OpenRouter) needs an OpenRouter key. Add one in Settings → Providers.")?;
            let program = resolve_agy()?;
            hooks::ensure_agy_hooks();
            env.extend(crate::antigravity::gateway_env(key).into_iter().map(|(k, v)| (k.to_string(), v)));
            env_remove.push("GEMINI_API_KEY".into());
            let mut args = Vec::new();
            let model = req.model.map(str::trim).filter(|m| !m.is_empty()).unwrap_or(DEFAULT_ANTIGRAVITY_MODEL);
            args.push("--model".into());
            args.push(model.to_string());
            if let Some(id) = req.resume_id.filter(|s| !s.trim().is_empty()) {
                args.push("--conversation".into());
                args.push(id.to_string());
            }
            (program, args)
        }
        GROK => {
            let program = resolve_grok()?;
            let mut args = Vec::new();
            if let Some(id) = req.resume_id.filter(|s| !s.trim().is_empty()) {
                args.push("--resume".into());
                args.push(id.to_string());
            }
            (program, args)
        }
        other => {
            let rule = req
                .rules
                .iter()
                .find(|r| r.id == other)
                .ok_or_else(|| format!("There's no agent rule \"{other}\" (Kynex's agents folder)."))?;
            let argv = agent_panes::resume_argv(rule, req.resume_id);
            let program = user_commands::find_program(&argv[0], req.cwd)?;
            (program, argv[1..].to_vec())
        }
    };

    Ok(ShellSpec {
        kind: ShellKind::default_shell(),
        program,
        args,
        env,
        env_remove,
        title: Some(title),
        history: None,
    })
}

fn resolve_claude() -> Result<PathBuf, String> {
    let p = crate::cli::program();
    if p.is_file() {
        Ok(p)
    } else {
        user_commands::find_program("claude", Path::new("."))
            .map_err(|_| "Claude Code isn't installed. Install it from Settings → Providers.".into())
    }
}

fn resolve_agy() -> Result<PathBuf, String> {
    let p = crate::antigravity::program();
    if p.is_file() {
        Ok(p)
    } else {
        user_commands::find_program("agy", Path::new("."))
            .map_err(|_| "Antigravity CLI (`agy`) isn't installed. Install it from Settings → Providers.".into())
    }
}

fn resolve_grok() -> Result<PathBuf, String> {
    let p = crate::grok::program();
    if p.is_file() {
        Ok(p)
    } else {
        user_commands::find_program("grok", Path::new("."))
            .map_err(|_| "Grok Build (`grok`) isn't installed. Install it from Settings → Providers.".into())
    }
}

fn base_kynex_env(
    pane_id: PaneId,
    instance_id: u64,
    provider: &str,
    cwd: &Path,
    control_addr: Option<&str>,
) -> Vec<(String, String)> {
    let exe = std::env::current_exe().ok();
    base_kynex_env_with(
        pane_id,
        instance_id,
        provider,
        cwd,
        control_addr,
        control_agent::ensure_shims(),
        exe.as_deref(),
    )
}

fn base_kynex_env_with(
    pane_id: PaneId,
    instance_id: u64,
    provider: &str,
    cwd: &Path,
    control_addr: Option<&str>,
    shim_dir: Option<&Path>,
    hook_bin: Option<&Path>,
) -> Vec<(String, String)> {
    let mut env = vec![
        (control_agent::PANE_VAR.into(), pane_id.to_string()),
        (control_agent::SESSION_VAR.into(), instance_id.to_string()),
        ("KYNEX_PROVIDER".into(), provider.to_string()),
        (control_agent::WORKSPACE_VAR.into(), cwd.to_string_lossy().into_owned()),
    ];
    if let Some(addr) = control_addr.filter(|a| !a.is_empty()) {
        env.push((control_agent::CONTROL_VAR.into(), addr.to_string()));
    }
    if let Some(bin) = hook_bin {
        env.push((control_agent::HOOK_BIN_VAR.into(), bin.to_string_lossy().into_owned()));
    }
    if let Some(dir) = shim_dir {
        let fresh_path = crate::fresh_env::vars()
            .into_iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
            .map(|(_, v)| v)
            .or_else(|| std::env::var_os("PATH"));
        let joined = control_agent::path_with(dir, fresh_path.as_deref());
        env.push(("PATH".into(), joined.to_string_lossy().into_owned()));
    }
    env
}

fn os_strings(list: Vec<OsString>) -> Vec<String> {
    list.into_iter().filter_map(|s| s.into_string().ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_legacy_provider_names() {
        assert_eq!(normalize_provider_id("anthropic"), CLAUDE);
        assert_eq!(normalize_provider_id("openrouter"), CLAUDE_OPENROUTER);
        assert_eq!(normalize_provider_id("openrouter-chat"), CLAUDE_OPENROUTER);
        assert_eq!(normalize_provider_id("antigravity"), AGY_OPENROUTER);
        assert_eq!(normalize_provider_id("agy"), AGY);
        assert_eq!(normalize_provider_id("grok"), GROK);
        assert_eq!(normalize_provider_id("codex"), "codex");
    }

    #[test]
    fn openrouter_overrides_require_key() {
        let req = LaunchRequest {
            pane_id: 7,
            instance_id: 42,
            provider: CLAUDE_OPENROUTER,
            cwd: Path::new("."),
            resume_id: None,
            model: Some("qwen/qwen3-coder"),
            openrouter_key: None,
            openrouter_cap: 8000,
            control_addr: None,
            rules: &[],
        };
        assert!(build_spec(&req).unwrap_err().contains("No OpenRouter key"));

        let agy_req = LaunchRequest { provider: AGY_OPENROUTER, ..req };
        assert!(build_spec(&agy_req).unwrap_err().contains("OpenRouter key"));
    }

    #[test]
    fn base_env_carries_instance_id_control_address_and_hook_bin() {
        let env = base_kynex_env_with(
            3,
            99,
            CLAUDE,
            Path::new("/work/app"),
            Some(r"\\.\pipe\kynex-test"),
            None,
            Some(Path::new("/bin/kynex")),
        );
        assert!(env.iter().any(|(k, v)| k == control_agent::PANE_VAR && v == "3"));
        assert!(env.iter().any(|(k, v)| k == control_agent::SESSION_VAR && v == "99"));
        assert!(env.iter().any(|(k, v)| k == control_agent::CONTROL_VAR && v == r"\\.\pipe\kynex-test"));
        assert!(env.iter().any(|(k, v)| k == control_agent::HOOK_BIN_VAR && v == "/bin/kynex"));
    }

    #[test]
    fn agy_openrouter_env_keeps_auto_update_enabled() {
        let env = crate::antigravity::gateway_env("sk-or-test");
        assert!(!env.iter().any(|(k, _)| *k == "AGY_CLI_DISABLE_AUTO_UPDATE"));
        assert!(env.iter().any(|(k, v)| *k == "AGY_LLM_GATEWAY_API_KEY" && v == "sk-or-test"));
    }
}
