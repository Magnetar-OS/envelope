// SPDX-License-Identifier: GPL-3.0-only

//! The conversation list.
//!
//! Conversations, not messages. A mailbox is a set of exchanges, and a list
//! that shows twelve rows for one back-and-forth is showing the transport
//! rather than the content.

use cosmic::iced::core::text::{Ellipsize, EllipsizeHeightLimit, Wrapping};
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Apply as _, Element};

use crate::app::Message;
use crate::fl;
use crate::mail::Conversation;

/// One line, clipped with an ellipsis rather than wrapped onto a second.
///
/// A row whose height depends on how long its subject happens to be makes the
/// list jump about as it scrolls, and costs the eye the column it was reading
/// down. This is the treatment libcosmic gives its own header-bar title, and
/// what every official COSMIC list does to its rows.
fn one_line(
    text: widget::Text<'_, cosmic::Theme, cosmic::Renderer>,
) -> widget::Text<'_, cosmic::Theme, cosmic::Renderer> {
    text.wrapping(Wrapping::None)
        .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
}

pub struct List<'a> {
    pub conversations: &'a [Conversation],
    /// Each conversation's label chips, parallel to `conversations`.
    pub labels: &'a [Vec<String>],
    pub selected: Option<usize>,
    pub loading: bool,
    /// Set when the folder could not be read at all.
    pub error: Option<&'a str>,
}

impl<'a> List<'a> {
    #[must_use]
    pub fn view(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        if let Some(error) = self.error {
            return crate::ui::destructive(error.to_owned())
                .apply(widget::container)
                .padding(spacing.space_m)
                .into();
        }

        if self.conversations.is_empty() {
            // Distinguished on purpose: "nothing here yet" and "nothing here"
            // look identical and mean completely different things.
            let text = if self.loading {
                fl!("loading")
            } else {
                fl!("empty-folder")
            };
            return crate::ui::empty_state(text);
        }

        // The gap between rows is deliberately wider than the gap between a
        // row's own lines. That difference is the whole reason a three-line
        // row reads as one thing: with both set to the same step the lines
        // pool into an undifferentiated column of text, and the eye has to
        // parse the content to find out where each message begins.
        let mut column =
            widget::column::with_capacity(self.conversations.len()).spacing(spacing.space_xxs);

        for (index, conversation) in self.conversations.iter().enumerate() {
            column = column.push(self.row(index, conversation));
        }

        widget::scrollable(column.padding(spacing.space_xxs))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn row(&self, index: usize, conversation: &'a Conversation) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let selected = self.selected == Some(index);

        let mut heading = widget::row::with_capacity(4)
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs);

        // Unread is carried by weight rather than by a dot: it is the property
        // the eye needs to pick up while scrolling, and a bold row reads at a
        // glance where a marker has to be looked for.
        let participants = conversation.participants.join(", ");
        heading = heading.push(if conversation.unread {
            one_line(widget::text::heading(participants)).width(Length::Fill)
        } else {
            one_line(widget::text::body(participants)).width(Length::Fill)
        });

        if conversation.flagged {
            heading = heading.push(widget::icon::from_name(crate::ui::STAR).size(12));
        }
        if conversation.has_attachments {
            heading = heading.push(widget::icon::from_name("mail-attachment-symbolic").size(12));
        }
        heading = heading.push(widget::text::caption(crate::ui::relative_date_ms(
            conversation.date_ms,
        )));

        // The index stores an absent subject as empty; a blank row reads as a
        // rendering bug rather than as a subjectless message.
        let subject = if conversation.subject.is_empty() {
            fl!("no-subject")
        } else {
            conversation.subject.clone()
        };
        let mut subject_line = widget::row::with_capacity(2)
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs)
            .push(if conversation.unread {
                one_line(widget::text::heading(subject)).width(Length::Fill)
            } else {
                one_line(widget::text::body(subject)).width(Length::Fill)
            });

        if conversation.uids.len() > 1 {
            subject_line =
                subject_line.push(widget::text::caption(conversation.uids.len().to_string()));
        }

        // At most two chips and a count, so a heavily labelled thread cannot
        // stretch its row.
        if let Some(labels) = self.labels.get(index).filter(|l| !l.is_empty()) {
            for name in labels.iter().take(2) {
                subject_line = subject_line.push(crate::ui::label_chip(name.clone()));
            }
            if labels.len() > 2 {
                subject_line =
                    subject_line.push(widget::text::caption(format!("+{}", labels.len() - 2)));
            }
        }

        let mut body = widget::column::with_capacity(3)
            .spacing(spacing.space_xxxs)
            .push(heading)
            .push(subject_line);

        // A message whose body is empty — or whose only content is an
        // attachment — gets two lines rather than two lines and a gap. An
        // empty third line is indistinguishable from a rendering fault, and
        // it breaks the row rhythm exactly where the eye is relying on it.
        if !conversation.snippet.trim().is_empty() {
            body = body.push(one_line(widget::text::caption(
                conversation.snippet.clone(),
            )));
        }

        widget::button::custom(body)
            .width(Length::Fill)
            .padding([spacing.space_xs, spacing.space_s])
            .selected(selected)
            .class(crate::ui::row_class())
            .on_press(Message::ConversationSelected(index))
            .into()
    }
}

/// The saved-draft list, in the same column the conversations use.
///
/// A separate function rather than a mode on [`List`]: a draft has no sender,
/// no flags, and no thread, so every field the conversation row shows would be
/// empty or invented.
pub fn drafts(saved: &[cosmic_pim_mail::drafts::Saved]) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();

    if saved.is_empty() {
        return widget::column::with_capacity(2)
            .spacing(spacing.space_xxs)
            .push(crate::ui::muted(fl!("no-drafts")))
            .push(widget::text::caption(fl!("drafts-sync-note")))
            .apply(widget::container)
            .padding(spacing.space_m)
            .into();
    }

    let mut column = widget::column::with_capacity(saved.len() + 1)
        .spacing(spacing.space_xxs)
        .push(
            widget::text::caption(fl!("drafts-sync-note"))
                .apply(widget::container)
                .padding(spacing.space_xxs),
        );

    for draft in saved {
        let subject = if draft.subject.trim().is_empty() {
            fl!("draft-no-subject")
        } else {
            draft.subject.clone()
        };

        let body = widget::column::with_capacity(2)
            .spacing(spacing.space_xxxs)
            .push(
                widget::row::with_capacity(2)
                    .align_y(Alignment::Center)
                    .spacing(spacing.space_xxs)
                    .push(one_line(widget::text::body(subject)).width(Length::Fill))
                    .push(widget::text::caption(crate::ui::relative_date_ms(
                        draft.saved_ms,
                    ))),
            )
            .push(one_line(widget::text::caption(draft.to.clone())));

        column = column.push(
            widget::row::with_capacity(2)
                .align_y(Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(
                    widget::button::custom(body)
                        .width(Length::Fill)
                        .padding([spacing.space_xs, spacing.space_s])
                        .class(crate::ui::row_class())
                        .on_press(Message::DraftOpened(draft.id.clone())),
                )
                .push(
                    widget::button::text(fl!("delete"))
                        .class(cosmic::theme::Button::Destructive)
                        .on_press(Message::DraftDeleted(draft.id.clone())),
                ),
        );
    }

    widget::scrollable(column.padding(spacing.space_xxs))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The search results, in the same column the conversations use.
///
/// Flat, not threaded. A search result is "the message I am looking for", and
/// grouping it back into its conversation buries the hit among its siblings —
/// which is why every mail client that threads its inbox shows a flat list here.
pub fn results<'a>(
    hits: &'a [cosmic_pim_mail::Hit],
    folders: &'a [cosmic_pim_mail::Folder],
    searching: bool,
    limit: usize,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();

    if hits.is_empty() {
        let text = if searching {
            fl!("searching")
        } else {
            fl!("no-results")
        };
        return widget::column::with_capacity(2)
            .spacing(spacing.space_xxs)
            .push(widget::text::body(text))
            .push(
                widget::text::caption(fl!("search-hint"))
                    .wrapping(cosmic::iced::core::text::Wrapping::Word),
            )
            .apply(widget::container)
            .padding(spacing.space_m)
            .into();
    }

    let mut column = widget::column::with_capacity(hits.len() + 1).spacing(spacing.space_xxs);

    for (index, hit) in hits.iter().enumerate() {
        // The folder is half of what the user wanted to know: a search that
        // crosses folders has to say where it landed.
        let folder = folders
            .iter()
            .find(|folder| folder.wire_name == hit.mailbox)
            .map_or(hit.mailbox.as_str(), |folder| folder.leaf_name());

        let mut heading = widget::row::with_capacity(3)
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs)
            .push(one_line(widget::text::body(hit.from_display().to_owned())).width(Length::Fill));

        if hit.has_attachments {
            heading = heading.push(widget::icon::from_name("mail-attachment-symbolic").size(12));
        }
        heading = heading.push(widget::text::caption(crate::ui::relative_date_ms(
            hit.date_ms,
        )));

        let body = widget::column::with_capacity(3)
            .spacing(spacing.space_xxxs)
            .push(heading)
            .push(one_line(widget::text::body(hit.subject.clone())))
            .push(
                widget::row::with_capacity(2)
                    .spacing(spacing.space_xxs)
                    .push(one_line(widget::text::caption(hit.snippet.clone())).width(Length::Fill))
                    .push(widget::text::caption(fl!(
                        "in-folder",
                        folder = folder.to_owned()
                    ))),
            );

        column = column.push(
            widget::button::custom(body)
                .width(Length::Fill)
                .padding([spacing.space_xs, spacing.space_s])
                .class(crate::ui::row_class())
                .on_press(Message::HitOpened(index)),
        );
    }

    // Said rather than paged: somebody with 200 hits needs a better query, not
    // a second page, and silently truncating would read as "that is all there
    // is".
    if hits.len() >= limit {
        column = column.push(
            widget::text::caption(fl!("results-capped", count = limit))
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
    }

    widget::scrollable(column.padding(spacing.space_xxs))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The outbox, in the same column the conversations use.
pub fn outbox(queued: &[cosmic_pim_mail::outbox::Queued]) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();

    if queued.is_empty() {
        return widget::text::body(fl!("outbox-empty"))
            .apply(widget::container)
            .padding(spacing.space_m)
            .into();
    }

    let mut column = widget::column::with_capacity(queued.len()).spacing(spacing.space_xxs);

    for message in queued {
        let mut body = widget::column::with_capacity(3)
            .spacing(spacing.space_xxxs)
            .push(one_line(widget::text::body(message.describe())));

        // A stopped message says so, in the theme's alarming colour: one
        // sitting in a queue the user thinks is working is the worst thing an
        // outbox can do.
        if message.given_up {
            body = body.push(crate::ui::destructive(fl!("outbox-stopped")));
        }
        if let Some(error) = &message.last_error {
            body = body.push(
                widget::text::caption(error.clone())
                    .wrapping(cosmic::iced::core::text::Wrapping::Word),
            );
        }

        let mut row = widget::row::with_capacity(3)
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs)
            .push(widget::container(body).width(Length::Fill));

        if message.given_up {
            row = row.push(
                widget::button::text(fl!("try-again"))
                    .on_press(Message::QueuedRetried(message.id.clone())),
            );
        }
        row = row.push(
            widget::button::text(fl!("discard"))
                .class(cosmic::theme::Button::Destructive)
                .on_press(Message::QueuedDiscarded(message.id.clone())),
        );

        column = column.push(widget::container(row).padding(spacing.space_xs));
    }

    widget::scrollable(column.padding(spacing.space_xxs))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The unified inbox: every account's INBOX, merged, each row saying whose.
pub fn unified(entries: &[crate::mail::UnifiedConversation]) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();

    if entries.is_empty() {
        return crate::ui::empty_state(fl!("empty-folder"));
    }

    let mut column = widget::column::with_capacity(entries.len()).spacing(spacing.space_xxs);

    for (index, entry) in entries.iter().enumerate() {
        let conversation = &entry.conversation;

        let mut heading = widget::row::with_capacity(3)
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs)
            .push(if conversation.unread {
                one_line(widget::text::heading(conversation.participants.join(", ")))
                    .width(Length::Fill)
            } else {
                one_line(widget::text::body(conversation.participants.join(", ")))
                    .width(Length::Fill)
            });
        heading = heading.push(widget::text::caption(crate::ui::relative_date_ms(
            conversation.date_ms,
        )));

        let subject = if conversation.subject.is_empty() {
            fl!("no-subject")
        } else {
            conversation.subject.clone()
        };

        let body = widget::column::with_capacity(3)
            .spacing(spacing.space_xxxs)
            .push(heading)
            .push(one_line(widget::text::body(subject)))
            .push(
                widget::row::with_capacity(2)
                    .spacing(spacing.space_xxs)
                    .push(
                        one_line(widget::text::caption(conversation.snippet.clone()))
                            .width(Length::Fill),
                    )
                    // Whose inbox this came from is half of what a merged row
                    // has to say.
                    .push(widget::text::caption(entry.account_name.clone())),
            );

        column = column.push(
            widget::button::custom(body)
                .width(Length::Fill)
                .padding([spacing.space_xs, spacing.space_s])
                .class(crate::ui::row_class())
                .on_press(Message::UnifiedOpened(index)),
        );
    }

    widget::scrollable(column.padding(spacing.space_xxs))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
