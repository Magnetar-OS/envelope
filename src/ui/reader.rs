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
//!
//! # What it does do with the text
//!
//! Show its structure. A plain-text body is not a paragraph, it is what was
//! written now on top of what was written before, and [`crate::text::blocks`]
//! is what separates the two. Quoted history arrives folded, because on the
//! fifth reply of a thread the two lines that are new are the message and the
//! forty below them are furniture. A Markdown renderer gets this for free
//! from the syntax; a plain-text reader has to find it for itself, because a
//! blockquote is a structure rather than indented prose.

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
    /// Which quoted runs the user has asked to see, by their index among the
    /// message's blocks. Empty is the state every message opens in.
    pub expanded_quotes: &'a std::collections::HashSet<usize>,
    /// Whether to offer the message a window of its own. False when this
    /// *is* that window.
    pub detachable: bool,
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

        if !opened.labels.is_empty() {
            let mut chips = widget::row::with_capacity(opened.labels.len())
                .spacing(spacing.space_xxs)
                .align_y(cosmic::iced::Alignment::Center);
            for name in &opened.labels {
                chips = chips.push(crate::ui::label_chip(name.clone()));
            }
            column = column.push(chips);
        }

        for notice in self.notices(opened) {
            column = column.push(notice);
        }

        // The iMIP hand-off. The decision about an invitation belongs in the
        // calendar; this button only moves the payload there. When Slate is
        // not running the press says so, and the part is still saveable below
        // like any attachment — degradation is part of the contract.
        if opened.invitation.is_some() {
            column = column.push(
                widget::row::with_capacity(2)
                    .align_y(Alignment::Center)
                    .spacing(spacing.space_xxs)
                    .push(widget::text::caption(fl!("invitation-notice")).width(Length::Fill))
                    .push(
                        widget::button::text(fl!("open-in-calendar"))
                            .class(cosmic::theme::Button::Suggested)
                            .on_press(Message::OpenInCalendar),
                    ),
            );
        }

        if !message.attachments.is_empty() {
            column = column.push(self.attachments(opened));
        }

        column = column
            .push(widget::divider::horizontal::default())
            .push(self.body(&message.body.text));

        widget::scrollable(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// The message text, as the structure it actually has.
    fn body(&self, text: &str) -> Element<'a, Message> {
        use crate::text::Block;

        let spacing = cosmic::theme::spacing();
        let blocks = crate::text::blocks(text);

        // A body that is only quoting, or only a signature, still has to
        // appear — but a genuinely empty one gets nothing rather than an
        // empty box.
        if blocks.is_empty() {
            return widget::Space::new().into();
        }

        let mut column = widget::column::with_capacity(blocks.len()).spacing(spacing.space_s);
        for (index, block) in blocks.into_iter().enumerate() {
            column = column.push(match block {
                Block::Prose(text) => prose(text),
                Block::Signature(text) => signature(text),
                Block::Quoted { depth, text } => self.quoted(index, depth, text),
            });
        }
        column.into()
    }

    /// One run of quoted history: a control that says how much there is, and
    /// the text itself once it has been asked for.
    ///
    /// Folded by default. The alternative — showing it and letting the user
    /// scroll — is what makes a long thread unreadable, and the count is on
    /// the control precisely so that folding never hides *how much* was
    /// folded.
    fn quoted(&self, index: usize, depth: usize, text: String) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let expanded = self.expanded_quotes.contains(&index);
        let lines = text.lines().count();

        let toggle = widget::button::text(if expanded {
            fl!("hide-quoted")
        } else {
            fl!("show-quoted", lines = lines)
        })
        .on_press(Message::ToggleQuote(index));

        let mut column = widget::column::with_capacity(2)
            .spacing(spacing.space_xxs)
            .push(
                widget::row::with_capacity(2)
                    .align_y(Alignment::Center)
                    .spacing(spacing.space_xxs)
                    .push(toggle)
                    // Only worth saying when it is more than one deep: "quoted
                    // text" already implies one level, and a caption on every
                    // reply is a caption nobody reads.
                    .push_maybe(
                        (depth > 1)
                            .then(|| widget::text::caption(fl!("quote-depth", depth = depth))),
                    ),
            );

        if expanded {
            column = column.push(
                widget::container(quoted_text(text))
                    .padding([spacing.space_xxs, spacing.space_s])
                    .class(cosmic::theme::Container::custom(|theme| {
                        let cosmic = theme.cosmic();
                        // The same soft-accent ground the label chips use, so
                        // "not written by this sender" reads the same way
                        // everywhere in the window.
                        let mut accent = cosmic.accent_color();
                        accent.alpha = 0.08;
                        widget::container::Style {
                            background: Some(cosmic::iced::Background::Color(accent.into())),
                            border: cosmic::iced::Border {
                                radius: cosmic.corner_radii.radius_s.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })),
            );
        }

        column.into()
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

        let mut filing = widget::row::with_capacity(7)
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

        // A window of its own, so the list can move on without losing the
        // message. Not offered in a window that already is one.
        if self.detachable {
            filing = filing.push(widget::button::text(fl!("detach")).on_press(Message::Detach));
        }

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

        // The OpenPGP contract: a broken signature is loud, an unknown or
        // mis-bound signer is a caption, a good signature from the sender is
        // quiet — a tick on every verified message trains people to ignore
        // it. Encryption is stated whatever the verdict, because it changes
        // what a reply may safely quote.
        {
            use cosmic_pim_mail::pgp::Verdict;
            match &opened.pgp.verdict {
                Verdict::Invalid => {
                    notices.push(crate::ui::destructive(fl!("pgp-invalid")));
                }
                Verdict::SignerMismatch { signer } => notices.push(
                    widget::text::caption(fl!("pgp-mismatch", signer = signer.clone())).into(),
                ),
                Verdict::UnknownSigner if !opened.pgp.encrypted => {
                    notices.push(widget::text::caption(fl!("pgp-unknown")).into());
                }
                Verdict::UnknownSigner | Verdict::Unsigned | Verdict::Verified { .. } => {}
            }
            if opened.pgp.encrypted {
                notices.push(widget::text::caption(fl!("pgp-encrypted")).into());
            }
        }

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
            let importable = attachment
                .mime_type
                .eq_ignore_ascii_case("application/pgp-keys");
            column = column.push(
                widget::row::with_capacity(3)
                    .align_y(Alignment::Center)
                    .spacing(spacing.space_xxs)
                    .push_maybe(importable.then(|| {
                        widget::button::text(fl!("pgp-import-key"))
                            .on_press(Message::PgpKeyImport(index))
                    }))
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

/// The message's own words, selectable so they can be copied out.
///
/// The metrics restate `widget::text::body`'s preset by hand because
/// `selectable_text` carries no typography presets — not in the pinned
/// revision, and not in any revision: `body` exists only on the
/// non-selectable builder in `widget/text.rs`. So this is not waiting on a pin
/// move and will not collapse on its own. It has to be kept in step with
/// `text::body` by hand; if that preset's size or line height changes, this is
/// the other half that must change with it.
fn prose<'a>(text: String) -> Element<'a, Message> {
    widget::selectable_text(text)
        .size(14.0)
        .line_height(cosmic::iced::widget::text::LineHeight::Absolute(
            21.0.into(),
        ))
        .font(cosmic::font::default())
        .wrapping(cosmic::iced::core::text::Wrapping::Word)
        .into()
}

/// Quoted history, dimmed. Still selectable — quoting back out of a quote is
/// one of the things a reply is for.
fn quoted_text<'a>(text: String) -> Element<'a, Message> {
    dimmed(prose_builder(text)).into()
}

/// The sender's signature. Dimmed and set apart, because it is the same four
/// lines under every message they have ever sent.
fn signature<'a>(text: String) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    widget::column::with_capacity(2)
        .spacing(spacing.space_xxs)
        .push(widget::divider::horizontal::light())
        .push(dimmed(prose_builder(text).size(12.0).line_height(
            cosmic::iced::widget::text::LineHeight::Absolute(18.0.into()),
        )))
        .into()
}

/// [`prose`] before it is turned into an `Element`, so the two dimmed variants
/// can adjust it without restating the metrics a third time.
fn prose_builder<'a>(text: String) -> widget::SelectableText<'a> {
    widget::selectable_text(text)
        .size(14.0)
        .line_height(cosmic::iced::widget::text::LineHeight::Absolute(
            21.0.into(),
        ))
        .font(cosmic::font::default())
        .wrapping(cosmic::iced::core::text::Wrapping::Word)
}

/// Text at the theme's secondary emphasis — what "someone else wrote this"
/// looks like without inventing a colour.
///
/// A `class` rather than a `style`: `theme::Text::Custom` holds a plain
/// function pointer, so the closure cannot capture, and the alpha has to be a
/// constant rather than a parameter. That is the toolkit's constraint, not a
/// simplification — see the `TODO` on `Text::Custom` in libcosmic.
fn dimmed(text: widget::SelectableText<'_>) -> widget::SelectableText<'_> {
    text.class(cosmic::theme::Text::Custom(|theme| {
        let mut color = theme.cosmic().on_bg_color();
        color.alpha *= 0.7;
        cosmic::iced::widget::text::Style {
            color: Some(color.into()),
            ..Default::default()
        }
    }))
}
