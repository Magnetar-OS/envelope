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

        let mut column = widget::column::with_capacity(self.matches.len() + 2)
            .spacing(spacing.space_xxs)
            .push(
                widget::text_input(fl!("palette-placeholder"), self.query)
                    .id(super::PALETTE_ID.clone())
                    .on_input(Message::PaletteQueryChanged)
                    // Enter runs the selected row. Through on_submit rather
                    // than the key handler, because the input has focus and
                    // owns the keystroke.
                    .on_submit(|_| Message::PaletteSubmitted)
                    .on_focus(Message::TextFocused)
                    .on_unfocus(Message::TextUnfocused),
            );

        if self.matches.is_empty() {
            column = column.push(widget::text::caption(fl!("palette-nothing")));
        }

        for (index, (action, shortcut)) in self.matches.iter().enumerate() {
            let row = widget::row::with_capacity(2)
                .align_y(Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(widget::text::body(action.label()).width(Length::Fill))
                .push(widget::text::caption(shortcut.clone()));

            column = column.push(
                widget::button::custom(row)
                    .width(Length::Fill)
                    .class(if index == self.selected {
                        cosmic::theme::Button::Suggested
                    } else {
                        cosmic::theme::Button::Text
                    })
                    .on_press(Message::PaletteInvoked(*action)),
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
