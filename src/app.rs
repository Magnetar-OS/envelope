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

const APP_ID: &str = "io.github.entro314labs.Envelope";
/// Read from the manifest rather than repeated here, so the About page cannot
/// name a repository the package does not come from.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const APP_ICON: &[u8] = include_bytes!("../resources/icons/hicolor/scalable/apps/icon.svg");

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
    list_error: Option<String>,
    reader_error: Option<String>,

    mail_form: Option<MailForm>,
    composer: Option<Composer>,
    /// The providers a browser sign-in can reach, loaded once — the registry
    /// is files on disk and does not change under a running app.
    sign_in_providers: Vec<crate::mail::SignInProvider>,
    /// The address typed into the sign-in row.
    sign_in_email: String,
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
    text_focus: usize,
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

    /// What is in the search box. Empty means the box is closed.
    search: String,
    results: Vec<cosmic_pim_mail::Hit>,
    searching: bool,
}

/// How many search results are shown.
///
/// A cap rather than paging: somebody who gets 500 hits needs a better query,
/// not a second page, and the honest answer to "there are more" is to say so.
const SEARCH_LIMIT: usize = 200;

/// The search box, so a keystroke can put the cursor in it.
static SEARCH_ID: std::sync::LazyLock<cosmic::widget::Id> =
    std::sync::LazyLock::new(|| cosmic::widget::Id::new("search"));

/// A message being written.
///
/// The addresses are held as **text**, not as parsed mailboxes, and that is on
/// purpose: a half-typed address is not a mailbox, and a composer that reparses
/// on every keystroke either rejects what the user is in the middle of typing or
/// silently drops it. Parsing happens once, on send, where a failure can be
/// explained.
pub struct Composer {
    pub draft: cosmic_pim_mail::Draft,
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
}

impl Composer {
    fn new(draft: cosmic_pim_mail::Draft, answering: Option<(Folder, u32)>) -> Self {
        Self {
            to: join(&draft.to),
            cc: join(&draft.cc),
            bcc: join(&draft.bcc),
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
            || !self.draft.body.trim().is_empty()
            || !self.to.trim().is_empty()
            || !self.cc.trim().is_empty()
            || !self.bcc.trim().is_empty()
    }

    /// The draft with the address fields as currently typed.
    fn resolved(&self) -> cosmic_pim_mail::Draft {
        let mut draft = self.draft.clone();
        draft.to = parse_addresses(&self.to);
        draft.cc = parse_addresses(&self.cc);
        draft.bcc = parse_addresses(&self.bcc);
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
        self.transport = transport_of(found.imap_security);
        self.smtp_host = found.smtp_host.clone();
        self.smtp_port = found.smtp_port.to_string();
        self.smtp_transport = transport_of(found.smtp_security);
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

fn transport_of(security: cosmic_pim_mail::imap::Security) -> Transport {
    match security {
        cosmic_pim_mail::imap::Security::Tls => Transport::Tls,
        cosmic_pim_mail::imap::Security::StartTls => Transport::StartTls,
        cosmic_pim_mail::imap::Security::Plaintext => Transport::Plaintext,
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
    /// The send-later presets — the same moments, a different verb.
    SendLater,
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
    ConversationsLoaded(Result<Vec<Conversation>, String>),
    ConversationSelected(usize),
    MessageOpened(Box<Result<Opened, String>>),

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
    SignInEmailChanged(String),
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
    ComposeBodyChanged(String),
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
    KeyPressed(
        cosmic::iced::keyboard::Modifiers,
        cosmic::iced::keyboard::Key,
        Option<cosmic::iced::keyboard::key::Physical>,
    ),
    TextFocused,
    TextUnfocused,
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
            list_error: None,
            reader_error: None,
            mail_form: None,
            composer: None,
            sign_in_providers: mail::sign_in_providers(),
            sign_in_email: String::new(),
            signing_in: false,
            palette: None,
            folder_dialog: None,
            move_rows: Vec::new(),
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
            text_focus: 0,
            pending_chord: None,
            poll_seconds: String::new(),
            send_delay: String::new(),
            watch_generation: 0,
            watching: false,
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
            results: Vec::new(),
            searching: false,
        };
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

        // A link the desktop handed us opens straight into the composer. Done
        // before the folder load so the user sees what they clicked on rather
        // than an inbox that turns into a composer a moment later.
        model.launch(flags.launch.as_ref());

        // The window is filled from disk first and the server is asked
        // afterwards, in that order: the mailbox is already there, so there is
        // no reason for the first frame to wait on a network round trip.
        let cached = model.load_cached_folders();
        let drafts = model.reload_drafts();
        let outbox = model.reload_outbox();
        let first_sync = model.sync_now();
        (model, Task::batch([cached, drafts, outbox, first_sync]))
    }

    fn header_end(&self) -> Vec<Element<'_, Self::Message>> {
        let search = widget::text_input(fl!("search"), &self.search)
            .id(SEARCH_ID.clone())
            .on_input(Message::SearchChanged)
            .on_clear(Message::SearchCleared)
            // Every text field reports focus, so single-letter shortcuts know
            // to stay out of the way.
            .on_focus(Message::TextFocused)
            .on_unfocus(Message::TextUnfocused)
            .width(Length::Fixed(260.0));
        vec![search.into()]
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        vec![
            menu::bar(vec![menu::Tree::with_children(
                menu::root(fl!("app-title")).apply(Element::from),
                menu::items(
                    &self.key_binds,
                    vec![
                        item(Action::Compose),
                        item(Action::ImportMbox),
                        menu::Item::Divider,
                        item(Action::Snooze),
                        item(Action::MoveToFolder),
                        item(Action::NewFolder),
                        item(Action::RenameFolder),
                        item(Action::DeleteFolder),
                        menu::Item::Divider,
                        item(Action::Search),
                        item(Action::Sync),
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
        Some(sidebar.map(cosmic::Action::App))
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
            // subscription does not have it.
            cosmic::iced::event::listen_with(|event, status, _| {
                use cosmic::iced::keyboard::Event;
                match event {
                    cosmic::iced::Event::Keyboard(Event::KeyPressed {
                        key,
                        modifiers,
                        physical_key,
                        ..
                    // A key a widget has already handled — a character going
                    // into a text field, a scroll — is not ours to reinterpret.
                    }) if status == cosmic::iced::event::Status::Ignored => {
                        Some(Message::KeyPressed(modifiers, key, Some(physical_key)))
                    }
                    _ => None,
                }
            }),
        ])
    }

    /// The application is closing.
    ///
    /// The composer's save-on-close path only runs when the *composer* is
    /// closed; without this, quitting the window with a half-written message
    /// open discards it — the exact loss the drafts store exists to prevent.
    /// The save is synchronous because a `Task` returned here would race the
    /// exit.
    fn on_app_exit(&mut self) -> Option<Self::Message> {
        if let (Some(composer), Some(connection)) = (self.composer.take(), self.connection.as_ref())
            && composer.is_worth_saving()
            && let Err(why) = mail::save_draft(
                connection,
                composer.draft_id.as_deref(),
                &composer.resolved(),
            )
        {
            tracing::error!(%why, "a draft was lost on exit");
        }
        None
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
                        self.open_mailto(url.as_str());
                        break;
                    }
                }
                Task::none()
            }
            cosmic::dbus_activation::Details::ActivateAction { action, .. } => {
                match action.parse::<crate::flags::Launch>() {
                    Ok(launch) => {
                        self.launch(Some(&launch));
                        Task::none()
                    }
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
                FolderDialog::Snooze => Some(crate::ui::folders::snooze_dialog()),
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
                    crate::ui::accounts::SignIn {
                        providers: &self.sign_in_providers,
                        email: &self.sign_in_email,
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
                selected: self.selected_conversation,
                loading: self.loading_conversations,
                error: self.list_error.as_deref(),
            }
            .view()
        };

        // The composer takes the reader's half of the window rather than a
        // dialog or a second window. Writing a reply is reading the thread with
        // extra steps: the list stays where it was, so the message being
        // answered is still one click away.
        let right = match self.composer.as_ref() {
            Some(composer) => {
                let selected = self
                    .connection
                    .as_ref()
                    .and_then(|c| {
                        c.identities.iter().position(|m| {
                            m.address.eq_ignore_ascii_case(&composer.draft.from.address)
                        })
                    })
                    .unwrap_or(0);
                crate::ui::composer::view(composer, &self.identity_labels, selected)
            }
            None => crate::ui::reader::Reader {
                opened: self.opened.as_ref(),
                error: self.reader_error.as_deref(),
                can_send: self
                    .connection
                    .as_ref()
                    .is_some_and(|c| c.submission.is_some()),
            }
            .view(),
        };

        widget::row::with_capacity(3)
            .push(
                widget::container(list)
                    // A fixed list column rather than a proportion: a message
                    // list is read down its left edge, and a column that grows
                    // with the window puts the sender and the date at opposite
                    // ends of a wide screen.
                    .width(Length::Fixed(380.0))
                    .height(Length::Fill),
            )
            .push(widget::divider::vertical::default())
            .push(
                widget::container(right)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .into()
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
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
                            self.status = Some(summarise(&report));
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
                        self.status = Some(fl!("sync-failed", reason = why));
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
                    Ok(conversations) => {
                        self.list_error = None;
                        if let Some(folder) = self.current_folder() {
                            self.unread.insert(
                                folder.wire_name.clone(),
                                conversations.iter().filter(|c| c.unread).count(),
                            );
                        }
                        self.conversations = conversations;
                        // The selection is an index into a list that just
                        // changed underneath it; keeping it would open an
                        // unrelated conversation.
                        self.selected_conversation = None;
                    }
                    Err(why) => {
                        self.conversations.clear();
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

            Message::MessageOpened(result) => {
                match *result {
                    Ok(opened) => {
                        self.reader_error = None;
                        let already_read = opened.flags.seen;
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
                let seen = self.opened.as_ref().is_some_and(|o| o.flags.seen);
                self.set_flags(move |flags| Flags {
                    seen: !seen,
                    ..flags
                })
            }

            Message::ToggleFlagged => {
                let flagged = self.opened.as_ref().is_some_and(|o| o.flags.flagged);
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
                    Err(why) => self.status = Some(why),
                }
                self.reload_conversations()
            }
            Message::Undone(result) => {
                match result {
                    Ok(what) => self.status = Some(fl!("undone", what = what)),
                    Err(why) => self.status = Some(fl!("undo-failed", reason = why)),
                }
                self.reload_conversations()
            }

            Message::MailFormStart(id) => {
                self.mail_form = self.accounts.iter().find(|a| a.id == id).map(MailForm::new);
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
            Message::SignInEmailChanged(email) => {
                self.sign_in_email = email;
                Task::none()
            }
            Message::SignInStarted(provider_id) => self.sign_in(&provider_id),
            Message::SignInFinished(result) => {
                self.signing_in = false;
                match *result {
                    Ok(account_id) => {
                        self.sign_in_email.clear();
                        self.accounts = load_accounts();
                        // Straight into the new account: the sign-in was the
                        // whole point of the visit.
                        self.update(Message::AccountSelected(account_id))
                    }
                    Err(why) => {
                        self.status = Some(fl!("sign-in-failed", reason = why));
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
            Message::ComposeSubjectChanged(text) => self.with_composer(|c| c.draft.subject = text),
            Message::ComposeBodyChanged(text) => self.with_composer(|c| c.draft.body = text),
            Message::ComposeCancel => self.close_composer(true),
            Message::ComposeDiscard => self.close_composer(false),
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
                    self.status = Some(why);
                }
                // Due now, so the next check takes it rather than waiting out a
                // backoff the user has just overridden.
                Task::batch([self.reload_outbox(), self.sync_now()])
            }
            Message::QueuedDiscarded(id) => {
                if let Some(connection) = self.connection.as_ref()
                    && let Err(why) = mail::discard_queued(connection, &id)
                {
                    self.status = Some(why);
                }
                self.reload_outbox()
            }
            Message::WatchEnded {
                generation,
                outcome,
            } => self.watch_ended(generation, outcome),
            Message::ConfigChanged(config) => {
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
            Message::TextFocused => {
                self.text_focus = self.text_focus.saturating_add(1);
                Task::none()
            }
            Message::TextUnfocused => {
                self.text_focus = self.text_focus.saturating_sub(1);
                Task::none()
            }
            Message::KeyPressed(modifiers, key, physical) => {
                self.key_pressed(&modifiers, &key, physical.as_ref())
            }
            Message::Act(action) => self.act(action),

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
                self.status = Some(fl!("importing"));
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
                        self.status = Some(if skipped > 0 {
                            fl!("imported-some", imported = imported, skipped = skipped)
                        } else {
                            fl!("imported", imported = imported)
                        });
                        // The upload is done; the pull is what makes them
                        // appear.
                        return self.sync_now();
                    }
                    Err(why) => self.status = Some(why),
                }
                Task::none()
            }
            Message::ExportMessage => {
                let (Some(connection), Some(folder), Some(uid)) = (
                    self.connection.clone(),
                    self.current_folder().cloned(),
                    self.opened.as_ref().map(|opened| opened.uid),
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
                self.status = Some(match result {
                    Ok(path) => fl!("attachment-saved", path = path.display().to_string()),
                    Err(why) => fl!("attachment-not-saved", reason = why),
                });
                Task::none()
            }
            Message::Unsubscribe => self.unsubscribe(),
            Message::Unsubscribed(result) => {
                self.status = Some(match result {
                    Ok(()) => fl!("unsubscribed"),
                    Err(why) => fl!("unsubscribe-failed", reason = why),
                });
                Task::none()
            }
            Message::SaveAttachment(index) => self.save_attachment(index),
            Message::AttachmentSaved(result) => {
                self.status = Some(match result {
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
                            if let Some(composer) = self.composer.as_mut() {
                                composer.draft.attachments.push(attachment);
                            }
                        }
                        Err(why) => {
                            if let Some(composer) = self.composer.as_mut() {
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
                        self.status = Some(fl!("draft-sync-failed", reason = why));
                    }
                    Ok(_) => {}
                    Err(why) => self.status = Some(fl!("draft-sync-failed", reason = why)),
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
                        self.composer = Some(composer);
                        self.reader_error = None;
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
                if index < self.rules.len() {
                    self.rules.remove(index);
                    self.save_rules_now();
                }
                Task::none()
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
                        if let Some((rule, why)) = report.failures.first() {
                            self.status = Some(fl!(
                                "rule-failed",
                                rule = rule.clone(),
                                reason = why.clone()
                            ));
                        } else if report.matched > 0 {
                            self.status = Some(fl!("rules-applied", count = report.matched));
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
                    Err(why) => self.status = Some(why),
                }
                Task::none()
            }

            Message::SnoozePicked(preset) => {
                self.folder_dialog = None;
                self.snooze_selected(preset.until_ms())
            }
            Message::SendLater => {
                if self.composer.is_none() {
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
                    self.composer = None;
                    self.status = Some(if grace {
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
                    Task::batch([self.reload_outbox(), self.reload_drafts(), timer])
                }
                Err(why) => {
                    if let Some(composer) = self.composer.as_mut() {
                        composer.sending = false;
                        composer.error = Some(why);
                    } else {
                        self.status = Some(why);
                    }
                    Task::none()
                }
            },
            Message::SendCancelled(result) => {
                match *result {
                    Ok(Some(draft)) => {
                        // The words the user wrote, back where they can be
                        // edited — the entire point of the grace.
                        self.composer = Some(Composer::new(draft, None));
                        self.status = Some(fl!("send-taken-back"));
                    }
                    Ok(None) => self.status = Some(fl!("send-already-gone")),
                    Err(why) => self.status = Some(why),
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
                    self.status = Some(fl!("snoozed-back", count = count));
                    // The messages are back in INBOX on the server; the next
                    // pull files them locally. Ask for one now rather than
                    // waiting out the poll.
                    self.sync_now()
                }
                Err(why) => {
                    self.status = Some(fl!("snooze-failed", reason = why));
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
                    self.status = Some(status);
                    // The folder list is the server's; the sync is what makes
                    // the change visible.
                    self.sync_now()
                }
                Err(why) => {
                    self.status = Some(why);
                    Task::none()
                }
            },

            Message::DraftOpened(id) => self.open_draft(&id),
            Message::DraftDeleted(id) => {
                if let Some(connection) = self.connection.as_ref()
                    && let Err(why) = mail::delete_draft(connection, &id)
                {
                    self.status = Some(why);
                }
                Task::batch([self.reload_drafts(), self.sweep_drafts_now()])
            }
            Message::ComposeSend => self.send_draft(),
            Message::ComposeSent(sent) => self.composer_finished(*sent),
        }
    }
}

impl AppModel {
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
                self.status = Some(why.to_string());
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
            Ok(None) => self.status = Some(fl!("no-mail-account")),
            Err(why) => self.status = Some(why),
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
            self.status = Some(fl!("no-mail-account"));
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

    /// Applies a flag change to the conversation's newest message.
    fn set_flags(&mut self, edit: impl Fn(Flags) -> Flags + Send + 'static) -> Task<Message> {
        let (Some(connection), Some(folder), Some(uid)) = (
            self.connection.clone(),
            self.current_folder().cloned(),
            self.selected_uid(),
        ) else {
            return Task::none();
        };

        // Reflected immediately so the button does not sit in the old state
        // waiting for a disk write; the reload that follows is what makes it
        // true rather than merely displayed.
        if let Some(opened) = self.opened.as_mut() {
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
            self.status = Some(fl!("no-archive-folder"));
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
        let (Some(connection), Some(folder), Some(index)) = (
            self.connection.clone(),
            self.current_folder().cloned(),
            self.selected_conversation,
        ) else {
            return Task::none();
        };
        if destination.wire_name == folder.wire_name {
            return Task::none();
        }
        let Some(uids) = self.conversations.get(index).map(|c| c.uids.clone()) else {
            return Task::none();
        };

        self.opened = None;
        self.selected_conversation = None;

        let description = fl!("undo-move", folder = destination.display_name.clone());
        cosmic::task::future(async move {
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
        })
    }

    /// Acts on what a launch asked for, whether it started this process or
    /// arrived over D-Bus at one already running.
    fn launch(&mut self, launch: Option<&crate::flags::Launch>) {
        match launch {
            Some(crate::flags::Launch::Mailto(url)) => self.open_mailto(url),
            Some(crate::flags::Launch::Compose) => {
                let _ = self.act(Action::Compose);
            }
            None => {}
        }
    }

    /// Opens the composer on a `mailto:` link.
    fn open_mailto(&mut self, url: &str) {
        let Some(identity) = self
            .connection
            .as_ref()
            .and_then(|c| c.submission.as_ref())
            .map(|s| s.identity.clone())
        else {
            self.status = Some(fl!("no-from-address"));
            self.context_page = ContextPage::Accounts;
            self.core.window.show_context = true;
            return;
        };
        if let Some(draft) = crate::mailto::prefill(url, identity) {
            self.composer = Some(Composer::new(draft, None));
        }
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
            self.status = Some(fl!("no-from-address"));
            self.context_page = ContextPage::Accounts;
            self.core.window.show_context = true;
            return Task::none();
        };

        // A reply goes out as the identity the original was addressed to —
        // answering mail sent to an alias from the primary address outs the
        // alias. A fresh compose stays on the primary, whatever is open.
        if match_recipient
            && let (Some(connection), Some(opened)) =
                (self.connection.as_ref(), self.opened.as_ref())
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
        let answering = self
            .current_folder()
            .cloned()
            .zip(self.opened.as_ref().map(|o| o.uid));

        let draft = build(self.opened.as_ref(), identity);
        self.composer = Some(Composer::new(draft, answering));
        Task::none()
    }

    /// Closes the composer, keeping what was typed unless told not to.
    ///
    /// Keeping is the default because the cost of the two mistakes is not
    /// symmetric: a stray draft is a line in a list, and a discarded one is
    /// gone. Discarding is a separate button that says what it does.
    fn close_composer(&mut self, keep: bool) -> Task<Message> {
        let Some(composer) = self.composer.take() else {
            return Task::none();
        };
        let Some(connection) = self.connection.as_ref() else {
            return Task::none();
        };

        if !keep {
            if let Some(id) = composer.draft_id.as_deref()
                && let Err(why) = mail::delete_draft(connection, id)
            {
                self.status = Some(why);
            }
            // The discard left a tombstone if the draft was mirrored; the
            // sweep retires the server copy now rather than at the next poll.
            return Task::batch([self.reload_drafts(), self.sweep_drafts_now()]);
        }

        if !composer.is_worth_saving() {
            return Task::none();
        }
        match mail::save_draft(
            connection,
            composer.draft_id.as_deref(),
            &composer.resolved(),
        ) {
            Ok(_) => Task::batch([self.reload_drafts(), self.sweep_drafts_now()]),
            Err(why) => {
                // The composer is already closed, so this cannot be shown
                // beside the text it lost. Saying so in the status line is the
                // least bad thing available, and it is why saving is also
                // possible before closing.
                self.status = Some(fl!("draft-not-saved", reason = why));
                Task::none()
            }
        }
    }

    /// Leaves the open message's list, by the least ceremonious route it
    /// offers.
    fn unsubscribe(&mut self) -> Task<Message> {
        let Some(route) = self
            .opened
            .as_ref()
            .and_then(|opened| mail::unsubscribe_route(&opened.message))
        else {
            return Task::none();
        };
        match route {
            mail::Unsubscribe::OneClick(url) => {
                self.status = Some(fl!("unsubscribing"));
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
            mail::Unsubscribe::Mailto(url) => {
                self.open_mailto(&url);
                Task::none()
            }
            mail::Unsubscribe::Browser(url) => {
                if let Err(why) = open::that_detached(&url) {
                    self.status = Some(why.to_string());
                }
                Task::none()
            }
        }
    }

    /// Saves one attachment of the open message.
    fn save_attachment(&mut self, index: usize) -> Task<Message> {
        let (Some(connection), Some(folder), Some(uid)) = (
            self.connection.clone(),
            self.current_folder().cloned(),
            self.opened.as_ref().map(|opened| opened.uid),
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
            self.status = Some(why);
        }
    }

    /// Loads the rules and rebuilds the move dropdown before the page shows.
    fn open_rules_page(&mut self) {
        if let Some(connection) = self.connection.as_ref() {
            match mail::load_rules(connection) {
                Ok(rules) => self.rules = rules,
                Err(why) => self.status = Some(why),
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
            self.status = Some(fl!("no-folder-selected"));
            return None;
        };
        if folder.special_use.is_some() || folder.wire_name.eq_ignore_ascii_case("INBOX") {
            self.status = Some(fl!("folder-is-special", name = folder.display_name));
            return None;
        }
        Some(folder)
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
        let email = self.sign_in_email.trim().to_owned();
        if email.is_empty() || !email.contains('@') {
            self.status = Some(fl!("sign-in-needs-address"));
            return Task::none();
        }
        self.signing_in = true;
        self.status = Some(fl!("sign-in-browser"));
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
        const WATCH: std::time::Duration = std::time::Duration::from_secs(4 * 60);

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
    fn typing(&self) -> bool {
        self.text_focus > 0
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

        // The move picker's arrows, same mechanism as the palette's below.
        if let Some(FolderDialog::Move { selected, .. }) = self.folder_dialog.as_mut() {
            use cosmic::iced::keyboard::key::Named;
            let last = self.move_rows.len().saturating_sub(1);
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
        match action {
            Action::Compose => self.update(Message::Compose),
            Action::Reply => self.update(Message::Reply { all: false }),
            Action::ReplyAll => self.update(Message::Reply { all: true }),
            Action::Forward => self.update(Message::Forward),
            Action::Send => {
                if self.composer.is_some() {
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

            Action::ImportMbox => self.update(Message::ImportMbox),
            Action::Undo => self.undo(),
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
            Action::Shortcuts => self.update(Message::ToggleContextPage(ContextPage::Shortcuts)),
            Action::Settings => self.update(Message::ToggleContextPage(ContextPage::Settings)),
            Action::Accounts => self.update(Message::ToggleContextPage(ContextPage::Accounts)),
            Action::About => self.update(Message::ToggleContextPage(ContextPage::About)),
        }
    }

    /// Takes back the most recent reversible action.
    fn undo(&mut self) -> Task<Message> {
        let Some(entry) = self.undo_stack.pop() else {
            self.status = Some(fl!("nothing-to-undo"));
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
            self.status = Some(fl!("no-such-folder"));
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
        if self.composer.is_some() {
            return self.update(Message::ComposeCancel);
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
            self.status = Some(fl!("hit-folder-gone"));
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

    /// Reopens a saved draft in the composer.
    fn open_draft(&mut self, id: &str) -> Task<Message> {
        let Some(connection) = self.connection.as_ref() else {
            return Task::none();
        };
        match mail::load_draft(connection, id) {
            Ok(Some(draft)) => {
                let mut composer = Composer::new(draft, None);
                // Carried, so re-saving replaces this draft rather than
                // leaving the old one beside a new one.
                composer.draft_id = Some(id.to_owned());
                self.composer = Some(composer);
            }
            Ok(None) => self.status = Some(fl!("draft-gone")),
            Err(why) => self.status = Some(why),
        }
        Task::none()
    }

    fn with_composer(&mut self, edit: impl FnOnce(&mut Composer)) -> Task<Message> {
        if let Some(composer) = self.composer.as_mut() {
            edit(composer);
            composer.error = None;
        }
        Task::none()
    }

    fn send_draft(&mut self) -> Task<Message> {
        let (Some(composer), Some(connection)) = (self.composer.as_mut(), self.connection.clone())
        else {
            return Task::none();
        };
        if composer.sending {
            return Task::none();
        }
        let draft = composer.resolved();
        if let Some(problem) = draft.problem() {
            composer.error = Some(problem.to_owned());
            return Task::none();
        }
        composer.sending = true;
        composer.error = None;

        // With a grace configured, sending is scheduling: the message waits
        // out the delay in the outbox, where undo can still reach it.
        let grace = self.config.send_delay();
        if grace > 0 {
            let not_before = chrono::Utc::now().timestamp_millis() + i64::from(grace) * 1_000;
            return self.schedule_send_at(not_before, true);
        }

        let folders = self.folders.clone();
        let answering = composer.answering.clone();
        let draft_id = composer.draft_id.clone();

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
        let (Some(composer), Some(connection)) = (self.composer.as_mut(), self.connection.clone())
        else {
            return Task::none();
        };
        let draft = composer.resolved();
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
        let Some(composer) = self.composer.as_mut() else {
            return Task::none();
        };
        composer.sending = false;
        match sent {
            mail::Sent::Ok { filed } => {
                // Closed only on success. A failed send that discarded what the
                // user wrote would be unforgivable, and is the whole reason the
                // composer stays open below.
                self.composer = None;
                self.status = Some(if filed {
                    fl!("sent")
                } else {
                    fl!("sent-not-filed")
                });
                // The sweep retires the sent draft's server mirror.
                return Task::batch([self.reload_drafts(), self.sweep_drafts_now()]);
            }
            mail::Sent::Queued => {
                // Closed, because the message is no longer the user's problem:
                // it is queued, durable, and will go out on the next check.
                self.composer = None;
                self.status = Some(fl!("send-queued"));
                return self.reload_outbox();
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

    #[test]
    fn a_malformed_directory_name_is_skipped_rather_than_guessed_at() {
        assert!(unescape_local_name("%ZZ").is_none());
        assert!(unescape_local_name("truncated%").is_none());
    }
}
