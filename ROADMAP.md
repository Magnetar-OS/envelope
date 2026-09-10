# Roadmap

The direction: a mail client that goes 1:1 against the best existing one,
pixel-perfect and delightful on COSMIC, with 100% suite integration — while
keeping the engineering positions that make Envelope worth building at all.

## The benchmark

**Thunderbird's mail core** — not all of Thunderbird. Calendar is Slate's job,
contacts are Circle's, and chat/RSS/news are nobody's. What remains is the
feature surface a daily mail user expects: every triage verb, every compose
mode, filters, encryption, multiple identities, and a reader that shows what
the sender sent. Geary would be too low a bar — Envelope already exceeds it on
engines (five protocols vs one), search, and keyboard.

Meltemi remains the *donor* benchmark (ports, not designs); Thunderbird is the
*parity* benchmark (what users notice missing).

## The one contradiction, resolved — and how it actually resolved

1:1 parity and "the reader renders text, not HTML" cannot both hold. Every
mainstream client renders HTML because most mail *is* HTML, and a text
extraction — however honest — is not pixel-anything against a message designed
visually. The position was never "no HTML"; it was **no parser differential,
no remote loads, no script**.

This was planned as `blitz` — Servo's `stylo` for CSS, `parley` for text,
**wgpu** for paint. That is not what shipped, and the difference is worth
stating rather than quietly closing the box.

**What shipped** is a *structural* reader: `nib-html` reads the message into a
Nib document against a schema, and Nib's widget draws it. The three properties
hold, and hold more strongly than the plan promised:

- **No script, no style** — not because no JavaScript engine is linked, but
  because the schema is the allow-list. A `<script>`, a `<style>` and an
  inline `style=` have nowhere in the model to land, so they are
  unrepresentable rather than stripped.
- **No remote loads** — not a default that could be flipped, but the absence
  of a loader. Nothing in Nib resolves a URL; an `<img>` draws its alt text.
- **One tree** — the same html5ever parse the text extractor walks. Nothing
  downstream re-parses, so there is still no sanitiser to disagree with.
- **Text view remains** the path for `text/plain`, and remains the source of
  quoting: a document has no `>` markers to fold.

**Styling shipped too**, as a subset rather than an engine. `nib-css` reads
`style=` and the presentational attributes into marks: colour, background,
weight, size, alignment. It is not a sanitiser — a declaration maps onto one of
those or it does not exist downstream — and it refuses three things by
construction: anything that fetches, anything that positions, and anything that
hides, the last being reported rather than obeyed. A sender's colour is then
checked against the pixels it will actually land on and dropped below 3:1
contrast, which closes white-on-white without a list of suspicious colours to
keep current.

**What did not ship** is *layout*: no box model, no floats, no positioning, no
sizing. A message designed as a poster reads as its structure. That is a
smaller promise than `blitz`, and it is the one obtainable without putting a
rendering engine inside the trust boundary. Whether to take the larger one is a
live question, not a scheduled task: it needs its own argument.

---

## Phase 0 — Gates and rails

Unblocks everything else; most items are small.

- [x] **Meltemi licence declaration** — the publishing gate for
      `cosmic-pim-mail`. One commit in the donor repo; until it lands, nothing
      ships beyond a git checkout.
- [x] **`rust-toolchain.toml` + `rustfmt.toml`** (`imports_granularity =
      "Module"`), agreeing with `rust-version` — the convention the rest of the
      ecosystem holds.
- [x] **Server zoo in CI.** *(Dovecot leg live; real-provider legs need credentials and stay manual.)* `tests/live_dovecot.rs` found two real bugs on its
      first run; that argument settles the investment. Add scripted/containered
      runs against Gmail-IMAP, Outlook.com/Exchange, Fastmail (JMAP), iCloud,
      and Yahoo, each feeding the **quirks table** (Dovecot 2.4's missing
      HIGHESTMODSEQ is entry #1).
- [ ] **Packaging** *(deb/rpm/arch ship via the release kit; Flatpak and per-size icons still open)*: `debian/`, Flatpak manifest, `just vendor` kept working,
      per-size icons alongside the scalable one.
- [x] **Crash and error surfacing** — a panic hook that writes a report file
      and says so on next launch. A client trusted with mail cannot fail
      silently.

## Phase 1 — Triage parity

The verbs daily users miss first, in the order they miss them
(from `04-envelope.md`, confirmed against the Thunderbird checklist):

- [x] **Server-side drafts.** UIDPLUS where offered, `Message-ID` match where
      not, reconciliation for servers that mangle both. Already specified;
      "worth building, not worth shipping half of" now comes due.
- [x] **Labels / keywords.** *(shipped over the dovecot-keywords mapping:
      chips in list and reader, an `l` picker that creates/applies/clears,
      `label:` in search, live-Dovecot round trip. Still open: chips on the
      Gmail engine's label folders, per-label colours.)*
- [x] **Rules / filters.** Client-side first (on-sync: move, label, mark,
      delete), stored in the suite's config; ManageSieve for server-side rules
      later. Rules compose from the same action registry the palette reads.
- [x] **Snooze.** The keyword scheme Meltemi used exists in the donor;
      snoozed mail leaves the inbox and returns through the poll.
- [x] **Folder management.** *(drag still open; create/rename/delete/move-picker shipped)* Create, rename, delete, subscribe; drag a
      conversation to a folder; the tree stops being read-only.
- [x] **Multiple identities / aliases** per account, with reply-from-the-
      address-it-was-sent-to as the default.
- [x] **Undo send** — a configurable delay before the SMTP conversation
      starts, which is the only honest implementation. **Scheduled send**
      rides the same outbox.
- [ ] **mbox import** *(done; Thunderbird/Geary profile import still open)* (export exists via `.eml`); import from Thunderbird and
      Geary profiles is the migration story.
- [x] **DSN ingestion** — bounce parsing wired to the outbox, so a failed
      delivery is a state on the message, not a mystery mail from MAILER-DAEMON.

## Phase 2 — The reader

The blitz decision above, executed:

- [x] **Decision record** in `04-envelope.md`: the display-security section
      rewritten around properties (nothing loads, nothing scripts or styles,
      one tree) rather than the text-only mechanism.
- [x] **HTML reading**, structurally: html5ever → `nib-html` → a schema-bound
      document → Nib's widget, read-only, inside the reader pane. No `stylo`
      and no per-sender remote allow list — there is no loader for an allow
      list to govern.
- [x] **Sender styling**, as `nib-css`'s subset: colour, background, weight,
      size and alignment, with contrast enforced at the point of drawing so a
      colour cannot hide text.
- [ ] **CID inline images.** Blocked on Nib, not on policy: `image` is an
      inline node and iced's `Span` carries no width, so there is no way to
      reserve space for a replaced element inside a shaped paragraph. Needs
      inline replaced-element layout in Nib. The bytes are local — nothing
      about the security position is in the way.
- [ ] **Visual layout.** The box model — floats, positioning, sizing, tables
      as grids. The `blitz` plan, if it is taken at all: a rendering engine in
      the trust boundary is a decision, not a task.
- [ ] **Reader parity details**: zoom, print (via xdg portal), find-in-message,
      full header view, message source view.
- [ ] **HTML composition** — the second body on the same `Draft`, text part
      always generated, exactly as the send path anticipated. Compose stays
      structured (fields, not bytes) until `build()`. Closer than this list
      suggests: the composer's body is already a Nib document, and
      `nib-html` serialises as well as it parses.

## Phase 3 — Trust

- [ ] **OpenPGP** — Sequoia or rPGP; verify and decrypt first, sign and
      encrypt second. Key discovery via WKD and Autocrypt headers. Display
      follows the house rule extended, not broken: failures are loud, an
      *encrypted* state is shown (it changes what you may write in reply),
      routine valid signatures stay quiet.
- [ ] **S/MIME**, because corporate mail is where parity is lost or won.
- [ ] Both are ports in spirit — `pgp_mail.rs` and `smime.rs` exist in the
      donor.

## Phase 4 — 100% COSMIC

Everything the conventions document checklist demands, plus the suite
contracts:

- [ ] **cosmic-config for every setting**, `watch_config` live reload —
      settings changed in one window apply everywhere, now.
- [ ] **Notifications** through the portal, with actions (Archive, Reply
      opens the composer), respecting Do Not Disturb; new-mail notifications
      driven by the IDLE connection, so they are push, not poll.
- [ ] **envelope-launcher** — the pop-launcher plugin over the search index,
      mirroring slate-launcher: type a sender or subject in the shell, land in
      the conversation.
- [ ] **A panel applet**: unread count, a peek popup of the newest inbox rows,
      one click to the app with an activation token and a systemd scope — the
      full three-step launch sequence from the conventions notes.
- [ ] **Suite contracts**: sender → Circle contact card in the reader;
      last-contact hook for Circle's CRM layer; **iMIP handoff to Slate**
      (the one cross-process contract — an invite in mail becomes an event
      with Accept/Decline that sends the reply); `mailto:` registration
      already works and stays the target for both apps.
- [ ] **Conventions audit** against the 15-item checklist: xdgen with the
      `CARGO_TARGET_DIR` fix, metainfo carrying
      `com.system76.CosmicApplication`, branding colours, screenshots, and a
      release matching Cargo.toml; icon names resolving COSMIC → Pop →
      hicolor with explicit fallbacks; the About icon embedded.
- [ ] **Accessibility and i18n**: AccessKit wiring audited with a screen
      reader, full keyboard-only pass, RTL layout check (Fluent's bidi
      isolation is already in), catalogue layout kept Weblate-shaped so
      translations arrive as PRs.

## Phase 5 — Pixel-perfect and delightful

Polish is a phase because it is verifiable work, not garnish:

- [ ] **Token purity**: no raw pixel value, no literal colour — spacing and
      radii from the theme, semantic container/button classes, frosted keys
      respected. A grep-able standard: `grep -n '\.padding([0-9]'` returns
      nothing.
- [ ] **Motion**: libcosmic's lilt-backed `Animation` for list mutations
      (archive slides away, undo slides back), reader transitions, toast
      timing — interruptible, and honouring reduced-motion.
- [ ] **A virtualized conversation list**, so a 100k-message folder scrolls
      at the same cost as a hundred. The index already makes the data cheap;
      the widget must match.
- [ ] **Performance budgets, asserted in CI** where measurable: folder switch
      is one index query; search-as-you-type under a keystroke's worth of
      latency (the **tantivy tier** lands when a mailbox outgrows bm25-on-FTS5
      — the port is already scoped); cold start to painted list without
      waiting on the network.
- [ ] **First-run and empty states**: account discovery already works out the
      server from the address — the wizard around it should feel like that.
      Every empty pane says what it is and what to do; every error names the
      account and the action that failed.
- [ ] **Keyboard completeness**: every palette action bindable, quick-step
      macros (one key = archive + label + next), and the `?` sheet stays
      generated, never hand-written.

## Phase 6 — 1.0

Ship when all of these are true, not when a date arrives:

1. The parity matrix below has no ✗ in the Thunderbird-core column.
2. The server zoo is green: Dovecot, Gmail, Exchange, Fastmail, iCloud, Yahoo.
3. The invariants still hold and are still asserted on the wire: verbatim
   bytes, push-before-pull, `BODY.PEEK[]`, Bcc off the wire copy.
4. Accessibility pass done with a screen reader; ≥ a handful of locales live
   via Weblate.
5. No known data-loss bug, of any severity, open.

## Parity matrix

| Capability | Thunderbird core | Envelope today | Phase |
|---|---|---|---|
| IMAP / JMAP / Gmail / Graph / POP3 | IMAP, POP3, Exchange via add-on | ✓ all five | — |
| Push (IDLE) | ✓ | ✓ | — |
| Unified inbox | ✓ | ✓ | — |
| Threading | ✓ | ✓ (JWZ, deterministic ids) | — |
| Search with operators | ✓ | ✓ (bodies, bm25) | tantivy tier: 5 |
| Undo, outbox, offline queue | partial | ✓ | — |
| Keyboard + palette | partial | ✓ (registry-generated) | — |
| Server-side drafts | ✓ | ✗ local only | 1 |
| Labels / tags | ✓ | ✗ | 1 |
| Filters / rules | ✓ | ✗ | 1 |
| Folder management | ✓ | ✗ read-only tree | 1 |
| Identities / aliases | ✓ | ✗ | 1 |
| Scheduled send / undo send | ✓ | ✗ | 1 |
| Import (mbox, profiles) | ✓ | ✗ (export only) | 1 |
| HTML reading | ✓ | ~ structure + styling, no layout | 2 |
| HTML composition | ✓ | ✗ plain text | 2 |
| Print / find-in-message | ✓ | ✗ | 2 |
| OpenPGP / S/MIME | ✓ | ✗ | 3 |
| Calendar invites (iMIP) | ✓ in-app | → Slate handoff | 4 |
| Notifications with actions | ✓ | ✗ | 4 |
| Address book integration | ✓ in-app | → Circle | 4 |

## Still not doing

Unchanged, and worth restating so the roadmap cannot be read as "everything":

- **A webview or JS engine**, ever. HTML arrives on Envelope's terms or not
  at all.
- **An encrypted store.** Files-as-truth is the suite's promise; encryption
  at rest is the disk's job.
- **The AI layer, energy pacing, message typing, alternative inbox views.**
  Meltemi's product, not the client underneath it.
- **Chat, RSS, news.** Thunderbird's accidents of history, not its mail core.

## Ordering logic

Phase 0 gates distribution; nothing later matters if the crates cannot be
published or a regression ships unnoticed. Phase 1 is highest
missed-per-user-per-day and needs no new architecture — every item lands on
the existing registry, outbox, and sync pass. Phase 2 is sequenced before
trust and polish because HTML rendering changes the reader's layout engine,
and pixel-perfecting a pane twice is doing it once, badly. Phases 4 and 5
overlap in practice — integration items are independent of each other and can
interleave. Phase 6 is a checklist, not a phase.
