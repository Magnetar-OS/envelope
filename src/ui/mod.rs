// SPDX-License-Identifier: GPL-3.0-only

//! The views. Each one is a function of the model, with no state of its own.

pub mod accounts;
pub mod composer;
pub mod folders;
pub mod list;
pub mod palette;
pub mod reader;
pub mod rules;
pub mod settings;
pub mod shortcuts;
pub mod sidebar;

use chrono::{DateTime, Datelike as _, Local, Utc};

/// [`relative_date`] for an epoch-milliseconds timestamp, as the index stores
/// them. Zero means "this message had no parseable Date", which shows as
/// nothing rather than as 1970.
#[must_use]
pub fn relative_date_ms(ms: i64) -> String {
    if ms == 0 {
        return String::new();
    }
    relative_date(DateTime::from_timestamp_millis(ms))
}

/// A timestamp as a mail list shows one: a time for today, a weekday for this
/// week, a date otherwise.
///
/// A message list is scanned, not read. Every column that repeats the same
/// information — the year, on mail that is all from this year — is a column the
/// eye has to skip past to reach the part that differs.
#[must_use]
pub fn relative_date(date: Option<DateTime<Utc>>) -> String {
    let Some(date) = date else {
        return String::new();
    };
    let local = date.with_timezone(&Local);
    let now = Local::now();
    let days = (now.date_naive() - local.date_naive()).num_days();

    if days == 0 {
        local.format("%H:%M").to_string()
    } else if days == 1 {
        local.format("Yesterday").to_string()
    } else if (0..7).contains(&days) {
        local.format("%a").to_string()
    } else if local.year() == now.year() {
        local.format("%-d %b").to_string()
    } else {
        local.format("%-d %b %Y").to_string()
    }
}

/// The palette's input, so opening it can focus it.
pub static PALETTE_ID: std::sync::LazyLock<cosmic::widget::Id> =
    std::sync::LazyLock::new(|| cosmic::widget::Id::new("palette"));

/// The folder dialogs' input — the name field, and the move picker's query —
/// so opening either can focus it.
pub static FOLDER_NAME_ID: std::sync::LazyLock<cosmic::widget::Id> =
    std::sync::LazyLock::new(|| cosmic::widget::Id::new("folder-name"));

/// A byte count as a person reads one.
///
/// One rule for the whole application: the reader and the composer are showing
/// the same fact about the same file, and two formatters would eventually
/// disagree about it in front of the user.
#[must_use]
pub fn size(bytes: usize) -> String {
    const UNITS: [&str; 4] = ["B", "kB", "MB", "GB"];
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Text in the theme's destructive colour, for the things that need it.
pub fn destructive<Message: 'static>(text: String) -> cosmic::Element<'static, Message> {
    use cosmic::widget;
    widget::text::body(text)
        .class(cosmic::theme::Text::Custom(|theme| {
            cosmic::iced::widget::text::Style {
                color: Some(theme.cosmic().destructive_color().into()),
                ..Default::default()
            }
        }))
        .wrapping(cosmic::iced::core::text::Wrapping::Word)
        .into()
}
