// SPDX-License-Identifier: GPL-3.0-only

//! Persisted settings, stored through `cosmic-config` so they live alongside
//! every other COSMIC app's configuration and are picked up live when changed.
//!
//! Deliberately small. Everything about the *mail* — which messages exist, what
//! is read, what is queued — is on disk in the maildir and its sidecars, which
//! is where it belongs; this is only what the window should look like when it
//! opens, and the two preferences that have an argument behind them.

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};

/// How often the mailbox is checked, in seconds.
///
/// A poll, not IDLE. Two minutes keeps a mail client feeling live without being
/// the reason a laptop's radio never sleeps, and it is short enough that a flag
/// change made offline reaches the server promptly.
pub const DEFAULT_POLL_SECONDS: u32 = 120;

/// The shortest interval that will be honoured.
///
/// Not a matter of taste: a client polling every few seconds is
/// indistinguishable from a broken one from the server's side, and providers
/// rate-limit or lock out accounts that do it. The floor protects the user from
/// a setting that would get their account suspended.
pub const MINIMUM_POLL_SECONDS: u32 = 30;

#[derive(Clone, Debug, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    /// The account selected when the window last closed.
    ///
    /// Remembered because with more than one account, always reopening on the
    /// first means somebody whose second account is the one they read has to
    /// re-select it every single time.
    pub last_account: String,
    /// The folder selected when the window last closed, as a wire name.
    ///
    /// A wire name rather than an index: folders are re-listed from the server
    /// on every sync, and an index would restore whatever happened to be third
    /// this time.
    pub last_folder: String,
    /// Seconds between checks. See [`Config::poll_interval`].
    pub poll_seconds: u32,
    /// Whether the reader marks a message read when it is opened.
    ///
    /// On, because that is what every mail client does and what people expect.
    /// Off is for the people who use their inbox as a to-do list, for whom a
    /// message losing its unread mark on a glance is losing a task.
    pub mark_read_on_open: bool,
    /// Seconds a send waits in the outbox before going, so it can be taken
    /// back. Zero sends immediately. See [`Config::send_delay`].
    pub send_delay_seconds: u32,
    /// How wide the folder sidebar is, in pixels.
    ///
    /// Remembered because a column width is a judgement about the user's own
    /// folder names and their own screen, and re-making it at every launch is
    /// the kind of small daily friction that makes an application feel
    /// borrowed rather than theirs. Clamped on read — see
    /// [`Config::sidebar_width`].
    pub sidebar_width: u32,
    /// How wide the message list is, in pixels. See
    /// [`Config::list_width`].
    pub list_width: u32,
}

/// What a column may be dragged to.
///
/// Floors rather than preferences: a sidebar under ~180px cannot show a
/// folder name, and a list under ~280px cannot show a sender and a date on
/// one line. The ceilings stop a drag from leaving no room for the message
/// itself, which is the thing the window is for.
pub const SIDEBAR_WIDTH: std::ops::RangeInclusive<u32> = 180..=460;
pub const LIST_WIDTH: std::ops::RangeInclusive<u32> = 280..=700;
pub const DEFAULT_SIDEBAR_WIDTH: u32 = 240;
pub const DEFAULT_LIST_WIDTH: u32 = 380;

impl Default for Config {
    fn default() -> Self {
        Self {
            last_account: String::new(),
            last_folder: String::new(),
            poll_seconds: DEFAULT_POLL_SECONDS,
            mark_read_on_open: true,
            send_delay_seconds: DEFAULT_SEND_DELAY_SECONDS,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            list_width: DEFAULT_LIST_WIDTH,
        }
    }
}

/// Long enough to catch the typo noticed as the button was released; short
/// enough that "sent" still means soon.
pub const DEFAULT_SEND_DELAY_SECONDS: u32 = 10;

/// The ceiling on the undo-send grace, hand-edited files included. Two
/// minutes is where a delay stops being a grace and becomes a scheduler,
/// and the scheduler is the Send later button.
pub const MAXIMUM_SEND_DELAY_SECONDS: u32 = 120;

impl Config {
    /// How long to wait between checks, floored.
    ///
    /// Clamped here rather than validated on write so that a hand-edited
    /// configuration file — which is a supported thing to do with
    /// `cosmic-config` — cannot produce a client that hammers a server.
    #[must_use]
    pub fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.poll_seconds.max(MINIMUM_POLL_SECONDS)))
    }

    /// The sidebar's width, clamped — a hand-edited configuration file is a
    /// supported thing to do, and it must not be able to produce a window
    /// with no message list in it.
    #[must_use]
    pub fn sidebar_width(&self) -> f32 {
        clamp(self.sidebar_width, SIDEBAR_WIDTH)
    }

    /// The message list's width, clamped for the same reason.
    #[must_use]
    pub fn list_width(&self) -> f32 {
        clamp(self.list_width, LIST_WIDTH)
    }

    /// The undo-send grace, ceilinged for the same hand-edited-file reason
    /// the poll is floored.
    #[must_use]
    pub fn send_delay(&self) -> u32 {
        self.send_delay_seconds.min(MAXIMUM_SEND_DELAY_SECONDS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_what_a_first_run_should_do() {
        let config = Config::default();
        assert_eq!(config.poll_interval().as_secs(), 120);
        assert!(
            config.mark_read_on_open,
            "off by default would surprise everybody who has used a mail client"
        );
        assert!(config.last_account.is_empty());
    }

    #[test]
    fn a_hand_edited_interval_cannot_produce_a_client_that_hammers_a_server() {
        // Editing the file is a supported thing to do, and a client polling
        // every second is indistinguishable from a broken one — providers
        // rate-limit and lock out accounts for it.
        for seconds in [0, 1, 5, 29] {
            let config = Config {
                poll_seconds: seconds,
                ..Config::default()
            };
            assert_eq!(
                config.poll_interval().as_secs(),
                u64::from(MINIMUM_POLL_SECONDS),
                "{seconds} was honoured"
            );
        }
    }

    #[test]
    fn a_longer_interval_is_honoured_as_written() {
        let config = Config {
            poll_seconds: 900,
            ..Config::default()
        };
        assert_eq!(config.poll_interval().as_secs(), 900);
    }
}

#[allow(clippy::cast_precision_loss, reason = "a column width in pixels")]
fn clamp(value: u32, range: std::ops::RangeInclusive<u32>) -> f32 {
    value.clamp(*range.start(), *range.end()) as f32
}
