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
mail server — a CalDAV URL says nothing about an IMAP host — and Envelope works
that out from the address rather than asking: a built-in table for the common
providers, Mozilla autoconfig for everyone else, and a probed guess as a last
resort.

[cosmic-pim/ARCHITECTURE.md](https://github.com/entro314-labs/cosmic-pim/blob/main/ARCHITECTURE.md)
describes how the layers fit and where new code belongs, including the section
on what the suite's invariants mean for mail. Read it before changing anything
here: verbatim storage, durable writeback, and push-before-pull apply to a
mailbox exactly as they apply to a calendar.

## Status

Envelope reads, threads, syncs, and sends. What works:

- **IMAP sync** — folder discovery with RFC 6154 special use, incremental
  fetch, CONDSTORE flag deltas, periodic full reconciliation, and durable
  writeback for flag changes, moves, and deletions.
- **Maildir on disk** — one maildir per mailbox under `$XDG_DATA_HOME/mail`.
  `mbsync`, `notmuch`, `mu`, and `mutt` read the same files. Delete the app and
  your mail is still there, in a format thirty years of tools understand.
- **Conversations** — JWZ threading with deterministic thread ids, so a reply
  finds its parent even when the parent was never downloaded. Backed by a
  rebuildable index, so opening a folder costs a query rather than a walk of
  every message in it.
- **Checks for mail on its own**, every two minutes, which also drains any
  writes made while offline.
- **A reader** that shows what a human would actually see, and says what the
  message tried to do.
- **Sending** — plain-text compose, reply, reply-all, and forward, over SMTP,
  with the sent copy filed to Sent and the message it answers marked.
- **`mailto:` links**, so Circle's "send a message" and Slate's attendee
  addresses land here.

- **Drafts**, kept on this device. Closing the composer saves; discarding is a
  separate button that says so.
- **Search** across folders, by sender, subject, and the first line, with
  `from:`, `subject:`, `is:unread`, `is:starred`, and `has:attachment`.
- **Attachments** — received ones save to Downloads, and files can be attached
  to what you send. Nothing is ever opened for you.
- **An outbox.** A send that could not reach the server waits there and goes out
  on the next check, with its attachments.
- **Keyboard shortcuts** — Gmail's single letters (`c`, `r`, `a`, `f`, `j`, `k`,
  `e`, `s`, `u`, `/`, `g i`), plus modifier combinations that work while you are
  typing. `?` shows the list, generated from the bindings themselves.

What does not work yet: **HTML composition**, and that is a decision rather than
a gap — the reader shows text, so an HTML composer would be writing in a format
the application cannot display. **Drafts do not sync**, and the reason is in the
next section.

Search does not cover message **bodies** yet — that needs a full-text index with
ranking, which is the `tantivy` port. The query language is already separate
from what executes it, so body search slots in behind the same parser.

Also not here, in the order they are likely to matter: JMAP, native Gmail and
Graph APIs, and OpenPGP and S/MIME. All three exist in the donor and are ports,
not designs.

## Sending

Two things are worth knowing about how sending behaves, because both are
deliberate and both differ from what a mail client usually does.

**Only one kind of failed send is retried.** Failures split in two: the server
definitely did not accept the message (refused connection, TLS failure, an
explicit 4xx), or it may have. The first kind goes to the outbox and is retried
on its own — that is what makes writing mail on a train work. The second kind
never is: a timeout in the middle of `DATA` is indistinguishable from a timeout
during the greeting, and the message may be in the recipient's inbox already.
Envelope says so in different words and leaves that one to you. A duplicate you
chose is a nuisance; a duplicate the client chose is a client that cannot be
trusted with a resignation letter.

**`Bcc` never reaches the recipients.** It rides the SMTP envelope and is
stripped from the message bytes — and it is kept in the copy filed to Sent, so
you can still see what you did. Both halves are asserted against the actual
bytes on the wire.

A `mailto:` link may set `to`, `cc`, `subject`, and `body`. It may **not** set
`bcc`, or `from`, or any other header: a page that can make your mail client
silently blind-copy a third party on a message you then write and send is an
attack, and "the field is visible in the composer" is not a defence.

## Keyboard

Single letters are Gmail's, because that is the vocabulary anybody who drives a
mail client from the keyboard already has; inventing a second one would be
asking people to learn something for no reason. They fire only when nothing is
expecting text — pressing `c` in the composer types a `c` — and every text field
in the application reports its focus so that rule is exact rather than a guess.

The modifier forms (`Ctrl+N`, `Ctrl+R`, `Ctrl+Shift+R`, `Ctrl+F`, `Escape`, `F5`)
work regardless, which is the reason for having both.

Bindings match on the **physical key** when the logical one does not, so `j` and
`k` still navigate on a Greek or Cyrillic layout. A mail client whose navigation
stops working when you switch layout to write an email is not keyboard-first.

The shortcuts, the menu entries, and the `?` sheet all read one registry, so the
sheet cannot be out of date — it is generated from the bindings the keyboard
handler actually matches against.

## Drafts are local

Saved drafts live on this machine, under `$XDG_DATA_HOME/mail/<account>/`, and
are not uploaded to your server's Drafts folder. The sidebar says so.

The reason is that saving to the server means `APPEND`, and without the UIDPLUS
extension the client is never told what UID the message was given — so the next
sync pulls the draft back down as a message it cannot recognise as the one it
just uploaded, and every edit leaves another copy. Doing it properly needs
UIDPLUS where it exists, a `Message-ID` match where it does not, and a
reconciliation pass for the servers that mangle both. That is worth building,
and it is not worth shipping half of: a duplicated draft is worse than a local
one.

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
just build-release
just install-user     # into ~/.local, no root needed
```

`just check-all` runs what CI does: metadata validation, formatting,
clippy, and the tests. Requires a sibling checkout of `cosmic-pim`.

## Licence

GPL-3.0-only for this application; the substrate it links is MPL-2.0. See
[cosmic-pim/LICENSING.md](https://github.com/entro314-labs/cosmic-pim/blob/main/LICENSING.md).
