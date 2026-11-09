//! One visible command catalog for global bindings, Terminal keys and help.
use crate::presenter::*;
use nsh_core::{keymap::When, prelude::*};
#[derive(Clone, Copy)]
pub struct Command {
    pub id: &'static str,
    pub title: &'static str,
    pub key: &'static str,
    pub display: &'static str,
    pub context: &'static str,
}
pub const CREATE_CARD: Command = Command {
    id: "create-card",
    title: "Create Card",
    key: "mod+enter",
    display: "⌘Enter",
    context: "New Card",
};
pub const SEND_COMMENT: Command = Command {
    id: "send-comment",
    title: "Send comment",
    key: "mod+enter",
    display: "⌘Enter",
    context: "Card comment",
};
pub const CATALOG: &[Command] = &[
    Command {
        id: "new-card",
        title: "New Card",
        key: "mod+n",
        display: "⌘N",
        context: "Global",
    },
    Command {
        id: "quick-open",
        title: "Quick Open",
        key: "mod+o",
        display: "⌘O",
        context: "Global",
    },
    Command {
        id: "shortcuts",
        title: "Show Shortcuts",
        key: "mod+/",
        display: "⌘/",
        context: "Global",
    },
    Command {
        id: "new-terminal",
        title: "New Terminal",
        key: "mod+t",
        display: "⌘T",
        context: "Terminals",
    },
    Command {
        id: "close-terminal",
        title: "Close Terminal",
        key: "mod+w",
        display: "⌘W",
        context: "Terminals",
    },
    Command {
        id: "next-tab",
        title: "Next Terminal tab",
        key: "mod+shift+]",
        display: "⌘⇧]",
        context: "Terminals",
    },
    Command {
        id: "previous-tab",
        title: "Previous Terminal tab",
        key: "mod+shift+[",
        display: "⌘⇧[",
        context: "Terminals",
    },
    CREATE_CARD,
    SEND_COMMENT,
    Command {
        id: "back",
        title: "Back",
        key: "mod+[",
        display: "⌘[",
        context: "Global",
    },
    Command {
        id: "forward",
        title: "Forward",
        key: "mod+]",
        display: "⌘]",
        context: "Global",
    },
];
pub fn execute(ui: Ui, id: &str) {
    crate::utilities::close(ui);
    match id {
        "back" => crate::navigation::go(ui, false),
        "forward" => crate::navigation::go(ui, true),
        "new-card" => ui.new_card(),
        "create-card" if ui.page.get_untracked() == "new-card" => {
            ui.card_submit.update(|n| *n += 1)
        }
        "send-comment" if ui.current_card().is_some() => ui.comment_submit.update(|n| *n += 1),
        "quick-open" => ui.show_search(false),
        "shortcuts" => crate::utilities::open(ui, "shortcuts"),
        "new-terminal" => {
            ui.navigate("terminals");
            crate::terminals::new_tab(ui);
        }
        "close-terminal" if ui.page.get_untracked() == "terminals" => {
            crate::terminals::close_tab(ui, &ui.terminal_selection.get_untracked())
        }
        "next-tab" => crate::terminals::cycle(ui, true),
        "previous-tab" => crate::terminals::cycle(ui, false),
        _ => {}
    }
}
pub fn bindings(ui: Ui) {
    crate::navigation::watch(ui);
    crate::terminals::watch(ui);
    effect(move || {
        if ui.page.get() == "new-card" && !ui.quick_open.get() && ui.utility_popup.get().is_empty()
        {
            on_key_with(
                CREATE_CARD.key,
                KeyOptions {
                    when: When::Always,
                    ..Default::default()
                },
                move |_| {
                    ui.card_submit.update(|n| *n += 1);
                },
            );
        }
    });
    for command in CATALOG.iter().filter(|c| c.context == "Global") {
        on_key_with(
            command.key,
            KeyOptions {
                when: When::Always,
                ..Default::default()
            },
            move |_| execute(ui, command.id),
        );
    }
}
