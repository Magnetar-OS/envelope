// SPDX-License-Identifier: GPL-3.0-only

//! The application's data path, against a real maildir on disk.
//!
//! The substrate's own `live_sync.rs` proves the IMAP half — a real client, a
//! real socket, into a real maildir. This proves the other half: that what
//! Envelope reads back out of that maildir is what the window shows, and that a
//! change made in the window becomes both a local fact and a queued write.
//!
//! No server is involved on purpose. Everything here happens while the network
//! is down, which is when a mail client is most often actually used.

use cosmic_pim_mail::folder::{self, Folder};
use cosmic_pim_mail::imap::{Endpoint, Security};
use cosmic_pim_mail::maildir::{self, MaildirStore};
use cosmic_pim_mail::model::Flags;
use cosmic_pim_mail::push::PushQueue;
use cosmic_pim_mail::smtp::{Outcome, SmtpEndpoint};
use cosmic_pim_mail::store::{MailStore, RemoteMessage};
use envelope::mail::{self, Connection};

const ACCOUNT: &str = "account-1";

fn inbox() -> Folder {
    folder::from_list_entry("INBOX", Some('/'), &[])
}

fn archive() -> Folder {
    folder::from_list_entry("Archive", Some('/'), &[])
}

fn connection(root: &std::path::Path) -> Connection {
    let mut account =
        cosmic_pim_accounts::Account::new("Test", "https://dav.example/", "me@example.com");
    account.mail = Some(cosmic_pim_accounts::MailEndpoint::tls("unused.invalid"));
    Connection {
        account,
        account_id: ACCOUNT.into(),
        endpoint: Endpoint {
            host: "unused.invalid".into(),
            port: 993,
            security: Security::Tls,
            username: "me".into(),
        },
        submission: Some(envelope::mail::Submission {
            endpoint: SmtpEndpoint {
                host: "unused.invalid".into(),
                port: 465,
                security: Security::Tls,
                username: "me".into(),
            },
            identity: cosmic_pim_mail::Mailbox {
                name: Some("Me".into()),
                address: "me@example.com".into(),
            },
        }),
        identities: vec![cosmic_pim_mail::Mailbox {
            name: Some("Me".into()),
            address: "me@example.com".into(),
        }],
        credentials: cosmic_pim_mail::sasl::Credentials::Password(String::new()),
        root: root.to_path_buf(),
        // Per-test, so tests neither collide on one file nor see each other's
        // threads. The app's default is the shared cache path.
        index_path: root.join("index.sqlite"),
    }
}

/// Delivers messages into the account's INBOX maildir, as a sync would have.
fn deliver(root: &std::path::Path, messages: &[(u32, &str, Flags)]) {
    let path = maildir::mailbox_path(root, ACCOUNT, &inbox());
    let mut store = MaildirStore::open(path).expect("open the maildir");
    for (uid, raw, flags) in messages {
        store
            .upsert(&RemoteMessage {
                uid: *uid,
                flags: *flags,
                raw: raw.as_bytes().to_vec(),
                internal_date_ms: i64::from(*uid) * 60_000,
            })
            .expect("deliver");
    }
}

fn message(
    id: &str,
    references: &str,
    subject: &str,
    from: &str,
    date: &str,
    body: &str,
) -> String {
    format!(
        "Message-ID: <{id}>\r\n\
         From: {from}\r\n\
         To: me@example.com\r\n\
         References: {references}\r\n\
         Subject: {subject}\r\n\
         Date: {date}\r\n\
         \r\n\
         {body}\r\n"
    )
}

#[test]
fn a_mailbox_reads_back_as_conversations_newest_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let hello = message(
        "hello@x",
        "",
        "Release plan",
        "Ada <ada@example.com>",
        "Mon, 3 Feb 2025 09:00:00 +0000",
        "Here is the plan.",
    );
    let reply = message(
        "reply@x",
        "<hello@x>",
        "Re: Release plan",
        "Bob <bob@example.net>",
        "Mon, 3 Feb 2025 10:00:00 +0000",
        "Looks good to me.",
    );
    let lunch = message(
        "lunch@x",
        "",
        "Lunch?",
        "Cleo <cleo@example.org>",
        "Tue, 4 Feb 2025 12:00:00 +0000",
        "One o'clock?",
    );

    deliver(
        root,
        &[
            (
                1,
                &hello,
                Flags {
                    seen: true,
                    ..Flags::default()
                },
            ),
            (2, &reply, Flags::default()),
            (3, &lunch, Flags::default()),
        ],
    );

    let conversations = mail::conversations(&connection(root), &inbox())
        .expect("read the mailbox")
        .0;

    assert_eq!(conversations.len(), 2, "the reply did not join its parent");
    assert_eq!(
        conversations[0].subject, "Lunch?",
        "the list is not newest-first: {conversations:?}"
    );

    let thread = &conversations[1];
    assert_eq!(
        thread.subject, "Release plan",
        "the thread kept a Re: prefix"
    );
    assert_eq!(thread.uids, vec![1, 2]);
    assert_eq!(thread.participants, ["Ada", "Bob"]);
    assert!(
        thread.unread,
        "a thread with one unread reply must read as unread"
    );
    assert_eq!(thread.snippet, "Looks good to me.");
    assert_eq!(thread.newest_uid(), Some(2));
}

#[test]
fn an_empty_or_missing_folder_is_not_an_error() {
    // The sidebar lists folders the server has; the user clicks one before it
    // has ever been synced. That must show an empty list, not a failure.
    let dir = tempfile::tempdir().expect("tempdir");
    let conversations = mail::conversations(&connection(dir.path()), &archive())
        .expect("no maildir yet")
        .0;
    assert!(conversations.is_empty());
}

#[test]
fn opening_a_message_extracts_what_a_human_would_see_and_flags_what_they_would_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let raw = "Message-ID: <html@x>\r\n\
From: Sender <sender@example.com>\r\n\
Subject: Newsletter\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<p>Visible text.</p>\
<div style=\"display:none\">Instructions you were not meant to read.</div>\
<img src=\"https://tracker.example/pixel.gif\">\r\n";

    deliver(root, &[(1, raw, Flags::default())]);

    let opened = mail::open(&connection(root), &inbox(), 1).expect("open");

    assert_eq!(opened.message.body.text, "Visible text.");
    assert_eq!(
        opened.message.body.hidden_elided, 1,
        "the reader would not have told the user anything was hidden"
    );
    assert!(
        opened.message.has_remote_content,
        "the tracking pixel was not noticed"
    );
}

#[test]
fn opening_a_message_that_is_gone_says_so_rather_than_panicking() {
    let dir = tempfile::tempdir().expect("tempdir");
    deliver(
        dir.path(),
        &[(
            1,
            &message("a@x", "", "s", "a@x", "", "b"),
            Flags::default(),
        )],
    );
    let error = mail::open(&connection(dir.path()), &inbox(), 99).expect_err("no such message");
    assert!(error.contains("no longer"), "{error}");
}

#[test]
fn a_flag_change_is_applied_locally_and_queued_for_the_server() {
    // Both halves. Local-only reverts on the next sync; queue-only leaves the
    // window showing yesterday's state until the network comes back.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    deliver(
        root,
        &[(
            1,
            &message("a@x", "", "Subject", "a@x", "", "body"),
            Flags::default(),
        )],
    );

    mail::set_flags(&connection(root), &inbox(), &[1], |flags| Flags {
        seen: true,
        ..flags
    })
    .expect("set flags");

    let store = MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
    assert!(
        store.state().expect("state").entries[&1].seen,
        "the change never reached the file"
    );
    assert_eq!(
        store.pending().expect("pending").len(),
        1,
        "the change will never reach the server"
    );
}

#[test]
fn a_flag_change_that_changes_nothing_queues_nothing() {
    // Re-opening a message the user has already read must not enqueue a write
    // every time.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    deliver(
        root,
        &[(
            1,
            &message("a@x", "", "Subject", "a@x", "", "body"),
            Flags {
                seen: true,
                ..Flags::default()
            },
        )],
    );

    mail::set_flags(&connection(root), &inbox(), &[1], |flags| Flags {
        seen: true,
        ..flags
    })
    .expect("set flags");

    let store = MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
    assert!(
        store.pending().expect("pending").is_empty(),
        "an unchanged flag was queued"
    );
}

#[test]
fn archiving_a_conversation_takes_all_of_it_and_queues_the_move() {
    // Half a conversation left in the inbox is not what anybody means by
    // "archive".
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    deliver(
        root,
        &[
            (
                1,
                &message("a@x", "", "Plan", "a@x", "", "one"),
                Flags::default(),
            ),
            (
                2,
                &message("b@x", "<a@x>", "Re: Plan", "b@x", "", "two"),
                Flags::default(),
            ),
        ],
    );

    let connection = connection(root);
    let conversation = mail::conversations(&connection, &inbox()).expect("read").0[0].clone();
    assert_eq!(conversation.uids.len(), 2);

    mail::move_to(&connection, &inbox(), &archive(), &conversation.uids).expect("move");

    let store = MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
    assert!(
        store.state().expect("state").entries.is_empty(),
        "the archived conversation is still in the inbox"
    );
    assert_eq!(
        store.pending().expect("pending").len(),
        2,
        "the move will never reach the server"
    );

    // And it survives a restart, because an archive made on a train has to.
    let reopened =
        MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("reopen");
    assert_eq!(reopened.pending().expect("pending").len(), 2);
}

#[test]
fn a_message_with_no_date_sorts_last_rather_than_first() {
    // A broken Date header must not pin a message to the top of the list
    // forever.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    deliver(
        root,
        &[
            (
                1,
                &message("a@x", "", "Undated", "a@x", "not a date at all", "one"),
                Flags::default(),
            ),
            (
                2,
                &message(
                    "b@x",
                    "",
                    "Dated",
                    "b@x",
                    "Mon, 3 Feb 2025 09:00:00 +0000",
                    "two",
                ),
                Flags::default(),
            ),
        ],
    );

    let conversations = mail::conversations(&connection(root), &inbox())
        .expect("read")
        .0;
    assert_eq!(conversations.len(), 2);
    assert_eq!(conversations[0].subject, "Dated");
    assert_eq!(conversations[1].subject, "Undated");
}

#[test]
fn the_index_is_a_cache_and_deleting_it_costs_only_a_rescan() {
    // The suite's promise made operational: the maildir is the truth, and
    // everything derived from it can be thrown away.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    deliver(
        root,
        &[
            (
                1,
                &message(
                    "a@x",
                    "",
                    "Plan",
                    "a@x",
                    "Mon, 3 Feb 2025 09:00:00 +0000",
                    "one",
                ),
                Flags::default(),
            ),
            (
                2,
                &message(
                    "b@x",
                    "<a@x>",
                    "Re: Plan",
                    "b@x",
                    "Mon, 3 Feb 2025 10:00:00 +0000",
                    "two",
                ),
                Flags::default(),
            ),
        ],
    );

    let connection = connection(root);
    let before = mail::conversations(&connection, &inbox()).expect("read").0;
    assert_eq!(before.len(), 1);

    std::fs::remove_file(&connection.index_path).expect("delete the cache");
    let after = mail::conversations(&connection, &inbox())
        .expect("read again")
        .0;
    assert_eq!(
        after, before,
        "the mailbox did not survive losing its cache"
    );
}

#[test]
fn a_draft_survives_being_closed_and_reopened() {
    // The whole reason drafts exist: closing the composer must not lose what
    // was typed.
    let dir = tempfile::tempdir().expect("tempdir");
    let connection = connection(dir.path());

    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: Some("Me".into()),
        address: "me@example.com".into(),
    });
    draft.to.push(cosmic_pim_mail::Mailbox {
        name: None,
        address: "ada@example.com".into(),
    });
    draft.subject = "Half-written".into();
    draft.body = "This is as far as I got.".into();

    let id = mail::save_draft(&connection, None, &draft).expect("save");
    let listed = mail::list_drafts(&connection).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].subject, "Half-written");

    let reopened = mail::load_draft(&connection, &id)
        .expect("load")
        .expect("the draft is still there");
    assert_eq!(reopened.subject, "Half-written");
    assert_eq!(reopened.body.trim(), "This is as far as I got.");
    assert_eq!(reopened.to[0].address, "ada@example.com");
}

#[test]
fn re_saving_a_draft_replaces_it_rather_than_leaving_a_trail() {
    let dir = tempfile::tempdir().expect("tempdir");
    let connection = connection(dir.path());

    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: None,
        address: "me@example.com".into(),
    });
    draft.subject = "First".into();

    let id = mail::save_draft(&connection, None, &draft).expect("save");
    draft.subject = "Second".into();
    let same = mail::save_draft(&connection, Some(&id), &draft).expect("re-save");

    assert_eq!(same, id, "re-saving minted a new id");
    let listed = mail::list_drafts(&connection).expect("list");
    assert_eq!(listed.len(), 1, "every edit left a file: {listed:?}");
    assert_eq!(listed[0].subject, "Second");
}

#[test]
fn deleting_a_draft_removes_it_and_doing_so_twice_is_fine() {
    let dir = tempfile::tempdir().expect("tempdir");
    let connection = connection(dir.path());
    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: None,
        address: "me@example.com".into(),
    });
    draft.subject = "Gone".into();

    let id = mail::save_draft(&connection, None, &draft).expect("save");
    mail::delete_draft(&connection, &id).expect("delete");
    mail::delete_draft(&connection, &id).expect("a retried send must not fail here");
    assert!(mail::list_drafts(&connection).expect("list").is_empty());
}

#[test]
fn drafts_do_not_appear_as_a_mailbox_to_anything_reading_the_maildirs() {
    // A sync engine that adopted them would try to give them UIDs.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let connection = connection(root);
    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: None,
        address: "me@example.com".into(),
    });
    draft.subject = "Not a mailbox".into();
    mail::save_draft(&connection, None, &draft).expect("save");

    let account_root = root.join(ACCOUNT);
    let visible: Vec<String> = std::fs::read_dir(&account_root)
        .expect("read the account root")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();
    assert!(
        !visible.iter().any(|name| name.contains("draft")),
        "the draft store is visible as a mailbox: {visible:?}"
    );
}

#[test]
fn a_search_finds_messages_by_sender_and_subject_across_folders() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    deliver(
        root,
        &[
            (
                1,
                &message(
                    "a@x",
                    "",
                    "Invoice 42 overdue",
                    "Ada <ada@example.com>",
                    "Mon, 3 Feb 2025 09:00:00 +0000",
                    "Please pay.",
                ),
                Flags::default(),
            ),
            (
                2,
                &message(
                    "b@x",
                    "",
                    "Release plan",
                    "Bob <bob@example.net>",
                    "Mon, 3 Feb 2025 10:00:00 +0000",
                    "Draft attached.",
                ),
                Flags {
                    seen: true,
                    ..Flags::default()
                },
            ),
        ],
    );

    let connection = connection(root);
    // Reading the folder is what populates the index.
    mail::conversations(&connection, &inbox()).expect("read");
    let folders = [inbox()];

    let hits = mail::search(&connection, &folders, "invoice", 50).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].subject, "Invoice 42 overdue");
    assert_eq!(hits[0].mailbox, "INBOX", "a hit must say where it is");

    assert_eq!(
        mail::search(&connection, &folders, "from:bob", 50)
            .expect("search")
            .len(),
        1
    );
    assert!(
        mail::search(&connection, &folders, "from:ada release", 50)
            .expect("search")
            .is_empty(),
        "two terms behaved as OR"
    );
}

#[test]
fn flag_filters_are_applied_against_the_maildir_not_the_index() {
    // Flags are not cached, so `is:unread` has to be answered from the store.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    deliver(
        root,
        &[
            (
                1,
                &message(
                    "a@x",
                    "",
                    "Report one",
                    "a@x",
                    "Mon, 3 Feb 2025 09:00:00 +0000",
                    "x",
                ),
                Flags::default(),
            ),
            (
                2,
                &message(
                    "b@x",
                    "",
                    "Report two",
                    "b@x",
                    "Mon, 3 Feb 2025 10:00:00 +0000",
                    "x",
                ),
                Flags {
                    seen: true,
                    ..Flags::default()
                },
            ),
        ],
    );

    let connection = connection(root);
    mail::conversations(&connection, &inbox()).expect("read");
    let folders = [inbox()];

    assert_eq!(
        mail::search(&connection, &folders, "report", 50)
            .expect("search")
            .len(),
        2
    );
    let unread = mail::search(&connection, &folders, "report is:unread", 50).expect("search");
    assert_eq!(unread.len(), 1);
    assert_eq!(unread[0].subject, "Report one");

    // And it tracks the store rather than a snapshot.
    mail::set_flags(&connection, &inbox(), &[1], |flags| Flags {
        seen: true,
        ..flags
    })
    .expect("mark read");
    assert!(
        mail::search(&connection, &folders, "report is:unread", 50)
            .expect("search")
            .is_empty(),
        "the filter used a stale read mark"
    );
}

#[test]
fn an_empty_search_returns_nothing_rather_than_the_mailbox() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    deliver(
        root,
        &[(
            1,
            &message(
                "a@x",
                "",
                "Anything",
                "a@x",
                "Mon, 3 Feb 2025 09:00:00 +0000",
                "x",
            ),
            Flags::default(),
        )],
    );
    let connection = connection(root);
    mail::conversations(&connection, &inbox()).expect("read");

    assert!(
        mail::search(&connection, &[inbox()], "", 50)
            .expect("search")
            .is_empty()
    );
    assert!(
        mail::search(&connection, &[inbox()], "   ", 50)
            .expect("search")
            .is_empty()
    );
}

const WITH_ATTACHMENT: &str = "Message-ID: <att@x>\r\n\
From: Ada <ada@example.com>\r\n\
Subject: Here it is\r\n\
Date: Mon, 3 Feb 2025 09:00:00 +0000\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
\r\n\
See attached.\r\n\
--b\r\n\
Content-Type: text/csv; name=\"report.csv\"\r\n\
Content-Disposition: attachment; filename=\"../../evil.csv\"\r\n\
\r\n\
a,b\r\n1,2\r\n\
--b--\r\n";

#[test]
fn an_attachment_can_be_saved_and_a_hostile_filename_cannot_escape() {
    // The filename is a string a stranger chose, about to be joined to a path.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    deliver(root, &[(1, WITH_ATTACHMENT, Flags::default())]);

    let connection = connection(root);
    let opened = mail::open(&connection, &inbox(), 1).expect("open");
    assert_eq!(opened.message.attachments.len(), 1);

    let into = root.join("downloads");
    let bytes = cosmic_pim_mail::attachment::bytes_of(
        &{
            let store =
                MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
            store.raw(1).expect("read").expect("uid 1")
        },
        0,
    )
    .expect("extract");
    let saved =
        cosmic_pim_mail::attachment::save_into(&into, &opened.message.attachments[0].name, &bytes)
            .expect("save");

    assert_eq!(
        saved.parent().expect("a parent"),
        into,
        "the file landed outside the folder it was saved into"
    );
    assert_eq!(std::fs::read(&saved).expect("read back"), b"a,b\r\n1,2");
}

#[test]
fn a_composed_message_carries_its_attachment_and_the_recipient_can_read_it() {
    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: Some("Me".into()),
        address: "me@example.com".into(),
    });
    draft.to.push(cosmic_pim_mail::Mailbox {
        name: None,
        address: "ada@example.com".into(),
    });
    draft.subject = "Report".into();
    draft.body = "Attached.".into();
    draft
        .attachments
        .push(cosmic_pim_mail::compose::Attachment {
            name: "report.csv".into(),
            mime_type: "text/csv".into(),
            bytes: b"a,b\n1,2\n".to_vec(),
        });

    let wire = draft.build(false).expect("build").formatted();
    let received = cosmic_pim_mail::Message::parse(&wire).expect("the recipient can parse it");
    assert_eq!(received.attachments.len(), 1);
    assert_eq!(received.attachments[0].name, "report.csv");
    assert_eq!(
        cosmic_pim_mail::attachment::bytes_of(&wire, 0).expect("extract"),
        b"a,b\n1,2\n"
    );
    assert!(
        received.body.text.contains("Attached."),
        "the body was lost"
    );
}

#[test]
fn an_attachment_survives_a_draft_being_saved_and_reopened() {
    // Bytes rather than a path, precisely so a draft saved on Monday and sent
    // on Thursday still has the file.
    let dir = tempfile::tempdir().expect("tempdir");
    let connection = connection(dir.path());

    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: None,
        address: "me@example.com".into(),
    });
    draft.subject = "With a file".into();
    draft
        .attachments
        .push(cosmic_pim_mail::compose::Attachment {
            name: "photo.png".into(),
            mime_type: "image/png".into(),
            bytes: vec![0x89, b'P', b'N', b'G'],
        });

    let id = mail::save_draft(&connection, None, &draft).expect("save");
    let reopened = mail::load_draft(&connection, &id)
        .expect("load")
        .expect("still there");
    assert_eq!(reopened.attachments.len(), 1);
    assert_eq!(reopened.attachments[0].bytes, vec![0x89, b'P', b'N', b'G']);
}

#[test]
fn a_send_that_never_reached_the_server_lands_in_the_outbox() {
    // Written offline. It must be queued, durable, and out of the composer's
    // hands — not lost, and not the user's problem to remember.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut connection = connection(root);
    // Port 1 refuses instantly, which is a definite non-acceptance.
    if let Some(submission) = connection.submission.as_mut() {
        submission.endpoint.host = "127.0.0.1".into();
        submission.endpoint.port = 1;
        submission.endpoint.security = Security::Plaintext;
    }

    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: None,
        address: "me@example.com".into(),
    });
    draft.to.push(cosmic_pim_mail::Mailbox {
        name: None,
        address: "ada@example.com".into(),
    });
    draft.subject = "On a train".into();
    draft.body = "Written offline.".into();

    let sent = mail::send(&connection, &draft, &[], None, None);
    assert!(
        matches!(sent, mail::Sent::Queued),
        "an offline send was not queued: {sent:?}"
    );

    let queued = mail::list_outbox(&connection).expect("list");
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].draft.subject, "On a train");
    assert!(queued[0].is_live(), "it will never be retried");
}

#[test]
fn queueing_a_send_takes_its_draft_with_it() {
    // Leaving both would show the message twice and send it once.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut connection = connection(root);
    if let Some(submission) = connection.submission.as_mut() {
        submission.endpoint.host = "127.0.0.1".into();
        submission.endpoint.port = 1;
        submission.endpoint.security = Security::Plaintext;
    }

    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: None,
        address: "me@example.com".into(),
    });
    draft.to.push(cosmic_pim_mail::Mailbox {
        name: None,
        address: "ada@example.com".into(),
    });
    draft.subject = "Saved then sent".into();

    let id = mail::save_draft(&connection, None, &draft).expect("save");
    assert_eq!(mail::list_drafts(&connection).expect("list").len(), 1);

    let sent = mail::send(&connection, &draft, &[], None, Some(&id));
    assert!(matches!(sent, mail::Sent::Queued), "{sent:?}");

    assert!(
        mail::list_drafts(&connection).expect("list").is_empty(),
        "the message is now in the outbox and in drafts"
    );
    assert_eq!(mail::list_outbox(&connection).expect("list").len(), 1);
}

#[test]
fn a_queued_message_can_be_retried_or_discarded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut connection = connection(root);
    if let Some(submission) = connection.submission.as_mut() {
        submission.endpoint.host = "127.0.0.1".into();
        submission.endpoint.port = 1;
        submission.endpoint.security = Security::Plaintext;
    }

    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: None,
        address: "me@example.com".into(),
    });
    draft.to.push(cosmic_pim_mail::Mailbox {
        name: None,
        address: "ada@example.com".into(),
    });
    draft.subject = "Waiting".into();
    mail::send(&connection, &draft, &[], None, None);

    let id = mail::list_outbox(&connection).expect("list")[0].id.clone();
    mail::retry_queued(&connection, &id).expect("retry");
    assert!(mail::list_outbox(&connection).expect("list")[0].is_live());

    mail::discard_queued(&connection, &id).expect("discard");
    assert!(mail::list_outbox(&connection).expect("list").is_empty());
}

#[test]
fn discovery_answers_for_a_known_provider_without_touching_the_network() {
    // On every keystroke, which is why it must not.
    let found = mail::known_settings("someone@gmail.com").expect("gmail");
    assert_eq!(found.imap_host, "imap.gmail.com");
    assert_eq!(found.smtp_host, "smtp.gmail.com");
    assert!(mail::known_settings("someone@example.invalid").is_none());
}

#[test]
fn discovery_refuses_an_address_that_would_probe_a_private_network() {
    // "Add an account" must not become a way to scan somebody's own network.
    for address in ["evil@127.0.0.1", "evil@192.168.1.1", "evil@localhost"] {
        assert!(
            mail::discover(address).is_err(),
            "{address} was accepted for probing"
        );
        assert!(mail::known_settings(address).is_none());
    }
}

#[test]
fn a_flag_change_reports_what_it_replaced_and_restore_puts_it_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    deliver(
        root,
        &[(
            1,
            &message(
                "a@x",
                "",
                "Subject",
                "a@x",
                "Mon, 3 Feb 2025 09:00:00 +0000",
                "body",
            ),
            Flags {
                flagged: true,
                ..Flags::default()
            },
        )],
    );
    let connection = connection(root);

    let previous = mail::set_flags(&connection, &inbox(), &[1], |flags| Flags {
        seen: true,
        flagged: false,
        ..flags
    })
    .expect("set");
    assert_eq!(
        previous,
        vec![(
            1,
            Flags {
                flagged: true,
                ..Flags::default()
            }
        )]
    );

    mail::restore_flags(&connection, &inbox(), &previous).expect("restore");
    let store = MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
    let flags = store.state().expect("state").entries[&1];
    assert!(
        flags.flagged && !flags.seen,
        "the restore did not put the old flags back"
    );
    // Both the change and its reverse went through the queue, so the server
    // ends where the user did.
    assert!(!store.pending().expect("pending").is_empty());
}

#[test]
fn an_unmove_before_the_drain_is_exact() {
    // The window the user actually regrets in: the archive happened a second
    // ago and nothing has touched the network yet.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let raw = message(
        "a@x",
        "",
        "Oops",
        "a@x",
        "Mon, 3 Feb 2025 09:00:00 +0000",
        "archived by accident",
    );
    deliver(root, &[(1, &raw, Flags::default())]);
    let connection = connection(root);

    let taken = mail::move_to(&connection, &inbox(), &archive(), &[1]).expect("move");
    assert_eq!(taken.len(), 1);
    {
        let store =
            MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
        assert!(store.state().expect("state").entries.is_empty());
        assert_eq!(store.pending().expect("pending").len(), 1);
    }

    mail::unmove(&connection, &inbox(), &taken).expect("unmove");
    let store = MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
    assert_eq!(
        store.state().expect("state").entries.len(),
        1,
        "the message did not come back"
    );
    assert!(
        store.pending().expect("pending").is_empty(),
        "the cancelled move is still queued and will archive it again"
    );
    let bytes = store.raw(1).expect("read").expect("bytes");
    assert!(String::from_utf8_lossy(&bytes).contains("archived by accident"));
}

#[test]
fn an_unmove_after_the_drain_says_so_instead_of_pretending() {
    // Once the queue drained, the move happened on the server and the message
    // holds a new UID there that MOVE never told us. Pretending to undo would
    // resurrect a local copy the next sync cannot reconcile.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let raw = message(
        "a@x",
        "",
        "Gone",
        "a@x",
        "Mon, 3 Feb 2025 09:00:00 +0000",
        "left already",
    );
    deliver(root, &[(1, &raw, Flags::default())]);
    let connection = connection(root);

    let taken = mail::move_to(&connection, &inbox(), &archive(), &[1]).expect("move");
    // Simulate the drain having succeeded: the queue entry is resolved.
    {
        let mut store =
            MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
        let pushed = store.pending().expect("pending").remove(0);
        store.resolve(&pushed).expect("resolve");
    }

    let error = mail::unmove(&connection, &inbox(), &taken).expect_err("must refuse");
    assert!(error.contains("already reached the server"), "{error}");
    let store = MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
    assert!(
        store.state().expect("state").entries.is_empty(),
        "a local copy was resurrected that the next sync cannot reconcile"
    );
}

#[test]
fn the_unified_inbox_merges_accounts_newest_first_and_says_whose() {
    // Two accounts, two maildirs, one list — the reason a multi-account user
    // opens a mail client at all.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let mut first = connection(root);
    first.account_id = "acct-one".into();
    first.account.display_name = "Work".into();
    let mut second = connection(root);
    second.account_id = "acct-two".into();
    second.account.display_name = "Home".into();

    for (connection, uid, subject, date) in [
        (
            &first,
            1,
            "Older, at work",
            "Mon, 3 Feb 2025 09:00:00 +0000",
        ),
        (
            &second,
            1,
            "Newer, at home",
            "Mon, 3 Feb 2025 11:00:00 +0000",
        ),
    ] {
        let path = maildir::mailbox_path(root, &connection.account_id, &inbox());
        let mut store = MaildirStore::open(path).expect("maildir");
        store
            .upsert(&cosmic_pim_mail::store::RemoteMessage {
                uid,
                flags: Flags::default(),
                raw: message(
                    &format!("{}@x", connection.account_id),
                    "",
                    subject,
                    "a@example.com",
                    date,
                    "body",
                )
                .into_bytes(),
                internal_date_ms: 0,
            })
            .expect("deliver");
    }

    let merged = mail::unified_inbox(&[first, second]).expect("merge");
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].conversation.subject, "Newer, at home");
    assert_eq!(
        merged[0].account_name, "Home",
        "the row does not say whose it is"
    );
    assert_eq!(merged[1].account_name, "Work");
}

#[test]
fn one_broken_account_does_not_empty_the_unified_view() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let mut good = connection(root);
    good.account_id = "good".into();
    deliver_as(root, "good", 1, "Still here");

    // The broken account's index path is a directory, which cannot be opened
    // as a database.
    let mut broken = connection(root);
    broken.account_id = "broken".into();
    broken.index_path = root.to_path_buf();
    deliver_as(root, "broken", 1, "Unreadable");

    let merged = mail::unified_inbox(&[broken, good]).expect("must not fail outright");
    assert_eq!(
        merged.len(),
        1,
        "the broken account took the good one with it: {merged:?}"
    );
    assert_eq!(merged[0].conversation.subject, "Still here");
}

fn deliver_as(root: &std::path::Path, account: &str, uid: u32, subject: &str) {
    let path = maildir::mailbox_path(root, account, &inbox());
    let mut store = MaildirStore::open(path).expect("maildir");
    store
        .upsert(&cosmic_pim_mail::store::RemoteMessage {
            uid,
            flags: Flags::default(),
            raw: message(
                &format!("{account}@x"),
                "",
                subject,
                "a@example.com",
                "Mon, 3 Feb 2025 09:00:00 +0000",
                "body",
            )
            .into_bytes(),
            internal_date_ms: 0,
        })
        .expect("deliver");
}

#[test]
fn a_label_becomes_a_chip_a_queued_write_and_a_search_filter() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let connection = connection(root);

    deliver(
        root,
        &[
            (
                1,
                &message(
                    "trip@x",
                    "",
                    "Flights",
                    "Ada <ada@example.com>",
                    "Mon, 3 Feb 2025 09:00:00 +0000",
                    "Booked the flights.",
                ),
                Flags::default(),
            ),
            (
                2,
                &message(
                    "other@x",
                    "",
                    "Unrelated",
                    "Bob <bob@example.net>",
                    "Mon, 3 Feb 2025 10:00:00 +0000",
                    "Something else.",
                ),
                Flags::default(),
            ),
        ],
    );

    // Label the first conversation.
    let previous = mail::set_label(&connection, &inbox(), &[1], "Travel", true).expect("label it");
    assert_eq!(previous.len(), 1, "nothing was labelled");

    // The chip comes back with the list, named.
    let (conversations, labels) = mail::conversations(&connection, &inbox()).expect("read");
    let index = conversations
        .iter()
        .position(|c| c.subject == "Flights")
        .expect("the labelled thread");
    assert_eq!(labels[index], vec!["Travel"]);
    let other = conversations
        .iter()
        .position(|c| c.subject == "Unrelated")
        .expect("the other thread");
    assert!(labels[other].is_empty(), "the label leaked to another row");

    // The write is queued for the server, carrying the keyword bit.
    let store =
        MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("maildir");
    let pending = store.pending().expect("pending");
    assert_eq!(pending.len(), 1, "no write reached the queue");
    assert_eq!(store.keywords(), vec!["Travel"]);

    // The reader names it too.
    let opened = mail::open(&connection, &inbox(), 1).expect("open");
    assert_eq!(opened.labels, vec!["Travel"]);

    // And label: filters search to exactly the labelled conversation.
    let folders = vec![inbox()];
    let hits = mail::search(&connection, &folders, "label:travel flights", 50).expect("search");
    assert_eq!(hits.len(), 1, "label: did not narrow the search: {hits:?}");
    let none = mail::search(&connection, &folders, "label:travel unrelated", 50).expect("search");
    assert!(none.is_empty(), "an unlabelled hit passed the label filter");

    // Taking the label off through the same verb clears the chip.
    mail::set_label(&connection, &inbox(), &[1], "travel", false).expect("unlabel");
    let (_, labels) = mail::conversations(&connection, &inbox()).expect("read again");
    assert!(labels[index].is_empty(), "the label did not come off");
}

#[test]
fn plain_mail_gets_a_quiet_pgp_verdict_and_a_bad_key_never_enters_the_ring() {
    use cosmic_pim_mail::pgp::Verdict;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let connection = connection(root);

    // A message with a pgp-keys attachment that is not actually a key.
    let raw = "Message-ID: <k@x>\r\n\
From: Ada <ada@example.com>\r\n\
Subject: my key\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
\r\n\
attached\r\n\
--b\r\n\
Content-Type: application/pgp-keys\r\n\
Content-Disposition: attachment; filename=\"ada.asc\"\r\n\
\r\n\
this is not an armored key\r\n\
--b--\r\n";
    deliver(root, &[(1, raw, Flags::default())]);

    // Unsigned plain mail says nothing.
    let opened = mail::open(&connection, &inbox(), 1).expect("open");
    assert_eq!(opened.pgp.verdict, Verdict::Unsigned);
    assert!(!opened.pgp.encrypted);

    // The invalid key is refused at the door, and the ring stays empty —
    // a keyring file that will not parse is a keyring that silently stops
    // verifying.
    let error = mail::import_pgp_key(&connection, &inbox(), 1, 0).expect_err("not a key");
    assert!(error.contains("public key"), "{error}");
    assert!(
        !root
            .join(ACCOUNT)
            .join(".keys")
            .join("ada@example.com.asc")
            .exists(),
        "an unparseable key was written to the ring"
    );
}

#[test]
fn an_encrypted_message_is_said_to_be_encrypted_not_unsigned() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let raw = "Message-ID: <e@x>\r\n\
From: Ada <ada@example.com>\r\n\
Subject: sealed\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=\"EE\"\r\n\
\r\n\
--EE\r\n\
Content-Type: application/pgp-encrypted\r\n\
\r\n\
Version: 1\r\n\
--EE\r\n\
Content-Type: application/octet-stream\r\n\
\r\n\
-----BEGIN PGP MESSAGE-----\r\n\r\nAAAA\r\n-----END PGP MESSAGE-----\r\n\
--EE--\r\n";
    deliver(root, &[(1, raw, Flags::default())]);

    let opened = mail::open(&connection(root), &inbox(), 1).expect("open");
    assert!(opened.pgp.encrypted, "the sealed message read as plain");
}

#[test]
fn snoozing_never_takes_a_message_that_nothing_could_bring_back() {
    // A snooze comes back by `Message-ID`. A message without one that went
    // to the Snoozed folder anyway would sit there for good, with nothing on
    // the schedule to wake it — deferred mail silently turned into lost mail.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let later = message(
        "later@x",
        "",
        "Later",
        "Ada <ada@example.com>",
        "Mon, 3 Feb 2025 09:00:00 +0000",
        "Deal with this.",
    );
    let anonymous = "From: Bob <bob@example.net>\r\n\
                     To: me@example.com\r\n\
                     References: <later@x>\r\n\
                     Subject: Re: Later\r\n\
                     Date: Mon, 3 Feb 2025 10:00:00 +0000\r\n\
                     \r\n\
                     And this.\r\n";
    deliver(
        root,
        &[
            (1, &later, Flags::default()),
            (2, anonymous, Flags::default()),
        ],
    );

    let connection = connection(root);
    let taken = mail::snooze(&connection, &inbox(), &[1, 2], i64::MAX / 2).expect("snooze");

    let uids: Vec<u32> = taken.iter().map(|message| message.uid).collect();
    assert_eq!(uids, [1], "only the message the schedule can wake is taken");
    let store = MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
    assert!(
        store.raw(1).expect("read").is_none(),
        "the snoozed one left"
    );
    assert!(
        store.raw(2).expect("read").is_some(),
        "a message with no Message-ID was put away where nothing brings it back"
    );
    assert!(
        store
            .pending()
            .expect("pending")
            .iter()
            .all(|entry| entry.op.uid() != 2),
        "a move was queued for a message the schedule cannot wake"
    );
}

/// A rule marking Ada's mail read, which is the whole rule set.
fn marking_adas_mail_read(connection: &Connection) {
    use cosmic_pim_mail::rules::{Actions, Condition, Field, Rule};
    mail::save_rules(
        connection,
        &[Rule {
            name: "Ada".into(),
            conditions: vec![Condition {
                field: Field::Sender,
                contains: "ada@example.com".into(),
            }],
            actions: Actions {
                mark_read: true,
                ..Actions::default()
            },
            ..Rule::default()
        }],
    )
    .expect("save the rules");
}

fn from_ada(n: u32) -> String {
    message(
        &format!("ada-{n}@x"),
        "",
        &format!("Note {n}"),
        "Ada <ada@example.com>",
        "Mon, 3 Feb 2025 09:00:00 +0000",
        "Hello.",
    )
}

/// What the server's renumbering does to the local mirror: every UID void,
/// the mailbox fetched again under a new UIDVALIDITY.
fn renumber(root: &std::path::Path, uid_validity: u32, uids: &[u32]) {
    let path = maildir::mailbox_path(root, ACCOUNT, &inbox());
    let mut store = MaildirStore::open(path).expect("open the maildir");
    store.reset(uid_validity).expect("reset");
    for uid in uids {
        store
            .upsert(&RemoteMessage {
                uid: *uid,
                flags: Flags::default(),
                raw: from_ada(*uid).into_bytes(),
                internal_date_ms: i64::from(*uid) * 60_000,
            })
            .expect("refetch");
    }
}

fn is_seen(root: &std::path::Path, uid: u32) -> bool {
    let store = MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
    store.state().expect("state").entries[&uid].seen
}

#[test]
fn rules_still_meet_new_mail_after_the_server_renumbers_the_inbox_lower() {
    // A renumbered mailbox usually starts again from 1. A high-water mark
    // kept from before would sit above every new UID, and the rules would
    // quietly stop running until the new numbers caught up with the old.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let connection = connection(root);
    marking_adas_mail_read(&connection);
    let old: Vec<(u32, String)> = (1..=5).map(|uid| (uid, from_ada(uid))).collect();
    let old: Vec<(u32, &str, Flags)> = old
        .iter()
        .map(|(uid, raw)| (*uid, raw.as_str(), Flags::default()))
        .collect();
    deliver(root, &old);
    assert_eq!(
        mail::apply_rules(&connection, &[]).expect("first pass"),
        None,
        "the first pass only records where new mail starts"
    );

    renumber(root, 2, &[1, 2, 3]);
    mail::apply_rules(&connection, &[]).expect("the pass after the renumbering");
    assert!(
        !is_seen(root, 1),
        "mail that came back with the renumbering is not new"
    );

    deliver(root, &[(4, &from_ada(4), Flags::default())]);
    mail::apply_rules(&connection, &[]).expect("the next pass");
    assert!(
        is_seen(root, 4),
        "new mail after a renumbering met no rules"
    );
}

#[test]
fn a_renumbering_to_higher_uids_is_not_mistaken_for_new_mail() {
    // The other direction: if the server's new numbers start above the old
    // mark, every message it re-sent would look newly arrived, and a rule
    // written today would run over the whole inbox.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let connection = connection(root);
    marking_adas_mail_read(&connection);
    deliver(root, &[(1, &from_ada(1), Flags::default())]);
    assert_eq!(
        mail::apply_rules(&connection, &[]).expect("first pass"),
        None
    );

    renumber(root, 2, &[10, 11, 12]);
    mail::apply_rules(&connection, &[]).expect("the pass after the renumbering");
    for uid in [10, 11, 12] {
        assert!(
            !is_seen(root, uid),
            "a renumbered message was treated as new mail"
        );
    }
}

#[test]
fn drafts_saved_in_the_same_instant_are_still_separate_drafts() {
    // Quitting with several unsaved composers open saves them in one loop,
    // well inside a millisecond of each other. A fresh id was the clock, so
    // each save after the first landed on the id before it and replaced
    // that draft — half-written messages lost on the way out.
    let dir = tempfile::tempdir().expect("tempdir");
    let connection = connection(dir.path());

    let mut ids = std::collections::HashSet::new();
    for n in 0..20 {
        let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
            name: None,
            address: "me@example.com".into(),
        });
        draft.subject = format!("Draft {n}");
        ids.insert(mail::save_draft(&connection, None, &draft).expect("save"));
    }

    assert_eq!(ids.len(), 20, "two new drafts were given the same id");
    assert_eq!(
        mail::list_drafts(&connection).expect("list").len(),
        20,
        "a new draft replaced another one"
    );
}

#[test]
fn snoozing_is_refused_where_nothing_could_wake_the_mail() {
    // Waking is an IMAP session: it finds the message in the Snoozed folder
    // and moves it back. On a POP3 account there is no Snoozed folder and no
    // way to move anything, so the "snoozed" message simply left the inbox —
    // and POP3 never downloads a message twice, so it never came back.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let later = message(
        "later@x",
        "",
        "Later",
        "Ada <ada@example.com>",
        "Mon, 3 Feb 2025 09:00:00 +0000",
        "Deal with this.",
    );
    deliver(root, &[(1, &later, Flags::default())]);

    let mut connection = connection(root);
    if let Some(endpoint) = connection.account.mail.as_mut() {
        endpoint.protocol = cosmic_pim_accounts::MailProtocol::Pop3;
    }

    assert!(
        mail::snooze(&connection, &inbox(), &[1], i64::MAX / 2).is_err(),
        "a snooze nothing can wake was accepted"
    );
    let store = MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
    assert!(
        store.raw(1).expect("read").is_some(),
        "the message left the inbox"
    );
    assert!(
        store.pending().expect("pending").is_empty(),
        "a move was queued anyway"
    );
}

/// Ada's message, delivered, and a reply to it scheduled in the outbox.
fn scheduled_reply(root: &std::path::Path) -> (Connection, String) {
    deliver(root, &[(1, &from_ada(1), Flags::default())]);
    let connection = connection(root);
    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: None,
        address: "me@example.com".into(),
    });
    draft.to.push(cosmic_pim_mail::Mailbox {
        name: None,
        address: "ada@example.com".into(),
    });
    draft.subject = "Re: 1".into();
    let id =
        mail::schedule_send(&connection, None, &draft, 0, Some(&(inbox(), 1))).expect("schedule");
    (connection, id)
}

fn is_answered(root: &std::path::Path, uid: u32) -> bool {
    let store = MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
    store.state().expect("state").entries[&uid].answered
}

#[test]
fn a_reply_that_leaves_the_outbox_marks_what_it_answers() {
    // With the undo grace on — the default — every reply goes through the
    // outbox, and the outbox has nowhere to keep what a message answers. The
    // original was never marked answered, while a send with the grace off
    // marked it. The drain is the substrate's own, as a sync pass runs it.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let (connection, _) = scheduled_reply(root);

    mail::settle_answered(&connection, &[]).expect("settle while queued");
    assert!(!is_answered(root, 1), "marked before the reply went");

    let drained = mail::outbox(&connection)
        .expect("outbox")
        .drain_with(|_| Outcome::Sent(b"sent".to_vec()), i64::MAX)
        .expect("drain");
    let sent: Vec<String> = drained.sent.into_iter().map(|(id, _)| id).collect();
    assert_eq!(sent.len(), 1);

    assert_eq!(
        mail::settle_answered(&connection, &sent).expect("settle"),
        1
    );
    assert!(
        is_answered(root, 1),
        "the reply went and its original is unmarked"
    );
    let store = MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
    assert!(
        store
            .pending()
            .expect("pending")
            .iter()
            .any(|entry| entry.op.uid() == 1),
        "the mark is local only; the server never hears of it"
    );
}

#[test]
fn a_reply_taken_back_marks_nothing_and_still_knows_what_it_answers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let (connection, id) = scheduled_reply(root);

    let mail::Cancelled::TakenBack(taken) = mail::cancel_send(&connection, &id).expect("cancel")
    else {
        panic!("it had not gone");
    };
    assert_eq!(
        taken.answering.map(|(folder, uid)| (folder.wire_name, uid)),
        Some(("INBOX".to_owned(), 1)),
        "the reopened reply would no longer mark its original when sent"
    );

    mail::settle_answered(&connection, &[id]).expect("settle");
    assert!(
        !is_answered(root, 1),
        "a reply that never went marked its original"
    );
}

#[test]
fn a_discarded_reply_marks_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let (connection, id) = scheduled_reply(root);

    mail::discard_queued(&connection, &id).expect("discard");
    mail::settle_answered(&connection, &[id]).expect("settle");
    assert!(
        !is_answered(root, 1),
        "a discarded reply marked its original"
    );
}

#[test]
fn a_reply_that_went_after_a_renumbering_marks_no_stranger() {
    // The UID it answered now names another message, or none.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let (connection, _) = scheduled_reply(root);
    renumber(root, 99, &[1]);

    let drained = mail::outbox(&connection)
        .expect("outbox")
        .drain_with(|_| Outcome::Sent(b"sent".to_vec()), i64::MAX)
        .expect("drain");
    let sent: Vec<String> = drained.sent.into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        mail::settle_answered(&connection, &sent).expect("settle"),
        0
    );
    assert!(
        !is_answered(root, 1),
        "a renumbered message was marked answered"
    );
}

#[test]
fn a_reply_that_left_the_queue_without_being_sent_marks_nothing() {
    // Only the drain's list of sent ids is evidence of a send. The old
    // bookkeeping read "no longer queued" as "sent", so a queue entry that
    // vanished any other way marked the original answered anyway.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let (connection, id) = scheduled_reply(root);
    std::fs::remove_dir_all(root.join(ACCOUNT).join(".outbox")).expect("lose the queue");

    assert_eq!(mail::settle_answered(&connection, &[]).expect("settle"), 0);
    assert!(
        !is_answered(root, 1),
        "a reply nobody sent marked its original"
    );

    // And the record is still there for the send that does go.
    assert_eq!(
        mail::settle_answered(&connection, &[id]).expect("settle"),
        1
    );
}

#[test]
fn a_send_in_flight_is_reported_as_sending_not_as_gone() {
    // The outbox claims a message before it talks to the server, so an undo
    // then cannot have it — but it has not gone either, and "it already
    // went" was the wrong news.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let (connection, id) = scheduled_reply(root);

    let mut during = None;
    let mut due_during = true;
    mail::outbox(&connection)
        .expect("outbox")
        .drain_with(
            |_| {
                during = Some(mail::cancel_send(&connection, &id).expect("cancel"));
                due_during = mail::has_due_sends(&connection, i64::MAX);
                Outcome::Sent(b"sent".to_vec())
            },
            i64::MAX,
        )
        .expect("drain");
    // Nor is it due for another pass while one is sending it.
    assert!(!due_during, "a send in flight counted as waiting");

    assert!(
        matches!(during, Some(mail::Cancelled::Sending)),
        "an undo during the send was told {during:?}"
    );
    assert!(matches!(
        mail::cancel_send(&connection, &id).expect("cancel"),
        mail::Cancelled::Gone
    ));
}

#[test]
fn every_account_with_a_due_send_is_driven_and_no_other() {
    // Sends used to leave only in the selected account's sync, so a message
    // queued in any other account waited until the user looked at it again.
    // No server answers here: the send is attempted and fails to connect,
    // which the queued message records — the evidence it was attempted at
    // all. A send that fails is not a failed drain; it stays queued.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let mut idle = connection(root);
    idle.account_id = "account-idle".into();
    idle.account.id = idle.account_id.clone();
    idle.account.display_name = "Idle".into();
    let mut later = connection(root);
    later.account_id = "account-later".into();
    later.account.id = later.account_id.clone();
    later.account.display_name = "Later".into();
    let mut due = connection(root);
    due.account_id = "account-due".into();
    due.account.id = due.account_id.clone();
    due.account.display_name = "Due".into();

    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: None,
        address: "me@example.com".into(),
    });
    draft.to.push(cosmic_pim_mail::Mailbox {
        name: None,
        address: "ada@example.com".into(),
    });
    let now = 1_000_000;
    mail::schedule_send(&later, None, &draft, now + 60_000, None).expect("schedule later");
    mail::schedule_send(&due, None, &draft, now - 1, None).expect("schedule due");

    assert!(!mail::has_due_sends(&idle, now));
    assert!(!mail::has_due_sends(&later, now));
    assert!(mail::has_due_sends(&due, now));

    let drained = mail::drain_due(&[idle.clone(), later.clone(), due.clone()], now);
    assert!(drained.failures.is_empty(), "{:?}", drained.failures);
    assert_eq!(drained.sent, 0);

    let attempts = |connection: &Connection| -> Vec<(u32, bool)> {
        mail::list_outbox(connection)
            .expect("list")
            .iter()
            .map(|queued| (queued.attempts, queued.last_error.is_some()))
            .collect()
    };
    assert_eq!(
        attempts(&due),
        [(1, true)],
        "the due send was not attempted"
    );
    assert_eq!(
        attempts(&later),
        [(0, false)],
        "a send not yet due was sent"
    );
    assert!(attempts(&idle).is_empty());
}

#[test]
fn a_draft_whose_record_cannot_be_read_is_not_adopted_as_a_second_one() {
    // Opening a mirror of one of this device's drafts reads the local record.
    // When that read failed, the mirror was adopted as a new record beside
    // the unreadable one — two records for one draft, and the sweep would
    // keep both on the server.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let connection = connection(root);
    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: None,
        address: "me@example.com".into(),
    });
    draft.subject = "Mine".into();
    let id = mail::save_draft(&connection, None, &draft).expect("save");
    let records = root.join(ACCOUNT).join(".drafts");
    std::fs::write(records.join(format!("{id}.draft.json")), "not a record").expect("corrupt");

    let drafts = folder::from_list_entry("Drafts", Some('/'), &[]);
    let mirror = format!(
        "Message-ID: <{id}.draft@example.com>\r\nFrom: me@example.com\r\n\
         Subject: Mine\r\n\r\nWriting.\r\n"
    );
    MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &drafts))
        .expect("open")
        .upsert(&RemoteMessage {
            uid: 1,
            flags: Flags::default(),
            raw: mirror.into_bytes(),
            internal_date_ms: 60_000,
        })
        .expect("mirror");

    assert!(
        mail::edit_server_draft(&connection, &drafts, 1).is_err(),
        "an unreadable record was treated as no record"
    );
    let count = std::fs::read_dir(&records)
        .expect("records")
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".draft.json"))
        .count();
    assert_eq!(count, 1, "the mirror was adopted as a second record");
}

/// A submission server on this machine that accepts every message it is
/// given, session after session, for as long as the test runs.
fn accepting_smtp() -> u16 {
    use std::io::{BufRead as _, BufReader, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut out = stream.try_clone().expect("clone");
            let mut reader = BufReader::new(stream);
            let _ = write!(out, "220 smtp.test ESMTP\r\n");
            let mut in_data = false;
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                let command = line.trim_end().to_ascii_uppercase();
                line.clear();
                let reply = if in_data {
                    if command != "." {
                        continue;
                    }
                    in_data = false;
                    "250 queued"
                } else if command.starts_with("EHLO") {
                    // AUTH PLAIN, so a password can be given without TLS.
                    "250-smtp.test\r\n250 AUTH PLAIN LOGIN"
                } else if command.starts_with("AUTH") {
                    "235 ok"
                } else if command == "DATA" {
                    in_data = true;
                    "354 go ahead"
                } else if command == "QUIT" {
                    let _ = write!(out, "221 bye\r\n");
                    break;
                } else {
                    "250 ok"
                };
                let _ = write!(out, "{reply}\r\n");
            }
        }
    });
    port
}

/// A POP3 server on this machine with an empty mailbox.
fn empty_pop3() -> u16 {
    use std::io::{BufRead as _, BufReader, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut out = stream.try_clone().expect("clone");
            let mut reader = BufReader::new(stream);
            let _ = write!(out, "+OK pop3.test ready\r\n");
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                let command = line.trim_end().to_ascii_uppercase();
                line.clear();
                let reply = match command.as_str() {
                    // No CAPA: the client assumes UIDL, as for an old server.
                    "CAPA" => "-ERR no",
                    "UIDL" => "+OK\r\n.",
                    "QUIT" => {
                        let _ = write!(out, "+OK bye\r\n");
                        break;
                    }
                    _ => "+OK",
                };
                let _ = write!(out, "{reply}\r\n");
            }
        }
    });
    port
}

/// A POP3 account submitting to `smtp` and reading from `pop3` on this
/// machine. Port 1 refuses: the server is down.
fn pop3_connection(root: &std::path::Path, smtp: u16, pop3: u16) -> Connection {
    let mut connection = connection(root);
    connection.account.id = ACCOUNT.into();
    connection.credentials = cosmic_pim_mail::sasl::Credentials::Password("pw".into());
    let endpoint = connection.account.mail.as_mut().expect("mail");
    endpoint.protocol = cosmic_pim_accounts::MailProtocol::Pop3;
    endpoint.pop3_host = "127.0.0.1".into();
    endpoint.pop3_port = pop3;
    endpoint.pop3_transport = cosmic_pim_accounts::Transport::Plaintext;
    endpoint.smtp_host = "127.0.0.1".into();
    endpoint.smtp_port = smtp;
    endpoint.smtp_transport = cosmic_pim_accounts::Transport::Plaintext;
    let submission = connection.submission.as_mut().expect("submission");
    submission.endpoint.host = "127.0.0.1".into();
    submission.endpoint.port = smtp;
    submission.endpoint.security = Security::Plaintext;
    connection
}

fn local_sent() -> Folder {
    folder::from_list_entry("Sent", Some('/'), &[])
}

/// What the account's Sent maildir on this machine holds: each message's
/// flags and bytes.
fn filed_in_sent(root: &std::path::Path) -> Vec<(Flags, String)> {
    let store =
        MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &local_sent())).expect("open Sent");
    store
        .state()
        .expect("state")
        .entries
        .into_iter()
        .map(|(uid, flags)| {
            let raw = store.raw(uid).expect("read").expect("held");
            (flags, String::from_utf8(raw).expect("utf-8"))
        })
        .collect()
}

/// A reply to Ada's message 1, with a blind copy, scheduled on `connection`.
fn scheduled_pop3_reply(root: &std::path::Path, connection: &Connection) -> String {
    deliver(root, &[(1, &from_ada(1), Flags::default())]);
    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: None,
        address: "me@example.com".into(),
    });
    draft.to.push(cosmic_pim_mail::Mailbox {
        name: None,
        address: "ada@example.com".into(),
    });
    draft.bcc.push(cosmic_pim_mail::Mailbox {
        name: None,
        address: "archive@example.com".into(),
    });
    draft.subject = "Re: Note 1".into();
    mail::schedule_send(connection, None, &draft, 0, Some(&(inbox(), 1))).expect("schedule")
}

#[test]
fn a_pop3_send_from_the_outbox_is_filed_read_in_sent_on_this_machine() {
    // POP3 has no Sent folder on the server, so a drained send was kept
    // nowhere at all: gone from the outbox, in no folder.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let connection = pop3_connection(root, accepting_smtp(), 1);
    let id = scheduled_pop3_reply(root, &connection);

    let drained = mail::drain_due(std::slice::from_ref(&connection), 1_000);
    assert!(drained.failures.is_empty(), "{:?}", drained.failures);
    assert_eq!(drained.sent, 1);

    let filed = filed_in_sent(root);
    assert_eq!(filed.len(), 1, "the sent message was not filed");
    let (flags, raw) = &filed[0];
    assert!(flags.seen, "the sender's own copy arrived unread");
    assert!(
        raw.contains(&format!("Message-ID: <{id}@example.com>")),
        "the copy is not the message that went:\n{raw}"
    );
    assert!(raw.contains("archive@example.com"), "the copy lost its Bcc");
    // The drain's report is what marks the original answered, too.
    assert!(
        is_answered(root, 1),
        "the reply went and its original is unmarked"
    );
}

#[test]
fn a_pop3_pass_files_what_it_sent_even_when_the_pop3_server_is_down() {
    // The pass sends before it reaches POP3; when POP3 then fails, the pass
    // is an error and its report of what went is lost with it.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let connection = pop3_connection(root, accepting_smtp(), 1);
    scheduled_pop3_reply(root, &connection);

    assert!(mail::sync(&connection, 1).is_err(), "POP3 is down");
    assert!(
        mail::list_outbox(&connection).expect("list").is_empty(),
        "not sent"
    );
    assert_eq!(filed_in_sent(root).len(), 1, "sent, and filed nowhere");
    assert!(is_answered(root, 1), "sent, and its original is unmarked");
}

#[test]
fn a_pop3_account_lists_the_sent_folder_its_mail_is_filed_in() {
    let dir = tempfile::tempdir().expect("tempdir");
    let connection = pop3_connection(dir.path(), accepting_smtp(), empty_pop3());

    let report = mail::sync(&connection, 1).expect("sync");
    let sent = report
        .folders
        .iter()
        .find(|folder| folder.wire_name == "Sent")
        .expect("a POP3 account's Sent folder is not listed");
    assert_eq!(sent.special_use, Some(folder::SpecialUse::Sent));
}

#[test]
fn a_pop3_send_without_the_outbox_is_filed_in_sent_too() {
    // With the undo grace off a message is sent at once, not queued; its
    // copy goes to the same place.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let connection = pop3_connection(root, accepting_smtp(), 1);
    let mut draft = cosmic_pim_mail::Draft::new(cosmic_pim_mail::Mailbox {
        name: None,
        address: "me@example.com".into(),
    });
    draft.to.push(cosmic_pim_mail::Mailbox {
        name: None,
        address: "ada@example.com".into(),
    });
    draft.subject = "Now".into();

    let sent = mail::send(&connection, &draft, &[inbox()], None, None);
    assert!(matches!(sent, mail::Sent::Ok { filed: true }), "{sent:?}");
    let filed = filed_in_sent(root);
    assert_eq!(filed.len(), 1);
    assert!(filed[0].0.seen);
    assert!(filed[0].1.contains("Subject: Now"));
}

#[test]
fn a_reply_drained_on_its_own_marks_what_it_answers() {
    // The drain that sends a due reply without a sync pass reports the ids
    // that went, as the pass does, and `.answering.json` turns them into
    // the \Answered mark. The IMAP server that would get the Sent copy is
    // down; the send is still a send.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let (mut connection, _) = scheduled_reply(root);
    connection.account.id = ACCOUNT.into();
    let endpoint = connection.account.mail.as_mut().expect("mail");
    endpoint.imap_host = "127.0.0.1".into();
    endpoint.imap_port = 1;
    endpoint.imap_transport = cosmic_pim_accounts::Transport::Plaintext;
    endpoint.smtp_host = "127.0.0.1".into();
    endpoint.smtp_port = accepting_smtp();
    endpoint.smtp_transport = cosmic_pim_accounts::Transport::Plaintext;
    connection.credentials = cosmic_pim_mail::sasl::Credentials::Password("pw".into());

    let drained = mail::drain_due(std::slice::from_ref(&connection), i64::MAX);
    assert_eq!(drained.sent, 1, "{drained:?}");
    assert!(
        is_answered(root, 1),
        "the reply went and its original is unmarked"
    );
}
