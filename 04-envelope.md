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

## Parity with Meltemi

The honest measurement, because "ported from Meltemi" invites the wrong
expectation. Meltemi is ~81 600 lines of Rust plus 347 TypeScript files.
`cosmic-pim-mail` plus Envelope is ~12 400 lines including tests.

That ratio is not a shortfall to be closed. Most of what Meltemi is, Envelope is
deliberately not: a five-engine client with an AI layer, an energy-pacing
system, and eight inbox view modes is a different product. What follows
separates "not done yet" from "not doing".

### The core client

| | Meltemi | Envelope |
|---|---|---|
| Engines | IMAP, JMAP, Gmail, Graph, POP3 | the same five, via cosmic-pim-sync |
| IMAP sync | CONDSTORE + QRESYNC, poison-message skip-list, snooze keywords | CONDSTORE + QRESYNC + IDLE, no skip-list |
| Store | SQLCipher-encrypted SQLite | maildir + rebuildable index |
| Threading | JWZ, server thread ids where offered | JWZ |
| Folder tree | full, drag-reorder | full, read-only |
| Read | sandboxed iframe + DOMPurify + CSP | text only, no renderer |
| Auth results | badges + explainer | failures shown, passes not |
| Compose | TipTap rich text, 3 window modes | plain text, one pane |
| Attachments | send, receive, drag-drop, inline CID | send, receive, save |
| Drafts | server-synced | local |
| Import/export | mbox/eml both ways | mbox import, .eml export |
| Send | + outbox, undo send, scheduled | typed pre-acceptance errors, outbox |
| Search | FTS5 + tantivy + semantic, saved queries | FTS5 with bm25, over the index |
| Keyboard | registry, palette, rebinding, quick steps | registry, Gmail keys, chords, cheat sheet, palette |
| Accounts | six-stage discovery, OAuth PKCE | registry + three-stage discovery, OAuth PKCE |

### Not done yet

In rough order of how much they are missed: **server-side drafts**, **rules**,
**labels**, **snooze**, and **OpenPGP and S/MIME**.

### Not doing, and why

- **Rich-text compose.** The reader shows text; an HTML composer would write in
  a format this application cannot display.
- **An HTML renderer.** Text-only is the stronger display-security position, not
  a lesser one — there is no sanitiser for a renderer to disagree with, and no
  remote content can load because nothing loads.
- **An encrypted store.** Files-as-truth is the suite's promise. Encryption at
  rest is the disk's job on a Linux desktop.
- **The AI layer, the energy system, message typing, and the alternative inbox
  views.** These are Meltemi's product, not the mail client underneath it. If
  any of them belong in the suite they arrive as their own thing, not as a
  reason this crate grows an inference budget.

### The shape of the difference

Envelope has roughly a fifth of Meltemi's feature surface and most of what makes
a mail client usable daily. What it is missing that genuinely bites: **one
protocol and one auth method** — IMAP with a password, so Gmail and Outlook need
an app password rather than OAuth. After that it is **undo** and **snooze**:
triage verbs that the registry now has somewhere to live, but that nothing
implements.

## Gates

1. **Meltemi licence declaration** (00). **Resolved.** Meltemi's
   `LICENSING.md` grants the ported layers — `mail_sync.rs`, `threading.rs`,
   `model_text.rs`, `auth_results.rs`, alongside the CalDAV and secrets
   modules — under MPL-2.0, Exhibit A only, with SPDX headers on the donor
   files. Future ports extend the grant table in the same commit that ports
   them.
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
4. Search: **done**, bodies included — FTS5 with bm25 in the index, which was
   the donor's own first tier. Neither `search_query.rs` nor
   `tantivy_search.rs` needed porting; tantivy is now for the day a mailbox
   outgrows bm25 over FTS5.
5. UI: **done**, including composition, drafts, and search.

## Sizes in the donor, corrected

The figures in the older version of this file and in the README were stale.

| Module | Lines | `crate::` refs | State |
|---|---:|---:|---|
| `mail_sync.rs` | 7897 | 167 | cycle ported; app layer stays |
| `jmap.rs` | 3685 | — | deferred |
| `graph.rs` | 3117 | — | deferred |
| `gmail.rs` | 2148 | — | deferred |
| `tantivy_search.rs` | 2008 | 6 | next — body ranking only |
| `folder_tree.rs` | 1774 | 30 | not portable as-is |
| `search_query.rs` | 1640 | 2 | superseded by `mail::search` |
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

## Drafts and search, as taken

- **Drafts are a `Draft` record, not RFC 5322, and local.** Both follow from a
  draft being unfinished: `lettre` will not build a message with no destination
  (correctly — such a thing cannot be sent), and no message format can represent
  an address somebody stopped halfway through typing. Local because APPEND
  without UIDPLUS means the next sync cannot recognise the draft it just
  uploaded, so every edit leaves another copy.
- **Search is headers plus the list snippet**, over the index, with a pure
  parser sitting in front of it. Bodies need the tantivy port; the split means
  that lands behind the same `Query`.
- **Results are messages, not conversations.** Grouping a hit back into its
  thread buries it among its siblings.

## Next, in order

1. **Server-side drafts.** UIDPLUS where it exists, `Message-ID` matching where
   it does not, and a reconciliation pass for servers that mangle both. Worth
   building; not worth shipping half of.
5. **A server quirks table**, shared in shape with the CalDAV one (01) — the
   IMAP zoo is the same problem, larger.

## Integration contracts, still outstanding

- **Sender → person.** `core::model::Contact` is a library call away and the
  reader already has the address; this is small and blocked only on Circle's
  read path being exposed the way the sender lookup wants.
- **Last-contact hook** for Circle's CRM layer (03).
- **iMIP handoff to Slate** — **done on Envelope's side**, to the contract in
  cosmic-pim's ARCHITECTURE.md: the reader offers "Open in calendar" on a
  `text/calendar` part with a METHOD and hands the verbatim bytes to Slate's
  `Scheduling1` (no `StartServiceByName`; absence degrades to save-the-file),
  and Envelope exports `SendSchedulingReply` on its own name, queueing the
  REPLY into the durable outbox. Waits only on Slate exporting its half.
- **`mailto:` registration**, so Circle's compose actions and Slate's attendee
  links have a target. Needs the composer.
- **envelope-launcher**, mirroring slate-launcher, over the tantivy index.

## Risks

- **Server zoo.** Now being exercised: `tests/live_dovecot.rs` drives the real
  client at a stock Dovecot container (gated on `PIM_TEST_IMAP`, silently
  skipped without it). Its first run found two real bugs — CONDSTORE was
  advertised-but-never-enabled, so the delta path had been dead against real
  servers; and a mailbox moved to empty could never empty locally, because the
  mass-delete guard cannot tell a genuine 1→0 from a hiccup (fixed by letting
  the drain's own departures leave the store on the server's OK). The quirks
  table has its first entry: Dovecot 2.4 omits HIGHESTMODSEQ from the SELECT
  of a mailbox APPEND just auto-created, against RFC 7162; the client degrades
  one cycle and self-heals. Exchange, Gmail-IMAP, and the rest of the zoo are
  still unexercised.
- **Composer scope creep** — unchanged, and now the immediate risk rather than a
  future one.
- **Threading cost: resolved.** `mail::index` holds what a list shows and parses
  only messages it has not seen, so a folder that has not changed costs a query.
  It is a cache in the same sense the calendar's is — delete it and it rebuilds,
  and a test asserts exactly that.
- **Send has no outbox.** A failed send keeps the composer open, which is
  correct but manual. See item 4 above; the classification that makes an
  automatic retry safe already exists, the queue does not.
