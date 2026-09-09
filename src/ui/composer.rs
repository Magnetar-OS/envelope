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

    // The header is one surface, not a stack of boxes. Every value sits on
    // the same left edge under a muted label, with hairlines between the
    // rows — which is the thing a bordered input per field cannot do: five
    // boxes read as a form to be filled in, and a message is not a form.
    let mut header =
        widget::column::with_capacity(6).push(from_row(composer, identities, selected_identity));

    header = header
        .push(widget::divider::horizontal::default())
        .push(to_row(composer));

    if composer.show_cc {
        header = header
            .push(widget::divider::horizontal::default())
            .push(field(fl!("cc"), &composer.cc, Message::ComposeCcChanged))
            .push(widget::divider::horizontal::default())
            .push(field(fl!("bcc"), &composer.bcc, Message::ComposeBccChanged));
    }

    header = header
        .push(widget::divider::horizontal::default())
        .push(field(
            fl!("subject"),
            &composer.draft.subject,
            Message::ComposeSubjectChanged,
        ));

    let mut column = widget::column::with_capacity(4)
        .push(header)
        // Heavier than the hairlines above it. This is the seam between what
        // the message is addressed to and what it says — the one division in
        // the window worth seeing without looking for it.
        .push(widget::divider::horizontal::light())
        // A real editor, not a text field. `text_input` is single-line by
        // construction — it fires `on_submit` for Enter and strips control
        // characters from every paste — so a body built on it could not hold
        // a paragraph break, and a reply opened with its quoted text
        // flattened onto one line. This also brings selection, word motion,
        // Home/End and a right-click menu, none of which had to be written.
        //
        // Borderless, and inset to the same left edge the field values sit
        // on, so the message reads as a continuation of what it is addressed
        // to rather than as a box dropped inside the window.
        .push(
            widget::text_editor::text_editor(&composer.body)
                .placeholder(fl!("compose-body-placeholder"))
                .height(Length::Fill)
                .padding([spacing.space_s, spacing.space_m])
                .style(seamless)
                .context_menu(true)
                .on_action(|action| Message::ComposeBodyAction(Box::new(action))),
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

/// One header row: a muted label on the shared column, and a borderless
/// value beside it.
fn field(
    label: String,
    value: &str,
    on_input: impl Fn(String) -> Message + 'static,
) -> Element<'_, Message> {
    row(
        label,
        widget::text_input(String::new(), value)
            .on_input(on_input)
            .style(cosmic::theme::TextInput::Inline)
            .width(Length::Fill)
            .into(),
    )
}

/// The frame every header row shares: label column, then the value.
fn row<'a>(label: String, control: Element<'a, Message>) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    widget::row::with_capacity(2)
        .align_y(Alignment::Center)
        .push(
            widget::text::caption(label)
                .width(Length::Fixed(crate::ui::LABEL_WIDTH))
                .class(cosmic::theme::Text::Custom(|theme| {
                    cosmic::iced::widget::text::Style {
                        // Muted: a label is furniture for finding the value,
                        // and at full contrast it competes with it.
                        color: Some(theme.cosmic().on_bg_color().into()),
                        ..Default::default()
                    }
                })),
        )
        .push(control)
        .padding([spacing.space_xxs, spacing.space_m])
        .into()
}

/// Who the message goes out as, and — where the account has more than one
/// address — which of them.
fn from_row<'a>(
    composer: &'a Composer,
    identities: &'a [String],
    selected: usize,
) -> Element<'a, Message> {
    row(
        fl!("from"),
        if identities.len() > 1 {
            widget::dropdown(identities, Some(selected), Message::ComposeFromSelected).into()
        } else {
            widget::text::body(from_line(composer)).into()
        },
    )
}

/// The `To` row, carrying the disclosure for the two fields that are usually
/// empty.
fn to_row(composer: &Composer) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let mut control = widget::row::with_capacity(2)
        .align_y(Alignment::Center)
        .spacing(spacing.space_xxs)
        .push(
            widget::text_input(String::new(), &composer.to)
                .on_input(Message::ComposeToChanged)
                .style(cosmic::theme::TextInput::Inline)
                .width(Length::Fill),
        );
    if !composer.show_cc {
        control = control.push(
            widget::button::text(fl!("show-cc"))
                .class(cosmic::theme::Button::Text)
                .on_press(Message::ComposeShowCc),
        );
    }
    row(fl!("to"), control.into())
}

/// The body's style: no box, no border, the window's own ground.
///
/// The editor is the largest thing in the window and the only one the user
/// is actually looking at; drawing a frame around it would be drawing a
/// frame around the message.
fn seamless(
    theme: &cosmic::Theme,
    _status: cosmic::widget::text_editor::Status,
) -> cosmic::widget::text_editor::Style {
    let cosmic = theme.cosmic();
    cosmic::widget::text_editor::Style {
        background: cosmic::iced::Background::Color(cosmic::iced::Color::TRANSPARENT),
        border: cosmic::iced::Border::default(),
        placeholder: cosmic.on_bg_color().into(),
        value: cosmic.on_bg_color().into(),
        selection: cosmic.accent.base.into(),
    }
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
