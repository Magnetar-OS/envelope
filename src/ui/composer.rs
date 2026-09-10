// SPDX-License-Identifier: GPL-3.0-only

//! The composer.
//!
//! One body, sent as `text/plain`. See `cosmic_pim_mail::compose` for why that
//! is a decision rather than a stage: the reader shows text, so an HTML
//! composer would be writing in a format this application cannot display.
//!
//! The editor underneath is Nib, and it holds a document rather than a string.
//! That is not a step towards an HTML composer — it is what makes the plain
//! text correct. A quote the caret can be *inside* continues itself on Enter,
//! and `nib_text` writes the `>` markers and the wrap column from the
//! structure on the way out, rather than a pass over a finished string trying
//! to work out which lines were quoted.

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
        // Nib, not a text field and no longer libcosmic's `text_editor`. The
        // body is a document: a reply's quoted text is a `blockquote` the
        // caret can be inside, which is what makes Enter continue the quote
        // without anything re-reading the line to find its `> `.
        //
        // Borderless, and inset to the same left edge the field values sit
        // on, so the message reads as a continuation of what it is addressed
        // to rather than as a box dropped inside the window.
        .push(
            nib::editor(&composer.body)
                .placeholder(fl!("compose-body-placeholder"))
                .id(crate::ui::COMPOSE_BODY_ID.clone())
                .height(Length::Fill)
                .style(seamless(&cosmic::theme::active()))
                .on_action(|action| Message::ComposeBodyAction(Box::new(action))),
        );

    // Only what is actually attached. An empty composer showing an
    // attachments section is a section about nothing.
    if !composer.draft.attachments.is_empty() {
        column = column.push(attachments(composer));
    }

    if let Some(error) = &composer.error {
        column = column.push(
            widget::container(crate::ui::destructive(error.clone())).padding([0, spacing.space_m]),
        );
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
        // The seam the header has, at the other end: the bar underneath is
        // about the message rather than part of it, and without a line the
        // buttons float on the same ground as the text being written.
        .push(widget::divider::horizontal::light())
        .push(
            widget::row::with_capacity(5)
                .align_y(Alignment::Center)
                .spacing(spacing.space_xs)
                // Attaching a file belongs with the other things done *to*
                // the message, not on a line of its own above them.
                .push(button(fl!("attach"), idle, Message::AttachFile))
                .push(button(fl!("save-draft"), idle, Message::ComposeCancel))
                .push(destructive_button(
                    fl!("discard"),
                    idle,
                    Message::ComposeDiscard,
                ))
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
                })
                .padding([spacing.space_xs, spacing.space_m]),
        )
        .into()
}

/// Destructive in colour, not in mass.
///
/// The theme's `Destructive` class is a filled red slab, which made the
/// loudest thing in the composer a button that throws the message away — and
/// left Send, the thing the window is for, quieter than it. The fill belongs
/// on the confirm button of a dialog about something irreversible; here the
/// warning is carried by the label's colour, which is enough on a button that
/// says "Discard".
fn destructive_button(label: String, enabled: bool, message: Message) -> Element<'static, Message> {
    if !enabled {
        // A red label on a button that cannot be pressed reads as pressable.
        return widget::button::text(label).into();
    }
    widget::button::custom(crate::ui::destructive(label))
        .class(cosmic::theme::Button::Text)
        .on_press(message)
        .into()
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
        .padding([spacing.space_xxs, spacing.space_m])
        .push(crate::ui::muted(fl!(
            "attachments-total",
            count = attached.len(),
            size = crate::ui::size(composer.draft.attachment_bytes())
        )));

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
                .id(crate::ui::COMPOSE_TO_ID.clone())
                .on_input(Message::ComposeToChanged)
                .style(cosmic::theme::TextInput::Inline)
                .width(Length::Fill),
        );
    if !composer.show_cc {
        control = control.push(
            widget::button::text(fl!("show-cc"))
                .class(cosmic::theme::Button::Text)
                // A button's own padding is taller than a field's, and every
                // header row has to be the same height or the label column
                // stops reading as a column.
                .padding([0, spacing.space_xxs])
                .on_press(Message::ComposeShowCc),
        );
    }
    row(fl!("to"), control.into())
}

/// The body's style: the theme's, at the size a message is read at.
///
/// Nib draws no frame of its own, so there is no box to turn off — the editor
/// is the largest thing in the window and the only one the user is actually
/// looking at, and a frame around it would be a frame around the message.
/// What is set here is the inset, so the text runs down the same left edge the
/// field values above it do.
fn seamless(theme: &cosmic::Theme) -> nib::Style {
    let spacing = cosmic::theme::spacing();
    nib::Style {
        padding: f32::from(spacing.space_m),
        ..nib::Style::from_theme(theme)
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
