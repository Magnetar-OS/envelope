// SPDX-License-Identifier: GPL-3.0-only

//! The command palette: every action, reachable by typing its name.
//!
//! A thin view over the same registry the keyboard, the menu, and the cheat
//! sheet read — which is the entire reason it could be built at all: there was
//! already exactly one list of what the application can do, with labels and
//! shortcuts attached.

use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Apply as _, Element};

use crate::actions::Action;
use crate::app::Message;
use crate::fl;

/// How tall the row list may grow before it scrolls.
///
/// About eight rows: enough that the answer is usually on screen, few enough
/// that the box stays a palette rather than becoming a window.
const ROWS_HEIGHT: f32 = 320.0;

/// What the palette shows for the current query.
pub struct Palette<'a> {
    pub query: &'a str,
    /// The actions the query matches, in registry order.
    pub matches: &'a [(Action, String)],
    pub selected: usize,
}

impl<'a> Palette<'a> {
    /// Consumes the struct so the element borrows the underlying data's
    /// lifetime rather than the struct's own.
    #[must_use]
    pub fn view(self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        // Room on the right for the scrollbar, which otherwise sits on top of
        // the shortcut column and cuts "Ctrl+Z" in half.
        let mut rows = widget::column::with_capacity(self.matches.len().max(1))
            .spacing(spacing.space_xxs)
            .padding([0, spacing.space_s, 0, 0]);

        if self.matches.is_empty() {
            rows = rows.push(crate::ui::muted(fl!("palette-nothing")));
        }

        for (index, (action, shortcut)) in self.matches.iter().enumerate() {
            let row = widget::row::with_capacity(2)
                .align_y(Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(widget::text::body(action.label()).width(Length::Fill))
                .push(widget::text::caption(shortcut.clone()));

            rows = rows.push(
                widget::button::custom(row)
                    .width(Length::Fill)
                    .selected(index == self.selected)
                    .class(crate::ui::row_class())
                    .on_press(Message::PaletteInvoked(*action)),
            );
        }

        // The rows scroll and the box does not grow past [`ROWS_HEIGHT`]. With
        // a message open the registry offers thirty-odd actions, which as a
        // plain column is a dialog taller than the window it is drawn over;
        // and a palette is typed at rather than browsed, so the answer to "my
        // command is below the fold" is another letter, not a longer box.
        let column = widget::column::with_capacity(2)
            .spacing(spacing.space_xxs)
            .push(
                widget::text_input(fl!("palette-placeholder"), self.query)
                    .id(super::PALETTE_ID.clone())
                    .on_input(Message::PaletteQueryChanged)
                    // Enter runs the selected row. Through on_submit rather
                    // than the key handler, because the input has focus and
                    // owns the keystroke.
                    .on_submit(|_| Message::PaletteSubmitted),
            )
            .push(
                widget::scrollable(rows)
                    .height(Length::Shrink)
                    .apply(widget::container)
                    .max_height(ROWS_HEIGHT),
            );

        crate::ui::picker(column.width(Length::Fill).height(Length::Shrink))
    }
}
