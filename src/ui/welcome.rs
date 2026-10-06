// SPDX-License-Identifier: GPL-3.0-only

//! What Envelope shows before there is an account: what it is for, and the
//! one thing to do about it.
//!
//! Three empty panes say nothing a person can act on. This page says one
//! thing and offers one button, which opens the desktop's Accounts window on
//! its add page — the account added there is the suite's, so its calendars
//! and contacts reach Slate and Circle in the same step.

use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::widget;

use crate::app::Message;
use crate::fl;

/// The welcome page, filling the window.
pub fn view() -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();

    let column = widget::column::with_capacity(4)
        .spacing(spacing.space_m)
        .align_x(Alignment::Center)
        .max_width(460.0)
        .push(widget::icon::from_name("mail-send-receive-symbolic").size(64))
        .push(widget::text::title2(fl!("welcome-title")))
        .push(
            widget::text::body(fl!("welcome-body"))
                .align_x(Alignment::Center)
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        )
        .push(widget::button::suggested(fl!("welcome-add")).on_press(Message::SetUpAccount));

    widget::container(column)
        .center(Length::Fill)
        .padding(spacing.space_l)
        .into()
}
