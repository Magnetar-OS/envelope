// SPDX-License-Identifier: GPL-3.0-only

//! The sidebar: which account, and which folder.

use cosmic::iced::Length;
use cosmic::widget;
use cosmic::{Apply as _, Element};
use cosmic_pim_mail::folder::{Folder, SpecialUse};

use crate::app::Message;
use crate::fl;

/// How big a row's icon is.
///
/// Sixteen: the size the symbolic icons are drawn for, and the size every
/// COSMIC list puts in front of a row of body text. Scaled icons are the
/// difference between a sidebar that looks native and one that looks
/// approximated.
const ICON: u16 = 16;

/// The icon for a mailbox's role.
///
/// Shape is found faster than text, and the five mailboxes with roles are the
/// five a person aims at all day. Everything else is a folder and says so —
/// a distinct icon per user folder would be decoration competing with the
/// names.
fn row_icon(folder: &Folder) -> &'static str {
    match folder.special_use {
        Some(SpecialUse::Inbox) => "mail-folder-inbox-symbolic",
        Some(SpecialUse::Sent) => "mail-send-symbolic",
        Some(SpecialUse::Drafts) => "emblem-documents-symbolic",
        Some(SpecialUse::Archive) => "mail-archive-symbolic",
        Some(SpecialUse::Junk) => "mail-mark-junk-symbolic",
        Some(SpecialUse::Trash) => "user-trash-symbolic",
        None => "folder-symbolic",
    }
}

pub struct Sidebar<'a> {
    pub accounts: &'a [cosmic_pim_accounts::Account],
    pub selected_account: Option<&'a str>,
    pub folders: &'a [Folder],
    pub selected_folder: Option<usize>,
    pub unread: &'a std::collections::HashMap<String, usize>,
    /// Draft records. Listed above the server's folders because they are the
    /// editable copies this device holds; the mirror keeps the server's
    /// Drafts folder showing the same set for IMAP accounts.
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

        widget::column::with_capacity(5)
            .spacing(spacing.space_xxs)
            .push(self.unified_row())
            .push(self.account_picker())
            .push(self.drafts_row())
            .push(self.outbox_row())
            // Scrolls, and takes the space the rows above it did not. A plain
            // column made every folder past the window's edge unreachable —
            // an ordinary Gmail label set is longer than a sidebar — and it
            // is what libcosmic's own nav bar wraps its content in.
            .push(
                self.folder_list()
                    .apply(widget::scrollable)
                    .height(Length::Fill),
            )
            .padding(spacing.space_xxs)
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

    /// One sidebar row: icon, name, and a count when there is one.
    ///
    /// Every row in the sidebar is built here, so the icon column lines up
    /// down the whole pane — including through the drafts and outbox rows,
    /// which are not folders but sit among them and would otherwise start
    /// their text at a different edge.
    fn row(
        icon: &'static str,
        name: String,
        count: usize,
        indent: f32,
        selected: bool,
        press: Message,
    ) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let mut row = widget::row::with_capacity(4)
            .align_y(cosmic::iced::Alignment::Center)
            .spacing(spacing.space_xxs);

        if indent > 0.0 {
            row = row.push(widget::Space::new().width(Length::Fixed(indent)));
        }

        row = row
            .push(widget::icon::from_name(icon).size(ICON))
            .push(widget::text::body(name).width(Length::Fill));

        if count > 0 {
            row = row.push(widget::text::caption(count.to_string()));
        }

        widget::button::custom(row)
            .width(Length::Fill)
            .padding([spacing.space_xxs, spacing.space_xs])
            .selected(selected)
            .class(crate::ui::row_class())
            .on_press(press)
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
            column = column.push(Self::row(
                "mail-unread-symbolic",
                account.display_name.clone(),
                0,
                0.0,
                selected,
                Message::AccountSelected(account.id.clone()),
            ));
        }
        column.into()
    }

    fn unified_row(&self) -> Element<'a, Message> {
        if !self.offer_unified {
            return widget::Space::new().height(Length::Fixed(0.0)).into();
        }
        Self::row(
            "mail-folder-inbox-symbolic",
            fl!("all-inboxes"),
            0,
            0.0,
            self.showing_unified,
            Message::ShowUnified,
        )
    }

    fn drafts_row(&self) -> Element<'a, Message> {
        if self.drafts == 0 && !self.showing_drafts {
            return widget::Space::new().height(Length::Fixed(0.0)).into();
        }
        Self::row(
            "emblem-documents-symbolic",
            fl!("drafts"),
            self.drafts,
            0.0,
            self.showing_drafts,
            Message::ShowDrafts,
        )
    }

    fn outbox_row(&self) -> Element<'a, Message> {
        if self.outbox == 0 && !self.showing_outbox {
            return widget::Space::new().height(Length::Fixed(0.0)).into();
        }
        Self::row(
            "mail-folder-outbox-symbolic",
            fl!("outbox"),
            self.outbox,
            0.0,
            self.showing_outbox,
            Message::ShowOutbox,
        )
    }

    fn folder_list(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        if self.folders.is_empty() {
            // The one empty state with something to be done about it, so it
            // carries the doing. A sentence that names a fix and leaves the
            // reader to find it is a sentence that could have been a button.
            return widget::column::with_capacity(2)
                .spacing(spacing.space_s)
                .push(
                    crate::ui::muted(fl!("no-folders"))
                        .wrapping(cosmic::iced::core::text::Wrapping::Word),
                )
                .push(
                    widget::button::text(fl!("set-up-mail"))
                        .width(Length::Fill)
                        .class(crate::ui::row_class())
                        .on_press(Message::OpenAccounts),
                )
                .padding(spacing.space_xs)
                .into();
        }

        // Which folder is *remembered* is not the same as which view is on
        // screen. Opening Drafts left the last folder still drawn as selected,
        // so the sidebar showed two selected rows and neither of them was a
        // lie — the folder is still where the list will return to. It just
        // is not what is being looked at.
        let showing_a_folder =
            !(self.showing_drafts || self.showing_outbox || self.showing_unified);

        let mut column =
            widget::column::with_capacity(self.folders.len()).spacing(spacing.space_xxxs);

        for (index, folder) in self.folders.iter().enumerate() {
            // Nesting is shown by indentation rather than by a collapsible
            // tree: a mail folder tree is browsed far more often than it is
            // restructured, and every expander is a click between the user and
            // a folder they can already see.
            // One step of the theme's own rhythm per level, capped at four:
            // deeper than that and the name has nowhere left to go.
            let indent =
                f32::from(u16::try_from(folder.depth().min(4)).unwrap_or(0) * spacing.space_s);

            column = column.push(Self::row(
                row_icon(folder),
                crate::ui::folder_name(folder),
                self.unread.get(&folder.wire_name).copied().unwrap_or(0),
                indent,
                showing_a_folder && self.selected_folder == Some(index),
                Message::FolderSelected(index),
            ));
        }

        column.into()
    }
}
