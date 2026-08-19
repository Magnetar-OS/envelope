// SPDX-License-Identifier: GPL-3.0-only

//! Envelope — mail for the COSMIC desktop.
//!
//! A scaffold. See the README: the IMAP and JMAP engines have not been ported
//! into `cosmic-pim` yet, and until they are there is nothing for this front
//! end to be a front end over. What is here is the shell, the account list read
//! from the shared account store, and a screen that says so honestly rather
//! than showing an empty inbox that looks broken.

pub mod app;
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
