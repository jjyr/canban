//! One visible command catalog for global bindings, Terminal keys and Command Palette (`⌘/`, `/`, `>…`).
//!
//! Every user-executable UI operation across global chrome, Activity & Notifications,
//! Board, Card Workspace, Git, Terminals, Managed Worktrees, GitHub, and
//! Settings is registered here and dispatches the same underlying intent as UI controls.
use crate::presenter::*;
use nsh_core::{keymap::When, prelude::*};
use serde_json::json;

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
    keywords: "submit",
};

pub const SEND_COMMENT: Command = Command {
    id: "send-comment",
    title: "Send comment",
    key: "mod+enter",
    display: "⌘Enter",
    context: "Card comment",
    keywords: "reply",
};

pub const CATALOG: &[Command] = &[
    Command {
        id: "toggle-notifications",
        title: "Toggle Activity & Notifications",
        key: "mod+i",
        display: "⌘I",
        context: "Global",
        keywords: "overview alerts unread inbox running status slots",
    },
    Command {
        id: "new-card",
        title: "New Card",
        key: "mod+n",
        display: "⌘N",
        context: "Global",
        keywords: "task create",
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
        title: "Commands & Shortcuts",
        key: "mod+/",
        display: "⌘/",
        context: "Global",
        keywords: "commands palette >",
    },
    Command {
        id: "toggle-card-terminal",
        title: "Toggle Card Live Terminal",
        key: "mod+j",
        display: "⌘J",
        context: "Card",
        keywords: "split cli side-by-side discuss conversation",
    },
    Command {
        id: "new-terminal",
        title: "New Terminal",
        key: "mod+t",
        display: "⌘T",
        context: "Global",
        keywords: "shell pty standalone",
    },
    Command {
        id: "new-worktree",
        title: "New Managed Branch Worktree…",
        key: "",
        display: "Terminals",
        context: "Worktrees",
        keywords: "git branch checkout worktree spike standalone",
    },
    Command {
        id: "close-terminal",
        title: "Close Terminal",
        key: "mod+w",
        display: "⌘W",
        context: "Terminals",
        keywords: "tab",
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
        keywords: "history",
    },
    Command {
        id: "forward",
        title: "Forward",
        key: "mod+]",
        display: "⌘]",
        context: "Global",
        keywords: "history",
    },
    Command {
        id: "mark-all-read",
        title: "Mark All Updates as Read",
        key: "",
        display: "Notifications",
        context: "Notifications",
        keywords: "notifications clear unread overview",
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
        id: "clear-board-filters",
        title: "Clear Board Filters",
        key: "",
        display: "Board",
        context: "Board",
        keywords: "tag filter reset show all",
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
        title: "Go to Git",
        key: "",
        display: "Git",
        context: "Navigation",
        keywords: "diff review compare patch branch commit",
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
        id: "edit-workflow-agent",
        title: "Edit Workflows with Default Agent",
        key: "",
        display: "Workflows",
        context: "Project",
        keywords: "edit with agent configuration assistant yaml terminal",
    },
    Command {
        id: "go-agent-setup",
        title: "Agents · Setup & Enabled Providers",
        key: "",
        display: "Agents",
        context: "Preferences",
        keywords: "agent-setup setup agent onboard enable disable herdr identities default",
    },
    Command {
        id: "go-project-agents",
        title: "Agents · Project Identities & Overrides",
        key: "",
        display: "Project",
        context: "Preferences",
        keywords: "project-agents setup agent identity override roles model effort",
    },
    Command {
        id: "go-settings-agents",
        title: "Agents · Identities & Custom Commands",
        key: "",
        display: "Settings",
        context: "Preferences",
        keywords: "agents identities command argv custom cli model effort defaults",
    },
    Command {
        id: "go-drafts",
        title: "Open Drafts",
        key: "",
        display: "Workspace",
        context: "Navigation",
        keywords: "unsent",
    },
    Command {
        id: "go-archive",
        title: "Open Project Archive",
        key: "",
        display: "Project",
        context: "Navigation",
        keywords: "archived closed",
    },
    Command {
        id: "go-settings",
        title: "Open Settings",
        key: "",
        display: "App",
        context: "Navigation",
        keywords: "preferences config identities",
    },
    Command {
        id: "go-acknowledgements",
        title: "Open Acknowledgements & Licenses",
        key: "",
        display: "Settings",
        context: "Preferences",
        keywords: "about credits open source licenses apache mit herdr sheprd herdr-gpui nsh ghostty",
    },
    Command {
        id: "go-machines",
        title: "Open Execution Machines",
        key: "",
        display: "Machines",
        context: "Navigation",
        keywords: "remote ssh host core",
    },
    Command {
        id: "go-project-location",
        title: "Configure Project Work Location",
        key: "",
        display: "Project",
        context: "Navigation",
        keywords: "directory folder repository path",
    },
    Command {
        id: "go-github-repo",
        title: "Configure Project GitHub Repository",
        key: "",
        display: "GitHub",
        context: "Navigation",
        keywords: "sync repo owner",
    },
    Command {
        id: "go-github-account",
        title: "GitHub Account & Access",
        key: "",
        display: "GitHub",
        context: "Preferences",
        keywords: "oauth login token",
    },
    Command {
        id: "new-project",
        title: "Create New Project…",
        key: "",
        display: "Workspace",
        context: "Navigation",
        keywords: "add repository",
    },
    Command {
        id: "switch-project",
        title: "Switch Project",
        key: "",
        display: "Workspace",
        context: "Navigation",
        keywords: "choose project",
    },
    Command {
        id: "popout-card-terminal",
        title: "Pop Out Card Terminal to Terminals",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "full screen expand pty",
    },
    Command {
        id: "open-card-diff",
        title: "Open Card Diff in Git",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "git review compare patch changes",
    },
    Command {
        id: "quick-merge-card",
        title: "Quick Merge Current Card",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "accept deliver merge branch",
    },
    Command {
        id: "retry-auto-merge",
        title: "Retry Auto-Merge",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "deliver queue merge conflict auto-merge merger",
    },
    Command {
        id: "edit-card",
        title: "Edit Card Description",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "task body title",
    },
    Command {
        id: "edit-last-comment",
        title: "Edit Latest Comment",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "modify update message comment discussion github",
    },
    Command {
        id: "delete-last-comment",
        title: "Delete Latest Comment…",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "remove erase trash message comment discussion github",
    },
    Command {
        id: "open-last-github-comment",
        title: "Open Latest GitHub Comment in Browser",
        key: "",
        display: "GitHub",
        context: "Navigation",
        keywords: "jump link external web browser anchor comment discussion pr issue",
    },
    Command {
        id: "card-workspace-details",
        title: "Toggle Card Workspace Details",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "worktree branch directory",
    },
    Command {
        id: "card-attachments",
        title: "Toggle Card Attachments",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "files images upload",
    },
    Command {
        id: "archive-card",
        title: "Archive or Restore Current Card",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "close reopen lifecycle",
    },
    Command {
        id: "toggle-terminal-sidebar",
        title: "Toggle Terminal Sidebar",
        key: "",
        display: "Terminals",
        context: "Terminals",
        keywords: "tree panel hide show",
    },
    Command {
        id: "rename-terminal",
        title: "Rename Current Terminal Tab…",
        key: "",
        display: "Terminals",
        context: "Terminals",
        keywords: "label title",
    },
    Command {
        id: "interrupt-terminal",
        title: "Interrupt Active Terminal",
        key: "",
        display: "Terminals",
        context: "Terminals",
        keywords: "stop sigint ctrl-c",
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
        id: "choose-terminal-font",
        title: "Choose Terminal Font…",
        key: "",
        display: "Appearance",
        context: "Preferences",
        keywords: "monospace family size",
    },
    Command {
        id: "toggle-awake",
        title: "Toggle Keep Host Awake",
        key: "",
        display: "System",
        context: "Preferences",
        keywords: "coffee sleep caffeinate power",
    },
    Command {
        id: "refresh-usage",
        title: "Refresh Subscription Usage",
        key: "",
        display: "System",
        context: "Preferences",
        keywords: "quota codex claude limits",
    },
    Command {
        id: "reload-config",
        title: "Reload Configuration",
        key: "",
        display: "System",
        context: "Preferences",
        keywords: "profiles json refresh",
    },
    Command {
        id: "copy-skill-command",
        title: "Copy Canban CLI Skill Install Command",
        key: "",
        display: "Agents",
        context: "Preferences",
        keywords: "npx skills add agent skill canban cli install",
    },
    Command {
        id: "check-herdr",
        title: "Check Herdr Runtime Connection",
        key: "",
        display: "System",
        context: "Preferences",
        keywords: "daemon pty status",
    },
    Command {
        id: "check-updates",
        title: "Check for Application Updates",
        key: "",
        display: "System",
        context: "Preferences",
        keywords: "upgrade version release",
    },
    Command {
        id: "report-issue",
        title: "Report an Issue…",
        key: "",
        display: "Support",
        context: "Preferences",
        keywords: "bug feedback diagnostics",
    },
    Command {
        id: "git-fetch",
        title: "Git: Fetch Remote",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "fetch origin remote sync prune",
    },
    Command {
        id: "git-pull",
        title: "Git: Pull (Fast-Forward Only)",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "pull ff-only fast-forward sync update",
    },
    Command {
        id: "git-push",
        title: "Git: Push Current Branch",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "push origin publish upstream",
    },
    Command {
        id: "git-stage-all",
        title: "Git: Stage All Changes",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "stage add working tree index",
    },
    Command {
        id: "git-unstage-all",
        title: "Git: Unstage All Changes",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "unstage reset index",
    },
    Command {
        id: "git-working-tree",
        title: "Git: Open Working Tree & Commit Composer",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "commit amend stage status diff",
    },
];

pub fn execute(ui: Ui, id: &str) {
    if id == "toggle-notifications" || id == "go-overview" {
        ui.quick_open.set(false);
        crate::overview::toggle(ui);
        return;
    }
    crate::utilities::close(ui);
    ui.quick_open.set(false);

    if let Some(stage_id) = id.strip_prefix("move-card:") {
        if let Some(card) = ui.current_card().filter(|c| c.active()) {
            crate::presenter::move_card(ui, &card.id, stage_id);
        }
        return;
    }
    if let Some(tag) = id.strip_prefix("filter-tag:") {
        let pid = ui.project.get_untracked();
        if !pid.is_empty() {
            crate::tags::toggle_board_filter(ui, &pid, tag);
            ui.navigate("board");
        }
        return;
    }
    if let Some(wt_id) = id.strip_prefix("worktree-shell:") {
        if let Some(wt) = ui
            .snapshot
            .with_untracked(|s| s.worktrees.iter().find(|w| w.id == wt_id).cloned())
        {
            crate::terminals::worktrees::open_shell(ui, &wt);
        }
        return;
    }
    if let Some(wt_id) = id.strip_prefix("worktree-promote:") {
        if ui
            .snapshot
            .with_untracked(|s| s.worktrees.iter().any(|w| w.id == wt_id))
        {
            ui.worktree_promote.set(Some(wt_id.to_owned()));
        }
        return;
    }
    if let Some(wt_id) = id.strip_prefix("worktree-diff:") {
        if let Some(wt) = ui
            .snapshot
            .with_untracked(|s| s.worktrees.iter().find(|w| w.id == wt_id).cloned())
        {
            crate::terminals::worktrees::open_diff(ui, &wt);
        }
        return;
    }
    if let Some(wt_id) = id.strip_prefix("worktree-delete:") {
        if ui
            .snapshot
            .with_untracked(|s| s.worktrees.iter().any(|w| w.id == wt_id))
        {
            crate::terminals::worktrees::request_remove(ui, wt_id.to_owned(), false);
        }
        return;
    }

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
            ui.search_query.set(">".into());
            ui.quick_open.set(true);
        }
        "mark-all-read" => crate::overview::mark_all_read(ui, None),
        "go-board" => ui.navigate("board"),
        "clear-board-filters" => {
            let pid = ui.project.get_untracked();
            if !pid.is_empty() {
                ui.github.filters.update(|m| {
                    m.remove(&pid);
                });
            }
            ui.navigate("board");
        }
        "go-terminals" => ui.navigate("terminals"),
        "go-review" => crate::review::open_compare(ui),
        "go-github" => ui.navigate("github"),
        "go-workflows" => ui.navigate("workflows"),
        "edit-workflow-agent" => crate::workflows::edit_agent(ui),
        "go-project-agents" => {
            if ui.current_project().is_some() {
                ui.navigate("project-agents");
            } else {
                ui.navigate("agent-setup");
            }
        }
        "go-agent-setup" => ui.navigate("agent-setup"),
        "go-settings-agents" => ui.open_settings_section("agents"),
        "go-drafts" => ui.navigate("drafts"),
        "go-archive" => ui.navigate("archive"),
        "go-settings" => ui.navigate("settings"),
        "go-acknowledgements" => ui.open_settings_section("acknowledgements"),
        "go-machines" => ui.navigate("machines"),
        "go-project-location" => {
            if ui.current_project().is_some() {
                ui.navigate("project-location");
            } else {
                ui.navigate("new-project");
            }
        }
        "go-github-repo" => {
            if ui.current_project().is_some() {
                ui.navigate("github-repository");
            } else {
                ui.navigate("github");
            }
        }
        "go-github-account" => ui.navigate("github-account"),
        "new-project" => ui.navigate("new-project"),
        "switch-project" => ui.show_search(true),
        "toggle-card-terminal" => {
            if ui.page.get_untracked() == "card" && !ui.card.get_untracked().is_empty() {
                crate::card::terminal::toggle(ui);
            } else if let Some(c) = ui.current_card() {
                crate::terminals::open_card_quick(ui, &c.id);
            } else {
                ui.notify("Open a Card first to toggle its Live Terminal");
            }
        }
        "popout-card-terminal" => {
            if !ui.card_id().is_empty() {
                crate::card::terminal::pop_out(ui);
            } else {
                ui.navigate("terminals");
            }
        }
        "open-card-diff" => {
            if let Some(c) = ui.current_card() {
                crate::review::open_card(ui, &c.id);
            } else {
                crate::review::open_compare(ui);
            }
        }
        "quick-merge-card" => {
            if let Some(c) = ui.current_card() {
                let project = ui.current_project();
                if crate::card::quick_merge_eligible(&c, project.as_ref()) {
                    crate::card::quick_merge_start(ui, c);
                } else {
                    ui.notify("Current Card is not eligible for local quick merge");
                }
            } else {
                ui.notify("Select a Card first");
            }
        }
        "retry-auto-merge" => {
            if let Some(c) = ui.current_card() {
                let (has_deliver_in_project, active_non_deliver) =
                    ui.snapshot.with_untracked(|s| {
                        let has_deliver = s.workflow_revisions.iter().any(|w| {
                            w.project_id == c.project_id
                                && w.definition.steps.values().any(|st| {
                                    matches!(st, canban_types::WorkflowStep::Deliver { .. })
                                })
                        });
                        let non_deliver = s
                            .workflow_runs
                            .iter()
                            .any(|r| r.card_id == c.id && r.active() && !r.step_is_deliver());
                        (has_deliver, non_deliver)
                    });
                if !c.active() || c.merged() || c.worktree.is_none() || !has_deliver_in_project {
                    ui.notify("Current Card cannot retry auto-merge");
                } else if active_non_deliver {
                    ui.notify("Card workflow is currently active at another step");
                } else {
                    dispatch(
                        ui,
                        "workflow.deliver",
                        json!({"card_id": c.id, "revision": c.revision}),
                    );
                }
            } else {
                ui.notify("Select a Card first");
            }
        }
        "edit-card" => {
            if let Some(c) = ui.current_card() {
                let key = format!("card:{}", c.id);
                let mut d = ui
                    .drafts
                    .with(|m| m.get(&key).cloned())
                    .unwrap_or_else(|| crate::drafts::Draft::card(&c));
                d.editing = !d.editing;
                crate::drafts::put(ui, key, d);
            }
        }
        "edit-last-comment" => {
            if ui.page.get_untracked() == "github" && ui.github.reference.get_untracked().is_some()
            {
                let sec = ui.github.section.get_untracked();
                if matches!(sec.as_str(), "discussion" | "threads" | "reviews") {
                    let my_login = crate::github::active_login_untracked(ui);
                    let target = ui.github.detail.with_untracked(|det| {
                        det["data"].as_array().and_then(|arr| {
                            arr.iter().rev().find_map(|entry| {
                                let cid = entry["id"].as_u64().unwrap_or(0);
                                let author = entry["user"]["login"].as_str().unwrap_or("");
                                (cid > 0
                                    && my_login
                                        .as_deref()
                                        .is_some_and(|me| author.eq_ignore_ascii_case(me)))
                                .then(|| (cid, entry["body"].as_str().unwrap_or("").to_owned()))
                            })
                        })
                    });
                    if let Some((cid, body)) = target {
                        batch(move || {
                            ui.github.edit_error.set(String::new());
                            ui.github.edit_draft.set(body);
                            ui.github.editing_comment.set(Some((sec, cid)));
                        });
                    } else {
                        ui.notify("No comments authored by you in this GitHub section");
                    }
                } else {
                    ui.notify("Open Discussion, Threads, or Reviews to edit a GitHub comment");
                }
            } else if let Some(c) = ui.current_card().filter(|c| c.active()) {
                let latest = ui.snapshot.with_untracked(|s| {
                    s.events
                        .iter()
                        .filter(|e| {
                            e.card_id == c.id && e.kind == "comment.created" && e.actor == "user"
                        })
                        .max_by_key(|e| e.sequence)
                        .map(|e| (e.id.clone(), e.sequence))
                });
                if let Some((eid, seq)) = latest {
                    if ui.page.get_untracked() != "card"
                        && !(ui.page.get_untracked() == "review"
                            && ui.review_state.card_drawer_open.get_untracked())
                    {
                        ui.open_card(c.id);
                    }
                    ui.card_event.set(Some(seq));
                    ui.deleting_comment.set(None);
                    ui.editing_comment.set(Some(eid));
                } else {
                    ui.notify("No user comments to edit on this Card");
                }
            } else {
                ui.notify("Open an active Card or GitHub conversation first");
            }
        }
        "delete-last-comment" => {
            if ui.page.get_untracked() == "github" && ui.github.reference.get_untracked().is_some()
            {
                let sec = ui.github.section.get_untracked();
                if matches!(sec.as_str(), "discussion" | "threads") {
                    let my_login = crate::github::active_login_untracked(ui);
                    let target = ui.github.detail.with_untracked(|det| {
                        det["data"].as_array().and_then(|arr| {
                            arr.iter().rev().find_map(|entry| {
                                let cid = entry["id"].as_u64().unwrap_or(0);
                                let author = entry["user"]["login"].as_str().unwrap_or("");
                                (cid > 0
                                    && my_login
                                        .as_deref()
                                        .is_some_and(|me| author.eq_ignore_ascii_case(me)))
                                .then_some(cid)
                            })
                        })
                    });
                    if let Some(cid) = target {
                        batch(move || {
                            ui.github.delete_error.set(String::new());
                            ui.github.editing_comment.set(None);
                            ui.github.deleting_comment.set(Some((sec, cid)));
                        });
                    } else {
                        ui.notify("No comments authored by you in this GitHub section");
                    }
                } else if sec == "reviews" {
                    ui.notify("GitHub does not support deleting submitted pull request reviews");
                } else {
                    ui.notify("Open Discussion or Review threads to delete a GitHub comment");
                }
            } else if ui.page.get_untracked() == "review"
                && ui.review_state.mode.get_untracked() == "github_pr"
            {
                let my_login = crate::github::active_login_untracked(ui);
                let target = ui.review_state.compare_data.with_untracked(|cd| {
                    cd["threads"].as_array().and_then(|arr| {
                        arr.iter().rev().find_map(|entry| {
                            let cid = entry["id"].as_u64().unwrap_or(0);
                            let author = entry["user"]["login"].as_str().unwrap_or("");
                            (cid > 0
                                && my_login
                                    .as_deref()
                                    .is_some_and(|me| author.eq_ignore_ascii_case(me)))
                            .then_some(cid)
                        })
                    })
                });
                if let Some(cid) = target {
                    batch(move || {
                        ui.review_state.remote_delete_error.set(String::new());
                        ui.review_state.editing_remote_comment.set(None);
                        ui.review_state.deleting_remote_comment.set(Some(cid));
                    });
                } else {
                    ui.notify("No inline PR comments authored by you on this diff");
                }
            } else if let Some(c) = ui.current_card().filter(|c| c.active()) {
                let latest = ui.snapshot.with_untracked(|s| {
                    s.events
                        .iter()
                        .filter(|e| {
                            e.card_id == c.id && e.kind == "comment.created" && e.actor == "user"
                        })
                        .max_by_key(|e| e.sequence)
                        .map(|e| (e.id.clone(), e.sequence))
                });
                if let Some((eid, seq)) = latest {
                    if ui.page.get_untracked() != "card"
                        && !(ui.page.get_untracked() == "review"
                            && ui.review_state.card_drawer_open.get_untracked())
                    {
                        ui.open_card(c.id);
                    }
                    ui.card_event.set(Some(seq));
                    ui.editing_comment.set(None);
                    ui.deleting_comment.set(Some(eid));
                } else {
                    ui.notify("No user comments to delete on this Card");
                }
            } else {
                ui.notify("Open an active Card or GitHub conversation first");
            }
        }
        "open-last-github-comment" => {
            if ui.page.get_untracked() == "github"
                && let Some(r) = ui.github.reference.get_untracked()
            {
                let sec = ui.github.section.get_untracked();
                if matches!(sec.as_str(), "discussion" | "threads" | "reviews") {
                    let url = ui.github.detail.with_untracked(|det| {
                        det["data"].as_array().and_then(|arr| {
                            arr.iter().rev().find_map(|entry| {
                                crate::github::comment_web_url(entry, Some(r.url.as_str()), &sec)
                            })
                        })
                    });
                    if let Some(url) = url {
                        crate::github::open_url(ui, url);
                    } else {
                        ui.notify("No GitHub comments in this section");
                    }
                } else {
                    crate::github::open_url(ui, r.url);
                }
            } else if ui.page.get_untracked() == "review"
                && let Some(pr) = ui.review_state.pr_target.get_untracked()
            {
                let base_url = pr["url"]
                    .as_str()
                    .filter(|u| !u.is_empty())
                    .map(str::to_owned)
                    .or_else(|| {
                        let repo = pr["repository"].as_str().unwrap_or("");
                        let num = pr["number"].as_u64().unwrap_or(0);
                        (!repo.is_empty() && num > 0)
                            .then(|| format!("https://github.com/{repo}/pull/{num}"))
                    });
                let url = ui.review_state.compare_data.with_untracked(|cd| {
                    cd["threads"].as_array().and_then(|arr| {
                        arr.iter().rev().find_map(|entry| {
                            crate::github::comment_web_url(entry, base_url.as_deref(), "threads")
                        })
                    })
                });
                if let Some(url) = url.or(base_url) {
                    crate::github::open_url(ui, url);
                } else {
                    ui.notify("No GitHub pull request link available");
                }
            } else if let Some(c) = ui.current_card()
                && let Some(src) = c.github_issue()
            {
                crate::github::open_url(ui, src.url.clone());
            } else {
                ui.notify("Open a GitHub conversation or GitHub Card first");
            }
        }
        "card-workspace-details" => {
            if let Some(c) = ui.current_card() {
                let key = format!("card:{}", c.id);
                let mut d = ui
                    .drafts
                    .with(|m| m.get(&key).cloned())
                    .unwrap_or_else(|| crate::drafts::Draft::card(&c));
                d.workspace_details = !d.workspace_details;
                crate::drafts::put(ui, key, d);
            }
        }
        "card-attachments" => {
            if let Some(c) = ui.current_card() {
                let key = format!("card:{}", c.id);
                let mut d = ui
                    .drafts
                    .with(|m| m.get(&key).cloned())
                    .unwrap_or_else(|| crate::drafts::Draft::card(&c));
                d.details = !d.details;
                crate::drafts::put(ui, key, d);
            }
        }
        "archive-card" => {
            if let Some(c) = ui.current_card() {
                let inactive = c.archived_at.is_some() || c.removed_at.is_some();
                crate::card_actions::apply(
                    ui,
                    c,
                    if inactive {
                        crate::card_actions::Action::Restore
                    } else {
                        crate::card_actions::Action::Archive
                    },
                );
            }
        }
        "new-terminal" => {
            if ui.page.get_untracked() == "card"
                && ui.card_terminal_open.get_untracked()
                && let Some(c) = ui.current_card()
            {
                crate::terminals::create_in_card(ui, c.project_id, c.id, false);
                ui.terminal_focus.update(|v| *v += 1);
            } else if ui.page.get_untracked() == "terminals" {
                crate::terminals::new_tab(ui);
                ui.terminal_focus.update(|v| *v += 1);
            } else {
                crate::terminals::open_global(ui);
            }
        }
        "new-worktree" => {
            let pid = ui.project.get_untracked();
            let target = if pid.is_empty() {
                ui.snapshot
                    .with_untracked(|s| s.projects.first().map(|p| p.id.clone()))
                    .unwrap_or_default()
            } else {
                pid
            };
            if target.is_empty() {
                ui.navigate("new-project");
            } else {
                ui.navigate("terminals");
                ui.worktree_modal.set(Some(target));
            }
        }
        "close-terminal" if ui.page.get_untracked() == "terminals" => {
            crate::terminals::close_tab(ui, &ui.terminal_selection.get_untracked())
        }
        "close-terminal"
            if ui.page.get_untracked() == "card" && ui.card_terminal_open.get_untracked() =>
        {
            let cid = ui.card_id();
            if let Some(entry) = crate::card::terminal::select_entry_for_card(ui, &cid) {
                crate::terminals::close_entry(ui, &entry.saved.id);
            }
        }
        "next-tab" => {
            if ui.page.get_untracked() == "card" && ui.card_terminal_open.get_untracked() {
                crate::terminals::cycle(ui, true);
            } else {
                ui.navigate("terminals");
                crate::terminals::cycle(ui, true);
            }
        }
        "previous-tab" => {
            if ui.page.get_untracked() == "card" && ui.card_terminal_open.get_untracked() {
                crate::terminals::cycle(ui, false);
            } else {
                ui.navigate("terminals");
                crate::terminals::cycle(ui, false);
            }
        }
        "toggle-terminal-sidebar" => {
            ui.navigate("terminals");
            ui.terminal_sidebar.update(|v| *v = !*v);
        }
        "rename-terminal" => {
            if ui.page.get_untracked() == "card" && ui.card_terminal_open.get_untracked() {
                let cid = ui.card_id();
                if let Some(entry) = crate::card::terminal::select_entry_for_card(ui, &cid) {
                    ui.terminal_rename.set(Some(entry.saved.id));
                }
            } else {
                let sel = ui.terminal_selection.get_untracked();
                if !sel.is_empty() {
                    ui.navigate("terminals");
                    ui.terminal_rename.set(Some(sel));
                }
            }
        }
        "interrupt-terminal" => {
            let entry =
                if ui.page.get_untracked() == "card" && ui.card_terminal_open.get_untracked() {
                    let cid = ui.card_id();
                    crate::card::terminal::select_entry_for_card(ui, &cid)
                } else {
                    crate::terminals::current(ui)
                };
            if let Some(e) = entry {
                if e.agent
                    .as_ref()
                    .and_then(|a| a.agent())
                    .is_some_and(|a| !a.finished() && a.terminal.is_some())
                {
                    dispatch(ui, "agent.interrupt", json!({"terminal_id": e.saved.id}));
                } else if !e.ended() && e.binding().is_some() {
                    dispatch(ui, "terminal.interrupt", json!({"terminal_id": e.saved.id}));
                }
            }
        }
        "choose-theme" => crate::utilities::open(ui, "theme"),
        "choose-terminal-font" => crate::utilities::open(ui, "terminal-font"),
        "toggle-awake" => crate::utilities::toggle_awake(ui),
        "refresh-usage" => crate::utilities::usage(ui, crate::utilities::machine(ui), true),
        "reload-config" => dispatch(ui, "config.reload", json!({})),
        "copy-skill-command" => crate::agents_setup::copy_skill_command(ui),
        "check-herdr" => dispatch(ui, "herdr.status", json!({})),
        "check-updates" => crate::updates::check(ui),
        "report-issue" => crate::utilities::report::open(ui),
        "git-fetch" => {
            if ui.page.get_untracked() != "review" {
                crate::review::open_compare(ui);
            }
            crate::review::git_ops::run_fetch(ui);
        }
        "git-pull" => {
            if ui.page.get_untracked() != "review" {
                crate::review::open_compare(ui);
            }
            crate::review::git_ops::run_pull(ui);
        }
        "git-push" => {
            if ui.page.get_untracked() != "review" {
                crate::review::open_compare(ui);
            }
            crate::review::git_ops::run_push(ui);
        }
        "git-stage-all" => {
            if ui.page.get_untracked() != "review" {
                crate::review::open_compare(ui);
            }
            crate::review::select_git_pair(ui, "HEAD", "WORKDIR");
            crate::review::git_ops::run_stage(ui, Vec::new(), true);
        }
        "git-unstage-all" => {
            if ui.page.get_untracked() != "review" {
                crate::review::open_compare(ui);
            }
            crate::review::select_git_pair(ui, "HEAD", "WORKDIR");
            crate::review::git_ops::run_unstage(ui, Vec::new(), true);
        }
        "git-working-tree" => {
            crate::review::open_compare(ui);
            crate::review::select_git_pair(ui, "HEAD", "WORKDIR");
        }
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
    effect(move || {
        if ui.page.get() == "card"
            && !ui.card.get().is_empty()
            && !ui.quick_open.get()
            && ui.utility_popup.get().is_empty()
            && !ui.native_modal.get()
        {
            on_key_with(
                "mod+j",
                KeyOptions {
                    when: When::Always,
                    ..Default::default()
                },
                move |_| crate::card::terminal::toggle(ui),
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
            move |_| {
                if command.id == "new-terminal"
                    && (ui.quick_open.get_untracked()
                        || !ui.utility_popup.get_untracked().is_empty()
                        || ui.native_modal.get_untracked()
                        || ui.terminal_rename.get_untracked().is_some()
                        || ui.terminal_close_confirm.get_untracked().is_some()
                        || ui.worktree_modal.get_untracked().is_some()
                        || ui.worktree_promote.get_untracked().is_some()
                        || ui.worktree_remove_confirm.get_untracked().is_some())
                {
                    return;
                }
                execute(ui, command.id);
            },
        );
    }
    // Plain `/` opens the command palette (`>` prefix) whenever focus is not in a text input or hosted terminal.
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
                && ui.terminal_rename.get_untracked().is_none()
                && ui.terminal_close_confirm.get_untracked().is_none()
                && ui.worktree_modal.get_untracked().is_none()
                && ui.worktree_promote.get_untracked().is_none()
                && ui.worktree_remove_confirm.get_untracked().is_none()
            {
                ui.project_picker.set(false);
                ui.search_query.set(">".into());
                ui.quick_open.set(true);
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_are_unique_and_shortcuts_parse() {
        let mut seen = std::collections::BTreeSet::new();
        for cmd in CATALOG {
            assert!(
                seen.insert(cmd.id),
                "duplicate command id in CATALOG: {}",
                cmd.id
            );
            if !cmd.key.is_empty() {
                nsh_core::desktop::Shortcut::parse(cmd.key, false).unwrap();
            }
        }
        let new_term = CATALOG
            .iter()
            .find(|c| c.id == "new-terminal")
            .expect("new-terminal registered");
        assert_eq!(new_term.context, "Global");
        assert_eq!(new_term.key, "mod+t");
        assert!(
            !crate::terminals::TAB_KEYS
                .iter()
                .any(|(k, _)| *k == "mod+t")
        );
        assert!(CATALOG.iter().any(|c| c.id == "copy-skill-command"));
        assert!(CATALOG.iter().any(|c| c.id == "edit-last-comment"));
        assert!(CATALOG.iter().any(|c| c.id == "delete-last-comment"));
        assert!(CATALOG.iter().any(|c| c.id == "open-last-github-comment"));
        assert!(CATALOG.iter().any(|c| c.id == "retry-auto-merge"));
        let go_git = CATALOG.iter().find(|c| c.id == "go-review").unwrap();
        assert_eq!(go_git.title, "Go to Git");
        assert_eq!(go_git.display, "Git");
        assert!(go_git.keywords.contains("diff"));
    }
}
