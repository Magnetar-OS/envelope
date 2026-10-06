// SPDX-License-Identifier: GPL-3.0-only

//! Handing off to another application of the desktop.
//!
//! Each is a program started by name, and none of them is a dependency of
//! Envelope's package: it may not be installed. A start that fails is the
//! caller's to answer — it has something else to offer, or something to say.

use std::process::{Command, Stdio};

/// The desktop's Accounts window. See `magnetar-accounts`.
pub const ACCOUNTS_WINDOW: &str = "magnetar-accounts";

/// The suite's contacts application.
pub const CONTACTS_APP: &str = "circle";

/// The command that opens the Accounts window on its add page, saying that
/// it is mail an account is wanted for.
#[must_use]
pub fn add_account(program: &str) -> Command {
    let mut command = Command::new(program);
    command.arg("--for=mail");
    command
}

/// The command that shows who has `address` in Circle: their card when they
/// have one, a search for the address when they do not. Circle is
/// single-instance, so one already open shows it.
///
/// The address comes out of a message, so it is whatever its sender wrote.
/// It travels as the value of one argument and is never read as an option
/// of its own.
#[must_use]
pub fn show_person(program: &str, address: &str) -> Command {
    let mut command = Command::new(program);
    command.arg(format!("--search={address}"));
    command
}

/// Starts `command` and leaves it running.
///
/// # Errors
///
/// The program could not be started: most often, it is not installed.
pub fn start(mut command: Command) -> std::io::Result<()> {
    let mut child = command.stdin(Stdio::null()).spawn()?;
    // Waited for, however long it runs: a child nobody waits for stays in the
    // process table after it exits, for as long as Envelope does. How it
    // ended is its own business.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_accounts_window_is_asked_for_its_add_page_for_mail() {
        let command = add_account(ACCOUNTS_WINDOW);

        assert_eq!(command.get_program(), "magnetar-accounts");
        assert_eq!(arguments(&command), ["--for=mail"]);
    }

    #[test]
    fn an_address_is_one_argument_whatever_it_holds() {
        // A sender chooses their own address. None of these may reach Circle
        // as a second argument or as an option.
        for address in [
            "ada@analytical.example",
            "--new-contact",
            "ada@analytical.example --new-contact",
            "",
        ] {
            let command = show_person(CONTACTS_APP, address);

            assert_eq!(command.get_program(), "circle");
            assert_eq!(arguments(&command), [format!("--search={address}")]);
        }
    }

    #[test]
    fn a_program_that_is_not_installed_is_an_error_to_answer() {
        let missing = start(add_account("/nonexistent/magnetar-accounts"));

        assert_eq!(
            missing.expect_err("started").kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn a_program_that_is_installed_is_started() {
        // `true` takes any arguments and exits at once.
        start(add_account("true")).expect("started");
    }
}
