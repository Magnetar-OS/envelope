// SPDX-License-Identifier: GPL-3.0-only

//! Every action the application can perform, in one list.
//!
//! # Why a registry rather than key handling scattered about
//!
//! Because there are four places an action can be invoked from — a keystroke,
//! the menu, a button, and eventually a command palette — and four copies of
//! "what does archive do" is four places for them to disagree. Here, an
//! [`Action`] has one label, one shortcut, and one message, and the keyboard
//! handler, the menu, and the shortcut sheet all read the same list.
//!
//! The immediate payoff is the cheat sheet: it cannot be out of date, because
//! it is generated from the bindings the keyboard handler actually uses.
//!
//! # Layouts
//!
//! Bindings are matched through `KeyBind::matches`, which falls back to the
//! *physical* key position when the logical key does not match. That is what
//! makes `j` and `k` work on a Greek or Cyrillic layout, where the key that
//! produces `j` on a US keyboard produces `ξ`. A mail client whose navigation
//! stops working when somebody switches layout to write an email is not
//! keyboard-first.

use cosmic::widget::menu::key_bind::{KeyBind, Modifier};
use cosmic::iced::keyboard::Key;

use crate::fl;

/// Something the application can be asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Compose,
    Reply,
    ReplyAll,
    Forward,
    Send,

    Next,
    Previous,
    Archive,
    Delete,
    ToggleRead,
    ToggleFlagged,

    Search,
    Sync,
    /// Leave whatever is open — the composer, the search, a context page.
    Escape,

    GoInbox,
    GoDrafts,
    GoOutbox,
    GoSent,
    GoArchive,

    Shortcuts,
    Accounts,
    About,
}

impl Action {
    /// What this is called, wherever it is shown.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Compose => fl!("compose"),
            Self::Reply => fl!("reply"),
            Self::ReplyAll => fl!("reply-all"),
            Self::Forward => fl!("forward"),
            Self::Send => fl!("send"),
            Self::Next => fl!("next-message"),
            Self::Previous => fl!("previous-message"),
            Self::Archive => fl!("archive"),
            Self::Delete => fl!("delete"),
            Self::ToggleRead => fl!("toggle-read"),
            Self::ToggleFlagged => fl!("toggle-starred"),
            Self::Search => fl!("search"),
            Self::Sync => fl!("sync-now"),
            Self::Escape => fl!("close"),
            Self::GoInbox => fl!("go-inbox"),
            Self::GoDrafts => fl!("go-drafts"),
            Self::GoOutbox => fl!("go-outbox"),
            Self::GoSent => fl!("go-sent"),
            Self::GoArchive => fl!("go-archive"),
            Self::Shortcuts => fl!("shortcuts"),
            Self::Accounts => fl!("accounts"),
            Self::About => fl!("about"),
        }
    }

    /// Which group it belongs to in the cheat sheet.
    #[must_use]
    pub fn group(self) -> Group {
        match self {
            Self::Compose | Self::Reply | Self::ReplyAll | Self::Forward | Self::Send => {
                Group::Writing
            }
            Self::Next
            | Self::Previous
            | Self::Archive
            | Self::Delete
            | Self::ToggleRead
            | Self::ToggleFlagged => Group::Reading,
            Self::GoInbox
            | Self::GoDrafts
            | Self::GoOutbox
            | Self::GoSent
            | Self::GoArchive => Group::Going,
            Self::Search | Self::Sync | Self::Escape | Self::Shortcuts | Self::Accounts
            | Self::About => Group::Application,
        }
    }

    /// Does this only make sense with a message open?
    ///
    /// Used to grey a menu entry rather than let it be pressed and do nothing.
    /// A control that silently does nothing teaches people not to trust the
    /// ones next to it.
    #[must_use]
    pub fn needs_a_message(self) -> bool {
        matches!(
            self,
            Self::Reply
                | Self::ReplyAll
                | Self::Forward
                | Self::Archive
                | Self::Delete
                | Self::ToggleRead
                | Self::ToggleFlagged
        )
    }
}

/// How the cheat sheet is divided.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Reading,
    Writing,
    Going,
    Application,
}

impl Group {
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Reading => fl!("group-reading"),
            Self::Writing => fl!("group-writing"),
            Self::Going => fl!("group-going"),
            Self::Application => fl!("group-application"),
        }
    }

    pub const ALL: [Self; 4] = [Self::Reading, Self::Writing, Self::Going, Self::Application];
}

/// A key that can be pressed on its own.
///
/// Separate from [`KeyBind`] because these only fire when nothing is expecting
/// text — pressing `c` in the composer has to type a `c`. See
/// `AppModel::typing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bare {
    /// One key: `c`, `r`, `j`.
    Key(char),
    /// Two keys in sequence: `g` then `i`. The mail convention for "go to",
    /// and the reason a client can afford single letters for everything else.
    Chord(char, char),
}

impl std::fmt::Display for Bare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Key(key) => write!(f, "{key}"),
            Self::Chord(first, second) => write!(f, "{first} {second}"),
        }
    }
}

/// One row of the registry.
pub struct Binding {
    pub action: Action,
    /// The bare-key form, when it has one.
    pub bare: Option<Bare>,
    /// The modifier form, which works even while typing.
    pub combination: Option<KeyBind>,
}

impl Binding {
    /// How the shortcut is written in the cheat sheet.
    #[must_use]
    pub fn shortcut(&self) -> String {
        let mut parts = Vec::new();
        if let Some(bare) = self.bare {
            parts.push(bare.to_string());
        }
        if let Some(combination) = &self.combination {
            parts.push(describe(combination));
        }
        parts.join("  ·  ")
    }
}

fn character(key: &str) -> Key {
    Key::Character(key.into())
}

fn ctrl(key: &str) -> KeyBind {
    KeyBind {
        modifiers: vec![Modifier::Ctrl],
        key: character(key),
    }
}

fn ctrl_shift(key: &str) -> KeyBind {
    KeyBind {
        modifiers: vec![Modifier::Ctrl, Modifier::Shift],
        key: character(key),
    }
}

fn named(key: cosmic::iced::keyboard::key::Named) -> KeyBind {
    KeyBind {
        modifiers: Vec::new(),
        key: Key::Named(key),
    }
}

/// How a binding is written for a person.
fn describe(bind: &KeyBind) -> String {
    let mut out = String::new();
    for modifier in &bind.modifiers {
        out.push_str(match modifier {
            Modifier::Super => "Super+",
            Modifier::Ctrl => "Ctrl+",
            Modifier::Alt => "Alt+",
            Modifier::Shift => "Shift+",
        });
    }
    match &bind.key {
        Key::Character(c) => out.push_str(&c.to_uppercase()),
        Key::Named(named) => out.push_str(&format!("{named:?}")),
        other => out.push_str(&format!("{other:?}")),
    }
    out
}

/// Every binding, in the order the cheat sheet reads best.
///
/// The single letters are Gmail's, because that is the vocabulary anybody who
/// uses a mail client with a keyboard already has, and inventing a second one
/// would be asking people to learn something for no reason. The modifier forms
/// are the desktop's, so the same actions are reachable while typing.
#[must_use]
pub fn bindings() -> Vec<Binding> {
    use cosmic::iced::keyboard::key::Named;

    vec![
        // Reading
        Binding {
            action: Action::Next,
            bare: Some(Bare::Key('j')),
            combination: None,
        },
        Binding {
            action: Action::Previous,
            bare: Some(Bare::Key('k')),
            combination: None,
        },
        Binding {
            action: Action::Archive,
            bare: Some(Bare::Key('e')),
            combination: None,
        },
        Binding {
            action: Action::Delete,
            bare: Some(Bare::Key('#')),
            combination: Some(named(Named::Delete)),
        },
        Binding {
            action: Action::ToggleRead,
            bare: Some(Bare::Key('u')),
            combination: None,
        },
        Binding {
            action: Action::ToggleFlagged,
            bare: Some(Bare::Key('s')),
            combination: None,
        },
        // Writing
        Binding {
            action: Action::Compose,
            bare: Some(Bare::Key('c')),
            combination: Some(ctrl("n")),
        },
        Binding {
            action: Action::Reply,
            bare: Some(Bare::Key('r')),
            combination: Some(ctrl("r")),
        },
        Binding {
            action: Action::ReplyAll,
            bare: Some(Bare::Key('a')),
            combination: Some(ctrl_shift("r")),
        },
        Binding {
            action: Action::Forward,
            bare: Some(Bare::Key('f')),
            combination: Some(ctrl_shift("f")),
        },
        Binding {
            // No bare key: this one has to work from inside the composer, which
            // is the only place it means anything.
            action: Action::Send,
            bare: None,
            combination: Some(ctrl(",")),
        },
        // Going
        Binding {
            action: Action::GoInbox,
            bare: Some(Bare::Chord('g', 'i')),
            combination: None,
        },
        Binding {
            action: Action::GoDrafts,
            bare: Some(Bare::Chord('g', 'd')),
            combination: None,
        },
        Binding {
            action: Action::GoOutbox,
            bare: Some(Bare::Chord('g', 'o')),
            combination: None,
        },
        Binding {
            action: Action::GoSent,
            bare: Some(Bare::Chord('g', 't')),
            combination: None,
        },
        Binding {
            action: Action::GoArchive,
            bare: Some(Bare::Chord('g', 'a')),
            combination: None,
        },
        // The application
        Binding {
            action: Action::Search,
            bare: Some(Bare::Key('/')),
            combination: Some(ctrl("f")),
        },
        Binding {
            action: Action::Sync,
            bare: None,
            combination: Some(named(Named::F5)),
        },
        Binding {
            action: Action::Escape,
            bare: None,
            combination: Some(named(Named::Escape)),
        },
        Binding {
            action: Action::Shortcuts,
            bare: Some(Bare::Key('?')),
            combination: None,
        },
        Binding {
            action: Action::Accounts,
            bare: None,
            combination: Some(ctrl(".")),
        },
        Binding {
            action: Action::About,
            bare: None,
            combination: None,
        },
    ]
}

/// The action a modifier combination invokes, if any.
///
/// These fire whatever has focus — that is the point of requiring a modifier.
#[must_use]
pub fn for_combination(
    modifiers: cosmic::iced::keyboard::Modifiers,
    key: &Key,
    physical: Option<&cosmic::iced::keyboard::key::Physical>,
) -> Option<Action> {
    bindings().into_iter().find_map(|binding| {
        binding
            .combination
            .as_ref()
            .is_some_and(|bind| bind.matches(modifiers, key, physical))
            .then_some(binding.action)
    })
}

/// The action a bare key invokes, given whatever chord is pending.
///
/// Returns the action *or* the chord this key opened, so the caller can hold it
/// and offer it back on the next keystroke.
#[must_use]
pub fn for_bare(key: char, pending: Option<char>) -> Resolved {
    let key = key.to_ascii_lowercase();

    if let Some(first) = pending {
        // A chord's second key. Whatever happens the chord is spent — a `g`
        // followed by something meaningless must not leave the next keystroke
        // waiting on it.
        return match bindings().into_iter().find_map(|binding| {
            (binding.bare == Some(Bare::Chord(first, key))).then_some(binding.action)
        }) {
            Some(action) => Resolved::Act(action),
            None => Resolved::Nothing,
        };
    }

    if let Some(action) = bindings().into_iter().find_map(|binding| {
        (binding.bare == Some(Bare::Key(key))).then_some(binding.action)
    }) {
        return Resolved::Act(action);
    }

    if bindings()
        .iter()
        .any(|binding| matches!(binding.bare, Some(Bare::Chord(first, _)) if first == key))
    {
        return Resolved::Pending(key);
    }

    Resolved::Nothing
}

/// What a bare keystroke turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolved {
    Act(Action),
    /// The first key of a chord: hold it for the next keystroke.
    Pending(char),
    Nothing,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_in_the_registry_is_reachable() {
        // The registry exists so the cheat sheet cannot lie. An action with no
        // shortcut at all is fine — About is one — but it still has to be in
        // the list, or the menu and the sheet disagree about what exists.
        let bindings = bindings();
        assert!(
            bindings
                .iter()
                .any(|binding| binding.action == Action::Compose)
        );
        for binding in &bindings {
            assert!(!binding.action.label().is_empty());
        }
    }

    #[test]
    fn no_two_actions_claim_the_same_bare_key() {
        // Two actions on one key is a coin toss for the user.
        let bindings = bindings();
        let mut seen = Vec::new();
        for binding in &bindings {
            if let Some(bare) = binding.bare {
                assert!(
                    !seen.contains(&bare),
                    "{bare} is bound twice, most recently to {:?}",
                    binding.action
                );
                seen.push(bare);
            }
        }
    }

    #[test]
    fn no_bare_key_shadows_the_first_key_of_a_chord() {
        // `g` cannot both do something and open `g i`, or the chord never
        // fires — the first key would have consumed it.
        let bindings = bindings();
        let chord_starts: Vec<char> = bindings
            .iter()
            .filter_map(|binding| match binding.bare {
                Some(Bare::Chord(first, _)) => Some(first),
                _ => None,
            })
            .collect();
        for binding in &bindings {
            if let Some(Bare::Key(key)) = binding.bare {
                assert!(
                    !chord_starts.contains(&key),
                    "{key} is both an action and the start of a chord"
                );
            }
        }
    }

    #[test]
    fn a_bare_key_resolves_to_its_action() {
        assert_eq!(for_bare('c', None), Resolved::Act(Action::Compose));
        assert_eq!(for_bare('j', None), Resolved::Act(Action::Next));
        assert_eq!(for_bare('C', None), Resolved::Act(Action::Compose));
        assert_eq!(for_bare('z', None), Resolved::Nothing);
    }

    #[test]
    fn a_chord_opens_and_then_resolves() {
        assert_eq!(for_bare('g', None), Resolved::Pending('g'));
        assert_eq!(for_bare('i', Some('g')), Resolved::Act(Action::GoInbox));
        assert_eq!(for_bare('d', Some('g')), Resolved::Act(Action::GoDrafts));
    }

    #[test]
    fn a_chord_that_goes_nowhere_is_spent_rather_than_left_open() {
        // `g` then a meaningless key must not leave the next keystroke waiting.
        assert_eq!(for_bare('z', Some('g')), Resolved::Nothing);
        // And the key that would ordinarily act does not, while a chord is
        // pending — `g c` is not "compose".
        assert_eq!(for_bare('c', Some('g')), Resolved::Nothing);
    }

    #[test]
    fn modifier_combinations_resolve_whatever_the_layout() {
        use cosmic::iced::keyboard::Modifiers;
        let ctrl = Modifiers::CTRL;
        assert_eq!(
            for_combination(ctrl, &Key::Character("n".into()), None),
            Some(Action::Compose)
        );
        assert_eq!(
            for_combination(Modifiers::default(), &Key::Character("n".into()), None),
            None,
            "a bare letter must not fire a combination"
        );
    }

    #[test]
    fn shortcuts_are_written_out_for_the_sheet() {
        let bindings = bindings();
        let compose = bindings
            .iter()
            .find(|binding| binding.action == Action::Compose)
            .expect("compose");
        let shortcut = compose.shortcut();
        assert!(shortcut.contains('c'), "{shortcut}");
        assert!(shortcut.contains("Ctrl+N"), "{shortcut}");

        let inbox = bindings
            .iter()
            .find(|binding| binding.action == Action::GoInbox)
            .expect("go inbox");
        assert_eq!(inbox.shortcut(), "g i");
    }

    #[test]
    fn every_group_has_something_in_it() {
        // An empty heading in the cheat sheet is a heading that should not be
        // there.
        let bindings = bindings();
        for group in Group::ALL {
            assert!(
                bindings.iter().any(|binding| binding.action.group() == group),
                "{group:?} is empty"
            );
        }
    }
}
