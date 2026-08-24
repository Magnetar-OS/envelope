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
use cosmic_pim_mail::smtp::SmtpEndpoint;
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

    let conversations = mail::conversations(&connection(root), &inbox()).expect("read the mailbox");

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
    let conversations =
        mail::conversations(&connection(dir.path()), &archive()).expect("no maildir yet");
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
        store.pending().len(),
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
    assert!(store.pending().is_empty(), "an unchanged flag was queued");
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
    let conversation = mail::conversations(&connection, &inbox()).expect("read")[0].clone();
    assert_eq!(conversation.uids.len(), 2);

    mail::move_to(&connection, &inbox(), &archive(), &conversation.uids).expect("move");

    let store = MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("open");
    assert!(
        store.state().expect("state").entries.is_empty(),
        "the archived conversation is still in the inbox"
    );
    assert_eq!(
        store.pending().len(),
        2,
        "the move will never reach the server"
    );

    // And it survives a restart, because an archive made on a train has to.
    let reopened =
        MaildirStore::open(maildir::mailbox_path(root, ACCOUNT, &inbox())).expect("reopen");
    assert_eq!(reopened.pending().len(), 2);
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

    let conversations = mail::conversations(&connection(root), &inbox()).expect("read");
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
    let before = mail::conversations(&connection, &inbox()).expect("read");
    assert_eq!(before.len(), 1);

    std::fs::remove_file(&connection.index_path).expect("delete the cache");
    let after = mail::conversations(&connection, &inbox()).expect("read again");
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
