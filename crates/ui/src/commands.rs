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
];
pub fn execute(ui: Ui, id: &str) {
    crate::utilities::close(ui);
    match id {
        "new-card" => ui.new_card(String::new()),
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
