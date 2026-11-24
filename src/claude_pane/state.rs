//! What the app gives a [`super::ClaudePane`] to start with, and what the pane tells
//! the app back (`src/app/claude_chat.rs`).

use std::path::PathBuf;

use kynex_core::agent_panes::AgentState;
use kynex_core::provider::Launch;

/// What a [`super::ClaudePane`] starts with.
#[derive(Clone, Debug)]
pub struct ClaudeChatConfig {
    pub workspace: PathBuf,
    pub model: Option<String>,
    pub resume_session_id: Option<String>,
    pub fork_from: Option<String>,
    /// The title it had, kept with the pane: a resumed session keeps it.
    pub title: Option<String>,
    /// The history of `workspace`, shared with the other panes on it.
    pub history: super::SharedHistory,
    pub launch: Launch,
    /// The OpenRouter settings (My models, the default, no-tools models), for the model picker.
    pub openrouter: kynex_core::openrouter::OpenRouterConfig,
    pub extra_env: Vec<(String, String)>,
}

/// Events a [`super::ClaudePane`] sends to `KynexApp` (`src/app/claude_chat.rs`).
#[derive(Clone, Debug)]
pub enum ClaudeChatEvent {
    StatusChanged {
        state: AgentState,
        detail: String,
    },
    SessionId(String),
    TitleChanged(String),
    TurnFinished,
    ForkRequested {
        from_session_id: String,
    },
    RunInTerminal(String),
    /// A file's whole diff, from the turn recap.
    OpenFileChanges {
        cwd: PathBuf,
        path: PathBuf,
    },
    /// A note for the app's toasts.
    Toast(kynex_core::toasts::Toast),
    /// The progress toast titled `progress` is done: it closes, and `outcome`
    /// goes up instead.
    ToastDone {
        progress: String,
        outcome: Option<kynex_core::toasts::Toast>,
    },
    /// `/clear`: a new session in this pane.
    NewSession,
    /// `/resume`: the sessions list.
    ShowSessions,
    /// A page of the app's settings.
    OpenSettings(SettingsPage),
    /// The project's Claude settings changed (effort, thinking, model): the
    /// other panes on it pick them up.
    ProjectSettingsChanged(PathBuf),
    /// The model pinned to this session (`None`: it follows its project's),
    /// kept with the pane so it comes back on a restart.
    ModelPinned(Option<String>),
    /// A background task ended (or a Monitor saw something): a toast if the
    /// pane isn't in sight.
    TaskEnded(kynex_core::claude::tasks::Notice),
    /// The account's usage limits, as the CLI reported them, for the footer.
    LimitsReported(kynex_core::claude::usage::Limits),
    /// A request failed because this OpenRouter model takes no tool calls.
    NoTools(String),
}

/// The app settings pages a chat pane opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    Providers,
    Keyboard,
}
