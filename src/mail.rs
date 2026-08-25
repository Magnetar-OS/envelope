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
use cosmic_pim_accounts::{
    Account, AccountStore, AuthMethod, MailEndpoint, MailProtocol, Transport,
};
use cosmic_pim_mail::attachment;
use cosmic_pim_mail::compose::Draft;
use cosmic_pim_mail::discovery::{self, Discovered};
use cosmic_pim_mail::drafts::{self, Drafts, Saved};
use cosmic_pim_mail::folder::{Folder, SpecialUse};
use cosmic_pim_mail::imap::{Endpoint, Security, Session, SyncOptions, Watched};
use cosmic_pim_mail::index::{self, Hit, Index};
use cosmic_pim_mail::maildir::{self, MaildirStore};
use cosmic_pim_mail::model::{Flags, Mailbox, Message};
use cosmic_pim_mail::outbox::{Outbox, Queued};
use cosmic_pim_mail::push::{PushOp, PushQueue};
use cosmic_pim_mail::sasl::Credentials;
use cosmic_pim_mail::smtp::{self, Outcome, SmtpEndpoint};
use cosmic_pim_mail::store::{MailStore, RemoteMessage};

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
    /// The account itself: the sync dispatch needs the whole thing — which
    /// protocol, which endpoint, which login — not a summary of it.
    pub account: Account,
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
        // Cheap reads only — this runs while the window is being built. An
        // OAuth token that turns out to be expired is renewed by the worker
        // that hits the wall, through fresh_credentials, not here.
        let credentials = match account.auth {
            AuthMethod::Password => Credentials::Password(
                accounts
                    .password(&account.id)
                    .map_err(|why| format!("could not read the stored password: {why}"))?
                    .ok_or_else(|| "no password is stored for this account".to_string())?,
            ),
            AuthMethod::OAuth => Credentials::OAuth2(
                accounts
                    .credential(&account.id)
                    .map_err(|why| format!("could not read the sign-in: {why}"))?
                    .ok_or_else(|| "no sign-in is saved for this account".to_string())?
                    .access_token,
            ),
        };

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
            account: account.clone(),
            account_id: account.id.clone(),
            submission,
            endpoint: Endpoint {
                host: mail.imap_host.clone(),
                port: mail.imap_port,
                security: transport(mail.imap_transport),
                username: account.mail_username().to_owned(),
            },
            credentials,
            root: maildir::default_root(),
            index_path: index::default_path(),
        }))
    }

    fn mailbox_path(&self, folder: &Folder) -> PathBuf {
        maildir::mailbox_path(&self.root, &self.account_id, folder)
    }
}

/// A provider somebody can sign in to with a browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInProvider {
    pub id: String,
    pub name: String,
}

/// The providers OAuth sign-in can offer.
///
/// Only those with a client id configured: a Sign in button that ends at the
/// provider's "invalid client" page is worse than its absence, and the
/// registry knows which those are.
#[must_use]
pub fn sign_in_providers() -> Vec<SignInProvider> {
    cosmic_pim_accounts::Registry::load()
        .all()
        .into_iter()
        .filter(|provider| {
            provider
                .oauth
                .as_ref()
                .is_some_and(cosmic_pim_accounts::OAuth::is_configured)
        })
        .map(|provider| SignInProvider {
            id: provider.id.clone(),
            name: provider.name.clone(),
        })
        .collect()
}

/// Runs one OAuth sign-in, start to finish, and stores the account.
///
/// Blocking for up to the flow's five-minute redirect deadline, so strictly a
/// worker call. The browser is the user's own — opened here, next to the
/// listener, so the port is already bound when the redirect comes back.
/// Everything else — PKCE, the loopback listener, the code exchange — is the
/// substrate's.
///
/// Returns the new account's id, already in the shared store with its grant,
/// so Slate and Circle see it too.
pub fn sign_in(provider_id: &str, email: &str) -> Result<String, String> {
    let registry = cosmic_pim_accounts::Registry::load();
    let provider = registry
        .get(provider_id)
        .ok_or_else(|| format!("no provider called {provider_id}"))?;
    let oauth = provider
        .oauth
        .as_ref()
        .ok_or_else(|| format!("{} does not use sign-in", provider.name))?;

    let pending = cosmic_pim_auth::begin(oauth).map_err(|why| why.to_string())?;
    open::that_detached(pending.authorize_url())
        .map_err(|why| format!("could not open the browser: {why}"))?;

    let code = pending.wait().map_err(|why| why.to_string())?;
    let credential = pending
        .exchange(&code, oauth)
        .map_err(|why| why.to_string())?;

    // The provider builds the account — protocol, endpoints, JMAP URL — which
    // is how a Google sign-in comes out as a Gmail-engine account rather than
    // an IMAP one with a token stuffed in the password slot.
    let account = provider.account_for(email.trim());
    let id = account.id.clone();
    let mut accounts = AccountStore::open_default().map_err(|why| why.to_string())?;
    accounts
        .add_oauth(account, &provider.id, &credential)
        .map_err(|why| why.to_string())?;
    Ok(id)
}

/// The credentials to use right now, renewed if the stored ones expired.
///
/// Called at the top of every worker that touches a server, because an OAuth
/// access token lives about an hour and a connection built at nine is stale by
/// ten. For a password account this is the held password and no I/O; for OAuth
/// it re-reads the stored grant and refreshes it over the network when it has
/// to — which is exactly why it must never run on the UI thread.
fn fresh_credentials(connection: &Connection) -> Result<Credentials, String> {
    if connection.account.auth == AuthMethod::Password {
        return Ok(connection.credentials.clone());
    }
    let mut accounts = AccountStore::open_default().map_err(|why| why.to_string())?;
    let registry = cosmic_pim_accounts::Registry::load();
    let secret = cosmic_pim_auth::resolve(&mut accounts, &registry, &connection.account_id)
        .map_err(|why| why.to_string())?;
    Ok(cosmic_pim_sync::credentials_for(&secret))
}

/// Runs one sync pass, over whichever protocol the account uses.
///
/// The pass itself lives in `cosmic-pim-sync`, which dispatches IMAP, JMAP,
/// POP3, Gmail, and Graph — and drains the outbox first, so a send waiting on
/// the network leaves before the pull that would file it. What is left here is
/// Envelope's own bookkeeping: turning the report into what the window shows,
/// and dropping index rows for a mailbox the server renumbered.
///
/// Blocking, and meant for a worker thread.
pub fn sync(connection: &Connection, cycle: u64) -> Result<SyncReport, String> {
    let options = SyncOptions {
        reconcile: cycle.is_multiple_of(RECONCILE_EVERY),
        since_ms: None,
    };

    let credentials = fresh_credentials(connection)?;
    let pass = cosmic_pim_sync::sync_account_mail(
        &connection.account,
        &credentials,
        &connection.root,
        options,
        now_ms(),
    )
    .map_err(|why| why.to_string())?;

    let mut report = SyncReport {
        sent: pass.sent,
        // Sends the outbox has given up on need a person, the same as a push
        // the flag queue has given up on.
        stuck: pass.given_up,
        ..SyncReport::default()
    };

    for mailbox in pass.mailboxes {
        // IMAP hands the full folder over — delimiter and the server's own
        // special-use declaration included. The label-shaped protocols hand
        // names, and the reconstruction recovers hierarchy and role from them
        // by convention, which for labels is all there ever was.
        let folder = mailbox.folder.unwrap_or_else(|| {
            cosmic_pim_mail::folder::from_list_entry(&mailbox.wire_name, Some('/'), &[])
        });
        match mailbox.outcome {
            Ok(outcome) => {
                report.fetched += outcome.fetched;
                report.pushed += outcome.pushed.succeeded;
                report.stuck += outcome.pushed.needs_reconcile + outcome.pushed.needs_user;

                // A renumbering does not make the index stale, it makes it
                // wrong: every UID in it now names a different message or
                // none. The next read rebuilds from the refetched maildir.
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
        report.folders.push(folder);
    }
    cosmic_pim_mail::folder::sort_for_display(&mut report.folders);

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

/// The provider registry's answer for an address, if it has one.
///
/// Richer than [`known_settings`]: a provider entry knows which *protocol* the
/// account should use and carries the JMAP session URL — which is how a
/// Fastmail address ends up on JMAP rather than on the IMAP fallback the
/// discovery table would give it. Consulted first for exactly that reason.
/// No network; the registry is files on disk.
#[must_use]
pub fn provider_settings(email: &str) -> Option<MailEndpoint> {
    let username = email.trim();
    let registry = cosmic_pim_accounts::provider::Registry::load();
    let provider = registry.for_email(username)?;
    provider
        .services
        .mail
        .as_ref()
        .map(|mail| mail.endpoint_for(username))
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
    /// This account cannot be watched — the server has no IDLE, or it does
    /// not speak IMAP at all. Do not watch again: the poll already covers it.
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
    // IDLE is IMAP's. The other protocols poll (JMAP will push over
    // EventSource when that lands); answering Unsupported rather than erroring
    // is what stops the watch loop reconnecting forever at a server that was
    // never going to say yes.
    let is_imap = connection
        .account
        .mail
        .as_ref()
        .is_none_or(|mail| mail.protocol == MailProtocol::Imap);
    if !is_imap {
        return Ok(WatchOutcome::Unsupported);
    }

    let credentials = fresh_credentials(connection)?;
    let mut session =
        Session::connect(&connection.endpoint, &credentials).map_err(|why| why.to_string())?;
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

/// One conversation in the unified inbox, tagged with whose it is.
#[derive(Debug, Clone)]
pub struct UnifiedConversation {
    pub account_id: String,
    /// The account's display name, for the row — with several inboxes merged,
    /// "which account is this from" is half of what a row has to say.
    pub account_name: String,
    pub conversation: Conversation,
}

/// Every configured account's connection, for the unified inbox.
///
/// Accounts without a mail endpoint are skipped rather than failed: a
/// calendar-only account in the shared store is the ordinary case, not an
/// error. An account whose credentials cannot be read is skipped with a log
/// line — one broken account must not take the other inboxes with it.
pub fn all_connections() -> Vec<Connection> {
    let Ok(accounts) = AccountStore::open_default() else {
        return Vec::new();
    };
    accounts
        .accounts()
        .to_vec()
        .iter()
        .filter_map(
            |account| match Connection::for_account(&accounts, account) {
                Ok(connection) => connection,
                Err(why) => {
                    tracing::warn!(
                        account = account.display_name,
                        why,
                        "skipped in the unified inbox"
                    );
                    None
                }
            },
        )
        .collect()
}

/// The inboxes of every account, merged, newest first.
///
/// Reads disk only — each account's index and maildir — so it is as cheap as
/// opening one folder times the number of accounts, and works offline like
/// everything else that reads.
pub fn unified_inbox(connections: &[Connection]) -> Result<Vec<UnifiedConversation>, String> {
    // `INBOX` is the one name RFC 3501 guarantees, and the label-shaped
    // protocols store under it too.
    let inbox = cosmic_pim_mail::folder::from_list_entry("INBOX", Some('/'), &[]);

    let mut merged = Vec::new();
    for connection in connections {
        match conversations(connection, &inbox) {
            Ok(list) => merged.extend(list.into_iter().map(|conversation| UnifiedConversation {
                account_id: connection.account_id.clone(),
                account_name: connection.account.display_name.clone(),
                conversation,
            })),
            Err(why) => {
                // Logged, not fatal: one account's unreadable index must not
                // empty the merged view of the others.
                tracing::warn!(
                    account = connection.account.display_name,
                    why,
                    "an inbox could not be read for the unified view"
                );
            }
        }
    }
    merged.sort_by_key(|entry| std::cmp::Reverse(entry.conversation.date_ms));
    Ok(merged)
}

/// Imports an mbox file into a folder, by APPENDing every message.
///
/// APPEND rather than writing into the maildir, because the maildir is a
/// *mirror* of the server: injecting messages locally creates entries the next
/// reconciliation would read as deleted-on-the-server and remove. Uploading
/// makes them real mail, and the sync that follows brings them back down the
/// same way everything else arrives.
///
/// Blocking for as long as the archive is large; strictly a worker call.
/// Unparseable chunks are counted and skipped — one mangled message in a
/// twenty-year archive must not abort the other ten thousand.
pub fn import_mbox(
    connection: &Connection,
    folder: &Folder,
    path: &std::path::Path,
) -> Result<(usize, usize), String> {
    let bytes = std::fs::read(path)
        .map_err(|why| format!("{} could not be read: {why}", path.display()))?;
    let messages = cosmic_pim_mail::mbox::messages(&bytes);
    if messages.is_empty() {
        return Err("that file does not look like an mbox archive".to_string());
    }

    let credentials = fresh_credentials(connection)?;
    let mut session =
        Session::connect(&connection.endpoint, &credentials).map_err(|why| why.to_string())?;

    let mut imported = 0usize;
    let mut skipped = 0usize;
    for raw in &messages {
        if Message::parse(raw).is_none() {
            skipped += 1;
            continue;
        }
        match session.append(&folder.wire_name, raw, Flags::default()) {
            Ok(()) => imported += 1,
            Err(why) => {
                // Stop rather than skip: an APPEND refused mid-run is the
                // server (quota, connection), not the message, and silently
                // dropping the rest of an archive is data loss with a success
                // message.
                let _ = session.logout();
                return Err(format!(
                    "{imported} imported, then the server refused: {why}"
                ));
            }
        }
    }
    let _ = session.logout();
    Ok((imported, skipped))
}

/// How a message says its list can be left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsubscribe {
    /// RFC 8058: one silent, honest POST. Done in place.
    OneClick(String),
    /// A `mailto:` target — leaving the list is sending a message, which the
    /// composer already knows how to do.
    Mailto(String),
    /// A plain `https:` page. Belongs in the browser: without the one-click
    /// header it is a page, and POSTing at pages is guessing.
    Browser(String),
}

/// The best way out of this message's list, if it offers one.
///
/// One-click beats mailto beats a page, because that is the order of least
/// ceremony for the user — and the order the sender's own headers rank them.
#[must_use]
pub fn unsubscribe_route(message: &Message) -> Option<Unsubscribe> {
    if message.one_click_unsubscribe
        && let Some(url) = message
            .unsubscribe
            .iter()
            .find(|url| url.to_ascii_lowercase().starts_with("https://"))
    {
        return Some(Unsubscribe::OneClick(url.clone()));
    }
    if let Some(url) = message
        .unsubscribe
        .iter()
        .find(|url| url.to_ascii_lowercase().starts_with("mailto:"))
    {
        return Some(Unsubscribe::Mailto(url.clone()));
    }
    message
        .unsubscribe
        .first()
        .map(|url| Unsubscribe::Browser(url.clone()))
}

/// Performs a one-click unsubscribe. Blocking, for a worker thread.
pub fn unsubscribe_one_click(url: &str) -> Result<(), String> {
    cosmic_pim_mail::unsubscribe::one_click(url).map_err(|why| why.to_string())
}

/// Saves a message as an `.eml` file in Downloads, returning where it landed.
///
/// The raw bytes, exactly as the server sent them — headers, MIME structure,
/// signatures — which is what makes the file importable by every other mail
/// tool ever written. This is the "walk away with your data" promise at the
/// granularity of one message.
pub fn export_message(
    connection: &Connection,
    folder: &Folder,
    uid: u32,
) -> Result<std::path::PathBuf, String> {
    let store =
        MaildirStore::open(connection.mailbox_path(folder)).map_err(|why| why.to_string())?;
    let raw = store
        .raw(uid)
        .map_err(|why| why.to_string())?
        .ok_or_else(|| "that message is no longer in this mailbox".to_string())?;
    let subject = Message::parse(&raw)
        .map(|message| message.subject)
        .filter(|subject| !subject.trim().is_empty())
        .unwrap_or_else(|| "message".to_string());

    attachment::save_into(&downloads(), &format!("{subject}.eml"), &raw)
        .map_err(|why| why.to_string())
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
///
/// Returns each message's flags as they were, which is everything an undo
/// needs: reversing a flag change is applying the previous flags through this
/// same path.
pub fn set_flags(
    connection: &Connection,
    folder: &Folder,
    uids: &[u32],
    edit: impl Fn(Flags) -> Flags,
) -> Result<Vec<(u32, Flags)>, String> {
    let mut store =
        MaildirStore::open(connection.mailbox_path(folder)).map_err(|why| why.to_string())?;
    let current = store.state().map_err(|why| why.to_string())?.entries;

    let mut previous = Vec::new();
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
        previous.push((*uid, existing));
    }
    Ok(previous)
}

/// Restores exact flags, one message at a time — the reverse of [`set_flags`].
pub fn restore_flags(
    connection: &Connection,
    folder: &Folder,
    previous: &[(u32, Flags)],
) -> Result<(), String> {
    let mut store =
        MaildirStore::open(connection.mailbox_path(folder)).map_err(|why| why.to_string())?;
    for (uid, flags) in previous {
        store.set_flags(*uid, *flags).map_err(|w| w.to_string())?;
        store
            .enqueue(PushOp::SetFlags {
                uid: *uid,
                flags: *flags,
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
///
/// Returns the messages as they were — bytes, flags, dates — which is what an
/// undo needs to put them back.
pub fn move_to(
    connection: &Connection,
    folder: &Folder,
    destination: &Folder,
    uids: &[u32],
) -> Result<Vec<RemoteMessage>, String> {
    let mut store =
        MaildirStore::open(connection.mailbox_path(folder)).map_err(|why| why.to_string())?;
    let state = store.state().map_err(|why| why.to_string())?;

    let mut taken = Vec::new();
    for uid in uids {
        if let Ok(Some(raw)) = store.raw(*uid) {
            taken.push(RemoteMessage {
                uid: *uid,
                flags: state.entries.get(uid).copied().unwrap_or_default(),
                raw,
                // Approximate on purpose: the true INTERNALDATE lives on the
                // server, and holding the message hostage to recover a
                // timestamp the next sync fixes anyway would be backwards.
                internal_date_ms: now_ms(),
            });
        }
        store
            .enqueue(PushOp::Move {
                uid: *uid,
                destination: destination.wire_name.clone(),
            })
            .map_err(|why| why.to_string())?;
        store.remove(*uid).map_err(|why| why.to_string())?;
    }
    Ok(taken)
}

/// Puts a move back, if it has not already left.
///
/// Undo has a deadline it does not control: the moment the writeback queue
/// drains, the move has happened on the server, the messages hold *new* UIDs
/// in the destination that the MOVE never told us, and there is nothing local
/// to reverse. Before that moment — which in practice is the window the user
/// actually regrets in — undoing is exact: the queued operation is cancelled
/// and the held bytes go back.
///
/// Partial states are handled per message: whichever of them still have their
/// queue entry come back, and the ones that already left are reported.
pub fn unmove(
    connection: &Connection,
    folder: &Folder,
    messages: &[RemoteMessage],
) -> Result<(), String> {
    let mut store =
        MaildirStore::open(connection.mailbox_path(folder)).map_err(|why| why.to_string())?;
    let pending: std::collections::HashSet<u32> =
        store.pending().iter().map(|entry| entry.op.uid()).collect();

    let mut departed = 0usize;
    for message in messages {
        if pending.contains(&message.uid) {
            store.resolve(message.uid).map_err(|why| why.to_string())?;
            store.upsert(message).map_err(|why| why.to_string())?;
        } else {
            departed += 1;
        }
    }
    if departed > 0 {
        return Err(format!(
            "{departed} already reached the server and cannot be brought back from here"
        ));
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

    let credentials = match fresh_credentials(connection) {
        Ok(credentials) => credentials,
        Err(why) => return Sent::Failed(why),
    };
    let filed_bytes = match smtp::send(&submission.endpoint, &credentials, draft) {
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

    let Ok(credentials) = fresh_credentials(connection) else {
        tracing::warn!("the message was sent but the Sent copy could not authenticate");
        return false;
    };
    match Session::connect(&connection.endpoint, &credentials).and_then(|mut session| {
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
