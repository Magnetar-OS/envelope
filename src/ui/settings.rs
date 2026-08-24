// SPDX-License-Identifier: GPL-3.0-only

//! The Settings context page.
//!
//! Two preferences, both of which exist because the obvious default is wrong
//! for a real group of people. Everything else Envelope could offer a switch
//! for has one right answer, and a switch for it would be a decision passed
//! back to the user for no reason.

use cosmic::Element;
use cosmic::widget;

use crate::app::Message;
use crate::config::{Config, MINIMUM_POLL_SECONDS};
use crate::fl;

pub fn view<'a>(config: &Config, interval: &'a str) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();

    let section = widget::settings::section()
        .add(
            widget::settings::item::builder(fl!("check-every"))
                .description(fl!("check-every-hint", minimum = MINIMUM_POLL_SECONDS))
                .control(
                    widget::text_input("120", interval)
                        .on_input(Message::PollSecondsChanged)
                        .on_focus(Message::TextFocused)
                        .on_unfocus(Message::TextUnfocused)
                        .width(cosmic::iced::Length::Fixed(120.0)),
                ),
        )
        .add(
            widget::settings::item::builder(fl!("mark-read-on-open"))
                .description(fl!("mark-read-on-open-hint"))
                .toggler(config.mark_read_on_open, Message::MarkReadOnOpenChanged),
        );

    widget::column::with_capacity(1)
        .spacing(spacing.space_s)
        .push(section)
        .into()
}
