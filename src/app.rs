// SPDX-License-Identifier: GPL-3.0-only

//! The Envelope application shell.
//!
//! # Where the work happens
//!
//! Not here. Every rule about mail — what a message is, what UIDVALIDITY means,
//! which order a push and a pull go in — lives in `cosmic-pim-mail`, and every
//! blocking call into it runs on a worker thread. This module is the state
//! machine in between: which account, which folder, which conversation, and
//! what to say while something is in flight.
//!
//! That split is the suite's shape, not a preference. Slate and Circle are thin
//! over the same substrate for the same reason: a sync bug is worth fixing
//! once.

use std::collections::HashMap;

use cosmic::app::{Core, Task, context_drawer};
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::Length;
use cosmic::iced::Subscription;
use cosmic::iced::window;
use cosmic::widget::{self, about::About, menu};
use cosmic::{Application as _, ApplicationExt as _};
use cosmic::{Apply as _, Element};
use cosmic_pim_accounts::{Account, AccountStore, MailEndpoint, MailProtocol, Transport};
use cosmic_pim_mail::folder::{Folder, SpecialUse};
use cosmic_pim_mail::model::Flags;

use crate::actions::{self, Action, Resolved};
use crate::config::Config;
use crate::fl;
use crate::mail::{self, Connection, Conversation, Opened, SyncReport};

const APP_ID: &str = "com.magnetaros.Envelope";
/// Read from the manifest rather than repeated here, so the About page cannot
/// name a repository the package does not come from.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const APP_ICON: &[u8] =
    include_bytes!("../resources/icons/hicolor/scalable/apps/com.magnetaros.Envelope.svg");

pub struct AppModel {
    core: Core,
    about: About,
    context_page: ContextPage,
    key_binds: HashMap<menu::KeyBind, MenuAction>,

    /// The suite's account store, re-read whenever it is written.
    accounts: Vec<Account>,
    selected_account: Option<String>,
    /// Built from the selected account; `None` when it has no mail endpoint.
    connection: Option<Connection>,

    folders: Vec<Folder>,
    selected_folder: Option<usize>,
    unread: HashMap<String, usize>,

    conversations: Vec<Conversation>,
    selected_conversation: Option<usize>,
    opened: Option<Opened>,

    loading_conversations: bool,
    syncing: bool,
    /// How many sync passes this session has run, so the expensive full
    /// reconciliation can happen on some of them and not all.
    cycle: u64,
    status: Option<String>,
    /// Transient confirmations, shown over whatever is on screen.
    toasts: widget::Toasts<Message>,
    /// What was said this turn, drained into toasts by `update`'s wrapper.
    pending_toasts: Vec<String>,
    list_error: Option<String>,
    reader_error: Option<String>,

    mail_form: Option<MailForm>,
    /// Every window but the main one: messages being written, and messages
    /// being read away from the list.
    ///
    /// A mail client writes in windows. The reading pane is where a mailbox is
    /// triaged, and it is the wrong place to answer from — a reply that
    /// replaces the thread it answers hides the thing being answered, and
    /// two replies at once were not expressible at all.
    windows: HashMap<window::Id, Detached>,
    /// The window whose message is being dispatched, for as long as it is.
    ///
    /// Set by [`AppModel::update`] around the dispatch of anything a detached
    /// window said, and by nothing else. It is what lets one `ComposeSend` arm
    /// serve every composer: the arm asks for *the* composer, and this decides
    /// which one that is. `None` means the main window.
    routed: Option<window::Id>,
    /// Which detached windows are maximized.
    ///
    /// Tracked here because `Core` tracks it for the main window only — every
    /// window-state update libcosmic handles is guarded on the main window id
    /// — and a header bar this application draws itself has to know whether to
    /// square its corners.
    maximized: std::collections::HashSet<window::Id>,
    /// The providers a browser sign-in can reach, loaded once — the registry
    /// is files on disk and does not change under a running app.
    sign_in_providers: Vec<crate::mail::SignInProvider>,
    /// The add-account form, while it is open.
    add_form: Option<AddForm>,
    /// A sign-in is in the browser. One at a time: the flow binds a fixed
    /// loopback port, so a second would fail on the bind and confuse the
    /// first.
    signing_in: bool,
    /// The command palette, while it is open: the query, and which row is
    /// selected.
    palette: Option<(String, usize)>,
    /// The folder dialog, while one is open. At most one of this and the
    /// palette: opening either closes the other.
    folder_dialog: Option<FolderDialog>,
    /// The folders the move picker is showing, as indices into `folders`.
    /// Held in the model because `dialog()` hands out borrows.
    move_rows: Vec<usize>,
    /// The column edge being dragged, if one is.
    dragging: Option<Drag>,
    /// Live column widths. Seeded from the configuration and written back to
    /// it when a drag ends, so the file is touched once per adjustment rather
    /// than once per frame of one.
    sidebar_width: f32,
    list_width: f32,
    /// Each conversation's label chips, parallel to `conversations`.
    conversation_labels: Vec<Vec<String>>,
    /// Every label the open folder knows, for the picker.
    known_labels: Vec<String>,
    /// What the label picker shows: `(name, applied to the selection)`.
    /// Held in the model because `dialog()` hands out borrows.
    label_rows: Vec<(String, bool)>,
    /// The composer's From choices, as dropdown labels. Rebuilt with the
    /// connection; held in the model because dropdown labels must outlive
    /// the view.
    identity_labels: Vec<String>,
    /// The account's filter rules, as loaded for the Rules page.
    rules: Vec<cosmic_pim_mail::rules::Rule>,
    rule_form: RuleForm,
    /// The move dropdown's rows: labels for the view, wire names to store.
    /// Index 0 is "no move". Held in the model because dropdown labels must
    /// outlive the view.
    rule_move_labels: Vec<String>,
    rule_move_wires: Vec<String>,
    /// What can be taken back, newest last. Capped, because each move entry
    /// holds its messages' bytes.
    undo_stack: Vec<UndoEntry>,
    /// The rows the palette is showing, recomputed when the query changes.
    /// Held in the model because `dialog()` hands out borrows, and a view
    /// cannot borrow from a value it computed itself.
    palette_rows: Vec<(Action, String)>,
    /// Local drafts for the selected account, newest first.
    drafts: Vec<cosmic_pim_mail::drafts::Saved>,
    showing_drafts: bool,
    outbox: Vec<cosmic_pim_mail::outbox::Queued>,
    showing_outbox: bool,
    /// The merged view of every account's inbox.
    unified: Vec<mail::UnifiedConversation>,
    showing_unified: bool,

    /// How many text inputs currently have focus.
    ///
    /// A counter rather than a flag: focus moves *between* inputs, and the
    /// unfocus of the one being left can arrive after the focus of the one
    /// being entered. A flag would flicker to false and let a keystroke meant
    /// for a text field fire a shortcut.
    search_focused: bool,
    /// The first key of a chord, waiting for its second.
    pending_chord: Option<char>,
    /// Settings that persist between runs, and are picked up live when another
    /// process changes them.
    config: Config,
    /// The interval field's text, which is not the setting: a half-typed number
    /// must not be rejected on every keystroke.
    poll_seconds: String,
    /// The undo-send grace field's text, for the same reason.
    send_delay: String,
    /// Which generation of inbox watch is current.
    ///
    /// A watch is a blocking thread parked in IDLE for minutes; it cannot be
    /// cancelled, only outlived. Switching accounts bumps this, the stale
    /// thread's eventual result carries the old number and is dropped, and the
    /// thread itself dies at its next timeout.
    watch_generation: u64,
    /// Whether a watch task is currently parked, so exactly one exists.
    watching: bool,
    /// The session bus connection libcosmic owns for single-instance, once
    /// handed over. What the iMIP hand-off calls Slate on; `None` before the
    /// runtime connects, in which case the hand-off degrades to the save
    /// affordance like any other absence.
    dbus: Option<zbus::Connection>,

    /// What is in the search box. Empty means the box is closed.
    search: String,
    results: Vec<cosmic_pim_mail::Hit>,
    searching: bool,

    /// Which of the open message's quoted runs the reader has been asked to
    /// show, by their index among its blocks.
    ///
    /// Cleared whenever a message is opened: "show this quote" is a decision
    /// about the message being read, and carrying it to the next one would
    /// unfold a run of history nobody asked to see.
    expanded_quotes: std::collections::HashSet<usize>,
}

/// How many search results are shown.
///
/// A cap rather than paging: somebody who gets 500 hits needs a better query,
/// not a second page, and the honest answer to "there are more" is to say so.
const SEARCH_LIMIT: usize = 200;

/// The search box, so a keystroke can put the cursor in it.
static SEARCH_ID: std::sync::LazyLock<cosmic::widget::Id> =
    std::sync::LazyLock::new(|| cosmic::widget::Id::new("search"));

/// What a window other than the main one is showing.
///
/// One enum rather than two maps because the answer to "what is in this
/// window" has to be exhaustive: a window whose contents cannot be named is a
/// window that cannot be drawn, and `view_window` is called for every id the
/// runtime knows about.
pub enum Detached {
    /// A message being written. Boxed, like the message being read: a composer
    /// carries an editor's worth of state, and every window map entry would
    /// otherwise be sized for it.
    Compose(Box<Composer>),
    /// A message being read away from the list.
    Read(Box<Reading>),
}

/// A message open in a window of its own.
///
/// Carries its own folder and conversation rather than reading the main
/// window's selection, and that is the whole point of the type: the list moves
/// on. Archiving from a window opened an hour ago has to file the exchange
/// that window is showing, not whatever happens to be selected now.
pub struct Reading {
    pub folder: Folder,
    /// The whole conversation's uids, so filing from here files the exchange
    /// — the same rule the reading pane follows.
    pub uids: Vec<u32>,
    pub opened: Opened,
    /// Which quoted runs this window has been asked to unfold. Per window,
    /// because it is a decision about the message on screen.
    pub expanded_quotes: std::collections::HashSet<usize>,
}

/// What a reader action is acting on.
///
/// Borrowed rather than cloned, and assembled per action rather than stored:
/// the answer to "which message" depends on which window asked, and a stored
/// answer would be the stale one exactly when the two disagree.
struct Target<'a> {
    folder: &'a Folder,
    /// The whole conversation, for the operations that file the exchange.
    uids: &'a [u32],
    /// The one message a flag, a save or an export is about.
    uid: u32,
    /// The message itself, when it is actually open. A conversation can be
    /// selected in the list without one.
    opened: Option<&'a Opened>,
}

/// What a composer's window is called.
///
/// The subject, because that is what the user is writing and what they will
/// look for in a window list. A message with no subject yet is named for what
/// it is, not called "(no subject)" — nobody has failed to write one, they
/// have not written one *yet*.
fn compose_title(subject: &str) -> String {
    let subject = subject.trim();
    if subject.is_empty() {
        fl!("compose")
    } else {
        subject.to_owned()
    }
}

/// What a detached message's window is called.
///
/// Here an empty subject is a fact about the message, and reads as the list
/// reads it.
fn read_title(subject: &str) -> String {
    let subject = subject.trim();
    if subject.is_empty() {
        fl!("no-subject")
    } else {
        subject.to_owned()
    }
}

/// A message being written.
///
/// The addresses are held as **text**, not as parsed mailboxes, and that is on
/// purpose: a half-typed address is not a mailbox, and a composer that reparses
/// on every keystroke either rejects what the user is in the middle of typing or
/// silently drops it. Parsing happens once, on send, where a failure can be
/// explained.
pub struct Composer {
    pub draft: cosmic_pim_mail::Draft,
    /// The body being typed.
    ///
    /// Editor content rather than the draft's `String`, because a message is
    /// prose: it has paragraphs, and it is edited around rather than only
    /// appended to. The draft keeps the string — it is what gets serialised,
    /// mirrored and sent — and [`Composer::resolved`] is where the two meet.
    pub body: widget::text_editor::Content,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    /// The message being answered, so it can be marked as answered once the
    /// reply is actually away.
    pub answering: Option<(Folder, u32)>,
    /// Set once this has been saved, so re-saving replaces rather than
    /// accumulating a file per edit.
    pub draft_id: Option<String>,
    pub sending: bool,
    pub error: Option<String>,
    /// Undo and redo for the body.
    ///
    /// The composer's, not the application's: `text_editor` remembers nothing,
    /// and before this Ctrl+Z in a half-written message reached past the
    /// composer and undid the last *mail* operation. See [`crate::text`].
    pub history: crate::text::History,
}

impl Composer {
    fn new(draft: cosmic_pim_mail::Draft, answering: Option<(Folder, u32)>) -> Self {
        Self {
            to: join(&draft.to),
            cc: join(&draft.cc),
            bcc: join(&draft.bcc),
            // Seeded from the draft, which is how a reply opens with the
            // quoted text it was built with — several lines of it.
            body: widget::text_editor::Content::with_text(&draft.body),
            // The floor undo stops at is the body as it opened — for a reply,
            // the quoted message. Undoing past that would empty a composer
            // the user never emptied.
            history: crate::text::History::new(draft.body.clone()),
            draft,
            answering,
            draft_id: None,
            sending: false,
            error: None,
        }
    }

    /// Is there anything here worth keeping?
    ///
    /// An empty composer opened and closed again must not leave a file behind
    /// — a Drafts list full of blanks is how the feature stops being useful.
    fn is_worth_saving(&self) -> bool {
        !self.draft.subject.trim().is_empty()
            || !self.body.text().trim().is_empty()
            || !self.to.trim().is_empty()
            || !self.cc.trim().is_empty()
            || !self.bcc.trim().is_empty()
    }

    /// Performs one editor action on the body, and remembers what it did.
    ///
    /// Two things happen here that the widget does not do on its own:
    ///
    /// - **Enter continues what the line was.** A `> ` quote, a `- ` bullet, a
    ///   numbered item. The quote is the case that matters: a reply written
    ///   inside quoted text that silently stops being quoted halfway down
    ///   attributes the rest of the paragraph to the person being quoted.
    /// - **Edits are recorded**, coalesced into steps a person would recognise
    ///   as one action. Cursor movement records nothing but still updates where
    ///   undo will return to.
    fn edit(&mut self, action: widget::text_editor::Action) {
        use widget::text_editor::{Action, Edit};

        let before = self.body.cursor().position;
        let before = (before.line, before.column);
        let kind = crate::text::EditKind::of(&action);

        match (&action, self.continuation()) {
            // An empty continued line: Enter clears it rather than making a
            // second empty one. That is how a list or a quote is left.
            (Action::Edit(Edit::Enter), Some(prefix)) if prefix.is_empty() => {
                self.body
                    .perform(Action::Move(widget::text_editor::Motion::Home));
                self.body
                    .perform(Action::Select(widget::text_editor::Motion::End));
                self.body.perform(Action::Edit(Edit::Delete));
            }
            (Action::Edit(Edit::Enter), Some(prefix)) => {
                self.body.perform(Action::Edit(Edit::Enter));
                self.body
                    .perform(Action::Edit(Edit::Paste(std::sync::Arc::new(prefix))));
            }
            _ => self.body.perform(action),
        }

        let after = self.body.cursor().position;
        let after = (after.line, after.column);
        match kind {
            Some(kind) => self.history.record(self.body.text(), before, after, kind),
            // Not an edit — but the cursor may have moved, and undo should
            // come back to where the user actually is.
            None => self.history.moved(after),
        }
    }

    /// What Enter on the current line should open the next one with.
    fn continuation(&self) -> Option<String> {
        let line = self.body.cursor().position.line;
        crate::text::continuation(&self.body.line(line)?.text)
    }

    /// Steps the body back one edit. Returns false when there is nothing to
    /// step back to, so the caller can fall through to whatever else Ctrl+Z
    /// might have meant.
    fn undo(&mut self) -> bool {
        let Some((text, caret)) = self.history.undo() else {
            return false;
        };
        let text = text.to_owned();
        crate::text::replace(&mut self.body, &text);
        crate::text::restore(&mut self.body, caret);
        true
    }

    fn redo(&mut self) -> bool {
        let Some((text, caret)) = self.history.redo() else {
            return false;
        };
        let text = text.to_owned();
        crate::text::replace(&mut self.body, &text);
        crate::text::restore(&mut self.body, caret);
        true
    }

    /// The draft as it should go out: [`Self::resolved`] with the body
    /// wrapped.
    ///
    /// Separate from `resolved` because a *draft* keeps what was typed. Wrapping
    /// on every save would reformat the user's paragraphs under them the next
    /// time they opened it; wrapping once, on the way out, is the only point at
    /// which the line lengths are anyone else's problem.
    fn outgoing(&self) -> cosmic_pim_mail::Draft {
        let mut draft = self.resolved();
        draft.body = crate::text::wrap(&draft.body, crate::text::WRAP_COLUMNS);
        draft
    }

    /// The draft with the address fields as currently typed.
    fn resolved(&self) -> cosmic_pim_mail::Draft {
        let mut draft = self.draft.clone();
        draft.to = parse_addresses(&self.to);
        draft.cc = parse_addresses(&self.cc);
        draft.bcc = parse_addresses(&self.bcc);
        draft.body = self.body.text();
        draft
    }

    /// Why this cannot be sent yet, if it cannot.
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        self.resolved().problem()
    }
}

fn join(mailboxes: &[cosmic_pim_mail::Mailbox]) -> String {
    mailboxes
        .iter()
        .map(|m| match &m.name {
            Some(name) => format!("{name} <{}>", m.address),
            None => m.address.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Splits a comma-separated address field, accepting both `a@b` and
/// `Name <a@b>`.
///
/// Commas inside a quoted display name would break this, and are left broken:
/// the alternative is an RFC 5322 address-list parser in the UI layer, and a
/// user whose contact has a comma in their name can drop the quotes. The
/// substrate validates what comes out.
fn parse_addresses(field: &str) -> Vec<cosmic_pim_mail::Mailbox> {
    field
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| match entry.rsplit_once('<') {
            Some((name, rest)) => cosmic_pim_mail::Mailbox {
                name: Some(name.trim().trim_matches('"').to_owned()).filter(|n| !n.is_empty()),
                address: rest.trim_end_matches('>').trim().to_ascii_lowercase(),
            },
            None => cosmic_pim_mail::Mailbox {
                name: None,
                address: entry.to_ascii_lowercase(),
            },
        })
        .collect()
}

/// The mail-endpoint form, while it is open.
pub struct MailForm {
    pub account_id: String,
    /// Shown as the username field's placeholder: leaving it empty means "the
    /// same login the account already has", which is right far more often than
    /// not.
    pub account_username: String,
    pub host: String,
    pub port: String,
    pub transport: Transport,
    pub username: String,
    /// Empty means "the same host as IMAP", which is right for nearly every
    /// provider.
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_transport: Transport,
    /// Empty means "the login, if it is an address".
    pub from_address: String,
    pub from_name: String,
    /// The additional addresses this account may send as.
    pub aliases: Vec<cosmic_pim_accounts::Alias>,
    /// The alias being typed, as `Name <address>` or a bare address.
    pub alias_input: String,
    /// Which protocol reads this account's mail.
    pub protocol: MailProtocol,
    /// The JMAP session resource. Meaningful only when the protocol is JMAP,
    /// and filled in by the provider registry far more often than by hand.
    pub jmap_url: String,
    /// The address discovery works from — usually the From address, which is
    /// the only thing the user reliably knows.
    pub discovering: bool,
    pub discovered_from: Option<String>,
    pub error: Option<String>,
}

impl MailForm {
    fn new(account: &Account) -> Self {
        let mail = account.mail.as_ref();
        Self {
            account_id: account.id.clone(),
            account_username: account.username.clone(),
            host: mail.map(|m| m.imap_host.clone()).unwrap_or_default(),
            port: mail.map_or_else(|| "993".to_string(), |m| m.imap_port.to_string()),
            transport: mail.map(|m| m.imap_transport).unwrap_or_default(),
            username: mail
                .and_then(|m| m.imap_username.clone())
                .unwrap_or_default(),
            smtp_host: mail.map(|m| m.smtp_host.clone()).unwrap_or_default(),
            smtp_port: mail.map_or_else(|| "465".to_string(), |m| m.smtp_port.to_string()),
            smtp_transport: mail.map(|m| m.smtp_transport).unwrap_or_default(),
            from_address: mail.map(|m| m.from_address.clone()).unwrap_or_default(),
            from_name: mail.map(|m| m.from_name.clone()).unwrap_or_default(),
            aliases: mail.map(|m| m.aliases.clone()).unwrap_or_default(),
            alias_input: String::new(),
            protocol: mail.map(|m| m.protocol).unwrap_or_default(),
            jmap_url: mail
                .and_then(|m| m.jmap_session_url.clone())
                .unwrap_or_default(),
            discovering: false,
            discovered_from: None,
            error: None,
        }
    }

    /// Fills the form in from a provider-registry entry.
    ///
    /// The registry knows more than discovery does — which protocol, and the
    /// JMAP session URL — so a hit here fills everything.
    fn apply_endpoint(&mut self, endpoint: &MailEndpoint) {
        self.protocol = endpoint.protocol;
        self.host = endpoint.imap_host.clone();
        self.port = endpoint.imap_port.to_string();
        self.transport = endpoint.imap_transport;
        self.smtp_host = endpoint.smtp_host.clone();
        self.smtp_port = endpoint.smtp_port.to_string();
        self.smtp_transport = endpoint.smtp_transport;
        self.jmap_url = endpoint.jmap_session_url.clone().unwrap_or_default();
        self.discovered_from = Some(fl!("found-provider"));
    }

    /// Fills the form in from what discovery found.
    ///
    /// The From address is left alone: discovery answers "where does this
    /// domain's mail live", and the user may well be sending as an alias.
    fn apply(&mut self, found: &cosmic_pim_mail::Discovered) {
        self.host = found.imap_host.clone();
        self.port = found.imap_port.to_string();
        self.transport = mail::transport_of(found.imap_security);
        self.smtp_host = found.smtp_host.clone();
        self.smtp_port = found.smtp_port.to_string();
        self.smtp_transport = mail::transport_of(found.smtp_security);
        self.username = found.username.clone();
        self.discovered_from = Some(match found.source {
            cosmic_pim_mail::discovery::Source::Known => fl!("found-known"),
            cosmic_pim_mail::discovery::Source::Autoconfig => fl!("found-autoconfig"),
            // Said differently on purpose: a guess that a socket accepted is
            // not the same confidence as a published setting.
            cosmic_pim_mail::discovery::Source::Guessed => fl!("found-guessed"),
        });
    }

    #[must_use]
    pub fn transport_index(&self) -> usize {
        transport_index(self.transport)
    }

    #[must_use]
    pub fn smtp_transport_index(&self) -> usize {
        transport_index(self.smtp_transport)
    }

    #[must_use]
    pub fn protocol_index(&self) -> usize {
        crate::ui::accounts::PROTOCOLS
            .iter()
            .position(|p| *p == self.protocol)
            .unwrap_or(0)
    }
}

/// The add-account form: who you are, and the password. The servers are
/// worked out from the address.
///
/// Envelope's own front door to the suite's account store. An account added
/// here is the same account Slate and Circle read; what this form has over
/// theirs is that the mail server is looked up rather than asked for.
#[derive(Debug, Clone, Default)]
pub struct AddForm {
    pub name: String,
    pub email: String,
    pub password: String,
    /// What the registry says about the address, refreshed as it is typed.
    pub provider: Option<mail::ProviderNote>,
    /// Discovery is on the network.
    pub adding: bool,
    pub error: Option<String>,
}

impl AddForm {
    /// The address belongs to a provider that signs in with the browser, and
    /// that route is open — so a password field would be asking for a thing
    /// the provider does not issue.
    #[must_use]
    pub fn wants_sign_in(&self) -> bool {
        self.provider
            .as_ref()
            .is_some_and(|p| p.uses_sign_in && p.sign_in_ready)
    }

    /// Whether Add can be pressed: a whole address and a password.
    #[must_use]
    pub fn can_add(&self) -> bool {
        !self.adding && self.email.trim().contains('@') && !self.password.is_empty()
    }
}

fn transport_index(transport: Transport) -> usize {
    crate::ui::accounts::TRANSPORTS
        .iter()
        .position(|t| *t == transport)
        .unwrap_or(0)
}

/// One reversible thing the user did.
#[derive(Clone, Debug)]
pub struct UndoEntry {
    /// What the status line says when it is undone.
    pub description: String,
    pub reverse: Reverse,
}

/// How an entry is taken back.
#[derive(Clone, Debug)]
pub enum Reverse {
    /// Restore these exact flags, through the same queued path that changed
    /// them.
    Flags {
        folder: Folder,
        previous: Vec<(u32, cosmic_pim_mail::model::Flags)>,
    },
    /// Cancel the queued move and put the held messages back.
    ///
    /// This one has a deadline the stack does not control: once the writeback
    /// queue drains, the move has happened and the reverse honestly fails.
    Unmove {
        folder: Folder,
        messages: Vec<cosmic_pim_mail::store::RemoteMessage>,
    },
    /// Take a scheduled send back out of the outbox and reopen it.
    ///
    /// The other reverse with a deadline: once the message goes, the honest
    /// answer is that it went.
    CancelSend { id: String },
}

/// How much history is kept.
///
/// Small, because a move entry holds its messages' bytes, and because an undo
/// stack is for the mistake just made — nobody unwinds forty steps of triage,
/// they re-triage.
const UNDO_DEPTH: usize = 10;

/// Which folder dialog is open, and its editable state.
///
/// Rename and delete act on the folder selected in the sidebar; the enum does
/// not carry one so the dialog cannot outlive a folder-list refresh and act
/// on a folder that moved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderDialog {
    Create {
        name: String,
    },
    Rename {
        name: String,
    },
    Delete,
    /// The snooze presets.
    Snooze,
    /// The label picker: its query, and which row is highlighted.
    Label {
        query: String,
        selected: usize,
    },
    /// The send-later presets — the same moments, a different verb.
    SendLater,
    /// Removing an account, which forgets its password.
    RemoveAccount {
        id: String,
        name: String,
    },
    /// Deleting a filter rule.
    DeleteRule {
        index: usize,
        name: String,
    },
    /// The move picker: its query, and which row is highlighted.
    Move {
        query: String,
        selected: usize,
    },
}

/// When snoozed mail comes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnoozePreset {
    /// Three hours from now.
    LaterToday,
    /// Tomorrow at 08:00, local time.
    Tomorrow,
    /// Next Monday at 08:00, local time.
    NextWeek,
}

impl SnoozePreset {
    /// The return time, computed at the moment of choice — never at view
    /// time, where "later today" would drift as the dialog sat open.
    #[must_use]
    pub fn until_ms(self) -> i64 {
        use chrono::{Datelike as _, Duration, Local, NaiveTime};
        let now = Local::now();
        let at_eight = |date: chrono::NaiveDate| {
            NaiveTime::from_hms_opt(8, 0, 0)
                .map(|time| date.and_time(time))
                .and_then(|naive| naive.and_local_timezone(Local).earliest())
                .map(|moment| moment.timestamp_millis())
        };
        match self {
            Self::LaterToday => (now + Duration::hours(3)).timestamp_millis(),
            Self::Tomorrow => at_eight(now.date_naive() + Duration::days(1))
                .unwrap_or_else(|| (now + Duration::hours(18)).timestamp_millis()),
            Self::NextWeek => {
                let ahead = 7 - i64::from(now.weekday().num_days_from_monday());
                let ahead = if ahead == 0 { 7 } else { ahead };
                at_eight(now.date_naive() + Duration::days(ahead))
                    .unwrap_or_else(|| (now + Duration::days(ahead)).timestamp_millis())
            }
        }
    }
}

/// Which column edge is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    /// Between the folder sidebar and the message list.
    Sidebar,
    /// Between the message list and the message.
    List,
}

/// A column drag in progress.
///
/// The origin width and the cursor's first position are both recorded so the
/// column tracks the pointer exactly, however much padding the shell puts
/// around the panes. Deriving the width from the raw cursor position instead
/// would make the edge jump to the pointer on the first pixel of movement.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub split: Split,
    origin: f32,
    from_x: Option<f32>,
}

#[derive(Clone, Debug)]
pub enum Message {
    LaunchUrl(String),
    ToggleContextPage(ContextPage),

    AccountSelected(String),
    SyncNow,
    /// The periodic check. Distinct from [`Self::SyncNow`] because it must be
    /// silent about doing nothing.
    Poll,
    /// Boxed, like the other loaded-a-lot-of-data variants below: an enum is
    /// sized to its largest variant, and a `LaunchUrl` should not carry a
    /// folder list's worth of padding around with it.
    SyncFinished(Box<Result<SyncReport, String>>),

    FolderSelected(usize),
    ConversationsLoaded(Result<(Vec<Conversation>, Vec<Vec<String>>), String>),
    ConversationSelected(usize),
    MessageOpened(Box<Result<Opened, String>>),

    /// Hand the open message's calendar part to Slate.
    OpenInCalendar,
    /// The hand-off came back — accepted, refused, or no calendar running.
    InvitationDelivered(Result<crate::scheduling::Delivered, String>),

    ToggleRead,
    ToggleFlagged,
    Archive,
    Delete,
    /// A local mutation finished; reload the folder either way, and remember
    /// how to take it back if it said how.
    Mutated(Result<Option<UndoEntry>, String>),
    /// An undo finished.
    Undone(Result<String, String>),

    MailFormStart(String),
    MailFormHostChanged(String),
    MailFormPortChanged(String),
    MailFormTransportChanged(Transport),
    MailFormProtocolChanged(MailProtocol),
    MailFormJmapUrlChanged(String),
    MailFormUsernameChanged(String),
    MailFormSmtpHostChanged(String),
    MailFormSmtpPortChanged(String),
    MailFormSmtpTransportChanged(Transport),
    MailFormFromAddressChanged(String),
    MailFormFromNameChanged(String),
    MailFormAliasInputChanged(String),
    MailFormAliasAdded,
    MailFormAliasRemoved(usize),
    MailFormCancel,
    MailFormSave,
    MailFormDiscover,
    AddFormStart,
    AddFormCancel,
    AddFormNameChanged(String),
    AddFormEmailChanged(String),
    AddFormPasswordChanged(String),
    AddFormConfirm,
    AddFormDiscovered(Box<Result<cosmic_pim_mail::Discovered, String>>),
    AccountRemove(String),
    SignInStarted(String),
    SignInFinished(Box<Result<String, String>>),
    PaletteQueryChanged(String),
    PaletteSubmitted,
    PaletteInvoked(Action),
    MailFormDiscovered(Box<Result<cosmic_pim_mail::Discovered, String>>),

    Compose,
    Reply {
        all: bool,
    },
    Forward,
    /// The composer's From dropdown.
    ComposeFromSelected(usize),
    ComposeToChanged(String),
    ComposeCcChanged(String),
    ComposeBccChanged(String),
    ComposeSubjectChanged(String),
    /// One edit in the body: a keystroke, a paste, a selection drag. The
    /// editor owns the text and reports what happened to it.
    ComposeBodyAction(Box<widget::text_editor::Action>),
    /// Close and keep what was typed.
    ComposeCancel,
    /// Close and throw it away — the explicit choice, not the default.
    ComposeDiscard,
    DraftsLoaded(Vec<cosmic_pim_mail::drafts::Saved>),
    ShowDrafts,
    ConfigChanged(Config),
    /// An inbox watch came back. The generation says whether it is still ours.
    WatchEnded {
        generation: u64,
        outcome: Result<crate::mail::WatchOutcome, String>,
    },
    PollSecondsChanged(String),
    MarkReadOnOpenChanged(bool),
    /// One of the registry's actions, however it was invoked.
    Act(Action),
    /// A toast expired or was dismissed.
    ToastClosed(widget::ToastId),
    /// A column edge was grabbed.
    SplitPressed(Split),
    /// The pointer moved while a column edge is held, in window coordinates.
    SplitMoved(f32),
    /// The column edge was let go; the width becomes the remembered one.
    SplitReleased,
    KeyPressed(
        cosmic::iced::keyboard::Modifiers,
        cosmic::iced::keyboard::Key,
        Option<cosmic::iced::keyboard::key::Physical>,
    ),
    /// Show or hide one of the open message's quoted runs.
    ToggleQuote(usize),
    SearchFocused,
    SearchUnfocused,
    ShowOutbox,
    ShowUnified,
    UnifiedLoaded(Vec<mail::UnifiedConversation>),
    /// Open one unified row: switch to its account, then its message.
    UnifiedOpened(usize),
    OutboxLoaded(Vec<cosmic_pim_mail::outbox::Queued>),
    QueuedRetried(String),
    QueuedDiscarded(String),
    SearchChanged(String),
    SearchFinished(Vec<cosmic_pim_mail::Hit>),
    SearchCleared,
    /// Open a search hit, which may be in a folder other than the current one.
    HitOpened(usize),

    SaveAttachment(usize),
    /// Import an `application/pgp-keys` attachment as the sender's key.
    PgpKeyImport(usize),
    PgpKeyImported(Box<Result<String, String>>),
    ExportMessage,
    ImportMbox,
    MboxPicked(Option<std::path::PathBuf>),
    MboxImported(Result<(usize, usize), String>),
    MessageExported(Result<std::path::PathBuf, String>),
    Unsubscribe,
    Unsubscribed(Result<(), String>),
    AttachmentSaved(Result<std::path::PathBuf, String>),
    /// Attach a file the user picked.
    AttachFile,
    FilePicked(Result<Vec<std::path::PathBuf>, String>),
    AttachmentRemoved(usize),
    /// A rule's enabled toggle.
    RuleToggled(usize, bool),
    RuleDeleted(usize),
    RuleFormNameChanged(String),
    RuleFormFieldSelected(usize),
    RuleFormContainsChanged(String),
    RuleFormMarkRead(bool),
    RuleFormStar(bool),
    RuleFormDelete(bool),
    RuleFormMoveSelected(usize),
    RuleFormSubmitted,
    /// A rules pass ran after a sync; `Ok(None)` means nothing to do.
    RulesApplied(Box<Result<Option<mail::RulesReport>, String>>),

    /// A snooze duration was chosen.
    SnoozePicked(SnoozePreset),
    /// The folder's label table arrived for the picker.
    LabelRowsLoaded(Box<Result<Vec<String>, String>>),
    LabelQueryChanged(String),
    /// Apply or clear one label on the selected conversation.
    LabelToggled(String, bool),
    /// The composer's Send later button.
    SendLater,
    /// A send-later moment was chosen.
    SendLaterPicked(SnoozePreset),
    /// A send was queued: `(queue id, when it goes, grace or scheduled)`.
    SendScheduled(Box<Result<(String, i64, bool), String>>),
    /// An undone send came back — or turned out to be gone.
    SendCancelled(Box<Result<Option<cosmic_pim_mail::Draft>, String>>),
    SendDelayChanged(String),
    /// The wake pass finished: how many snoozed messages returned.
    SnoozeWoken(Box<Result<usize, String>>),

    /// One of the folder dialogs was asked for.
    FolderDialogOpened(FolderDialog),
    /// The create/rename dialog's name field changed.
    FolderNameChanged(String),
    /// The move picker's query changed.
    MoveQueryChanged(String),
    /// A folder was picked in the move picker.
    MovePicked(usize),
    FolderDialogConfirmed,
    FolderDialogCancelled,
    /// A folder operation finished on the worker; the string is the status to
    /// show.
    FolderOpFinished(Box<Result<String, String>>),

    DraftOpened(String),
    DraftDeleted(String),
    /// The drafts mirror finished a pass: `Ok(None)` when there was nothing
    /// to do, which is the ordinary case.
    DraftsSwept(Box<Result<Option<cosmic_pim_mail::draft_sync::SweepReport>, String>>),
    /// A message in the Drafts folder came back as something editable.
    ServerDraftOpened(Box<Result<(String, cosmic_pim_mail::Draft), String>>),
    ComposeSend,
    ComposeSent(Box<crate::mail::Sent>),

    /// Something that happened in a window, to be routed only if that window
    /// is one of ours.
    ///
    /// Keystrokes arrive with the window they were typed in, main window
    /// included; routing the main window's own keys through
    /// [`Self::InWindow`] would make every one of them look detached.
    InWindowIfDetached(window::Id, Box<Message>),
    /// Something a detached window said, and which window said it.
    ///
    /// Every message a detached window's view produces is wrapped in this, and
    /// so is every message the resulting task produces — which is what carries
    /// a send's outcome back to the composer that started it rather than to
    /// whichever window happens to be focused when it lands.
    InWindow(window::Id, Box<Message>),
    /// Move the open message into a window of its own.
    Detach,
    /// A window finished opening.
    WindowOpened(window::Id),
    /// A window was asked to close — by its own header bar, or by the
    /// compositor. Distinct from [`Self::WindowClosed`]: this is the request,
    /// and it is where a draft gets saved.
    WindowCloseRequested(window::Id),
    /// A window is gone, and whatever it held goes with it.
    WindowClosed(window::Id),
    /// The main window is closing, which ends the process and every other
    /// window with it. The last moment anything can be kept.
    WindowsClosing,
    /// A window was resized, which is also the only notice a maximize gives.
    WindowResized(window::Id),
    WindowMaximizedChanged(window::Id, bool),
    /// The header bar of a detached window: drag, and the window buttons.
    WindowDrag,
    WindowToggleMaximize,
    WindowMinimize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
    Accounts,
    Settings,
    Shortcuts,
    Rules,
}

/// The add-a-rule form, as typed so far.
///
/// Indices rather than values for the two dropdowns, because a dropdown
/// speaks in rows: `field` indexes [`crate::ui::rules::FIELDS`], `move_to`
/// indexes the labels built beside `rule_move_wires` (0 is "no move").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleForm {
    pub name: String,
    pub field: usize,
    pub contains: String,
    pub mark_read: bool,
    pub star: bool,
    pub delete: bool,
    pub move_to: usize,
}

/// The menu's entries are registry actions, so a menu item and the keystroke
/// beside it cannot mean different things.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MenuAction(pub Action);

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        Message::Act(self.0)
    }
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    /// What this launch is asking for. See [`crate::flags`] — it is a type
    /// rather than a string because that is what lets a second launch reach
    /// the first over D-Bus instead of opening another window.
    type Flags = crate::flags::Flags;
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let about = About::default()
            .name(fl!("app-title"))
            .icon(widget::icon::from_svg_bytes(APP_ICON))
            .version(env!("CARGO_PKG_VERSION"))
            .license(env!("CARGO_PKG_LICENSE"))
            .links([(fl!("repository"), REPOSITORY)]);

        let accounts = load_accounts();

        let mut model = Self {
            core,
            about,
            context_page: ContextPage::default(),
            // The map libcosmic reads to draw an accelerator beside a menu
            // entry. Built from the registry rather than written out, so the
            // menu shows the shortcut the keyboard handler actually matches.
            key_binds: actions::combinations()
                .into_iter()
                .map(|(bind, action)| (bind, MenuAction(action)))
                .collect(),
            accounts,
            // Filled in below, once the saved settings are loaded.
            selected_account: None,
            connection: None,
            folders: Vec::new(),
            selected_folder: None,
            unread: HashMap::new(),
            conversations: Vec::new(),
            selected_conversation: None,
            opened: None,
            loading_conversations: false,
            syncing: false,
            cycle: 0,
            // A crash last session is said once, here, rather than never: the
            // process is usually started by a desktop entry, so the panic
            // message on stderr went nowhere anyone will look.
            status: crate::crash::take_unreported()
                .last()
                .map(|report| fl!("crashed-last-time", path = report.display().to_string())),
            toasts: widget::Toasts::new(Message::ToastClosed),
            pending_toasts: Vec::new(),
            list_error: None,
            reader_error: None,
            mail_form: None,
            windows: HashMap::new(),
            routed: None,
            maximized: std::collections::HashSet::new(),
            sign_in_providers: mail::sign_in_providers(),
            add_form: None,
            signing_in: false,
            palette: None,
            folder_dialog: None,
            move_rows: Vec::new(),
            dragging: None,
            sidebar_width: crate::config::DEFAULT_SIDEBAR_WIDTH as f32,
            list_width: crate::config::DEFAULT_LIST_WIDTH as f32,
            conversation_labels: Vec::new(),
            known_labels: Vec::new(),
            label_rows: Vec::new(),
            identity_labels: Vec::new(),
            rules: Vec::new(),
            rule_form: RuleForm::default(),
            rule_move_labels: Vec::new(),
            rule_move_wires: Vec::new(),
            palette_rows: Vec::new(),
            undo_stack: Vec::new(),
            drafts: Vec::new(),
            showing_drafts: false,
            outbox: Vec::new(),
            showing_outbox: false,
            unified: Vec::new(),
            showing_unified: false,
            search_focused: false,
            pending_chord: None,
            poll_seconds: String::new(),
            send_delay: String::new(),
            watch_generation: 0,
            watching: false,
            dbus: None,
            config: cosmic_config::Config::new(Self::APP_ID, Config::VERSION)
                .map(|context| match Config::get_entry(&context) {
                    Ok(config) => config,
                    Err((why, config)) => {
                        // Reported, not swallowed: a configuration that failed
                        // to load looks exactly like one that was never saved,
                        // and the user would put the setting back and watch it
                        // vanish again.
                        for why in why {
                            tracing::warn!(%why, "could not read the saved settings");
                        }
                        config
                    }
                })
                .unwrap_or_default(),
            search: String::new(),
            expanded_quotes: std::collections::HashSet::new(),
            results: Vec::new(),
            searching: false,
        };
        // The crash notice is a toast as well as a status line: the drawer
        // is closed on launch, and a notice nobody can see is not a notice.
        if let Some(text) = model.status.clone() {
            model.pending_toasts.push(text);
        }
        model.sidebar_width = model.config.sidebar_width();
        model.list_width = model.config.list_width();
        // The settings fields show the loaded values from the first frame,
        // not from the first config-change event.
        model.poll_seconds = model.config.poll_seconds.to_string();
        model.send_delay = model.config.send_delay_seconds.to_string();
        // The account that was open last, if it is still there. Falling back to
        // the first rather than to none: an application that opens on nothing
        // when its remembered account was removed is one the user has to
        // rescue.
        model.selected_account = model
            .accounts
            .iter()
            .find(|account| account.id == model.config.last_account)
            .or_else(|| model.accounts.first())
            .map(|account| account.id.clone());
        model.rebuild_connection();

        // A link the desktop handed us opens straight into a composer window.
        // Done before the folder load so the user sees what they clicked on
        // rather than an inbox that grows a composer a moment later.
        let launched = model.launch(flags.launch.as_ref());

        // The window is filled from disk first and the server is asked
        // afterwards, in that order: the mailbox is already there, so there is
        // no reason for the first frame to wait on a network round trip.
        let cached = model.load_cached_folders();
        let drafts = model.reload_drafts();
        let outbox = model.reload_outbox();
        let first_sync = model.sync_now();
        (
            model,
            Task::batch([launched, cached, drafts, outbox, first_sync]),
        )
    }

    fn header_end(&self) -> Vec<Element<'_, Self::Message>> {
        let search = widget::text_input(fl!("search"), &self.search)
            .id(SEARCH_ID.clone())
            .on_input(Message::SearchChanged)
            .on_clear(Message::SearchCleared)
            // Every text field reports focus, so single-letter shortcuts know
            // to stay out of the way.
            .on_focus(Message::SearchFocused)
            .on_unfocus(Message::SearchUnfocused)
            .width(Length::Fixed(260.0));
        vec![search.into()]
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        vec![
            // libcosmic only draws its own toggle when `nav_model` is `Some`,
            // and ours is not, so the stock button is placed here by hand and
            // routed through the registry like every other action.
            widget::nav_bar_toggle()
                .active(self.core().nav_bar_active())
                .on_toggle(Message::Act(Action::ToggleSidebar))
                .into(),
            menu::bar(vec![menu::Tree::with_children(
                menu::root(fl!("app-title")).apply(Element::from),
                menu::items(
                    &self.key_binds,
                    vec![
                        item(Action::Compose),
                        item(Action::ImportMbox),
                        menu::Item::Divider,
                        item(Action::Snooze),
                        item(Action::Label),
                        item(Action::Detach),
                        item(Action::MoveToFolder),
                        item(Action::NewFolder),
                        item(Action::RenameFolder),
                        item(Action::DeleteFolder),
                        menu::Item::Divider,
                        item(Action::Search),
                        item(Action::Sync),
                        item(Action::ToggleSidebar),
                        menu::Item::Divider,
                        item(Action::Rules),
                        item(Action::Shortcuts),
                        item(Action::Settings),
                        item(Action::Accounts),
                        item(Action::About),
                    ],
                ),
            )])
            .into(),
        ]
    }

    fn nav_model(&self) -> Option<&widget::nav_bar::Model> {
        // Folders are a filter over one view, not a set of pages, and the
        // sidebar also carries the account picker. Both are the wrong shape for
        // the stock single-select model, so `nav_bar` below fills the slot.
        None
    }

    fn nav_bar(&self) -> Option<Element<'_, cosmic::Action<Self::Message>>> {
        if !self.core().nav_bar_active() {
            return None;
        }
        let sidebar = crate::ui::sidebar::Sidebar {
            accounts: &self.accounts,
            selected_account: self.selected_account.as_deref(),
            folders: &self.folders,
            selected_folder: self.selected_folder,
            unread: &self.unread,
            drafts: self.drafts.len(),
            showing_drafts: self.showing_drafts,
            outbox: self.outbox.len(),
            showing_outbox: self.showing_outbox,
            // Two mailboxes are what make a merged view a view; with one it is
            // the inbox with an extra name.
            offer_unified: self.accounts.iter().filter(|a| a.mail.is_some()).count() >= 2,
            showing_unified: self.showing_unified,
        }
        .view();
        // The shell puts the nav slot in a row with no width of its own, so
        // the sidebar states its own — and carries the edge that adjusts it,
        // which has to live on this side of the slot boundary.
        Some(
            widget::row::with_capacity(2)
                .push(
                    widget::container(sidebar)
                        .width(Length::Fixed(self.sidebar_width))
                        .height(Length::Fill),
                )
                .push(crate::ui::column_handle(Message::SplitPressed(
                    Split::Sidebar,
                )))
                .apply(Element::from)
                .map(cosmic::Action::App),
        )
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::batch([
            // Unconditional. Gating it on "is an account configured" would mean
            // the timer does not exist yet when one is added, and the first
            // check would wait for a restart.
            cosmic::iced::time::every(self.config.poll_interval()).map(|_| Message::Poll),
            // Settings changed by another process — cosmic-settings, a text
            // editor — take effect without a restart.
            self.core()
                .watch_config::<Config>(Self::APP_ID)
                .map(|update| Message::ConfigChanged(update.config)),
            // Every key press, decided in `update` — the decision needs the
            // model (is a text field focused, is a chord half-typed) and a
            // subscription does not have it. Carrying the window it was typed
            // in, because that decides *what* it acts on: `e` in a detached
            // message archives that message, and `e` in a composer is a
            // letter.
            cosmic::iced::event::listen_with(|event, status, id| {
                use cosmic::iced::keyboard::Event;
                match event {
                    cosmic::iced::Event::Keyboard(Event::KeyPressed {
                        key,
                        modifiers,
                        physical_key,
                        ..
                    // A key a widget has already handled — a character going
                    // into a text field, a scroll — is not ours to reinterpret.
                    }) if status == cosmic::iced::event::Status::Ignored => Some(
                        Message::InWindowIfDetached(
                            id,
                            Box::new(Message::KeyPressed(modifiers, key, Some(physical_key))),
                        ),
                    ),
                    _ => None,
                }
            }),
            // The window frame this application draws itself needs the state
            // libcosmic keeps for the main window only: whether each window is
            // maximized, and when one has gone.
            // Only while a column edge is held. `mouse_area` reports movement
            // solely while the pointer is over it, and a fast gesture leaves a
            // seven-pixel strip immediately — so the drag is followed at the
            // window level instead, by a subscription that exists for exactly
            // as long as the drag does.
            if self.dragging.is_some() {
                cosmic::iced::event::listen_with(|event, _, _| {
                    use cosmic::iced::mouse::{Button, Event};
                    match event {
                        cosmic::iced::Event::Mouse(Event::CursorMoved { position }) => {
                            Some(Message::SplitMoved(position.x))
                        }
                        cosmic::iced::Event::Mouse(Event::ButtonReleased(Button::Left)) => {
                            Some(Message::SplitReleased)
                        }
                        _ => None,
                    }
                })
            } else {
                Subscription::none()
            },
            cosmic::iced::event::listen_with(|event, _, id| match event {
                cosmic::iced::Event::Window(window::Event::Resized(_)) => {
                    Some(Message::WindowResized(id))
                }
                cosmic::iced::Event::Window(window::Event::CloseRequested) => {
                    Some(Message::WindowCloseRequested(id))
                }
                cosmic::iced::Event::Window(window::Event::Closed) => {
                    Some(Message::WindowClosed(id))
                }
                _ => None,
            }),
        ])
    }

    /// The application is closing.
    ///
    /// A composer's save-on-close path only runs when that *composer* is
    /// closed; without this, quitting with half-written messages open would
    /// discard them — the exact loss the drafts store exists to prevent. Every
    /// open composer, because the main window closing takes the others with
    /// it. The save is synchronous because a `Task` returned here would race
    /// the exit.
    fn on_app_exit(&mut self) -> Option<Self::Message> {
        self.save_open_drafts();
        None
    }

    /// A window is going away.
    ///
    /// libcosmic reports this once the window is actually gone, so it is a
    /// notice rather than a veto — which is why a composer's window is opened
    /// with `exit_on_close_request` off and routed through
    /// [`Message::WindowCloseRequested`] instead, where there is still
    /// something to save. This is the backstop for the closes that do not come
    /// through there, and the main window's own close, which takes every other
    /// window with it.
    fn on_close_requested(&self, id: window::Id) -> Option<Self::Message> {
        Some(if self.core.main_window_is(id) {
            Message::WindowsClosing
        } else {
            Message::WindowClosed(id)
        })
    }

    /// Escape, routed here by libcosmic's `keyboard_nav`.
    ///
    /// Implemented as a hook rather than as one of our own bindings because the
    /// framework already dispatches it: matching it twice would close a
    /// composer *and* a context drawer on one keystroke.
    fn on_escape(&mut self) -> Task<Self::Message> {
        self.escape()
    }

    /// `Ctrl+F`, likewise routed here rather than matched twice.
    fn on_search(&mut self) -> Task<Self::Message> {
        self.act(Action::Search)
    }

    /// A `mailto:` link, or a desktop-entry action, arriving at an instance
    /// that is already running.
    ///
    /// Without this the desktop file lies: it declares `DBusActivatable=true`
    /// and a `compose` action, and libcosmic's single-instance support hands
    /// the second launch over here rather than starting a second process. A
    /// `mailto:` clicked while Envelope is open would otherwise do nothing at
    /// all — the new process exits, and the running one is never told.
    fn dbus_activation(
        &mut self,
        message: cosmic::dbus_activation::Message,
    ) -> Task<Self::Message> {
        match message.msg {
            cosmic::dbus_activation::Details::Open { url } => {
                // The first mailto: wins. Several at once is not a thing that
                // happens, and opening four composers because a page had four
                // links would be worse than opening one.
                for url in url {
                    if url.scheme().eq_ignore_ascii_case("mailto") {
                        return self.open_mailto(url.as_str());
                    }
                }
                Task::none()
            }
            cosmic::dbus_activation::Details::ActivateAction { action, .. } => {
                match action.parse::<crate::flags::Launch>() {
                    Ok(launch) => self.launch(Some(&launch)),
                    Err(why) => {
                        tracing::warn!(action, %why, "an activation this build does not understand");
                        Task::none()
                    }
                }
            }
            // Plain activation: the window is being raised, which the runtime
            // has already done.
            cosmic::dbus_activation::Details::Activate => Task::none(),
        }
    }

    /// The runtime's session-bus connection, which owns this app's
    /// single-instance name.
    ///
    /// Two uses, both the iMIP hand-off: outgoing calls to Slate ride it, and
    /// the mailer side of the contract — `SendSchedulingReply`, the method
    /// Slate calls to queue a reply — is exported here, on the name the
    /// connection already owns. An export that fails leaves Envelope a mailer
    /// without the hand-off, which is the degraded mode the contract already
    /// allows for; it is logged, not fatal.
    fn dbus_connection(&mut self, conn: zbus::Connection) -> Task<Self::Message> {
        self.dbus = Some(conn.clone());
        cosmic::iced::Task::future(async move {
            if let Err(why) = conn
                .object_server()
                .at(
                    crate::scheduling::ENVELOPE_PATH,
                    crate::scheduling::Scheduling,
                )
                .await
            {
                tracing::warn!(%why, "the scheduling interface could not be exported");
            }
        })
        .discard()
    }

    fn dialog(&self) -> Option<Element<'_, Self::Message>> {
        if let Some(dialog) = self.folder_dialog.as_ref() {
            return match dialog {
                FolderDialog::Create { name } => Some(crate::ui::folders::name_dialog(
                    fl!("new-folder"),
                    fl!("create"),
                    name,
                )),
                FolderDialog::Rename { name } => Some(crate::ui::folders::name_dialog(
                    fl!("rename-folder"),
                    fl!("rename"),
                    name,
                )),
                FolderDialog::Delete => {
                    self.current_folder().map(crate::ui::folders::delete_dialog)
                }
                FolderDialog::RemoveAccount { name, .. } => {
                    Some(crate::ui::folders::confirm_dialog(
                        fl!("remove-account-title", name = name.clone()),
                        fl!("remove-account-warning"),
                        fl!("remove"),
                    ))
                }
                FolderDialog::DeleteRule { name, .. } => Some(crate::ui::folders::confirm_dialog(
                    fl!("delete-rule-title", name = name.clone()),
                    fl!("delete-rule-warning"),
                    fl!("delete"),
                )),
                FolderDialog::Snooze => Some(crate::ui::folders::snooze_dialog()),
                FolderDialog::Label { query, selected } => Some(
                    crate::ui::folders::LabelPicker {
                        query,
                        rows: &self.label_rows,
                        selected: *selected,
                    }
                    .view(),
                ),
                FolderDialog::SendLater => Some(crate::ui::folders::send_later_dialog()),
                FolderDialog::Move { query, selected } => Some(
                    crate::ui::folders::MovePicker {
                        query,
                        matches: &self.move_rows,
                        folders: &self.folders,
                        selected: *selected,
                    }
                    .view(),
                ),
            };
        }
        let (query, selected) = self.palette.as_ref()?;
        let palette = crate::ui::palette::Palette {
            query,
            matches: &self.palette_rows,
            selected: *selected,
        };
        Some(palette.view())
    }

    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Self::Message>> {
        if !self.core.window.show_context {
            return None;
        }
        Some(match self.context_page {
            ContextPage::About => context_drawer::about(
                &self.about,
                |url| Message::LaunchUrl(url.to_string()),
                Message::ToggleContextPage(ContextPage::About),
            ),
            ContextPage::Accounts => context_drawer::context_drawer(
                crate::ui::accounts::view(
                    &self.accounts,
                    self.mail_form.as_ref(),
                    self.add_form.as_ref(),
                    crate::ui::accounts::SignIn {
                        providers: &self.sign_in_providers,
                        in_flight: self.signing_in,
                    },
                    self.syncing,
                    self.status.as_deref(),
                ),
                Message::ToggleContextPage(ContextPage::Accounts),
            )
            .title(fl!("accounts")),
            ContextPage::Settings => context_drawer::context_drawer(
                crate::ui::settings::view(&self.config, &self.poll_seconds, &self.send_delay),
                Message::ToggleContextPage(ContextPage::Settings),
            )
            .title(fl!("settings")),
            ContextPage::Shortcuts => context_drawer::context_drawer(
                crate::ui::shortcuts::view(),
                Message::ToggleContextPage(ContextPage::Shortcuts),
            )
            .title(fl!("shortcuts")),
            ContextPage::Rules => context_drawer::context_drawer(
                crate::ui::rules::Rules {
                    rules: &self.rules,
                    form: &self.rule_form,
                    move_labels: &self.rule_move_labels,
                    folders: &self.folders,
                }
                .view(),
                Message::ToggleContextPage(ContextPage::Rules),
            )
            .title(fl!("rules")),
        })
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let list = if self.showing_unified {
            crate::ui::list::unified(&self.unified)
        } else if self.is_searching() {
            crate::ui::list::results(&self.results, &self.folders, self.searching, SEARCH_LIMIT)
        } else if self.showing_outbox {
            crate::ui::list::outbox(&self.outbox)
        } else if self.showing_drafts {
            crate::ui::list::drafts(&self.drafts)
        } else {
            crate::ui::list::List {
                conversations: &self.conversations,
                labels: &self.conversation_labels,
                selected: self.selected_conversation,
                loading: self.loading_conversations,
                error: self.list_error.as_deref(),
            }
            .view()
        };

        // The reading pane, and only the reading pane. Writing happens in a
        // window of its own — see [`Detached`] — so the thread being answered
        // stays on screen beside the answer instead of being replaced by it.
        let right = crate::ui::reader::Reader {
            opened: self.opened.as_ref(),
            error: self.reader_error.as_deref(),
            can_send: self.can_send(),
            expanded_quotes: &self.expanded_quotes,
            detachable: true,
        }
        .view();

        let content = widget::row::with_capacity(3)
            .push(
                widget::container(list)
                    // A set width rather than a proportion: a message list is
                    // read down its left edge, and a column that grows with
                    // the window puts the sender and the date at opposite ends
                    // of a wide screen. Which width is the user's to decide —
                    // their folder names and their screen — so the edge is
                    // draggable and the answer is remembered.
                    .width(Length::Fixed(self.list_width))
                    .height(Length::Fill),
            )
            .push(crate::ui::column_handle(Message::SplitPressed(Split::List)))
            .push(
                widget::container(right)
                    .width(Length::Fill)
                    .height(Length::Fill),
            );

        // The standard chrome: confirmations surface over the content,
        // wherever the user is looking.
        widget::toaster(&self.toasts, content)
    }

    /// A detached window: the frame, and whatever it is holding.
    ///
    /// libcosmic's window template stops at the main window — `view_main` is
    /// applied to that one and `view_window` is handed the bare element for
    /// every other — so the header bar, the background and the corners are
    /// drawn here rather than inherited. Everything the contents say is
    /// wrapped in [`Message::InWindow`], which is what lets one `update` serve
    /// every window without a composer having to know its own window id.
    fn view_window(&self, id: window::Id) -> Element<'_, Self::Message> {
        let Some(detached) = self.windows.get(&id) else {
            // A window the runtime knows about and the model does not, which
            // is what a closing window looks like for a frame. The default
            // implementation of this method panics; a blank frame on the way
            // out is a better answer than a crash.
            return widget::Space::new().into();
        };

        let content: Element<'_, Message> = match detached {
            Detached::Compose(composer) => crate::ui::composer::view(
                composer,
                &self.identity_labels,
                self.identity_index(composer),
            ),
            Detached::Read(reading) => crate::ui::reader::Reader {
                opened: Some(&reading.opened),
                error: None,
                can_send: self.can_send(),
                expanded_quotes: &reading.expanded_quotes,
                // Already in a window of its own; the button would open a
                // window onto the window the user is looking at.
                detachable: false,
            }
            .view(),
        };

        // Squared off when maximized, like every other COSMIC window. The
        // state is this application's to track: libcosmic keeps `Core`'s copy
        // for the main window only.
        let maximized = self.maximized.contains(&id);

        widget::column::with_capacity(2)
            .push(
                widget::header_bar()
                    .title(self.title(id))
                    .focused(self.core.focused_window() == Some(id))
                    .maximized(maximized)
                    .sharp_corners(maximized)
                    .on_close(Message::WindowCloseRequested(id))
                    .on_drag(Message::WindowDrag)
                    .on_maximize(Message::WindowToggleMaximize)
                    .on_double_click(Message::WindowToggleMaximize)
                    .on_minimize(Message::WindowMinimize),
            )
            .push(
                widget::container(content)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .apply(widget::container)
            .class(cosmic::theme::Container::WindowBackground)
            .width(Length::Fill)
            .height(Length::Fill)
            .apply(Element::from)
            .map(move |message| Message::InWindow(id, Box::new(message)))
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        let task = self.dispatch(message);
        // Anything said this turn becomes a toast, so a confirmation is
        // seen wherever the user is looking — not only inside the
        // Accounts drawer, which was the one place that rendered the
        // status line.
        let announced = self.flush_toasts();
        Task::batch([task, announced])
    }
}

impl AppModel {
    /// Says something transient to the user, wherever they are looking.
    ///
    /// Also mirrors into the status line, which the Accounts drawer still
    /// renders inline — sync progress reads better as standing text there.
    fn say(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.status = Some(text.clone());
        self.pending_toasts.push(text);
    }

    /// Turns this turn's sayings into toasts, returning the expiry tasks —
    /// which must reach the runtime, or a toast never leaves on its own.
    fn flush_toasts(&mut self) -> Task<Message> {
        let tasks: Vec<Task<Message>> = self
            .pending_toasts
            .drain(..)
            .map(|text| {
                self.toasts
                    .push(widget::Toast::new(text))
                    .map(cosmic::Action::App)
            })
            .collect();
        Task::batch(tasks)
    }

    /// The message handler proper — `update` wraps it so every status
    /// write in here is announced exactly once per turn.
    #[allow(clippy::too_many_lines)]
    fn dispatch(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::LaunchUrl(url) => {
                if let Err(why) = open::that_detached(&url) {
                    tracing::warn!(url, %why, "could not open the link");
                }
                Task::none()
            }

            Message::ToggleContextPage(page) => {
                if self.context_page == page {
                    self.core.window.show_context = !self.core.window.show_context;
                } else {
                    self.context_page = page;
                    self.core.window.show_context = true;
                }
                Task::none()
            }

            Message::AccountSelected(id) => {
                if self.selected_account.as_deref() == Some(id.as_str()) {
                    return Task::none();
                }
                self.remember(|config| config.last_account = id.clone());
                self.selected_account = Some(id);
                // Retire the old account's watch: its thread cannot be
                // cancelled, but its result now carries a stale generation and
                // will be dropped on arrival.
                self.watch_generation += 1;
                self.watching = false;
                // The entries name folders and hold messages of the account
                // being left; applying one to the next account would file its
                // mail somewhere it has never been.
                self.undo_stack.clear();
                self.clear_mailbox_state();
                self.rebuild_connection();
                Task::batch([
                    self.load_cached_folders(),
                    self.reload_drafts(),
                    self.reload_outbox(),
                    self.sync_now(),
                ])
            }

            Message::SyncNow => self.sync_now(),

            Message::Poll => {
                // A poll is not a command. If a pass is already running, or
                // there is nothing to sync, it does nothing at all — and it
                // never clears a status line the user is in the middle of
                // reading, which is what makes an automatic check different
                // from the button.
                if self.syncing || self.connection.is_none() {
                    return Task::none();
                }
                self.sync_now()
            }

            Message::SyncFinished(result) => {
                self.syncing = false;
                match *result {
                    Ok(report) => {
                        // A pass that found nothing says nothing. An automatic
                        // check that overwrites the status line every two
                        // minutes with "0 new" trains the user to ignore it.
                        if report.is_worth_reporting() {
                            self.say(summarise(&report));
                        }
                        // Folders come from the server, so this is also how a
                        // newly created mailbox appears.
                        if !report.folders.is_empty() {
                            self.folders = report.folders;
                            if self.selected_folder.is_none() {
                                self.selected_folder = self.restore_folder();
                            }
                        }
                        Task::batch([
                            self.reload_conversations(),
                            self.reload_outbox(),
                            if self.showing_unified {
                                self.reload_unified()
                            } else {
                                Task::none()
                            },
                            // Edits and discards made while offline reach the
                            // server's Drafts folder on the same schedule as
                            // every other queued write.
                            self.sweep_drafts_now(),
                            // Filters run over what the pass just brought in.
                            self.apply_rules_now(),
                            // And anything snoozed past its time comes back.
                            self.wake_snoozed_now(),
                            // The first successful sync proves the connection
                            // works; that is the moment to park the watch.
                            self.start_watch(),
                        ])
                    }
                    Err(why) => {
                        self.say(fl!("sync-failed", reason = why));
                        Task::none()
                    }
                }
            }

            Message::FolderSelected(index) => {
                self.showing_drafts = false;
                self.showing_unified = false;
                if self.selected_folder == Some(index) {
                    return Task::none();
                }
                self.selected_folder = Some(index);
                self.selected_conversation = None;
                self.opened = None;
                self.reader_error = None;
                if let Some(folder) = self.current_folder().cloned() {
                    self.remember(|config| config.last_folder = folder.wire_name);
                }
                Task::batch([self.reload_conversations(), self.update_title()])
            }

            Message::ConversationsLoaded(result) => {
                self.loading_conversations = false;
                match result {
                    Ok((conversations, labels)) => {
                        self.list_error = None;
                        if let Some(folder) = self.current_folder() {
                            self.unread.insert(
                                folder.wire_name.clone(),
                                conversations.iter().filter(|c| c.unread).count(),
                            );
                        }
                        self.conversations = conversations;
                        self.conversation_labels = labels;
                        // The selection is an index into a list that just
                        // changed underneath it; keeping it would open an
                        // unrelated conversation.
                        self.selected_conversation = None;
                    }
                    Err(why) => {
                        self.conversations.clear();
                        self.conversation_labels.clear();
                        self.list_error = Some(why);
                    }
                }
                Task::none()
            }

            Message::ConversationSelected(index) => {
                self.selected_conversation = Some(index);
                // The draft stays. Clicking another conversation to check
                // something while writing a reply is normal, and losing what
                // was typed for it would be indefensible — the composer is
                // restored the moment the reader is not showing.
                self.reader_error = None;
                self.open_selected()
            }

            Message::OpenInCalendar => {
                let Some(invitation) = self
                    .target()
                    .and_then(|target| target.opened)
                    .and_then(|opened| opened.invitation.clone())
                else {
                    return Task::none();
                };
                let Some(account_id) = self
                    .connection
                    .as_ref()
                    .map(|connection| connection.account_id.clone())
                else {
                    return Task::none();
                };
                let Some(conn) = self.dbus.clone() else {
                    self.say(fl!("calendar-not-running"));
                    return Task::none();
                };
                cosmic::task::future(async move {
                    Message::InvitationDelivered(
                        crate::scheduling::deliver_invitation(&conn, invitation.ics, account_id)
                            .await,
                    )
                })
            }

            Message::InvitationDelivered(result) => {
                use crate::scheduling::Delivered;
                self.say(match result {
                    Ok(Delivered::Accepted) => fl!("invitation-in-calendar"),
                    Ok(Delivered::Refused) => fl!("invitation-refused"),
                    // Absence, not an error: the part is still saveable from
                    // the attachment list, and the status says so.
                    Ok(Delivered::NoCalendar) => fl!("calendar-not-running"),
                    Err(why) => fl!("invitation-failed", reason = why),
                });
                Task::none()
            }

            Message::MessageOpened(result) => {
                match *result {
                    Ok(opened) => {
                        self.reader_error = None;
                        let already_read = opened.flags.seen;
                        self.expanded_quotes.clear();
                        self.opened = Some(opened);
                        // Opening a message marks it read, which is what every
                        // mail client does and what users expect. It goes
                        // through the same queued path as the button, so it
                        // reaches the server rather than being a local lie —
                        // and it is a setting, because for somebody using their
                        // inbox as a to-do list, a message losing its unread
                        // mark on a glance is losing a task.
                        if !already_read && self.config.mark_read_on_open {
                            return self.set_flags(|flags| Flags {
                                seen: true,
                                ..flags
                            });
                        }
                    }
                    Err(why) => {
                        self.opened = None;
                        self.reader_error = Some(why);
                    }
                }
                Task::none()
            }

            Message::ToggleRead => {
                let seen = self
                    .target()
                    .and_then(|target| target.opened)
                    .is_some_and(|opened| opened.flags.seen);
                self.set_flags(move |flags| Flags {
                    seen: !seen,
                    ..flags
                })
            }

            Message::ToggleFlagged => {
                let flagged = self
                    .target()
                    .and_then(|target| target.opened)
                    .is_some_and(|opened| opened.flags.flagged);
                self.set_flags(move |flags| Flags {
                    flagged: !flagged,
                    ..flags
                })
            }

            Message::Archive => self.move_selected(SpecialUse::Archive),
            Message::Delete => self.move_selected(SpecialUse::Trash),

            Message::Mutated(result) => {
                match result {
                    Ok(Some(entry)) => {
                        self.undo_stack.push(entry);
                        if self.undo_stack.len() > UNDO_DEPTH {
                            self.undo_stack.remove(0);
                        }
                    }
                    Ok(None) => {}
                    Err(why) => self.say(why),
                }
                self.reload_conversations()
            }
            Message::Undone(result) => {
                match result {
                    Ok(what) => self.say(fl!("undone", what = what)),
                    Err(why) => self.say(fl!("undo-failed", reason = why)),
                }
                self.reload_conversations()
            }

            Message::MailFormStart(id) => {
                self.mail_form = self.accounts.iter().find(|a| a.id == id).map(MailForm::new);
                self.add_form = None;
                self.context_page = ContextPage::Accounts;
                self.core.window.show_context = true;
                Task::none()
            }
            Message::MailFormHostChanged(host) => self.with_form(|form| form.host = host),
            Message::MailFormPortChanged(port) => self.with_form(|form| form.port = port),
            Message::MailFormTransportChanged(transport) => self.with_form(|form| {
                // The conventional port follows the encryption unless the user
                // has already typed something that is not one of them.
                if form.port == "993" || form.port == "143" {
                    form.port = match transport {
                        Transport::Tls => "993".to_string(),
                        Transport::StartTls | Transport::Plaintext => "143".to_string(),
                    };
                }
                form.transport = transport;
            }),
            Message::MailFormUsernameChanged(username) => {
                self.with_form(|form| form.username = username)
            }
            Message::MailFormProtocolChanged(protocol) => {
                self.with_form(|form| form.protocol = protocol)
            }
            Message::MailFormJmapUrlChanged(url) => self.with_form(|form| form.jmap_url = url),
            Message::MailFormSmtpHostChanged(host) => self.with_form(|form| form.smtp_host = host),
            Message::MailFormSmtpPortChanged(port) => self.with_form(|form| form.smtp_port = port),
            Message::MailFormSmtpTransportChanged(transport) => self.with_form(|form| {
                if form.smtp_port == "465" || form.smtp_port == "587" {
                    form.smtp_port = match transport {
                        Transport::Tls => "465".to_string(),
                        // 587 is the submission port for STARTTLS. Never 25 —
                        // a client has no business talking to a relay port.
                        Transport::StartTls | Transport::Plaintext => "587".to_string(),
                    };
                }
                form.smtp_transport = transport;
            }),
            Message::MailFormFromAddressChanged(address) => {
                // The no-network half of discovery runs on every keystroke, so
                // a recognised provider fills the form in as soon as the
                // domain is typed. The registry first — it knows the protocol
                // and the JMAP URL, which the table does not.
                let provider = mail::provider_settings(&address);
                let known = mail::known_settings(&address);
                self.with_form(|form| {
                    form.from_address = address;
                    if form.host.trim().is_empty() {
                        if let Some(endpoint) = provider {
                            form.apply_endpoint(&endpoint);
                        } else if let Some(found) = known {
                            form.apply(&found);
                        }
                    }
                })
            }
            Message::MailFormFromNameChanged(name) => self.with_form(|form| form.from_name = name),
            Message::MailFormAliasInputChanged(text) => {
                self.with_form(|form| form.alias_input = text)
            }
            Message::MailFormAliasAdded => {
                // Not through with_form: that helper clears the error after
                // the edit, and "that is not an address" must survive it.
                if let Some(form) = self.mail_form.as_mut() {
                    // `Name <address>` or a bare address; the substrate fills
                    // a missing name in with the primary's when it lists
                    // identities.
                    let input = form.alias_input.trim();
                    let (name, address) = match (input.rfind('<'), input.rfind('>')) {
                        (Some(open), Some(close)) if open < close => (
                            input[..open].trim().to_owned(),
                            input[open + 1..close].trim().to_owned(),
                        ),
                        _ => (String::new(), input.to_owned()),
                    };
                    if !address.contains('@') {
                        form.error = Some(fl!("alias-needs-address"));
                    } else if form
                        .aliases
                        .iter()
                        .any(|alias| alias.address.eq_ignore_ascii_case(&address))
                    {
                        form.alias_input.clear();
                    } else {
                        form.aliases
                            .push(cosmic_pim_accounts::Alias { name, address });
                        form.alias_input.clear();
                        form.error = None;
                    }
                }
                Task::none()
            }
            Message::MailFormAliasRemoved(index) => self.with_form(|form| {
                if index < form.aliases.len() {
                    form.aliases.remove(index);
                }
            }),
            Message::PaletteQueryChanged(query) => {
                if let Some((text, selected)) = self.palette.as_mut() {
                    *text = query;
                    // The list under the cursor just changed; keeping the old
                    // position would highlight an unrelated row.
                    *selected = 0;
                }
                self.refresh_palette_rows();
                Task::none()
            }
            Message::PaletteSubmitted => {
                let action = self
                    .palette_rows
                    .get(self.palette.as_ref().map_or(0, |(_, s)| *s))
                    .map(|(action, _)| *action);
                match action {
                    Some(action) => self.update(Message::PaletteInvoked(action)),
                    None => Task::none(),
                }
            }
            Message::PaletteInvoked(action) => {
                // Closed before dispatch, so an action that opens something —
                // the composer, a context page — is not immediately covered by
                // the palette it came from.
                self.palette = None;
                self.act(action)
            }
            Message::AddFormStart => {
                self.add_form = Some(AddForm::default());
                self.mail_form = None;
                self.context_page = ContextPage::Accounts;
                self.core.window.show_context = true;
                Task::none()
            }
            Message::AddFormCancel => {
                self.add_form = None;
                Task::none()
            }
            Message::AddFormNameChanged(name) => self.with_add_form(|form| form.name = name),
            Message::AddFormEmailChanged(email) => {
                // The registry answers without a network, so the form can say
                // "this one signs in with the browser" as soon as the domain
                // is typed.
                let provider = mail::provider_note(&email);
                self.with_add_form(|form| {
                    form.email = email;
                    form.provider = provider;
                })
            }
            Message::AddFormPasswordChanged(password) => {
                self.with_add_form(|form| form.password = password)
            }
            Message::AddFormConfirm => self.confirm_add(),
            Message::AddFormDiscovered(result) => {
                self.add_account(result.map(|found| mail::endpoint_of(&found)))
            }
            Message::AccountRemove(id) => {
                let name = self
                    .accounts
                    .iter()
                    .find(|account| account.id == id)
                    .map_or_else(|| id.clone(), |account| account.display_name.clone());
                self.update(Message::FolderDialogOpened(FolderDialog::RemoveAccount {
                    id,
                    name,
                }))
            }
            Message::SignInStarted(provider_id) => self.sign_in(&provider_id),
            Message::SignInFinished(result) => {
                self.signing_in = false;
                match *result {
                    Ok(account_id) => {
                        self.add_form = None;
                        self.accounts = load_accounts();
                        // Straight into the new account: the sign-in was the
                        // whole point of the visit.
                        self.update(Message::AccountSelected(account_id))
                    }
                    Err(why) => {
                        let why = fl!("sign-in-failed", reason = why);
                        match self.add_form.as_mut() {
                            Some(form) => form.error = Some(why),
                            None => self.say(why),
                        }
                        Task::none()
                    }
                }
            }
            Message::MailFormDiscover => self.discover_settings(),
            Message::MailFormDiscovered(result) => {
                if let Some(form) = self.mail_form.as_mut() {
                    form.discovering = false;
                    match *result {
                        Ok(found) => form.apply(&found),
                        Err(why) => form.error = Some(why),
                    }
                }
                Task::none()
            }
            Message::MailFormCancel => {
                self.mail_form = None;
                Task::none()
            }
            Message::MailFormSave => self.save_form(),

            Message::Compose => self.compose(false, |_, from| cosmic_pim_mail::Draft::new(from)),
            Message::Reply { all } => self.compose(true, move |opened, from| match opened {
                Some(opened) => cosmic_pim_mail::Draft::reply(&opened.message, from, all),
                None => cosmic_pim_mail::Draft::new(from),
            }),
            Message::Forward => self.compose(true, |opened, from| match opened {
                Some(opened) => cosmic_pim_mail::Draft::forward(&opened.message, from),
                None => cosmic_pim_mail::Draft::new(from),
            }),

            Message::ComposeFromSelected(index) => {
                if let Some(from) = self
                    .connection
                    .as_ref()
                    .and_then(|c| c.identities.get(index))
                    .cloned()
                {
                    return self.with_composer(|c| c.draft.from = from);
                }
                Task::none()
            }
            Message::ComposeToChanged(text) => self.with_composer(|c| c.to = text),
            Message::ComposeCcChanged(text) => self.with_composer(|c| c.cc = text),
            Message::ComposeBccChanged(text) => self.with_composer(|c| c.bcc = text),
            Message::ComposeSubjectChanged(text) => {
                let edited = self.with_composer(|c| c.draft.subject = text);
                // The window is named after the subject, so a window list is
                // useful while three replies are open. Retitled as it is
                // typed, because a title that lags is a title that lies.
                let (Some(id), Some(composer)) = (self.routed, self.composer()) else {
                    return edited;
                };
                let title = compose_title(&composer.draft.subject);
                Task::batch([edited, self.set_window_title(title, id)])
            }
            Message::ComposeBodyAction(action) => {
                self.with_composer(|composer| composer.edit(*action))
            }
            Message::ComposeCancel => self.finish_composing(true),
            Message::ComposeDiscard => self.finish_composing(false),

            // Dispatched here rather than in `update` so that anything which
            // reaches `dispatch` directly is routed the same way. The task is
            // wrapped straight back up: a send started in one composer lands
            // in that composer, whichever window has focus when the answer
            // arrives.
            Message::InWindow(id, inner) => {
                let previous = self.routed.replace(id);
                let task = self.dispatch(*inner);
                self.routed = previous;
                task.map(move |action| match action {
                    cosmic::Action::App(message) => {
                        cosmic::Action::App(Message::InWindow(id, Box::new(message)))
                    }
                    // Framework messages belong to the framework, not to a
                    // window of ours.
                    other => other,
                })
            }
            Message::InWindowIfDetached(id, inner) => {
                if self.windows.contains_key(&id) {
                    return self.dispatch(Message::InWindow(id, inner));
                }
                self.dispatch(*inner)
            }
            Message::Detach => self.detach_opened(),
            Message::WindowOpened(id) => {
                // Nothing to do but ask what state it opened in: a window that
                // the compositor tiled or maximized on the way up has to draw
                // the corners it actually has.
                window::is_maximized(id).map(move |maximized| {
                    cosmic::Action::App(Message::WindowMaximizedChanged(id, maximized))
                })
            }
            Message::WindowCloseRequested(id) => {
                // Ours to close, because a detached window is opened with
                // `exit_on_close_request` off so that a draft can be kept
                // first. The main window's close is the framework's, and
                // taking it here would be closing it twice.
                if self.windows.contains_key(&id) {
                    return self.close_window(id);
                }
                Task::none()
            }
            Message::WindowClosed(id) => {
                // The window is already gone, so there is nothing to close and
                // nothing to save that `WindowCloseRequested` did not already
                // save. This is the model catching up.
                self.windows.remove(&id);
                self.maximized.remove(&id);
                Task::none()
            }
            Message::WindowsClosing => {
                self.save_open_drafts();
                Task::none()
            }
            Message::WindowResized(id) => window::is_maximized(id).map(move |maximized| {
                cosmic::Action::App(Message::WindowMaximizedChanged(id, maximized))
            }),
            Message::WindowMaximizedChanged(id, maximized) => {
                if maximized {
                    self.maximized.insert(id);
                } else {
                    self.maximized.remove(&id);
                }
                Task::none()
            }
            Message::WindowDrag => self.core.drag(self.routed),
            Message::WindowToggleMaximize => self.core.toggle_maximize(self.routed),
            Message::WindowMinimize => self.core.minimize(self.routed),
            Message::DraftsLoaded(drafts) => {
                self.drafts = drafts;
                Task::none()
            }
            Message::ShowUnified => {
                self.showing_unified = !self.showing_unified;
                if self.showing_unified {
                    self.showing_drafts = false;
                    self.showing_outbox = false;
                    self.selected_conversation = None;
                    self.opened = None;
                    self.reader_error = None;
                    return self.reload_unified();
                }
                Task::none()
            }
            Message::UnifiedLoaded(unified) => {
                self.unified = unified;
                Task::none()
            }
            Message::UnifiedOpened(index) => self.open_unified(index),
            Message::ShowOutbox => {
                self.showing_outbox = !self.showing_outbox;
                if self.showing_outbox {
                    self.showing_drafts = false;
                    self.showing_unified = false;
                    self.selected_conversation = None;
                    self.opened = None;
                    self.reader_error = None;
                }
                self.reload_outbox()
            }
            Message::OutboxLoaded(queued) => {
                self.outbox = queued;
                Task::none()
            }
            Message::QueuedRetried(id) => {
                if let Some(connection) = self.connection.as_ref()
                    && let Err(why) = mail::retry_queued(connection, &id)
                {
                    self.say(why);
                }
                // Due now, so the next check takes it rather than waiting out a
                // backoff the user has just overridden.
                Task::batch([self.reload_outbox(), self.sync_now()])
            }
            Message::QueuedDiscarded(id) => {
                if let Some(connection) = self.connection.as_ref()
                    && let Err(why) = mail::discard_queued(connection, &id)
                {
                    self.say(why);
                }
                self.reload_outbox()
            }
            Message::WatchEnded {
                generation,
                outcome,
            } => self.watch_ended(generation, outcome),
            Message::ConfigChanged(config) => {
                // Not while a drag is in flight: the write this app is about
                // to make would arrive back as a change and fight the pointer.
                if self.dragging.is_none() {
                    self.sidebar_width = config.sidebar_width();
                    self.list_width = config.list_width();
                }
                self.poll_seconds = config.poll_seconds.to_string();
                self.send_delay = config.send_delay_seconds.to_string();
                self.config = config;
                Task::none()
            }
            Message::PollSecondsChanged(text) => {
                // The field holds text so a half-typed number is not rejected
                // mid-keystroke; only a whole one reaches the setting.
                if let Ok(seconds) = text.trim().parse::<u32>() {
                    self.remember(|config| config.poll_seconds = seconds);
                }
                self.poll_seconds = text;
                Task::none()
            }
            Message::MarkReadOnOpenChanged(on) => {
                self.remember(|config| config.mark_read_on_open = on);
                Task::none()
            }
            Message::ToggleQuote(index) => {
                let expanded = self.expanded_quotes_mut();
                if !expanded.remove(&index) {
                    expanded.insert(index);
                }
                Task::none()
            }
            Message::SearchFocused => {
                self.search_focused = true;
                Task::none()
            }
            Message::SearchUnfocused => {
                self.search_focused = false;
                Task::none()
            }
            Message::KeyPressed(modifiers, key, physical) => {
                self.key_pressed(&modifiers, &key, physical.as_ref())
            }
            Message::Act(action) => self.act(action),
            Message::SplitPressed(split) => {
                self.dragging = Some(Drag {
                    split,
                    origin: match split {
                        Split::Sidebar => self.sidebar_width,
                        Split::List => self.list_width,
                    },
                    from_x: None,
                });
                Task::none()
            }
            Message::SplitMoved(x) => {
                if let Some(drag) = self.dragging.as_mut() {
                    // The first move fixes the reference point, so the edge
                    // stays under the pointer rather than jumping to it.
                    let from = *drag.from_x.get_or_insert(x);
                    let wanted = drag.origin + (x - from);
                    let (range, target) = match drag.split {
                        Split::Sidebar => (crate::config::SIDEBAR_WIDTH, &mut self.sidebar_width),
                        Split::List => (crate::config::LIST_WIDTH, &mut self.list_width),
                    };
                    #[allow(clippy::cast_precision_loss, reason = "a width in pixels")]
                    let clamped = wanted.clamp(*range.start() as f32, *range.end() as f32);
                    *target = clamped;
                }
                Task::none()
            }
            Message::SplitReleased => {
                // Written once, at the end. A config write per frame of a drag
                // would be a file rewrite per pointer event, and every other
                // process watching the key would hear all of them.
                if let Some(drag) = self.dragging.take() {
                    #[allow(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "a width in pixels, already clamped to a small positive range"
                    )]
                    match drag.split {
                        Split::Sidebar => {
                            let width = self.sidebar_width as u32;
                            self.remember(|config| config.sidebar_width = width);
                        }
                        Split::List => {
                            let width = self.list_width as u32;
                            self.remember(|config| config.list_width = width);
                        }
                    }
                }
                Task::none()
            }
            Message::ToastClosed(id) => {
                self.toasts.remove(id);
                Task::none()
            }

            Message::ShowDrafts => {
                self.showing_drafts = !self.showing_drafts;
                if self.showing_drafts {
                    self.showing_outbox = false;
                    self.showing_unified = false;
                    self.selected_conversation = None;
                    self.opened = None;
                    self.reader_error = None;
                }
                Task::none()
            }
            Message::SearchChanged(text) => {
                self.search = text;
                self.run_search()
            }
            Message::SearchFinished(results) => {
                self.searching = false;
                self.results = results;
                Task::none()
            }
            Message::SearchCleared => {
                self.search.clear();
                self.results.clear();
                Task::none()
            }
            Message::HitOpened(index) => self.open_hit(index),

            Message::ImportMbox => cosmic::task::future(async move {
                let picked = cosmic::dialog::file_chooser::open::Dialog::new()
                    .title(fl!("choose-mbox"))
                    .open_file()
                    .await
                    .ok()
                    .and_then(|response| response.url().to_file_path().ok());
                Message::MboxPicked(picked)
            }),
            Message::MboxPicked(path) => {
                let (Some(path), Some(connection), Some(folder)) = (
                    path,
                    self.connection.clone(),
                    self.current_folder().cloned(),
                ) else {
                    return Task::none();
                };
                self.say(fl!("importing"));
                cosmic::task::future(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        mail::import_mbox(&connection, &folder, &path)
                    })
                    .await
                    .unwrap_or_else(|why| Err(why.to_string()));
                    Message::MboxImported(result)
                })
            }
            Message::MboxImported(result) => {
                match result {
                    Ok((imported, skipped)) => {
                        self.say(if skipped > 0 {
                            fl!("imported-some", imported = imported, skipped = skipped)
                        } else {
                            fl!("imported", imported = imported)
                        });
                        // The upload is done; the pull is what makes them
                        // appear.
                        return self.sync_now();
                    }
                    Err(why) => self.say(why),
                }
                Task::none()
            }
            Message::ExportMessage => {
                let connection = self.connection.clone();
                let (Some(connection), Some((folder, uid))) = (
                    connection,
                    self.target()
                        .map(|target| (target.folder.clone(), target.uid)),
                ) else {
                    return Task::none();
                };
                cosmic::task::future(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        mail::export_message(&connection, &folder, uid)
                    })
                    .await
                    .unwrap_or_else(|why| Err(why.to_string()));
                    Message::MessageExported(result)
                })
            }
            Message::MessageExported(result) => {
                self.say(match result {
                    Ok(path) => fl!("attachment-saved", path = path.display().to_string()),
                    Err(why) => fl!("attachment-not-saved", reason = why),
                });
                Task::none()
            }
            Message::Unsubscribe => self.unsubscribe(),
            Message::Unsubscribed(result) => {
                self.say(match result {
                    Ok(()) => fl!("unsubscribed"),
                    Err(why) => fl!("unsubscribe-failed", reason = why),
                });
                Task::none()
            }
            Message::PgpKeyImport(index) => {
                let connection = self.connection.clone();
                let (Some(connection), Some((folder, uid))) = (
                    connection,
                    self.target()
                        .map(|target| (target.folder.clone(), target.uid)),
                ) else {
                    return Task::none();
                };
                cosmic::task::future(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        mail::import_pgp_key(&connection, &folder, uid, index)
                    })
                    .await
                    .unwrap_or_else(|why| Err(why.to_string()));
                    Message::PgpKeyImported(Box::new(result))
                })
            }
            Message::PgpKeyImported(result) => {
                match *result {
                    Ok(address) => {
                        self.say(fl!("pgp-key-imported", address = address));
                        // The verdict may have just changed from "unknown
                        // signer" to "verified"; reopen so the reader says so.
                        return self.open_selected();
                    }
                    Err(why) => self.say(why),
                }
                Task::none()
            }
            Message::SaveAttachment(index) => self.save_attachment(index),
            Message::AttachmentSaved(result) => {
                self.say(match result {
                    // Where it *landed*: the name is sanitised and a collision
                    // gets a counter, so naming the folder would be unhelpful
                    // if the file is actually `report (3).pdf`.
                    Ok(path) => fl!("attachment-saved", path = path.display().to_string()),
                    Err(why) => fl!("attachment-not-saved", reason = why),
                });
                Task::none()
            }
            Message::AttachFile => cosmic::task::future(async move {
                let dialog =
                    cosmic::dialog::file_chooser::open::Dialog::new().title(fl!("choose-files"));
                match dialog.open_files().await {
                    Ok(response) => Message::FilePicked(Ok(response
                        .urls()
                        .iter()
                        .filter_map(|url| url.to_file_path().ok())
                        .collect())),
                    // Cancelling is not a failure and must not be reported as
                    // one; anything else is, because a portal that is missing
                    // or refused looks identical to "nothing happened"
                    // otherwise, and the user presses the button again.
                    Err(cosmic::dialog::file_chooser::Error::Cancelled) => {
                        Message::FilePicked(Ok(Vec::new()))
                    }
                    Err(why) => Message::FilePicked(Err(why.to_string())),
                }
            }),
            Message::FilePicked(paths) => {
                let paths = match paths {
                    Ok(paths) => paths,
                    Err(why) => {
                        return self.with_composer(|composer| {
                            composer.error = Some(fl!("no-file-dialog", reason = why));
                        });
                    }
                };
                for path in paths {
                    match cosmic_pim_mail::compose::Attachment::from_path(&path) {
                        Ok(attachment) => {
                            if let Some(composer) = self.composer_mut() {
                                composer.draft.attachments.push(attachment);
                            }
                        }
                        Err(why) => {
                            if let Some(composer) = self.composer_mut() {
                                composer.error = Some(why.to_string());
                            }
                        }
                    }
                }
                Task::none()
            }
            Message::AttachmentRemoved(index) => self.with_composer(|composer| {
                if index < composer.draft.attachments.len() {
                    composer.draft.attachments.remove(index);
                }
            }),

            Message::DraftsSwept(result) => {
                // A quiet sweep is the ordinary case and says nothing. What
                // must not be quiet: a draft that is not reaching the server,
                // because "kept on this device" is exactly what mirroring
                // promises is no longer true.
                match *result {
                    Ok(Some(report)) if !report.failed.is_empty() => {
                        let (_, why) = report.failed[0].clone();
                        self.say(fl!("draft-sync-failed", reason = why));
                    }
                    Ok(_) => {}
                    Err(why) => self.say(fl!("draft-sync-failed", reason = why)),
                }
                Task::none()
            }

            Message::ServerDraftOpened(result) => {
                match *result {
                    Ok((id, draft)) => {
                        let mut composer = Composer::new(draft, None);
                        // Carried, so a save here replaces the server copy
                        // rather than leaving a second one beside it.
                        composer.draft_id = Some(id);
                        self.reader_error = None;
                        return self.open_composer(composer);
                    }
                    Err(why) => self.reader_error = Some(why),
                }
                Task::none()
            }

            Message::RuleToggled(index, on) => {
                if let Some(rule) = self.rules.get_mut(index) {
                    rule.enabled = on;
                    self.save_rules_now();
                }
                Task::none()
            }
            Message::RuleDeleted(index) => {
                let Some(rule) = self.rules.get(index) else {
                    return Task::none();
                };
                let name = rule.name.clone();
                self.update(Message::FolderDialogOpened(FolderDialog::DeleteRule {
                    index,
                    name,
                }))
            }
            Message::RuleFormNameChanged(name) => {
                self.rule_form.name = name;
                Task::none()
            }
            Message::RuleFormFieldSelected(index) => {
                self.rule_form.field = index;
                Task::none()
            }
            Message::RuleFormContainsChanged(text) => {
                self.rule_form.contains = text;
                Task::none()
            }
            Message::RuleFormMarkRead(on) => {
                self.rule_form.mark_read = on;
                Task::none()
            }
            Message::RuleFormStar(on) => {
                self.rule_form.star = on;
                Task::none()
            }
            Message::RuleFormDelete(on) => {
                self.rule_form.delete = on;
                Task::none()
            }
            Message::RuleFormMoveSelected(index) => {
                self.rule_form.move_to = index;
                Task::none()
            }
            Message::RuleFormSubmitted => {
                use cosmic_pim_mail::rules::{Actions, Condition, Rule};
                let contains = self.rule_form.contains.trim().to_owned();
                if contains.is_empty() {
                    return Task::none();
                }
                let field = crate::ui::rules::FIELDS
                    .get(self.rule_form.field)
                    .copied()
                    .unwrap_or(cosmic_pim_mail::rules::Field::Sender);
                // Index 0 of the dropdown is "no move"; the wires start at 1.
                let move_to = self
                    .rule_form
                    .move_to
                    .checked_sub(1)
                    .and_then(|index| self.rule_move_wires.get(index).cloned());
                let name = if self.rule_form.name.trim().is_empty() {
                    // A rule needs a name for its row; the pattern is the
                    // honest default.
                    contains.clone()
                } else {
                    self.rule_form.name.trim().to_owned()
                };
                self.rules.push(Rule {
                    name,
                    conditions: vec![Condition { field, contains }],
                    actions: Actions {
                        move_to,
                        mark_read: self.rule_form.mark_read,
                        star: self.rule_form.star,
                        delete: self.rule_form.delete,
                    },
                    ..Rule::default()
                });
                self.rule_form = RuleForm::default();
                self.save_rules_now();
                Task::none()
            }
            Message::RulesApplied(result) => {
                match *result {
                    Ok(Some(report)) => {
                        // A delivery failure outranks "rules filed n": one is
                        // news about the user's own words not arriving.
                        if let Some((recipient, _)) = report.bounces.first() {
                            self.status =
                                Some(fl!("bounce-arrived", recipient = recipient.clone()));
                        } else if let Some((rule, why)) = report.failures.first() {
                            self.say(fl!(
                                "rule-failed",
                                rule = rule.clone(),
                                reason = why.clone()
                            ));
                        } else if report.matched > 0 {
                            self.say(fl!("rules-applied", count = report.matched));
                        }
                        if report.matched > 0 {
                            // Rules moved or reflagged messages after the
                            // post-sync reload; show the result, not the
                            // moment before it.
                            return Task::batch([
                                self.reload_conversations(),
                                if self.showing_unified {
                                    self.reload_unified()
                                } else {
                                    Task::none()
                                },
                            ]);
                        }
                    }
                    Ok(None) => {}
                    Err(why) => self.say(why),
                }
                Task::none()
            }

            Message::LabelRowsLoaded(result) => {
                match *result {
                    Ok(names) => {
                        self.known_labels = names;
                        self.refresh_label_rows();
                    }
                    Err(why) => self.say(why),
                }
                Task::none()
            }
            Message::LabelQueryChanged(query) => {
                if let Some(FolderDialog::Label {
                    query: field,
                    selected,
                }) = self.folder_dialog.as_mut()
                {
                    *field = query;
                    *selected = 0;
                }
                self.refresh_label_rows();
                Task::none()
            }
            Message::LabelToggled(name, on) => {
                // Optimistically flip the row so the picker answers the
                // click; the reload that follows makes it true.
                if let Some(row) = self
                    .label_rows
                    .iter_mut()
                    .find(|(existing, _)| existing.eq_ignore_ascii_case(&name))
                {
                    row.1 = on;
                } else if on {
                    self.label_rows.push((name.clone(), true));
                }
                if !self
                    .known_labels
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(&name))
                {
                    self.known_labels.push(name.clone());
                }
                self.set_label_on_selection(name, on)
            }
            Message::SnoozePicked(preset) => {
                self.folder_dialog = None;
                self.snooze_selected(preset.until_ms())
            }
            Message::SendLater => {
                if self.composer().is_none() {
                    return Task::none();
                }
                self.update(Message::FolderDialogOpened(FolderDialog::SendLater))
            }
            Message::SendLaterPicked(preset) => {
                self.folder_dialog = None;
                self.schedule_send_at(preset.until_ms(), false)
            }
            Message::SendScheduled(result) => match *result {
                Ok((id, not_before_ms, grace)) => {
                    // Queued and durable, so the window has nothing left to
                    // hold — the same rule an immediate send follows.
                    let closed = self.discard_composer();
                    self.say(if grace {
                        fl!(
                            "send-scheduled-grace",
                            seconds = i64::from(self.config.send_delay())
                        )
                    } else {
                        fl!(
                            "send-scheduled-later",
                            when = crate::ui::relative_date_ms(not_before_ms)
                        )
                    });
                    self.undo_stack.push(UndoEntry {
                        description: fl!("undo-send-desc"),
                        reverse: Reverse::CancelSend { id },
                    });
                    if self.undo_stack.len() > UNDO_DEPTH {
                        self.undo_stack.remove(0);
                    }
                    // A timer for sends going soon; anything further out is
                    // the poll's job, and an in-process timer for tomorrow
                    // would not survive the app closing tonight anyway.
                    let wait_ms = not_before_ms - chrono::Utc::now().timestamp_millis();
                    let timer = if (0..5 * 60_000).contains(&wait_ms) {
                        cosmic::task::future(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                u64::try_from(wait_ms).unwrap_or(0) + 1_000,
                            ))
                            .await;
                            Message::SyncNow
                        })
                    } else {
                        Task::none()
                    };
                    Task::batch([closed, self.reload_outbox(), self.reload_drafts(), timer])
                }
                Err(why) => {
                    if let Some(composer) = self.composer_mut() {
                        composer.sending = false;
                        composer.error = Some(why);
                    } else {
                        self.say(why);
                    }
                    Task::none()
                }
            },
            Message::SendCancelled(result) => {
                match *result {
                    Ok(Some(draft)) => {
                        // The words the user wrote, back where they can be
                        // edited — the entire point of the grace.
                        self.say(fl!("send-taken-back"));
                        return self.open_composer(Composer::new(draft, None));
                    }
                    Ok(None) => self.say(fl!("send-already-gone")),
                    Err(why) => self.say(why),
                }
                self.reload_outbox()
            }
            Message::SendDelayChanged(text) => {
                if let Ok(seconds) = text.trim().parse::<u32>() {
                    self.remember(|config| config.send_delay_seconds = seconds);
                }
                self.send_delay = text;
                Task::none()
            }
            Message::SnoozeWoken(result) => match *result {
                Ok(0) => Task::none(),
                Ok(count) => {
                    self.say(fl!("snoozed-back", count = count));
                    // The messages are back in INBOX on the server; the next
                    // pull files them locally. Ask for one now rather than
                    // waiting out the poll.
                    self.sync_now()
                }
                Err(why) => {
                    self.say(fl!("snooze-failed", reason = why));
                    Task::none()
                }
            },

            Message::FolderDialogOpened(dialog) => {
                // One overlay at a time; a picker under a palette is neither.
                self.palette = None;
                self.palette_rows.clear();
                self.folder_dialog = Some(dialog);
                self.refresh_move_rows();
                cosmic::widget::text_input::focus(crate::ui::FOLDER_NAME_ID.clone())
            }
            Message::FolderNameChanged(name) => {
                if let Some(
                    FolderDialog::Create { name: field } | FolderDialog::Rename { name: field },
                ) = self.folder_dialog.as_mut()
                {
                    *field = name;
                }
                Task::none()
            }
            Message::MoveQueryChanged(query) => {
                if let Some(FolderDialog::Move {
                    query: field,
                    selected,
                }) = self.folder_dialog.as_mut()
                {
                    *field = query;
                    // The list under the cursor just changed.
                    *selected = 0;
                }
                self.refresh_move_rows();
                Task::none()
            }
            Message::MovePicked(index) => {
                self.folder_dialog = None;
                self.move_rows.clear();
                match self.folders.get(index).cloned() {
                    Some(destination) => self.move_conversation_to(destination),
                    None => Task::none(),
                }
            }
            Message::FolderDialogCancelled => {
                self.folder_dialog = None;
                self.move_rows.clear();
                Task::none()
            }
            Message::FolderDialogConfirmed => self.confirm_folder_dialog(),
            Message::FolderOpFinished(result) => match *result {
                Ok(status) => {
                    self.say(status);
                    // The folder list is the server's; the sync is what makes
                    // the change visible.
                    self.sync_now()
                }
                Err(why) => {
                    self.say(why);
                    Task::none()
                }
            },

            Message::DraftOpened(id) => self.open_draft(&id),
            Message::DraftDeleted(id) => {
                if let Some(connection) = self.connection.as_ref()
                    && let Err(why) = mail::delete_draft(connection, &id)
                {
                    self.say(why);
                }
                Task::batch([self.reload_drafts(), self.sweep_drafts_now()])
            }
            Message::ComposeSend => self.send_draft(),
            Message::ComposeSent(sent) => self.composer_finished(*sent),
        }
    }

    fn account(&self) -> Option<&Account> {
        let id = self.selected_account.as_deref()?;
        self.accounts.iter().find(|a| a.id == id)
    }

    fn current_folder(&self) -> Option<&Folder> {
        self.folders.get(self.selected_folder?)
    }

    /// The folder to open when there is no reason to prefer another.
    fn default_folder(&self) -> Option<usize> {
        self.folders
            .iter()
            .position(|f| f.special_use == Some(SpecialUse::Inbox))
            .or(if self.folders.is_empty() {
                None
            } else {
                Some(0)
            })
    }

    fn rebuild_connection(&mut self) {
        self.connection = None;
        self.status = None;
        let Some(account) = self.account().cloned() else {
            return;
        };
        let store = match AccountStore::open_default() {
            Ok(store) => store,
            Err(why) => {
                self.say(why.to_string());
                return;
            }
        };
        match Connection::for_account(&store, &account) {
            Ok(Some(connection)) => {
                self.identity_labels = connection
                    .identities
                    .iter()
                    .map(|identity| identity.display().to_owned())
                    .collect();
                self.connection = Some(connection);
            }
            Ok(None) => self.say(fl!("no-mail-account")),
            Err(why) => self.say(why),
        }
    }

    fn clear_mailbox_state(&mut self) {
        self.folders.clear();
        self.selected_folder = None;
        self.conversations.clear();
        self.selected_conversation = None;
        self.opened = None;
        self.unread.clear();
        self.list_error = None;
        self.reader_error = None;
    }

    /// Reads the folders that already have a maildir on disk.
    ///
    /// A maildir is not enough on its own to reconstruct a folder's wire name
    /// and hierarchy, so this is only ever a starting point — the authoritative
    /// list arrives with the next sync. It exists so the window is not empty
    /// while a server is being waited on.
    fn load_cached_folders(&mut self) -> Task<Message> {
        let Some(connection) = self.connection.clone() else {
            return Task::none();
        };
        let root = connection.root.join(&connection.account_id);
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Task::none();
        };

        let mut folders: Vec<Folder> = entries
            .flatten()
            .filter(|entry| entry.path().join("cur").is_dir())
            .filter_map(|entry| {
                let local = entry.file_name().to_string_lossy().into_owned();
                let wire = unescape_local_name(&local)?;
                Some(cosmic_pim_mail::folder::from_list_entry(
                    &wire,
                    Some('/'),
                    &[],
                ))
            })
            .collect();
        cosmic_pim_mail::folder::sort_for_display(&mut folders);

        self.folders = folders;
        self.selected_folder = self.restore_folder();
        Task::batch([self.reload_conversations(), self.update_title()])
    }

    fn reload_conversations(&mut self) -> Task<Message> {
        let (Some(connection), Some(folder)) =
            (self.connection.clone(), self.current_folder().cloned())
        else {
            self.conversations.clear();
            return Task::none();
        };
        self.loading_conversations = true;
        cosmic::task::future(async move {
            let result =
                tokio::task::spawn_blocking(move || mail::conversations(&connection, &folder))
                    .await
                    .unwrap_or_else(|why| Err(why.to_string()));
            Message::ConversationsLoaded(result)
        })
    }

    fn open_selected(&mut self) -> Task<Message> {
        let (Some(connection), Some(folder), Some(uid)) = (
            self.connection.clone(),
            self.current_folder().cloned(),
            self.selected_uid(),
        ) else {
            return Task::none();
        };
        // A message in the Drafts folder is unfinished writing, and opening it
        // means resuming it — in the composer, not the reader. This is also
        // how a draft written on another device becomes editable here.
        if folder.special_use == Some(SpecialUse::Drafts) {
            return cosmic::task::future(async move {
                let result = tokio::task::spawn_blocking(move || {
                    mail::edit_server_draft(&connection, &folder, uid)
                })
                .await
                .unwrap_or_else(|why| Err(why.to_string()));
                Message::ServerDraftOpened(Box::new(result))
            });
        }
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || mail::open(&connection, &folder, uid))
                .await
                .unwrap_or_else(|why| Err(why.to_string()));
            Message::MessageOpened(Box::new(result))
        })
    }

    /// Mirrors draft edits and discards to the server, when there are any.
    ///
    /// Cheap to call optimistically: with nothing dirty and no tombstones the
    /// worker returns without opening a connection.
    fn sweep_drafts_now(&self) -> Task<Message> {
        let Some(connection) = self.connection.clone() else {
            return Task::none();
        };
        let folders = self.folders.clone();
        cosmic::task::future(async move {
            let result =
                tokio::task::spawn_blocking(move || mail::sweep_drafts(&connection, &folders))
                    .await
                    .unwrap_or_else(|why| Err(why.to_string()));
            Message::DraftsSwept(Box::new(result))
        })
    }

    /// The message the reader is showing: the newest in the selected
    /// conversation.
    fn selected_uid(&self) -> Option<u32> {
        self.conversations
            .get(self.selected_conversation?)?
            .newest_uid()
    }

    fn sync_now(&mut self) -> Task<Message> {
        let Some(connection) = self.connection.clone() else {
            self.say(fl!("no-mail-account"));
            return Task::none();
        };
        if self.syncing {
            return Task::none();
        }
        self.syncing = true;
        self.status = None;
        self.cycle += 1;
        let cycle = self.cycle;

        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || mail::sync(&connection, cycle))
                .await
                .unwrap_or_else(|why| Err(why.to_string()));
            Message::SyncFinished(Box::new(result))
        })
    }

    /// Applies a flag change to the message the action was about.
    fn set_flags(&mut self, edit: impl Fn(Flags) -> Flags + Send + 'static) -> Task<Message> {
        let connection = self.connection.clone();
        let Some((folder, uid)) = self
            .target()
            .map(|target| (target.folder.clone(), target.uid))
        else {
            return Task::none();
        };
        let Some(connection) = connection else {
            return Task::none();
        };

        // Reflected immediately so the button does not sit in the old state
        // waiting for a disk write; the reload that follows is what makes it
        // true rather than merely displayed.
        if let Some(opened) = self.opened_mut() {
            opened.flags = edit(opened.flags);
        }

        let description = fl!("undo-flags");
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                mail::set_flags(&connection, &folder, &[uid], edit).map(|previous| {
                    (!previous.is_empty()).then_some(UndoEntry {
                        description,
                        reverse: Reverse::Flags { folder, previous },
                    })
                })
            })
            .await
            .unwrap_or_else(|why| Err(why.to_string()));
            Message::Mutated(result)
        })
    }

    /// Moves the whole selected conversation to a special-use folder.
    ///
    /// The whole conversation, not the one message the reader is showing:
    /// "archive" means the exchange is finished with, and leaving four of its
    /// six messages in the inbox is not what anybody meant.
    fn move_selected(&mut self, role: SpecialUse) -> Task<Message> {
        let Some(destination) = mail::special(&self.folders, role).cloned() else {
            self.say(fl!("no-archive-folder"));
            return Task::none();
        };
        self.move_conversation_to(destination)
    }

    /// Snoozes the selected conversation until `until_ms`, with undo — a
    /// snooze is a move underneath, and regrettable in the same window.
    fn snooze_selected(&mut self, until_ms: i64) -> Task<Message> {
        let (Some(connection), Some(folder), Some(index)) = (
            self.connection.clone(),
            self.current_folder().cloned(),
            self.selected_conversation,
        ) else {
            return Task::none();
        };
        let Some(uids) = self.conversations.get(index).map(|c| c.uids.clone()) else {
            return Task::none();
        };
        self.opened = None;
        self.selected_conversation = None;

        let description = fl!("undo-snooze");
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                mail::snooze(&connection, &folder, &uids, until_ms).map(|messages| {
                    (!messages.is_empty()).then_some(UndoEntry {
                        description,
                        reverse: Reverse::Unmove { folder, messages },
                    })
                })
            })
            .await
            .unwrap_or_else(|why| Err(why.to_string()));
            Message::Mutated(result)
        })
    }

    /// Wakes due snoozes, on the worker — skipping the connection entirely
    /// when the schedule has nothing due, which is the two-minute common case.
    fn wake_snoozed_now(&self) -> Task<Message> {
        let Some(connection) = self.connection.clone() else {
            return Task::none();
        };
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                let now = chrono::Utc::now().timestamp_millis();
                if !mail::has_due_snoozes(&connection, now) {
                    return Ok(0);
                }
                mail::wake_snoozed(&connection, now)
            })
            .await
            .unwrap_or_else(|why| Err(why.to_string()));
            Message::SnoozeWoken(Box::new(result))
        })
    }

    /// Moves the selected conversation to `destination`, with undo.
    fn move_conversation_to(&mut self, destination: Folder) -> Task<Message> {
        let connection = self.connection.clone();
        let Some((folder, uids)) = self
            .target()
            .map(|target| (target.folder.clone(), target.uids.to_vec()))
        else {
            return Task::none();
        };
        let Some(connection) = connection else {
            return Task::none();
        };
        if destination.wire_name == folder.wire_name {
            return Task::none();
        }

        // Filed, so whatever was showing it has nothing left to show: the
        // window if it came from one, the reading pane otherwise.
        let emptied = match self.detached_read() {
            Some(id) => self.drop_window(id),
            None => {
                self.opened = None;
                self.selected_conversation = None;
                Task::none()
            }
        };

        let description = fl!("undo-move", folder = destination.display_name.clone());
        let moved = cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                mail::move_to(&connection, &folder, &destination, &uids).map(|messages| {
                    (!messages.is_empty()).then_some(UndoEntry {
                        description,
                        reverse: Reverse::Unmove { folder, messages },
                    })
                })
            })
            .await
            .unwrap_or_else(|why| Err(why.to_string()));
            Message::Mutated(result)
        });
        Task::batch([emptied, moved])
    }

    /// Acts on what a launch asked for, whether it started this process or
    /// arrived over D-Bus at one already running.
    fn launch(&mut self, launch: Option<&crate::flags::Launch>) -> Task<Message> {
        match launch {
            Some(crate::flags::Launch::Mailto(url)) => {
                let url = url.clone();
                self.open_mailto(&url)
            }
            Some(crate::flags::Launch::Compose) => self.act(Action::Compose),
            None => Task::none(),
        }
    }

    /// The composer this message is about.
    ///
    /// Which is to say: the one belonging to the window the message came from.
    /// A composer only exists in a window of its own, so a message that
    /// arrived without a window — a menu entry pressed in the main window, a
    /// task nobody routed — is not about a composer at all, and says so.
    fn composer(&self) -> Option<&Composer> {
        match self.windows.get(&self.routed?) {
            Some(Detached::Compose(composer)) => Some(composer.as_ref()),
            _ => None,
        }
    }

    fn composer_mut(&mut self) -> Option<&mut Composer> {
        match self.windows.get_mut(&self.routed?) {
            Some(Detached::Compose(composer)) => Some(composer.as_mut()),
            _ => None,
        }
    }

    /// Is there an address to send from?
    ///
    /// The reply buttons are drawn either way, greyed rather than hidden: a
    /// missing button reads as a missing feature.
    fn can_send(&self) -> bool {
        self.connection
            .as_ref()
            .is_some_and(|connection| connection.submission.is_some())
    }

    /// Which of the account's identities a composer is writing as, as an index
    /// into the dropdown's labels.
    fn identity_index(&self, composer: &Composer) -> usize {
        self.connection
            .as_ref()
            .and_then(|connection| {
                connection.identities.iter().position(|identity| {
                    identity
                        .address
                        .eq_ignore_ascii_case(&composer.draft.from.address)
                })
            })
            .unwrap_or(0)
    }

    /// Which window an action is being invoked from.
    fn surface(&self) -> actions::Surface {
        match self.routed.and_then(|id| self.windows.get(&id)) {
            Some(Detached::Compose(_)) => actions::Surface::Writing,
            Some(Detached::Read(_)) => actions::Surface::Reading,
            None => actions::Surface::Main,
        }
    }

    /// Which window a keystroke the framework swallowed was aimed at.
    ///
    /// Every message this application routes carries its window, so this is
    /// only for the hooks that do not: `on_escape` is a key libcosmic matched
    /// before this saw it, and the window it was meant for is the focused one.
    /// Deliberately not consulted by anything that arrives already routed —
    /// focus is a second answer to a question that already has one, and two
    /// answers is how they come to disagree.
    fn focused_detached(&self) -> Option<window::Id> {
        let id = self.core.focused_window()?;
        self.windows.contains_key(&id).then_some(id)
    }

    /// What a reader action is acting on: the message in the window the action
    /// came from, or the one in the reading pane.
    fn target(&self) -> Option<Target<'_>> {
        if let Some(id) = self.routed
            && let Some(Detached::Read(reading)) = self.windows.get(&id)
        {
            return Some(Target {
                folder: &reading.folder,
                uids: &reading.uids,
                uid: reading.opened.uid,
                opened: Some(&reading.opened),
            });
        }
        let conversation = self.conversations.get(self.selected_conversation?)?;
        Some(Target {
            folder: self.current_folder()?,
            uids: &conversation.uids,
            uid: conversation.newest_uid()?,
            opened: self.opened.as_ref(),
        })
    }

    /// The window this message came from, when it is one showing a message.
    fn detached_read(&self) -> Option<window::Id> {
        let id = self.routed?;
        matches!(self.windows.get(&id), Some(Detached::Read(_))).then_some(id)
    }

    /// The open message, wherever it is open, for the changes that are
    /// reflected before the disk agrees.
    fn opened_mut(&mut self) -> Option<&mut Opened> {
        if let Some(id) = self.routed
            && let Some(Detached::Read(reading)) = self.windows.get_mut(&id)
        {
            return Some(&mut reading.opened);
        }
        self.opened.as_mut()
    }

    /// The quoted-run state of whichever reader a message came from.
    fn expanded_quotes_mut(&mut self) -> &mut std::collections::HashSet<usize> {
        if let Some(id) = self.routed
            && let Some(Detached::Read(reading)) = self.windows.get_mut(&id)
        {
            return &mut reading.expanded_quotes;
        }
        &mut self.expanded_quotes
    }

    /// Opens a window, with the chrome a window of our own drawing needs.
    ///
    /// Undecorated on purpose: `view_window` draws a COSMIC header bar into
    /// it, the same one the main window gets from the framework's template.
    /// The template itself is not available here — libcosmic applies it to the
    /// main window only — so the window's frame is this application's job.
    fn open_window(&mut self, detached: Detached, title: String) -> Task<Message> {
        let size = match &detached {
            // Wide enough for a quoted reply not to wrap at every line, tall
            // enough to see the fields and the first paragraph at once.
            Detached::Compose(_) => cosmic::iced::Size::new(760.0, 640.0),
            Detached::Read(_) => cosmic::iced::Size::new(820.0, 720.0),
        };
        let mut settings = window::Settings {
            size,
            min_size: Some(cosmic::iced::Size::new(400.0, 320.0)),
            resizable: true,
            resize_border: 8,
            decorations: false,
            transparent: true,
            // The close goes through `WindowCloseRequested` first, because a
            // composer has a draft to save before its window is allowed to
            // stop existing.
            exit_on_close_request: false,
            ..Default::default()
        };
        // The same app id the main window gets from the framework. Without it
        // a composer is a second application to the desktop: its own dock
        // entry, a fallback icon, and no grouping with the mail it came from.
        #[cfg(target_os = "linux")]
        {
            settings.platform_specific.application_id = Self::APP_ID.to_string();
        }
        let (id, opening) = window::open(settings);
        self.windows.insert(id, detached);
        Task::batch([
            opening.map(|id| cosmic::Action::App(Message::WindowOpened(id))),
            self.set_window_title(title, id),
        ])
    }

    /// Opens a composer in a window of its own.
    fn open_composer(&mut self, composer: Composer) -> Task<Message> {
        let title = compose_title(&composer.draft.subject);
        self.open_window(Detached::Compose(Box::new(composer)), title)
    }

    /// Puts the open message in a window of its own, and gives the reading
    /// pane back to the list.
    ///
    /// Moves rather than copies: the same message in two places is two places
    /// to press Archive, and the point of detaching is that the list can move
    /// on while this one stays put.
    fn detach_opened(&mut self) -> Task<Message> {
        // Already in a window of its own. Asking again means the user wants to
        // see it, so raise it rather than opening a second copy.
        if self.routed.is_some() {
            // Already in a window of its own, or in a composer, which has no
            // message to detach. Asking again means the user wants to see it,
            // so raise it rather than opening a second copy of the same
            // message.
            return match self.detached_read() {
                Some(id) => window::gain_focus(id),
                None => Task::none(),
            };
        }
        let (Some(folder), Some(index)) =
            (self.current_folder().cloned(), self.selected_conversation)
        else {
            return Task::none();
        };
        let Some(uids) = self.conversations.get(index).map(|c| c.uids.clone()) else {
            return Task::none();
        };
        let Some(opened) = self.opened.take() else {
            return Task::none();
        };
        let expanded_quotes = std::mem::take(&mut self.expanded_quotes);
        let title = read_title(&opened.message.subject);
        self.open_window(
            Detached::Read(Box::new(Reading {
                folder,
                uids,
                opened,
                expanded_quotes,
            })),
            title,
        )
    }

    /// A window is closing: keep whatever it was holding, then let it go.
    ///
    /// The draft save is the reason this exists. A composer closed by its own
    /// header bar and one closed by the compositor have to lose the same
    /// amount of writing, which is none.
    fn close_window(&mut self, id: window::Id) -> Task<Message> {
        let previous = self.routed.replace(id);
        let saved = self.save_composer(true);
        self.routed = previous;
        Task::batch([saved, self.drop_window(id)])
    }

    /// Saves every open composer, for the moment the process is about to stop
    /// existing and there is no later.
    ///
    /// Synchronous, like the single-composer path it replaces: a `Task`
    /// returned at exit races the exit and loses.
    fn save_open_drafts(&mut self) {
        let Some(connection) = self.connection.clone() else {
            return;
        };
        for detached in self.windows.values_mut() {
            let Detached::Compose(composer) = detached else {
                continue;
            };
            if !composer.is_worth_saving() {
                continue;
            }
            // The id is kept, not discarded, and that is the whole
            // correctness of this loop. Quitting through the application
            // rather than the window runs `on_app_exit` *and then* closes the
            // main window, which arrives back here as `WindowsClosing` — so
            // this saves twice. A composer that has never been saved carries
            // no id, `save_draft` mints one from the clock, and a second pass
            // a few milliseconds later would mint a different one: two draft
            // files for one half-written message, and the mirror would then
            // put both on the server. Writing the id back makes the second
            // pass a replacement, which is what `Drafts::save` is built for.
            match mail::save_draft(
                &connection,
                composer.draft_id.as_deref(),
                &composer.resolved(),
            ) {
                Ok(id) => composer.draft_id = Some(id),
                Err(why) => tracing::error!(%why, "a draft was lost on exit"),
            }
        }
    }

    /// Opens a composer on a `mailto:` link, in a window of its own.
    fn open_mailto(&mut self, url: &str) -> Task<Message> {
        let Some(identity) = self
            .connection
            .as_ref()
            .and_then(|c| c.submission.as_ref())
            .map(|s| s.identity.clone())
        else {
            self.say(fl!("no-from-address"));
            self.context_page = ContextPage::Accounts;
            self.core.window.show_context = true;
            return Task::none();
        };
        if let Some(draft) = crate::mailto::prefill(url, identity) {
            return self.open_composer(Composer::new(draft, None));
        }
        Task::none()
    }

    /// Opens the composer with a draft built from the current state.
    fn compose(
        &mut self,
        match_recipient: bool,
        build: impl FnOnce(Option<&Opened>, cosmic_pim_mail::Mailbox) -> cosmic_pim_mail::Draft,
    ) -> Task<Message> {
        let Some(mut identity) = self
            .connection
            .as_ref()
            .and_then(|c| c.submission.as_ref())
            .map(|s| s.identity.clone())
        else {
            // Not a silent no-op: without a From address there is nothing to
            // send as, and the user needs to be told where to fix it.
            self.say(fl!("no-from-address"));
            self.context_page = ContextPage::Accounts;
            self.core.window.show_context = true;
            return Task::none();
        };

        // Whichever reader asked — the pane, or the window the Reply button
        // was pressed in. A reply opened from a detached message answers that
        // message, however far the list has moved on since.
        let target = self.target();
        let opened = target.as_ref().and_then(|target| target.opened);

        // A reply goes out as the identity the original was addressed to —
        // answering mail sent to an alias from the primary address outs the
        // alias. A fresh compose stays on the primary, whatever is open.
        if match_recipient
            && let (Some(connection), Some(opened)) = (self.connection.as_ref(), opened)
            && let Some(matched) =
                opened
                    .message
                    .to
                    .iter()
                    .chain(&opened.message.cc)
                    .find_map(|recipient| {
                        connection
                            .identities
                            .iter()
                            .find(|m| m.address.eq_ignore_ascii_case(&recipient.address))
                    })
        {
            identity = matched.clone();
        }

        // A reply marks the message it answers — but only once it is actually
        // away, so a cancelled reply leaves no trace.
        let answering = target
            .as_ref()
            .and_then(|target| Some((target.folder.clone(), target.opened?.uid)));

        let draft = build(opened, identity);
        self.open_composer(Composer::new(draft, answering))
    }

    /// Keeps what the current window's composer was holding, unless told not
    /// to.
    ///
    /// Keeping is the default because the cost of the two mistakes is not
    /// symmetric: a stray draft is a line in a list, and a discarded one is
    /// gone. Discarding is a separate button that says what it does.
    ///
    /// The window itself is somebody else's business — this is called both by
    /// the composer's own buttons and by a window closing under it, and only
    /// one of those has a window left to close afterwards.
    fn save_composer(&mut self, keep: bool) -> Task<Message> {
        let Some(connection) = self.connection.clone() else {
            return Task::none();
        };
        let Some(composer) = self.composer() else {
            return Task::none();
        };
        let draft_id = composer.draft_id.clone();
        let resolved = composer.is_worth_saving().then(|| composer.resolved());

        if !keep {
            if let Some(id) = draft_id.as_deref()
                && let Err(why) = mail::delete_draft(&connection, id)
            {
                self.say(why);
            }
            // The discard left a tombstone if the draft was mirrored; the
            // sweep retires the server copy now rather than at the next poll.
            return Task::batch([self.reload_drafts(), self.sweep_drafts_now()]);
        }

        let Some(resolved) = resolved else {
            return Task::none();
        };
        match mail::save_draft(&connection, draft_id.as_deref(), &resolved) {
            Ok(_) => Task::batch([self.reload_drafts(), self.sweep_drafts_now()]),
            Err(why) => {
                // The window is going, so this cannot be shown beside the text
                // it lost. Saying so in a toast is the least bad thing
                // available, and it is why saving is also possible without
                // closing.
                self.say(fl!("draft-not-saved", reason = why));
                Task::none()
            }
        }
    }

    /// The composer's own Save-draft and Discard buttons: deal with the
    /// writing, then close the window it was in.
    fn finish_composing(&mut self, keep: bool) -> Task<Message> {
        let Some(id) = self.routed else {
            return Task::none();
        };
        let saved = self.save_composer(keep);
        Task::batch([saved, self.drop_window(id)])
    }

    /// The message is away: the window has nothing left to hold.
    fn discard_composer(&mut self) -> Task<Message> {
        match self.routed {
            Some(id) => self.drop_window(id),
            None => Task::none(),
        }
    }

    /// Lets go of a window — what it was holding, and the window itself.
    fn drop_window(&mut self, id: window::Id) -> Task<Message> {
        self.windows.remove(&id);
        self.maximized.remove(&id);
        window::close(id)
    }

    /// Leaves the open message's list, by the least ceremonious route it
    /// offers.
    fn unsubscribe(&mut self) -> Task<Message> {
        let Some(route) = self
            .target()
            .and_then(|target| target.opened)
            .and_then(|opened| mail::unsubscribe_route(&opened.message))
        else {
            return Task::none();
        };
        match route {
            mail::Unsubscribe::OneClick(url) => {
                self.say(fl!("unsubscribing"));
                cosmic::task::future(async move {
                    let result =
                        tokio::task::spawn_blocking(move || mail::unsubscribe_one_click(&url))
                            .await
                            .unwrap_or_else(|why| Err(why.to_string()));
                    Message::Unsubscribed(result)
                })
            }
            // Leaving the list is sending a message, and the composer already
            // knows how — the same mailto path a link would take.
            mail::Unsubscribe::Mailto(url) => self.open_mailto(&url),
            mail::Unsubscribe::Browser(url) => {
                if let Err(why) = open::that_detached(&url) {
                    self.say(why.to_string());
                }
                Task::none()
            }
        }
    }

    /// Saves one attachment of the open message.
    fn save_attachment(&mut self, index: usize) -> Task<Message> {
        let connection = self.connection.clone();
        let (Some(connection), Some((folder, uid))) = (
            connection,
            self.target()
                .map(|target| (target.folder.clone(), target.uid)),
        ) else {
            return Task::none();
        };
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                mail::save_attachment(&connection, &folder, uid, index)
            })
            .await
            .unwrap_or_else(|why| Err(why.to_string()));
            Message::AttachmentSaved(result)
        })
    }

    /// Writes the rules as shown back to disk, saying so if it fails.
    fn save_rules_now(&mut self) {
        if let Some(connection) = self.connection.as_ref()
            && let Err(why) = mail::save_rules(connection, &self.rules)
        {
            self.say(why);
        }
    }

    /// Loads the rules and rebuilds the move dropdown before the page shows.
    fn open_rules_page(&mut self) {
        if let Some(connection) = self.connection.as_ref() {
            match mail::load_rules(connection) {
                Ok(rules) => self.rules = rules,
                Err(why) => self.say(why),
            }
        }
        let movable: Vec<&Folder> = self.folders.iter().filter(|f| !f.no_select).collect();
        self.rule_move_labels = std::iter::once(fl!("rule-move-none"))
            .chain(movable.iter().map(|f| f.display_name.clone()))
            .collect();
        self.rule_move_wires = movable.iter().map(|f| f.wire_name.clone()).collect();
    }

    /// Runs the rules over newly arrived mail, on the worker.
    fn apply_rules_now(&self) -> Task<Message> {
        let Some(connection) = self.connection.clone() else {
            return Task::none();
        };
        let folders = self.folders.clone();
        cosmic::task::future(async move {
            let result =
                tokio::task::spawn_blocking(move || mail::apply_rules(&connection, &folders))
                    .await
                    .unwrap_or_else(|why| Err(why.to_string()));
            Message::RulesApplied(Box::new(result))
        })
    }

    /// The selected folder, if renaming or deleting it is a thing that can be
    /// offered — with the reason in the status line when it cannot.
    ///
    /// Special-use folders are refused: INBOX cannot be deleted at all, and a
    /// deleted Trash or Sent breaks every verb that files into it. The server
    /// would refuse some of these anyway; refusing them all here means the
    /// refusal comes with words rather than a server error code.
    fn actionable_folder(&mut self) -> Option<Folder> {
        let Some(folder) = self.current_folder().cloned() else {
            self.say(fl!("no-folder-selected"));
            return None;
        };
        if folder.special_use.is_some() || folder.wire_name.eq_ignore_ascii_case("INBOX") {
            self.say(fl!("folder-is-special", name = folder.display_name));
            return None;
        }
        Some(folder)
    }

    /// Recomputes what the label picker shows for its current query.
    fn refresh_label_rows(&mut self) {
        let Some(FolderDialog::Label { query, .. }) = self.folder_dialog.as_ref() else {
            self.label_rows.clear();
            return;
        };
        let applied = self
            .selected_conversation
            .and_then(|index| self.conversation_labels.get(index))
            .cloned()
            .unwrap_or_default();
        self.label_rows = self
            .known_labels
            .iter()
            .filter(|name| actions::label_matches(query, name))
            .map(|name| {
                let on = applied
                    .iter()
                    .any(|carried| carried.eq_ignore_ascii_case(name));
                (name.clone(), on)
            })
            .collect();
    }

    /// Applies or clears one label on the selected conversation, with undo.
    fn set_label_on_selection(&mut self, name: String, on: bool) -> Task<Message> {
        let (Some(connection), Some(folder), Some(index)) = (
            self.connection.clone(),
            self.current_folder().cloned(),
            self.selected_conversation,
        ) else {
            return Task::none();
        };
        let Some(uids) = self.conversations.get(index).map(|c| c.uids.clone()) else {
            return Task::none();
        };
        let description = if on {
            fl!("undo-labelled", label = name.clone())
        } else {
            fl!("undo-unlabelled", label = name.clone())
        };
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                mail::set_label(&connection, &folder, &uids, &name, on).map(|previous| {
                    (!previous.is_empty()).then_some(UndoEntry {
                        description,
                        reverse: Reverse::Flags { folder, previous },
                    })
                })
            })
            .await
            .unwrap_or_else(|why| Err(why.to_string()));
            Message::Mutated(result)
        })
    }

    /// Recomputes what the move picker shows for its current query.
    fn refresh_move_rows(&mut self) {
        let Some(FolderDialog::Move { query, .. }) = self.folder_dialog.as_ref() else {
            self.move_rows.clear();
            return;
        };
        let current = self.current_folder().map(|f| f.wire_name.clone());
        self.move_rows = self
            .folders
            .iter()
            .enumerate()
            .filter(|(_, folder)| {
                // Not the folder the message is in — moving there is staying —
                // and not a container the server refuses to SELECT.
                !folder.no_select
                    && Some(&folder.wire_name) != current.as_ref()
                    && actions::label_matches(query, &folder.display_name)
            })
            .map(|(index, _)| index)
            .collect();
    }

    /// Runs whichever folder dialog is open, on the worker.
    fn confirm_folder_dialog(&mut self) -> Task<Message> {
        let Some(dialog) = self.folder_dialog.take() else {
            return Task::none();
        };
        // Read before the rows are cleared: the picker's selection is an index
        // into them.
        let picked = if let FolderDialog::Move { selected, .. } = &dialog {
            self.move_rows.get(*selected).copied()
        } else {
            None
        };
        self.move_rows.clear();

        // Handled before the connection guard below: neither of these touches
        // a server, and an account with no working mail endpoint is exactly
        // the one a user is most likely to be removing.
        match &dialog {
            FolderDialog::RemoveAccount { id, .. } => {
                let id = id.clone();
                return self.remove_account(&id);
            }
            FolderDialog::DeleteRule { index, .. } => {
                let index = *index;
                if index < self.rules.len() {
                    self.rules.remove(index);
                    self.save_rules_now();
                }
                return Task::none();
            }
            _ => {}
        }

        let Some(connection) = self.connection.clone() else {
            return Task::none();
        };

        match dialog {
            FolderDialog::Create { name } => {
                let status = fl!("folder-created", name = name.clone());
                cosmic::task::future(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        mail::create_folder(&connection, &name).map(|()| status)
                    })
                    .await
                    .unwrap_or_else(|why| Err(why.to_string()));
                    Message::FolderOpFinished(Box::new(result))
                })
            }
            FolderDialog::Rename { name } => {
                let Some(folder) = self.current_folder().cloned() else {
                    return Task::none();
                };
                let status = fl!("folder-renamed", name = name.clone());
                cosmic::task::future(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        mail::rename_folder(&connection, &folder, &name).map(|()| status)
                    })
                    .await
                    .unwrap_or_else(|why| Err(why.to_string()));
                    Message::FolderOpFinished(Box::new(result))
                })
            }
            FolderDialog::Delete => {
                let Some(folder) = self.current_folder().cloned() else {
                    return Task::none();
                };
                // The view must not keep showing a mailbox that is going away.
                self.selected_folder = None;
                self.conversations.clear();
                self.selected_conversation = None;
                self.opened = None;
                let status = fl!("folder-deleted", name = folder.display_name.clone());
                cosmic::task::future(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        mail::delete_folder(&connection, &folder).map(|()| status)
                    })
                    .await
                    .unwrap_or_else(|why| Err(why.to_string()));
                    Message::FolderOpFinished(Box::new(result))
                })
            }
            FolderDialog::Snooze | FolderDialog::SendLater => Task::none(),
            // Both returned above, before the connection guard — neither needs
            // a server.
            FolderDialog::RemoveAccount { .. } | FolderDialog::DeleteRule { .. } => Task::none(),
            FolderDialog::Label { query, selected } => {
                // Enter toggles the highlighted row; with no row and a typed
                // name, it creates the label and applies it.
                if let Some((name, on)) = self.label_rows.get(selected).cloned() {
                    // Reopen the dialog: labelling is often several labels.
                    self.folder_dialog = Some(FolderDialog::Label { query, selected });
                    self.refresh_label_rows();
                    return self.update(Message::LabelToggled(name, !on));
                }
                let name = query.trim().to_owned();
                if name.is_empty() {
                    return Task::none();
                }
                self.folder_dialog = Some(FolderDialog::Label {
                    query: String::new(),
                    selected: 0,
                });
                self.refresh_label_rows();
                self.update(Message::LabelToggled(name, true))
            }
            FolderDialog::Move { .. } => match picked.and_then(|i| self.folders.get(i)).cloned() {
                Some(destination) => self.move_conversation_to(destination),
                None => Task::none(),
            },
        }
    }

    /// Recomputes what the palette shows for its current query.
    fn refresh_palette_rows(&mut self) {
        let Some((query, _)) = self.palette.as_ref() else {
            self.palette_rows.clear();
            return;
        };
        let has_message = self.opened.is_some();
        self.palette_rows = actions::bindings()
            .into_iter()
            .filter(|binding| {
                // An action that needs a message, offered with none open, is a
                // row that does nothing — hidden rather than greyed, because a
                // palette is searched, not browsed.
                (has_message || !binding.action.needs_a_message())
                    && binding.action != Action::Palette
                    && actions::label_matches(query, &binding.action.label())
            })
            .map(|binding| {
                let shortcut = binding.shortcut();
                (binding.action, shortcut)
            })
            .collect();
    }

    /// Runs a browser sign-in on a worker thread.
    fn sign_in(&mut self, provider_id: &str) -> Task<Message> {
        if self.signing_in {
            return Task::none();
        }
        // The address is the add form's: the sign-in buttons live on it.
        let email = self
            .add_form
            .as_ref()
            .map(|form| form.email.trim().to_owned())
            .unwrap_or_default();
        if !email.contains('@') {
            return self.with_add_form(|form| form.error = Some(fl!("sign-in-needs-address")));
        }
        self.signing_in = true;
        self.say(fl!("sign-in-browser"));
        let provider_id = provider_id.to_owned();

        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || mail::sign_in(&provider_id, &email))
                .await
                .unwrap_or_else(|why| Err(why.to_string()));
            Message::SignInFinished(Box::new(result))
        })
    }

    /// Looks up the account's servers from its address.
    fn discover_settings(&mut self) -> Task<Message> {
        let Some(form) = self.mail_form.as_mut() else {
            return Task::none();
        };
        // The From address if it has one, else the account's username — which
        // for an account Slate created is often the address anyway.
        let email = if form.from_address.trim().is_empty() {
            form.account_username.clone()
        } else {
            form.from_address.trim().to_owned()
        };
        if form.discovering {
            return Task::none();
        }
        // The registry answers without a network and with more authority than
        // the probing stages have; only a miss goes on to them.
        if let Some(endpoint) = mail::provider_settings(&email) {
            form.apply_endpoint(&endpoint);
            return Task::none();
        }
        form.discovering = true;
        form.error = None;

        cosmic::task::future(async move {
            let found = tokio::task::spawn_blocking(move || mail::discover(&email))
                .await
                .unwrap_or_else(|why| Err(why.to_string()));
            Message::MailFormDiscovered(Box::new(found))
        })
    }

    /// Writes a setting through `cosmic-config`.
    ///
    /// Best-effort: a setting that could not be saved is worth a log line and
    /// nothing more. Refusing to change the selection because the disk is full
    /// would be making a small problem into the user's problem.
    fn remember(&mut self, edit: impl FnOnce(&mut Config)) {
        edit(&mut self.config);
        let Ok(context) = cosmic_config::Config::new(Self::APP_ID, Config::VERSION) else {
            return;
        };
        if let Err(why) = self.config.write_entry(&context) {
            tracing::warn!(%why, "could not save the settings");
        }
    }

    /// The folder to open when there is no reason to prefer another.
    ///
    /// The one that was open last, if the server still has it; otherwise the
    /// inbox. Remembering is what stops somebody who reads out of Archive from
    /// having to navigate there on every launch.
    fn restore_folder(&self) -> Option<usize> {
        self.folders
            .iter()
            .position(|folder| folder.wire_name == self.config.last_folder)
            .or_else(|| self.default_folder())
    }

    /// Puts the folder and the account in the window title.
    ///
    /// Which matters more here than in a single-document application: with two
    /// accounts open in two windows, "Envelope" twice in the switcher is not
    /// enough to tell them apart.
    fn update_title(&mut self) -> Task<Message> {
        let mut title = fl!("app-title");
        if let Some(folder) = self.current_folder() {
            title = format!("{} — {title}", folder.leaf_name());
        }
        if self.accounts.len() > 1
            && let Some(account) = self.account()
        {
            title = format!("{title} ({})", account.display_name);
        }
        match self.core.main_window_id() {
            Some(id) => self.set_window_title(title, id),
            None => Task::none(),
        }
    }

    /// Parks a watch on the inbox, if none is parked.
    ///
    /// Push mail: the server tells us, instead of the poll asking every two
    /// minutes. The poll stays — it is the fallback for servers without IDLE,
    /// and it is what drains the writeback queue on a schedule.
    fn start_watch(&mut self) -> Task<Message> {
        let Some(connection) = self.connection.clone() else {
            return Task::none();
        };
        if self.watching {
            return Task::none();
        }
        self.watching = true;
        let generation = self.watch_generation;

        // Under IDLE's 29-minute ceiling, and short enough that a thread
        // orphaned by an account switch dies within minutes rather than
        // holding a connection for half an hour.
        const WATCH: std::time::Duration = std::time::Duration::from_mins(4);

        cosmic::task::future(async move {
            let outcome =
                tokio::task::spawn_blocking(move || mail::watch_inbox(&connection, WATCH))
                    .await
                    .unwrap_or_else(|why| Err(why.to_string()));
            Message::WatchEnded {
                generation,
                outcome,
            }
        })
    }

    fn watch_ended(
        &mut self,
        generation: u64,
        outcome: Result<mail::WatchOutcome, String>,
    ) -> Task<Message> {
        if generation != self.watch_generation {
            // A watch from before an account switch. Its news is about a
            // mailbox that is no longer showing.
            return Task::none();
        }
        self.watching = false;
        match outcome {
            Ok(mail::WatchOutcome::Changed) => {
                // The sync is what acts on the news; the watch restarts once
                // it has told us. Quiet, like the poll — push mail arriving is
                // not something to narrate.
                let sync = if self.syncing {
                    Task::none()
                } else {
                    self.sync_now()
                };
                Task::batch([sync, self.start_watch()])
            }
            Ok(mail::WatchOutcome::TimedOut) => self.start_watch(),
            Ok(mail::WatchOutcome::Unsupported) => {
                // The poll covers this account. Reconnecting forever to hear
                // the same no would be all cost.
                tracing::info!("the server has no IDLE; staying on the poll");
                Task::none()
            }
            Err(why) => {
                // The network dropped, or the server did. Wait out a beat
                // before reconnecting so a hard-down server is not hammered.
                tracing::debug!(why, "the inbox watch ended with an error; will retry");
                let generation = self.watch_generation;
                cosmic::task::future(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    Message::WatchEnded {
                        generation,
                        outcome: Ok(mail::WatchOutcome::TimedOut),
                    }
                })
            }
        }
    }

    /// Is something expecting characters?
    ///
    /// Single-letter shortcuts must not fire while a text field has focus —
    /// pressing `c` in the composer has to type a `c`. Modifier combinations
    /// are exempt, which is the whole reason they exist alongside the letters.
    ///
    /// Derived from what is on screen rather than counted from focus events,
    /// and that is the whole point. A field removed from the tree never sends
    /// the unfocus that would balance a count — the palette closing on Enter
    /// does exactly that — so a counter drifts up and never comes down, and
    /// every single-letter shortcut stays dead for the rest of the session
    /// with nothing to show why. The search box in the header is the one
    /// field that outlives every surface, so it is the one whose focus is
    /// worth tracking; everything else lives inside something this can see.
    /// The composer moved into a window of its own, so this is now a question
    /// about *which* window: `c` typed into a composer must be a `c`, and the
    /// same `c` typed into a detached message must still open a composer.
    fn typing(&self) -> bool {
        if self.composer().is_some() {
            return true;
        }
        // A detached message is a reader. It has no text fields, so the
        // single-letter shortcuts work there exactly as they do in the list —
        // and the main window's own state is not its business.
        if self.detached_read().is_some() {
            return false;
        }
        self.search_focused
            || self.palette.is_some()
            || self.folder_dialog.is_some()
            || self.core.window.show_context
    }

    /// Decides what a key press meant.
    fn key_pressed(
        &mut self,
        modifiers: &cosmic::iced::keyboard::Modifiers,
        key: &cosmic::iced::keyboard::Key,
        physical: Option<&cosmic::iced::keyboard::key::Physical>,
    ) -> Task<Message> {
        // Combinations first, and whatever has focus: that is what a modifier
        // is for. Matched against the same map the menu draws its accelerators
        // from, so the two cannot drift.
        for (bind, action) in &self.key_binds {
            if bind.matches(*modifiers, key, physical) {
                let action = action.0;
                self.pending_chord = None;
                return self.act(action);
            }
        }

        // The move and label pickers' arrows, same mechanism as the
        // palette's below.
        let picker_rows = match self.folder_dialog.as_ref() {
            Some(FolderDialog::Label { .. }) => Some(self.label_rows.len()),
            Some(FolderDialog::Move { .. }) => Some(self.move_rows.len()),
            _ => None,
        };
        if let (
            Some(rows),
            Some(FolderDialog::Move { selected, .. } | FolderDialog::Label { selected, .. }),
        ) = (picker_rows, self.folder_dialog.as_mut())
        {
            use cosmic::iced::keyboard::key::Named;
            let last = rows.saturating_sub(1);
            match key {
                cosmic::iced::keyboard::Key::Named(Named::ArrowDown) => {
                    *selected = (*selected + 1).min(last);
                    return Task::none();
                }
                cosmic::iced::keyboard::Key::Named(Named::ArrowUp) => {
                    *selected = selected.saturating_sub(1);
                    return Task::none();
                }
                _ => {}
            }
        }

        // The palette's arrows, while it is open. Its input has focus, so
        // these arrive here only because a single-line input ignores vertical
        // arrows — which is exactly the gap that makes this work.
        if let Some((_, selected)) = self.palette.as_mut() {
            use cosmic::iced::keyboard::key::Named;
            let last = self.palette_rows.len().saturating_sub(1);
            match key {
                cosmic::iced::keyboard::Key::Named(Named::ArrowDown) => {
                    *selected = (*selected + 1).min(last);
                    return Task::none();
                }
                cosmic::iced::keyboard::Key::Named(Named::ArrowUp) => {
                    *selected = selected.saturating_sub(1);
                    return Task::none();
                }
                _ => {}
            }
        }

        if self.typing() {
            // A half-typed chord does not survive somebody clicking into a
            // field and typing; it would fire on whatever they pressed after.
            self.pending_chord = None;
            return Task::none();
        }

        if !bare(modifiers) {
            self.pending_chord = None;
            return Task::none();
        }

        let cosmic::iced::keyboard::Key::Character(text) = key else {
            self.pending_chord = None;
            return Task::none();
        };
        let Some(character) = text.chars().next().filter(|_| text.chars().count() == 1) else {
            self.pending_chord = None;
            return Task::none();
        };

        match actions::for_bare(character, self.pending_chord.take()) {
            Resolved::Act(action) => self.act(action),
            Resolved::Pending(first) => {
                self.pending_chord = Some(first);
                Task::none()
            }
            Resolved::Nothing => Task::none(),
        }
    }

    /// Performs one of the registry's actions.
    ///
    /// The single place an action becomes behaviour, so a keystroke, a menu
    /// entry, and a button cannot drift apart about what it does.
    fn act(&mut self, action: Action) -> Task<Message> {
        // A window of one's own only does what that window is for. Without
        // this, `e` typed into a message opened an hour ago would archive
        // whatever the list happens to have selected now.
        if !action.allowed_in(self.surface()) {
            return Task::none();
        }
        match action {
            Action::Compose => self.update(Message::Compose),
            Action::Reply => self.update(Message::Reply { all: false }),
            Action::ReplyAll => self.update(Message::Reply { all: true }),
            Action::Forward => self.update(Message::Forward),
            Action::Send => {
                if self.composer().is_some() {
                    self.update(Message::ComposeSend)
                } else {
                    Task::none()
                }
            }

            Action::Next => self.step_selection(1),
            Action::Previous => self.step_selection(-1),
            Action::Archive => self.update(Message::Archive),
            Action::Delete => self.update(Message::Delete),
            Action::ToggleRead => self.update(Message::ToggleRead),
            Action::ToggleFlagged => self.update(Message::ToggleFlagged),
            Action::Detach => self.update(Message::Detach),

            Action::ImportMbox => self.update(Message::ImportMbox),
            // Ctrl+Z means "take back what I just did", and inside the
            // composer what the user just did was type. Before this it reached
            // past the composer and un-archived a conversation they had
            // finished with — the same keystroke, two windows away from what
            // they were looking at.
            Action::Undo => {
                if self.composer_mut().is_some_and(Composer::undo) {
                    return Task::none();
                }
                self.undo()
            }
            Action::Redo => {
                if let Some(composer) = self.composer_mut() {
                    composer.redo();
                }
                Task::none()
            }
            Action::Search => {
                // Focus rather than a mode: the box is always there, and this
                // is the keystroke that puts the cursor in it.
                self.showing_drafts = false;
                self.showing_outbox = false;
                cosmic::widget::text_input::focus(SEARCH_ID.clone())
            }
            Action::Sync => self.update(Message::SyncNow),
            Action::Escape => self.escape(),

            Action::GoInbox => self.go_to(SpecialUse::Inbox),
            Action::GoSent => self.go_to(SpecialUse::Sent),
            Action::GoArchive => self.go_to(SpecialUse::Archive),
            Action::GoDrafts => {
                if !self.showing_drafts {
                    return self.update(Message::ShowDrafts);
                }
                Task::none()
            }
            Action::GoOutbox => {
                if !self.showing_outbox {
                    return self.update(Message::ShowOutbox);
                }
                Task::none()
            }

            Action::Snooze => {
                if self.selected_conversation.is_none() {
                    return Task::none();
                }
                self.update(Message::FolderDialogOpened(FolderDialog::Snooze))
            }
            Action::Label => {
                if self.selected_conversation.is_none() {
                    return Task::none();
                }
                let task = self.update(Message::FolderDialogOpened(FolderDialog::Label {
                    query: String::new(),
                    selected: 0,
                }));
                // The folder's table loads on the worker while the dialog is
                // already up.
                let load = match (self.connection.clone(), self.current_folder().cloned()) {
                    (Some(connection), Some(folder)) => cosmic::task::future(async move {
                        let result =
                            tokio::task::spawn_blocking(move || mail::labels(&connection, &folder))
                                .await
                                .unwrap_or_else(|why| Err(why.to_string()));
                        Message::LabelRowsLoaded(Box::new(result))
                    }),
                    _ => Task::none(),
                };
                Task::batch([task, load])
            }
            Action::MoveToFolder => {
                if self.selected_conversation.is_none() {
                    return Task::none();
                }
                self.update(Message::FolderDialogOpened(FolderDialog::Move {
                    query: String::new(),
                    selected: 0,
                }))
            }
            Action::NewFolder => self.update(Message::FolderDialogOpened(FolderDialog::Create {
                name: String::new(),
            })),
            Action::RenameFolder => match self.actionable_folder() {
                Some(folder) => {
                    let name = folder.leaf_name().to_owned();
                    self.update(Message::FolderDialogOpened(FolderDialog::Rename { name }))
                }
                None => Task::none(),
            },
            Action::DeleteFolder => match self.actionable_folder() {
                Some(_) => self.update(Message::FolderDialogOpened(FolderDialog::Delete)),
                None => Task::none(),
            },

            Action::Palette => {
                if self.palette.take().is_some() {
                    self.palette_rows.clear();
                    return Task::none();
                }
                self.palette = Some((String::new(), 0));
                self.refresh_palette_rows();
                cosmic::widget::text_input::focus(crate::ui::PALETTE_ID.clone())
            }
            Action::Rules => {
                self.open_rules_page();
                self.update(Message::ToggleContextPage(ContextPage::Rules))
            }
            Action::ToggleSidebar => {
                // The shell keeps two flags: one for a window wide enough to
                // hold the sidebar beside the content, one for a narrow window
                // where it overlays the content instead. Its own header button
                // flips whichever applies, and so does this.
                if self.core().is_condensed() {
                    self.core_mut().nav_bar_toggle_condensed();
                } else {
                    self.core_mut().nav_bar_toggle();
                }
                Task::none()
            }
            Action::Shortcuts => self.update(Message::ToggleContextPage(ContextPage::Shortcuts)),
            Action::Settings => self.update(Message::ToggleContextPage(ContextPage::Settings)),
            Action::Accounts => self.update(Message::ToggleContextPage(ContextPage::Accounts)),
            Action::About => self.update(Message::ToggleContextPage(ContextPage::About)),
        }
    }

    /// Takes back the most recent reversible action.
    fn undo(&mut self) -> Task<Message> {
        let Some(entry) = self.undo_stack.pop() else {
            self.say(fl!("nothing-to-undo"));
            return Task::none();
        };
        let Some(connection) = self.connection.clone() else {
            return Task::none();
        };
        let description = entry.description.clone();

        // Taking a send back ends in a composer, not a status line, so it
        // reports through its own message.
        if let Reverse::CancelSend { id } = entry.reverse {
            return cosmic::task::future(async move {
                let result =
                    tokio::task::spawn_blocking(move || mail::cancel_send(&connection, &id))
                        .await
                        .unwrap_or_else(|why| Err(why.to_string()));
                Message::SendCancelled(Box::new(result))
            });
        }

        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || match entry.reverse {
                Reverse::Flags { folder, previous } => {
                    mail::restore_flags(&connection, &folder, &previous)
                }
                Reverse::Unmove { folder, messages } => {
                    mail::unmove(&connection, &folder, &messages)
                }
                Reverse::CancelSend { .. } => unreachable!("handled above"),
            })
            .await
            .unwrap_or_else(|why| Err(why.to_string()));
            Message::Undone(result.map(|()| description))
        })
    }

    /// Moves the selection through the conversation list.
    fn step_selection(&mut self, by: isize) -> Task<Message> {
        match next_selection(self.selected_conversation, self.conversations.len(), by) {
            Some(next) => self.update(Message::ConversationSelected(next)),
            None => Task::none(),
        }
    }

    /// Selects a folder by its role.
    fn go_to(&mut self, role: SpecialUse) -> Task<Message> {
        self.showing_drafts = false;
        self.showing_outbox = false;
        let Some(index) = self
            .folders
            .iter()
            .position(|folder| folder.special_use == Some(role))
        else {
            self.say(fl!("no-such-folder"));
            return Task::none();
        };
        self.update(Message::FolderSelected(index))
    }

    /// Backs out of whatever is open, innermost first.
    ///
    /// The order is what makes one key enough: Escape in a composer closes the
    /// composer, not the window, and Escape with nothing open clears the
    /// search.
    fn escape(&mut self) -> Task<Message> {
        if self.folder_dialog.take().is_some() {
            self.move_rows.clear();
            return Task::none();
        }
        if self.palette.take().is_some() {
            self.palette_rows.clear();
            return Task::none();
        }
        // Escape in a window of its own closes that window — the composer
        // keeping what was typed, a detached message simply going away.
        // `on_escape` arrives without a window, so the focused one is the one
        // it was aimed at. Escape in the main window keeps working down the
        // list below.
        if let Some(id) = self.routed.or_else(|| self.focused_detached()) {
            return self.close_window(id);
        }
        if self.core.window.show_context {
            self.core.window.show_context = false;
            return Task::none();
        }
        if self.is_searching() {
            return self.update(Message::SearchCleared);
        }
        if self.showing_drafts || self.showing_outbox {
            self.showing_drafts = false;
            self.showing_outbox = false;
            return Task::none();
        }
        Task::none()
    }

    fn is_searching(&self) -> bool {
        !self.search.trim().is_empty()
    }

    fn run_search(&mut self) -> Task<Message> {
        let (Some(connection), true) = (self.connection.clone(), self.is_searching()) else {
            self.results.clear();
            self.searching = false;
            return Task::none();
        };
        self.searching = true;
        let folders = self.folders.clone();
        let input = self.search.clone();

        cosmic::task::future(async move {
            let results = tokio::task::spawn_blocking(move || {
                mail::search(&connection, &folders, &input, SEARCH_LIMIT)
            })
            .await
            .unwrap_or_else(|why| Err(why.to_string()))
            .unwrap_or_else(|why| {
                tracing::warn!(why, "search failed");
                Vec::new()
            });
            Message::SearchFinished(results)
        })
    }

    /// Opens a search hit, switching folders if it is in another one.
    ///
    /// Switching is the point: a search that can only open what is already in
    /// front of you is a filter, not a search.
    fn open_hit(&mut self, index: usize) -> Task<Message> {
        let Some(hit) = self.results.get(index).cloned() else {
            return Task::none();
        };
        let Some(folder) = self
            .folders
            .iter()
            .position(|folder| folder.wire_name == hit.mailbox)
        else {
            self.say(fl!("hit-folder-gone"));
            return Task::none();
        };

        self.showing_drafts = false;
        self.selected_folder = Some(folder);
        self.reader_error = None;

        let (Some(connection), Some(folder)) =
            (self.connection.clone(), self.current_folder().cloned())
        else {
            return Task::none();
        };
        let uid = hit.uid;

        // The list behind the results is reloaded too, so leaving the search
        // lands in the folder the message is in rather than the one that was
        // showing when the search started.
        Task::batch([
            self.reload_conversations(),
            cosmic::task::future(async move {
                let result =
                    tokio::task::spawn_blocking(move || mail::open(&connection, &folder, uid))
                        .await
                        .unwrap_or_else(|why| Err(why.to_string()));
                Message::MessageOpened(Box::new(result))
            }),
        ])
    }

    fn reload_unified(&mut self) -> Task<Message> {
        cosmic::task::future(async move {
            let unified = tokio::task::spawn_blocking(|| {
                let connections = mail::all_connections();
                mail::unified_inbox(&connections)
            })
            .await
            .unwrap_or_else(|why| Err(why.to_string()))
            .unwrap_or_else(|why| {
                tracing::warn!(why, "could not build the unified inbox");
                Vec::new()
            });
            Message::UnifiedLoaded(unified)
        })
    }

    /// Opens a unified row by switching to its account and inbox.
    ///
    /// A switch rather than a side-channel read, for the same reason a search
    /// hit switches folders: everything the reader's buttons do — archive,
    /// delete, reply — operates on the selected account, and a reader showing
    /// one account's message while the sidebar claims another is a desync
    /// with buttons attached.
    fn open_unified(&mut self, index: usize) -> Task<Message> {
        let Some(entry) = self.unified.get(index) else {
            return Task::none();
        };
        let account_id = entry.account_id.clone();
        let uid = entry.conversation.newest_uid();

        self.showing_unified = false;
        let switch = if self.selected_account.as_deref() == Some(account_id.as_str()) {
            self.go_to(SpecialUse::Inbox)
        } else {
            self.update(Message::AccountSelected(account_id))
        };

        // The message itself, through the row's own connection: the account
        // switch reloads lists asynchronously, and the reader should not wait
        // on that to show what was clicked.
        let open = match (self.connection.clone(), uid) {
            (Some(connection), Some(uid)) => {
                let inbox = cosmic_pim_mail::folder::from_list_entry("INBOX", Some('/'), &[]);
                cosmic::task::future(async move {
                    let result =
                        tokio::task::spawn_blocking(move || mail::open(&connection, &inbox, uid))
                            .await
                            .unwrap_or_else(|why| Err(why.to_string()));
                    Message::MessageOpened(Box::new(result))
                })
            }
            _ => Task::none(),
        };
        Task::batch([switch, open])
    }

    fn reload_outbox(&mut self) -> Task<Message> {
        let Some(connection) = self.connection.clone() else {
            self.outbox.clear();
            return Task::none();
        };
        cosmic::task::future(async move {
            let queued = tokio::task::spawn_blocking(move || mail::list_outbox(&connection))
                .await
                .unwrap_or_else(|why| Err(why.to_string()))
                .unwrap_or_else(|why| {
                    tracing::warn!(why, "could not list the outbox");
                    Vec::new()
                });
            Message::OutboxLoaded(queued)
        })
    }

    fn reload_drafts(&mut self) -> Task<Message> {
        let Some(connection) = self.connection.clone() else {
            self.drafts.clear();
            return Task::none();
        };
        cosmic::task::future(async move {
            let drafts = tokio::task::spawn_blocking(move || mail::list_drafts(&connection))
                .await
                .unwrap_or_else(|why| Err(why.to_string()))
                .unwrap_or_else(|why| {
                    tracing::warn!(why, "could not list drafts");
                    Vec::new()
                });
            Message::DraftsLoaded(drafts)
        })
    }

    /// Reopens a saved draft, in a window of its own.
    fn open_draft(&mut self, id: &str) -> Task<Message> {
        let Some(connection) = self.connection.clone() else {
            return Task::none();
        };
        match mail::load_draft(&connection, id) {
            Ok(Some(draft)) => {
                let mut composer = Composer::new(draft, None);
                // Carried, so re-saving replaces this draft rather than
                // leaving the old one beside a new one.
                composer.draft_id = Some(id.to_owned());
                return self.open_composer(composer);
            }
            Ok(None) => self.say(fl!("draft-gone")),
            Err(why) => self.say(why),
        }
        Task::none()
    }

    fn with_composer(&mut self, edit: impl FnOnce(&mut Composer)) -> Task<Message> {
        if let Some(composer) = self.composer_mut() {
            edit(composer);
            composer.error = None;
        }
        Task::none()
    }

    fn send_draft(&mut self) -> Task<Message> {
        // Everything the model owns is read before the composer is borrowed:
        // the composer lives in a window now, so holding it means holding the
        // window map, which means holding the model.
        let Some(connection) = self.connection.clone() else {
            return Task::none();
        };
        let grace = self.config.send_delay();
        let folders = self.folders.clone();

        let Some(composer) = self.composer_mut() else {
            return Task::none();
        };
        if composer.sending {
            return Task::none();
        }
        let draft = composer.outgoing();
        if let Some(problem) = draft.problem() {
            composer.error = Some(problem.to_owned());
            return Task::none();
        }
        composer.sending = true;
        composer.error = None;
        let answering = composer.answering.clone();
        let draft_id = composer.draft_id.clone();

        // With a grace configured, sending is scheduling: the message waits
        // out the delay in the outbox, where undo can still reach it.
        if grace > 0 {
            let not_before = chrono::Utc::now().timestamp_millis() + i64::from(grace) * 1_000;
            return self.schedule_send_at(not_before, true);
        }

        cosmic::task::future(async move {
            let sent = tokio::task::spawn_blocking(move || {
                mail::send(
                    &connection,
                    &draft,
                    &folders,
                    answering,
                    draft_id.as_deref(),
                )
            })
            .await
            .unwrap_or_else(|why| mail::Sent::Uncertain(why.to_string()));
            Message::ComposeSent(Box::new(sent))
        })
    }

    /// Queues the composer's message to go at `not_before_ms`.
    fn schedule_send_at(&mut self, not_before_ms: i64, grace: bool) -> Task<Message> {
        let connection = self.connection.clone();
        let (Some(composer), Some(connection)) = (self.composer_mut(), connection) else {
            return Task::none();
        };
        let draft = composer.outgoing();
        if let Some(problem) = draft.problem() {
            composer.error = Some(problem.to_owned());
            composer.sending = false;
            return Task::none();
        }
        composer.sending = true;
        composer.error = None;
        let draft_id = composer.draft_id.clone();

        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                mail::schedule_send(&connection, draft_id.as_deref(), &draft, not_before_ms)
                    .map(|id| (id, not_before_ms, grace))
            })
            .await
            .unwrap_or_else(|why| Err(why.to_string()));
            Message::SendScheduled(Box::new(result))
        })
    }

    fn composer_finished(&mut self, sent: mail::Sent) -> Task<Message> {
        let Some(composer) = self.composer_mut() else {
            return Task::none();
        };
        composer.sending = false;
        match sent {
            mail::Sent::Ok { filed } => {
                // The window goes only on success. A failed send that took the
                // window and what the user wrote with it would be
                // unforgivable, and is the whole reason the composer stays
                // open below.
                let closed = self.discard_composer();
                self.say(if filed {
                    fl!("sent")
                } else {
                    fl!("sent-not-filed")
                });
                // The sweep retires the sent draft's server mirror.
                return Task::batch([closed, self.reload_drafts(), self.sweep_drafts_now()]);
            }
            mail::Sent::Queued => {
                // Gone from the screen, because the message is no longer the
                // user's problem: it is queued, durable, and will go out on
                // the next check.
                let closed = self.discard_composer();
                self.say(fl!("send-queued"));
                return Task::batch([closed, self.reload_outbox()]);
            }
            mail::Sent::Failed(why) => {
                composer.error = Some(fl!("send-failed", reason = why));
            }
            mail::Sent::Uncertain(why) => {
                // Deliberately different words. "Try again" would be wrong
                // advice here: the message may already have arrived, and the
                // client must not be the thing that sends it twice.
                composer.error = Some(fl!("send-uncertain", reason = why));
            }
        }
        Task::none()
    }

    fn with_form(&mut self, edit: impl FnOnce(&mut MailForm)) -> Task<Message> {
        if let Some(form) = self.mail_form.as_mut() {
            edit(form);
            // Any edit invalidates the last validation failure.
            form.error = None;
        }
        Task::none()
    }

    fn with_add_form(&mut self, edit: impl FnOnce(&mut AddForm)) -> Task<Message> {
        if let Some(form) = self.add_form.as_mut() {
            edit(form);
            form.error = None;
        }
        Task::none()
    }

    /// Adds the account the form describes, with a password.
    ///
    /// The servers come from the address: the registry and the built-in table
    /// answer at once, and any other domain goes to the network first.
    fn confirm_add(&mut self) -> Task<Message> {
        let Some(form) = self.add_form.as_mut() else {
            return Task::none();
        };
        if form.adding {
            return Task::none();
        }
        let email = form.email.trim().to_owned();
        if !email.contains('@') {
            form.error = Some(fl!("add-needs-address"));
            return Task::none();
        }
        if form.password.is_empty() {
            form.error = Some(fl!("add-needs-password"));
            return Task::none();
        }
        if let Some(endpoint) = mail::password_settings(&email) {
            return self.add_account(Ok(endpoint));
        }
        form.adding = true;
        form.error = None;

        cosmic::task::future(async move {
            let found = tokio::task::spawn_blocking(move || mail::discover(&email))
                .await
                .unwrap_or_else(|why| Err(why.to_string()));
            Message::AddFormDiscovered(Box::new(found))
        })
    }

    /// Stores the account the add form describes, and opens it.
    ///
    /// With no server found it is stored all the same: the address and the
    /// password are right, and the server form opens on the new account with
    /// the reason, so the one missing fact is typed where it belongs rather
    /// than the whole form done over.
    fn add_account(&mut self, endpoint: Result<MailEndpoint, String>) -> Task<Message> {
        let Some(form) = self.add_form.take() else {
            return Task::none();
        };
        let (endpoint, problem) = match endpoint {
            Ok(endpoint) => (Some(endpoint), None),
            Err(why) => (None, Some(why)),
        };
        let account = mail_account(&form.name, &form.email, endpoint);
        let id = account.id.clone();

        // Written through the shared store, so Slate and Circle see the same
        // account the moment they next read the file.
        let stored = AccountStore::open_default()
            .and_then(|mut store| store.add(account, &form.password).map(|()| store));
        let store = match stored {
            Ok(store) => store,
            Err(why) => {
                self.add_form = Some(AddForm {
                    adding: false,
                    error: Some(why.to_string()),
                    ..form
                });
                return Task::none();
            }
        };
        self.accounts = store.accounts().to_vec();
        let task = self.update(Message::AccountSelected(id.clone()));
        if let Some(why) = problem {
            self.mail_form = self.accounts.iter().find(|a| a.id == id).map(MailForm::new);
            if let Some(form) = self.mail_form.as_mut() {
                form.error = Some(fl!("add-no-server", reason = why));
            }
        }
        task
    }

    /// Forgets an account and its password.
    ///
    /// The mail already on disk stays: it is the user's, in maildir, and a
    /// removed account is not an instruction to delete it.
    fn remove_account(&mut self, id: &str) -> Task<Message> {
        let removed =
            AccountStore::open_default().and_then(|mut store| store.remove(id).map(|()| store));
        let store = match removed {
            Ok(store) => store,
            Err(why) => {
                self.say(why.to_string());
                return Task::none();
            }
        };
        self.accounts = store.accounts().to_vec();
        if self
            .mail_form
            .as_ref()
            .is_some_and(|form| form.account_id == id)
        {
            self.mail_form = None;
        }
        if self.selected_account.as_deref() != Some(id) {
            return Task::none();
        }
        // The window was showing the account just removed: move to another,
        // or to nothing.
        self.selected_account = None;
        self.watch_generation += 1;
        self.watching = false;
        self.undo_stack.clear();
        self.clear_mailbox_state();
        if let Some(next) = self.accounts.first().map(|account| account.id.clone()) {
            return self.update(Message::AccountSelected(next));
        }
        self.rebuild_connection();
        Task::none()
    }

    fn save_form(&mut self) -> Task<Message> {
        let Some(form) = self.mail_form.as_ref() else {
            return Task::none();
        };
        let Ok(port) = form.port.trim().parse::<u16>() else {
            return self.with_form(|form| form.error = Some(fl!("bad-port")));
        };

        let Ok(smtp_port) = form.smtp_port.trim().parse::<u16>() else {
            return self.with_form(|form| form.error = Some(fl!("bad-port")));
        };

        // Struct-update over the constructor, so a field the substrate grows —
        // it has already grown five — defaults sensibly here instead of
        // breaking the build or, worse, being zeroed.
        // The one "incoming server" pair the form shows maps to whichever
        // protocol was chosen — a POP3 user typed their POP3 server into it,
        // and asking them which field family that was would be exposing the
        // storage schema as UI.
        let mut endpoint = MailEndpoint {
            protocol: form.protocol,
            imap_port: port,
            imap_transport: form.transport,
            imap_username: Some(form.username.trim().to_owned()).filter(|u| !u.is_empty()),
            smtp_host: form.smtp_host.trim().to_owned(),
            smtp_port,
            smtp_transport: form.smtp_transport,
            jmap_session_url: Some(form.jmap_url.trim().to_owned()).filter(|u| !u.is_empty()),
            from_address: form.from_address.trim().to_owned(),
            from_name: form.from_name.trim().to_owned(),
            aliases: form.aliases.clone(),
            ..MailEndpoint::tls(form.host.trim())
        };
        if form.protocol == MailProtocol::Pop3 {
            endpoint.pop3_host = form.host.trim().to_owned();
            endpoint.pop3_port = port;
            endpoint.pop3_transport = form.transport;
        }
        let account_id = form.account_id.clone();

        // Written through the shared store, so Slate and Circle see the same
        // file the moment they next read it.
        let mut store = match AccountStore::open_default() {
            Ok(store) => store,
            Err(why) => return self.with_form(|form| form.error = Some(why.to_string())),
        };
        if let Err(why) = store.set_mail_endpoint(&account_id, Some(endpoint)) {
            return self.with_form(|form| form.error = Some(why.to_string()));
        }

        self.mail_form = None;
        self.accounts = store.accounts().to_vec();
        self.selected_account = Some(account_id);
        self.clear_mailbox_state();
        self.rebuild_connection();
        self.load_cached_folders()
    }
}

/// Where `j` or `k` moves the selection, or `None` when it does not move.
///
/// Clamped rather than wrapped. Wrapping from the end of a hundred-message list
/// Does this key press carry no modifier a shortcut would have claimed?
///
/// Bare keys mean bare. Every binding that wants a modifier is in the key-bind
/// map and is matched before this; anything still holding one is a combination
/// this build does not know, and letting it through to the single-letter path
/// fires the wrong verb — Ctrl+C is the universal copy key, not "compose".
/// Shift is exempt, because it is how `?` and `#` are typed at all.
fn bare(modifiers: &cosmic::iced::keyboard::Modifiers) -> bool {
    !modifiers.control() && !modifiers.alt() && !modifiers.logo()
}

/// back to its start is never what somebody holding `j` meant, and it is
/// disorienting in a way a stop at the end is not.
fn next_selection(current: Option<usize>, len: usize, by: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len - 1;
    let next = match current {
        // A fresh list selects its newest message rather than doing nothing:
        // pressing `j` should visibly do something the first time.
        None => 0,
        Some(current) => {
            let moved = isize::try_from(current).unwrap_or(0).saturating_add(by);
            usize::try_from(moved.clamp(0, isize::try_from(last).unwrap_or(0))).unwrap_or(0)
        }
    };
    (Some(next) != current).then_some(next)
}

/// One menu entry for a registry action.
fn item(action: Action) -> menu::Item<MenuAction, String> {
    menu::Item::Button(action.label(), None, MenuAction(action))
}

/// A password account for an address, as the add form describes it.
///
/// No `CalDAV` URL: this is a mail account until somebody tells Slate
/// otherwise. The name typed goes on outgoing mail and names the account in
/// the sidebar, falling back to the address, which is the only other thing
/// an account can be called. A login discovery reports as the address itself
/// is not stored: the account already carries it.
fn mail_account(name: &str, email: &str, endpoint: Option<MailEndpoint>) -> Account {
    let email = email.trim();
    let name = name.trim();
    let mut account = Account::new(if name.is_empty() { email } else { name }, "", email);
    account.mail = endpoint.map(|endpoint| MailEndpoint {
        from_address: email.to_owned(),
        from_name: name.to_owned(),
        imap_username: endpoint.imap_username.filter(|login| login != email),
        ..endpoint
    });
    account
}

fn load_accounts() -> Vec<Account> {
    match AccountStore::open_default() {
        Ok(store) => store.accounts().to_vec(),
        Err(why) => {
            tracing::warn!(%why, "could not read the shared account store");
            Vec::new()
        }
    }
}

fn summarise(report: &SyncReport) -> String {
    let mut parts = vec![fl!(
        "sync-summary",
        fetched = report.fetched,
        pushed = report.pushed
    )];
    if report.sent > 0 {
        parts.push(fl!("sync-sent", count = report.sent));
    }
    if !report.failures.is_empty() {
        parts.push(fl!("sync-partial", count = report.failures.len()));
        for (folder, why) in &report.failures {
            tracing::warn!(folder, why, "a mailbox could not be synced");
        }
    }
    if report.stuck > 0 {
        parts.push(fl!("sync-stuck", count = report.stuck));
    }
    parts.join(" ")
}

/// The inverse of `Folder::local_name` — percent-decoding a maildir directory
/// name back into the wire name it was made from.
fn unescape_local_name(local: &str) -> Option<String> {
    let bytes = local.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = local.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_modifier_stops_a_letter_from_firing_its_bare_shortcut() {
        use cosmic::iced::keyboard::Modifiers;

        assert!(
            bare(&Modifiers::empty()),
            "an unmodified letter is a shortcut"
        );
        // Shift is how the punctuation shortcuts are typed at all.
        assert!(bare(&Modifiers::SHIFT), "shift is part of typing `?`");

        // The bug this guards: without it, `c` is reached through Ctrl+C and
        // the copy key opens the composer.
        assert!(!bare(&Modifiers::CTRL), "ctrl+c must not compose");
        assert!(!bare(&Modifiers::ALT), "alt+j must not move the selection");
        assert!(
            !bare(&Modifiers::LOGO),
            "super+k must not move the selection"
        );
        assert!(!bare(&(Modifiers::CTRL | Modifiers::SHIFT)));
    }

    #[test]
    fn an_added_account_is_named_sends_as_its_address_and_has_no_calendar() {
        let found = cosmic_pim_mail::discovery::known("ada@gmail.com").expect("gmail is known");
        let account = mail_account("  Ada ", " ada@gmail.com ", Some(mail::endpoint_of(&found)));

        assert_eq!(account.display_name, "Ada");
        assert_eq!(account.username, "ada@gmail.com");
        assert_eq!(account.url, "");
        assert_eq!(account.auth, cosmic_pim_accounts::AuthMethod::Password);
        assert_eq!(
            account.from_identity(),
            Some(("Ada".to_owned(), "ada@gmail.com".to_owned()))
        );
        let mail = account.mail.as_ref().expect("the endpoint was given");
        assert_eq!(mail.imap_host, "imap.gmail.com");
        assert_eq!(mail.smtp_host, "smtp.gmail.com");
        // Discovery reports the address as the login; that is the account's
        // own username and storing it twice would be one more thing to drift.
        assert_eq!(mail.imap_username, None);
        assert_eq!(account.mail_username(), "ada@gmail.com");
    }

    #[test]
    fn an_added_account_with_no_name_is_called_by_its_address() {
        let account = mail_account("", "ada@example.org", None);
        assert_eq!(account.display_name, "ada@example.org");
        assert!(account.mail.is_none());
        // Still sends as the address: the login is one.
        assert_eq!(
            account.from_identity(),
            Some(("ada@example.org".to_owned(), "ada@example.org".to_owned()))
        );
    }

    #[test]
    fn the_add_form_needs_a_whole_address_and_a_password() {
        let mut form = AddForm {
            email: "ada".to_owned(),
            password: "hunter2".to_owned(),
            ..AddForm::default()
        };
        assert!(!form.can_add());
        form.email = "ada@example.org".to_owned();
        assert!(form.can_add());
        form.password.clear();
        assert!(!form.can_add());
        form.password = "hunter2".to_owned();
        form.adding = true;
        assert!(
            !form.can_add(),
            "not twice while the first is on the network"
        );
    }

    #[test]
    fn the_add_form_hides_the_password_only_when_a_sign_in_can_actually_happen() {
        let mut form = AddForm {
            provider: Some(mail::ProviderNote {
                name: "Google".to_owned(),
                hint: None,
                uses_sign_in: true,
                sign_in_ready: false,
            }),
            ..AddForm::default()
        };
        // No client id: the browser route is shut, and an app password over
        // IMAP is the one way in, so the field has to be there.
        assert!(!form.wants_sign_in());
        form.provider.as_mut().unwrap().sign_in_ready = true;
        assert!(form.wants_sign_in());
    }

    #[test]
    fn maildir_directory_names_round_trip_back_to_wire_names() {
        // The cached-folder path depends on this being exact: a name that does
        // not round-trip produces a folder the server has never heard of.
        for wire in [
            "INBOX",
            "INBOX.Projects.Alpha",
            "&A6ADsQPBA7EDuwOuA8ADxAO1A8I-",
            "[Gmail]/All Mail",
            "Work/2024",
        ] {
            let folder = cosmic_pim_mail::folder::from_list_entry(wire, Some('/'), &[]);
            assert_eq!(
                unescape_local_name(&folder.local_name()).as_deref(),
                Some(wire),
                "{wire}"
            );
        }
    }

    #[test]
    fn stepping_the_selection_starts_at_the_top_and_stops_at_the_ends() {
        // Pressing `j` on a fresh list must visibly do something.
        assert_eq!(next_selection(None, 3, 1), Some(0));
        assert_eq!(next_selection(None, 3, -1), Some(0));

        assert_eq!(next_selection(Some(0), 3, 1), Some(1));
        assert_eq!(next_selection(Some(2), 3, -1), Some(1));

        // Clamped, not wrapped: holding `j` at the bottom of a long list must
        // not jump back to the top.
        assert_eq!(next_selection(Some(2), 3, 1), None);
        assert_eq!(next_selection(Some(0), 3, -1), None);
    }

    #[test]
    fn stepping_an_empty_list_does_nothing_rather_than_panicking() {
        assert_eq!(next_selection(None, 0, 1), None);
        assert_eq!(next_selection(Some(0), 0, -1), None);
    }

    #[test]
    fn address_fields_accept_both_shapes_people_actually_type() {
        let parsed = parse_addresses("ada@example.com, Bob Smith <Bob@Example.NET> ,, cleo@x.org");
        assert_eq!(parsed.len(), 3, "{parsed:?}");
        assert_eq!(parsed[0].address, "ada@example.com");
        assert_eq!(parsed[0].name, None);
        assert_eq!(parsed[1].name.as_deref(), Some("Bob Smith"));
        assert_eq!(
            parsed[1].address, "bob@example.net",
            "the address was not folded, so it will not match an address book"
        );
        assert_eq!(parsed[2].address, "cleo@x.org");
    }

    #[test]
    fn an_empty_address_field_yields_no_recipients_rather_than_a_blank_one() {
        // A blank Cc must not become an unsendable draft.
        assert!(parse_addresses("").is_empty());
        assert!(parse_addresses("  ,  , ").is_empty());
    }

    #[test]
    fn address_fields_round_trip_through_the_composer() {
        // What is shown in the field has to parse back to what it came from,
        // or opening a reply and pressing send changes the recipients.
        let original = parse_addresses("Ada <ada@example.com>, bob@example.net");
        assert_eq!(parse_addresses(&join(&original)), original);
    }

    #[test]
    fn a_keystroke_cannot_act_on_a_window_it_was_not_typed_in() {
        use crate::actions::Surface;

        // The whole reason a composer has a window of its own: `e` typed into
        // one is a letter, and must not archive whatever the list has
        // selected two windows away.
        assert!(!Action::Archive.allowed_in(Surface::Writing));
        assert!(!Action::Delete.allowed_in(Surface::Writing));
        assert!(!Action::Next.allowed_in(Surface::Writing));
        assert!(!Action::Search.allowed_in(Surface::Writing));
        // What a composer is for still works there.
        assert!(Action::Send.allowed_in(Surface::Writing));
        assert!(Action::Undo.allowed_in(Surface::Writing));
        assert!(Action::Escape.allowed_in(Surface::Writing));

        // A detached message can be filed from where it is being read.
        assert!(Action::Archive.allowed_in(Surface::Reading));
        assert!(Action::Reply.allowed_in(Surface::Reading));
        assert!(Action::ToggleFlagged.allowed_in(Surface::Reading));
        // But it has no list, so nothing that walks one belongs to it.
        assert!(!Action::Next.allowed_in(Surface::Reading));
        assert!(!Action::Previous.allowed_in(Surface::Reading));
        assert!(!Action::GoInbox.allowed_in(Surface::Reading));

        // The main window is still the whole application.
        for binding in crate::actions::bindings() {
            assert!(
                binding.action.allowed_in(Surface::Main),
                "{:?} became unreachable from the main window",
                binding.action
            );
        }
    }

    #[test]
    fn a_window_is_named_after_what_it_holds() {
        assert_eq!(compose_title("  Plan  "), "Plan");
        assert_eq!(read_title("Re: Plan"), "Re: Plan");
        // Two different absences: nothing written yet, and a message that
        // arrived without one.
        assert_ne!(compose_title(""), read_title(""));
        assert!(!compose_title("   ").is_empty());
        assert!(!read_title("").is_empty());
    }

    #[test]
    fn a_composer_reports_the_same_problem_the_send_would_fail_with() {
        let me = cosmic_pim_mail::Mailbox {
            name: Some("Me".into()),
            address: "me@example.com".into(),
        };
        let mut composer = Composer::new(cosmic_pim_mail::Draft::new(me), None);
        assert_eq!(composer.problem(), Some("this draft has no recipients"));

        composer.to = "not-an-address".into();
        assert_eq!(
            composer.problem(),
            Some("one of the addresses is not an address")
        );

        composer.to = "ada@example.com".into();
        assert!(composer.problem().is_none());
        assert!(composer.resolved().build(false).is_ok());
    }

    #[test]
    fn a_composer_opened_as_a_reply_carries_the_threading_and_the_recipients() {
        let me = cosmic_pim_mail::Mailbox {
            name: Some("Me".into()),
            address: "me@example.com".into(),
        };
        let original = cosmic_pim_mail::Message::parse(
            b"Message-ID: <parent@x>\r\nFrom: Ada <ada@example.com>\r\nTo: me@example.com\r\nSubject: Plan\r\n\r\nbody\r\n",
        )
        .expect("parse");

        let composer = Composer::new(cosmic_pim_mail::Draft::reply(&original, me, false), None);
        assert_eq!(composer.to, "Ada <ada@example.com>");
        assert_eq!(composer.draft.subject, "Re: Plan");
        // The field text is what gets sent, not the draft's original list.
        assert_eq!(composer.resolved().to[0].address, "ada@example.com");
    }

    /// A composer with `body` already typed into it.
    fn composing(body: &str) -> Composer {
        let me = cosmic_pim_mail::Mailbox {
            name: Some("Me".into()),
            address: "me@example.com".into(),
        };
        let mut draft = cosmic_pim_mail::Draft::new(me);
        draft.body = body.to_owned();
        draft.to = parse_addresses("ada@example.com");
        let mut composer = Composer::new(draft, None);
        composer.to = "ada@example.com".into();
        composer
    }

    /// Types `text` a character at a time, the way the editor delivers it.
    fn type_into(composer: &mut Composer, text: &str) {
        for character in text.chars() {
            composer.edit(widget::text_editor::Action::Edit(
                widget::text_editor::Edit::Insert(character),
            ));
        }
    }

    fn press_enter(composer: &mut Composer) {
        composer.edit(widget::text_editor::Action::Edit(
            widget::text_editor::Edit::Enter,
        ));
    }

    #[test]
    fn enter_inside_a_quote_keeps_the_reply_quoted() {
        // The failure this guards against is silent and expensive: the rest of
        // the paragraph stops being marked as quoted, so the recipient reads
        // the sender's words as the words of whoever was being quoted.
        let mut composer = composing("> what do you think?");
        composer.edit(widget::text_editor::Action::Move(
            widget::text_editor::Motion::DocumentEnd,
        ));
        press_enter(&mut composer);
        type_into(&mut composer, "agreed");

        assert_eq!(composer.body.text(), "> what do you think?\n> agreed");
    }

    #[test]
    fn enter_twice_leaves_the_quote() {
        // The second Enter clears the empty `> ` and leaves the cursor on that
        // line rather than opening another one — the convention every editor
        // with list continuation uses for leaving a list, and the one people
        // already have in their fingers.
        let mut composer = composing("> what do you think?");
        composer.edit(widget::text_editor::Action::Move(
            widget::text_editor::Motion::DocumentEnd,
        ));
        press_enter(&mut composer);
        press_enter(&mut composer);
        type_into(&mut composer, "mine now");

        assert_eq!(composer.body.text(), "> what do you think?\nmine now");
    }

    #[test]
    fn undo_walks_back_by_word_and_stops_at_the_quoted_reply() {
        // The floor matters as much as the steps: undoing past the body the
        // composer opened with would throw away the quoted message the user
        // never typed and cannot get back.
        let opened = "> original\n\n";
        let mut composer = composing(opened);
        composer.edit(widget::text_editor::Action::Move(
            widget::text_editor::Motion::DocumentEnd,
        ));
        type_into(&mut composer, "hello there");
        assert_eq!(composer.body.text(), format!("{opened}hello there"));

        assert!(composer.undo(), "the last word should come back off");
        assert_eq!(composer.body.text(), format!("{opened}hello "));

        while composer.undo() {}
        assert_eq!(composer.body.text(), opened);
        assert!(!composer.undo(), "undo went past the body it opened with");
    }

    #[test]
    fn redo_puts_back_what_undo_took() {
        let mut composer = composing("");
        type_into(&mut composer, "hello");
        assert!(composer.undo());
        assert_eq!(composer.body.text(), "");
        assert!(composer.redo());
        assert_eq!(composer.body.text(), "hello");
        assert!(!composer.redo());
    }

    #[test]
    fn a_sent_body_is_wrapped_but_the_draft_kept_is_not() {
        let long = "word ".repeat(40);
        let composer = composing(long.trim_end());

        let saved = composer.resolved().body;
        assert_eq!(saved, long.trim_end(), "a saved draft was reformatted");

        let sent = composer.outgoing().body;
        assert!(sent.lines().count() > 1, "the body went out unwrapped");
        for line in sent.lines() {
            assert!(
                line.chars().count() <= crate::text::WRAP_COLUMNS,
                "too long for text/plain: {line:?}"
            );
        }
        assert_eq!(
            sent.replace('\n', " "),
            long.trim_end(),
            "wrapping changed the words"
        );
    }

    #[test]
    fn a_malformed_directory_name_is_skipped_rather_than_guessed_at() {
        assert!(unescape_local_name("%ZZ").is_none());
        assert!(unescape_local_name("truncated%").is_none());
    }
}
