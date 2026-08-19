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
            error: None,
        }
    }

    #[must_use]
    pub fn transport_index(&self) -> usize {
        crate::ui::accounts::TRANSPORTS
            .iter()
            .position(|t| *t == self.transport)
            .unwrap_or(0)
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    LaunchUrl(String),
    ToggleContextPage(ContextPage),

    AccountSelected(String),
    SyncNow,
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
    MailFormCancel,
    MailFormSave,
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
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            Self::About => Message::ToggleContextPage(ContextPage::About),
            Self::Accounts => Message::ToggleContextPage(ContextPage::Accounts),
        }
    }
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
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
        };
        model.rebuild_connection();

        // Nothing is fetched at startup. The folders and messages are already
        // on disk, so the window fills immediately; syncing is something the
        // user asks for or a timer does, not a thing that makes the first
        // frame wait for a server.
        let task = model.load_cached_folders();
        (model, task)
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        vec![
            menu::bar(vec![menu::Tree::with_children(
                menu::root(fl!("app-title")).apply(Element::from),
                menu::items(
                    &self.key_binds,
                    vec![
                        menu::Item::Button(fl!("accounts"), None, MenuAction::Accounts),
                        menu::Item::Divider,
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
        }
        .view();
        Some(sidebar.map(cosmic::Action::App))
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
        let list = crate::ui::list::List {
            conversations: &self.conversations,
            selected: self.selected_conversation,
            loading: self.loading_conversations,
            error: self.list_error.as_deref(),
        }
        .view();

        let reader = crate::ui::reader::Reader {
            opened: self.opened.as_ref(),
            error: self.reader_error.as_deref(),
        }
        .view();

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
            .push(widget::container(reader).width(Length::Fill).height(Length::Fill))
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
                self.load_cached_folders()
            }

            Message::SyncNow => self.sync_now(),

            Message::SyncFinished(result) => {
                self.syncing = false;
                match *result {
                    Ok(report) => {
                        self.status = Some(summarise(&report));
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
            Message::MailFormCancel => {
                self.mail_form = None;
                Task::none()
            }
            Message::MailFormSave => self.save_form(),
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

        let endpoint = MailEndpoint {
            imap_host: form.host.trim().to_owned(),
            imap_port: port,
            imap_transport: form.transport,
            imap_username: Some(form.username.trim().to_owned()).filter(|u| !u.is_empty()),
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
    fn a_malformed_directory_name_is_skipped_rather_than_guessed_at() {
        assert!(unescape_local_name("%ZZ").is_none());
        assert!(unescape_local_name("truncated%").is_none());
    }
}
