// SPDX-License-Identifier: GPL-3.0-only

//! The iMIP hand-off: Envelope's side of the one cross-process contract.
//!
//! The payload of an invitation arrives in mail; the decision about it
//! belongs in the calendar. The two are separate processes, so this — and
//! only this — crosses D-Bus, on the `Scheduling` interfaces each app
//! exports at its own single-instance name (the contract lives in
//! cosmic-pim's ARCHITECTURE.md). Envelope moves bytes, names an account and
//! says who a message was from; the iTIP semantics stay in the calendar
//! library.
//!
//! The contract, as this side calls and serves it:
//!
//! ```text
//! Envelope calls, on Slate's com.magnetaros.CosmicPim.Scheduling1:
//!   DeliverInvitation2(ics: s, account_id: s, sender: s) → (accepted: b)
//!   DeliverInvitation(ics: s, account_id: s)             → (accepted: b)
//!
//! Envelope exports:
//!   com.magnetaros.CosmicPim.Scheduling2
//!     SendSchedulingReply(ics: s, account_id: s, to: s, from: s) → (queued: b)
//!   com.magnetaros.CosmicPim.Scheduling1
//!     SendSchedulingReply(ics: s, account_id: s, to: s)          → (queued: b)
//! ```
//!
//! `sender` is the message's `From` address — the address alone, no display
//! name — so the calendar can refuse an invitation or a cancellation that
//! names one person as organizer and was mailed by another. A Slate from
//! before `DeliverInvitation2` answers `UnknownMethod`, and the payload is
//! handed to `DeliverInvitation` instead, without the sender.
//!
//! `from` is the address the reply answers as: the ATTENDEE the invitation
//! was sent to. When it is one of the account's identities the reply goes
//! out from it; otherwise, and on `Scheduling1`, from the account's primary
//! address.
//!
//! Degradation is part of the contract: Slate's name being unowned means the
//! feature is absent, not broken. Nothing here calls `StartServiceByName` —
//! an invitation must not *launch* a calendar — and the caller falls back to
//! the save-the-file affordance the reader already has.

use std::sync::Arc;

use cosmic_pim_mail::compose::{Attachment, Draft};
use cosmic_pim_mail::model::{Mailbox, Message};

use crate::mail::Connection;

/// The shared interface name. The suffix is the versioning policy: a
/// breaking change is a new name exported alongside this one, never a
/// changed signature under it.
const INTERFACE: &str = "com.magnetaros.CosmicPim.Scheduling1";

const SLATE_NAME: &str = "com.magnetaros.Slate";
const SLATE_PATH: &str = "/com/magnetaros/Slate";

/// Where Envelope exports its side, on its own owned name.
pub const ENVELOPE_PATH: &str = "/com/magnetaros/Envelope";

/// What handing an invitation to the calendar came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivered {
    /// Slate has the payload and will put its invitation view in front of
    /// the user — asynchronously, and it never talks back about it here.
    Accepted,
    /// Slate answered, and declined the payload.
    Refused,
    /// Slate's name is unowned: the feature is absent, and the caller should
    /// offer the fallback instead of an error.
    NoCalendar,
}

/// Who mailed a message, for the calendar's organizer check: its first
/// `From` address, without the display name. Empty for a message that names
/// nobody, which the calendar then refuses — an invitation from no one is
/// not from its organizer.
#[must_use]
pub fn sender_of(message: &Message) -> String {
    message
        .sender()
        .map(|from| from.address.clone())
        .unwrap_or_default()
}

/// Hands the verbatim `text/calendar` part to Slate, if Slate is running,
/// with the address of whoever mailed it.
pub async fn deliver_invitation(
    conn: &zbus::Connection,
    ics: String,
    account_id: String,
    sender: String,
) -> Result<Delivered, String> {
    let dbus = zbus::fdo::DBusProxy::new(conn)
        .await
        .map_err(|why| why.to_string())?;
    let name = zbus::names::BusName::try_from(SLATE_NAME).map_err(|why| why.to_string())?;
    if !dbus
        .name_has_owner(name)
        .await
        .map_err(|why| why.to_string())?
    {
        return Ok(Delivered::NoCalendar);
    }

    let sent = conn
        .call_method(
            Some(SLATE_NAME),
            SLATE_PATH,
            Some(INTERFACE),
            "DeliverInvitation2",
            &(&ics, &account_id, &sender),
        )
        .await;
    let reply = match sent {
        // A Slate from before `DeliverInvitation2`. It gets the payload the
        // way it knows, and checks it without a sender.
        Err(zbus::Error::MethodError(error, ..))
            if error.as_str() == "org.freedesktop.DBus.Error.UnknownMethod" =>
        {
            conn.call_method(
                Some(SLATE_NAME),
                SLATE_PATH,
                Some(INTERFACE),
                "DeliverInvitation",
                &(&ics, &account_id),
            )
            .await
        }
        sent => sent,
    }
    .map_err(|why| why.to_string())?;
    let accepted: bool = reply.body().deserialize().map_err(|why| why.to_string())?;
    Ok(if accepted {
        Delivered::Accepted
    } else {
        Delivered::Refused
    })
}

/// What the mailer side of the contract runs on: the accounts a reply may go
/// out from, and what starts a queued reply on its way.
///
/// The app's is [`Mailer::live`]. They are two functions, rather than calls
/// written into the interface, so the interface can be served on a private
/// bus over a temporary maildir, with no account store, keyring or mail
/// server behind it.
#[derive(Clone)]
pub struct Mailer {
    connections: Arc<dyn Fn() -> Vec<Connection> + Send + Sync>,
    drain: Arc<dyn Fn(&Connection) + Send + Sync>,
}

impl Mailer {
    /// Every mail account on this machine, and a send that starts at once.
    #[must_use]
    pub fn live() -> Self {
        Self {
            connections: Arc::new(crate::mail::all_connections),
            drain: Arc::new(|connection| {
                let drained = crate::mail::drain_due(
                    std::slice::from_ref(connection),
                    chrono::Utc::now().timestamp_millis(),
                );
                for (account, why) in drained.failures {
                    tracing::warn!(account, why, "a scheduling reply waits in the outbox");
                }
            }),
        }
    }

    /// Queues the `METHOD:REPLY` text Slate built, to the organizer, from
    /// the named account — as `from` when that is one of its identities.
    /// `true` is the promise that the reply is in the durable outbox —
    /// queued, not delivered; the outbox owns retries and the honest cancel
    /// from there.
    async fn send(
        &self,
        ics: String,
        account_id: String,
        to: String,
        from: Option<String>,
    ) -> bool {
        let connections = self.connections.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            queue_reply(connections(), &ics, &account_id, &to, from.as_deref())
        })
        .await;
        match outcome {
            Ok(Ok(connection)) => {
                // Queued is the promise; sending starts now rather than at
                // the next poll, and whichever account the window shows.
                // Not awaited: the caller asked for the queueing, and the
                // outbox owns what happens from here.
                let drain = self.drain.clone();
                tokio::task::spawn_blocking(move || drain(&connection));
                true
            }
            Ok(Err(why)) => {
                tracing::warn!(why, "a scheduling reply could not be queued");
                false
            }
            Err(why) => {
                tracing::warn!(%why, "a scheduling reply could not be queued");
                false
            }
        }
    }
}

/// Exports the mailer side of the contract on `conn`: `Scheduling2`, and
/// `Scheduling1` beside it for a Slate that does not know the newer one.
pub async fn serve(conn: &zbus::Connection, mailer: Mailer) -> zbus::Result<()> {
    let server = conn.object_server();
    server.at(ENVELOPE_PATH, Scheduling(mailer.clone())).await?;
    server.at(ENVELOPE_PATH, Scheduling2(mailer)).await?;
    Ok(())
}

/// The mailer side of the contract as first published: a reply, sent from
/// the account's primary address.
struct Scheduling(Mailer);

#[zbus::interface(name = "com.magnetaros.CosmicPim.Scheduling1")]
impl Scheduling {
    async fn send_scheduling_reply(&self, ics: String, account_id: String, to: String) -> bool {
        self.0.send(ics, account_id, to, None).await
    }
}

/// The mailer side of the contract: a reply, and the address it answers as.
struct Scheduling2(Mailer);

#[zbus::interface(name = "com.magnetaros.CosmicPim.Scheduling2")]
impl Scheduling2 {
    async fn send_scheduling_reply(
        &self,
        ics: String,
        account_id: String,
        to: String,
        from: String,
    ) -> bool {
        self.0.send(ics, account_id, to, Some(from)).await
    }
}

/// Builds the reply message and drops it in the account's outbox, returning
/// the account's connection for the drain that follows.
///
/// The reply goes out as `from` when the account has that identity — an
/// invitation that reached an alias is answered from the alias — and from
/// the primary one otherwise: an address the account does not send as is one
/// its server would refuse or rewrite.
///
/// Blocking — disk and credentials — which is why [`Mailer::send`] wraps it
/// in `spawn_blocking`.
fn queue_reply(
    connections: Vec<Connection>,
    ics: &str,
    account_id: &str,
    to: &str,
    from: Option<&str>,
) -> Result<Connection, String> {
    let connection = connections
        .into_iter()
        .find(|connection| connection.account_id == account_id)
        .ok_or_else(|| format!("no mail account is configured as {account_id}"))?;
    let primary = connection
        .identities
        .first()
        .ok_or_else(|| "that account has no From address".to_owned())?;
    let identity = from
        .map(bare)
        .and_then(|from| {
            connection
                .identities
                .iter()
                .find(|identity| identity.address.eq_ignore_ascii_case(from))
        })
        .unwrap_or(primary)
        .clone();

    let mut draft = Draft::new(identity);
    draft.to.push(Mailbox {
        name: None,
        address: bare(to).to_owned(),
    });
    // Presentation only — the SUMMARY line names the event for clients that
    // show the message before the calendar part. Anything more about the
    // payload's meaning belongs to the library that built it.
    draft.subject = match property(ics, "SUMMARY:") {
        Some(summary) => format!("Re: {summary}"),
        None => crate::fl!("scheduling-reply-subject"),
    };
    draft.body = crate::fl!("scheduling-reply-body");
    draft.attachments.push(Attachment {
        name: "reply.ics".to_owned(),
        // The method parameter is what makes receiving software treat this
        // as a scheduling message rather than a file (RFC 6047).
        mime_type: "text/calendar; method=REPLY; charset=utf-8".to_owned(),
        bytes: ics.as_bytes().to_vec(),
    });

    let id = crate::mail::fresh_id(&connection)?;
    crate::mail::outbox(&connection)?
        .submit(&id, &draft, chrono::Utc::now().timestamp_millis())
        .map_err(|why| why.to_string())?;
    Ok(connection)
}

/// An address as the contract may spell it: bare, or as a `mailto:` URI.
fn bare(address: &str) -> &str {
    let address = address.trim();
    address.strip_prefix("mailto:").unwrap_or(address)
}

/// The value of a top-level iCalendar property, by its `NAME:` prefix.
fn property(ics: &str, prefix: &str) -> Option<String> {
    ics.lines()
        .map(str::trim_end)
        .find_map(|line| line.strip_prefix(prefix).map(str::to_owned))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testbus::PrivateBus;
    use std::sync::Mutex;
    use std::time::Duration;

    const REQUEST: &str = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nEND:VCALENDAR\r\n";
    const REPLY: &str = "BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nSUMMARY:Standup\r\nEND:VCALENDAR\r\n";
    const ENVELOPE_NAME: &str = "com.magnetaros.Envelope";
    const REPLY_INTERFACE: &str = "com.magnetaros.CosmicPim.Scheduling2";

    #[test]
    fn the_summary_line_names_the_event() {
        assert_eq!(property(REPLY, "SUMMARY:").unwrap(), "Standup");
        assert_eq!(property(REPLY, "LOCATION:"), None);
    }

    #[test]
    fn the_sender_is_the_from_address_without_its_name() {
        let message = Message::parse(
            b"From: \"Boss, The\" <Boss@Example.com>\r\nSubject: Review\r\n\r\nSee attached.\r\n",
        )
        .unwrap();
        assert_eq!(sender_of(&message), "boss@example.com");

        let nobody = Message::parse(b"Subject: Review\r\n\r\nSee attached.\r\n").unwrap();
        assert_eq!(sender_of(&nobody), "");
    }

    /// The calls a stand-in Slate received, each as its arguments.
    type Seen = Arc<Mutex<Vec<Vec<String>>>>;

    /// Slate as first released: `DeliverInvitation`, and nothing else.
    struct SlateBefore(Seen);

    #[zbus::interface(name = "com.magnetaros.CosmicPim.Scheduling1")]
    impl SlateBefore {
        fn deliver_invitation(&self, ics: String, account_id: String) -> bool {
            self.0.lock().unwrap().push(vec![ics, account_id]);
            true
        }
    }

    /// Slate with `DeliverInvitation2` beside the older method.
    struct SlateNow(Seen);

    #[zbus::interface(name = "com.magnetaros.CosmicPim.Scheduling1")]
    impl SlateNow {
        fn deliver_invitation(&self, ics: String, account_id: String) -> bool {
            self.0.lock().unwrap().push(vec![ics, account_id]);
            true
        }

        #[zbus(name = "DeliverInvitation2")]
        fn deliver_invitation2(&self, ics: String, account_id: String, sender: String) -> bool {
            self.0.lock().unwrap().push(vec![ics, account_id, sender]);
            true
        }
    }

    /// A stand-in Slate on a private bus, and Envelope's connection to it.
    async fn slate_on_private_bus(
        current: bool,
    ) -> (PrivateBus, zbus::Connection, zbus::Connection, Seen) {
        let bus = PrivateBus::start();
        let seen = Seen::default();
        let slate = bus.connect().await;
        let server = slate.object_server();
        if current {
            server.at(SLATE_PATH, SlateNow(seen.clone())).await.unwrap();
        } else {
            server
                .at(SLATE_PATH, SlateBefore(seen.clone()))
                .await
                .unwrap();
        }
        slate.request_name(SLATE_NAME).await.unwrap();
        let envelope = bus.connect().await;
        (bus, slate, envelope, seen)
    }

    #[tokio::test]
    async fn an_invitation_is_delivered_with_who_mailed_it() {
        let (_bus, _slate, envelope, seen) = slate_on_private_bus(true).await;

        let delivered = deliver_invitation(
            &envelope,
            REQUEST.into(),
            "work".into(),
            "boss@example.com".into(),
        )
        .await;

        assert_eq!(delivered, Ok(Delivered::Accepted));
        assert_eq!(
            *seen.lock().unwrap(),
            [[REQUEST, "work", "boss@example.com"]]
        );
    }

    #[tokio::test]
    async fn a_slate_from_before_the_sender_still_gets_the_invitation() {
        let (_bus, _slate, envelope, seen) = slate_on_private_bus(false).await;

        let delivered = deliver_invitation(
            &envelope,
            REQUEST.into(),
            "work".into(),
            "boss@example.com".into(),
        )
        .await;

        assert_eq!(delivered, Ok(Delivered::Accepted));
        assert_eq!(*seen.lock().unwrap(), [[REQUEST, "work"]]);
    }

    #[tokio::test]
    async fn without_a_calendar_there_is_nothing_to_deliver_to() {
        let bus = PrivateBus::start();
        let envelope = bus.connect().await;

        let delivered =
            deliver_invitation(&envelope, String::new(), "work".into(), String::new()).await;

        assert_eq!(delivered, Ok(Delivered::NoCalendar));
    }

    /// The account `work`: me@work.example, which also sends as
    /// sales@work.example.
    fn work(root: &std::path::Path) -> Connection {
        let mut connection = crate::mail::offline_connection("work", root);
        connection.identities.push(Mailbox {
            name: Some("Sales".into()),
            address: "sales@work.example".into(),
        });
        connection
    }

    /// Envelope serving the contract on a private bus over the maildir at
    /// `root`, Slate's connection to it, and the accounts whose drain was
    /// started.
    async fn envelope_on_private_bus(
        root: &std::path::Path,
    ) -> (
        PrivateBus,
        zbus::Connection,
        zbus::Connection,
        std::sync::mpsc::Receiver<String>,
    ) {
        let bus = PrivateBus::start();
        let (drained, drains) = std::sync::mpsc::channel();
        let root = root.to_path_buf();
        let mailer = Mailer {
            connections: Arc::new(move || vec![work(&root)]),
            drain: Arc::new(move |connection| {
                // A test that does not look at the drains has dropped the
                // receiver by now.
                let _ = drained.send(connection.account_id.clone());
            }),
        };
        let envelope = bus.connect().await;
        serve(&envelope, mailer).await.unwrap();
        envelope.request_name(ENVELOPE_NAME).await.unwrap();
        let slate = bus.connect().await;
        (bus, envelope, slate, drains)
    }

    /// Who the one queued message is from and to.
    fn queued(root: &std::path::Path) -> (String, String) {
        let queued = crate::mail::list_outbox(&work(root)).unwrap();
        assert_eq!(queued.len(), 1, "one reply was queued");
        let draft = &queued[0].draft;
        (draft.from.address.clone(), draft.to[0].address.clone())
    }

    #[tokio::test]
    async fn a_reply_goes_out_from_the_address_the_invitation_was_sent_to() {
        let root = tempfile::tempdir().unwrap();
        let (_bus, _envelope, slate, drains) = envelope_on_private_bus(root.path()).await;

        let reply = slate
            .call_method(
                Some(ENVELOPE_NAME),
                ENVELOPE_PATH,
                Some(REPLY_INTERFACE),
                "SendSchedulingReply",
                &(
                    REPLY,
                    "work",
                    "mailto:boss@example.com",
                    "Sales@work.example",
                ),
            )
            .await
            .unwrap();

        assert!(reply.body().deserialize::<bool>().unwrap());
        assert_eq!(
            queued(root.path()),
            (
                "sales@work.example".to_owned(),
                "boss@example.com".to_owned()
            )
        );
        // Queued is sent at once, not at the next poll.
        assert_eq!(
            drains.recv_timeout(Duration::from_secs(10)).unwrap(),
            "work"
        );
    }

    #[tokio::test]
    async fn a_reply_as_an_address_the_account_does_not_have_goes_from_the_primary() {
        let root = tempfile::tempdir().unwrap();
        let (_bus, _envelope, slate, _drains) = envelope_on_private_bus(root.path()).await;

        let reply = slate
            .call_method(
                Some(ENVELOPE_NAME),
                ENVELOPE_PATH,
                Some(REPLY_INTERFACE),
                "SendSchedulingReply",
                &(REPLY, "work", "boss@example.com", "someone@else.example"),
            )
            .await
            .unwrap();

        assert!(reply.body().deserialize::<bool>().unwrap());
        assert_eq!(queued(root.path()).0, "me@work.example");
    }

    #[tokio::test]
    async fn a_slate_from_before_scheduling2_is_still_served() {
        let root = tempfile::tempdir().unwrap();
        let (_bus, _envelope, slate, _drains) = envelope_on_private_bus(root.path()).await;

        let reply = slate
            .call_method(
                Some(ENVELOPE_NAME),
                ENVELOPE_PATH,
                Some(INTERFACE),
                "SendSchedulingReply",
                &(REPLY, "work", "boss@example.com"),
            )
            .await
            .unwrap();

        assert!(reply.body().deserialize::<bool>().unwrap());
        assert_eq!(
            queued(root.path()),
            ("me@work.example".to_owned(), "boss@example.com".to_owned())
        );
    }

    #[tokio::test]
    async fn a_reply_for_an_account_that_is_not_there_is_not_queued() {
        let root = tempfile::tempdir().unwrap();
        let (_bus, _envelope, slate, _drains) = envelope_on_private_bus(root.path()).await;

        let reply = slate
            .call_method(
                Some(ENVELOPE_NAME),
                ENVELOPE_PATH,
                Some(REPLY_INTERFACE),
                "SendSchedulingReply",
                &(REPLY, "home", "boss@example.com", "me@home.example"),
            )
            .await
            .unwrap();

        assert!(!reply.body().deserialize::<bool>().unwrap());
    }
}
