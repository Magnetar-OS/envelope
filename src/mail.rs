// SPDX-License-Identifier: GPL-3.0-only

//! The join between the application and the substrate.
//!
//! Everything here is blocking and knows nothing about widgets, which is what
//! lets [`app`](crate::app) run all of it on a worker thread and keep the UI
//! answering. It is deliberately thin: the rules about UIDVALIDITY, flag
//! ordering, and what a message *is* live in `cosmic-pim-mail`, where Slate and
//! Circle's equivalents live for their formats, and duplicating any of them
//! here would be the start of a second implementation.

use std::path::PathBuf;

use std::collections::{BTreeMap, HashMap};

use chrono::Utc;
use cosmic_pim_accounts::{Account, AccountStore, Transport};
use cosmic_pim_mail::attachment;
use cosmic_pim_mail::compose::Draft;
use cosmic_pim_mail::discovery::{self, Discovered};
use cosmic_pim_mail::drafts::{self, Drafts, Saved};
use cosmic_pim_mail::folder::{Folder, SpecialUse};
use cosmic_pim_mail::imap::{self, Endpoint, Security, Session, SyncOptions, Watched};
use cosmic_pim_mail::index::{self, Hit, Index};
use cosmic_pim_mail::maildir::{self, MaildirStore};
use cosmic_pim_mail::model::{Flags, Mailbox, Message};
use cosmic_pim_mail::outbox::{Outbox, Queued};
use cosmic_pim_mail::push::{PushOp, PushQueue};
use cosmic_pim_mail::sasl::Credentials;
use cosmic_pim_mail::smtp::{self, Outcome, SmtpEndpoint};
use cosmic_pim_mail::store::MailStore;

/// How often a cycle does the full-mailbox reconciliation pass.
///
/// The pass costs a round trip proportional to the mailbox and exists to notice
/// deletions another client made, which is not urgent. Every tenth cycle keeps
/// it cheap while still bounding how long a stale message can linger.
const RECONCILE_EVERY: u64 = 10;

/// One conversation, as the list shows it.
///
/// Re-exported from the substrate rather than redefined: the index already
/// assembles exactly this, and a second shape here would be a mapping layer
/// whose only job is to be kept in step.
pub use cosmic_pim_mail::index::Conversation;

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
    /// Messages that left the outbox on this pass.
    pub sent: usize,
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
    /// Where to submit outgoing mail. `None` when the account has no usable
    /// From identity — a composer that cannot say who a message is from is a
    /// composer that cannot send, and offering one anyway wastes what the user
    /// typed.
    pub submission: Option<Submission>,
    /// A password today; OAuth when the app grows a token flow. The substrate
    /// already speaks both, which is why this is the typed form rather than a
    /// string.
    pub credentials: Credentials,
    pub root: PathBuf,
    /// Where the conversation index lives.
    ///
    /// Carried on the connection rather than looked up at the point of use so
    /// that a root pointed somewhere else — a test, a second profile — cannot
    /// end up sharing a cache that describes a different mailbox.
    pub index_path: PathBuf,
}

/// Everything needed to send, as opposed to read.
#[derive(Debug, Clone)]
pub struct Submission {
    pub endpoint: SmtpEndpoint,
    /// Who mail is from. One credential serves both directions — SMTP AUTH uses
    /// the account's, whatever address is on the From line.
    pub identity: Mailbox,
}

impl SyncReport {
    /// Is there anything here a person needs to see?
    ///
    /// A quiet pass is the normal case, several times an hour. Saying "0 new"
    /// each time is how a status line stops being read.
    #[must_use]
    pub fn is_worth_reporting(&self) -> bool {
        self.fetched > 0
            || self.pushed > 0
            || self.sent > 0
            || !self.failures.is_empty()
            || self.stuck > 0
    }
}

impl Connection {
    /// Builds a connection for `account`, or explains what is missing.
    ///
    /// Returns `Ok(None)` rather than an error when the account simply has no
    /// mail endpoint: that is every account Slate created, and it is a prompt to
    /// fill one in, not a failure.
    pub fn for_account(accounts: &AccountStore, account: &Account) -> Result<Option<Self>, String> {
        let Some(mail) = account.mail.as_ref() else {
            return Ok(None);
        };
        let password = accounts
            .password(&account.id)
            .map_err(|why| format!("could not read the stored password: {why}"))?
            .ok_or_else(|| "no password is stored for this account".to_string())?;

        let submission = account.from_identity().map(|(name, address)| Submission {
            endpoint: SmtpEndpoint {
                host: mail.submission_host().to_owned(),
                port: mail.smtp_port,
                security: transport(mail.smtp_transport),
                username: account.mail_username().to_owned(),
            },
            identity: Mailbox {
                name: Some(name).filter(|n| !n.trim().is_empty()),
                address,
            },
        });

        Ok(Some(Self {
            account_id: account.id.clone(),
            submission,
            endpoint: Endpoint {
                host: mail.imap_host.clone(),
                port: mail.imap_port,
                security: transport(mail.imap_transport),
                username: account.mail_username().to_owned(),
            },
            credentials: Credentials::Password(password),
            root: maildir::default_root(),
            index_path: index::default_path(),
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
    let mut session = Session::connect(&connection.endpoint, &connection.credentials)
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

                // A renumbering does not make the index stale, it makes it
                // wrong: every UID in it now names a different message or
                // none. Dropping the rows is the only correct response, and
                // the next read rebuilds them from the refetched maildir.
                if outcome.renumbered
                    && let Err(why) = forget_index(connection, &folder.wire_name)
                {
                    report.failures.push((folder.display_name.clone(), why));
                }
            }
            Err(why) => report
                .failures
                .push((folder.display_name.clone(), why.to_string())),
        }
    }

    let _ = session.logout();

    // After the folder pass, so a message that just went out is filed into the
    // Sent copy this cycle already refreshed.
    match drain_outbox(connection, &folders) {
        Ok(sent) => report.sent = sent,
        Err(why) => report.failures.push(("outbox".to_string(), why)),
    }

    Ok(report)
}

/// Works out an account's servers from its address.
///
/// Blocking — it fetches and it probes — so it belongs on a worker thread. The
/// no-network half is [`known_settings`], which is cheap enough to call while
/// somebody is still typing.
pub fn discover(email: &str) -> Result<Discovered, String> {
    discovery::discover(email).map_err(|why| why.to_string())
}

/// The built-in table only: no network, no waiting.
#[must_use]
pub fn known_settings(email: &str) -> Option<Discovered> {
    discovery::known(email)
}

/// This account's outbox.
pub fn outbox(connection: &Connection) -> Result<Outbox, String> {
    Outbox::open(connection.root.join(&connection.account_id)).map_err(|why| why.to_string())
}

pub fn list_outbox(connection: &Connection) -> Result<Vec<Queued>, String> {
    outbox(connection)?.list().map_err(|why| why.to_string())
}

pub fn retry_queued(connection: &Connection, id: &str) -> Result<(), String> {
    outbox(connection)?.retry(id).map_err(|why| why.to_string())
}

pub fn discard_queued(connection: &Connection, id: &str) -> Result<(), String> {
    outbox(connection)?
        .remove(id)
        .map_err(|why| why.to_string())
}

/// Attempts everything in the outbox that is due, filing what goes out.
///
/// Called from the sync pass, so a message written offline leaves as soon as
/// the next check finds the network — without anybody remembering to press
/// anything, which is the whole reason the queue exists.
fn drain_outbox(connection: &Connection, folders: &[Folder]) -> Result<usize, String> {
    let Some(submission) = connection.submission.as_ref() else {
        return Ok(0);
    };
    let outbox = outbox(connection)?;
    let outcome = outbox
        .drain(&submission.endpoint, &connection.credentials, now_ms())
        .map_err(|why| why.to_string())?;

    for (id, filed) in &outcome.sent {
        tracing::info!(id, "a queued message was sent");
        file_to_sent(connection, folders, filed);
    }
    Ok(outcome.sent.len())
}

/// This account's local drafts.
///
/// Local, and Envelope says so in the sidebar. Saving to the server's Drafts
/// folder means APPEND, and without UIDPLUS the next sync pulls the draft back
/// down as a message the client cannot recognise as the one it just uploaded —
/// so every edit leaves another copy. See `cosmic_pim_mail::drafts` for the
/// whole argument; the short version is that a duplicated draft is worse than a
/// local one.
pub fn drafts(connection: &Connection) -> Result<Drafts, String> {
    Drafts::open(connection.root.join(&connection.account_id)).map_err(|why| why.to_string())
}

/// Saves a draft, minting an id if it does not have one yet.
pub fn save_draft(
    connection: &Connection,
    id: Option<&str>,
    draft: &Draft,
) -> Result<String, String> {
    let store = drafts(connection)?;
    let id = id
        .filter(|id| drafts::is_valid_id(id))
        .map_or_else(|| drafts::new_id(now_ms()), ToOwned::to_owned);
    store
        .save(&id, draft, now_ms())
        .map_err(|why| why.to_string())?;
    Ok(id)
}

/// Every saved draft, newest first.
pub fn list_drafts(connection: &Connection) -> Result<Vec<Saved>, String> {
    drafts(connection)?.list().map_err(|why| why.to_string())
}

/// Reopens one draft for editing.
pub fn load_draft(connection: &Connection, id: &str) -> Result<Option<Draft>, String> {
    let Some(identity) = connection.submission.as_ref().map(|s| s.identity.clone()) else {
        return Err("this account has no From address".into());
    };
    drafts(connection)?
        .load(id, identity)
        .map_err(|why| why.to_string())
}

pub fn delete_draft(connection: &Connection, id: &str) -> Result<(), String> {
    drafts(connection)?
        .delete(id)
        .map_err(|why| why.to_string())
}

/// Searches the account, applying the flag filters the index cannot.
///
/// The index answers the text half in SQL. Flags live in the store — they change
/// constantly and a UID's bytes never do, so caching them would mean a cache
/// write per read mark — which means `is:unread` and `is:starred` are applied
/// here, against the maildirs the hits actually came from.
pub fn search(
    connection: &Connection,
    folders: &[Folder],
    input: &str,
    limit: usize,
) -> Result<Vec<Hit>, String> {
    let query = cosmic_pim_mail::search::parse(input);
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let index = Index::open(&connection.index_path).map_err(|why| why.to_string())?;
    let hits = index
        .search(&connection.account_id, None, &query, limit)
        .map_err(|why| why.to_string())?;

    if !query.unread && !query.starred {
        return Ok(hits);
    }

    // Only the mailboxes the hits are actually in get opened, and each one only
    // once: a flag filter must not cost a walk of every folder on the server.
    let mut flags: HashMap<String, BTreeMap<u32, Flags>> = HashMap::new();
    let mut kept = Vec::with_capacity(hits.len());
    for hit in hits {
        let entry = match flags.get(&hit.mailbox) {
            Some(entry) => entry,
            None => {
                let Some(folder) = folders.iter().find(|f| f.wire_name == hit.mailbox) else {
                    // A mailbox the server no longer lists. The index will drop
                    // it on the next sync; until then it cannot be filtered.
                    continue;
                };
                let loaded = MaildirStore::open(connection.mailbox_path(folder))
                    .and_then(|store| store.state())
                    .map(|state| state.entries)
                    .unwrap_or_default();
                flags.entry(hit.mailbox.clone()).or_insert(loaded)
            }
        };
        let hit_flags = entry.get(&hit.uid).copied().unwrap_or_default();
        if (query.unread && hit_flags.seen) || (query.starred && !hit_flags.flagged) {
            continue;
        }
        kept.push(hit);
    }
    Ok(kept)
}

/// Where a saved attachment goes.
///
/// `$XDG_DOWNLOAD_DIR`, the same place a browser puts one, so the user does not
/// have to learn a second answer to "where did it go". A file picker is the
/// better long-term answer and needs the desktop portal.
#[must_use]
pub fn downloads() -> PathBuf {
    dirs::download_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
}

/// Saves one of an open message's attachments, returning where it landed.
///
/// Where it *landed*, not where it was asked to go: the name is sanitised and a
/// collision gets a counter, so "saved to Downloads" would be unhelpful if the
/// file is actually `report (3).pdf`.
pub fn save_attachment(
    connection: &Connection,
    folder: &Folder,
    uid: u32,
    index: usize,
) -> Result<PathBuf, String> {
    let store =
        MaildirStore::open(connection.mailbox_path(folder)).map_err(|why| why.to_string())?;
    let raw = store
        .raw(uid)
        .map_err(|why| why.to_string())?
        .ok_or_else(|| "that message is no longer in this mailbox".to_string())?;

    let message =
        Message::parse(&raw).ok_or_else(|| "that message could not be read".to_string())?;
    let attachment = message
        .attachments
        .get(index)
        .ok_or_else(|| "that attachment is not in the message".to_string())?;

    let bytes = attachment::bytes_of(&raw, index).map_err(|why| why.to_string())?;
    attachment::save_into(&downloads(), &attachment.name, &bytes).map_err(|why| why.to_string())
}

/// How a watch ended, as the app needs to hear it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchOutcome {
    /// The server reported news. Sync, then watch again.
    Changed,
    /// Nothing happened within the timeout. Watch again.
    TimedOut,
    /// The server has no IDLE. Do not watch again — the poll already covers
    /// this account, and retrying would reconnect forever to hear the same no.
    Unsupported,
}

/// Parks a dedicated connection in IMAP IDLE on the inbox until something
/// happens.
///
/// Blocking for up to `timeout`, so strictly a worker-thread call. A dedicated
/// connection because IDLE monopolises the one it is on — sharing it with the
/// sync session would make every cycle wait for the watch.
///
/// Only the inbox. One watched mailbox is one connection, and new mail lands
/// in INBOX; a change anywhere else is the kind of news the periodic
/// reconciliation exists for. `INBOX` is the one name RFC 3501 guarantees.
pub fn watch_inbox(
    connection: &Connection,
    timeout: std::time::Duration,
) -> Result<WatchOutcome, String> {
    let mut session = Session::connect(&connection.endpoint, &connection.credentials)
        .map_err(|why| why.to_string())?;
    if !session.supports_idle() {
        let _ = session.logout();
        return Ok(WatchOutcome::Unsupported);
    }
    let outcome = session
        .watch("INBOX", timeout)
        .map_err(|why| why.to_string())?;
    let _ = session.logout();
    Ok(match outcome {
        Watched::Changed => WatchOutcome::Changed,
        Watched::TimedOut => WatchOutcome::TimedOut,
    })
}

/// Drops one mailbox's index rows.
fn forget_index(connection: &Connection, mailbox: &str) -> Result<(), String> {
    let mut index = Index::open(&connection.index_path).map_err(|why| why.to_string())?;
    index
        .forget(&connection.account_id, mailbox)
        .map_err(|why| why.to_string())
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
///
/// The expensive half — parsing — happens in the index and only for messages it
/// has not seen. A folder that has not changed since the last look costs a
/// query, which is what makes clicking between folders feel like navigation
/// rather than loading.
pub fn conversations(
    connection: &Connection,
    folder: &Folder,
) -> Result<Vec<Conversation>, String> {
    let path = connection.mailbox_path(folder);
    if !path.join("cur").is_dir() {
        return Ok(Vec::new());
    }
    let store = MaildirStore::open(&path).map_err(|why| why.to_string())?;
    let flags = store.state().map_err(|why| why.to_string())?.entries;

    let mut index = Index::open(&connection.index_path).map_err(|why| why.to_string())?;
    index
        .sync_mailbox(&connection.account_id, &folder.wire_name, &store)
        .map_err(|why| why.to_string())?;
    index
        .conversations(&connection.account_id, &folder.wire_name, &flags)
        .map_err(|why| why.to_string())
}

/// Opens one message for the reader.
pub fn open(connection: &Connection, folder: &Folder, uid: u32) -> Result<Opened, String> {
    let store = MaildirStore::open(connection.mailbox_path(folder)).map_err(|w| w.to_string())?;
    let raw = store
        .raw(uid)
        .map_err(|why| why.to_string())?
        .ok_or_else(|| "that message is no longer in this mailbox".to_string())?;
    let message =
        Message::parse(&raw).ok_or_else(|| "that message could not be read".to_string())?;
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

/// The same three choices, spelled in `accounts` and in `mail` because
/// `accounts` sits below every protocol crate. This is the boundary where they
/// meet, and it is the only place either spelling appears twice.
fn transport(transport: Transport) -> Security {
    match transport {
        Transport::Tls => Security::Tls,
        Transport::StartTls => Security::StartTls,
        Transport::Plaintext => Security::Plaintext,
    }
}

/// What happened to a send, as the composer needs to hear it.
#[derive(Debug, Clone)]
pub enum Sent {
    /// Delivered, and filed to Sent if there was somewhere to file it.
    Ok { filed: bool },
    /// Not delivered, but definitely not delivered — so it is in the outbox and
    /// will go out on its own.
    Queued,
    /// Definitely not delivered. The draft is intact and may be sent again.
    Failed(String),
    /// It may or may not have been delivered.
    ///
    /// Carried separately from [`Self::Failed`] all the way to the user,
    /// because the two need different words: one says "try again", the other
    /// says "check before you do".
    Uncertain(String),
}

/// Sends a draft, files the Sent copy, and marks the message it answers.
///
/// Blocking, for a worker thread. Everything after the send itself is
/// best-effort: a message that reached its recipients has succeeded, and
/// reporting failure because the Sent copy could not be filed would be telling
/// the user something untrue about the part they care about.
pub fn send(
    connection: &Connection,
    draft: &Draft,
    folders: &[Folder],
    answering: Option<(Folder, u32)>,
    draft_id: Option<&str>,
) -> Sent {
    let Some(submission) = connection.submission.as_ref() else {
        return Sent::Failed("this account has no From address".into());
    };

    let filed_bytes = match smtp::send(&submission.endpoint, &connection.credentials, draft) {
        Outcome::Sent(bytes) => bytes,
        // Definitely not delivered, so it can wait for the network rather than
        // for the user. An ambiguous failure is not queued — it may already
        // have arrived, and the queue would send it twice.
        outcome @ Outcome::NotSent(_) => {
            let id = draft_id
                .filter(|id| cosmic_pim_mail::drafts::is_valid_id(id))
                .map_or_else(|| drafts::new_id(now_ms()), ToOwned::to_owned);
            return match outbox(connection).and_then(|outbox| {
                outbox
                    .queue(&id, draft, &outcome, now_ms())
                    .map_err(|why| why.to_string())
            }) {
                // The draft becomes the queued message; leaving both would show
                // it twice and send it once.
                Ok(()) => {
                    if let Some(previous) = draft_id
                        && let Err(why) = delete_draft(connection, previous)
                    {
                        tracing::warn!(%why, "a queued message left its draft behind");
                    }
                    Sent::Queued
                }
                Err(why) => Sent::Failed(why),
            };
        }
        Outcome::Ambiguous(why) => return Sent::Uncertain(why.to_string()),
    };

    // Marking the original answered is local and queued, so it works offline
    // and survives a restart — the same path every other flag change takes.
    if let Some((folder, uid)) = answering
        && let Err(why) = set_flags(connection, &folder, &[uid], |flags| Flags {
            answered: true,
            ..flags
        })
    {
        tracing::warn!(%why, "the message was sent but not marked as answered");
    }

    // Only once it is away. A draft deleted before the send would be lost by
    // a failure, which is the one thing the draft was there to prevent.
    if let Some(id) = draft_id
        && let Err(why) = delete_draft(connection, id)
    {
        tracing::warn!(%why, "the message was sent but its draft was left behind");
    }

    let filed = file_to_sent(connection, folders, &filed_bytes);
    Sent::Ok { filed }
}

/// Puts the sent copy in the Sent folder, on the server and on disk.
///
/// Returns whether it landed. Most servers do not file SMTP-submitted mail
/// themselves, so without this a sent message simply never appears anywhere the
/// user can see it.
fn file_to_sent(connection: &Connection, folders: &[Folder], raw: &[u8]) -> bool {
    let Some(sent) = special(folders, SpecialUse::Sent) else {
        tracing::info!("the server has no Sent folder; the copy was not filed");
        return false;
    };

    match Session::connect(&connection.endpoint, &connection.credentials).and_then(|mut session| {
        let result = session.append(
            &sent.wire_name,
            raw,
            Flags {
                seen: true,
                ..Flags::default()
            },
        );
        let _ = session.logout();
        result
    }) {
        Ok(()) => true,
        Err(why) => {
            // Not fatal: the message was delivered, which is the part that
            // cannot be undone. The next sync of Sent will not find it, and
            // that is a visible gap rather than a silent one.
            tracing::warn!(%why, "the message was sent but the Sent copy was not filed");
            false
        }
    }
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
