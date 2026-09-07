# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

- Removing an account or deleting a filter rule asks first. Both went
  through on a single misclick, with nothing to undo them — while deleting a
  folder had asked all along.
- Single-letter shortcuts keep working. Using the command palette once left
  every one of them dead for the rest of the session, with nothing on screen
  to say why.
- A held modifier no longer fires a bare shortcut: Ctrl+C was reaching the
  composer, Ctrl+V the folder picker, and Ctrl+S the star.
- The folder list scrolls. An account with more folders than fit the window
  had the rest simply unreachable.
- Message text can be selected and copied.
- List rows keep a fixed height: a long subject or a crowd of recipients is
  clipped with an ellipsis instead of wrapping onto a second line and making
  the list jump as it scrolls.

### Added

- OpenPGP, the read half: signed mail is verified against the stored bytes —
  the verbatim original, the only thing a signature can be checked against —
  with honest states: a broken signature is loud, a signer this device holds
  no key for is a quiet note, a good signature bound to a different address
  says exactly that, and a verified one says nothing at all. Encrypted mail
  is declared as encrypted (decryption comes later). Keys arrive the way
  they are actually sent: an `application/pgp-keys` attachment grows an
  Import key button, binding the key to the sender's address in a
  plain-files keyring (`.keys/<address>.asc`) beside the account's mail.

- Labels: `l` opens a picker that applies, clears, filters, and creates
  labels on a conversation; chips show in the list and the reader, and
  `label:` (also `tag:`, `keyword:`) narrows search. Stored as IMAP custom
  keywords through Dovecot's `dovecot-keywords` mapping — the same letters
  and mapping file `mbsync`, `notmuch`, and Dovecot itself read — so labels
  set here appear in other clients and vice versa. Undoable, offline-safe,
  synced by name to the server.

- Accounts are added in Envelope itself: Accounts → Add account takes a
  name, an address, and a password, and finds the servers from the address —
  provider registry, built-in table, autoconfig, probe — so IMAP, JMAP, and
  POP3 accounts are one form. Addresses at a provider whose browser sign-in
  is configured get the sign-in button instead of a password field. When no
  server can be found the account is saved anyway and the server form opens
  on it with the reason. Accounts can be removed from the same page; the
  mail already on disk stays.
- Calendar invitations hand off to the calendar: a message carrying a
  `text/calendar` part with a METHOD grows an "Open in calendar" button that
  hands the invitation, byte-for-byte, to Slate when it is running — the
  accept/decline decision happens there. In return, Envelope accepts
  scheduling replies from Slate and sends them through the durable outbox.
  Neither app launches the other; without Slate, the .ics saves like any
  attachment.
- Bounces read as reports, not correspondence: a delivery failure arriving in
  the inbox is announced in the status line, and opening one shows who could
  not be reached and the server's reason — with the classification that
  matters (a DMARC or quota failure is your provider's problem, not a dead
  address; a full mailbox is not a gone correspondent).
- Send-as aliases: add the other addresses your provider accepts on the
  Accounts page, and the composer grows a From picker. A reply goes out as
  the address the original was sent to, drafts keep the identity they were
  written as, and a missing alias name borrows the account's.
- Undo send: a sent message waits a configurable grace (10 seconds by
  default, in Settings) in the outbox, where `z` takes it back into the
  composer. Send later queues it for later today, tomorrow morning, or next
  week through the same outbox.
- Snooze: `b` (or the menu) puts a conversation away until later today,
  tomorrow morning, or next week. Snoozed mail waits in a Snoozed folder on
  the server — visible from every device — and comes back through the same
  check that fetches new mail: to the folder it was deferred from, and
  unread, with the star it had left untouched. A wake that cannot reach the
  server stays due and is tried again rather than being dropped. Undoable,
  like any move.
- Filter rules: match arriving mail on sender, recipients, subject, or
  mailing list, and file, read, star, or delete it — automation of the verbs
  you already have. Edited on the Filter rules page or by hand in the
  account's `.rules.toml`; rules act on mail that arrives after they exist,
  never retroactively on the archive.
- Server-side drafts: a saved draft now appears in the account's Drafts
  folder, edits replace the server copy rather than accumulating beside it,
  and a discard retires it — including discards made offline. Opening a
  message in the Drafts folder resumes it in the composer, drafts written on
  other devices included. IMAP accounts only for now.
- Folder management: create, rename, and delete folders from the menu or the
  command palette, and move a conversation to any folder with `v` — a picker
  in the palette's shape, with undo.
- A crash now leaves a report under the state directory, and the next launch
  says so once in the status line.
