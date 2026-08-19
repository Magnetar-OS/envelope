// SPDX-License-Identifier: GPL-3.0-only

//! The join between the application and the substrate.
//!
//! Everything here is blocking and knows nothing about widgets, which is what
//! lets [`app`](crate::app) run all of it on a worker thread and keep the UI
//! answering. It is deliberately thin: the rules about UIDVALIDITY, flag
//! ordering, and what a message *is* live in `cosmic-pim-mail`, where Slate and
//! Circle's equivalents live for their formats, and duplicating any of them
//! here would be the start of a second implementation.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use cosmic_pim_accounts::{Account, AccountStore, Transport};
use cosmic_pim_mail::folder::{Folder, SpecialUse};
use cosmic_pim_mail::imap::{self, Endpoint, Security, Session, SyncOptions};
use cosmic_pim_mail::maildir::{self, MaildirStore};
use cosmic_pim_mail::model::{Flags, Message};
use cosmic_pim_mail::push::{PushOp, PushQueue};
use cosmic_pim_mail::store::MailStore;

/// How often a cycle does the full-mailbox reconciliation pass.
///
/// The pass costs a round trip proportional to the mailbox and exists to notice
/// deletions another client made, which is not urgent. Every tenth cycle keeps
/// it cheap while still bounding how long a stale message can linger.
const RECONCILE_EVERY: u64 = 10;

/// One conversation, summarised for the list.
#[derive(Debug, Clone)]
pub struct Conversation {
    pub thread_id: String,
    pub subject: String,
    /// Who is in it, in the order they appear, deduplicated.
    pub participants: String,
    pub date: Option<DateTime<Utc>>,
    pub snippet: String,
    /// UIDs, oldest first.
    pub uids: Vec<u32>,
    pub unread: bool,
    pub flagged: bool,
    pub has_attachments: bool,
}

impl Conversation {
    #[must_use]
    pub fn newest_uid(&self) -> Option<u32> {
        self.uids.last().copied()
    }
}

/// A message opened in the reader.
#[derive(Debug, Clone)]
pub struct Opened {
    pub uid: u32,
    pub message: Message,
    pub flags: Flags,
    /// The `Authentication-Results` rollup, or empty when the message carried
    /// none.
    pub auth: &'static str,
}

/// What one sync pass did, as the status line reports it.
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub folders: Vec<Folder>,
    pub fetched: usize,
    pub pushed: usize,
    /// Mailboxes that could not be synced, with the server's reason.
    pub failures: Vec<(String, String)>,
    /// Writes that will not move on their own — bad credentials, no permission,
    /// a renumbering mid-flight.
    pub stuck: usize,
}

/// Everything needed to reach one account's mail.
#[derive(Debug, Clone)]
pub struct Connection {
    pub account_id: String,
    pub endpoint: Endpoint,
    pub password: String,
    pub root: PathBuf,
}

impl Connection {
    /// Builds a connection for `account`, or explains what is missing.
    ///
    /// Returns `Ok(None)` rather than an error when the account simply has no
    /// mail endpoint: that is every account Slate created, and it is a prompt to
    /// fill one in, not a failure.
    pub fn for_account(
        accounts: &AccountStore,
        account: &Account,
    ) -> Result<Option<Self>, String> {
        let Some(mail) = account.mail.as_ref() else {
            return Ok(None);
        };
        let password = accounts
            .password(&account.id)
            .map_err(|why| format!("could not read the stored password: {why}"))?
            .ok_or_else(|| "no password is stored for this account".to_string())?;

        Ok(Some(Self {
            account_id: account.id.clone(),
            endpoint: Endpoint {
                host: mail.imap_host.clone(),
                port: mail.imap_port,
                security: match mail.imap_transport {
                    Transport::Tls => Security::Tls,
                    Transport::StartTls => Security::StartTls,
                    Transport::Plaintext => Security::Plaintext,
                },
                username: account.mail_username().to_owned(),
            },
            password,
            root: maildir::default_root(),
        }))
    }

    fn mailbox_path(&self, folder: &Folder) -> PathBuf {
        maildir::mailbox_path(&self.root, &self.account_id, folder)
    }
}

/// Runs one sync pass over every selectable folder.
///
/// Blocking, and meant for a worker thread. A failure in one mailbox is
/// recorded and the pass continues — the same per-collection error isolation
/// `cosmic-pim-sync` gives calendars, and for the same reason: one broken
/// folder must not cost the user the other nineteen.
pub fn sync(connection: &Connection, cycle: u64) -> Result<SyncReport, String> {
    let mut session = Session::connect(&connection.endpoint, &connection.password)
        .map_err(|why| why.to_string())?;

    let folders = session.folders().map_err(|why| why.to_string())?;
    let mut report = SyncReport {
        folders: folders.clone(),
        ..SyncReport::default()
    };

    let options = SyncOptions {
        reconcile: cycle.is_multiple_of(RECONCILE_EVERY),
        since_ms: None,
    };

    for folder in folders.iter().filter(|f| !f.no_select) {
        let path = connection.mailbox_path(folder);
        let mut store = match MaildirStore::open(&path) {
            Ok(store) => store,
            Err(why) => {
                report
                    .failures
                    .push((folder.display_name.clone(), why.to_string()));
                continue;
            }
        };
        match imap::sync_mailbox(
            &mut session,
            &folder.wire_name,
            &mut store,
            options,
            now_ms(),
        ) {
            Ok(outcome) => {
                report.fetched += outcome.fetched;
                report.pushed += outcome.pushed.succeeded;
                report.stuck += outcome.pushed.needs_reconcile + outcome.pushed.needs_user;
            }
            Err(why) => report
                .failures
                .push((folder.display_name.clone(), why.to_string())),
        }
    }

    let _ = session.logout();
    Ok(report)
}

/// Every folder we have a maildir for, without touching the network.
///
/// This is what the sidebar shows before the first sync of a session finishes:
/// the mailbox is on disk, so there is no reason to stare at an empty window
/// waiting for a server.
pub fn cached_folders(connection: &Connection, folders: &[Folder]) -> Vec<Folder> {
    folders
        .iter()
        .filter(|folder| connection.mailbox_path(folder).join("cur").is_dir())
        .cloned()
        .collect()
}

/// Reads one mailbox off disk and groups it into conversations, newest first.
pub fn conversations(connection: &Connection, folder: &Folder) -> Result<Vec<Conversation>, String> {
    let path = connection.mailbox_path(folder);
    if !path.join("cur").is_dir() {
        return Ok(Vec::new());
    }
    let store = MaildirStore::open(&path).map_err(|why| why.to_string())?;
    let flags = store.state().map_err(|why| why.to_string())?.entries;

    let threads = imap::thread_mailbox(&store, &connection.account_id, &folder.wire_name)
        .map_err(|why| why.to_string())?;

    let mut conversations: Vec<Conversation> = threads
        .into_iter()
        .filter_map(|(thread_id, uids)| {
            summarise(&store, &flags, thread_id, uids)
        })
        .collect();

    // Newest first, and undated messages last rather than first: a message with
    // no parseable Date is a curiosity, not the most important thing the user
    // owns.
    conversations.sort_by_key(|c| std::cmp::Reverse(c.date));
    Ok(conversations)
}

fn summarise(
    store: &MaildirStore,
    flags: &BTreeMap<u32, Flags>,
    thread_id: String,
    uids: Vec<u32>,
) -> Option<Conversation> {
    let mut participants: Vec<String> = Vec::new();
    let mut subject = String::new();
    let mut snippet = String::new();
    let mut date = None;
    let mut has_attachments = false;

    for uid in &uids {
        let Ok(Some(raw)) = store.raw(*uid) else {
            continue;
        };
        let Some(message) = Message::parse(&raw) else {
            continue;
        };
        if let Some(sender) = message.sender() {
            let name = sender.display().to_owned();
            if !participants.contains(&name) {
                participants.push(name);
            }
        }
        has_attachments |= message.attachments.iter().any(|a| !a.inline);
        // The thread's title is its oldest message's subject with the Re:
        // prefixes off, so a long conversation does not rename itself every
        // time somebody's client spells the prefix differently.
        if subject.is_empty() && !message.subject_norm.is_empty() {
            subject = message.subject_norm.clone();
        }
        // The snippet and date come from the newest, which is what the user is
        // deciding whether to read.
        if message.date >= date || date.is_none() {
            date = message.date.or(date);
            snippet = message.body.text.lines().next().unwrap_or("").to_owned();
        }
    }

    if subject.is_empty() {
        subject = "(no subject)".to_owned();
    }

    Some(Conversation {
        unread: uids.iter().any(|uid| !flags.get(uid).is_some_and(|f| f.seen)),
        flagged: uids.iter().any(|uid| flags.get(uid).is_some_and(|f| f.flagged)),
        participants: participants.join(", "),
        thread_id,
        subject,
        date,
        snippet,
        uids,
        has_attachments,
    })
}

/// Opens one message for the reader.
pub fn open(connection: &Connection, folder: &Folder, uid: u32) -> Result<Opened, String> {
    let store = MaildirStore::open(connection.mailbox_path(folder)).map_err(|w| w.to_string())?;
    let raw = store
        .raw(uid)
        .map_err(|why| why.to_string())?
        .ok_or_else(|| "that message is no longer in this mailbox".to_string())?;
    let message = Message::parse(&raw).ok_or_else(|| "that message could not be read".to_string())?;
    let flags = store
        .state()
        .map_err(|why| why.to_string())?
        .entries
        .get(&uid)
        .copied()
        .unwrap_or_default();

    Ok(Opened {
        auth: cosmic_pim_mail::auth::rollup(&message.auth),
        uid,
        message,
        flags,
    })
}

/// Applies a flag change locally and queues it for the server.
///
/// Both halves, in that order, and neither is optional. Local-only would revert
/// on the next sync; queue-only would leave the UI showing the old state until
/// the network came back. The queue is durable, so a change made offline is
/// still a change.
pub fn set_flags(
    connection: &Connection,
    folder: &Folder,
    uids: &[u32],
    edit: impl Fn(Flags) -> Flags,
) -> Result<(), String> {
    let mut store =
        MaildirStore::open(connection.mailbox_path(folder)).map_err(|why| why.to_string())?;
    let current = store.state().map_err(|why| why.to_string())?.entries;

    for uid in uids {
        let Some(existing) = current.get(uid).copied() else {
            continue;
        };
        let updated = edit(existing);
        if updated == existing {
            continue;
        }
        store.set_flags(*uid, updated).map_err(|w| w.to_string())?;
        store
            .enqueue(PushOp::SetFlags {
                uid: *uid,
                flags: updated,
            })
            .map_err(|why| why.to_string())?;
    }
    Ok(())
}

/// Queues a move to another mailbox, and drops the messages locally.
///
/// The local removal is immediate on purpose: a message the user archived
/// should leave the list now, not when the network agrees. If the move fails
/// permanently the next reconciliation pass finds the message still on the
/// server and brings it back — which is the right failure, because the message
/// was never lost.
pub fn move_to(
    connection: &Connection,
    folder: &Folder,
    destination: &Folder,
    uids: &[u32],
) -> Result<(), String> {
    let mut store =
        MaildirStore::open(connection.mailbox_path(folder)).map_err(|why| why.to_string())?;
    for uid in uids {
        store
            .enqueue(PushOp::Move {
                uid: *uid,
                destination: destination.wire_name.clone(),
            })
            .map_err(|why| why.to_string())?;
        store.remove(*uid).map_err(|why| why.to_string())?;
    }
    Ok(())
}

/// The folder with a given role, if the server has one.
#[must_use]
pub fn special(folders: &[Folder], role: SpecialUse) -> Option<&Folder> {
    folders.iter().find(|f| f.special_use == Some(role))
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
