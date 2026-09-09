// SPDX-License-Identifier: GPL-3.0-only

//! The views. Each one is a function of the model, with no state of its own.

pub mod accounts;
pub mod composer;
pub mod folders;
pub mod list;
pub mod palette;
pub mod reader;
pub mod rules;
pub mod settings;
pub mod shortcuts;
pub mod sidebar;

use chrono::{DateTime, Datelike as _, Local, Utc};

/// [`relative_date`] for an epoch-milliseconds timestamp, as the index stores
/// them. Zero means "this message had no parseable Date", which shows as
/// nothing rather than as 1970.
#[must_use]
pub fn relative_date_ms(ms: i64) -> String {
    if ms == 0 {
        return String::new();
    }
    relative_date(DateTime::from_timestamp_millis(ms))
}

/// A timestamp as a mail list shows one: a time for today, a weekday for this
/// week, a date otherwise.
///
/// A message list is scanned, not read. Every column that repeats the same
/// information — the year, on mail that is all from this year — is a column the
/// eye has to skip past to reach the part that differs.
#[must_use]
pub fn relative_date(date: Option<DateTime<Utc>>) -> String {
    let Some(date) = date else {
        return String::new();
    };
    let local = date.with_timezone(&Local);
    let now = Local::now();
    let days = (now.date_naive() - local.date_naive()).num_days();

    if days == 0 {
        local.format("%H:%M").to_string()
    } else if days == 1 {
        local.format("Yesterday").to_string()
    } else if (0..7).contains(&days) {
        local.format("%a").to_string()
    } else if local.year() == now.year() {
        local.format("%-d %b").to_string()
    } else {
        local.format("%-d %b %Y").to_string()
    }
}

/// The palette's input, so opening it can focus it.
pub static PALETTE_ID: std::sync::LazyLock<cosmic::widget::Id> =
    std::sync::LazyLock::new(|| cosmic::widget::Id::new("palette"));

/// How much of the foreground secondary text keeps.
///
/// libcosmic's own convention for a secondary line is a smaller size —
/// `text::caption` under a `text::body` — and nothing else; the theme has no
/// secondary *colour* to ask for. That works for a settings row and not for a
/// label beside a value or an empty state, where the text has to stay
/// readable at body size and still stand back from what it is describing.
/// Alpha on the theme's own foreground is the one dimming that cannot clash
/// with a palette: it is the same hue on the same ground in every theme the
/// user might pick.
const MUTED_ALPHA: f32 = 0.7;

/// The foreground, dimmed. See [`MUTED_ALPHA`].
#[must_use]
pub fn muted_color(theme: &cosmic::Theme) -> cosmic::iced::Color {
    let mut color = theme.cosmic().on_bg_color();
    color.alpha *= MUTED_ALPHA;
    color.into()
}

/// Text in the muted tone labels and secondary notes share.
#[must_use]
pub fn muted(text: String) -> cosmic::widget::Text<'static, cosmic::Theme> {
    cosmic::widget::text::body(text).class(cosmic::theme::Text::Custom(|theme| {
        cosmic::iced::widget::text::Style {
            color: Some(muted_color(theme)),
            ..Default::default()
        }
    }))
}

/// What a pane says when it has nothing to show.
///
/// Centred and muted, in every pane that has one. An empty state is not the
/// content — it is a note about the absence of it — and at full contrast in
/// the top-left corner it reads as the first row of a list that never
/// arrives. Centring also puts it where the eye already is when a pane turns
/// out to be empty.
#[must_use]
pub fn empty_state<M: 'static>(text: String) -> cosmic::Element<'static, M> {
    use cosmic::Apply as _;
    muted(text)
        .align_x(cosmic::iced::alignment::Horizontal::Center)
        .apply(cosmic::widget::container)
        .center(cosmic::iced::Length::Fill)
        .padding(cosmic::theme::spacing().space_m)
        .into()
}

/// How wide a field's label column is.
///
/// One number for every form in the application. Labels that do not share a
/// column read as several forms stacked rather than one, and the eye has to
/// find the start of each value instead of running down a single edge.
pub const LABEL_WIDTH: f32 = 72.0;

/// How wide a settings control is.
pub const CONTROL_WIDTH: f32 = 220.0;

/// How wide a dialog that holds a list is.
///
/// Wide enough for a subject or a folder path, narrow enough to stay a
/// dialog rather than becoming a second window.
pub const PICKER_WIDTH: f32 = 480.0;

/// The class a selectable row wears.
///
/// `ListItem` rather than a filled `Suggested` button, and the difference is
/// the whole look of the window. `Suggested` is the accent-filled call to
/// action — right on a Send button, wrong on the forty rows of a mailbox,
/// where it turns the selection into the loudest thing on screen and leaves
/// nothing for hover to say. `ListItem` takes its rest, hover and pressed
/// colours from the theme's own list component and its selected state from
/// the accent, so a row reacts to the pointer the way every other COSMIC list
/// does, and the accent still marks the selection without shouting.
///
/// The radius is the theme's, so rows round exactly as the surfaces around
/// them do.
#[must_use]
pub fn row_class() -> cosmic::theme::Button {
    cosmic::theme::Button::ListItem(cosmic::theme::active().cosmic().corner_radii.radius_s)
}

/// How wide a column handle's grab zone is.
///
/// The line itself stays a hairline; this is the area the pointer actually
/// has to hit. Seven pixels is about the smallest that does not turn grabbing
/// a column edge into a test of aim — every desktop toolkit lands between six
/// and eight, for that reason.
const HANDLE_WIDTH: f32 = 7.0;

/// A draggable column edge.
///
/// Drawn as the same hairline the fixed divider was, so making the window
/// adjustable does not add furniture to it. The affordance is the pointer:
/// over the grab zone it becomes a resize cursor, which is the whole
/// discoverability story and costs no pixels.
#[must_use]
pub fn column_handle<'a>(press: crate::app::Message) -> cosmic::Element<'a, crate::app::Message> {
    cosmic::widget::mouse_area(
        cosmic::widget::container(cosmic::widget::divider::vertical::default())
            .center_x(cosmic::iced::Length::Fixed(HANDLE_WIDTH))
            .height(cosmic::iced::Length::Fill),
    )
    .interaction(cosmic::iced::mouse::Interaction::ResizingHorizontally)
    .on_press(press)
    .into()
}

/// One label, as a small chip.
///
/// Hand-rolled: libcosmic has no chip widget (verified against the whole
/// ecosystem), and the idiomatic composition is a captioned container on a
/// soft accent ground.
#[must_use]
pub fn label_chip<'a, M: 'a>(name: String) -> cosmic::Element<'a, M> {
    let spacing = cosmic::theme::spacing();
    cosmic::widget::container(cosmic::widget::text::caption(name))
        .padding([0, spacing.space_xxs])
        .class(cosmic::theme::Container::custom(|theme| {
            let cosmic = theme.cosmic();
            let mut accent = cosmic.accent_color();
            accent.alpha = 0.16;
            cosmic::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(accent.into())),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_s.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .into()
}

/// The folder dialogs' input — the name field, and the move picker's query —
/// so opening either can focus it.
pub static FOLDER_NAME_ID: std::sync::LazyLock<cosmic::widget::Id> =
    std::sync::LazyLock::new(|| cosmic::widget::Id::new("folder-name"));

/// A byte count as a person reads one.
///
/// One rule for the whole application: the reader and the composer are showing
/// the same fact about the same file, and two formatters would eventually
/// disagree about it in front of the user.
#[must_use]
pub fn size(bytes: usize) -> String {
    const UNITS: [&str; 4] = ["B", "kB", "MB", "GB"];
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Text in the theme's destructive colour, for the things that need it.
#[must_use]
pub fn destructive<Message: 'static>(text: String) -> cosmic::Element<'static, Message> {
    use cosmic::widget;
    widget::text::body(text)
        .class(cosmic::theme::Text::Custom(|theme| {
            cosmic::iced::widget::text::Style {
                color: Some(theme.cosmic().destructive_color().into()),
                ..Default::default()
            }
        }))
        .wrapping(cosmic::iced::core::text::Wrapping::Word)
        .into()
}
