# Envelope — Mail

## As built

Reads the shared accounts.toml — deliberately the only working part. The
engine exists in Meltemi (~25k lines across IMAP sync, JMAP with RFC 8887
push, Graph, Gmail API, tantivy search, query parsing, PGP, S/MIME, DSN,
JWZ threading, SPF/DKIM/DMARC, html5ever text extraction), running in
production. The stated porting order — substrate mail crate → threading →
IMAP behind a store trait → UI — is dependency-correct; the CalDAV-port
precedent (pure reconciler, four-method store trait, in-memory test impl)
is the template. Nothing below changes that order; it fills in decisions
around it.

## Gates before porting

1. **Meltemi licence declaration** (00). Nothing more moves until it exists.
2. Transport settled: reqwest blocking, never ureq 3 (hardcoded method
   allowlist — already bit CalDAV, would bite JMAP).

## Substrate decisions (with 01)

- **Maildir as the message store.** Files-as-truth extends to mail:
  interoperable with mbsync/notmuch/mu, index rebuildable, data walkable.
  Tantivy is the disposable index, same standing as the calendar's SQLite
  cache. Message bytes verbatim (RFC 5322 untouched); `mail-parser` extracts;
  nothing re-serialises a message.
- **Invariant translation** written into ARCHITECTURE.md (00/01):
  - Durable writeback → flag changes, moves, deletes queued in a sidecar with
    the retryable/non-retryable split. IMAP's 412-analogs: UIDVALIDITY change
    (full re-reconcile, never retry blindly), \Noselect, auth failures.
  - Push before pull → identical reasoning; the presenting symptom is "my
    read marks keep reverting."
  - Flavor does **not** apply — IMAP is not a WebDAV flavor; `cosmic-pim-mail`
    stands beside `caldav`, sharing `accounts` and `core`, not the DAV engine.

## Port order, annotated

1. `cosmic-pim-mail`: message model, maildir store behind a trait, parser
   boundary, folder tree (`folder_tree.rs`).
2. Zero-`crate::`-reference modules, essentially as-is: `threading.rs`
   (conversations, deterministic thread ids), `auth_results.rs`,
   `dsn.rs`, `model_text.rs` (the html5ever DOM path — this is also the
   HTML-display sanitizer, so it's UI-critical, not just parsing).
3. IMAP (`mail_sync.rs`) against the store trait; live-test harness in the
   `live_sync.rs` style — a canned-response IMAP server driving the real
   client into a real maildir, plus containerized Dovecot in CI (as cheap as
   Radicale).
4. Search: `tantivy_search.rs` + `search_query.rs` as the rebuildable index.
5. UI: folder tree, threaded list, reader (sanitized HTML with remote-content
   blocking default-on), composer, account status. Thin over the substrate,
   per the suite pattern.

Defer, explicitly and in this order of likelihood-to-matter: JMAP (second
protocol, Fastmail users, WebSocket push already written), Gmail/Graph native
APIs (OAuth verification + quota burden — CalDAV-equivalent reasoning says
IMAP covers Gmail initially), PGP/S-MIME (ported late; key management UX is
its own project).

## Integration contracts (what the suite buys)

- **Sender → person.** Resolve addresses through `core::model::Contact` — a
  library call, no daemon (00). Displayed name, avatar, and "add to
  contacts" for unknowns land in the reader for free once Circle's write
  path exists.
- **Last-contact hook.** On send/receive involving a linked address, record
  the interaction where Circle's CRM layer reads it (03). Library-level.
- **iMIP handoff to Slate.** The one cross-process contract in the suite:
  detect text/calendar + METHOD on delivery, hand payload over D-Bus
  (deliver-payload); accept a send-REPLY request back. Kept minimal so Slate
  degrades to .ics import without Envelope, and Envelope ships mail long
  before Slate consumes invitations. iTIP semantics themselves live in the
  substrate scheduling module (01) so both apps interpret one implementation.
- **mailto: registration** — Envelope becomes the target for Circle's compose
  actions and Slate's attendee links.
- **envelope-launcher** later: search over the tantivy index, mirroring
  slate-launcher's shape.

## Display security defaults

Remote content blocked by default (tracking pixels), per-sender allow;
auth_results surfaced as a quiet indicator (fail states prominent, pass
states not); attachments never auto-open; text extraction only through
`model_text` — no ad-hoc HTML stripping anywhere else.

## Risks

- `mail_sync.rs` is 6k lines with, presumably, storage assumptions the trait
  must absorb — the CalDAV precedent says budget the boundary-drawing, not
  the protocol code.
- IMAP server zoo (Exchange IMAP, Gmail's IMAP quirks, UIDPLUS absence)
  is the DAV quirks problem again, larger; same table discipline.
- Composer scope creep is the classic mail-client death; ship plain-text +
  reply/forward with quoting before HTML composition is even discussed.
