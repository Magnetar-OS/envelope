// SPDX-License-Identifier: GPL-3.0-only

//! The Rules context page: the account's filters, and a form to add one.
//!
//! A thin editor over `cosmic_pim_mail::rules` — the file the form writes is
//! the same `.rules.toml` a person can edit by hand, and nothing here knows
//! how matching works.

use std::sync::LazyLock;

use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic_pim_mail::folder::Folder;
use cosmic_pim_mail::rules::{Field, Rule};

use crate::app::{Message, RuleForm};
use crate::fl;

/// The condition fields, in the order the dropdown shows them.
pub const FIELDS: [Field; 4] = [
    Field::Sender,
    Field::Recipients,
    Field::Subject,
    Field::List,
];

/// Dropdown labels have to outlive the view, so they are resolved once,
/// after `i18n::init` — the idiom the conventions notes prescribe.
static FIELD_LABELS: LazyLock<Vec<String>> = LazyLock::new(|| {
    vec![
        fl!("rule-field-sender"),
        fl!("rule-field-recipients"),
        fl!("rule-field-subject"),
        fl!("rule-field-list"),
    ]
});

pub struct Rules<'a> {
    pub rules: &'a [Rule],
    pub form: &'a RuleForm,
    /// Move destinations: index 0 is "no move", the rest align with
    /// `move_wires` in the app.
    pub move_labels: &'a [String],
    pub folders: &'a [Folder],
}

impl<'a> Rules<'a> {
    #[must_use]
    pub fn view(self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let mut listing = widget::settings::section().title(fl!("rules-active"));
        if self.rules.is_empty() {
            listing = listing.add(widget::text::caption(fl!("rules-none")));
        }
        for (index, rule) in self.rules.iter().enumerate() {
            let row = widget::row::with_capacity(3)
                .align_y(Alignment::Center)
                .spacing(spacing.space_xs)
                .push(
                    widget::toggler(rule.enabled)
                        .on_toggle(move |on| Message::RuleToggled(index, on)),
                )
                .push(
                    widget::column::with_capacity(2)
                        .push(widget::text::body(rule.name.clone()))
                        .push(widget::text::caption(summarise(rule, self.folders)))
                        .width(Length::Fill),
                )
                .push(
                    widget::button::icon(widget::icon::from_name("edit-delete-symbolic"))
                        .on_press(Message::RuleDeleted(index)),
                );
            listing = listing.add(row);
        }

        let form = widget::settings::section()
            .title(fl!("rules-add"))
            .add(
                widget::text_input(fl!("rule-name-placeholder"), &self.form.name)
                    .on_input(Message::RuleFormNameChanged)
                    .on_focus(Message::TextFocused)
                    .on_unfocus(Message::TextUnfocused),
            )
            .add(
                widget::row::with_capacity(2)
                    .align_y(Alignment::Center)
                    .spacing(spacing.space_xs)
                    .push(widget::dropdown(
                        &FIELD_LABELS[..],
                        Some(self.form.field),
                        Message::RuleFormFieldSelected,
                    ))
                    .push(
                        widget::text_input(fl!("rule-contains-placeholder"), &self.form.contains)
                            .on_input(Message::RuleFormContainsChanged)
                            .on_focus(Message::TextFocused)
                            .on_unfocus(Message::TextUnfocused)
                            .width(Length::Fill),
                    ),
            )
            .add(
                widget::settings::item::builder(fl!("rule-mark-read"))
                    .toggler(self.form.mark_read, Message::RuleFormMarkRead),
            )
            .add(
                widget::settings::item::builder(fl!("rule-star"))
                    .toggler(self.form.star, Message::RuleFormStar),
            )
            .add(
                widget::settings::item::builder(fl!("rule-delete"))
                    .description(fl!("rule-delete-hint"))
                    .toggler(self.form.delete, Message::RuleFormDelete),
            )
            .add(
                widget::settings::item::builder(fl!("rule-move")).control(widget::dropdown(
                    self.move_labels,
                    Some(self.form.move_to),
                    Message::RuleFormMoveSelected,
                )),
            )
            .add(widget::button::suggested(fl!("rule-add")).on_press_maybe(
                (!self.form.contains.trim().is_empty()).then_some(Message::RuleFormSubmitted),
            ));

        widget::column::with_capacity(2)
            .spacing(spacing.space_s)
            .push(listing)
            .push(form)
            .into()
    }
}

/// One line saying what a rule does, for its row.
fn summarise(rule: &Rule, folders: &[Folder]) -> String {
    let condition = rule
        .conditions
        .first()
        .map(|condition| {
            let field = match condition.field {
                Field::Sender => fl!("rule-field-sender"),
                Field::Recipients => fl!("rule-field-recipients"),
                Field::Subject => fl!("rule-field-subject"),
                Field::List => fl!("rule-field-list"),
            };
            fl!(
                "rule-summary-match",
                field = field,
                text = condition.contains.clone()
            )
        })
        .unwrap_or_default();

    let mut actions = Vec::new();
    if rule.actions.delete {
        actions.push(fl!("rule-delete"));
    } else if let Some(wire) = &rule.actions.move_to {
        let name = folders
            .iter()
            .find(|folder| &folder.wire_name == wire)
            .map_or_else(|| wire.clone(), |folder| folder.display_name.clone());
        actions.push(fl!("rule-summary-move", folder = name));
    }
    if rule.actions.mark_read {
        actions.push(fl!("rule-mark-read"));
    }
    if rule.actions.star {
        actions.push(fl!("rule-star"));
    }
    format!("{condition} → {}", actions.join(", "))
}
