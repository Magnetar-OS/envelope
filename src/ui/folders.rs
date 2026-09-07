// SPDX-License-Identifier: GPL-3.0-only

//! The folder dialogs: create, rename, delete, and the move picker.
//!
//! Small on purpose. Each is a `widget::dialog` whose primary action says
//! exactly what will happen — "Create", "Rename", "Delete" — because a
//! confirm button labelled OK is a question the user answers blind.

use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Apply as _, Element};
use cosmic_pim_mail::folder::Folder;

use crate::app::Message;
use crate::fl;

/// A name prompt shared by create and rename; `title` and `confirm` carry the
/// difference.
#[must_use]
pub fn name_dialog<'a>(title: String, confirm: String, name: &'a str) -> Element<'a, Message> {
    widget::dialog()
        .title(title)
        .control(
            widget::text_input(fl!("folder-name-placeholder"), name)
                .id(super::FOLDER_NAME_ID.clone())
                .on_input(Message::FolderNameChanged)
                .on_submit(|_| Message::FolderDialogConfirmed)
                .on_focus(Message::TextFocused)
                .on_unfocus(Message::TextUnfocused),
        )
        .primary_action(widget::button::suggested(confirm).on_press(Message::FolderDialogConfirmed))
        .secondary_action(
            widget::button::standard(fl!("cancel")).on_press(Message::FolderDialogCancelled),
        )
        .into()
}

/// The delete confirmation. The body says the part that matters: the messages
/// go with the folder.
#[must_use]
pub fn delete_dialog<'a>(folder: &Folder) -> Element<'a, Message> {
    widget::dialog()
        .title(fl!(
            "delete-folder-title",
            name = folder.display_name.clone()
        ))
        .body(fl!("delete-folder-warning"))
        .primary_action(
            widget::button::destructive(fl!("delete")).on_press(Message::FolderDialogConfirmed),
        )
        .secondary_action(
            widget::button::standard(fl!("cancel")).on_press(Message::FolderDialogCancelled),
        )
        .into()
}

/// The snooze presets: three moments people actually mean, not a calendar
/// widget. Each button says when; the exact timestamp is computed at press
/// time so "later today" cannot drift while the dialog sits open.
#[must_use]
pub fn snooze_dialog<'a>() -> Element<'a, Message> {
    use crate::app::SnoozePreset;
    let spacing = cosmic::theme::spacing();
    let preset = |label: String, preset: SnoozePreset| {
        widget::button::standard(label)
            .width(Length::Fill)
            .on_press(Message::SnoozePicked(preset))
    };
    widget::dialog()
        .title(fl!("snooze-title"))
        .control(
            widget::column::with_capacity(3)
                .spacing(spacing.space_xxs)
                .push(preset(fl!("snooze-later-today"), SnoozePreset::LaterToday))
                .push(preset(fl!("snooze-tomorrow"), SnoozePreset::Tomorrow))
                .push(preset(fl!("snooze-next-week"), SnoozePreset::NextWeek)),
        )
        .secondary_action(
            widget::button::standard(fl!("cancel")).on_press(Message::FolderDialogCancelled),
        )
        .into()
}

/// Send later: the snooze presets with a different verb — the same three
/// moments people mean.
#[must_use]
pub fn send_later_dialog<'a>() -> Element<'a, Message> {
    use crate::app::SnoozePreset;
    let spacing = cosmic::theme::spacing();
    let preset = |label: String, preset: SnoozePreset| {
        widget::button::standard(label)
            .width(Length::Fill)
            .on_press(Message::SendLaterPicked(preset))
    };
    widget::dialog()
        .title(fl!("send-later-title"))
        .control(
            widget::column::with_capacity(3)
                .spacing(spacing.space_xxs)
                .push(preset(fl!("snooze-later-today"), SnoozePreset::LaterToday))
                .push(preset(fl!("snooze-tomorrow"), SnoozePreset::Tomorrow))
                .push(preset(fl!("snooze-next-week"), SnoozePreset::NextWeek)),
        )
        .secondary_action(
            widget::button::standard(fl!("cancel")).on_press(Message::FolderDialogCancelled),
        )
        .into()
}

/// The move picker: a query box over the folders that can receive a message,
/// in the palette's shape because it is the same interaction.
pub struct MovePicker<'a> {
    pub query: &'a str,
    /// Indices into the app's folder list, already filtered to the query.
    pub matches: &'a [usize],
    pub folders: &'a [Folder],
    pub selected: usize,
}

impl<'a> MovePicker<'a> {
    #[must_use]
    pub fn view(self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let mut column = widget::column::with_capacity(self.matches.len() + 2)
            .spacing(spacing.space_xxs)
            .push(
                widget::text_input(fl!("move-placeholder"), self.query)
                    .id(super::FOLDER_NAME_ID.clone())
                    .on_input(Message::MoveQueryChanged)
                    .on_submit(|_| Message::FolderDialogConfirmed)
                    .on_focus(Message::TextFocused)
                    .on_unfocus(Message::TextUnfocused),
            );

        if self.matches.is_empty() {
            column = column.push(widget::text::caption(fl!("move-nothing")));
        }

        for (row, folder_index) in self.matches.iter().enumerate() {
            let Some(folder) = self.folders.get(*folder_index) else {
                continue;
            };
            let indent = u16::try_from(folder.depth()).unwrap_or(0) * spacing.space_s;
            let line = widget::row::with_capacity(2)
                .align_y(Alignment::Center)
                .push(widget::Space::new().width(Length::Fixed(f32::from(indent))))
                .push(widget::text::body(folder.display_name.clone()).width(Length::Fill));

            column = column.push(
                widget::button::custom(line)
                    .width(Length::Fill)
                    .class(if row == self.selected {
                        cosmic::theme::Button::Suggested
                    } else {
                        cosmic::theme::Button::Text
                    })
                    .on_press(Message::MovePicked(*folder_index)),
            );
        }

        widget::dialog()
            .control(
                column
                    .padding(spacing.space_s)
                    .apply(widget::container)
                    .width(Length::Fixed(480.0)),
            )
            .into()
    }
}
