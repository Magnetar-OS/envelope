# Envelope — Mail

## As built

Reads, threads, syncs, and sends. The substrate crate the earlier version of
this document was waiting on now exists: `cosmic-pim-mail`, standing beside
`cosmic-pim-caldav` rather than on it, with the message model over verbatim RFC
5322 bytes, a maildir store, JWZ threading, HTML-to-visible-text extraction, RFC
8601 parsing, IMAP with durable writeback, SMTP, and a rebuildable conversation
index. ~135 tests, including two scripted-server end-to-end suites.

Envelope itself is the front end: sidebar, conversation list, reader, composer,
`mailto:` handling, a two-minute poll, and an Accounts page whose only job is to
attach a mail server to an account the suite already has.

The invariants are pinned where they can be pinned. Three of them can only be
asserted about the wire and are: the fetch uses `BODY.PEEK[]` and the STORE goes
out before the FETCH (`tests/live_sync.rs`), and `Bcc` reaches the envelope but
never the message (`tests/live_send.rs`).

## Gates

1. **Meltemi licence declaration** (00). Still outstanding, now *verified* to be
   a one-commit fix: `git log` on the donor shows a single author, so no outside
   agreement is needed. Until it exists, `cosmic-pim-mail` and
   `cosmic-pim-caldav` are `publish = false`.
2. Transport: settled. `reqwest` blocking for anything HTTP-shaped (JMAP, Graph,
   Gmail), never `ureq` 3. IMAP is the `imap` crate on `native-tls` — a separate
   axis, and the ureq rule does not bear on it.

## Substrate decisions, as taken

- **Maildir**, one per mailbox under `$XDG_DATA_HOME/mail/<account-id>/`. UID
  and flags in the filename (`,U=42:2,S`) as `offlineimap` and `mbsync` spell
  them, so the index rebuilds by walking `cur/`. UIDVALIDITY, the UID cursor,
  MODSEQ, and the writeback queue live in an `.imap-state.json` sidecar — the
  same split, for the same reason, as `.caldav-state.json` beside a vdir.
- **`mail-parser` extracts; nothing re-serialises.** The DKIM argument turned
  out to be the decisive one: re-folding a single header invalidates the only
  cryptographic evidence a message carries.
- **`MailStore`**, seven methods, mirroring `CalDavStore`. `set_flags` is
  separate from `upsert` because a flag change must not need the bytes — that
  is the whole point of CONDSTORE. `reset` is separate from N × `remove`
  because a crash midway through the latter is unrecoverable: nothing on disk
  would say which numbering each surviving message belonged to.
- **`core::atomic::write_bytes`.** The existing writer took `&str`; message
  bytes are not UTF-8 and routing them through one would corrupt 8-bit MIME
  and break signatures.
- **`accounts::MailEndpoint`.** A CalDAV URL says nothing about an IMAP host
  and there was nowhere else to put one without breaking "one account, one
  password, one file". `Transport` is spelled in `accounts` as well as in
  `mail` on purpose: `accounts` sits below every protocol crate and must not
  depend on one.
- **Custom keywords: not stored.** Dovecot's extension needs a
  `dovecot-keywords` mapping file that another program rewrites. The five
  system flags are what the UI exposes; when keywords are wanted, that mapping
  is the thing to implement, not a private scheme no other tool can read.

## Port order, as it actually went

1. `cosmic-pim-mail`: model, `MailStore`, maildir, `plan`, `push`. **Done.**
2. The pure modules. **Done** for threading, `auth_results`, and `model_text`.
   Two corrections to the earlier plan:
   - the "zero `crate::` references" claim held only for `auth_results.rs` and
     `model_text.rs`; `threading.rs` had one and `search_query.rs` two.
   - `folder_tree.rs` was listed here and does **not** belong: it has 30
     `crate::` references and is coupled to the donor's storage. What Envelope
     needed from it — the modified-UTF-7 codecs and special-use detection — came
     out of `mail_sync.rs` instead and is ~200 lines in `mail::folder`.
3. IMAP. **Done.** `mail_sync.rs` is 7897 lines, not the 5983 this document
   used to say; almost all of the excess is the donor's application layer
   (scheduled sends, snooze, campaigns, receipts, SMTP), none of which is
   substrate. The cycle itself — cursor, windowed discovery, CONDSTORE deltas,
   held-back MODSEQ, periodic reconcile — is perhaps 400 lines and ported
   cleanly.
4. Search: **not done.** `tantivy_search.rs` + `search_query.rs`. The
   structured half of what search needs now exists — `mail::index` holds
   senders, subjects, dates, and thread ids — so the port is the full-text
   ranking, not the plumbing.
5. UI: **done**, including composition.

## Sizes in the donor, corrected

The figures in the older version of this file and in the README were stale.

| Module | Lines | `crate::` refs | State |
|---|---:|---:|---|
| `mail_sync.rs` | 7897 | 167 | cycle ported; app layer stays |
| `jmap.rs` | 3685 | — | deferred |
| `graph.rs` | 3117 | — | deferred |
| `gmail.rs` | 2148 | — | deferred |
| `tantivy_search.rs` | 2008 | 6 | next |
| `folder_tree.rs` | 1774 | 30 | not portable as-is |
| `search_query.rs` | 1640 | 2 | next |
| `pgp_mail.rs` | 1586 | — | deferred |
| `dsn.rs` | 896 | 5 | deferred, see below |
| `smime.rs` | 887 | — | deferred |
| `threading.rs` | 616 | 1 | **ported** |
| `model_text.rs` | 535 | 0 | **ported** |
| `auth_results.rs` | 515 | 0 | **ported** |

`dsn.rs` was listed as a step-2 freebie and is not one. Its *parsing* is
portable; its ingestion is built on `campaigns`, `verify`, and rusqlite. It is
also of little use without a send path, so it belongs after SMTP, not before.

## Send path, as taken

- **`Draft` is a structure, not bytes.** `build()` produces RFC 5322 once, at
  send time. Quoting, recipient edits, and validation operate on fields — the
  same reasoning that keeps a stored message from being re-serialised, arrived
  at from the other direction.
- **Plain text only**, and this is a decision rather than a stage. The reader
  shows text, so an HTML composer would be writing in a format the application
  cannot display. When HTML composition arrives it is a second body on the same
  draft, with the text part still generated.
- **The send outcome is a three-way type, not a `Result`.** Pre-acceptance
  failures (refused connection, TLS, an explicit 4xx) are safe to retry;
  anything from a mid-`DATA` timeout onwards may have been delivered and is
  terminal. Timeouts count as ambiguous even though they usually are not: the
  two directions of being wrong are not symmetric.
- **`Bcc` splits.** The wire copy has no `Bcc` header, the Sent copy does. Built
  twice rather than built once and edited, because editing RFC 5322 bytes is the
  thing this crate does not do.
- **`mailto:` honours `to`, `cc`, `subject`, `body` and nothing else.** RFC 6068
  permits arbitrary headers and its security consideration is real: a link that
  can set `bcc` on a message the user then writes and sends is an attack, and
  "the field is visible" is not a defence.

## Next, in order

1. **Drafts.** The composer is discarded on cancel, because a Drafts folder is
   the right answer and does not exist. It needs `APPEND` to Drafts with the
   `\Draft` flag — the mechanism is already there (`Session::append`) — plus
   the UI for reopening one.
2. **Search.** `tantivy_search.rs` + `search_query.rs`. The structured half is
   done; this is the ranking.
3. **IDLE**, so the mailbox updates without waiting up to two minutes.
4. **An outbox.** A `Sent::Failed` currently keeps the composer open and leaves
   the retry to the user. A queued outbox with the same durability the flag
   queue has is the right shape — but only for the retryable class, and the
   distinction is already typed.
5. **A server quirks table**, shared in shape with the CalDAV one (01) — the
   IMAP zoo is the same problem, larger.

## Integration contracts, still outstanding

- **Sender → person.** `core::model::Contact` is a library call away and the
  reader already has the address; this is small and blocked only on Circle's
  read path being exposed the way the sender lookup wants.
- **Last-contact hook** for Circle's CRM layer (03).
- **iMIP handoff to Slate** — the one cross-process contract in the suite.
  Needs the scheduling module in 01 and a send path here, so it stays at suite
  step 7.
- **`mailto:` registration**, so Circle's compose actions and Slate's attendee
  links have a target. Needs the composer.
- **envelope-launcher**, mirroring slate-launcher, over the tantivy index.

## Risks

- **Server zoo.** Anticipated and unchanged: Exchange's IMAP, Gmail's quirks,
  servers without UIDPLUS (handled — the message is marked `\Deleted` and left
  rather than expunging another client's pending deletions), servers without
  CONDSTORE (handled — full flag reconciliation instead). What has not been
  exercised is any of it against a real server; the scripted suite proves the
  client is coherent, not that it is compatible. Containerised Dovecot in CI is
  the cheap next step, as Radicale was for CalDAV.
- **Composer scope creep** — unchanged, and now the immediate risk rather than a
  future one.
- **Threading cost: resolved.** `mail::index` holds what a list shows and parses
  only messages it has not seen, so a folder that has not changed costs a query.
  It is a cache in the same sense the calendar's is — delete it and it rebuilds,
  and a test asserts exactly that.
- **Send has no outbox.** A failed send keeps the composer open, which is
  correct but manual. See item 4 above; the classification that makes an
  automatic retry safe already exists, the queue does not.
