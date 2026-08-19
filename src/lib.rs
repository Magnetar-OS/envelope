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

pub mod app;
pub mod mail;
pub mod ui;
pub mod i18n;

/// Runs the application.
pub fn run() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "envelope=warn".into()),
        )
        .init();

    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
    i18n::init(&requested_languages);

    let settings = cosmic::app::Settings::default()
        .size(cosmic::iced::Size::new(1200.0, 800.0));

    cosmic::app::run::<app::AppModel>(settings, ())
}
