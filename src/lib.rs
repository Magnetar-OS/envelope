// SPDX-License-Identifier: GPL-3.0-only

//! Envelope — mail for the COSMIC desktop.
//!
//! A front end over `cosmic-pim-mail`, the way Slate is one over
//! `cosmic-pim-caldav`. Everything that decides what a message *is*, where it
//! is stored, and how it reaches a server lives in the substrate; what is here
//! is the window, the state machine, and the decisions about what to show.
//!
//! - [`mail`] — the join: connections, conversations, and the blocking calls
//!   the app runs on a worker thread.
//! - [`app`] — the state machine.
//! - [`ui`] — the views, each a function of the model.

// Envelope is a binary. The modules below are `pub` so that `main.rs` and the
// integration tests can reach them, not because anything outside this crate
// links against them — there is no published API here to document. `# Errors`
// sections earn their keep in `cosmic-pim-*`, which other applications do
// consume; forty-three of them here would be forty-three sections nobody
// reads. Every other pedantic lint stays on.
#![allow(
    clippy::missing_errors_doc,
    reason = "binary crate: the pub surface exists for the lib/bin split, not for consumers"
)]

pub mod actions;
pub mod app;
pub mod config;
pub mod crash;
pub mod flags;
pub mod i18n;
pub mod mail;
pub mod mailto;
pub mod scheduling;
pub mod ui;

/// Runs the application.
pub fn run() -> cosmic::iced::Result {
    // Before anything can panic: a crash must leave a report even when it
    // happens during start-up.
    crash::install_hook();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "envelope=warn".into()),
        )
        .init();

    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
    i18n::init(&requested_languages);

    let settings = cosmic::app::Settings::default().size(cosmic::iced::Size::new(1200.0, 800.0));

    // `run_single_instance`, not `run`. A mail client registered as the
    // desktop's `mailto:` handler is nearly always already running when
    // somebody clicks a link, and this is what hands the link to the instance
    // that exists rather than opening a second one. It is also what makes the
    // desktop entry's `DBusActivatable=true` true.
    cosmic::app::run_single_instance::<app::AppModel>(
        settings,
        flags::Flags {
            launch: flags::Launch::from_args(std::env::args()),
        },
    )
}
