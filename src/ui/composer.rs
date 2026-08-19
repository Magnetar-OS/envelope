// SPDX-License-Identifier: GPL-3.0-only

//! The composer.
//!
//! Plain text, one body, no formatting toolbar. See
//! `cosmic_pim_mail::compose` for why that is a decision rather than a stage:
//! the reader shows text, so an HTML composer would be writing in a format this
//! application cannot display.

use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::Element;

use crate::app::{Composer, Message};
use crate::fl;

pub fn view(composer: &Composer) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();

    let fields = widget::column::with_capacity(4)
        .spacing(spacing.space_xxs)
        .push(field(fl!("to"), &composer.to, Message::ComposeToChanged))
        .push(field(fl!("cc"), &composer.cc, Message::ComposeCcChanged))
        .push(field(fl!("bcc"), &composer.bcc, Message::ComposeBccChanged))
        .push(field(
            fl!("subject"),
            &composer.draft.subject,
            Message::ComposeSubjectChanged,
        ));

    let mut column = widget::column::with_capacity(5)
        .spacing(spacing.space_s)
        .padding(spacing.space_m)
        .push(
            widget::row::with_capacity(2)
                .align_y(Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(widget::text::title3(fl!("compose")).width(Length::Fill))
                .push(widget::text::caption(from_line(composer))),
        )
        .push(fields)
        .push(
            widget::text_input(String::new(), &composer.draft.body)
                .on_input(Message::ComposeBodyChanged)
                .width(Length::Fill),
        );

    if let Some(error) = &composer.error {
        column = column.push(crate::ui::destructive(error.clone()));
    }

    let send = widget::button::text(if composer.sending {
        fl!("sending")
    } else {
        fl!("send")
    })
    .class(cosmic::theme::Button::Suggested);

    column
        .push(
            widget::row::with_capacity(2)
                .spacing(spacing.space_xs)
                // No discard while a send is in flight: the outcome still has to
                // land somewhere the user can see it, and a composer that
                // vanished mid-send takes an "it may have been delivered" with
                // it.
                .push(if composer.sending {
                    widget::button::text(fl!("discard"))
                } else {
                    widget::button::text(fl!("discard")).on_press(Message::ComposeCancel)
                })
                // Greyed for the same reasons the send would fail, rather than
                // letting the user press it and find out.
                .push(if composer.sending || composer.problem().is_some() {
                    send
                } else {
                    send.on_press(Message::ComposeSend)
                }),
        )
        .into()
}

fn field(
    label: String,
    value: &str,
    on_input: impl Fn(String) -> Message + 'static,
) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    widget::row::with_capacity(2)
        .align_y(Alignment::Center)
        .spacing(spacing.space_xxs)
        .push(widget::text::caption(label).width(Length::Fixed(64.0)))
        .push(
            widget::text_input(String::new(), value)
                .on_input(on_input)
                .width(Length::Fill),
        )
        .into()
}

/// Who this is going out as. Shown rather than editable: an account has one
/// From identity, set in the Accounts page, and a picker here would be a second
/// place to change the same thing.
fn from_line(composer: &Composer) -> String {
    match &composer.draft.from.name {
        Some(name) => format!("{name} <{}>", composer.draft.from.address),
        None => composer.draft.from.address.clone(),
    }
}
