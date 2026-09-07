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
use cosmic_pim_mail::snooze;
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
    /// When this message is a bounce: `(recipient, reason)` per failed
    /// delivery, so the reader can say what happened instead of showing raw
    /// MTA prose as if it were correspondence.
    pub bounces: Vec<(String, String)>,
    /// The scheduling payload, when the message carries one — a
    /// `text/calendar` part with a METHOD. What the reader's "Open in
    /// calendar" hands to Slate, verbatim.
    pub invitation: Option<cosmic_pim_mail::calendar::Invitation>,
    /// The labels on this message, named through the mailbox's table.
    pub labels: Vec<String>,
    /// The message's OpenPGP state, examined against the account's keyring
    /// and the stored bytes — the verbatim original, which is the only thing
    /// a signature can be checked against.
    pub pgp: cosmic_pim_mail::pgp::Examined,
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
    /// Every identity mail may go out as — the primary first. The composer's
    /// From choices.
    pub identities: Vec<Mailbox>,
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

        let identities = account
            .identities()
            .into_iter()
            .map(|(name, address)| Mailbox {
                name: Some(name).filter(|n| !n.trim().is_empty()),
                address,
            })
            .collect();

        Ok(Some(Self {
            account: account.clone(),
            account_id: account.id.clone(),
            submission,
            identities,
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

/// What the provider registry says about an address being added.
///
/// Enough for the add-account form to say the right thing before anything is
/// stored: whether this provider's own route is a browser sign-in, whether
/// that route is actually open on this machine, and the manifest's note —
/// "create an app password first", most often.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderNote {
    pub name: String,
    pub hint: Option<String>,
    /// The provider signs in with OAuth rather than a password.
    pub uses_sign_in: bool,
    /// ...and a client id is configured, so that route can be taken.
    pub sign_in_ready: bool,
}

/// The registry's note on an address, by its domain. No network.
#[must_use]
pub fn provider_note(email: &str) -> Option<ProviderNote> {
    let registry = cosmic_pim_accounts::Registry::load();
    let provider = registry.for_email(email.trim())?;
    Some(ProviderNote {
        name: provider.name.clone(),
        hint: provider.hint.clone(),
        uses_sign_in: provider.oauth.is_some(),
        sign_in_ready: provider
            .oauth
            .as_ref()
            .is_some_and(cosmic_pim_accounts::OAuth::is_configured),
    })
}

/// The servers a password can drive for an address, without a network.
///
/// The registry's endpoint when the provider takes a password — Fastmail's
/// JMAP, iCloud's IMAP — and the discovery table otherwise. An OAuth
/// provider's registry entry is skipped on purpose: it names the Gmail or
/// Graph engine, which a password cannot log in to, whereas the table's IMAP
/// hosts still take an app password.
#[must_use]
pub fn password_settings(email: &str) -> Option<MailEndpoint> {
    let email = email.trim();
    let registry = cosmic_pim_accounts::Registry::load();
    if let Some(provider) = registry.for_email(email)
        && provider.oauth.is_none()
        && let Some(mail) = provider.services.mail.as_ref()
    {
        return Some(mail.endpoint_for(email));
    }
    known_settings(email).map(|found| endpoint_of(&found))
}

/// A discovered server pair as a storable endpoint — IMAP, since that is all
/// discovery can find.
#[must_use]
pub fn endpoint_of(found: &Discovered) -> MailEndpoint {
    MailEndpoint {
        imap_port: found.imap_port,
        imap_transport: transport_of(found.imap_security),
        imap_username: Some(found.username.clone()).filter(|u| !u.is_empty()),
        smtp_host: found.smtp_host.clone(),
        smtp_port: found.smtp_port,
        smtp_transport: transport_of(found.smtp_security),
        ..MailEndpoint::tls(found.imap_host.clone())
    }
}

/// The inverse of [`transport`], for what discovery reports.
#[must_use]
pub fn transport_of(security: Security) -> Transport {
    match security {
        Security::Tls => Transport::Tls,
        Security::StartTls => Transport::StartTls,
        Security::Plaintext => Transport::Plaintext,
    }
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

/// This account's drafts: local records, mirrored to the server.
///
/// The record on this device is the authority the composer edits; the mirror
/// ([`sweep_drafts`]) keeps the server's Drafts folder holding exactly one
/// copy per draft, so other devices see it. See `cosmic_pim_mail::draft_sync`
/// for how replacement avoids the duplication that kept drafts local-only.
pub fn drafts(connection: &Connection) -> Result<Drafts, String> {
    Drafts::open(connection.root.join(&connection.account_id)).map_err(|why| why.to_string())
}

/// Pushes local draft edits and discards to the server's Drafts folder.
///
/// Cheap when there is nothing to say: the dirty check reads disk only, and
/// no connection is opened for a clean store — which is what makes this safe
/// to call after every save, discard, and sync pass. IMAP only for now; the
/// label-shaped engines keep drafts local until their own draft APIs land.
///
/// Blocking, for a worker thread. Returns `None` when there was nothing to do
/// or the account cannot mirror.
pub fn sweep_drafts(
    connection: &Connection,
    folders: &[Folder],
) -> Result<Option<cosmic_pim_mail::draft_sync::SweepReport>, String> {
    let is_imap = connection
        .account
        .mail
        .as_ref()
        .is_none_or(|mail| mail.protocol == MailProtocol::Imap);
    if !is_imap {
        return Ok(None);
    }

    let store = drafts(connection)?;
    let dirty = store.dirty().map_err(|why| why.to_string())?;
    if dirty.is_empty() && store.pending_retractions().is_empty() {
        return Ok(None);
    }

    let domain = connection
        .submission
        .as_ref()
        .and_then(|submission| submission.identity.address.split('@').next_back())
        .unwrap_or_default()
        .to_owned();

    let credentials = fresh_credentials(connection)?;
    let mut session =
        Session::connect(&connection.endpoint, &credentials).map_err(|why| why.to_string())?;
    // The folder the server declares for the role, or the conventional name —
    // created if the account has never had one, because a mirror with nowhere
    // to land is a mirror that silently is not one.
    let wire = match special(folders, SpecialUse::Drafts) {
        Some(folder) => folder.wire_name.clone(),
        None => {
            let _ = session.create_mailbox("Drafts");
            "Drafts".to_owned()
        }
    };
    let report = cosmic_pim_mail::draft_sync::sweep(&mut session, &wire, &store, &domain, now_ms());
    let _ = session.logout();
    Ok(Some(report))
}

/// Opens a message in the Drafts folder for **editing**, not reading.
///
/// Returns the local draft id and the editable draft. A mirror this device
/// uploaded opens as its own record — the local copy is the authority. A
/// draft another device wrote is adopted: parsed back into an editable form
/// and linked to where it lives, so the first edit here replaces it there.
pub fn edit_server_draft(
    connection: &Connection,
    folder: &Folder,
    uid: u32,
) -> Result<(String, Draft), String> {
    let Some(identity) = connection.submission.as_ref().map(|s| s.identity.clone()) else {
        return Err("this account has no From address".into());
    };
    let store =
        MaildirStore::open(connection.mailbox_path(folder)).map_err(|why| why.to_string())?;
    let raw = store
        .raw(uid)
        .map_err(|why| why.to_string())?
        .ok_or_else(|| "that message is no longer in this mailbox".to_string())?;
    let message = Message::parse(&raw).ok_or_else(|| "that draft could not be read".to_string())?;
    let drafts_store = drafts(connection)?;

    // One of ours? The mirror's Message-ID carries the record's id.
    if let Some(id) = message.message_id.as_deref().and_then(own_draft_id)
        && let Ok(Some(draft)) = load_draft(connection, id)
    {
        return Ok((id.to_owned(), draft));
    }

    // Another device's. Adopt it: editable here, replaced there on save —
    // keeping the alias it was being written as, when it is one of ours.
    let identity = message
        .from
        .first()
        .and_then(|from| {
            connection
                .identities
                .iter()
                .find(|m| m.address.eq_ignore_ascii_case(&from.address))
        })
        .cloned()
        .unwrap_or(identity);
    let draft = Draft::from_mirror(&message, &raw, identity);
    let id = drafts::new_id(now_ms());
    let uid_validity = store
        .state()
        .map_err(|why| why.to_string())?
        .cursor
        .uid_validity;
    let message_id = message
        .message_id
        .clone()
        .unwrap_or_else(|| cosmic_pim_mail::draft_sync::mint_message_id(&id, ""));
    drafts_store
        .adopt(&id, &draft, &message_id, uid_validity, uid, now_ms())
        .map_err(|why| why.to_string())?;
    Ok((id, draft))
}

/// Reads a mirrored draft's record id out of its `Message-ID`, when it is one
/// of ours: `<hexid>.draft@<domain>`.
fn own_draft_id(message_id: &str) -> Option<&str> {
    let (id, _domain) = message_id.split_once(".draft@")?;
    drafts::is_valid_id(id).then_some(id)
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
///
/// The stored From survives when it is still one of the account's
/// identities — somebody who chose an alias should not find the primary
/// swapped in behind their back — and falls back to the current primary when
/// it is not, which is the reason the fallback exists at all.
pub fn load_draft(connection: &Connection, id: &str) -> Result<Option<Draft>, String> {
    let Some(identity) = connection.submission.as_ref().map(|s| s.identity.clone()) else {
        return Err("this account has no From address".into());
    };
    let Some(mut draft) = drafts(connection)?
        .peek(id)
        .map_err(|why| why.to_string())?
    else {
        return Ok(None);
    };
    if !connection
        .identities
        .iter()
        .any(|m| m.address.eq_ignore_ascii_case(&draft.from.address))
    {
        draft.from = identity;
    }
    Ok(Some(draft))
}

pub fn delete_draft(connection: &Connection, id: &str) -> Result<(), String> {
    drafts(connection)?
        .delete(id)
        .map_err(|why| why.to_string())
}

/// Queues a message to go out at `not_before_ms` — the undo-send grace and
/// "send later" both land here. Returns the queue id an undo needs.
///
/// The draft record becomes the queued message, exactly as it does when a
/// failed send is queued: leaving both would show it twice and send it once.
pub fn schedule_send(
    connection: &Connection,
    draft_id: Option<&str>,
    draft: &Draft,
    not_before_ms: i64,
) -> Result<String, String> {
    let id = draft_id
        .filter(|id| drafts::is_valid_id(id))
        .map_or_else(|| drafts::new_id(now_ms()), ToOwned::to_owned);
    outbox(connection)?
        .schedule(&id, draft, not_before_ms)
        .map_err(|why| why.to_string())?;
    if let Some(previous) = draft_id
        && let Err(why) = delete_draft(connection, previous)
    {
        tracing::warn!(%why, "a scheduled message left its draft behind");
    }
    Ok(id)
}

/// Takes a scheduled send back, returning the draft to edit. `None` means it
/// already went — and the caller must say so, not reopen a composer for a
/// message the recipients already have.
pub fn cancel_send(connection: &Connection, id: &str) -> Result<Option<Draft>, String> {
    outbox(connection)?
        .cancel(id)
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

    if !query.unread && !query.starred && query.labels.is_empty() {
        return Ok(hits);
    }

    // Only the mailboxes the hits are actually in get opened, and each one only
    // once: a flag filter must not cost a walk of every folder on the server.
    let mut flags: HashMap<String, (BTreeMap<u32, Flags>, Vec<String>)> = HashMap::new();
    let mut kept = Vec::with_capacity(hits.len());
    for hit in hits {
        let (entry, table) = match flags.get(&hit.mailbox) {
            Some(entry) => entry,
            None => {
                let Some(folder) = folders.iter().find(|f| f.wire_name == hit.mailbox) else {
                    // A mailbox the server no longer lists. The index will drop
                    // it on the next sync; until then it cannot be filtered.
                    continue;
                };
                let loaded = MaildirStore::open(connection.mailbox_path(folder))
                    .map(|store| {
                        let table = store.keywords();
                        let entries = store.state().map(|state| state.entries).unwrap_or_default();
                        (entries, table)
                    })
                    .unwrap_or_default();
                flags.entry(hit.mailbox.clone()).or_insert(loaded)
            }
        };
        let hit_flags = entry.get(&hit.uid).copied().unwrap_or_default();
        if (query.unread && hit_flags.seen) || (query.starred && !hit_flags.flagged) {
            continue;
        }
        if !query.labels.is_empty() {
            let named = display_labels(table, hit_flags.keywords);
            let carries_all = query
                .labels
                .iter()
                .all(|wanted| named.iter().any(|name| name.eq_ignore_ascii_case(wanted)));
            if !carries_all {
                continue;
            }
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
            Ok((list, _labels)) => {
                merged.extend(list.into_iter().map(|conversation| UnifiedConversation {
                    account_id: connection.account_id.clone(),
                    account_name: connection.account.display_name.clone(),
                    conversation,
                }))
            }
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
) -> Result<(Vec<Conversation>, Vec<Vec<String>>), String> {
    let path = connection.mailbox_path(folder);
    if !path.join("cur").is_dir() {
        return Ok((Vec::new(), Vec::new()));
    }
    let store = MaildirStore::open(&path).map_err(|why| why.to_string())?;
    let flags = store.state().map_err(|why| why.to_string())?.entries;

    let mut index = Index::open(&connection.index_path).map_err(|why| why.to_string())?;
    index
        .sync_mailbox(&connection.account_id, &folder.wire_name, &store)
        .map_err(|why| why.to_string())?;
    let conversations = index
        .conversations(&connection.account_id, &folder.wire_name, &flags)
        .map_err(|why| why.to_string())?;

    // The chips each row shows: the union of its messages' keywords, named
    // through the mailbox's table. Computed here because the table and the
    // flags are both already in hand.
    let table = store.keywords();
    let labels = conversations
        .iter()
        .map(|conversation| {
            let bits = conversation
                .uids
                .iter()
                .filter_map(|uid| flags.get(uid))
                .fold(0u32, |acc, f| acc | f.keywords);
            display_labels(&table, bits)
        })
        .collect();
    Ok((conversations, labels))
}

/// The names a keyword bitmask displays as: table rows for set bits, minus
/// gap rows and the `$`-prefixed conventions ($Forwarded, $MDNSent, …) that
/// are bookkeeping between servers, not something a person filed.
fn display_labels(table: &[String], bits: u32) -> Vec<String> {
    let flags = Flags {
        keywords: bits,
        ..Flags::default()
    };
    flags
        .keyword_bits()
        .filter_map(|bit| table.get(usize::from(bit)))
        .filter(|name| !name.is_empty() && !name.starts_with('$'))
        .cloned()
        .collect()
}

/// The keyring directory, beside the account's maildirs: one armored public
/// key per file, `<address>.asc`, the address it is bound to as the stem.
/// Files-as-truth — `gpg --import`-able, droppable, diffable.
const KEYS_DIRECTORY: &str = ".keys";

/// Every public key held for this account, bound to the address its
/// filename names.
fn keyring(connection: &Connection) -> Vec<cosmic_pim_mail::pgp::Certificate> {
    let dir = connection
        .root
        .join(&connection.account_id)
        .join(KEYS_DIRECTORY);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let address = path.file_stem()?.to_str()?.to_lowercase();
            let armored = std::fs::read_to_string(&path).ok()?;
            let certificate = cosmic_pim_mail::pgp::Certificate::from_armored(&address, &armored);
            if certificate.is_none() {
                tracing::warn!(path = %path.display(), "a keyring file is not an armored key");
            }
            certificate
        })
        .collect()
}

/// Imports a public key a message carries (`application/pgp-keys`), binding
/// it to the **sender's** address — that binding is what turns "someone
/// signed this" into "the sender signed this", so it is never taken from the
/// key's own self-description.
pub fn import_pgp_key(
    connection: &Connection,
    folder: &Folder,
    uid: u32,
    index: usize,
) -> Result<String, String> {
    let store =
        MaildirStore::open(connection.mailbox_path(folder)).map_err(|why| why.to_string())?;
    let raw = store
        .raw(uid)
        .map_err(|why| why.to_string())?
        .ok_or_else(|| "that message is no longer in this mailbox".to_string())?;
    let message =
        Message::parse(&raw).ok_or_else(|| "that message could not be read".to_string())?;
    let address = message
        .sender()
        .map(|from| from.address.clone())
        .filter(|address| address.contains('@') && !address.contains(['/', '\\']))
        .ok_or_else(|| "this message does not say who it is from".to_string())?;

    let bytes = attachment::bytes_of(&raw, index).map_err(|why| why.to_string())?;
    let armored = String::from_utf8(bytes)
        .map_err(|_| "that attachment is not an armored key".to_string())?;
    // Validated before it is written: a keyring file that will not parse is
    // a keyring that silently stops verifying.
    if cosmic_pim_mail::pgp::Certificate::from_armored(&address, &armored).is_none() {
        return Err("that attachment is not an OpenPGP public key".to_string());
    }

    let dir = connection
        .root
        .join(&connection.account_id)
        .join(KEYS_DIRECTORY);
    std::fs::create_dir_all(&dir).map_err(|why| why.to_string())?;
    std::fs::write(dir.join(format!("{address}.asc")), armored)
        .map_err(|why| why.to_string())?;
    Ok(address)
}

/// Every label this folder's mailbox can offer.
pub fn labels(connection: &Connection, folder: &Folder) -> Result<Vec<String>, String> {
    let store =
        MaildirStore::open(connection.mailbox_path(folder)).map_err(|why| why.to_string())?;
    Ok(store
        .keywords()
        .into_iter()
        .filter(|name| !name.is_empty() && !name.starts_with('$'))
        .collect())
}

/// Sets or clears one label on messages — the keyword is interned into the
/// mailbox's table first, then the change takes the same queued flag path as
/// read and star, so it works offline and reaches the server by name.
///
/// Returns each message's flags as they were, which is what undo needs.
pub fn set_label(
    connection: &Connection,
    folder: &Folder,
    uids: &[u32],
    name: &str,
    on: bool,
) -> Result<Vec<(u32, Flags)>, String> {
    let bit = MaildirStore::open(connection.mailbox_path(folder))
        .map_err(|why| why.to_string())?
        .intern_keyword(name)
        .map_err(|why| why.to_string())?;
    set_flags(connection, folder, uids, move |flags| {
        flags.with_keyword(bit, on)
    })
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
        bounces: bounces_in(&raw, &message),
        invitation: cosmic_pim_mail::calendar::invitation(&raw),
        labels: display_labels(&store.keywords(), flags.keywords),
        pgp: cosmic_pim_mail::pgp::examine(&raw, &keyring(connection)),
        uid,
        message,
        flags,
    })
}

/// The delivery failures a message reports, as `(recipient, reason)` lines.
///
/// Structured reports first, the loose Postfix-style prose bounce as the
/// fallback. Delayed notifications are kept — "still trying" is worth a
/// line — and complaints are not: an ARF report in a personal inbox is the
/// list operator's business.
fn bounces_in(raw: &[u8], message: &Message) -> Vec<(String, String)> {
    use cosmic_pim_mail::dsn::{self, BounceKind};
    let records = dsn::parse_report(raw).or_else(|| {
        let from = message
            .sender()
            .map(|m| m.address.as_str())
            .unwrap_or_default();
        dsn::parse_loose(from, &message.subject, &message.body.text).map(|record| vec![record])
    });
    records
        .unwrap_or_default()
        .into_iter()
        .filter(|record| record.kind != BounceKind::Complaint)
        .map(|record| {
            let mut reason = record.status_code;
            if !record.diagnostic.is_empty() {
                if !reason.is_empty() {
                    reason.push_str(" — ");
                }
                reason.push_str(&record.diagnostic);
            }
            (record.recipient, reason)
        })
        .collect()
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

/// The folder snoozed mail waits in, created on first use.
///
/// A constant name rather than a discovered one: no server declares a
/// special-use for snoozing, and inventing detection for a folder this
/// client creates itself would be guessing at our own convention.
const SNOOZED_FOLDER: &str = "Snoozed";

/// The account's snooze schedule.
///
/// The timer lives in the substrate — one engine for every app in the suite,
/// and a TOML file a person can read and cancel by hand. What is Envelope's
/// alone is the holding folder and the moves, which is the half that needs a
/// server.
fn snooze_schedule(connection: &Connection) -> Result<snooze::Schedule, String> {
    snooze::Schedule::open(connection.root.join(&connection.account_id))
        .map_err(|why| why.to_string())
}

/// Snoozes messages: they leave `folder` for the Snoozed folder now, through
/// the durable queue, and the schedule brings them back at `until_ms`.
///
/// Returns what a move returns — the taken messages — so the caller's undo
/// works exactly like archive's.
pub fn snooze(
    connection: &Connection,
    folder: &Folder,
    uids: &[u32],
    until_ms: i64,
) -> Result<Vec<RemoteMessage>, String> {
    // Identity and subject are read before the move takes the files away.
    // The subject is carried so a pending snooze can be listed as something
    // a person recognises rather than as a `Message-ID`.
    let store =
        MaildirStore::open(connection.mailbox_path(folder)).map_err(|why| why.to_string())?;
    let deferred: Vec<(String, String)> = uids
        .iter()
        .filter_map(|uid| store.raw(*uid).ok().flatten())
        .filter_map(|raw| Message::parse(&raw))
        .filter_map(|message| {
            let subject = message.subject;
            message.message_id.map(|id| (id, subject))
        })
        .collect();
    if deferred.is_empty() {
        return Err("those messages carry no Message-ID, so nothing could bring them back".into());
    }

    // Make sure there is somewhere to go. Cheap when it already exists — the
    // server answers NO and the move proceeds against the existing folder.
    if let Ok(mut session) = folder_session(connection) {
        let _ = session.create_mailbox(SNOOZED_FOLDER);
        let _ = session.logout();
    }

    let destination = cosmic_pim_mail::folder::from_list_entry(SNOOZED_FOLDER, Some('/'), &[]);
    let taken = move_to(connection, folder, &destination, uids)?;

    let mut schedule = snooze_schedule(connection)?;
    let snoozed_at_ms = now_ms();
    for (message_id, subject) in deferred {
        schedule
            .snooze(snooze::Snoozed {
                message_id,
                // Where it came from is where it goes back to. A message
                // deferred out of Archive returns to Archive, not to the
                // inbox it had already left.
                origin: folder.wire_name.clone(),
                holding: SNOOZED_FOLDER.to_owned(),
                wake_at_ms: until_ms,
                snoozed_at_ms,
                subject,
            })
            .map_err(|why| why.to_string())?;
    }
    Ok(taken)
}

/// Returns every snoozed message whose time has come to where it came from.
///
/// Blocking, for the worker, after a sync pass — the poll that drains every
/// other queued write is also what wakes snoozes, so a laptop that was
/// asleep past the deadline wakes them on its next check. Returns how many
/// came back. IMAP only, like the mirror.
pub fn wake_snoozed(connection: &Connection, now_ms: i64) -> Result<usize, String> {
    let mut schedule = snooze_schedule(connection)?;
    let due = schedule.due(now_ms);
    if due.is_empty() {
        return Ok(0);
    }

    let mut session = folder_session(connection)?;
    let mut woken = 0usize;
    for wake in due {
        match wake_one(&mut session, &wake) {
            // Retired only once the message is actually back. A wake that
            // failed stays due and is tried again on the next pass, rather
            // than being dropped with the message still put away — mail the
            // user deliberately deferred must not be lost by a failed move.
            Ok(moved) => {
                schedule
                    .woke(&wake.message_id)
                    .map_err(|why| why.to_string())?;
                woken += moved;
            }
            Err(why) => tracing::warn!(
                message_id = wake.message_id,
                why,
                "a snoozed message could not be brought back; it stays due"
            ),
        }
    }
    let _ = session.logout();
    Ok(woken)
}

/// Brings one message back, returning how many copies moved.
///
/// Not finding it is success rather than failure: the user dealt with it from
/// another client, which is the snooze resolving itself, and the record
/// should retire rather than be searched for forever.
fn wake_one(session: &mut Session, wake: &snooze::Wake) -> Result<usize, String> {
    use cosmic_pim_mail::push::Writeback as _;

    session
        .select_mailbox(&wake.from)
        .map_err(|why| why.to_string())?;
    let uids = session
        .uids_by_message_id(&wake.message_id)
        .map_err(|why| why.to_string())?;

    let mut moved = 0usize;
    for uid in uids {
        // Before the move, always. Neither MOVE nor the COPY fallback reports
        // the destination UID, so this is the last moment the message can be
        // addressed at all — and a message that comes back already-read comes
        // back invisible.
        if wake.mark_unread {
            session.mark_unread(uid).map_err(|why| why.to_string())?;
        }
        if wake.is_move() {
            session
                .move_message(uid, &wake.to)
                .map_err(|why| why.to_string())?;
        }
        moved += 1;
    }
    Ok(moved)
}

/// Is there anything on the snooze schedule whose time has come?
///
/// The cheap pre-check that lets the poll skip connecting when nothing is
/// due — the common case, every two minutes.
#[must_use]
pub fn has_due_snoozes(connection: &Connection, now_ms: i64) -> bool {
    match snooze_schedule(connection) {
        Ok(schedule) => !schedule.due(now_ms).is_empty(),
        // A schedule that will not load is not "nothing to do". It is a
        // problem the wake pass should report, so let it run and say so
        // rather than skipping silently on the strength of a read error.
        Err(_) => true,
    }
}

/// This account's filter rules, as stored.
pub fn load_rules(connection: &Connection) -> Result<Vec<cosmic_pim_mail::rules::Rule>, String> {
    cosmic_pim_mail::rules::Rules::open(connection.root.join(&connection.account_id))
        .map(|store| store.rules)
        .map_err(|why| why.to_string())
}

/// Writes the account's rules back.
pub fn save_rules(
    connection: &Connection,
    rules: &[cosmic_pim_mail::rules::Rule],
) -> Result<(), String> {
    let mut store =
        cosmic_pim_mail::rules::Rules::open(connection.root.join(&connection.account_id))
            .map_err(|why| why.to_string())?;
    store.rules = rules.to_vec();
    store.save().map_err(|why| why.to_string())
}

/// What one rules pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RulesReport {
    /// Messages at least one rule acted on.
    pub matched: usize,
    /// Rules that asked for something the account cannot do — a move to a
    /// folder that is gone, a delete with no Trash. `(rule name, why)`.
    pub failures: Vec<(String, String)>,
    /// Bounces among the new arrivals: `(recipient, reason)`. Collected here
    /// because this pass is already parsing exactly the messages that just
    /// arrived, and a delivery failure is news worth a status line.
    pub bounces: Vec<(String, String)>,
}

/// Where the rules pass keeps its high-water mark, per mailbox.
///
/// Client state in the same sense as `.imap-state.json`: derived from
/// nothing, rebuildable by accepting a one-time re-run, dot-prefixed so no
/// maildir walker adopts it.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct RulesState {
    /// Highest UID already offered to the rules, by wire name.
    #[serde(default)]
    processed: HashMap<String, u32>,
}

/// Applies the account's rules to mail that arrived since the last pass.
///
/// The inbox only — rules are about arriving mail, and everything arrives in
/// INBOX. The first pass with rules present records the high-water mark and
/// processes nothing: rules act on what arrives after they exist, not on ten
/// years of archive the moment one is written.
///
/// Blocking, for a worker thread, after a sync pass. Returns `None` when
/// there are no rules or nothing new.
pub fn apply_rules(
    connection: &Connection,
    folders: &[Folder],
) -> Result<Option<RulesReport>, String> {
    let rules = load_rules(connection)?;

    let inbox = special(folders, SpecialUse::Inbox)
        .cloned()
        .unwrap_or_else(|| cosmic_pim_mail::folder::from_list_entry("INBOX", Some('/'), &[]));
    let store =
        MaildirStore::open(connection.mailbox_path(&inbox)).map_err(|why| why.to_string())?;
    let entries = store.state().map_err(|why| why.to_string())?.entries;
    let newest = entries.keys().max().copied().unwrap_or(0);

    let state_path = connection
        .root
        .join(&connection.account_id)
        .join(".rules-state.json");
    let mut state: RulesState = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();

    let Some(&mark) = state.processed.get(&inbox.wire_name) else {
        // First pass: record where "new" starts and touch nothing.
        state.processed.insert(inbox.wire_name.clone(), newest);
        write_rules_state(&state_path, &state)?;
        return Ok(None);
    };

    let fresh: Vec<u32> = entries.keys().copied().filter(|uid| *uid > mark).collect();
    if fresh.is_empty() {
        return Ok(None);
    }

    let mut report = RulesReport::default();
    for uid in &fresh {
        let Ok(Some(raw)) = store.raw(*uid) else {
            continue;
        };
        let Some(message) = Message::parse(&raw) else {
            continue;
        };
        report.bounces.extend(bounces_in(&raw, &message));

        let plan = cosmic_pim_mail::rules::evaluate(&rules, &message);
        if plan.is_empty() {
            continue;
        }
        report.matched += 1;

        if plan.mark_read || plan.star {
            let (mark_read, star) = (plan.mark_read, plan.star);
            set_flags(connection, &inbox, &[*uid], move |flags| Flags {
                seen: flags.seen || mark_read,
                flagged: flags.flagged || star,
                ..flags
            })?;
        }
        // Deletion means Trash, the same as the Delete key: a rule is
        // automation of a verb the user has, not a stronger one.
        let destination = if plan.delete {
            match special(folders, SpecialUse::Trash) {
                Some(trash) => Some(trash.clone()),
                None => {
                    report.failures.push((
                        plan.matched.join(", "),
                        "this account has no Trash folder to delete into".into(),
                    ));
                    None
                }
            }
        } else if let Some(wire) = &plan.move_to {
            match folders.iter().find(|f| &f.wire_name == wire) {
                Some(folder) => Some(folder.clone()),
                None => {
                    report.failures.push((
                        plan.matched.join(", "),
                        format!("{wire} is no longer a folder on this account"),
                    ));
                    None
                }
            }
        } else {
            None
        };
        if let Some(destination) = destination {
            move_to(connection, &inbox, &destination, &[*uid])?;
        }
    }

    state.processed.insert(inbox.wire_name.clone(), newest);
    write_rules_state(&state_path, &state)?;
    Ok(Some(report))
}

fn write_rules_state(path: &std::path::Path, state: &RulesState) -> Result<(), String> {
    let json = serde_json::to_string_pretty(state).map_err(|why| why.to_string())?;
    // Write-then-rename, so a crash mid-write cannot leave a torn file. A
    // torn file here would silently re-initialise the high-water mark and
    // skip a window of mail the rules were supposed to see.
    let temp = path.with_extension("json.new");
    std::fs::write(&temp, json).map_err(|why| why.to_string())?;
    std::fs::rename(&temp, path).map_err(|why| why.to_string())
}

/// A session for a one-off folder operation. IMAP only: the label-shaped
/// engines manage folders through their own APIs, which are not wired yet.
fn folder_session(connection: &Connection) -> Result<Session, String> {
    let is_imap = connection
        .account
        .mail
        .as_ref()
        .is_none_or(|mail| mail.protocol == MailProtocol::Imap);
    if !is_imap {
        return Err("this account's folders are managed by its provider".into());
    }
    let credentials = fresh_credentials(connection)?;
    Session::connect(&connection.endpoint, &credentials).map_err(|why| why.to_string())
}

/// Creates a folder on the server. The next sync lists it.
pub fn create_folder(connection: &Connection, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a folder needs a name".into());
    }
    let mut session = folder_session(connection)?;
    let result = session
        .create_mailbox(&cosmic_pim_mail::folder::encode_modified_utf7(name))
        .map_err(|why| why.to_string());
    let _ = session.logout();
    result
}

/// Renames a folder, keeping its place in the hierarchy.
///
/// RFC 3501 renames the subtree with it. The local maildir and index rows
/// still carry the old name and are dropped; the next sync re-mirrors the
/// folder under its new name — the server is the authority on what exists.
pub fn rename_folder(
    connection: &Connection,
    folder: &Folder,
    new_name: &str,
) -> Result<(), String> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err("a folder needs a name".into());
    }
    if new_name.contains(folder.delimiter) {
        return Err(format!(
            "a folder name cannot contain this server's separator ({})",
            folder.delimiter
        ));
    }
    // The rename replaces the leaf; parents stay, so the folder does not
    // move. The wire wants modified UTF-7, same as LIST returned.
    let leaf = cosmic_pim_mail::folder::encode_modified_utf7(new_name);
    let to = match folder.wire_name.rfind(folder.delimiter) {
        Some(cut) => format!("{}{}{leaf}", &folder.wire_name[..cut], folder.delimiter),
        None => leaf,
    };

    let mut session = folder_session(connection)?;
    let result = session
        .rename_mailbox(&folder.wire_name, &to)
        .map_err(|why| why.to_string());
    let _ = session.logout();
    result?;
    forget_local(connection, folder);
    Ok(())
}

/// Deletes a folder — the folder itself, with every message in it.
///
/// The caller has already asked the user; this executes and then drops the
/// local mirror.
pub fn delete_folder(connection: &Connection, folder: &Folder) -> Result<(), String> {
    let mut session = folder_session(connection)?;
    let result = session
        .delete_mailbox(&folder.wire_name)
        .map_err(|why| why.to_string());
    let _ = session.logout();
    result?;
    forget_local(connection, folder);
    Ok(())
}

/// Drops a folder's local mirror: the maildir and its index rows.
///
/// Best-effort — the server operation already succeeded, and a leftover
/// directory is disk residue the next sync ignores, not an error worth
/// failing the operation the user asked for.
fn forget_local(connection: &Connection, folder: &Folder) {
    let path = connection.mailbox_path(folder);
    if path.is_dir()
        && let Err(why) = std::fs::remove_dir_all(&path)
    {
        tracing::warn!(%why, path = %path.display(), "a renamed or deleted folder's maildir survived");
    }
    if let Err(why) = forget_index(connection, &folder.wire_name) {
        tracing::warn!(
            why,
            mailbox = folder.wire_name,
            "index rows survived their folder"
        );
    }
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
