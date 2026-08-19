# Envelope

Mail for the [COSMIC desktop](https://github.com/pop-os/cosmic-epoch).

## Part of a suite

Envelope is one of three applications over a shared substrate,
[cosmic-pim](https://github.com/entro314-labs/cosmic-pim):

| App | Repository | What it is |
|---|---|---|
| **Slate** | [slate](https://github.com/entro314-labs/slate) | Calendar and tasks |
| **Circle** | [circle](https://github.com/entro314-labs/circle) | Contacts |
| **Envelope** | you are here | Mail |

**Accounts are already shared.** Envelope reads
`$XDG_CONFIG_HOME/cosmic-pim/accounts.toml`, so an account added in Slate shows
up here without being re-entered. That much works today — it is the only part
that does.

[cosmic-pim/ARCHITECTURE.md](https://github.com/entro314-labs/cosmic-pim/blob/main/ARCHITECTURE.md)
describes how the layers fit and where new code belongs. Read it before porting
anything: the invariants there (verbatim storage, durable writeback, push before
pull) apply to mail as much as to calendars.

## Status: scaffold

This is deliberately further behind Slate and Circle, and the reason is worth
being precise about.

Slate and Circle were fast because their engines already existed: the vdir
store, the iCalendar/vCard layer, and the CalDAV/CardDAV client all live in
`cosmic-pim`, so each app was a front end over a tested substrate. **There is no
mail engine in `cosmic-pim` yet.** Envelope cannot be a thin front end until
there is something for it to be thin over.

## What exists to port

The engine is in [Meltemi](https://github.com/entro314-labs/meltemi), running in
production, at roughly these sizes:

| Module | Lines | What it is |
|---|---:|---|
| `mail_sync.rs` | 5983 | IMAP sync engine |
| `jmap.rs` | 3685 | JMAP, including WebSocket push (RFC 8887) |
| `graph.rs` | 3117 | Microsoft Graph |
| `gmail.rs` | 2148 | Gmail API |
| `tantivy_search.rs` | 1925 | Ranked search over a candidate set |
| `search_query.rs` | 1640 | Query parsing |
| `pgp_mail.rs` | 1586 | OpenPGP |
| `folder_tree.rs` | 1108 | Folder hierarchy |
| `smime.rs` | 887 | S/MIME (RFC 8551) |
| `dsn.rs` | 896 | Bounce parsing |
| `threading.rs` | 676 | JWZ threading, deterministic thread ids |
| `auth_results.rs` | 515 | SPF/DKIM/DMARC |
| `model_text.rs` | 506 | Safe text extraction via a real html5ever DOM |

`threading.rs`, `auth_results.rs`, `dsn.rs`, and `model_text.rs` have **zero**
`crate::` references and port essentially as-is — the same property that made
the CalDAV port cheap.

## Suggested order

1. **`cosmic-pim-mail`** in the substrate repo: the message model, a maildir or
   SQLite store, and the parser boundary (`mail-parser`).
2. **`threading.rs`** — pure, tested, and the thing that makes a mailbox read as
   conversations rather than a list.
3. **IMAP** via `mail_sync.rs`, behind a store trait, mirroring how
   `cosmic-pim-caldav` was done.
4. **Envelope's UI** — only once 1–3 exist.

The precedent to copy is `cosmic-pim-caldav`. Its protocol layer ported almost
verbatim because it had no dependency on its donor's storage; the reconciler was
already a pure function over `(listing, href→etag)`; and everything
storage-shaped went behind a four-method trait with an in-memory implementation
for tests. Mail is bigger but the same shape, and the same three moves apply.

Two things that will be reused rather than rewritten: `cosmic-pim-accounts`
(credentials, already working here) and — once the substrate has a mail
model — `cosmic-pim-core::model::Contact`, so that a sender resolves to a real
person from the same address book Circle shows.

## Two things settled before starting

- **Transport.** `ureq` 3 cannot be used: it enforces a hardcoded HTTP-method
  allowlist. That bit the CalDAV port and would bite JMAP too. `reqwest` with
  the blocking client is what the substrate uses.
- **Licence.** The Meltemi repository carries no licence declaration. That has
  to be fixed before more of it is lifted — see `LICENSING.md` in `cosmic-pim`.

## Building

```sh
cargo build --release
./target/release/envelope
```

Requires a sibling checkout of `cosmic-pim`.

## Licence

GPL-3.0-only for this application; the substrate it links is MPL-2.0. See
[cosmic-pim/LICENSING.md](https://github.com/entro314-labs/cosmic-pim/blob/main/LICENSING.md).
