// SPDX-License-Identifier: GPL-3.0-only

//! The composer.
//!
//! Plain text, one body, no formatting toolbar. See
//! `cosmic_pim_mail::compose` for why that is a decision rather than a stage:
//! the reader shows text, so an HTML composer would be writing in a format this
//! application cannot display.

use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::widget;

use crate::app::{Composer, Message};
use crate::fl;

pub fn view<'a>(
    composer: &'a Composer,
    identities: &'a [String],
    selected_identity: usize,
) -> Element<'a, Message> {
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
                // One identity is a fact and shows as one; several are a
                // choice and show as a dropdown.
                .push(if identities.len() > 1 {
                    Element::from(widget::dropdown(
                        identities,
                        Some(selected_identity),
                        Message::ComposeFromSelected,
                    ))
                } else {
                    Element::from(widget::text::caption(from_line(composer)))
                }),
        )
        .push(fields)
        .push(
            widget::text_input(String::new(), &composer.draft.body)
                .on_input(Message::ComposeBodyChanged)
                .width(Length::Fill),
        );

    column = column.push(attachments(composer));

    if let Some(error) = &composer.error {
        column = column.push(crate::ui::destructive(error.clone()));
    }

    let send = widget::button::text(if composer.sending {
        fl!("sending")
    } else {
        fl!("send")
    })
    .class(cosmic::theme::Button::Suggested);

    // Nothing is actionable while a send is in flight: the outcome still has to
    // land somewhere the user can see it, and a composer that vanished mid-send
    // would take an "it may have been delivered" with it.
    let idle = !composer.sending;

    column
        .push(
            widget::row::with_capacity(5)
                .spacing(spacing.space_xs)
                .push(destructive_button(
                    fl!("discard"),
                    idle,
                    Message::ComposeDiscard,
                ))
                .push(button(fl!("save-draft"), idle, Message::ComposeCancel))
                .push(widget::Space::new().width(Length::Fill))
                .push(button(
                    fl!("send-later"),
                    idle && composer.problem().is_none(),
                    Message::SendLater,
                ))
                // Greyed for the same reasons the send would fail, rather than
                // letting the user press it and find out.
                .push(if idle && composer.problem().is_none() {
                    send.on_press(Message::ComposeSend)
                } else {
                    send
                }),
        )
        .into()
}

fn destructive_button(label: String, enabled: bool, message: Message) -> Element<'static, Message> {
    let button = widget::button::text(label).class(cosmic::theme::Button::Destructive);
    if enabled {
        button.on_press(message).into()
    } else {
        button.into()
    }
}

fn button(label: String, enabled: bool, message: Message) -> Element<'static, Message> {
    let button = widget::button::text(label);
    if enabled {
        button.on_press(message).into()
    } else {
        button.into()
    }
}

/// The attach button, and whatever is already attached.
fn attachments(composer: &Composer) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let attached = &composer.draft.attachments;

    let mut column = widget::column::with_capacity(attached.len() + 1)
        .spacing(spacing.space_xxxs)
        .push(
            widget::row::with_capacity(2)
                .align_y(Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(button(
                    fl!("attach"),
                    !composer.sending,
                    Message::AttachFile,
                ))
                .push(if attached.is_empty() {
                    widget::text::caption(String::new())
                } else {
                    widget::text::caption(fl!(
                        "attachments-total",
                        count = attached.len(),
                        size = crate::ui::size(composer.draft.attachment_bytes())
                    ))
                }),
        );

    for (index, attachment) in attached.iter().enumerate() {
        column = column.push(
            widget::row::with_capacity(2)
                .align_y(Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(
                    widget::text::caption(format!(
                        "{} · {}",
                        attachment.name,
                        crate::ui::size(attachment.size())
                    ))
                    .width(Length::Fill),
                )
                .push(button(
                    fl!("remove"),
                    !composer.sending,
                    Message::AttachmentRemoved(index),
                )),
        );
    }

    column.into()
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

/// Who this is going out as, when the account has only one identity — a fact
/// worth showing, not a control. With aliases configured the caption becomes
/// the dropdown above.
fn from_line(composer: &Composer) -> String {
    match &composer.draft.from.name {
        Some(name) => format!("{name} <{}>", composer.draft.from.address),
        None => composer.draft.from.address.clone(),
    }
}
