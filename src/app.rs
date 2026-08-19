// SPDX-License-Identifier: GPL-3.0-only

//! The Envelope application shell.

use cosmic::app::{Core, Task, context_drawer};
use cosmic::iced::Length;
use cosmic::widget::{self, about::About, menu};
use cosmic::{Apply as _, Element};
use cosmic_pim_accounts::AccountStore;

use crate::fl;

const APP_ID: &str = "io.github.entro314labs.Envelope";
const REPOSITORY: &str = "https://github.com/entro314-labs/envelope";

pub struct AppModel {
    core: Core,
    about: About,
    context_page: ContextPage,
    key_binds: std::collections::HashMap<menu::KeyBind, MenuAction>,
    /// Read from the shared account store, so an account added in Slate is
    /// already here. Nothing is fetched with it yet.
    accounts: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum Message {
    LaunchUrl(String),
    ToggleContextPage(ContextPage),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    About,
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::About => Message::ToggleContextPage(ContextPage::About),
        }
    }
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let about = About::default()
            .name(fl!("app-title"))
            .version(env!("CARGO_PKG_VERSION"))
            .license("GPL-3.0-only")
            .links([("Repository", REPOSITORY)]);

        // The account store is shared across the suite; an account added in
        // Slate shows up here without being re-entered.
        let accounts = AccountStore::open_default()
            .map(|store| {
                store
                    .accounts()
                    .iter()
                    .map(|a| format!("{} · {}", a.display_name, a.username))
                    .collect()
            })
            .unwrap_or_default();

        (
            Self {
                core,
                about,
                context_page: ContextPage::default(),
                key_binds: std::collections::HashMap::new(),
                accounts,
            },
            Task::none(),
        )
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        vec![
            menu::bar(vec![menu::Tree::with_children(
                menu::root(fl!("about")).apply(Element::from),
                menu::items(
                    &self.key_binds,
                    vec![menu::Item::Button(fl!("about"), None, MenuAction::About)],
                ),
            )])
            .into(),
        ]
    }

    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Self::Message>> {
        if !self.core.window.show_context {
            return None;
        }
        Some(match self.context_page {
            ContextPage::About => context_drawer::about(
                &self.about,
                |url| Message::LaunchUrl(url.to_string()),
                Message::ToggleContextPage(ContextPage::About),
            ),
        })
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let spacing = cosmic::theme::spacing();

        // Deliberately not a fake empty inbox: an empty message list would read
        // as "your mail failed to load" rather than "this is not built yet".
        let mut column = widget::column::with_capacity(4)
            .spacing(spacing.space_s)
            .padding(spacing.space_m)
            .push(widget::text::title2(fl!("no-mail-engine")))
            .push(
                widget::text::body(fl!("no-mail-engine-description"))
                    .wrapping(cosmic::iced::core::text::Wrapping::Word),
            );

        if self.accounts.is_empty() {
            column = column.push(widget::text::caption(fl!("no-accounts")));
        } else {
            let mut section = widget::settings::section().title(fl!("accounts"));
            for account in &self.accounts {
                section = section.add(
                    widget::settings::item::builder(account.clone())
                        .control(widget::text::caption(String::new())),
                );
            }
            column = column.push(section);
        }

        widget::container(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::LaunchUrl(url) => {
                if let Err(why) = open::that_detached(&url) {
                    tracing::warn!(url, %why, "could not open the link");
                }
            }
            Message::ToggleContextPage(page) => {
                if self.context_page == page {
                    self.core.window.show_context = !self.core.window.show_context;
                } else {
                    self.context_page = page;
                    self.core.window.show_context = true;
                }
            }
        }
        Task::none()
    }
}
