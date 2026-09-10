# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- An HTML message shows the sender's styling: colour, background, bold and
  italic, text size and alignment, read from `style=` and from the `bgcolor`
  and `<font>` attributes mail generators still emit. A receipt looks like a
  receipt.

  **A colour cannot hide text.** Every colour is checked against the pixels it
  will actually land on and swapped for your own when it would fall below
  readable contrast — so white-on-white, and the near-misses either side of it,
  cannot be shown at all. Not a filter that might miss one: the check is on the
  result, after transparency is resolved, so there is no way to phrase a colour
  that gets past it. Anything that would fetch, position, or hide is refused
  before it reaches the message at all.

- An HTML message shows its structure. Headings, lists, tables, links and the
  quoted blockquotes a thread is actually made of, instead of the flattened
  text they were being reduced to. Plain-text messages are unchanged — their
  quoted history still folds, which is what the `>` markers are for.

  Nothing new can be fetched. The message is read into a document whose schema
  is the allow-list: a `<script>`, a `<style>` and an inline `style=` have
  nowhere to land, and an image draws its alt text because there is no image
  loader anywhere in the path. A tracking pixel could not fire before and
  still cannot.

### Changed

- The display-security position is now written as the properties it always
  stood for — nothing loads, nothing scripts or styles, one parser — rather
  than as "text only". `04-envelope.md`, `PARITY.md` and `ROADMAP.md` say what
  actually shipped, including what did not: there is no CSS, so a visually
  designed message reads as its structure, and inline CID images remain alt
  text.

- The composer's body is a document rather than a string, on the Nib text
  engine. What you notice: a reply opens with the cursor already in it
  instead of nowhere, Enter inside quoted text keeps the reply quoted
  because the cursor is genuinely inside the quote, and a sent message is
  wrapped from that structure — so a wrapped quoted line keeps its markers
  instead of being re-read out of the finished text to guess where they went.

- Undo steps back over a run of typing rather than a word. A pause starts a
  new step, as does typing somewhere else; the floor is unchanged, so Ctrl+Z
  in a reply still cannot eat the quoted message you never typed.

- Two paragraphs inside a quote go out with a bare `>` between them, and a
  reply written under a quote is separated from it by a blank line. Both are
  what the structure means in `text/plain`, and both are what the next
  client needs in order to re-quote the thread without collapsing it.

- A plain-text message is no longer run through the HTML parser on its way to
  the screen. The parser it came from invents an HTML version of any message
  that has none, and that invention was being taken at face value.

### Fixed

- What the app says, you now see. Every confirmation and warning — sent,
  queued, snoozed mail returning, a rule filing something, a delivery
  failure, a key imported, last session's crash — was written to a status
  line that only the Accounts page rendered. They surface as toasts now,
  over whatever is on screen; the Accounts page keeps its inline line for
  sync progress.

- The composer writes real messages. Its body was a single-line field, so
  Enter did nothing, a pasted quote arrived stripped of its line breaks, and
  every reply opened with the text it was answering flattened into one line.
  It is a proper editor now, which also brings selection, word-by-word
  movement, Home and End, and a right-click menu.

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
- `Ctrl+Z` in the composer undoes typing rather than the last mail
  operation. It was reaching past the composer entirely and un-archiving a
  conversation the user had finished with — two windows away from what they
  were looking at.
- Outgoing message bodies are wrapped at 72 columns. A paragraph typed into
  the editor went out as one line however long it was, which renders badly
  in clients that do not soft-wrap and worse once quoted into a reply. URLs
  and the signature separator are left intact, and saved drafts keep what
  was typed — the wrap happens on the way out, not on every save.
- List rows keep a fixed height: a long subject or a crowd of recipients is
  clipped with an ellipsis instead of wrapping onto a second line and making
  the list jump as it scrolls.

### Added

- The columns are yours to size: drag the edge beside the sidebar or the
  message list, and both widths are remembered. The edge stays a hairline —
  only the pointer changes over it.
- The composer is one surface rather than a stack of boxes. From, To and the
  subject sit on a shared left edge under muted labels, separated by
  hairlines, with the body running on from them borderless and inset to the
  same edge. Cc and Bcc stay out of the way behind a Cc/Bcc button until
  they are wanted, and appear on their own whenever a draft already carries
  either — a reply-all, or a draft reopened.
- Rows react to the pointer. Every list in the application — folders,
  messages, drafts, the outbox, search results, the palette — now uses the
  desktop's own list-row styling, so hovering highlights and the selection
  is marked with the accent instead of being filled with it.

- Mail is written and read in windows of its own. Composing, replying and
  forwarding open a window per message, the way every desktop mail client
  does it, so a reply sits beside the thread it answers instead of
  replacing it — and two replies can be open at once, which was not
  expressible before. A message can be sent to its own window from the
  reader ("Open in new window", `o`), which leaves the list free to move on
  without taking the message with it. Each window carries the subject as
  its title, files and flags the message it is actually showing, and keeps
  what was typed when it is closed — by its own button, by the compositor,
  or by the application quitting with it open.

- Quoted history folds. A plain-text body is now read as the structure it
  actually has — what this sender wrote, what they were quoting, and their
  signature — with quoted runs collapsed behind a control that says how many
  lines they hold, and shown dimmed when opened. On the fifth reply of a
  thread the two lines that are new are no longer buried under forty that
  are not.

- The composer continues what a line was. Enter inside `> ` quoted text
  keeps the reply quoted, which is what stops the rest of a paragraph being
  attributed to whoever was being quoted; bullets and numbered items
  continue and renumber the same way, and pressing Enter on an empty one
  leaves the list.

- Undo and redo in the composer, by word rather than by keystroke, with the
  cursor returned to where the edit started. `Ctrl+Z` and `Ctrl+Shift+Z`;
  undo stops at the body the composer opened with, so a reply's quoted text
  cannot be undone away.

- The sidebar can be hidden: the header's toggle, `F9`, the menu, and the
  palette all flip it, over the desktop's own show/hide state — so in a
  narrow window it overlays the list, as in the other COSMIC apps.
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
