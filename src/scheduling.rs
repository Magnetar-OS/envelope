// SPDX-License-Identifier: GPL-3.0-only

//! The iMIP hand-off: Envelope's side of the one cross-process contract.
//!
//! The payload of an invitation arrives in mail; the decision about it
//! belongs in the calendar. The two are separate processes, so this — and
//! only this — crosses D-Bus, on the `Scheduling1` interface each app
//! exports at its own single-instance name (the contract lives in
//! cosmic-pim's ARCHITECTURE.md). Envelope moves bytes and names an
//! account; the iTIP semantics stay in the calendar library.
//!
//! Degradation is part of the contract: Slate's name being unowned means the
//! feature is absent, not broken. Nothing here calls `StartServiceByName` —
//! an invitation must not *launch* a calendar — and the caller falls back to
//! the save-the-file affordance the reader already has.

use cosmic_pim_mail::compose::{Attachment, Draft};
use cosmic_pim_mail::model::Mailbox;

/// The shared interface name. The `1` suffix is the versioning policy: a
/// breaking change is a new name exported alongside this one, never a
/// changed signature under it.
const INTERFACE: &str = "io.github.entro314labs.CosmicPim.Scheduling1";

const SLATE_NAME: &str = "io.github.entro314labs.Slate";
const SLATE_PATH: &str = "/io/github/entro314labs/Slate";

/// Where Envelope exports [`Scheduling`], on its own owned name.
pub const ENVELOPE_PATH: &str = "/io/github/entro314labs/Envelope";

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

/// Hands the verbatim `text/calendar` part to Slate, if Slate is running.
pub async fn deliver_invitation(
    conn: &zbus::Connection,
    ics: String,
    account_id: String,
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

    let reply = conn
        .call_method(
            Some(SLATE_NAME),
            SLATE_PATH,
            Some(INTERFACE),
            "DeliverInvitation",
            &(ics, account_id),
        )
        .await
        .map_err(|why| why.to_string())?;
    let accepted: bool = reply.body().deserialize().map_err(|why| why.to_string())?;
    Ok(if accepted {
        Delivered::Accepted
    } else {
        Delivered::Refused
    })
}

/// The mailer side of the contract, exported on Envelope's own name.
pub struct Scheduling;

#[zbus::interface(name = "io.github.entro314labs.CosmicPim.Scheduling1")]
impl Scheduling {
    /// Queues the `METHOD:REPLY` text Slate built, to the organizer, from
    /// the named account. `true` is the promise that the reply is in the
    /// durable outbox — queued, not delivered; the outbox owns retries and
    /// the honest cancel from there.
    async fn send_scheduling_reply(&self, ics: String, account_id: String, to: String) -> bool {
        let outcome =
            tokio::task::spawn_blocking(move || queue_reply(&ics, &account_id, &to)).await;
        match outcome {
            Ok(Ok(())) => true,
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

/// Builds the reply message and drops it in the account's outbox.
///
/// Blocking — disk and credentials — which is why the interface method wraps
/// it in `spawn_blocking`.
fn queue_reply(ics: &str, account_id: &str, to: &str) -> Result<(), String> {
    let connection = crate::mail::all_connections()
        .into_iter()
        .find(|connection| connection.account_id == account_id)
        .ok_or_else(|| format!("no mail account is configured as {account_id}"))?;
    let from = connection
        .identities
        .first()
        .cloned()
        .ok_or_else(|| "that account has no From address".to_owned())?;

    let mut draft = Draft::new(from);
    draft.to.push(Mailbox {
        name: None,
        address: to.strip_prefix("mailto:").unwrap_or(to).to_owned(),
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

    let now_ms = chrono::Utc::now().timestamp_millis();
    crate::mail::outbox(&connection)?
        .submit(&cosmic_pim_mail::drafts::new_id(now_ms), &draft, now_ms)
        .map_err(|why| why.to_string())
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

    #[test]
    fn the_summary_line_names_the_event() {
        let ics = "BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nSUMMARY:Standup\r\nEND:VCALENDAR\r\n";
        assert_eq!(property(ics, "SUMMARY:").unwrap(), "Standup");
        assert_eq!(property(ics, "LOCATION:"), None);
    }
}
