// SPDX-License-Identifier: GPL-3.0-only

//! The Accounts context page: point an account at a mail server, and sync.
//!
//! Envelope does not *create* accounts. They are the suite's, held in
//! `$XDG_CONFIG_HOME/cosmic-pim/accounts.toml`, and an account added in Slate
//! is already here with its password. What is missing on one of those is only
//! the mail endpoint — a CalDAV URL says nothing about an IMAP host — so that
//! is the single thing this page asks for.

use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget;
use cosmic_pim_accounts::{Account, Transport};

use crate::app::{MailForm, Message};
use crate::fl;

pub fn view<'a>(
    accounts: &'a [Account],
    form: Option<&'a MailForm>,
    syncing: bool,
    status: Option<&'a str>,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let mut column = widget::column::with_capacity(5).spacing(spacing.space_s);

    if accounts.is_empty() {
        return column
            .push(
                widget::text::body(fl!("no-accounts-description"))
                    .wrapping(cosmic::iced::core::text::Wrapping::Word),
            )
            .into();
    }

    let mut section = widget::settings::section().title(fl!("accounts"));
    for account in accounts {
        let description = match account.mail.as_ref() {
            Some(mail) => format!("{}:{}", mail.imap_host, mail.imap_port),
            None => fl!("no-mail-server"),
        };
        section = section.add(
            widget::settings::item::builder(account.display_name.clone())
                .description(description)
                .control(
                    widget::button::text(if account.mail.is_some() {
                        fl!("change")
                    } else {
                        fl!("set-up-mail")
                    })
                    .on_press(Message::MailFormStart(account.id.clone())),
                ),
        );
    }
    column = column.push(section);

    if let Some(form) = form {
        column = column.push(endpoint_form(form));
    }

    let label = if syncing {
        fl!("syncing")
    } else {
        fl!("sync-now")
    };
    let button = widget::button::text(label);
    // No `on_press` while a pass is in flight: a second concurrent pass would
    // race the first one on the same sidecar files.
    column = column.push(if syncing {
        button
    } else {
        button.on_press(Message::SyncNow)
    });

    if let Some(status) = status {
        column = column.push(
            widget::text::caption(status.to_owned())
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
    }

    column.into()
}

fn endpoint_form(form: &MailForm) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();

    let section = widget::settings::section()
        .title(fl!("mail-server"))
        .add(
            widget::settings::item::builder(fl!("imap-host")).control(
                widget::text_input("imap.example.com", &form.host)
                    .on_input(Message::MailFormHostChanged)
                    .on_focus(Message::TextFocused)
                    .on_unfocus(Message::TextUnfocused)
                    .width(Length::Fixed(220.0)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("port")).control(
                widget::text_input("993", &form.port)
                    .on_input(Message::MailFormPortChanged)
                    .on_focus(Message::TextFocused)
                    .on_unfocus(Message::TextUnfocused)
                    .width(Length::Fixed(220.0)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("encryption")).control(
                widget::dropdown(TRANSPORT_LABELS, Some(form.transport_index()), |index| {
                    Message::MailFormTransportChanged(TRANSPORTS[index])
                })
                .width(Length::Fixed(220.0)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("username"))
                .description(fl!("imap-username-hint"))
                .control(
                    widget::text_input(form.account_username.clone(), &form.username)
                        .on_input(Message::MailFormUsernameChanged)
                        .on_focus(Message::TextFocused)
                        .on_unfocus(Message::TextUnfocused)
                        .width(Length::Fixed(220.0)),
                ),
        );

    let sending = widget::settings::section()
        .title(fl!("sending-section"))
        .add(
            widget::settings::item::builder(fl!("from-address"))
                .description(fl!("from-address-hint"))
                .control(
                    widget::text_input("you@example.com", &form.from_address)
                        .on_input(Message::MailFormFromAddressChanged)
                        .on_focus(Message::TextFocused)
                        .on_unfocus(Message::TextUnfocused)
                        .width(Length::Fixed(220.0)),
                ),
        )
        .add(
            widget::settings::item::builder(fl!("from-name")).control(
                widget::text_input(String::new(), &form.from_name)
                    .on_input(Message::MailFormFromNameChanged)
                    .on_focus(Message::TextFocused)
                    .on_unfocus(Message::TextUnfocused)
                    .width(Length::Fixed(220.0)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("smtp-host"))
                .description(fl!("smtp-host-hint"))
                .control(
                    widget::text_input(form.host.clone(), &form.smtp_host)
                        .on_input(Message::MailFormSmtpHostChanged)
                        .on_focus(Message::TextFocused)
                        .on_unfocus(Message::TextUnfocused)
                        .width(Length::Fixed(220.0)),
                ),
        )
        .add(
            widget::settings::item::builder(fl!("smtp-port")).control(
                widget::text_input("465", &form.smtp_port)
                    .on_input(Message::MailFormSmtpPortChanged)
                    .on_focus(Message::TextFocused)
                    .on_unfocus(Message::TextUnfocused)
                    .width(Length::Fixed(220.0)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("smtp-encryption")).control(
                widget::dropdown(
                    TRANSPORT_LABELS,
                    Some(form.smtp_transport_index()),
                    |index| Message::MailFormSmtpTransportChanged(TRANSPORTS[index]),
                )
                .width(Length::Fixed(220.0)),
            ),
        );

    // Above the fields, because filling them in by hand is the thing it is
    // there to avoid.
    let mut discovery = widget::row::with_capacity(2)
        .align_y(cosmic::iced::Alignment::Center)
        .spacing(spacing.space_xxs);
    discovery = discovery.push(if form.discovering {
        widget::button::text(fl!("finding-settings"))
    } else {
        widget::button::text(fl!("find-settings")).on_press(Message::MailFormDiscover)
    });
    if let Some(found) = &form.discovered_from {
        discovery = discovery.push(
            widget::text::caption(found.clone()).wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
    }

    let mut column = widget::column::with_capacity(5)
        .spacing(spacing.space_s)
        .push(discovery)
        .push(section)
        .push(sending);

    if let Some(error) = &form.error {
        column = column.push(crate::ui::destructive(error.clone()));
    }

    // An incoming server and two valid ports are the minimum that could work.
    // The From address is deliberately not required: an account that can only
    // read mail is a useful account, and demanding a field to save the ones
    // that matter would be the wrong trade.
    let can_save = !form.host.trim().is_empty()
        && form.port.trim().parse::<u16>().is_ok()
        && form.smtp_port.trim().parse::<u16>().is_ok();

    column
        .push(
            widget::row::with_capacity(2)
                .spacing(spacing.space_xs)
                .push(widget::button::text(fl!("cancel")).on_press(Message::MailFormCancel))
                .push({
                    let save =
                        widget::button::text(fl!("save")).class(cosmic::theme::Button::Suggested);
                    if can_save {
                        save.on_press(Message::MailFormSave)
                    } else {
                        save
                    }
                }),
        )
        .into()
}

pub const TRANSPORTS: [Transport; 3] = [Transport::Tls, Transport::StartTls, Transport::Plaintext];
const TRANSPORT_LABELS: &[&str] = &["TLS", "STARTTLS", "None"];
