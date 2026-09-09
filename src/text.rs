// SPDX-License-Identifier: GPL-3.0-only

//! The text machinery: what the reader makes of a body, and what the composer
//! does while one is being written.
//!
//! # Why this is a module rather than code in the views
//!
//! Everything here is a pure function of text. That is the whole point: the
//! decisions about what a quote *is*, where a signature starts, what Enter
//! should continue, and where a line may be broken are the kind of decisions
//! that are wrong in small ways for years if they are only ever exercised by
//! looking at the screen. As functions over `&str` they are tested at the
//! bottom of this file, and the views are left with nothing but layout.
//!
//! # The three decisions worth stating
//!
//! Each of these is a choice rather than a default, and each is noted again
//! where it happens:
//!
//! - **Quoting is the mail case.** A general text editor continues `- ` and
//!   `1. `; a mail composer's overwhelming case is `> `, nested. Both are
//!   handled and the quote is the one that matters — a reply written inside
//!   quoted text that silently stops being quoted halfway down attributes the
//!   rest of the paragraph to the person being quoted. See [`continuation`].
//! - **Undo coalesces.** A history entry per keystroke makes Ctrl+Z walk back
//!   one letter at a time. [`History`] groups a run of typing into one entry,
//!   so Ctrl+Z walks back one word at a time — which is what the keystroke
//!   means everywhere else on the desktop.
//! - **Snapshots, not diffs.** Storing patches buys a saving that only pays
//!   on a large document, at the cost of keeping them alive. A mail body is a
//!   few kilobytes and the history is bounded, so [`History`] keeps whole
//!   strings and owns them.
//!
//! # What is deliberately not here
//!
//! - **A formatting toolbar.** Envelope composes `text/plain` on purpose (see
//!   `cosmic_pim_mail::compose`), so markers a recipient would read literally
//!   — `**bold**`, `# headings` — are not an improvement.
//! - **Reflowing a received body.** A hard-wrapped paragraph could be joined
//!   back up before display, but only `format=flowed` (RFC 3676) says which
//!   line breaks were the sender's and which were the wrapper's. Without that
//!   parameter — which the stored message does not currently carry up —
//!   unwrapping is guessing, and guessing wrong reflows a table into prose.

use std::fmt::Write as _;

/// Where an outgoing `text/plain` body is wrapped.
///
/// RFC 5322 asks for lines under 78 characters; 72 is the convention on top of
/// it, and the eight characters of headroom are what lets the message survive
/// being quoted twice without the wrap collapsing.
pub const WRAP_COLUMNS: usize = 72;

/// The signature separator, exactly as RFC 3676 §4.3 spells it — two hyphens,
/// a space, end of line. Emitted without the space by enough clients that the
/// bare form is recognised too, but never *written* that way.
const SIGNATURE_MARKER: &str = "--";

// ---------------------------------------------------------------------------
// The reader: what a body is made of
// ---------------------------------------------------------------------------

/// One run of a plain-text body.
///
/// A mail body is not a paragraph of text, it is a small stack: what was
/// written now, what was written before, and who wrote it. Flattening that into
/// one string is what makes a twentieth reply unreadable — the two lines that
/// are new sit above forty that are not, in the same colour, at the same size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Written by the sender of this message.
    Prose(String),
    /// Quoted history, with one level of `>` removed. Deeper nesting keeps its
    /// markers, which is how every mail client has ever shown it.
    Quoted {
        /// The shallowest depth in the run, so a reader can say how deep the
        /// history goes without re-scanning.
        depth: usize,
        text: String,
    },
    /// Everything after the sender's `-- ` line.
    Signature(String),
}

/// Splits a plain-text body into prose, quoted history, and a signature.
///
/// The rules, in the order they are applied:
///
/// - A line's quote depth is its leading run of `>`, ignoring spaces between
///   them, so `>>`, `> >` and `> > ` are all depth two.
/// - Consecutive lines at depth ≥ 1 are one [`Block::Quoted`]. They are *not*
///   split by depth changes: a reply that dips into a deeper quote and comes
///   back is one piece of history, and three blocks where the user sees one
///   run would fold into three separate controls.
/// - A blank line inside a quoted run stays in the run when quoted text
///   resumes after it. Senders leave real blank lines between quoted
///   paragraphs, and treating each as a boundary shatters the run.
/// - The first unquoted `--` or `-- ` line ends the message; the rest is the
///   signature. Quoted signatures are at depth ≥ 1 and so cannot trigger this.
#[must_use]
pub fn blocks(body: &str) -> Vec<Block> {
    let lines: Vec<&str> = body.lines().collect();
    let mut blocks = Vec::new();
    let mut run: Vec<&str> = Vec::new();
    let mut run_depth: Option<usize> = None;
    let mut index = 0;

    // Pushes whatever has accumulated, as prose or as quoted history.
    fn flush(blocks: &mut Vec<Block>, run: &mut Vec<&str>, depth: Option<usize>) {
        if run.iter().all(|line| line.trim().is_empty()) {
            run.clear();
            return;
        }
        blocks.push(match depth {
            Some(depth) => {
                let stripped: Vec<&str> =
                    run.iter().map(|line| strip_one_quote_level(line)).collect();
                Block::Quoted {
                    depth,
                    text: trimmed_join(&stripped),
                }
            }
            None => Block::Prose(trimmed_join(run)),
        });
        run.clear();
    }

    while index < lines.len() {
        let line = lines[index];
        let depth = quote_depth(line);

        if depth == 0 && is_signature_marker(line) {
            flush(&mut blocks, &mut run, run_depth);
            let signature = trimmed_join(&lines[index + 1..]);
            if !signature.is_empty() {
                blocks.push(Block::Signature(signature));
            }
            return blocks;
        }

        // A blank line belongs to the run it sits inside. It only ends a
        // quoted run when nothing quoted follows it.
        let blank = line.trim().is_empty();
        let joins_quote = blank && run_depth.is_some() && next_quoted(&lines[index + 1..]);

        if blank && joins_quote {
            run.push(line);
            index += 1;
            continue;
        }

        let kind = if depth > 0 { Some(depth) } else { None };
        match (run_depth, kind) {
            // Same kind of run continues; a quoted run keeps its shallowest
            // depth, which is the level the reader is being shown.
            (Some(current), Some(depth)) => run_depth = Some(current.min(depth)),
            (None, None) => {}
            _ => {
                flush(&mut blocks, &mut run, run_depth);
                run_depth = kind;
            }
        }
        run.push(line);
        index += 1;
    }

    flush(&mut blocks, &mut run, run_depth);
    blocks
}

/// Joins lines and trims the blank ones off both ends, which is what every
/// block boundary leaves behind.
fn trimmed_join(lines: &[&str]) -> String {
    lines.join("\n").trim_matches('\n').trim_end().to_owned()
}

/// Is a quoted line coming, before any unquoted text does?
fn next_quoted(rest: &[&str]) -> bool {
    rest.iter()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| quote_depth(line) > 0)
}

/// How many `>` a line opens with, ignoring the spaces between them.
#[must_use]
pub fn quote_depth(line: &str) -> usize {
    let mut depth = 0;
    for byte in line.bytes() {
        match byte {
            b'>' => depth += 1,
            b' ' | b'\t' => {}
            _ => break,
        }
    }
    depth
}

/// The leading quote markers of a line, as written — `> `, `>> `, `> > `.
///
/// Returned verbatim rather than normalised, so continuing a line reproduces
/// the sender's spelling instead of imposing ours halfway down a quote.
#[must_use]
pub fn quote_prefix(line: &str) -> &str {
    let end = line
        .bytes()
        .position(|byte| !matches!(byte, b'>' | b' ' | b'\t'))
        .unwrap_or(line.len());
    // Only the part up to and including the last `>`, plus the single space
    // that conventionally follows it. Anything beyond that is the quoted
    // line's own indentation and belongs to its text.
    let markers = &line[..end];
    match markers.rfind('>') {
        Some(last) => {
            let after = last + 1;
            if line[after..].starts_with(' ') {
                &line[..after + 1]
            } else {
                &line[..after]
            }
        }
        None => "",
    }
}

/// Removes exactly one `>` and the space after it, if there is one.
fn strip_one_quote_level(line: &str) -> &str {
    let trimmed = line.trim_start_matches([' ', '\t']);
    match trimmed.strip_prefix('>') {
        Some(rest) => rest.strip_prefix(' ').unwrap_or(rest),
        None => line,
    }
}

fn is_signature_marker(line: &str) -> bool {
    // Trailing whitespace only: `-- ` is the spelling, and the space is
    // stripped by enough intermediaries that requiring it would miss most of
    // the signatures that exist.
    line.trim_end() == SIGNATURE_MARKER
}

// ---------------------------------------------------------------------------
// The composer: what Enter continues
// ---------------------------------------------------------------------------

/// What the next line should open with, when Enter is pressed on `line`.
///
/// - `None` — nothing to continue; Enter is an ordinary line break.
/// - `Some("")` — the line is an *empty* continued item, so Enter should clear
///   it rather than produce a second empty one. That is how a list or a quote
///   is left: press Enter twice.
/// - `Some(prefix)` — open the next line with this.
///
/// Quote prefixes come first, because in a mail composer they are the case
/// that happens: a reply is written *inside* quoted text as often as above it,
/// and a client that drops the `> ` halfway through attributes the rest of the
/// paragraph to whoever is being quoted.
#[must_use]
pub fn continuation(line: &str) -> Option<String> {
    let quote = quote_prefix(line);
    if !quote.is_empty() {
        let rest = &line[quote.len()..];
        // An empty quoted line: the user is leaving the quote.
        if rest.trim().is_empty() {
            return Some(String::new());
        }
        // A list inside a quote continues both.
        return Some(match list_prefix(rest) {
            Some(marker) if !rest[marker.len()..].trim().is_empty() => format!("{quote}{marker}"),
            _ => quote.to_owned(),
        });
    }

    let indent = leading_spaces(line);
    let rest = &line[indent.len()..];
    let marker = list_prefix(rest)?;
    if rest[marker.len()..].trim().is_empty() {
        return Some(String::new());
    }
    Some(format!("{indent}{marker}"))
}

/// The list marker a line opens with, in the form the next line should use.
///
/// A numbered item returns the *next* number, a checked box returns an
/// unchecked one — continuing `- [x] ` with `- [x] ` would tick a box nobody
/// ticked.
fn list_prefix(rest: &str) -> Option<String> {
    for prefix in ["- [ ] ", "- [x] ", "- [X] "] {
        if rest.starts_with(prefix) {
            return Some("- [ ] ".to_owned());
        }
    }
    for prefix in ["- ", "* ", "+ "] {
        if rest.starts_with(prefix) {
            return Some(prefix.to_owned());
        }
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        let separator = rest[digits..].chars().next()?;
        if matches!(separator, '.' | ')') && rest[digits + 1..].starts_with(' ') {
            let number: usize = rest[..digits].parse().ok()?;
            let mut prefix = String::new();
            let _ = write!(prefix, "{}{separator} ", number + 1);
            return Some(prefix);
        }
    }
    None
}

fn leading_spaces(line: &str) -> &str {
    let end = line
        .bytes()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .unwrap_or(line.len());
    &line[..end]
}

// ---------------------------------------------------------------------------
// The composer: what goes out
// ---------------------------------------------------------------------------

/// Hard-wraps a `text/plain` body at `columns`.
///
/// # Why the composer wraps at all
///
/// A body typed into a text editor is one line per paragraph, however long
/// that paragraph is. Sent as `text/plain` it reaches a recipient whose client
/// may not soft-wrap at all, and it reaches the *next* reply as a quoted line
/// that is now four characters longer. Wrapping once, at the point of sending,
/// is what keeps a thread readable at its fifth reply.
///
/// # What it will not do
///
/// - Break a word that does not fit. A URL is one word, and a URL broken
///   across two lines is a URL that no longer works — an over-long line is the
///   lesser damage, and the only one that is reversible.
/// - Touch the signature separator. `-- ` must survive exactly, trailing space
///   and all, or the recipient's client stops recognising the signature.
/// - Lose a quote prefix. A wrapped quoted line continues with the same
///   markers, so the recipient's client still sees consistent depth.
#[must_use]
pub fn wrap(body: &str, columns: usize) -> String {
    let mut out = String::with_capacity(body.len() + body.len() / 8);
    let mut past_signature = false;

    for line in body.lines() {
        if !out.is_empty() {
            out.push('\n');
        }
        // Everything from the separator down is the signature, and a signature
        // is laid out by whoever wrote it.
        if past_signature || is_signature_marker(line) {
            past_signature = true;
            out.push_str(line);
            continue;
        }
        wrap_line(line, columns, &mut out);
    }

    // `lines()` drops a trailing newline; a body that ended with one still
    // ends with one, because the user put it there.
    if body.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn wrap_line(line: &str, columns: usize, out: &mut String) {
    if line.chars().count() <= columns {
        out.push_str(line);
        return;
    }

    // Continuation lines carry the quote markers, then the line's own indent.
    // A quoted line's indent is measured after the markers, or the quote's
    // own space would be counted twice.
    let quote = quote_prefix(line);
    let body = &line[quote.len()..];
    let indent = leading_spaces(body);
    let prefix = format!("{quote}{indent}");
    let prefix_width = prefix.chars().count();

    // A prefix that already fills the line leaves nothing to wrap into;
    // emitting the line whole beats emitting one word per line forever.
    let Some(available) = columns.checked_sub(prefix_width).filter(|width| *width > 0) else {
        out.push_str(line);
        return;
    };

    let mut first = true;
    let mut column = 0;
    for word in body[indent.len()..].split(' ').filter(|w| !w.is_empty()) {
        let width = word.chars().count();
        if first {
            out.push_str(&prefix);
            first = false;
        } else if column + 1 + width > available {
            out.push('\n');
            out.push_str(&prefix);
            column = 0;
        } else {
            out.push(' ');
            column += 1;
        }
        out.push_str(word);
        column += width;
    }

    // A line that was only whitespace past its prefix still has to appear.
    if first {
        out.push_str(line);
    }
}

// ---------------------------------------------------------------------------
// The composer: undo
// ---------------------------------------------------------------------------

/// What kind of edit produced a state, which is what decides whether the next
/// one joins it or starts a new step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// A printable character. Runs of these are one undo step.
    Typing,
    /// A space, a tab, or a line break — the boundary between words, and so
    /// the boundary between undo steps.
    Break,
    /// Backspace or delete. Runs of these are one undo step, separately from
    /// typing: undoing a deletion should not also undo what was typed before
    /// it.
    Erase,
    /// A paste, an indent, or anything else that arrives whole. Never joins
    /// anything — it was one action for the user, so it is one for undo.
    Bulk,
}

impl EditKind {
    /// What an editor action counts as, or `None` if it did not change the
    /// text at all.
    #[must_use]
    pub fn of(action: &cosmic::widget::text_editor::Action) -> Option<Self> {
        use cosmic::widget::text_editor::{Action, Edit};
        match action {
            Action::Edit(Edit::Insert(character)) => Some(if character.is_whitespace() {
                Self::Break
            } else {
                Self::Typing
            }),
            Action::Edit(Edit::Enter) => Some(Self::Break),
            Action::Edit(Edit::Backspace | Edit::Delete) => Some(Self::Erase),
            Action::Edit(Edit::Paste(_) | Edit::Indent | Edit::Unindent) => Some(Self::Bulk),
            _ => None,
        }
    }

    /// May an edit of this kind extend a step that ended with `previous`?
    const fn joins(self, previous: Self) -> bool {
        match self {
            Self::Typing => matches!(previous, Self::Typing),
            Self::Break => matches!(previous, Self::Break),
            Self::Erase => matches!(previous, Self::Erase),
            Self::Bulk => false,
        }
    }
}

/// A byte offset into a line — the coordinate `text_editor` speaks in.
///
/// `Position::column` is a *byte* index within its line, not a character
/// index; the editor's backend hands `cosmic_text`'s `Cursor::index` straight
/// through. Restoring a cursor therefore has to clamp in bytes and land on a
/// character boundary, which [`History::restore`]'s caller does.
pub type Caret = (usize, usize);

/// One recorded state of the body.
#[derive(Debug, Clone)]
struct Step {
    text: String,
    /// Where the cursor was while this text was the current one.
    caret: Caret,
    /// What produced it. The first step has none — it is where the composer
    /// opened, not the result of an edit.
    kind: Option<EditKind>,
}

/// Undo and redo for the composer's body.
///
/// # Why the composer needs its own
///
/// `text_editor` has none: the widget performs actions and never remembers
/// them. Without this, Ctrl+Z in a half-written message either does nothing or
/// — as it did here — reaches past the composer entirely and undoes the last
/// *mail* operation, un-archiving a conversation the user had finished with.
///
/// # Bounded on purpose
///
/// [`Self::LIMIT`] steps of a body that is already capped in practice by what
/// a person will type. Older steps fall off the front; the oldest surviving
/// step becomes the floor that undo stops at, rather than the history quietly
/// growing for as long as the composer is open.
#[derive(Debug)]
pub struct History {
    steps: Vec<Step>,
    /// Which step is current. Everything above it is redo.
    index: usize,
}

impl History {
    /// How many steps are kept.
    pub const LIMIT: usize = 200;

    /// A history whose only state is the body as it opened — which for a reply
    /// is the quoted message, and is a floor undo must not go below.
    #[must_use]
    pub fn new(text: String) -> Self {
        Self {
            steps: vec![Step {
                text,
                caret: (0, 0),
                kind: None,
            }],
            index: 0,
        }
    }

    /// Notes where the cursor is without recording a step.
    ///
    /// Moving about is not something to undo, but it is worth remembering:
    /// undoing back to this state should land the cursor where the user
    /// actually was, not where they were the last time they typed.
    pub fn moved(&mut self, caret: Caret) {
        self.steps[self.index].caret = caret;
    }

    /// Records the body after an edit.
    ///
    /// `before` is where the cursor was when the edit started, and is written
    /// back onto the *current* step: that is what makes undo return the cursor
    /// to where the user was, even when they moved it without typing since the
    /// last recorded step.
    pub fn record(&mut self, text: String, before: Caret, after: Caret, kind: EditKind) {
        if self.current().text == text {
            self.moved(after);
            return;
        }

        self.steps[self.index].caret = before;
        let joins = self.index + 1 == self.steps.len()
            && self.current().kind.is_some_and(|prev| kind.joins(prev));

        if joins {
            let step = &mut self.steps[self.index];
            step.text = text;
            step.caret = after;
            step.kind = Some(kind);
            return;
        }

        self.steps.truncate(self.index + 1);
        self.steps.push(Step {
            text,
            caret: after,
            kind: Some(kind),
        });
        self.index = self.steps.len() - 1;

        if self.steps.len() > Self::LIMIT {
            let excess = self.steps.len() - Self::LIMIT;
            self.steps.drain(..excess);
            self.index -= excess;
        }
    }

    /// Steps back, returning the text to restore and where to put the cursor.
    pub fn undo(&mut self) -> Option<(&str, Caret)> {
        if self.index == 0 {
            return None;
        }
        self.index -= 1;
        let step = &self.steps[self.index];
        Some((&step.text, step.caret))
    }

    /// Steps forward again.
    pub fn redo(&mut self) -> Option<(&str, Caret)> {
        if self.index + 1 >= self.steps.len() {
            return None;
        }
        self.index += 1;
        let step = &self.steps[self.index];
        Some((&step.text, step.caret))
    }

    #[must_use]
    pub const fn can_undo(&self) -> bool {
        self.index > 0
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.index + 1 < self.steps.len()
    }

    fn current(&self) -> &Step {
        &self.steps[self.index]
    }
}

/// Puts `caret` back on `content`, clamped to somewhere that exists.
///
/// The column is a byte offset (see [`Caret`]), so it is clamped to the line's
/// length in bytes and then walked back to a character boundary — a cursor
/// left inside a multi-byte character is a panic waiting for the next
/// keystroke, and mail is not ASCII.
pub fn restore(content: &mut cosmic::widget::text_editor::Content, caret: Caret) {
    use cosmic::widget::text_editor::{Cursor, Position};

    let (line, column) = caret;
    let line = line.min(content.line_count().saturating_sub(1));
    let column = content.line(line).map_or(0, |text| {
        let mut column = column.min(text.text.len());
        while column > 0 && !text.text.is_char_boundary(column) {
            column -= 1;
        }
        column
    });

    content.move_to(Cursor {
        position: Position { line, column },
        selection: None,
    });
}

/// Replaces a `text_editor`'s whole contents in place.
///
/// Select-all-and-paste rather than a fresh [`Content`], because the widget
/// keeps its scroll position and its focus in the editor it already has.
/// Rebuilding the content throws both away, which on undo means the composer
/// jumps to the top of a long message every time.
///
/// [`Content`]: cosmic::widget::text_editor::Content
pub fn replace(content: &mut cosmic::widget::text_editor::Content, text: &str) {
    use cosmic::widget::text_editor::{Action, Edit};

    content.perform(Action::SelectAll);
    if text.is_empty() {
        // Pasting an empty string is a no-op in the editor, so the selection
        // has to be deleted instead — otherwise undoing back to an empty body
        // leaves the body untouched and selected.
        content.perform(Action::Edit(Edit::Backspace));
    } else {
        content.perform(Action::Edit(Edit::Paste(std::sync::Arc::new(
            text.to_owned(),
        ))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- blocks -----------------------------------------------------------

    #[test]
    fn a_body_with_no_quoting_is_one_block() {
        assert_eq!(
            blocks("Hello.\n\nHow are you?"),
            vec![Block::Prose("Hello.\n\nHow are you?".to_owned())]
        );
    }

    #[test]
    fn a_reply_separates_what_is_new_from_what_is_quoted() {
        let body = "Thanks, that works.\n\nOn Tue, Ada wrote:\n> the original\n> over two lines";
        assert_eq!(
            blocks(body),
            vec![
                Block::Prose("Thanks, that works.\n\nOn Tue, Ada wrote:".to_owned()),
                Block::Quoted {
                    depth: 1,
                    text: "the original\nover two lines".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn nesting_stays_one_run_and_keeps_its_inner_markers() {
        let body = "> outer\n>> inner\n> outer again";
        assert_eq!(
            blocks(body),
            vec![Block::Quoted {
                depth: 1,
                text: "outer\n> inner\nouter again".to_owned(),
            }]
        );
    }

    #[test]
    fn a_blank_line_inside_a_quote_does_not_split_it() {
        let body = "> first paragraph\n\n> second paragraph";
        let blocks = blocks(body);
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        assert!(matches!(&blocks[0], Block::Quoted { depth: 1, .. }));
    }

    #[test]
    fn a_blank_line_after_a_quote_ends_it() {
        let body = "> quoted\n\nback to me";
        assert_eq!(
            blocks(body),
            vec![
                Block::Quoted {
                    depth: 1,
                    text: "quoted".to_owned(),
                },
                Block::Prose("back to me".to_owned()),
            ]
        );
    }

    #[test]
    fn writing_below_a_quote_and_above_it_gives_three_blocks() {
        let body = "above\n> quoted\nbelow";
        assert_eq!(blocks(body).len(), 3);
    }

    #[test]
    fn the_signature_is_its_own_block() {
        let body = "Message.\n\n-- \nAda Lovelace\nAnalytical Engines";
        assert_eq!(
            blocks(body),
            vec![
                Block::Prose("Message.".to_owned()),
                Block::Signature("Ada Lovelace\nAnalytical Engines".to_owned()),
            ]
        );
    }

    #[test]
    fn a_quoted_signature_marker_does_not_end_the_message() {
        let body = "Mine.\n> -- \n> Ada\nStill mine.";
        let blocks = blocks(body);
        assert_eq!(blocks.len(), 3, "{blocks:?}");
        assert_eq!(blocks[2], Block::Prose("Still mine.".to_owned()));
    }

    #[test]
    fn a_marker_with_nothing_after_it_adds_no_block() {
        assert_eq!(
            blocks("Message.\n-- \n"),
            vec![Block::Prose("Message.".to_owned())]
        );
    }

    #[test]
    fn an_empty_body_has_no_blocks() {
        assert!(blocks("").is_empty());
        assert!(blocks("\n\n\n").is_empty());
    }

    #[test]
    fn quote_depth_counts_spaced_markers() {
        assert_eq!(quote_depth("> > deep"), 2);
        assert_eq!(quote_depth(">>> deep"), 3);
        assert_eq!(quote_depth("not quoted"), 0);
        assert_eq!(quote_depth("a > b"), 0);
    }

    // --- continuation -----------------------------------------------------

    #[test]
    fn enter_inside_a_quote_continues_the_quote() {
        assert_eq!(continuation("> answering here"), Some("> ".to_owned()));
        assert_eq!(continuation(">> deeper"), Some(">> ".to_owned()));
        assert_eq!(continuation("> > spaced"), Some("> > ".to_owned()));
    }

    #[test]
    fn enter_on_an_empty_quoted_line_leaves_the_quote() {
        assert_eq!(continuation("> "), Some(String::new()));
        assert_eq!(continuation(">"), Some(String::new()));
    }

    #[test]
    fn enter_continues_a_list() {
        assert_eq!(continuation("- milk"), Some("- ".to_owned()));
        assert_eq!(continuation("* milk"), Some("* ".to_owned()));
        assert_eq!(continuation("  - indented"), Some("  - ".to_owned()));
    }

    #[test]
    fn a_numbered_list_counts_on() {
        assert_eq!(continuation("1. first"), Some("2. ".to_owned()));
        assert_eq!(continuation("9. ninth"), Some("10. ".to_owned()));
        assert_eq!(continuation("3) third"), Some("4) ".to_owned()));
    }

    #[test]
    fn a_checked_box_continues_unchecked() {
        assert_eq!(continuation("- [x] done"), Some("- [ ] ".to_owned()));
        assert_eq!(continuation("- [ ] todo"), Some("- [ ] ".to_owned()));
    }

    #[test]
    fn enter_on_an_empty_item_leaves_the_list() {
        assert_eq!(continuation("- "), Some(String::new()));
        assert_eq!(continuation("1. "), Some(String::new()));
        assert_eq!(continuation("- [ ] "), Some(String::new()));
    }

    #[test]
    fn a_list_inside_a_quote_continues_both() {
        assert_eq!(continuation("> - milk"), Some("> - ".to_owned()));
        assert_eq!(continuation("> 1. first"), Some("> 2. ".to_owned()));
    }

    #[test]
    fn ordinary_prose_gets_an_ordinary_line_break() {
        assert_eq!(continuation("just a sentence"), None);
        assert_eq!(continuation(""), None);
        assert_eq!(continuation("1.no space"), None);
        assert_eq!(continuation("-no space"), None);
    }

    // --- wrap -------------------------------------------------------------

    #[test]
    fn short_lines_are_left_alone() {
        let body = "One.\nTwo.\n\nThree.";
        assert_eq!(wrap(body, WRAP_COLUMNS), body);
    }

    #[test]
    fn a_long_paragraph_is_broken_at_spaces() {
        let body = "word ".repeat(40);
        let wrapped = wrap(body.trim_end(), 20);
        for line in wrapped.lines() {
            assert!(line.chars().count() <= 20, "too long: {line:?}");
        }
        assert_eq!(
            wrapped.replace('\n', " "),
            body.trim_end(),
            "wrapping must not lose or add words"
        );
    }

    #[test]
    fn a_word_longer_than_the_line_is_not_broken() {
        let url = "https://example.com/".to_owned() + &"a".repeat(120);
        let wrapped = wrap(&format!("See {url} please"), 40);
        assert!(
            wrapped.lines().any(|line| line.contains(&url)),
            "the url was broken: {wrapped}"
        );
    }

    #[test]
    fn a_wrapped_quote_keeps_its_prefix() {
        let body = format!("> {}", "word ".repeat(30).trim_end());
        let wrapped = wrap(&body, 30);
        assert!(wrapped.lines().count() > 1);
        for line in wrapped.lines() {
            assert!(line.starts_with("> "), "lost the prefix: {line:?}");
            assert!(line.chars().count() <= 30, "too long: {line:?}");
        }
    }

    #[test]
    fn the_signature_survives_exactly() {
        let body = format!("Hi.\n\n-- \nAda\n{}", "long ".repeat(40));
        let wrapped = wrap(&body, 20);
        assert!(
            wrapped.contains("\n-- \nAda\n"),
            "the separator changed: {wrapped:?}"
        );
        assert!(
            wrapped.ends_with(&"long ".repeat(40)),
            "the signature was rewrapped"
        );
    }

    #[test]
    fn a_trailing_newline_is_preserved() {
        assert_eq!(wrap("one\n", WRAP_COLUMNS), "one\n");
        assert_eq!(wrap("one", WRAP_COLUMNS), "one");
    }

    #[test]
    fn wrapping_is_idempotent() {
        let body = "> ".to_owned() + &"word ".repeat(60);
        let once = wrap(body.trim_end(), WRAP_COLUMNS);
        assert_eq!(wrap(&once, WRAP_COLUMNS), once);
    }

    #[test]
    fn blank_lines_are_kept() {
        assert_eq!(wrap("a\n\n\nb", WRAP_COLUMNS), "a\n\n\nb");
    }

    // --- history ----------------------------------------------------------

    fn typed(history: &mut History, text: &str, kind: EditKind) {
        let caret = (0, text.len());
        history.record(text.to_owned(), caret, caret, kind);
    }

    #[test]
    fn a_fresh_history_has_nothing_to_undo() {
        let history = History::new("quoted reply".to_owned());
        assert!(!history.can_undo());
        assert!(!history.can_redo());
    }

    #[test]
    fn a_run_of_typing_is_one_step() {
        let mut history = History::new(String::new());
        for text in ["h", "he", "hel", "hell", "hello"] {
            typed(&mut history, text, EditKind::Typing);
        }
        assert_eq!(history.undo().map(|(text, _)| text), Some(""));
        assert!(!history.can_undo(), "five keystrokes became one step");
    }

    #[test]
    fn a_space_ends_the_run() {
        let mut history = History::new(String::new());
        typed(&mut history, "hello", EditKind::Typing);
        typed(&mut history, "hello ", EditKind::Break);
        typed(&mut history, "hello world", EditKind::Typing);

        assert_eq!(history.undo().map(|(text, _)| text), Some("hello "));
        assert_eq!(history.undo().map(|(text, _)| text), Some("hello"));
        assert_eq!(history.undo().map(|(text, _)| text), Some(""));
        assert!(!history.can_undo());
    }

    #[test]
    fn deleting_does_not_join_typing() {
        let mut history = History::new(String::new());
        typed(&mut history, "hello", EditKind::Typing);
        typed(&mut history, "hell", EditKind::Erase);
        assert_eq!(history.undo().map(|(text, _)| text), Some("hello"));
    }

    #[test]
    fn a_paste_is_always_its_own_step() {
        let mut history = History::new(String::new());
        typed(&mut history, "a", EditKind::Bulk);
        typed(&mut history, "ab", EditKind::Bulk);
        assert_eq!(history.undo().map(|(text, _)| text), Some("a"));
    }

    #[test]
    fn redo_walks_back_up() {
        let mut history = History::new(String::new());
        typed(&mut history, "one", EditKind::Typing);
        typed(&mut history, "one ", EditKind::Break);

        assert_eq!(history.undo().map(|(text, _)| text), Some("one"));
        assert_eq!(history.redo().map(|(text, _)| text), Some("one "));
        assert!(!history.can_redo());
    }

    #[test]
    fn editing_after_an_undo_drops_the_redo_branch() {
        let mut history = History::new(String::new());
        typed(&mut history, "one", EditKind::Typing);
        typed(&mut history, "one ", EditKind::Break);
        history.undo();
        typed(&mut history, "one!", EditKind::Bulk);

        assert!(!history.can_redo());
        assert_eq!(history.undo().map(|(text, _)| text), Some("one"));
    }

    #[test]
    fn undo_returns_the_cursor_to_where_the_edit_started() {
        let mut history = History::new("ab".to_owned());
        history.record("axb".to_owned(), (0, 1), (0, 2), EditKind::Bulk);
        assert_eq!(history.undo(), Some(("ab", (0, 1))));
    }

    #[test]
    fn the_history_is_bounded() {
        let mut history = History::new(String::new());
        for step in 0..History::LIMIT * 2 {
            typed(&mut history, &format!("{step}"), EditKind::Bulk);
        }
        let mut depth = 0;
        while history.undo().is_some() {
            depth += 1;
            assert!(depth <= History::LIMIT, "history grew without bound");
        }
        assert_eq!(depth, History::LIMIT - 1);
    }
}
