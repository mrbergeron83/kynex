//! How long an idle Claude pane keeps its `claude` process.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The "Sleep idle sessions" setting: how long a session that
/// [can sleep](super::session::Session::can_sleep) keeps its `claude` process (and the MCP
/// servers under it) once nothing happens in it. A sleeping session keeps its
/// transcript and starts again when typed in or sent a message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SleepAfter {
    Minutes15,
    #[default]
    Minutes30,
    Hour1,
    Never,
}

impl SleepAfter {
    pub const ALL: [Self; 4] = [Self::Minutes15, Self::Minutes30, Self::Hour1, Self::Never];

    /// How often the app checks for sessions to put to sleep.
    pub const CHECK_EVERY: Duration = Duration::from_secs(60);

    pub fn label(self) -> &'static str {
        match self {
            Self::Minutes15 => "After 15 minutes idle",
            Self::Minutes30 => "After 30 minutes idle",
            Self::Hour1 => "After an hour idle",
            Self::Never => "Never",
        }
    }

    pub fn duration(self) -> Option<Duration> {
        const MIN: u64 = 60;
        match self {
            Self::Minutes15 => Some(Duration::from_secs(15 * MIN)),
            Self::Minutes30 => Some(Duration::from_secs(30 * MIN)),
            Self::Hour1 => Some(Duration::from_secs(60 * MIN)),
            Self::Never => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_after_durations() {
        assert_eq!(SleepAfter::default(), SleepAfter::Minutes30);
        assert_eq!(SleepAfter::Minutes15.duration(), Some(Duration::from_secs(900)));
        assert_eq!(SleepAfter::Hour1.duration(), Some(Duration::from_secs(3600)));
        assert_eq!(SleepAfter::Never.duration(), None);
    }
}
