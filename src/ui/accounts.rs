// SPDX-License-Identifier: GPL-3.0-only

//! The Accounts context page: add an account, point one at a mail server,
//! remove one, and sync. An account GNOME Online Accounts keeps is not
//! removed here — it would be linked again — but in the Accounts window.
//!
//! Accounts are the suite's, held in `$XDG_CONFIG_HOME/cosmic-pim/accounts.toml`,
//! and an account added in Slate is already here with its password. What is
//! missing on one of those is only the mail endpoint — a `CalDAV` URL says
//! nothing about an IMAP host — so the endpoint form asks for that alone.
//!
//! An account added *here* starts from the other end: an address and a
//! password, with the servers worked out from the address — the provider
//! registry, then the built-in table, then autoconfig and a probe — and the
//! browser sign-in for the providers whose route that is. For the domain
//! nothing could find, the add form asks for the servers, and tries them
//! before anything is stored.

use cosmic::iced::Length;
use cosmic::widget;
use cosmic::{Apply as _, Element};
use cosmic_pim_accounts::{Account, MailProtocol, Transport};

use crate::app::{AddForm, MailForm, Message, ServerEdit, ServerFields};
use crate::fl;

pub fn view<'a>(
    accounts: &'a [Account],
    form: Option<&'a MailForm>,
    add: Option<&'a AddForm>,
    sign_in: SignIn<'a>,
    syncing: bool,
    status: Option<&'a str>,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let mut column = widget::column::with_capacity(7).spacing(spacing.space_s);

    if accounts.is_empty() {
        column = column.push(
            widget::text::body(fl!("no-accounts-description"))
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
    } else {
        let mut section = widget::settings::section().title(fl!("accounts"));
        for account in accounts {
            let description = match account.mail.as_ref() {
                Some(mail) => format!("{}:{}", mail.imap_host, mail.imap_port),
                None => fl!("no-mail-server"),
            };
            let controls = widget::row::with_capacity(2)
                .spacing(spacing.space_xxs)
                .align_y(cosmic::iced::Alignment::Center)
                .push(
                    widget::button::text(if account.mail.is_some() {
                        fl!("change")
                    } else {
                        fl!("set-up-mail")
                    })
                    .on_press(Message::MailFormStart(account.id.clone())),
                )
                .push(if account.online_account().is_some() {
                    // Removed here, it would be linked again from GNOME
                    // Online Accounts; the Accounts window is where it goes.
                    widget::button::text(fl!("manage-in-accounts"))
                        .on_press(Message::AccountManageElsewhere(account.id.clone()))
                        .apply(Element::from)
                } else {
                    widget::button::icon(widget::icon::from_name("edit-delete-symbolic"))
                        .on_press(Message::AccountRemove(account.id.clone()))
                        .apply(Element::from)
                });
            section = section.add(
                widget::settings::item::builder(account.display_name.clone())
                    .description(description)
                    .flex_control(controls),
            );
        }
        column = column.push(section);
    }

    // One of the two forms at a time: the add form makes an account, the
    // endpoint form finishes one.
    match (add, form) {
        (Some(add), _) => column = column.push(add_form(add, &sign_in)),
        (None, Some(form)) => column = column.push(endpoint_form(form)),
        (None, None) => {
            column = column
                .push(widget::button::text(fl!("add-account")).on_press(Message::AddFormStart));
        }
    }

    if !accounts.is_empty() {
        let label = if syncing {
            fl!("syncing")
        } else {
            fl!("sync-now")
        };
        let button = widget::button::text(label);
        // No `on_press` while a pass is in flight: a second concurrent pass
        // would race the first one on the same sidecar files.
        column = column.push(if syncing {
            button
        } else {
            button.on_press(Message::SyncNow)
        });
    }

    if let Some(status) = status {
        column = column.push(
            widget::text::caption(status.to_owned())
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
    }

    column.into()
}

/// What the sign-in rows need from the model.
pub struct SignIn<'a> {
    pub providers: &'a [crate::mail::SignInProvider],
    pub in_flight: bool,
}

/// The add-account form: a name, an address, a password — and a sign-in
/// button per provider whose route is the browser instead.
///
/// The password field steps aside when the address belongs to a provider
/// whose sign-in is configured: Google issues no password a client could
/// use, and a field for one would be a field that cannot be filled in right.
/// The sign-in rows are absent entirely when no provider has a client id — a
/// button that ends at the provider's "invalid client" page is worse than
/// none.
fn add_form<'a>(add: &'a AddForm, sign_in: &SignIn<'a>) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let wants_sign_in = add.wants_sign_in();

    let mut section = widget::settings::section()
        .title(fl!("add-account-title"))
        .add(
            widget::settings::item::builder(fl!("add-account-name")).flex_control(
                widget::text_input(String::new(), &add.name)
                    .on_input(Message::AddFormNameChanged)
                    .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("add-account-address")).flex_control(
                widget::text_input("you@example.com", &add.email)
                    .on_input(Message::AddFormEmailChanged)
                    .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
            ),
        );

    if wants_sign_in {
        let provider = add
            .provider
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_default();
        section = section.add(
            widget::text::caption(fl!("provider-uses-sign-in", provider = provider))
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
    } else {
        // The provider's own note when it has one — "make an app password
        // first" — and the general one otherwise.
        let hint = match add.provider.as_ref() {
            Some(provider) if provider.uses_sign_in => {
                fl!(
                    "provider-sign-in-unavailable",
                    provider = provider.name.clone()
                )
            }
            Some(provider) => provider
                .hint
                .clone()
                .unwrap_or_else(|| fl!("app-password-hint")),
            None => fl!("app-password-hint"),
        };
        section = section.add(
            widget::settings::item::builder(fl!("password"))
                .description(hint)
                .flex_control(
                    widget::secure_input(fl!("password"), &add.password, None, true)
                        .on_input(Message::AddFormPasswordChanged)
                        .on_submit(|_| Message::AddFormConfirm)
                        .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
                ),
        );
    }

    let has_address = add.email.trim().contains('@');
    for provider in sign_in.providers {
        let button = widget::button::text(if sign_in.in_flight {
            fl!("sign-in-waiting")
        } else {
            fl!("sign-in-with", provider = provider.name.clone())
        });
        section = section.add(
            widget::settings::item::builder(provider.name.clone()).flex_control(
                button.on_press_maybe(
                    (!sign_in.in_flight && has_address)
                        .then(|| Message::SignInStarted(provider.id.clone())),
                ),
            ),
        );
    }

    let mut column = widget::column::with_capacity(6)
        .spacing(spacing.space_s)
        .push(section);

    // The second step, for an address nothing could find servers for: the
    // person names them, and both are signed in to before anything is saved.
    if let Some(servers) = &add.servers {
        column = column
            .push(
                widget::text::body(fl!(
                    "add-servers-needed",
                    domain = add.domain().unwrap_or_default()
                ))
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
            )
            .push(incoming(
                widget::settings::section().title(fl!("mail-server")),
                servers,
                add.email.trim(),
                Message::AddFormServer,
            ))
            .push(outgoing(
                widget::settings::section().title(fl!("sending-section")),
                servers,
                Message::AddFormServer,
            ));
    }

    if let Some(error) = &add.error {
        column = column.push(crate::ui::destructive(error.clone()));
    }

    let mut buttons = widget::row::with_capacity(2)
        .spacing(spacing.space_xs)
        .push(widget::button::text(fl!("cancel")).on_press(Message::AddFormCancel));
    if !wants_sign_in {
        let label = match (add.adding, add.servers.is_some()) {
            (false, _) => fl!("add"),
            (true, false) => fl!("adding"),
            (true, true) => fl!("adding-at-servers"),
        };
        buttons = buttons.push(
            widget::button::text(label)
                .class(cosmic::theme::Button::Suggested)
                .on_press_maybe(add.can_add().then_some(Message::AddFormConfirm)),
        );
    }

    column.push(buttons).into()
}

fn endpoint_form(form: &MailForm) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();

    let section = incoming(
        widget::settings::section().title(fl!("mail-server")),
        &form.servers,
        &form.account_username,
        Message::MailFormServer,
    );
    // Only for an account that has one: a browser sign-in is renewed by
    // signing in again, not by typing anything here.
    let section = if form.uses_password {
        section.add(
            widget::settings::item::builder(fl!("password"))
                .description(fl!("new-password-hint"))
                .flex_control(
                    widget::secure_input(fl!("password"), &form.password, None, true)
                        .on_input(Message::MailFormPasswordChanged)
                        .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
                ),
        )
    } else {
        section
    };

    let sending = widget::settings::section()
        .title(fl!("sending-section"))
        .add(
            widget::settings::item::builder(fl!("from-address"))
                .description(fl!("from-address-hint"))
                .flex_control(
                    widget::text_input("you@example.com", &form.from_address)
                        .on_input(Message::MailFormFromAddressChanged)
                        .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
                ),
        )
        .add(
            widget::settings::item::builder(fl!("from-name")).flex_control(
                widget::text_input(String::new(), &form.from_name)
                    .on_input(Message::MailFormFromNameChanged)
                    .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
            ),
        )
        .add(aliases(form));
    let sending = outgoing(sending, &form.servers, Message::MailFormServer);

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

    // The From address is deliberately not required: an account that can only
    // read mail is a useful account, and demanding a field to save the ones
    // that matter would be the wrong trade.
    let can_save = form.servers.is_complete();

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

/// The incoming-server rows: protocol, server, port, encryption, JMAP
/// session and login. `login` is the login the account already has, shown
/// where the field is empty because empty means "that one".
fn incoming<'a>(
    section: widget::settings::Section<'a, Message>,
    fields: &'a ServerFields,
    login: &str,
    on_edit: fn(ServerEdit) -> Message,
) -> widget::settings::Section<'a, Message> {
    section
        .add(
            widget::settings::item::builder(fl!("protocol"))
                .description(fl!("protocol-hint"))
                .flex_control(
                    widget::dropdown(
                        PROTOCOL_LABELS,
                        Some(fields.protocol_index()),
                        move |index| on_edit(ServerEdit::Protocol(PROTOCOLS[index])),
                    )
                    .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
                ),
        )
        .add(
            widget::settings::item::builder(fl!("imap-host")).flex_control(
                widget::text_input("imap.example.com", &fields.host)
                    .on_input(move |host| on_edit(ServerEdit::Host(host)))
                    .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("port")).flex_control(
                widget::text_input("993", &fields.port)
                    .on_input(move |port| on_edit(ServerEdit::Port(port)))
                    .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("encryption")).flex_control(
                widget::dropdown(
                    TRANSPORT_LABELS,
                    Some(fields.transport_index()),
                    move |index| on_edit(ServerEdit::Transport(TRANSPORTS[index])),
                )
                .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("jmap-url"))
                .description(fl!("jmap-url-hint"))
                .flex_control(
                    widget::text_input("https://…/.well-known/jmap", &fields.jmap_url)
                        .on_input(move |url| on_edit(ServerEdit::JmapUrl(url)))
                        .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
                ),
        )
        .add(
            widget::settings::item::builder(fl!("username"))
                .description(fl!("imap-username-hint"))
                .flex_control(
                    widget::text_input(login.to_owned(), &fields.username)
                        .on_input(move |username| on_edit(ServerEdit::Username(username)))
                        .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
                ),
        )
}

/// The outgoing-server rows: server, port and encryption.
fn outgoing<'a>(
    section: widget::settings::Section<'a, Message>,
    fields: &'a ServerFields,
    on_edit: fn(ServerEdit) -> Message,
) -> widget::settings::Section<'a, Message> {
    section
        .add(
            widget::settings::item::builder(fl!("smtp-host"))
                .description(fl!("smtp-host-hint"))
                .flex_control(
                    widget::text_input(fields.host.clone(), &fields.smtp_host)
                        .on_input(move |host| on_edit(ServerEdit::SmtpHost(host)))
                        .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
                ),
        )
        .add(
            widget::settings::item::builder(fl!("smtp-port")).flex_control(
                widget::text_input("465", &fields.smtp_port)
                    .on_input(move |port| on_edit(ServerEdit::SmtpPort(port)))
                    .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("smtp-encryption")).flex_control(
                widget::dropdown(
                    TRANSPORT_LABELS,
                    Some(fields.smtp_transport_index()),
                    move |index| on_edit(ServerEdit::SmtpTransport(TRANSPORTS[index])),
                )
                .width(Length::Fixed(crate::ui::CONTROL_WIDTH)),
            ),
        )
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
        .flex_control(column)
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
