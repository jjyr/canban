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
    pub keywords: &'static str,
}
pub const CREATE_CARD: Command = Command {
    id: "create-card",
    title: "Create Card",
    key: "mod+enter",
    display: "⌘Enter",
    context: "New Card",
    keywords: "",
};
pub const SEND_COMMENT: Command = Command {
    id: "send-comment",
    title: "Send comment",
    key: "mod+enter",
    display: "⌘Enter",
    context: "Card comment",
    keywords: "",
};
pub const CATALOG: &[Command] = &[
    Command {
        id: "new-card",
        title: "New Card",
        key: "mod+n",
        display: "⌘N",
        context: "Global",
        keywords: "",
    },
    Command {
        id: "quick-open",
        title: "Quick Open",
        key: "mod+o",
        display: "⌘O",
        context: "Global",
        keywords: "search jump",
    },
    Command {
        id: "shortcuts",
        title: "Show Shortcuts",
        key: "mod+/",
        display: "⌘/",
        context: "Global",
        keywords: "commands palette /",
    },
    Command {
        id: "new-terminal",
        title: "New Terminal",
        key: "mod+t",
        display: "⌘T",
        context: "Terminals",
        keywords: "shell",
    },
    Command {
        id: "close-terminal",
        title: "Close Terminal",
        key: "mod+w",
        display: "⌘W",
        context: "Terminals",
        keywords: "",
    },
    Command {
        id: "next-tab",
        title: "Next Terminal tab",
        key: "mod+shift+]",
        display: "⌘⇧]",
        context: "Terminals",
        keywords: "",
    },
    Command {
        id: "previous-tab",
        title: "Previous Terminal tab",
        key: "mod+shift+[",
        display: "⌘⇧[",
        context: "Terminals",
        keywords: "",
    },
    CREATE_CARD,
    SEND_COMMENT,
    Command {
        id: "save-card",
        title: "Save Card changes",
        key: "mod+enter",
        display: "⌘Enter",
        context: "Card editor",
        keywords: "",
    },
    Command {
        id: "back",
        title: "Back",
        key: "mod+[",
        display: "⌘[",
        context: "Global",
        keywords: "",
    },
    Command {
        id: "forward",
        title: "Forward",
        key: "mod+]",
        display: "⌘]",
        context: "Global",
        keywords: "",
    },
    Command {
        id: "go-board",
        title: "Go to Board",
        key: "",
        display: "Board",
        context: "Navigation",
        keywords: "kanban cards",
    },
    Command {
        id: "go-overview",
        title: "Go to Overview",
        key: "",
        display: "Overview",
        context: "Navigation",
        keywords: "activity",
    },
    Command {
        id: "go-terminals",
        title: "Go to Terminals",
        key: "",
        display: "Terminals",
        context: "Navigation",
        keywords: "shell cli",
    },
    Command {
        id: "go-review",
        title: "Go to Diff & Review",
        key: "",
        display: "Review",
        context: "Navigation",
        keywords: "git compare patch",
    },
    Command {
        id: "go-github",
        title: "Go to GitHub",
        key: "",
        display: "GitHub",
        context: "Navigation",
        keywords: "issues pull requests pr",
    },
    Command {
        id: "go-workflows",
        title: "Open Project Workflows",
        key: "",
        display: "Project",
        context: "Navigation",
        keywords: "studio stages yaml",
    },
    Command {
        id: "go-project-agents",
        title: "Configure Project Agents",
        key: "",
        display: "Project",
        context: "Preferences",
        keywords: "project-agents setup agent identity override roles",
    },
    Command {
        id: "go-agent-setup",
        title: "Set up Agents",
        key: "",
        display: "Agents",
        context: "Preferences",
        keywords: "agent-setup setup agent onboard enable disable herdr identities default",
    },
    Command {
        id: "go-drafts",
        title: "Open Drafts",
        key: "",
        display: "Workspace",
        context: "Navigation",
        keywords: "",
    },
    Command {
        id: "go-archive",
        title: "Open Project Archive",
        key: "",
        display: "Project",
        context: "Navigation",
        keywords: "",
    },
    Command {
        id: "go-settings",
        title: "Open Settings",
        key: "",
        display: "App",
        context: "Navigation",
        keywords: "preferences config agents identities",
    },
    Command {
        id: "choose-theme",
        title: "Choose Theme",
        key: "",
        display: "Appearance",
        context: "Preferences",
        keywords: "dark light color",
    },
    Command {
        id: "switch-project",
        title: "Switch Project",
        key: "",
        display: "Workspace",
        context: "Navigation",
        keywords: "choose project",
    },
];
pub fn execute(ui: Ui, id: &str) {
    crate::utilities::close(ui);
    ui.quick_open.set(false);
    match id {
        "back" => crate::navigation::go(ui, false),
        "forward" => crate::navigation::go(ui, true),
        "new-card" => ui.new_card(),
        "create-card" if ui.page.get_untracked() == "new-card" => {
            ui.card_submit.update(|n| *n += 1)
        }
        "save-card" => ui.task_submit.update(|n| *n += 1),
        "send-comment" if ui.current_card().is_some() => ui.comment_submit.update(|n| *n += 1),
        "quick-open" => ui.show_search(false),
        "shortcuts" => {
            ui.utility_popup.set(String::new());
            ui.project_picker.set(false);
            ui.search_query.set("/".into());
            ui.quick_open.set(true);
        }
        "go-board" => ui.navigate("board"),
        "go-overview" => ui.navigate("overview"),
        "go-terminals" => ui.navigate("terminals"),
        "go-review" => crate::review::open_compare(ui),
        "go-github" => ui.navigate("github"),
        "go-workflows" => ui.navigate("workflows"),
        "go-project-agents" => {
            if ui.current_project().is_some() {
                ui.navigate("project-agents");
            } else {
                ui.navigate("agent-setup");
            }
        }
        "go-agent-setup" => ui.navigate("agent-setup"),
        "go-drafts" => ui.navigate("drafts"),
        "go-archive" => ui.navigate("archive"),
        "go-settings" => ui.navigate("settings"),
        "choose-theme" => crate::utilities::open(ui, "theme"),
        "switch-project" => ui.show_search(true),
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
    for command in CATALOG
        .iter()
        .filter(|c| c.context == "Global" && !c.key.is_empty())
    {
        on_key_with(
            command.key,
            KeyOptions {
                when: When::Always,
                ..Default::default()
            },
            move |_| execute(ui, command.id),
        );
    }
    // Plain `/` opens the command palette whenever focus is not in a text input or hosted terminal.
    on_key_with(
        "/",
        KeyOptions {
            when: When::NotEditing,
            ..Default::default()
        },
        move |_| {
            if !ui.quick_open.get_untracked()
                && ui.utility_popup.get_untracked().is_empty()
                && !ui.native_modal.get_untracked()
            {
                ui.project_picker.set(false);
                ui.search_query.set("/".into());
                ui.quick_open.set(true);
            }
        },
    );
}
