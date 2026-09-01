# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

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
