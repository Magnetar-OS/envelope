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

**Accounts are shared.** Envelope reads
`$XDG_CONFIG_HOME/cosmic-pim/accounts.toml`, so an account added in Slate shows
up here with its password already stored. The one thing it will not have is a
mail server — a CalDAV URL says nothing about an IMAP host — so that is the
single field Envelope's Accounts page asks for.

[cosmic-pim/ARCHITECTURE.md](https://github.com/entro314-labs/cosmic-pim/blob/main/ARCHITECTURE.md)
describes how the layers fit and where new code belongs, including the section
on what the suite's invariants mean for mail. Read it before changing anything
here: verbatim storage, durable writeback, and push-before-pull apply to a
mailbox exactly as they apply to a calendar.

## Status

Envelope reads, threads, and syncs a real mailbox. What works:

- **IMAP sync** — folder discovery with RFC 6154 special use, incremental
  fetch, CONDSTORE flag deltas, periodic full reconciliation, and durable
  writeback for flag changes, moves, and deletions.
- **Maildir on disk** — one maildir per mailbox under `$XDG_DATA_HOME/mail`.
  `mbsync`, `notmuch`, `mu`, and `mutt` read the same files. Delete the app and
  your mail is still there, in a format thirty years of tools understand.
- **Conversations** — JWZ threading with deterministic thread ids, so a reply
  finds its parent even when the parent was never downloaded.
- **A reader** that shows what a human would actually see, and says what the
  message tried to do.

What does not work yet: **composing**. There is no SMTP, no composer, and no
drafts. That is the next thing and it is deliberately last — a mail client that
reads well and cannot send is useful; the reverse is not, and composer scope
creep is the classic way a mail client never ships.

Also not here, in the order they are likely to matter: JMAP, native Gmail and
Graph APIs, OpenPGP and S/MIME, and ranked search over a tantivy index. All four
exist in the donor and are ports, not designs.

## Display security

Envelope renders **text**, not HTML. That is the position, not a limitation
waiting to be lifted:

- **Remote content cannot load.** A tracking pixel has nothing to fire from,
  so "block remote images" is not a setting that can be got wrong. The reader
  says when a message *wanted* to phone home.
- **There is no parser differential.** Every sanitising-renderer bug in history
  is a renderer disagreeing with a sanitiser about what some bytes mean. There
  is one html5ever tree here and nothing downstream re-parses it.
- **Hidden text is counted and reported.** `display:none`, white-on-white,
  1px fonts, zero-width splices, and prose smuggled into HTML comments are all
  dropped from what you read — and the reader tells you how much was dropped.
- **Authentication failures are shown; passes are not.** A green tick on the
  99% of mail that authenticates correctly trains people to ignore the
  indicator, which is exactly how it stops working on the message where it
  mattered.
- **Attachments are listed, never opened.**

## Where the engine came from

The substrate's `cosmic-pim-mail` crate is a port from
[Meltemi](https://github.com/entro314-labs/meltemi), rearranged rather than
copied: the donor's protocol code issued SQL inline, and here everything
storage-shaped sits behind a `MailStore` trait with an in-memory implementation
for tests — the same three moves that made the CalDAV port cheap.

Ported essentially as-is (they had no dependency on the donor's storage):
threading, `Authentication-Results` parsing, and the HTML text extractor.

Still in the donor, still to port: `jmap.rs`, `graph.rs`, `gmail.rs`,
`tantivy_search.rs`, `search_query.rs`, `pgp_mail.rs`, `smime.rs`, `dsn.rs`.

**Licence.** The Meltemi repository carries no licence declaration. It has a
single author, so this is a one-commit fix, but until it exists neither
`cosmic-pim-mail` nor `cosmic-pim-caldav` can be published — see `LICENSING.md`
in `cosmic-pim`.

## Building

```sh
cargo build --release
./target/release/envelope
```

Requires a sibling checkout of `cosmic-pim`.

## Licence

GPL-3.0-only for this application; the substrate it links is MPL-2.0. See
[cosmic-pim/LICENSING.md](https://github.com/entro314-labs/cosmic-pim/blob/main/LICENSING.md).
