// SPDX-License-Identifier: GPL-3.0-only

//! What Envelope was launched with, and how a second launch reaches the first.
//!
//! # Why this is not just a `String`
//!
//! Envelope registers as the desktop's `mailto:` handler, and a mail client
//! that is registered as one is nearly always **already running** when somebody
//! clicks a link. What happens then is decided here.
//!
//! libcosmic's [`run_single_instance`] hands the second launch's flags to the
//! first over D-Bus and exits, rather than opening a second window — but only
//! if the flags implement [`CosmicFlags`]. Without that, `cosmic::app::run`
//! starts a whole second Envelope, and the desktop file's
//! `DBusActivatable=true` is a claim the application does not honour.
//!
//! So a launch argument is a [`Launch`], it serialises to a string the D-Bus
//! `ActivateAction` call carries, and the running instance parses it back in
//! `dbus_activation`.
//!
//! [`run_single_instance`]: cosmic::app::run_single_instance
//! [`CosmicFlags`]: cosmic::app::CosmicFlags

use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// What a launch is asking for beyond "open the window".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Launch {
    /// A `mailto:` URL, from a browser, from Circle, from Slate's attendee
    /// list.
    Mailto(String),
    /// The desktop entry's "New Message" action.
    Compose,
}

impl Launch {
    /// Reads a command line, ignoring the program name.
    ///
    /// Deliberately tolerant: an argument that is not a `mailto:` URL is not an
    /// error, because the desktop passes `%u` whether or not there is a URL to
    /// pass, and refusing to start over it would be absurd.
    #[must_use]
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Option<Self> {
        args.into_iter().skip(1).find_map(|argument| {
            if argument.to_ascii_lowercase().starts_with("mailto:") {
                Some(Self::Mailto(argument))
            } else if argument == "compose" {
                Some(Self::Compose)
            } else {
                None
            }
        })
    }
}

impl Display for Launch {
    /// JSON, because this crosses a D-Bus boundary as one string and has to
    /// come back the same shape.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_string(self) {
            Ok(json) => write!(f, "{json}"),
            Err(_) => write!(f, "{{}}"),
        }
    }
}

impl FromStr for Launch {
    type Err = serde_json::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(text)
    }
}

/// The flags Envelope starts with.
#[derive(Debug, Clone, Default)]
pub struct Flags {
    pub launch: Option<Launch>,
}

impl cosmic::app::CosmicFlags for Flags {
    type SubCommand = Launch;
    type Args = Vec<String>;

    fn action(&self) -> Option<&Launch> {
        self.launch.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(rest: &[&str]) -> Vec<String> {
        std::iter::once("envelope")
            .chain(rest.iter().copied())
            .map(ToOwned::to_owned)
            .collect()
    }

    #[test]
    fn a_mailto_argument_is_recognised_whatever_its_case() {
        assert_eq!(
            Launch::from_args(args(&["mailto:ada@example.com"])),
            Some(Launch::Mailto("mailto:ada@example.com".into()))
        );
        assert_eq!(
            Launch::from_args(args(&["MAILTO:ada@example.com"])),
            Some(Launch::Mailto("MAILTO:ada@example.com".into()))
        );
    }

    #[test]
    fn the_desktop_actions_argument_is_recognised() {
        assert_eq!(Launch::from_args(args(&["compose"])), Some(Launch::Compose));
    }

    #[test]
    fn an_argument_that_means_nothing_is_not_an_error() {
        // The desktop passes `%u` whether or not there is a URL, and refusing
        // to start over it would be absurd.
        assert_eq!(Launch::from_args(args(&[])), None);
        assert_eq!(Launch::from_args(args(&["--verbose"])), None);
        assert_eq!(Launch::from_args(args(&[""])), None);
    }

    #[test]
    fn the_program_name_is_never_read_as_an_argument() {
        // A binary that happened to live at a path containing "compose" must
        // not open a composer on every launch.
        assert_eq!(
            Launch::from_args(vec!["/usr/bin/compose".to_string()]),
            None
        );
    }

    #[test]
    fn a_launch_survives_the_trip_across_dbus() {
        // It goes over as one string and has to come back the same shape, or
        // the running instance receives a click it cannot act on.
        for launch in [
            Launch::Mailto("mailto:a@example.com?subject=Hi%20there".into()),
            Launch::Compose,
        ] {
            let round_tripped: Launch = launch.to_string().parse().expect("parse");
            assert_eq!(round_tripped, launch);
        }
    }
}
