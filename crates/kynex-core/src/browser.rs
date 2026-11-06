//! The browser Kynex gives every Claude session: `@playwright/mcp` in its own
//! Edge or Chrome window, picked in Settings. Handed to each `claude` with
//! `--mcp-config` under the name the old presets used (`playwright-edge`), so it
//! takes the place of such an entry in Claude's own config instead of adding a
//! second browser. Playwright keeps a profile per folder, so logins made in its
//! window stay.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::json;

/// A browser Playwright can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Browser {
    Edge,
    Chrome,
}

impl Browser {
    pub const ALL: [Self; 2] = [Self::Edge, Self::Chrome];

    pub fn label(self) -> &'static str {
        match self {
            Self::Edge => "Edge",
            Self::Chrome => "Chrome",
        }
    }

    /// `@playwright/mcp`'s `--browser` value.
    fn channel(self) -> &'static str {
        match self {
            Self::Edge => "msedge",
            Self::Chrome => "chrome",
        }
    }

    /// The MCP server's name, which its tools carry (`mcp__playwright-edge__…`).
    pub fn server(self) -> String {
        format!("playwright-{}", self.label().to_ascii_lowercase())
    }

    /// Whether it's installed, by where its installers put it. Blocks on a few
    /// file checks.
    pub fn installed(self) -> bool {
        self.places().iter().any(|p| p.exists())
    }

    #[cfg(windows)]
    fn places(self) -> Vec<PathBuf> {
        let rel = match self {
            Self::Edge => ["Microsoft", "Edge", "Application", "msedge.exe"],
            Self::Chrome => ["Google", "Chrome", "Application", "chrome.exe"],
        };
        ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|root| rel.iter().fold(PathBuf::from(root), |p, part| p.join(part)))
            .collect()
    }

    #[cfg(target_os = "macos")]
    fn places(self) -> Vec<PathBuf> {
        let app = match self {
            Self::Edge => "Microsoft Edge.app",
            Self::Chrome => "Google Chrome.app",
        };
        let mut out = vec![std::path::Path::new("/Applications").join(app)];
        out.extend(dirs::home_dir().map(|h| h.join("Applications").join(app)));
        out
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    fn places(self) -> Vec<PathBuf> {
        let names: &[&str] = match self {
            Self::Edge => &["microsoft-edge", "microsoft-edge-stable"],
            Self::Chrome => &["google-chrome", "google-chrome-stable"],
        };
        names.iter().filter_map(|n| crate::mcp_config::on_path(n)).collect()
    }
}

/// The Settings choice: which browser sessions get, if any.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pick {
    Off,
    #[default]
    Edge,
    Chrome,
}

impl Pick {
    pub const ALL: [Self; 3] = [Self::Edge, Self::Chrome, Self::Off];

    pub fn browser(self) -> Option<Browser> {
        match self {
            Self::Off => None,
            Self::Edge => Some(Browser::Edge),
            Self::Chrome => Some(Browser::Chrome),
        }
    }

    pub fn label(self) -> &'static str {
        self.browser().map_or("Off", Browser::label)
    }

    fn code(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Edge => 1,
            Self::Chrome => 2,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Off,
            2 => Self::Chrome,
            _ => Self::Edge,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(1);

/// Sets the app-wide choice; sessions started from now on follow it.
pub fn set(pick: Pick) {
    CURRENT.store(pick.code(), Ordering::Relaxed);
}

pub fn current() -> Pick {
    Pick::from_code(CURRENT.load(Ordering::Relaxed))
}

/// `--mcp-config` for `browser`.
pub fn mcp_config(browser: Browser) -> String {
    json!({ "mcpServers": { browser.server(): {
        "type": "stdio",
        "command": "npx",
        "args": ["@playwright/mcp@latest", "--browser", browser.channel()],
    } } })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn config_names_the_server_by_browser() {
        let v: Value = serde_json::from_str(&mcp_config(Browser::Edge)).unwrap();
        assert_eq!(
            v,
            json!({ "mcpServers": { "playwright-edge": {
                "type": "stdio", "command": "npx", "args": ["@playwright/mcp@latest", "--browser", "msedge"],
            } } })
        );
        assert_eq!(Browser::Chrome.server(), "playwright-chrome");
    }

    #[test]
    fn picks_round_trip() {
        for p in Pick::ALL {
            assert_eq!(Pick::from_code(p.code()), p);
        }
        assert_eq!(Pick::default(), Pick::Edge);
        assert_eq!(Pick::Off.label(), "Off");
        assert_eq!(Pick::Chrome.browser(), Some(Browser::Chrome));
    }
}
