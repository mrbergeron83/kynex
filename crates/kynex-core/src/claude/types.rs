//! Strongly-typed wire events, configuration, and session metadata for the
//! native Claude Code `stream-json` control protocol.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::provider::Launch;

/// Zero-polling notification callback invoked whenever new [`ClaudeEvent`]s
/// arrive from the subprocess.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// How a Claude Code process starts or continues a session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SessionStart {
    #[default]
    New,
    Resume(String),
    Fork(String),
}

/// Permission mode supported by `claude --permission-mode` and
/// `set_permission_mode` control requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionMode {
    #[default]
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "dontAsk")]
    DontAsk,
    #[serde(rename = "bypassPermissions")]
    BypassPermissions,
}

impl PermissionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Auto => "auto",
            Self::Plan => "plan",
            Self::DontAsk => "dontAsk",
            Self::BypassPermissions => "bypassPermissions",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s.trim() {
            "default" => Some(Self::Default),
            "acceptEdits" | "accept_edits" => Some(Self::AcceptEdits),
            "auto" => Some(Self::Auto),
            "plan" => Some(Self::Plan),
            "dontAsk" | "dont_ask" => Some(Self::DontAsk),
            "bypassPermissions" | "bypass_permissions" => Some(Self::BypassPermissions),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Default => "Ask before edits",
            Self::AcceptEdits => "Accept edits",
            Self::Auto => "Auto",
            Self::Plan => "Plan mode",
            Self::DontAsk => "Don't ask",
            Self::BypassPermissions => "Bypass permissions",
        }
    }

    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Default => "Ask",
            Self::AcceptEdits => "Accept edits",
            Self::Auto => "Auto",
            Self::Plan => "Plan",
            Self::DontAsk => "Don't ask",
            Self::BypassPermissions => "Bypass",
        }
    }

    /// Next mode when cycling via `Shift+Tab` in the composer.
    pub const fn cycle_next(self, supports_auto: bool) -> Self {
        match self {
            Self::Default => Self::AcceptEdits,
            Self::AcceptEdits => {
                if supports_auto {
                    Self::Auto
                } else {
                    Self::Plan
                }
            }
            Self::Auto => Self::Plan,
            Self::Plan | Self::DontAsk | Self::BypassPermissions => Self::Default,
        }
    }
}

/// Reasoning effort level supported by `claude --effort` and
/// `apply_flag_settings` (`effortLevel`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
}

impl EffortLevel {
    pub const ALL: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::XHigh, Self::Max];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" | "extra_high" => Some(Self::XHigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::XHigh => "Extra high",
            Self::Max => "Max",
        }
    }
}

/// One model advertised by `claude`'s `initialize` control response.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelInfo {
    pub value: String,
    pub resolved_model: Option<String>,
    pub display_name: String,
    pub description: String,
    pub supports_effort: bool,
    pub supported_effort_levels: Vec<EffortLevel>,
    pub supports_adaptive_thinking: bool,
    pub supports_fast_mode: bool,
    pub supports_auto_mode: bool,
    pub supports_thinking_off: bool,
}

/// A slash command or skill advertised by `claude`'s `initialize` response.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub aliases: Vec<String>,
    pub builtin: bool,
}

/// A subagent definition advertised by `claude`'s `initialize` response.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentDef {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
}

/// Account metadata advertised by `claude`'s `initialize` response.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountInfo {
    pub email: Option<String>,
    pub organization: Option<String>,
    pub subscription_type: Option<String>,
    pub api_provider: Option<String>,
}

/// Full handshake payload parsed from the `initialize` `control_response`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InitInfo {
    pub models: Vec<ModelInfo>,
    pub commands: Vec<SlashCommand>,
    pub agents: Vec<AgentDef>,
    pub account: Option<AccountInfo>,
    pub current_permission_mode: Option<PermissionMode>,
    pub output_style: Option<String>,
    pub available_output_styles: Vec<String>,
    pub fast_mode_state: Option<String>,
    pub claude_code_version: Option<String>,
}

/// Metadata emitted by a `system` / `init` frame once the session starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemInitInfo {
    pub session_id: String,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub tools: Vec<String>,
    pub slash_commands: Vec<String>,
    pub cwd: Option<String>,
    pub claude_code_version: Option<String>,
    /// `ultrathink` in a prompt raises that turn's effort on this model.
    pub ultrathink_available: bool,
}

/// Outgoing content block sent inside a `user` turn or mid-turn steer message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptBlock {
    Text(String),
    Image { media_type: String, base64: String },
    FileContext { path: String, content: String },
}

/// An inline image returned inside a `tool_result` content array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolResultImage {
    pub media_type: String,
    pub base64: String,
}

/// One selectable option in an `AskUserQuestion` prompt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskQuestionOption {
    pub label: String,
    pub description: String,
}

/// One structured question in an `AskUserQuestion` tool request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskQuestion {
    pub question: String,
    pub header: String,
    pub options: Vec<AskQuestionOption>,
    pub multi_select: bool,
}

/// Classified `can_use_tool` control request.
#[derive(Clone, Debug, PartialEq)]
pub enum PermissionKind {
    ToolApproval {
        tool_name: String,
        input: Value,
        permission_suggestions: Vec<Value>,
        decision_reason: Option<String>,
        decision_reason_type: Option<String>,
        decision_reason_rule_source: Option<String>,
        blocked_path: Option<String>,
        description: Option<String>,
    },
    AskUserQuestion {
        questions: Vec<AskQuestion>,
        raw_input: Value,
    },
    ExitPlanMode {
        plan_text: Option<String>,
        plan_path: Option<String>,
        allowed_prompts: Vec<Value>,
        raw_input: Value,
    },
}

/// An incoming permission / interactive card request from the CLI.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionRequest {
    pub request_id: String,
    pub tool_use_id: Option<String>,
    pub kind: PermissionKind,
}

/// The user's response to a [`PermissionRequest`].
#[derive(Clone, Debug, PartialEq)]
pub enum PermissionAnswer {
    AllowOnce,
    AllowSession(Vec<Value>),
    Deny { message: Option<String>, interrupt: bool },
    Questions(Map<String, Value>),
    ApprovePlan { next_mode: PermissionMode },
}

/// Token counts reported on a message or turn result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

impl TokenUsage {
    /// Total prompt tokens occupying the model's context window for this step.
    pub const fn context_input_tokens(self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }

    pub const fn is_empty(self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.cache_creation_input_tokens == 0
            && self.cache_read_input_tokens == 0
    }
}

/// Per-model usage & context-window breakdown from a `result` frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cost_usd: f64,
    pub context_window: u64,
    pub max_output_tokens: u64,
}

/// Final turn summary parsed from a `result` frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnResult {
    pub session_id: Option<String>,
    pub subtype: String,
    pub total_cost_usd: f64,
    pub duration_ms: u64,
    pub duration_api_ms: u64,
    pub num_turns: u32,
    pub usage: TokenUsage,
    pub model_usage: Vec<ModelUsage>,
    pub context_window: Option<u64>,
    pub is_error: bool,
    pub result_text: Option<String>,
    pub errors: Vec<String>,
}

/// Forwarded content from a subagent (`parent_tool_use_id` present).
#[derive(Clone, Debug, PartialEq)]
pub enum SubagentMessage {
    Text(String),
    Thought(String),
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool, images: Vec<ToolResultImage> },
}

/// An event attributed to a running subagent tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct SubagentEvent {
    pub parent_tool_use_id: String,
    pub message: SubagentMessage,
}

/// Live progress update for a background/subagent task.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskProgress {
    pub task_id: String,
    pub tool_use_id: Option<String>,
    pub description: Option<String>,
    pub last_tool_name: Option<String>,
    pub summary: Option<String>,
    pub total_tokens: Option<u64>,
    pub tool_uses: Option<u32>,
    pub duration_ms: Option<u64>,
}

/// Status of one configured MCP server from `mcp_status`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpServerInfo {
    pub name: String,
    pub status: String,
    pub error: Option<String>,
}

/// Rate-limit notice from `rate_limit_event`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RateLimitInfo {
    pub status: String,
    pub resets_at: Option<String>,
    pub rate_limit_type: Option<String>,
    pub message: Option<String>,
}

/// Every event emitted by [`crate::claude::ClaudeClient`].
#[derive(Clone, Debug, PartialEq)]
pub enum ClaudeEvent {
    Initialized(InitInfo),
    SystemInit(SystemInitInfo),
    StatusUpdate { status: Option<String>, permission_mode: Option<PermissionMode> },
    CompactBoundary { trigger: Option<String>, pre_tokens: Option<u64> },
    UserReplayed { uuid: String, text: String },
    TextDelta(String),
    ThoughtDelta(String),
    ToolInputDelta { id: String, name: String, partial_json: String },
    ToolCallStart { id: String, name: String, input: Value },
    ToolCallUpdate { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool, images: Vec<ToolResultImage> },
    MessageUsage { model: Option<String>, usage: TokenUsage },
    SubagentEvent(SubagentEvent),
    TaskStarted { task_id: String, tool_use_id: Option<String>, description: String, task_type: Option<String> },
    TaskProgress(TaskProgress),
    TaskNotification { task_id: String, tool_use_id: Option<String>, status: String, summary: Option<String> },
    PermissionRequested(PermissionRequest),
    PromptSuggestion(String),
    RateLimit(RateLimitInfo),
    McpStatus(Vec<McpServerInfo>),
    TurnFinished(TurnResult),
    ControlAck { request_id: String, response: Value },
    ControlError { request_id: String, error: String },
    Error(String),
    Exited { code: Option<i32>, stderr: String },
}

/// Configuration for spawning a [`crate::claude::ClaudeClient`] subprocess.
#[derive(Clone)]
pub struct SpawnConfig {
    pub cwd: PathBuf,
    pub session: SessionStart,
    pub launch: Launch,
    pub model: Option<String>,
    pub effort: Option<EffortLevel>,
    pub permission_mode: Option<PermissionMode>,
    pub settings_json: Option<String>,
    pub extra_env: Vec<(String, String)>,
    pub on_wake: Option<Wake>,
}

impl Default for SpawnConfig {
    fn default() -> Self {
        Self {
            cwd: PathBuf::new(),
            session: SessionStart::New,
            launch: Launch::Anthropic,
            model: None,
            effort: None,
            permission_mode: None,
            settings_json: None,
            extra_env: Vec::new(),
            on_wake: None,
        }
    }
}

impl fmt::Debug for SpawnConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpawnConfig")
            .field("cwd", &self.cwd)
            .field("session", &self.session)
            .field("launch", &self.launch)
            .field("model", &self.model)
            .field("effort", &self.effort)
            .field("permission_mode", &self.permission_mode)
            .field("settings_json", &self.settings_json.as_ref().map(|_| "<json>"))
            .field("extra_env_len", &self.extra_env.len())
            .field("on_wake", &self.on_wake.is_some())
            .finish()
    }
}
