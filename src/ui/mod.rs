// SPDX-License-Identifier: GPL-3.0-only

//! The views. Each one is a function of the model, with no state of its own.

pub mod accounts;
pub mod composer;
pub mod list;
pub mod reader;
pub mod sidebar;

use chrono::{DateTime, Datelike as _, Local, Utc};

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
