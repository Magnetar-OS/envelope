# Parity

The audit ROADMAP.md (Milestone 2) asks for: Envelope measured against its
benchmarks, row by row, **have / partial / gap / rejected** — a rejection with
a reason is an answer; an unlisted feature is a hole.

- **Baseline: Geary** (GNOME 46 series) — must fully cover.
- **Ceiling: Thunderbird** (2026 releases) — the parity target.
- **Polish reference: Apple Mail** — how it should feel, not what it does;
  excluded from this audit by design.

Method: rows were built from Geary's and Thunderbird's documented feature
surfaces (checked against current sources, August 2026), cross-read with
[04-envelope.md](04-envelope.md)'s Meltemi comparison and its "not doing, and
why" list, and verified against `src/` where the documents were behind the
code. This is a first pass from documented surfaces, not yet the
menu-by-menu walk of a live install the roadmap ultimately wants; rows marked
**verify** are exactly the ones that walk must settle. Audited 2026-08-27.

One correction to the other documents, found during this audit: **server-side
drafts landed** (substrate commit "the drafts mirror", wired in
`src/mail.rs` — a local draft record stays the authority, each sweep uploads
what the server has not seen and retires the copy it replaces, via UIDPLUS
where offered and a minted Message-ID everywhere else). README.md and
04-envelope.md still describe drafts as local-only; they are behind the code,
not the other way round.

---

## Baseline: Geary

Everything Geary does, and whether Envelope does it.

### Accounts and protocols

| Feature | Status | Notes |
|---|---|---|
| IMAP + SMTP | have | Plus JMAP, Gmail API, Microsoft Graph, POP3 — Geary is IMAP-only. |
| OAuth sign-in (Gmail, Outlook) | have | Browser-based PKCE, never an embedded page; token stored in the suite account store. |
| Automatic account setup | have | Provider table → Mozilla autoconfig → probe. |
| Password storage in keyring | have | Suite secret store; accounts shared across Slate/Circle/Envelope via `accounts.toml`. |
| Multiple accounts | have | |
| Unified inbox | have | Geary does not have one; long-requested there. |

### Folder management

| Feature | Status | Notes |
|---|---|---|
| Folder tree with special-use detection | have | RFC 6154, modified-UTF-7 names. |
| Move message to folder | have | `MoveToFolder` with a type-ahead picker. |
| Archive / Trash verbs | have | Delete moves to Trash, never expunges directly. |
| Mark as junk / not junk | gap | No junk verb; a Junk special-use folder is only reachable as a plain folder. |
| Folder create / rename / delete | have | Exceeds Geary, which cannot create folders. |

### Reading

| Feature | Status | Notes |
|---|---|---|
| Conversation view | have | JWZ threading, deterministic thread ids, rebuildable index. |
| HTML message display | rejected | 04's display-security position: text extraction only, no renderer, so there is no sanitiser for a renderer to disagree with and nothing remote can load. **ROADMAP flags the reversal**: text-first stays the default, but an opt-in, per-message *sanitized* HTML view (no remote content, no scripts) is a Milestone 3 decision — a receipt or boarding pass is unreadable as extracted text. Until that lands, this is the one baseline row not covered. |
| Remote image blocking | have | Structurally: nothing loads, so the setting cannot be got wrong; the reader says when a message *wanted* to phone home. Stronger than Geary's per-sender allow. |
| Inline images (CID) | gap | Follows from the HTML decision; listed as attachments instead. Resolves with the sanitized view or stays rejected with it. |
| Hidden-text detection | have | Exceeds Geary: `display:none`, white-on-white, zero-width splices counted and reported. |
| Mark read / unread, star | have | |
| Desktop notifications for new mail | gap | Geary has them; Envelope has push (IMAP IDLE) but posts no notification. |
| Print | gap | Roadmap Milestone 3 long tail. |

### Composing

| Feature | Status | Notes |
|---|---|---|
| Compose, reply, reply-all, forward | have | Plain text, quoting on fields not bytes. |
| Rich-text (HTML) composer | rejected | The reader shows text; an HTML composer would write in a format the application cannot display. Per ROADMAP, revisit only after the opt-in sanitized HTML view exists — the argument then dissolves on its own. |
| Spell check | gap | Geary has it in the composer. |
| Drafts, server-synced | have | Just landed (see intro). Replace-not-accumulate; offline discards leave tombstones and retire the server copy on the next sweep. Proven against live Dovecot; rest of the server zoo unexercised. |
| Per-account signature | gap | One From identity per account, no signature text. |
| Attachment-missing reminder | verify | Geary warns when the text mentions an attachment none is attached; not found in Envelope's composer. |
| Undo send (send delay) | gap | Geary 3.36+ has it. Registry has the slot; Milestone 2 item. |

### Triage and search

| Feature | Status | Notes |
|---|---|---|
| Undo for archive / move / flag | have | Until the write reaches the server, then refused honestly rather than resurrecting a copy sync cannot reconcile. |
| Full-text search with operators | have | `from:`, `subject:`, `is:unread`, `is:starred`, `has:attachment`; bodies included, bm25, prefix matching. |
| Search result snippets | have | |

### Attachments

| Feature | Status | Notes |
|---|---|---|
| Save received attachments | have | To Downloads; never opened for you. |
| Attach files to send | have | Outbox carries them through offline retries. |
| Drag-and-drop attach | verify | |

### Security and privacy

| Feature | Status | Notes |
|---|---|---|
| Authentication results surfaced | have | Exceeds Geary: failures shown, passes deliberately not. |
| Tracking protection | have | Structural — see remote image row. |

### Keyboard

| Feature | Status | Notes |
|---|---|---|
| Shortcuts | have | Gmail single letters + modifier forms, physical-key fallback for non-Latin layouts, `?` sheet and `Ctrl+K` palette generated from one registry. Exceeds Geary. |

### Import/export

| Feature | Status | Notes |
|---|---|---|
| Save message as file | have | Byte-exact `.eml`. |
| mbox import | have | Uploads into the open folder, so the archive becomes server mail. Geary has neither direction. |

**Baseline verdict:** covered, except — HTML display (rejected, reversal
flagged for Milestone 3), and the small-but-real Geary rows: notifications,
spell check, signatures, junk verb, print, undo send. None of these is
data-loss-shaped; all are listed as gaps, not waved away.

---

## Ceiling: Thunderbird

Only the rows Thunderbird adds beyond the baseline table above.

### Accounts and protocols

| Feature | Status | Notes |
|---|---|---|
| POP3 | have | |
| Exchange | have | Via Microsoft Graph rather than EWS; same accounts covered by a different route. |
| Multiple identities per account | gap | One From identity, set on the Accounts page. |
| NNTP newsgroups | rejected | Not mail. Per 04's rule for Meltemi's non-mail layers: if it belongs in the suite it arrives as its own thing, not as a reason this client grows. |
| RSS feeds | rejected | Same reason. |
| Chat (IRC/XMPP/Matrix) | rejected | Same reason. |
| Calendar | rejected (as a built-in) | The suite's answer is Slate, not a calendar inside the mail client. The connective tissue — iMIP: detecting `text/calendar` + METHOD, handing to Slate, sending the REPLY — is a real **gap** until Milestone 4. |

### Folder management and triage

| Feature | Status | Notes |
|---|---|---|
| Message filters / rules | gap | Milestone 2: client-side filters first, Sieve where the server offers it. |
| Tags / labels (IMAP keywords) | gap | Milestone 2. Substrate position recorded in 04: the five system flags today; when keywords come, implement Dovecot's `dovecot-keywords` mapping, not a private scheme no other tool can read. |
| Snooze | gap | Milestone 2; registry slot exists. |
| Quick Filter bar | partial | Search with operators covers the queries; there is no live filter bar over the open list. |
| Saved searches / virtual folders | gap | Milestone 3 long tail; the search parser is the reusable half. |
| Junk classifier (Bayesian) | gap | No junk verb at all yet (see baseline); an adaptive classifier is a further, unscoped step. |

### Composing and sending

| Feature | Status | Notes |
|---|---|---|
| Per-identity signatures | gap | |
| Address autocomplete from contacts | gap | Planned via Circle (`core::model::Contact` is a library call away); Milestone 3/4. |
| Scheduled send (Send Later) | gap | Meltemi had it; the outbox is the natural home. |
| Return receipts (MDN) / DSN | gap | `dsn.rs` parsing is portable in the donor, deferred behind the send path. |

### Reading and search

| Feature | Status | Notes |
|---|---|---|
| Global search across accounts | partial | Verified in `src/mail.rs`: search is cross-folder but scoped to one account's index; it does not span accounts the way the unified inbox does. |
| Message tags in list / colors | gap | With labels, above. |

### Security

| Feature | Status | Notes |
|---|---|---|
| OpenPGP | gap | Donor module exists (`pgp_mail.rs`, 1586 lines); port, not design. First of the two, per 04. |
| S/MIME | gap | After OpenPGP (`smime.rs`, 887 lines). |
| Encrypted local store | rejected | Files-as-truth is the suite's promise; encryption at rest is the disk's job on a Linux desktop. (Thunderbird's own store is plaintext too — this rejection is really against Meltemi's SQLCipher store, recorded here so the row is not a hole.) |

### Import/export

| Feature | Status | Notes |
|---|---|---|
| mbox import (per folder) | have | The Thunderbird migration path in practice. |
| mbox export | gap | Verified: the substrate's `mbox` module only parses (`messages()` — import direction). ROADMAP's "(export exists)" refers to `.eml` export; 04's table is the accurate one. |
| Full profile / account-settings import | gap | Per-folder mbox is the unit today; no importer for accounts, filters, or address books. |

### Extensibility

| Feature | Status | Notes |
|---|---|---|
| Add-ons / extensions | rejected | Not a platform; the suite's extension points are the substrate crates and the cross-app contracts. |

---

## Data-loss-shaped gaps

The Milestone 2 exit criterion is that this section be empty against the
baseline. As of this audit, **no known gap loses data**:

- Verbatim RFC 5322 bytes on disk, maildir any tool can read; delete the app
  and the mail remains.
- Ambiguously-failed sends are never auto-retried; `Bcc` never reaches
  recipients; undo refuses rather than resurrects once the server has the
  write.
- Drafts now mirror with replace-not-accumulate semantics — the duplication
  failure mode that kept them local is the thing the design removes.

Watch items — not known losses, but the places one would appear first:

- **The server zoo.** The drafts mirror, the mass-delete guard, and CONDSTORE
  are proven against Dovecot only. Exchange, Gmail-IMAP, and the rest are
  unexercised; the quirks table has one entry.
- **POP3 semantics** — verify what Envelope does about leave-on-server and
  deletion; a wrong default there is data loss by definition.
- **Junk-verb absence** pushes users toward Delete for spam, which is a
  Trash-then-expunge away from loss they did not intend. A triage-verb gap
  with a data-loss smell; it goes with labels/rules in Milestone 2–3.

## Ceiling gaps, ranked by how much they are missed

04's ranking, updated for what has since landed (server-side drafts and mbox
import are off the list), then extended:

1. **Rules** — client-side filters, then Sieve.
2. **Labels / IMAP keywords** — with the Dovecot mapping-file approach.
3. **Snooze.**
4. **OpenPGP**, then **S/MIME** — ports, not designs.
5. **Undo send** — the delay queue is small once the outbox exists.
6. **Multiple identities and signatures** — daily-driver, not exotic.
7. **Desktop notifications** — push already works; the last inch is missing.
8. **Address autocomplete via Circle** — also the first cross-app payoff.
9. **Saved searches / virtual folders.**
10. **Spell check.**
11. **Scheduled send.**
12. **Junk handling** — verb first, classifier later if at all.
13. **iMIP handoff to Slate** — Milestone 4; the suite's answer to
    Thunderbird's built-in calendar.
14. **Print.**
15. **The sanitized HTML view** — listed last not because it is minor but
    because it is a *decision* scheduled for Milestone 3, not a backlog item;
    once taken, it also reopens rich-text compose on 04's own terms.
