// SPDX-License-Identifier: GPL-3.0-only

//! The conversation list.
//!
//! Conversations, not messages. A mailbox is a set of exchanges, and a list
//! that shows twelve rows for one back-and-forth is showing the transport
//! rather than the content.

use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Apply as _, Element};

use crate::app::Message;
use crate::fl;
use crate::mail::Conversation;

pub struct List<'a> {
    pub conversations: &'a [Conversation],
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
            return widget::text::body(text)
                .apply(widget::container)
                .padding(spacing.space_m)
                .into();
        }

        let mut column =
            widget::column::with_capacity(self.conversations.len()).spacing(spacing.space_xxxs);

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
        heading = heading.push(if conversation.unread {
            widget::text::heading(conversation.participants.clone()).width(Length::Fill)
        } else {
            widget::text::body(conversation.participants.clone()).width(Length::Fill)
        });

        if conversation.flagged {
            heading = heading.push(widget::icon::from_name("starred-symbolic").size(12));
        }
        if conversation.has_attachments {
            heading = heading.push(widget::icon::from_name("mail-attachment-symbolic").size(12));
        }
        heading = heading.push(widget::text::caption(crate::ui::relative_date(
            conversation.date,
        )));

        let mut subject_line = widget::row::with_capacity(2)
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs)
            .push(if conversation.unread {
                widget::text::heading(conversation.subject.clone()).width(Length::Fill)
            } else {
                widget::text::body(conversation.subject.clone()).width(Length::Fill)
            });

        if conversation.uids.len() > 1 {
            subject_line =
                subject_line.push(widget::text::caption(conversation.uids.len().to_string()));
        }

        let body = widget::column::with_capacity(3)
            .spacing(spacing.space_xxxs)
            .push(heading)
            .push(subject_line)
            .push(widget::text::caption(conversation.snippet.clone()));

        widget::button::custom(body)
            .width(Length::Fill)
            .padding(spacing.space_xs)
            .class(if selected {
                cosmic::theme::Button::Suggested
            } else {
                cosmic::theme::Button::Text
            })
            .on_press(Message::ConversationSelected(index))
            .into()
    }
}
