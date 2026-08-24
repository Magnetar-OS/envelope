// SPDX-License-Identifier: GPL-3.0-only

//! The keyboard cheat sheet.
//!
//! Generated from the registry rather than written out, so it cannot be wrong:
//! the rows here are the bindings the keyboard handler actually matches
//! against. A shortcut list that has drifted from the shortcuts is worse than
//! none, because somebody trusted it.

use cosmic::Element;
use cosmic::widget;

use crate::actions::{Group, bindings};
use crate::app::Message;
use crate::fl;

#[must_use]
pub fn view() -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let bindings = bindings();

    let mut column = widget::column::with_capacity(Group::ALL.len() + 1)
        .spacing(spacing.space_s)
        .push(
            widget::text::caption(fl!("shortcuts-hint"))
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );

    for group in Group::ALL {
        let mut section = widget::settings::section().title(group.label());
        let mut any = false;
        for binding in bindings.iter().filter(|b| b.action.group() == group) {
            let shortcut = binding.shortcut();
            // An action with no shortcut is real — it lives in the menu — but
            // it has no business in a list of shortcuts.
            if shortcut.is_empty() {
                continue;
            }
            any = true;
            section = section.add(
                widget::settings::item::builder(binding.action.label())
                    .control(widget::text::body(shortcut)),
            );
        }
        if any {
            column = column.push(section);
        }
    }

    column.into()
}
