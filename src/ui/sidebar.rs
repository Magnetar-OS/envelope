// SPDX-License-Identifier: GPL-3.0-only

//! The sidebar: which account, and which folder.

use cosmic::iced::Length;
use cosmic::widget;
use cosmic::{Apply as _, Element};
use cosmic_pim_mail::folder::Folder;

use crate::app::Message;
use crate::fl;

/// Padding either side of the sidebar's contents.
const GUTTER: u16 = 8;

pub struct Sidebar<'a> {
    pub accounts: &'a [cosmic_pim_accounts::Account],
    pub selected_account: Option<&'a str>,
    pub folders: &'a [Folder],
    pub selected_folder: Option<usize>,
    pub unread: &'a std::collections::HashMap<String, usize>,
    /// Local drafts. Listed above the server's folders and labelled, because
    /// they are on this device and nowhere else — presenting them as just
    /// another folder would imply they sync.
    pub drafts: usize,
    pub showing_drafts: bool,
    /// Messages waiting to go out. Shown whenever there are any: a message the
    /// user believes they sent, sitting in a queue they cannot see, is the
    /// worst thing an outbox can do.
    pub outbox: usize,
    pub showing_outbox: bool,
    /// Whether the merged view is worth offering — two or more accounts have
    /// mail. With one, it is the inbox with an extra name.
    pub offer_unified: bool,
    pub showing_unified: bool,
}

impl<'a> Sidebar<'a> {
    #[must_use]
    pub fn view(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        widget::column::with_capacity(3)
            .spacing(spacing.space_s)
            .push(self.unified_row())
            .push(self.account_picker())
            .push(self.drafts_row())
            .push(self.outbox_row())
            .push(self.folder_list())
            .push(widget::Space::new().height(Length::Fill))
            .padding(GUTTER)
            .apply(widget::container)
            // Wears the desktop's own nav-bar surface, so it matches the
            // sidebar in cosmic-files and cosmic-settings rather than
            // approximating it.
            .class(cosmic::theme::Container::custom(
                widget::nav_bar::nav_bar_style,
            ))
            .height(Length::Fill)
            .into()
    }

    /// Only shown when there is a choice to make.
    ///
    /// One account is the overwhelmingly common case, and a picker with one
    /// entry is a control that can only ever do nothing.
    fn account_picker(&self) -> Element<'a, Message> {
        if self.accounts.len() < 2 {
            return widget::Space::new().height(Length::Fixed(0.0)).into();
        }
        let mut column = widget::column::with_capacity(self.accounts.len())
            .spacing(cosmic::theme::spacing().space_xxxs);
        for account in self.accounts {
            let selected = self.selected_account == Some(account.id.as_str());
            column = column.push(
                widget::button::text(account.display_name.clone())
                    .width(Length::Fill)
                    .class(if selected {
                        cosmic::theme::Button::Suggested
                    } else {
                        cosmic::theme::Button::Text
                    })
                    .on_press(Message::AccountSelected(account.id.clone())),
            );
        }
        column.into()
    }

    fn unified_row(&self) -> Element<'a, Message> {
        if !self.offer_unified {
            return widget::Space::new().height(Length::Fixed(0.0)).into();
        }
        widget::button::custom(widget::text::body(fl!("all-inboxes")).width(Length::Fill))
            .width(Length::Fill)
            .class(if self.showing_unified {
                cosmic::theme::Button::Suggested
            } else {
                cosmic::theme::Button::Text
            })
            .on_press(Message::ShowUnified)
            .into()
    }

    fn drafts_row(&self) -> Element<'a, Message> {
        if self.drafts == 0 && !self.showing_drafts {
            return widget::Space::new().height(Length::Fixed(0.0)).into();
        }
        let spacing = cosmic::theme::spacing();
        let row = widget::row::with_capacity(2)
            .align_y(cosmic::iced::Alignment::Center)
            .spacing(spacing.space_xxs)
            .push(widget::text::body(fl!("drafts")).width(Length::Fill))
            .push(widget::text::caption(self.drafts.to_string()));

        widget::button::custom(row)
            .width(Length::Fill)
            .class(if self.showing_drafts {
                cosmic::theme::Button::Suggested
            } else {
                cosmic::theme::Button::Text
            })
            .on_press(Message::ShowDrafts)
            .into()
    }

    fn outbox_row(&self) -> Element<'a, Message> {
        if self.outbox == 0 && !self.showing_outbox {
            return widget::Space::new().height(Length::Fixed(0.0)).into();
        }
        let spacing = cosmic::theme::spacing();
        let row = widget::row::with_capacity(2)
            .align_y(cosmic::iced::Alignment::Center)
            .spacing(spacing.space_xxs)
            .push(widget::text::body(fl!("outbox")).width(Length::Fill))
            .push(widget::text::caption(self.outbox.to_string()));

        widget::button::custom(row)
            .width(Length::Fill)
            .class(if self.showing_outbox {
                cosmic::theme::Button::Suggested
            } else {
                cosmic::theme::Button::Text
            })
            .on_press(Message::ShowOutbox)
            .into()
    }

    fn folder_list(&self) -> Element<'a, Message> {
        if self.folders.is_empty() {
            return widget::text::caption(fl!("no-folders"))
                .wrapping(cosmic::iced::core::text::Wrapping::Word)
                .into();
        }

        let spacing = cosmic::theme::spacing();
        let mut column =
            widget::column::with_capacity(self.folders.len()).spacing(spacing.space_xxxs);

        for (index, folder) in self.folders.iter().enumerate() {
            let selected = self.selected_folder == Some(index);
            let unread = self.unread.get(&folder.wire_name).copied().unwrap_or(0);

            // Nesting is shown by indentation rather than by a collapsible
            // tree: a mail folder tree is browsed far more often than it is
            // restructured, and every expander is a click between the user and
            // a folder they can already see.
            let indent = f32::from(u16::try_from(folder.depth().min(4)).unwrap_or(0)) * 12.0;

            let mut row = widget::row::with_capacity(3)
                .align_y(cosmic::iced::Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(widget::Space::new().width(Length::Fixed(indent)))
                .push(widget::text::body(folder.leaf_name().to_owned()).width(Length::Fill));

            if unread > 0 {
                row = row.push(widget::text::caption(unread.to_string()));
            }

            column = column.push(
                widget::button::custom(row)
                    .width(Length::Fill)
                    .class(if selected {
                        cosmic::theme::Button::Suggested
                    } else {
                        cosmic::theme::Button::Text
                    })
                    .on_press(Message::FolderSelected(index)),
            );
        }

        column.into()
    }
}
