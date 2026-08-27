// SPDX-License-Identifier: GPL-3.0-only

//! Crash reports: written by a panic hook, surfaced on the next launch.
//!
//! A mail client trusted with people's correspondence cannot fail silently.
//! A panic in a release build otherwise vanishes — the process is usually
//! started by a desktop entry or a `mailto:` activation, so stderr goes
//! nowhere anyone will look. The hook writes what stderr would have said to a
//! file under the state directory, and the next launch says so, once, in the
//! status line.

use std::path::{Path, PathBuf};

/// Where reports live: `$XDG_STATE_HOME/envelope`. State, not data — a crash
/// report is diagnostic residue, not the user's mail.
fn crash_dir() -> Option<PathBuf> {
    dirs::state_dir().map(|d| d.join("envelope"))
}

/// Installs a panic hook that writes a report file, chained to the default
/// hook so stderr still gets the message when there is one to read.
pub fn install_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(dir) = crash_dir() {
            let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
            let report = format!(
                "envelope {} panicked at {stamp}\n\n{info}\n\nbacktrace:\n{}\n",
                env!("CARGO_PKG_VERSION"),
                std::backtrace::Backtrace::force_capture(),
            );
            // Failures here are swallowed deliberately: the process is
            // already dying, and a second panic inside the hook aborts
            // without running the default hook's stderr print.
            let _ = std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(dir.join(format!("crash-{stamp}.log")), report));
        }
        previous(info);
    }));
}

/// Reports written since the last launch that surfaced them, oldest first.
/// Each is renamed `.seen.log` as it is returned, so the notice appears once
/// rather than nagging on every start; the files themselves stay for reading.
pub fn take_unreported() -> Vec<PathBuf> {
    crash_dir().map(|dir| take_unreported_in(&dir)).unwrap_or_default()
}

fn take_unreported_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut fresh: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("crash-") && n.ends_with(".log") && !n.ends_with(".seen.log"))
        })
        .collect();
    fresh.sort();
    fresh
        .into_iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?;
            let seen = p.with_file_name(format!("{}.seen.log", name.trim_end_matches(".log")));
            std::fs::rename(&p, &seen).ok()?;
            Some(seen)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::take_unreported_in;

    #[test]
    fn a_fresh_report_is_returned_once_and_then_counts_as_seen() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("crash-20260827T000000.000Z.log"), "boom").unwrap();

        let first = take_unreported_in(dir.path());
        assert_eq!(first.len(), 1, "the report should surface on the next launch");
        assert!(first[0].to_string_lossy().ends_with(".seen.log"));
        assert!(first[0].exists(), "the file stays for reading, renamed");

        let second = take_unreported_in(dir.path());
        assert!(second.is_empty(), "a surfaced report must not nag again");
    }

    #[test]
    fn an_empty_or_missing_directory_reports_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(take_unreported_in(dir.path()).is_empty());
        assert!(take_unreported_in(&dir.path().join("never-created")).is_empty());
    }

    #[test]
    fn unrelated_files_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "keep").unwrap();
        assert!(take_unreported_in(dir.path()).is_empty());
        assert!(dir.path().join("notes.txt").exists());
    }
}
