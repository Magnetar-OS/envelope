# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

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
  the server — visible from every device — and comes back to the inbox
  through the same check that fetches new mail. Undoable, like any move.
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
