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
use cosmic_pim_accounts::{Account, MailProtocol, Transport};

use crate::app::{MailForm, Message};
use crate::fl;

pub fn view<'a>(
    accounts: &'a [Account],
    form: Option<&'a MailForm>,
    sign_in: SignIn<'a>,
    syncing: bool,
    status: Option<&'a str>,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let mut column = widget::column::with_capacity(6).spacing(spacing.space_s);

    if accounts.is_empty() {
        column = column.push(
            widget::text::body(fl!("no-accounts-description"))
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
        column = column.push(sign_in_section(&sign_in));
        return column.into();
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

    column = column.push(sign_in_section(&sign_in));

    if let Some(status) = status {
        column = column.push(
            widget::text::caption(status.to_owned())
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
    }

    column.into()
}

/// What the sign-in section needs from the model.
pub struct SignIn<'a> {
    pub providers: &'a [crate::mail::SignInProvider],
    pub email: &'a str,
    pub in_flight: bool,
}

/// One row per provider a browser sign-in can reach.
///
/// Absent entirely when no provider has a client id configured: a section
/// whose every button fails at the provider's "invalid client" page is worse
/// than no section.
fn sign_in_section<'a>(sign_in: &SignIn<'a>) -> Element<'a, Message> {
    if sign_in.providers.is_empty() {
        return widget::Space::new().height(Length::Fixed(0.0)).into();
    }

    let mut section = widget::settings::section().title(fl!("sign-in")).add(
        widget::settings::item::builder(fl!("sign-in-address")).control(
            widget::text_input("you@example.com", sign_in.email)
                .on_input(Message::SignInEmailChanged)
                .on_focus(Message::TextFocused)
                .on_unfocus(Message::TextUnfocused)
                .width(Length::Fixed(220.0)),
        ),
    );

    for provider in sign_in.providers {
        let button = widget::button::text(if sign_in.in_flight {
            fl!("sign-in-waiting")
        } else {
            fl!("sign-in-with", provider = provider.name.clone())
        });
        section = section.add(
            widget::settings::item::builder(provider.name.clone()).control(if sign_in.in_flight {
                button
            } else {
                button.on_press(Message::SignInStarted(provider.id.clone()))
            }),
        );
    }

    section.into()
}

fn endpoint_form(form: &MailForm) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();

    let section = widget::settings::section()
        .title(fl!("mail-server"))
        .add(
            widget::settings::item::builder(fl!("protocol"))
                .description(fl!("protocol-hint"))
                .control(
                    widget::dropdown(PROTOCOL_LABELS, Some(form.protocol_index()), |index| {
                        Message::MailFormProtocolChanged(PROTOCOLS[index])
                    })
                    .width(Length::Fixed(220.0)),
                ),
        )
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
            widget::settings::item::builder(fl!("jmap-url"))
                .description(fl!("jmap-url-hint"))
                .control(
                    widget::text_input("https://…/.well-known/jmap", &form.jmap_url)
                        .on_input(Message::MailFormJmapUrlChanged)
                        .on_focus(Message::TextFocused)
                        .on_unfocus(Message::TextUnfocused)
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
        .add(aliases(form))
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

/// The send-as aliases: what is configured, and a field to add one.
fn aliases(form: &MailForm) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let mut column = widget::column::with_capacity(form.aliases.len() + 2)
        .spacing(spacing.space_xxs)
        .push(
            widget::row::with_capacity(2)
                .align_y(cosmic::iced::Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(
                    widget::text_input(fl!("alias-placeholder"), &form.alias_input)
                        .on_input(Message::MailFormAliasInputChanged)
                        .on_submit(|_| Message::MailFormAliasAdded)
                        .on_focus(Message::TextFocused)
                        .on_unfocus(Message::TextUnfocused)
                        .width(Length::Fill),
                )
                .push(widget::button::standard(fl!("alias-add")).on_press_maybe(
                    (!form.alias_input.trim().is_empty()).then_some(Message::MailFormAliasAdded),
                )),
        );
    for (index, alias) in form.aliases.iter().enumerate() {
        let label = if alias.name.trim().is_empty() {
            alias.address.clone()
        } else {
            format!("{} <{}>", alias.name, alias.address)
        };
        column = column.push(
            widget::row::with_capacity(2)
                .align_y(cosmic::iced::Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(widget::text::caption(label).width(Length::Fill))
                .push(
                    widget::button::icon(widget::icon::from_name("edit-delete-symbolic"))
                        .on_press(Message::MailFormAliasRemoved(index)),
                ),
        );
    }
    widget::settings::item::builder(fl!("aliases"))
        .description(fl!("aliases-hint"))
        .control(column)
        .into()
}

pub const TRANSPORTS: [Transport; 3] = [Transport::Tls, Transport::StartTls, Transport::Plaintext];
const TRANSPORT_LABELS: &[&str] = &["TLS", "STARTTLS", "None"];

/// The protocols a password can drive.
///
/// Gmail and Graph are real engines in the substrate but need an OAuth token,
/// and a dropdown entry that can only ever fail at sync time with "wrong
/// password" is worse than its absence. They join the list with the token
/// flow.
pub const PROTOCOLS: [MailProtocol; 3] =
    [MailProtocol::Imap, MailProtocol::Jmap, MailProtocol::Pop3];
const PROTOCOL_LABELS: &[&str] = &["IMAP", "JMAP", "POP3"];
