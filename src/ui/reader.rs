// SPDX-License-Identifier: GPL-3.0-only

//! The reader.
//!
//! # What is deliberately not here
//!
//! A web engine, and anything that can make a request. The reader shows an
//! HTML message's *structure* — its headings, lists, tables and the
//! blockquotes a thread is genuinely made of — by reading it into a Nib
//! document. That is not a retreat from the display-security position; it is
//! the same position, and both halves of it still hold:
//!
//! - **A tracking pixel cannot fire.** Not because it is blocked, but because
//!   nothing downstream can fetch: Nib has no image loader, and an `<img>`
//!   becomes a node that draws its alt text. "Block remote content" remains a
//!   setting that cannot be got wrong, because there is no code path it would
//!   have to switch off.
//! - **There is still one parser.** Every sanitiser bypass in history is a
//!   renderer disagreeing with a sanitiser about what some bytes mean.
//!   `nib-html` reads with html5ever — the same parser
//!   `cosmic_pim_mail::text` walks — into a *schema*, and the schema is the
//!   allow-list. A `<script>`, a `<style>` and an inline `style=` are not
//!   stripped; there is nowhere in the model to put them, so they cannot
//!   survive the read. That is a stronger claim than sanitising, because it
//!   is a property of the data structure rather than of a filter's coverage.
//!
//! What the reader does on top of that is *tell the user what the message
//! tried*: that it wanted to load remote content, that it hid text from them,
//! and whether it was sent by the domain it claims.
//!
//! # Two bodies, two shapes
//!
//! A `text/plain` message keeps the older path, because it has structure a
//! document does not: its quoted history is `>` markers, and folding that
//! history is what makes the fifth reply in a thread readable. See
//! [`nib_text::quote`]. A document has no markers to fold, so converting plain
//! text into one would lose the fold to gain nothing.
//!
//! # What it does do with a plain-text body
//!
//! Show its structure. A plain-text body is not a paragraph, it is what was
//! written now on top of what was written before, and [`nib_text::quote`]
//! is what separates the two. Quoted history arrives folded, because on the
//! fifth reply of a thread the two lines that are new are the message and the
//! forty below them are furniture. An HTML message gets this from its
//! `<blockquote>` elements; a plain-text reader has to find it for itself,
//! because a blockquote is a structure rather than indented prose.

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
            return crate::ui::empty_state(fl!("no-message-selected"));
        };

        let message = &opened.message;
        let mut column = widget::column::with_capacity(8)
            .spacing(spacing.space_s)
            .padding(spacing.space_m)
            // Title 4 rather than Title 3. A subject is a label on the
            // message, not a headline: at 24px an ordinary two-clause subject
            // takes three lines of a narrow reading pane and pushes the
            // message itself under the fold, which is the one thing the pane
            // exists to show.
            .push(widget::text::title4(if message.subject.is_empty() {
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
            .push(self.body(opened));

        widget::scrollable(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// The message, as the structure it actually has.
    ///
    /// Two paths, because the two kinds of body have different structure to
    /// show. An HTML message has its own — headings, lists, tables, the
    /// blockquotes a thread is genuinely made of — and is rendered as a
    /// read-only document. A `text/plain` message has only what its `>`
    /// markers say, and that is what the folding path below reads.
    fn body(&self, opened: &'a Opened) -> Element<'a, Message> {
        match &opened.body_doc {
            Some(doc) => self.rich_body(doc),
            None => self.text_body(&opened.message.body.text),
        }
    }

    /// An HTML message, as a document nothing can type into.
    ///
    /// Read-only is the whole point and it is enforced by the widget rather
    /// than by not wiring a handler: without it the editor would still apply
    /// edits to its own working copy, and a received message would appear to
    /// accept typing that went nowhere.
    ///
    /// A link is the one thing that acts. It opens in the user's browser,
    /// which is where a decision about visiting somebody else's URL belongs.
    fn rich_body(&self, state: &'a nib_model::state::EditorState) -> Element<'a, Message> {
        nib::editor(state)
            .read_only()
            .style(nib::Style::from_theme(&cosmic::theme::active()))
            .on_action(|action| match action {
                nib::Action::Link(href) => Message::LaunchUrl(href),
                _ => Message::Ignored,
            })
            .into()
    }

    /// A `text/plain` message, split into what is new and what is quoted.
    fn text_body(&self, text: &str) -> Element<'a, Message> {
        use nib_text::quote::Block;

        let spacing = cosmic::theme::spacing();
        let blocks = nib_text::quote::blocks(text);

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
        // No left padding: this control sits in the flow of the message, and
        // a button's own inset would step its label in from the column the
        // prose above it runs down.
        .padding([spacing.space_xxs, 0])
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

        // Who wrote it, then where from. One line held both — "Marta
        // Halvorsen <marta@fjordline.no>" — which reads as an address with a
        // name attached rather than as a person. The name is what identifies
        // the sender to the reader; the address is what identifies them to
        // the machine, and is the part worth checking rather than the part
        // worth reading first.
        let from = message.sender();
        let name = from.and_then(|from| from.name.clone());
        let address = from.map_or_else(|| fl!("unknown-sender"), |from| from.address.clone());

        let recipients = message
            .to
            .iter()
            .chain(message.cc.iter())
            .map(|to| to.display().to_owned())
            .collect::<Vec<_>>()
            .join(", ");

        let mut identity = widget::column::with_capacity(2).spacing(spacing.space_xxxs);
        identity = match name {
            Some(name) => identity
                .push(widget::text::heading(name))
                .push(crate::ui::muted(address).size(12)),
            // Mail from an address with no display name says the address
            // once, in the place the name would have been, rather than
            // twice down two lines.
            None => identity.push(widget::text::heading(address)),
        };

        let mut column = widget::column::with_capacity(3)
            .spacing(spacing.space_xxs)
            .push(
                widget::row::with_capacity(2)
                    .align_y(Alignment::Start)
                    .spacing(spacing.space_xxs)
                    .push(identity.width(Length::Fill))
                    .push(widget::text::caption(crate::ui::relative_date(
                        message.date,
                    ))),
            );

        if !recipients.is_empty() {
            column =
                column.push(crate::ui::muted(fl!("to-line", recipients = recipients)).size(12));
        }

        column.into()
    }

    /// Everything that can be done to the open message.
    ///
    /// Words for answering, icons for filing, on two lines of their own. Answering is the decision the
    /// reader is making — reply, reply to everyone, or pass it on — and the
    /// difference between those three is worth spelling out. Filing is a
    /// verb applied to a message already read: which one is wanted is known
    /// before the bar is looked at, and a row of eight words is a sentence
    /// that has to be read to find any of them. It is also the arrangement
    /// every mail client with a reading pane arrived at, for the same reason:
    /// the pane is narrow, and words do not fit.
    ///
    /// Two rows rather than one wrapping row. A single row of nine controls
    /// wraps wherever the pane's width happens to put it — mid-group, so a
    /// lone icon ends up beside Forward reading as a status dot rather than a
    /// button. Splitting at the seam that is actually there keeps the break
    /// in the same place at every width.
    ///
    /// Only Reply is filled. A toolbar where two buttons are filled has no
    /// primary action, and Delete in the theme's destructive fill made the
    /// loudest thing in the reader a button that moves a message to Trash —
    /// recoverable, and pressed dozens of times a day. That fill is kept for
    /// what it is for: the confirm button of a dialog about something that
    /// cannot be undone.
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

        let answering: Vec<Element<'a, Message>> = vec![
            reply(fl!("reply"), Message::Reply { all: false })
                .class(cosmic::theme::Button::Suggested)
                .into(),
            reply(fl!("reply-all"), Message::Reply { all: true }).into(),
            reply(fl!("forward"), Message::Forward).into(),
        ];

        let mut filing: Vec<Element<'a, Message>> = vec![
            if opened.flags.seen {
                crate::ui::icon_button(
                    "mail-mark-unread-symbolic",
                    fl!("mark-unread"),
                    Message::ToggleRead,
                )
            } else {
                crate::ui::icon_button(
                    "mail-mark-read-symbolic",
                    fl!("mark-read"),
                    Message::ToggleRead,
                )
            },
            crate::ui::icon_button(
                crate::ui::STAR,
                if opened.flags.flagged {
                    fl!("unstar")
                } else {
                    fl!("star")
                },
                Message::ToggleFlagged,
            ),
            crate::ui::icon_button("mail-archive-symbolic", fl!("archive"), Message::Archive),
            crate::ui::icon_button("user-trash-symbolic", fl!("delete"), Message::Delete),
            crate::ui::icon_button(
                "document-save-symbolic",
                fl!("save-as-file"),
                Message::ExportMessage,
            ),
        ];

        // A window of its own, so the list can move on without losing the
        // message. Not offered in a window that already is one.
        if self.detachable {
            filing.push(crate::ui::icon_button(
                "window-new-symbolic",
                fl!("detach"),
                Message::Detach,
            ));
        }

        // Only for mail that is a mailing, which is exactly what the header's
        // presence says. Everything else showing an Unsubscribe button would
        // be a button that does nothing on most of the mailbox. Kept as a
        // word: it is rare, it is not a filing action, and no icon means it.
        if crate::mail::unsubscribe_route(&opened.message).is_some() {
            filing.push(
                widget::button::text(fl!("unsubscribe"))
                    .on_press(Message::Unsubscribe)
                    .into(),
            );
        }

        widget::column::with_capacity(2)
            .spacing(spacing.space_xxs)
            .push(
                widget::flex_row(answering)
                    .spacing(spacing.space_xxs)
                    .align_items(Alignment::Center)
                    .width(Length::Fill),
            )
            .push(
                widget::flex_row(filing)
                    .spacing(spacing.space_xxs)
                    .align_items(Alignment::Center)
                    .width(Length::Fill),
            )
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
                    notices.push(crate::ui::notice(
                        fl!("pgp-invalid"),
                        crate::ui::Tone::Warning,
                    ));
                }
                Verdict::SignerMismatch { signer } => notices.push(crate::ui::notice(
                    fl!("pgp-mismatch", signer = signer.clone()),
                    crate::ui::Tone::Warning,
                )),
                Verdict::UnknownSigner if !opened.pgp.encrypted => {
                    notices.push(crate::ui::notice(fl!("pgp-unknown"), crate::ui::Tone::Note));
                }
                Verdict::UnknownSigner | Verdict::Unsigned | Verdict::Verified { .. } => {}
            }
            if opened.pgp.encrypted {
                notices.push(crate::ui::notice(
                    fl!("pgp-encrypted"),
                    crate::ui::Tone::Note,
                ));
            }
        }

        // A bounce read as correspondence is MTA prose; read as a report it
        // is one line per failed recipient, in words. The raw text stays
        // below for the diagnosis the line cannot carry.
        for (recipient, reason) in &opened.bounces {
            notices.push(crate::ui::notice(
                fl!(
                    "bounce-notice",
                    recipient = recipient.clone(),
                    reason = reason.clone()
                ),
                crate::ui::Tone::Warning,
            ));
        }

        match opened.auth {
            "fail" => notices.push(crate::ui::notice(
                fl!(
                    "auth-fail",
                    domain = opened
                        .message
                        .sender()
                        .map_or_else(String::new, |from| from.domain().to_owned())
                ),
                crate::ui::Tone::Warning,
            )),
            "partial" => notices.push(crate::ui::notice(
                fl!("auth-partial"),
                crate::ui::Tone::Note,
            )),
            _ => {}
        }

        if opened.message.body.hidden_elided > 0 {
            notices.push(crate::ui::notice(
                fl!("hidden-content", count = opened.message.body.hidden_elided),
                crate::ui::Tone::Note,
            ));
        }

        if opened.message.has_remote_content {
            notices.push(crate::ui::notice(
                fl!("remote-content-blocked"),
                crate::ui::Tone::Note,
            ));
        }

        notices
    }

    /// What came with the message, as objects rather than as a list of file
    /// names.
    ///
    /// One card per attachment, on the same ground the notices use. An
    /// attachment is not part of what the sender wrote — it is a thing
    /// fastened to it — and a caption line among the prose is the one shape
    /// that does not say so. The card also gives the name room to be read at
    /// body size: a file name is the only evidence the reader has about what
    /// they are being asked to save.
    fn attachments(&self, opened: &'a Opened) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let mut column = widget::column::with_capacity(opened.message.attachments.len() + 1)
            .spacing(spacing.space_xxs)
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

            let described = widget::column::with_capacity(2)
                .spacing(spacing.space_xxxs)
                .push(
                    widget::text::body(attachment.name.clone())
                        .wrapping(cosmic::iced::core::text::Wrapping::None)
                        .ellipsize(cosmic::iced::core::text::Ellipsize::End(
                            cosmic::iced::core::text::EllipsizeHeightLimit::Lines(1),
                        )),
                )
                .push(crate::ui::muted(format!(
                    "{} · {}",
                    attachment.mime_type,
                    crate::ui::size(attachment.size)
                )))
                .width(Length::Fill);

            let row = widget::row::with_capacity(4)
                .align_y(Alignment::Center)
                .spacing(spacing.space_s)
                .push(widget::icon::from_name("mail-attachment-symbolic").size(16))
                .push(described)
                .push_maybe(importable.then(|| {
                    widget::button::text(fl!("pgp-import-key"))
                        .on_press(Message::PgpKeyImport(index))
                }))
                .push(widget::button::text(fl!("save")).on_press(Message::SaveAttachment(index)));

            column = column.push(
                widget::container(row)
                    .padding([spacing.space_xs, spacing.space_s])
                    .width(Length::Fill)
                    .class(cosmic::theme::Container::Card),
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
