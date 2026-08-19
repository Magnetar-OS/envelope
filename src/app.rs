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
use cosmic::iced::Subscription;
use cosmic::iced::Length;
use cosmic::widget::{self, about::About, menu};
use cosmic::{Apply as _, Element};
use cosmic_pim_accounts::{Account, AccountStore, MailEndpoint, Transport};
use cosmic_pim_mail::folder::{Folder, SpecialUse};
use cosmic_pim_mail::model::Flags;

use crate::fl;
use crate::mail::{self, Connection, Conversation, Opened, SyncReport};

const APP_ID: &str = "io.github.entro314labs.Envelope";
const REPOSITORY: &str = "https://github.com/entro314-labs/envelope";

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
    /// Local drafts for the selected account, newest first.
    drafts: Vec<cosmic_pim_mail::drafts::Saved>,
    showing_drafts: bool,

    /// What is in the search box. Empty means the box is closed.
    search: String,
    results: Vec<cosmic_pim_mail::Hit>,
    searching: bool,
}

/// How often the mailbox is checked.
///
/// A poll, not IDLE. IDLE is the right answer and needs a connection held open
/// on its own thread; until that exists, two minutes is the interval that keeps
/// a mail client feeling live without being the reason a laptop's radio never
/// sleeps. It is also short enough that the writeback queue drains promptly —
/// a flag change made offline reaches the server on the next tick, not the next
/// time somebody presses a button.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(120);

/// How many search results are shown.
///
/// A cap rather than paging: somebody who gets 500 hits needs a better query,
/// not a second page, and the honest answer to "there are more" is to say so.
const SEARCH_LIMIT: usize = 200;

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
            error: None,
        }
    }

    #[must_use]
    pub fn transport_index(&self) -> usize {
        transport_index(self.transport)
    }

    #[must_use]
    pub fn smtp_transport_index(&self) -> usize {
        transport_index(self.smtp_transport)
    }
}

fn transport_index(transport: Transport) -> usize {
    crate::ui::accounts::TRANSPORTS
        .iter()
        .position(|t| *t == transport)
        .unwrap_or(0)
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
    /// A local mutation finished; reload the folder either way.
    Mutated(Result<(), String>),

    MailFormStart(String),
    MailFormHostChanged(String),
    MailFormPortChanged(String),
    MailFormTransportChanged(Transport),
    MailFormUsernameChanged(String),
    MailFormSmtpHostChanged(String),
    MailFormSmtpPortChanged(String),
    MailFormSmtpTransportChanged(Transport),
    MailFormFromAddressChanged(String),
    MailFormFromNameChanged(String),
    MailFormCancel,
    MailFormSave,

    Compose,
    Reply { all: bool },
    Forward,
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
    SearchChanged(String),
    SearchFinished(Vec<cosmic_pim_mail::Hit>),
    SearchCleared,
    /// Open a search hit, which may be in a folder other than the current one.
    HitOpened(usize),

    SaveAttachment(usize),
    AttachmentSaved(Result<std::path::PathBuf, String>),
    /// Attach a file the user picked.
    AttachFile,
    FilePicked(Option<Vec<std::path::PathBuf>>),
    AttachmentRemoved(usize),
    DraftOpened(String),
    DraftDeleted(String),
    ComposeSend,
    ComposeSent(Box<crate::mail::Sent>),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
    Accounts,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    About,
    Accounts,
    Compose,
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            Self::About => Message::ToggleContextPage(ContextPage::About),
            Self::Accounts => Message::ToggleContextPage(ContextPage::Accounts),
            Self::Compose => Message::Compose,
        }
    }
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    /// A `mailto:` URL the desktop launched us with, if any.
    type Flags = Option<String>;
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, mailto: Self::Flags) -> (Self, Task<Self::Message>) {
        let about = About::default()
            .name(fl!("app-title"))
            .version(env!("CARGO_PKG_VERSION"))
            .license("GPL-3.0-only")
            .links([("Repository", REPOSITORY)]);

        let accounts = load_accounts();
        let selected_account = accounts.first().map(|a| a.id.clone());

        let mut model = Self {
            core,
            about,
            context_page: ContextPage::default(),
            key_binds: HashMap::new(),
            accounts,
            selected_account,
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
            status: None,
            list_error: None,
            reader_error: None,
            mail_form: None,
            composer: None,
            drafts: Vec::new(),
            showing_drafts: false,
            search: String::new(),
            results: Vec::new(),
            searching: false,
        };
        model.rebuild_connection();

        // A link the desktop handed us opens straight into the composer. Done
        // before the folder load so the user sees what they clicked on rather
        // than an inbox that turns into a composer a moment later.
        if let Some(url) = mailto {
            model.open_mailto(&url);
        }

        // The window is filled from disk first and the server is asked
        // afterwards, in that order: the mailbox is already there, so there is
        // no reason for the first frame to wait on a network round trip.
        let cached = model.load_cached_folders();
        let drafts = model.reload_drafts();
        let first_sync = model.sync_now();
        (model, Task::batch([cached, drafts, first_sync]))
    }

    fn header_end(&self) -> Vec<Element<'_, Self::Message>> {
        let search = widget::text_input(fl!("search"), &self.search)
            .on_input(Message::SearchChanged)
            .on_clear(Message::SearchCleared)
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
                        menu::Item::Button(fl!("compose"), None, MenuAction::Compose),
                        menu::Item::Divider,
                        menu::Item::Button(fl!("accounts"), None, MenuAction::Accounts),
                        menu::Item::Button(fl!("about"), None, MenuAction::About),
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
        }
        .view();
        Some(sidebar.map(cosmic::Action::App))
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        // Unconditional. Gating it on "is an account configured" would mean the
        // timer does not exist yet when one is added, and the first check would
        // wait for a restart.
        cosmic::iced::time::every(POLL_INTERVAL).map(|_| Message::Poll)
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
                    self.syncing,
                    self.status.as_deref(),
                ),
                Message::ToggleContextPage(ContextPage::Accounts),
            )
            .title(fl!("accounts")),
        })
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let list = if self.is_searching() {
            crate::ui::list::results(&self.results, &self.folders, self.searching, SEARCH_LIMIT)
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
            Some(composer) => crate::ui::composer::view(composer),
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
            .push(widget::container(right).width(Length::Fill).height(Length::Fill))
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
                self.selected_account = Some(id);
                self.clear_mailbox_state();
                self.rebuild_connection();
                Task::batch([self.load_cached_folders(), self.reload_drafts()])
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
                                self.selected_folder = self.default_folder();
                            }
                        }
                        self.reload_conversations()
                    }
                    Err(why) => {
                        self.status = Some(fl!("sync-failed", reason = why));
                        Task::none()
                    }
                }
            }

            Message::FolderSelected(index) => {
                self.showing_drafts = false;
                if self.selected_folder == Some(index) {
                    return Task::none();
                }
                self.selected_folder = Some(index);
                self.selected_conversation = None;
                self.opened = None;
                self.reader_error = None;
                self.reload_conversations()
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
                        // reaches the server rather than being a local lie.
                        if !already_read {
                            return self.set_flags(|flags| Flags { seen: true, ..flags });
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
                self.set_flags(move |flags| Flags { seen: !seen, ..flags })
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
                if let Err(why) = result {
                    self.status = Some(why);
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
                self.with_form(|form| form.from_address = address)
            }
            Message::MailFormFromNameChanged(name) => self.with_form(|form| form.from_name = name),
            Message::MailFormCancel => {
                self.mail_form = None;
                Task::none()
            }
            Message::MailFormSave => self.save_form(),

            Message::Compose => self.compose(|_, from| cosmic_pim_mail::Draft::new(from)),
            Message::Reply { all } => {
                self.compose(move |opened, from| match opened {
                    Some(opened) => cosmic_pim_mail::Draft::reply(&opened.message, from, all),
                    None => cosmic_pim_mail::Draft::new(from),
                })
            }
            Message::Forward => self.compose(|opened, from| match opened {
                Some(opened) => cosmic_pim_mail::Draft::forward(&opened.message, from),
                None => cosmic_pim_mail::Draft::new(from),
            }),

            Message::ComposeToChanged(text) => self.with_composer(|c| c.to = text),
            Message::ComposeCcChanged(text) => self.with_composer(|c| c.cc = text),
            Message::ComposeBccChanged(text) => self.with_composer(|c| c.bcc = text),
            Message::ComposeSubjectChanged(text) => {
                self.with_composer(|c| c.draft.subject = text)
            }
            Message::ComposeBodyChanged(text) => self.with_composer(|c| c.draft.body = text),
            Message::ComposeCancel => self.close_composer(true),
            Message::ComposeDiscard => self.close_composer(false),
            Message::DraftsLoaded(drafts) => {
                self.drafts = drafts;
                Task::none()
            }
            Message::ShowDrafts => {
                self.showing_drafts = !self.showing_drafts;
                if self.showing_drafts {
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
                let picked = cosmic::dialog::file_chooser::open::Dialog::new()
                    .open_files()
                    .await
                    .ok()
                    .map(|response| {
                        response
                            .urls()
                            .iter()
                            .filter_map(|url| url.to_file_path().ok())
                            .collect()
                    });
                Message::FilePicked(picked)
            }),
            Message::FilePicked(paths) => {
                let Some(paths) = paths else {
                    return Task::none();
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

            Message::DraftOpened(id) => self.open_draft(&id),
            Message::DraftDeleted(id) => {
                if let Some(connection) = self.connection.as_ref()
                    && let Err(why) = mail::delete_draft(connection, &id)
                {
                    self.status = Some(why);
                }
                self.reload_drafts()
            }
            Message::ComposeSend => self.send_draft(),
            Message::ComposeSent(sent) => {
                self.composer_finished(*sent);
                Task::none()
            }
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
            .or(if self.folders.is_empty() { None } else { Some(0) })
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
            Ok(Some(connection)) => self.connection = Some(connection),
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
                Some(cosmic_pim_mail::folder::from_list_entry(&wire, Some('/'), &[]))
            })
            .collect();
        cosmic_pim_mail::folder::sort_for_display(&mut folders);

        self.folders = folders;
        self.selected_folder = self.default_folder();
        self.reload_conversations()
    }

    fn reload_conversations(&mut self) -> Task<Message> {
        let (Some(connection), Some(folder)) = (self.connection.clone(), self.current_folder().cloned())
        else {
            self.conversations.clear();
            return Task::none();
        };
        self.loading_conversations = true;
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                mail::conversations(&connection, &folder)
            })
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
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || mail::open(&connection, &folder, uid))
                .await
                .unwrap_or_else(|why| Err(why.to_string()));
            Message::MessageOpened(Box::new(result))
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

        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                mail::set_flags(&connection, &folder, &[uid], edit)
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
        let (Some(connection), Some(folder), Some(index)) = (
            self.connection.clone(),
            self.current_folder().cloned(),
            self.selected_conversation,
        ) else {
            return Task::none();
        };
        let Some(destination) = mail::special(&self.folders, role).cloned() else {
            self.status = Some(fl!("no-archive-folder"));
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

        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                mail::move_to(&connection, &folder, &destination, &uids)
            })
            .await
            .unwrap_or_else(|why| Err(why.to_string()));
            Message::Mutated(result)
        })
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
        build: impl FnOnce(Option<&Opened>, cosmic_pim_mail::Mailbox) -> cosmic_pim_mail::Draft,
    ) -> Task<Message> {
        let Some(identity) = self
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
            return self.reload_drafts();
        }

        if !composer.is_worth_saving() {
            return Task::none();
        }
        match mail::save_draft(
            connection,
            composer.draft_id.as_deref(),
            &composer.resolved(),
        ) {
            Ok(_) => self.reload_drafts(),
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
        let (Some(composer), Some(connection)) =
            (self.composer.as_mut(), self.connection.clone())
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

        let folders = self.folders.clone();
        let answering = composer.answering.clone();
        let draft_id = composer.draft_id.clone();

        cosmic::task::future(async move {
            let sent = tokio::task::spawn_blocking(move || {
                mail::send(&connection, &draft, &folders, answering, draft_id.as_deref())
            })
            .await
            .unwrap_or_else(|why| mail::Sent::Uncertain(why.to_string()));
            Message::ComposeSent(Box::new(sent))
        })
    }

    fn composer_finished(&mut self, sent: mail::Sent) {
        let Some(composer) = self.composer.as_mut() else {
            return;
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

        let endpoint = MailEndpoint {
            imap_host: form.host.trim().to_owned(),
            imap_port: port,
            imap_transport: form.transport,
            imap_username: Some(form.username.trim().to_owned()).filter(|u| !u.is_empty()),
            smtp_host: form.smtp_host.trim().to_owned(),
            smtp_port,
            smtp_transport: form.smtp_transport,
            from_address: form.from_address.trim().to_owned(),
            from_name: form.from_name.trim().to_owned(),
        };
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

        let composer = Composer::new(
            cosmic_pim_mail::Draft::reply(&original, me, false),
            None,
        );
        assert_eq!(composer.to, "Ada <ada@example.com>");
        assert_eq!(composer.draft.subject, "Re: Plan");
        // The field text is what gets sent, not the draft's original list.
        assert_eq!(
            composer.resolved().to[0].address,
            "ada@example.com"
        );
    }

        #[test]
    fn a_malformed_directory_name_is_skipped_rather_than_guessed_at() {
        assert!(unescape_local_name("%ZZ").is_none());
        assert!(unescape_local_name("truncated%").is_none());
    }
}
