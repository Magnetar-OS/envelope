// SPDX-License-Identifier: GPL-3.0-only

//! The reader.
//!
//! # What is deliberately not here
//!
//! An HTML renderer. Envelope shows the message's visible *text*, extracted by
//! `cosmic_pim_mail::text` from a real html5ever tree. That is not a
//! simplification to be undone later — it is the display-security position, and
//! it is stronger than any sanitiser:
//!
//! - a tracking pixel cannot fire from text that was never given to a renderer,
//!   so "block remote content" is not a setting that can be got wrong;
//! - every sanitiser bypass in history is a renderer disagreeing with a
//!   sanitiser about what some bytes mean, and there is only one parser here.
//!
//! What the reader does instead is *tell the user what the message tried*: that
//! it wanted to load remote content, that it hid text from them, and whether it
//! was sent by the domain it claims.

use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Apply as _, Element};

use crate::app::Message;
use crate::fl;
use crate::mail::Opened;

pub struct Reader<'a> {
    pub opened: Option<&'a Opened>,
    pub error: Option<&'a str>,
    /// False when the account has no From address. The reply buttons are shown
    /// greyed rather than hidden: a missing button reads as a missing feature,
    /// and the fix is one page away.
    pub can_send: bool,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn view(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        if let Some(error) = self.error {
            return crate::ui::destructive(error.to_owned())
                .apply(widget::container)
                .padding(spacing.space_m)
                .into();
        }

        let Some(opened) = self.opened else {
            return widget::text::body(fl!("no-message-selected"))
                .apply(widget::container)
                .center(Length::Fill)
                .into();
        };

        let message = &opened.message;
        let mut column = widget::column::with_capacity(8)
            .spacing(spacing.space_s)
            .padding(spacing.space_m)
            .push(widget::text::title3(if message.subject.is_empty() {
                fl!("no-subject")
            } else {
                message.subject.clone()
            }))
            .push(self.header(opened))
            .push(self.actions(opened));

        for notice in self.notices(opened) {
            column = column.push(notice);
        }

        if !message.attachments.is_empty() {
            column = column.push(self.attachments(opened));
        }

        column = column.push(widget::divider::horizontal::default()).push(
            widget::text::body(message.body.text.clone())
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );

        widget::scrollable(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn header(&self, opened: &'a Opened) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let message = &opened.message;

        let sender = message.sender().map_or_else(
            || fl!("unknown-sender"),
            |from| match &from.name {
                Some(name) => format!("{name} <{}>", from.address),
                None => from.address.clone(),
            },
        );

        let recipients = message
            .to
            .iter()
            .chain(message.cc.iter())
            .map(|to| to.display().to_owned())
            .collect::<Vec<_>>()
            .join(", ");

        let mut column = widget::column::with_capacity(3)
            .spacing(spacing.space_xxxs)
            .push(
                widget::row::with_capacity(2)
                    .align_y(Alignment::Center)
                    .spacing(spacing.space_xxs)
                    .push(widget::text::body(sender).width(Length::Fill))
                    .push(widget::text::caption(crate::ui::relative_date(
                        message.date,
                    ))),
            );

        if !recipients.is_empty() {
            column = column.push(widget::text::caption(fl!(
                "to-line",
                recipients = recipients
            )));
        }

        column.into()
    }

    fn actions(&self, opened: &'a Opened) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let reply = |label: String, message: Message| {
            let button = widget::button::text(label);
            if self.can_send {
                button.on_press(message)
            } else {
                button
            }
        };

        let answering = widget::row::with_capacity(3)
            .spacing(spacing.space_xxs)
            .push(
                reply(fl!("reply"), Message::Reply { all: false })
                    .class(cosmic::theme::Button::Suggested),
            )
            .push(reply(fl!("reply-all"), Message::Reply { all: true }))
            .push(reply(fl!("forward"), Message::Forward));

        let mut filing = widget::row::with_capacity(6)
            .spacing(spacing.space_xxs)
            .push(
                widget::button::text(if opened.flags.seen {
                    fl!("mark-unread")
                } else {
                    fl!("mark-read")
                })
                .on_press(Message::ToggleRead),
            )
            .push(
                widget::button::text(if opened.flags.flagged {
                    fl!("unstar")
                } else {
                    fl!("star")
                })
                .on_press(Message::ToggleFlagged),
            )
            .push(widget::button::text(fl!("archive")).on_press(Message::Archive))
            .push(
                widget::button::text(fl!("delete"))
                    .class(cosmic::theme::Button::Destructive)
                    .on_press(Message::Delete),
            )
            .push(widget::button::text(fl!("save-as-file")).on_press(Message::ExportMessage));

        // Only for mail that is a mailing, which is exactly what the header's
        // presence says. Everything else showing an Unsubscribe button would
        // be a button that does nothing on most of the mailbox.
        if crate::mail::unsubscribe_route(&opened.message).is_some() {
            filing = filing
                .push(widget::button::text(fl!("unsubscribe")).on_press(Message::Unsubscribe));
        }

        widget::column::with_capacity(2)
            .spacing(spacing.space_xxs)
            .push(answering)
            .push(filing)
            .into()
    }

    /// What the message tried to do, when it tried something worth saying.
    ///
    /// Failures are shown and passes are not. A green tick on the 99% of mail
    /// that authenticates correctly trains people to ignore the indicator,
    /// which is precisely how it stops working on the one message where it
    /// mattered.
    fn notices(&self, opened: &'a Opened) -> Vec<Element<'a, Message>> {
        let mut notices: Vec<Element<'a, Message>> = Vec::new();

        // A bounce read as correspondence is MTA prose; read as a report it
        // is one line per failed recipient, in words. The raw text stays
        // below for the diagnosis the line cannot carry.
        for (recipient, reason) in &opened.bounces {
            notices.push(crate::ui::destructive(fl!(
                "bounce-notice",
                recipient = recipient.clone(),
                reason = reason.clone()
            )));
        }

        match opened.auth {
            "fail" => notices.push(crate::ui::destructive(fl!(
                "auth-fail",
                domain = opened
                    .message
                    .sender()
                    .map_or_else(String::new, |from| from.domain().to_owned())
            ))),
            "partial" => notices.push(widget::text::caption(fl!("auth-partial")).into()),
            _ => {}
        }

        if opened.message.body.hidden_elided > 0 {
            notices.push(
                widget::text::caption(fl!(
                    "hidden-content",
                    count = opened.message.body.hidden_elided
                ))
                .wrapping(cosmic::iced::core::text::Wrapping::Word)
                .into(),
            );
        }

        if opened.message.has_remote_content {
            notices.push(widget::text::caption(fl!("remote-content-blocked")).into());
        }

        notices
    }

    fn attachments(&self, opened: &'a Opened) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let attachments: Vec<_> = opened
            .message
            .attachments
            .iter()
            .filter(|a| !a.inline)
            .collect();

        let mut column = widget::column::with_capacity(attachments.len() + 1)
            .spacing(spacing.space_xxxs)
            .push(widget::text::heading(fl!("attachments")));

        for (index, attachment) in opened
            .message
            .attachments
            .iter()
            .enumerate()
            .filter(|(_, a)| !a.inline)
        {
            // Saved on request, never opened for the user. An attachment that
            // opens itself is the oldest delivery mechanism there is, and the
            // one step between "saved" and "ran" is the whole defence.
            column = column.push(
                widget::row::with_capacity(2)
                    .align_y(Alignment::Center)
                    .spacing(spacing.space_xxs)
                    .push(
                        widget::text::caption(format!(
                            "{} · {} · {}",
                            attachment.name,
                            attachment.mime_type,
                            crate::ui::size(attachment.size)
                        ))
                        .width(Length::Fill),
                    )
                    .push(
                        widget::button::text(fl!("save")).on_press(Message::SaveAttachment(index)),
                    ),
            );
        }
        column.into()
    }
}
