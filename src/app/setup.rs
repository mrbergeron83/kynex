//! Background CLI discovery and update monitoring for Claude Code, Grok Build,
//! and Antigravity CLI (`agy`).
//!
//! No blocking setup screen: Kynex always opens straight into workspaces, and each
//! `agent_cli` pane's inline provider picker lets you launch or install any CLI.

use std::time::{Duration, SystemTime};

use gpui::{AppContext, Context};
use kynex_core::antigravity::setup as agy_setup;
use kynex_core::cli::{self, UpdateState};
use kynex_core::grok::setup as grok_setup;

use super::KynexApp;

pub(super) const CLI_CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// The native agent CLIs Kynex monitors for updates and can update in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ManagedCli {
    Claude,
    Grok,
    Antigravity,
}

impl ManagedCli {
    pub const ALL: [Self; 3] = [Self::Claude, Self::Grok, Self::Antigravity];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Grok => "grok",
            Self::Antigravity => "agy",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Grok => "Grok Build",
            Self::Antigravity => "Antigravity CLI",
        }
    }

    pub const fn update_cmd(self) -> &'static str {
        match self {
            Self::Claude => "claude update",
            Self::Grok => "grok update",
            Self::Antigravity => "agy update",
        }
    }

    pub fn is_installed(self) -> bool {
        match self {
            Self::Claude => cli::program().is_file(),
            Self::Grok => kynex_core::grok::installed(),
            Self::Antigravity => kynex_core::antigravity::installed(),
        }
    }

    pub fn installed_version(self) -> Result<String, String> {
        match self {
            Self::Claude => cli::installed_version(),
            Self::Grok => grok_setup::installed_version(),
            Self::Antigravity => agy_setup::installed_version(),
        }
    }

    pub fn check(self) -> Result<(String, String), String> {
        match self {
            Self::Claude => cli::check(),
            Self::Grok => grok_setup::check(),
            Self::Antigravity => agy_setup::check(),
        }
    }

    pub fn update(self) -> Result<String, String> {
        match self {
            Self::Claude => cli::update(),
            Self::Grok => grok_setup::update(),
            Self::Antigravity => agy_setup::update(),
        }
    }
}

/// The last version check for one CLI: running now, when, and why it failed.
#[derive(Default)]
pub(super) struct CliCheck {
    pub checking: bool,
    pub at: Option<SystemTime>,
    pub error: Option<String>,
}

/// Version and update status for one managed CLI.
pub(super) struct CliTracker {
    pub installed: bool,
    pub version: Option<String>,
    pub update: UpdateState,
    pub check: CliCheck,
}

impl Default for CliTracker {
    fn default() -> Self {
        Self { installed: false, version: None, update: UpdateState::Idle, check: CliCheck::default() }
    }
}

/// Update tracking for Claude Code, Grok Build, and Antigravity CLI.
#[derive(Default)]
pub(super) struct CliUpdates {
    pub claude: CliTracker,
    pub grok: CliTracker,
    pub agy: CliTracker,
}

impl CliUpdates {
    pub fn get(&self, cli: ManagedCli) -> &CliTracker {
        match cli {
            ManagedCli::Claude => &self.claude,
            ManagedCli::Grok => &self.grok,
            ManagedCli::Antigravity => &self.agy,
        }
    }

    pub fn get_mut(&mut self, cli: ManagedCli) -> &mut CliTracker {
        match cli {
            ManagedCli::Claude => &mut self.claude,
            ManagedCli::Grok => &mut self.grok,
            ManagedCli::Antigravity => &mut self.agy,
        }
    }

    pub fn any_available(&self) -> bool {
        ManagedCli::ALL.iter().any(|&c| matches!(self.get(c).update, UpdateState::Available { .. }))
    }
}

impl KynexApp {
    /// Background-probes installed CLIs (`claude`, `grok`, `agy`, elevation) once at startup
    /// so later UI renders never block on PATH lookup.
    pub(super) fn probe_clis(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let probed = cx
                .background_spawn(async {
                    kynex_core::grok::installed();
                    kynex_core::antigravity::installed();
                    kynex_core::terminal::elevated();
                    ManagedCli::ALL.map(|c| {
                        let installed = c.is_installed();
                        let version = installed.then(|| c.installed_version().ok()).flatten();
                        (c, installed, version)
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                for (c, installed, version) in probed {
                    let tr = this.cli_updates.get_mut(c);
                    tr.installed = installed;
                    if let Some(v) = version {
                        tr.version = Some(v);
                    }
                }
                this.refresh_provider_catalog(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Looks for newer versions of every installed CLI (`claude`, `grok`, `agy`) now and every few hours after.
    pub(super) fn watch_cli_updates(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let results = cx
                    .background_spawn(async {
                        ManagedCli::ALL.map(|c| {
                            if !c.is_installed() {
                                return (c, None);
                            }
                            (c, Some(c.check()))
                        })
                    })
                    .await;
                let ok = this.update(cx, |this, cx| {
                    for (c, found) in results {
                        match found {
                            Some(res) => {
                                this.cli_updates.get_mut(c).installed = true;
                                this.cli_checked(c, res, false, cx);
                            }
                            None => {
                                this.cli_updates.get_mut(c).installed = false;
                            }
                        }
                    }
                });
                if ok.is_err() {
                    break;
                }
                cx.background_executor().timer(CLI_CHECK_EVERY).await;
            }
        })
        .detach();
    }

    /// Checks every installed CLI for updates right now (from the Kynex menu or Command Palette).
    pub(super) fn check_all_clis_now(&mut self, cx: &mut Context<Self>) {
        for c in ManagedCli::ALL {
            if self.cli_updates.get(c).installed || c == ManagedCli::Claude {
                self.check_cli_now(c, cx);
            }
        }
    }

    /// Checks for a newer release of `cli` right now, from the About modal.
    pub(super) fn check_cli_now(&mut self, cli: ManagedCli, cx: &mut Context<Self>) {
        let tr = self.cli_updates.get_mut(cli);
        if tr.check.checking || matches!(tr.update, UpdateState::Updating { .. }) {
            return;
        }
        tr.check.checking = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async move { cli.check() }).await;
            this.update(cx, |this, cx| this.cli_checked(cli, found, true, cx)).ok();
        })
        .detach();
    }

    fn cli_checked(
        &mut self,
        cli: ManagedCli,
        found: Result<(String, String), String>,
        manual: bool,
        cx: &mut Context<Self>,
    ) {
        let skipped = (cli == ManagedCli::Claude).then(|| self.saved.skipped_cli_version.clone()).flatten();
        let tr = self.cli_updates.get_mut(cli);
        if manual {
            tr.check.checking = false;
        }
        tr.check.at = Some(SystemTime::now());
        tr.check.error = found.as_ref().err().cloned();
        if let Ok((installed, _)) = &found {
            tr.installed = true;
            tr.version = Some(installed.clone());
        }
        if matches!(tr.update, UpdateState::Updating { .. }) {
            return cx.notify();
        }
        match found {
            Ok((installed, latest)) if cli::is_newer(&latest, &installed) => {
                if manual || skipped.as_deref() != Some(latest.as_str()) {
                    tr.update = UpdateState::Available { installed, latest };
                }
            }
            Ok(_) if matches!(tr.update, UpdateState::Available { .. }) => tr.update = UpdateState::Idle,
            Ok(_) => {}
            Err(e) => eprintln!("kynex: couldn't check for {} updates: {e}", cli.label()),
        }
        cx.notify();
    }

    /// Runs `<cli> update` off the UI thread. Active interactive panes stay untouched;
    /// new panes pick up the updated CLI.
    pub(super) fn update_cli(&mut self, cli: ManagedCli, cx: &mut Context<Self>) {
        let latest = match &self.cli_updates.get(cli).update {
            UpdateState::Available { latest, .. } => latest.clone(),
            _ => return,
        };
        self.cli_updates.get_mut(cli).update = UpdateState::Updating { latest };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { cli.update() }).await;
            this.update(cx, |this, cx| {
                let tr = this.cli_updates.get_mut(cli);
                tr.update = match result {
                    Ok(version) => {
                        tr.installed = true;
                        tr.version = Some(version.clone());
                        tr.check.error = None;
                        UpdateState::Updated { version }
                    }
                    Err(e) => UpdateState::Failed(e),
                };
                match cli {
                    ManagedCli::Grok => this.load_grok_status(cx),
                    ManagedCli::Antigravity => this.load_antigravity_status(cx),
                    ManagedCli::Claude => {}
                }
                this.refresh_provider_catalog(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
