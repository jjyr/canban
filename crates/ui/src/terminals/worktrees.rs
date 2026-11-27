//! Unified Worktree scopes, standalone Canban-managed Git branch worktrees,
//! Git status summaries, Card promotion, and guarded deletion.
use super::*;
use canban_types::{ManagedWorktree, Project, Snapshot};
use canban_ui_kit::compact_home;
use nsh_core::element::desktop::{MenuItem, popup_menu, popup_menu_at};
use serde_json::Value;

const DEFAULT_BASE_ID: &str = "__default_base__";

pub(crate) fn effective_default_base(project: Option<&Project>, refs: &Value) -> Option<String> {
    project
        .and_then(|p| p.workflow.workspace.base_branch.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            refs["default_branch"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "HEAD")
                .map(str::to_owned)
        })
        .or_else(|| {
            project
                .and_then(|p| p.target_branch.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
}

pub(crate) fn effective_sync_remote(
    project: Option<&Project>,
    refs: &Value,
    base_branch: &str,
) -> String {
    let base = base_branch.trim();
    if !base.is_empty() {
        let mut remotes: Vec<&str> = refs["branches"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|br| br["kind"].as_str() == Some("remote"))
            .filter_map(|br| br["name"].as_str())
            .filter_map(|name| name.split('/').next())
            .filter(|r| !r.is_empty() && !r.starts_with('-'))
            .collect();
        if let Some(sr) = refs["sync_remote"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.starts_with('-'))
        {
            remotes.push(sr);
        }
        if let Some(rn) = refs["remote_name"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.starts_with('-'))
        {
            remotes.push(rn);
        }
        remotes.sort_by_key(|r| (std::cmp::Reverse(r.len()), *r));
        remotes.dedup();
        let has_local_exact =
            refs["branches"].as_array().into_iter().flatten().any(|br| {
                br["kind"].as_str() == Some("local") && br["name"].as_str() == Some(base)
            });
        if !has_local_exact {
            for r in remotes {
                if let Some(rest) = base.strip_prefix(&format!("{r}/"))
                    && !rest.is_empty()
                {
                    return r.to_owned();
                }
            }
        }
    }
    refs["sync_remote"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('-'))
        .or_else(|| {
            refs["remote_name"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.starts_with('-'))
        })
        .or_else(|| {
            project
                .map(|p| p.workflow.workspace.remote.trim())
                .filter(|s| !s.is_empty() && !s.starts_with('-'))
        })
        .unwrap_or("origin")
        .to_owned()
}

pub(crate) fn build_base_branch_options(
    refs: &Value,
    default_branch: Option<&str>,
    sync_remote: &str,
    selected: &str,
) -> Vec<PickerOption> {
    let selected = selected.trim();
    let default_trimmed = default_branch.map(str::trim).filter(|s| !s.is_empty());
    let branches = refs["branches"].as_array().cloned().unwrap_or_default();
    let mut options = Vec::new();

    if let Some(def) = default_trimmed {
        let is_current = branches.iter().any(|br| {
            br["kind"].as_str() == Some("local")
                && br["name"].as_str() == Some(def)
                && br["is_current"].as_bool().unwrap_or(false)
        });
        options.push(
            PickerOption::new(def, def)
                .group("Default")
                .detail(if is_current {
                    "default · current"
                } else {
                    "default"
                })
                .icon(Icon::Branch)
                .mono(true)
                .selected(selected.is_empty() || selected == def)
                .keywords(["default", def]),
        );
    } else {
        options.push(
            PickerOption::new(DEFAULT_BASE_ID, "Repository default")
                .group("Default")
                .detail("auto")
                .icon(Icon::Branch)
                .selected(selected.is_empty())
                .keywords(["default"]),
        );
    }

    for br in &branches {
        if br["kind"].as_str() != Some("local") {
            continue;
        }
        let name = br["name"].as_str().unwrap_or("").trim();
        if name.is_empty() || default_trimmed == Some(name) {
            continue;
        }
        let is_cur = br["is_current"].as_bool().unwrap_or(false);
        let short = br["short_sha"].as_str().unwrap_or("").to_owned();
        let detail = if is_cur {
            "current".to_owned()
        } else {
            short.clone()
        };
        options.push(
            PickerOption::new(name, name)
                .group("Local Branches")
                .detail(detail)
                .icon(Icon::Branch)
                .mono(true)
                .selected(selected == name)
                .keywords([short]),
        );
    }

    let default_remote_ref = default_trimmed.map(|def| format!("{sync_remote}/{def}"));
    for br in &branches {
        if br["kind"].as_str() != Some("remote") {
            continue;
        }
        let name = br["name"].as_str().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        let short = br["short_sha"].as_str().unwrap_or("").to_owned();
        let detail = if default_remote_ref.as_deref() == Some(name) {
            "default remote".to_owned()
        } else {
            short.clone()
        };
        options.push(
            PickerOption::new(name, name)
                .group("Remote Branches")
                .detail(detail)
                .icon(Icon::Branch)
                .mono(true)
                .selected(selected == name)
                .keywords([short]),
        );
    }

    options
}

pub fn display_directory_path(path: &str, host: &str) -> String {
    if host.is_empty() || host == "local" {
        let home = std::env::var("HOME").unwrap_or_default();
        return compact_home(path, &home);
    }
    path.to_owned()
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorktreeScope {
    ProjectRoot(String),
    CardWorktree(String),
    ManagedWorktree(String),
    Unassigned(String),
    Directory {
        host: String,
        path: String,
        project: Option<String>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScopeGitSummary {
    pub branch: Option<String>,
    pub dirty: bool,
    pub additions: u32,
    pub deletions: u32,
    pub ahead: u32,
    pub behind: u32,
}

impl ScopeGitSummary {
    pub fn indicators(&self) -> Vec<String> {
        format_git_indicators(self.dirty, self.ahead, self.behind)
    }

    pub fn tooltip_indicators(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.dirty {
            parts.push(format!("+{} −{}", self.additions, self.deletions));
        }
        if self.ahead > 0 {
            parts.push(format!("↑{}", self.ahead));
        }
        if self.behind > 0 {
            parts.push(format!("↓{}", self.behind));
        }
        parts
    }
}

pub fn format_git_indicators(dirty: bool, ahead: u32, behind: u32) -> Vec<String> {
    let mut parts = Vec::new();
    if dirty {
        parts.push("changed".into());
    }
    if ahead > 0 {
        parts.push(format!("↑{ahead}"));
    }
    if behind > 0 {
        parts.push(format!("↓{behind}"));
    }
    parts
}

impl WorktreeScope {
    pub fn recent_key(&self) -> String {
        match self {
            Self::Directory { host, path, .. } => format!("scope:directory:{host}:{path}"),
            Self::ProjectRoot(pid) => format!("scope:project:{pid}"),
            Self::CardWorktree(cid) => format!("scope:card:{cid}"),
            Self::ManagedWorktree(wid) => format!("scope:worktree:{wid}"),
            Self::Unassigned(machine) => format!("scope:unassigned:{machine}"),
        }
    }

    pub fn project_id(&self, s: &Snapshot) -> Option<String> {
        match self {
            Self::ProjectRoot(pid) => Some(pid.clone()),
            Self::CardWorktree(cid) => s
                .cards
                .iter()
                .find(|c| &c.id == cid)
                .map(|c| c.project_id.clone()),
            Self::ManagedWorktree(wid) => s
                .worktrees
                .iter()
                .find(|w| &w.id == wid)
                .map(|w| w.project_id.clone()),
            Self::Unassigned(_) => None,
            Self::Directory { project, .. } => project.clone(),
        }
    }

    pub fn display_name(&self, s: &Snapshot, children: &[Entry], ui: Option<Ui>) -> String {
        let git = scope_git_summary(s, self, children);
        match self {
            Self::ProjectRoot(pid) => git
                .branch
                .or_else(|| {
                    s.projects
                        .iter()
                        .find(|p| &p.id == pid)
                        .and_then(|p| p.target_branch.clone())
                        .filter(|b| !b.is_empty())
                })
                .unwrap_or_else(|| "main".into()),
            Self::CardWorktree(cid) => {
                let card = s.cards.iter().find(|c| &c.id == cid);
                git.branch
                    .or_else(|| {
                        card.and_then(|c| c.branch.clone())
                            .filter(|b| !b.is_empty())
                    })
                    .unwrap_or_else(|| {
                        card.map(|c| format!("#{} {}", c.number, c.title))
                            .unwrap_or_else(|| "Card worktree".into())
                    })
            }
            Self::ManagedWorktree(wid) => git
                .branch
                .or_else(|| {
                    s.worktrees
                        .iter()
                        .find(|w| &w.id == wid)
                        .map(|w| w.branch.clone())
                })
                .unwrap_or_else(|| "worktree".into()),
            Self::Directory { host, path, .. } => {
                let short = display_directory_path(path, host);
                git.branch
                    .map(|b| format!("{short} · {b}"))
                    .unwrap_or(short)
            }
            Self::Unassigned(host_id) => {
                if host_id.is_empty() || host_id == "local" {
                    "Local".into()
                } else if let Some(ui) = ui {
                    crate::hosts::host_name(ui, host_id)
                } else {
                    s.host_display_name(host_id).to_owned()
                }
            }
        }
    }
}

/// Location only: Agent task ownership remains on Entry.agent and never pins the tree.
/// Before initial directory discovery arrives (`location` missing or `cwd` unset), fall
/// back to the terminal's registered origin worktree/project so newly created project
/// terminals mount directly under their Project instead of transiently flashing in Other.
pub fn entry_scope(s: &Snapshot, e: &Entry) -> WorktreeScope {
    let Some(loc) = e.location.as_ref().filter(|l| l.cwd.is_some()) else {
        if let Some(cid) = e.saved.origin_card_id.as_ref().filter(|id| {
            s.cards.iter().any(|c| {
                &c.id == *id
                    && c.archived_at.is_none()
                    && c.worktree.as_deref().is_some_and(|w| !w.is_empty())
            })
        }) {
            return WorktreeScope::CardWorktree(cid.clone());
        }
        if let Some(pid) = e
            .project_id
            .as_ref()
            .filter(|id| s.projects.iter().any(|p| &p.id == *id))
        {
            return WorktreeScope::ProjectRoot(pid.clone());
        }
        return WorktreeScope::Unassigned(e.host_id.clone());
    };
    if let Some(wid) = loc
        .worktree_id
        .as_ref()
        .filter(|id| s.worktrees.iter().any(|w| &w.id == *id))
    {
        return WorktreeScope::ManagedWorktree(wid.clone());
    }
    if let Some(cid) = loc.card_id.as_ref().filter(|id| {
        s.cards
            .iter()
            .any(|c| &c.id == *id && c.archived_at.is_none())
    }) {
        return WorktreeScope::CardWorktree(cid.clone());
    }
    let path = loc
        .directory_root
        .as_ref()
        .or(loc.cwd.as_ref())
        .unwrap()
        .clone();
    if let Some(pid) = loc.project_id.as_ref()
        && s.projects
            .iter()
            .any(|p| &p.id == pid && p.directory == path)
    {
        return WorktreeScope::ProjectRoot(pid.clone());
    }
    WorktreeScope::Directory {
        host: e.host_id.clone(),
        path,
        project: loc.project_id.clone(),
    }
}

/// Resolution/delivery history of the Card at the observed location, independent of Git
/// dirtiness and of the Terminal's task owner. No Git or network discovery is needed.
pub fn scope_resolution_label(s: &Snapshot, scope: &WorktreeScope) -> Option<String> {
    let WorktreeScope::CardWorktree(id) = scope else {
        return None;
    };
    let card = s
        .cards
        .iter()
        .find(|c| &c.id == id && c.archived_at.is_none() && c.resolved())?;
    let target = card
        .delivery
        .as_ref()
        .map(|d| d.target_branch.as_str())
        .filter(|b| !b.is_empty())
        .or_else(|| {
            card.pull_request
                .as_ref()
                .filter(|p| p.merged && !p.base_branch.is_empty())
                .map(|p| p.base_branch.as_str())
        });
    Some(match target {
        Some(target) => format!("Resolved (merged into {target})"),
        None => "Resolved".into(),
    })
}

/// Stable UI identity independent of names, branches and inferred business associations.
pub fn location_key(e: &Entry) -> String {
    match &e.location {
        Some(l) if l.directory_root.is_some() || l.cwd.is_some() => format!(
            "location:{}:{}",
            e.host_id,
            l.directory_root
                .as_deref()
                .or(l.cwd.as_deref())
                .unwrap_or("?")
        ),
        _ => match e.project_id.as_deref() {
            Some(pid) => format!("location:{}:project:{pid}", e.host_id),
            None => format!("location:{}:?", e.host_id),
        },
    }
}

pub fn scope_git_summary(
    s: &Snapshot,
    scope: &WorktreeScope,
    children: &[Entry],
) -> ScopeGitSummary {
    for child in children {
        if matches!(scope, WorktreeScope::ProjectRoot(_))
            && s.merge_batches.iter().any(|b| b.id == child.saved.id)
        {
            continue;
        }
        if let Some(loc) = &child.location
            && (loc.branch.is_some() || loc.dirty || loc.ahead > 0 || loc.behind > 0)
        {
            return ScopeGitSummary {
                branch: loc.branch.clone(),
                dirty: loc.dirty,
                additions: loc.additions,
                deletions: loc.deletions,
                ahead: loc.ahead,
                behind: loc.behind,
            };
        }
    }
    match scope {
        WorktreeScope::ProjectRoot(pid) => ScopeGitSummary {
            branch: s
                .projects
                .iter()
                .find(|p| &p.id == pid)
                .and_then(|p| p.target_branch.clone())
                .filter(|b| !b.is_empty()),
            ..Default::default()
        },
        WorktreeScope::CardWorktree(cid) => ScopeGitSummary {
            branch: s
                .cards
                .iter()
                .find(|c| &c.id == cid)
                .and_then(|c| c.branch.clone())
                .filter(|b| !b.is_empty()),
            ..Default::default()
        },
        WorktreeScope::ManagedWorktree(wid) => ScopeGitSummary {
            branch: s
                .worktrees
                .iter()
                .find(|w| &w.id == wid)
                .map(|w| w.branch.clone()),
            ..Default::default()
        },
        WorktreeScope::Unassigned(_) | WorktreeScope::Directory { .. } => {
            ScopeGitSummary::default()
        }
    }
}

/// Active worktree scopes with >= 1 live terminal for a given Project (`Some(pid)`) or `Other` (`None`),
/// ordered deterministically: `ProjectRoot`, then `CardWorktree` (by card number), then `ManagedWorktree`.
pub fn project_scopes_in_tree_order(
    s: &Snapshot,
    live_rows: &[Entry],
    project: Option<&str>,
) -> Vec<(WorktreeScope, Vec<Entry>)> {
    let mut groups = std::collections::BTreeMap::<String, (WorktreeScope, Vec<Entry>)>::new();
    for e in live_rows {
        let scope = entry_scope(s, e);
        if scope.project_id(s).as_deref() == project {
            groups
                .entry(location_key(e))
                .or_insert_with(|| (scope, Vec::new()))
                .1
                .push(e.clone());
        }
    }
    groups.into_values().collect()
}

pub fn create_in_scope(ui: Ui, scope: &WorktreeScope) {
    match scope {
        WorktreeScope::ProjectRoot(pid) => super::create(ui, pid.clone(), None),
        WorktreeScope::CardWorktree(cid) => {
            if let Some(card) = ui
                .snapshot
                .with_untracked(|s| s.cards.iter().find(|c| &c.id == cid).cloned())
            {
                super::create(ui, card.project_id, Some(card.id));
            }
        }
        WorktreeScope::ManagedWorktree(wid) => {
            if let Some(wt) = ui
                .snapshot
                .with_untracked(|s| s.worktrees.iter().find(|w| &w.id == wid).cloned())
            {
                open_shell(ui, &wt);
            }
        }
        WorktreeScope::Directory {
            host,
            path,
            project,
        } => {
            super::launch_custom(
                ui,
                json!({ "host_id": host, "cwd": path, "project_id": project }),
            );
        }
        WorktreeScope::Unassigned(host_id) => {
            super::launch_custom(ui, json!({ "host_id": host_id }));
        }
    }
}

/// Switch to the remembered live terminal in `scope`, or spawn a shell in `scope` when empty.
pub fn activate_scope(ui: Ui, scope: &WorktreeScope) {
    let target = ui.closing_terminals.with_untracked(|closing| {
        ui.snapshot.with_untracked(|s| {
            ui.terminal_entries.with_untracked(|rows| {
                let live: Vec<&Entry> = rows
                    .iter()
                    .filter(|e| {
                        e.listed() && !closing.contains(&e.saved.id) && &entry_scope(s, e) == scope
                    })
                    .collect();
                let key = scope.recent_key();
                let recent = ui.terminal_recent.with(|m| m.get(&key).cloned());
                recent
                    .filter(|rid| live.iter().any(|e| e.saved.id.as_str() == *rid))
                    .or_else(|| live.first().map(|e| e.saved.id.clone()))
            })
        })
    });
    if let Some(pid) = ui.snapshot.with_untracked(|s| scope.project_id(s)) {
        ui.project.set(pid);
    }
    if let Some(id) = target {
        ui.open_terminal(id);
    } else {
        ui.navigate("terminals");
        create_in_scope(ui, scope);
    }
}

pub async fn card_scope_menu(ui: Ui, card: canban_types::Card, at: Option<Point<Points>>) {
    let cid_open = card.id.clone();
    let cid_diff = card.id.clone();
    let pid_shell = card.project_id.clone();
    let cid_shell = card.id.clone();
    let pid_agent = card.project_id.clone();
    let cid_agent = card.id.clone();
    let wt_copy = card.worktree.clone();

    let mut items = vec![
        MenuItem::new("New Terminal in Card Worktree")
            .on_select(move || super::create(ui, pid_shell.clone(), Some(cid_shell.clone()))),
        MenuItem::new("Open Agent in Card Worktree").on_select(move || {
            super::create_in_card(ui, pid_agent.clone(), cid_agent.clone(), true)
        }),
        MenuItem::separator(),
        MenuItem::new("Open Card").on_select(move || ui.terminal_card(cid_open.clone(), None)),
        MenuItem::new("Open in Git").on_select(move || crate::review::open_card(ui, &cid_diff)),
    ];
    if let Some(path) = wt_copy {
        items.push(
            MenuItem::new("Copy Worktree Path").on_select(move || copy_text(ui, path.clone())),
        );
    }

    if let Some(pos) = at {
        popup_menu_at(items, pos).await;
    } else {
        popup_menu(items).await;
    }
    ui.terminal_focus.update(|v| *v += 1);
}

pub fn open_shell(ui: Ui, wt: &ManagedWorktree) {
    if !crate::backend::is_connected() {
        let id = format!("gallery-wt-shell-{}", canban_types::id());
        let saved = canban_types::TerminalEntry {
            id: id.clone(),
            conversation_id: None,
            number: ui
                .snapshot
                .with_untracked(|s| s.terminals.iter().map(|t| t.number).max().unwrap_or(8))
                + 1,
            name: Some(wt.branch.clone()),
            origin_project_id: Some(wt.project_id.clone()),
            origin_card_id: None,
            agent_engine: None,
            agent: None,
            created_at: canban_types::now(),
            runtime: Some(canban_types::TerminalRuntime {
                core_id: Some("gallery".into()),
                terminal: Some(id.clone()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
                command: None,
            }),
        };
        let ctx = canban_types::TerminalContext {
            terminal_id: id.clone(),
            core_id: "gallery".into(),
            terminal: id.clone(),
            cwd: Some(wt.path.clone()),
            branch: Some(wt.branch.clone()),
            project_id: Some(wt.project_id.clone()),
            card_id: None,
            worktree_id: Some(wt.id.clone()),
            ..Default::default()
        };
        ui.snapshot.update(|s| {
            s.terminals.push(saved);
            s.terminal_contexts.push(ctx);
        });
        ui.open_terminal(id);
        return;
    }
    launch_custom(
        ui,
        json!({
            "project_id": wt.project_id,
            "cwd": wt.path,
            "name": wt.branch,
        }),
    );
}

pub fn open_agent(ui: Ui, wt: &ManagedWorktree, profile_id: &str) {
    if !crate::backend::is_connected() {
        open_shell(ui, wt);
        return;
    }
    launch_custom(
        ui,
        json!({
            "project_id": wt.project_id,
            "cwd": wt.path,
            "profile_id": profile_id,
            "name": format!("{} · @{profile_id}", wt.branch),
        }),
    );
}

/// Navigation follows observed checkout identity, never the Agent's original Card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompareTarget {
    Card(String),
    Checkout {
        project: String,
        cwd: String,
        base: String,
        branch: String,
        three_dot: bool,
    },
}

pub fn compare_target(s: &Snapshot, e: &Entry) -> Option<CompareTarget> {
    let loc = e.location.as_ref()?;
    let branch = loc.branch.as_ref()?.clone();
    loc.cwd.as_ref()?;
    match entry_scope(s, e) {
        WorktreeScope::CardWorktree(id) => Some(CompareTarget::Card(id)),
        WorktreeScope::ManagedWorktree(id) => {
            let wt = s.worktrees.iter().find(|w| w.id == id)?;
            Some(CompareTarget::Checkout {
                project: wt.project_id.clone(),
                cwd: wt.path.clone(),
                base: wt.base_branch.clone(),
                branch,
                three_dot: true,
            })
        }
        WorktreeScope::ProjectRoot(id) => {
            let p = s.projects.iter().find(|p| p.id == id)?;
            Some(CompareTarget::Checkout {
                project: id,
                cwd: p.directory.clone(),
                base: "HEAD".into(),
                branch,
                three_dot: false,
            })
        }
        WorktreeScope::Directory { .. } | WorktreeScope::Unassigned(_) => None,
    }
}

pub fn open_current_diff(ui: Ui) {
    let target = current(ui).and_then(|e| ui.snapshot.with_untracked(|s| compare_target(s, &e)));
    if let Some(target) = target {
        open_compare_target(ui, &target);
    } else {
        ui.notify("Changes unavailable: no Git checkout could be resolved for this terminal");
    }
}

pub fn open_compare_target(ui: Ui, target: &CompareTarget) {
    if let CompareTarget::Card(id) = target {
        crate::review::open_card(ui, id);
        return;
    }
    let CompareTarget::Checkout {
        project,
        cwd,
        base,
        branch,
        three_dot,
    } = target
    else {
        unreachable!()
    };
    crate::navigation::checkpoint(ui);
    batch(|| {
        ui.project.set(project.clone());
        ui.review_state.checkout_cwd.set(cwd.clone());
        ui.review_state.log_branch.set(branch.clone());
        ui.review_state.three_dot.set(*three_dot);
        crate::review::select_git_pair(ui, base.clone(), "WORKDIR");
        ui.navigate("review");
    });
    crate::navigation::checkpoint(ui);
    crate::review::load_refs(ui);
    crate::review::git_ops::load_branch_log(ui, branch.clone(), false);
}

pub fn open_diff(ui: Ui, wt: &ManagedWorktree) {
    crate::navigation::checkpoint(ui);
    ui.project.set(wt.project_id.clone());
    crate::review::git_ops::select_checkout(ui, wt.path.clone());
    ui.navigate("review");
    crate::navigation::checkpoint(ui);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitSyncTarget {
    pub host_id: String,
    pub project_id: String,
    pub card_id: Option<String>,
    pub cwd: String,
    pub branch: String,
    pub scope_key: String,
}

impl GitSyncTarget {
    pub fn sync_key(&self) -> String {
        format!("{}:{}", self.host_id, self.cwd)
    }
}

pub fn git_sync_target(s: &Snapshot, e: &Entry) -> Option<GitSyncTarget> {
    let loc = e.location.as_ref()?;
    let branch = loc
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())?
        .to_owned();
    loc.cwd.as_ref()?;
    let scope = entry_scope(s, e);
    let scope_key = scope.recent_key();
    match scope {
        WorktreeScope::CardWorktree(id) => {
            let card = s.cards.iter().find(|c| c.id == id)?;
            let cwd = loc
                .directory_root
                .clone()
                .or_else(|| card.worktree.clone())
                .or_else(|| loc.cwd.clone())?;
            Some(GitSyncTarget {
                host_id: e.host_id.clone(),
                project_id: card.project_id.clone(),
                card_id: Some(card.id.clone()),
                cwd,
                branch,
                scope_key,
            })
        }
        WorktreeScope::ManagedWorktree(id) => {
            let wt = s.worktrees.iter().find(|w| w.id == id)?;
            Some(GitSyncTarget {
                host_id: e.host_id.clone(),
                project_id: wt.project_id.clone(),
                card_id: None,
                cwd: wt.path.clone(),
                branch,
                scope_key,
            })
        }
        WorktreeScope::ProjectRoot(id) => {
            let proj = s.projects.iter().find(|p| p.id == id)?;
            Some(GitSyncTarget {
                host_id: e.host_id.clone(),
                project_id: proj.id.clone(),
                card_id: None,
                cwd: proj.directory.clone(),
                branch,
                scope_key,
            })
        }
        WorktreeScope::Directory {
            path,
            project: Some(pid),
            ..
        } => s
            .projects
            .iter()
            .any(|p| p.id == pid)
            .then(|| GitSyncTarget {
                host_id: e.host_id.clone(),
                project_id: pid,
                card_id: None,
                cwd: path,
                branch,
                scope_key,
            }),
        WorktreeScope::Directory { project: None, .. } | WorktreeScope::Unassigned(_) => None,
    }
}

pub fn sync_git_button_hint(
    branch: &str,
    ahead: u32,
    behind: u32,
    syncing: bool,
    offline: bool,
) -> String {
    if syncing {
        format!("Syncing Git branch {branch} (pull & push)…")
    } else if offline {
        format!("Sync Git branch {branch} unavailable: host is offline")
    } else if ahead > 0 && behind > 0 {
        format!("Sync Git branch {branch} (pull ↓{behind} & push ↑{ahead})")
    } else if behind > 0 {
        format!("Sync Git branch {branch} (pull ↓{behind} & push)")
    } else if ahead > 0 {
        format!("Sync Git branch {branch} (pull & push ↑{ahead})")
    } else {
        format!("Sync Git branch {branch} (pull & push)")
    }
}

pub fn format_git_sync_summary(val: &Value, fallback_branch: &str) -> String {
    let branch = val["branch"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_branch);
    let remote = val["remote"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("origin");
    let remote_branch = val["remote_branch"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(branch);
    let pulled = val["fast_forwarded"].as_bool().unwrap_or(false);
    let pulled_n = val["pulled_commits"].as_u64().unwrap_or(0);
    let pushed = val["pushed"].as_bool().unwrap_or(false);
    let pushed_n = val["pushed_commits"].as_u64().unwrap_or(0);
    let published =
        val["set_upstream"].as_bool().unwrap_or(false) && !val["pulled"].as_bool().unwrap_or(true);

    if published {
        format!("Published {branch} to {remote}/{remote_branch}")
    } else if pulled && pushed {
        let pull_word = if pulled_n == 1 { "commit" } else { "commits" };
        let push_word = if pushed_n == 1 { "commit" } else { "commits" };
        format!("Synced {branch}: pulled {pulled_n} {pull_word}, pushed {pushed_n} {push_word}")
    } else if pulled {
        let word = if pulled_n == 1 { "commit" } else { "commits" };
        format!("Pulled {pulled_n} {word} into {branch}")
    } else if pushed {
        let word = if pushed_n == 1 { "commit" } else { "commits" };
        format!("Pushed {pushed_n} {word} from {branch} to {remote}/{remote_branch}")
    } else {
        format!("{branch} is already up to date")
    }
}

pub fn sync_git_target(ui: Ui, target: GitSyncTarget) {
    if ui.page.get_untracked() == "terminals" {
        ui.terminal_focus.update(|v| *v += 1);
    }
    let offline = ui
        .snapshot
        .with_untracked(|s| s.is_host_offline(&target.host_id));
    if offline {
        ui.notify(format!(
            "Sync Git branch {} unavailable: host is offline",
            target.branch
        ));
        return;
    }
    let key = target.sync_key();
    if ui
        .terminal_git_syncing
        .with_untracked(|set| set.contains(&key))
    {
        return;
    }
    if !crate::backend::is_connected() {
        let scope_key = target.scope_key.clone();
        let target_cwd = target.cwd.clone();
        let target_branch = target.branch.clone();
        let mut had_ahead = false;
        let mut had_behind = false;
        let mut ahead_n = 0u32;
        let mut behind_n = 0u32;
        let target_host = target.host_id.clone();
        ui.snapshot.update(|s| {
            let entries_now = super::entries(s);
            let mut matching_ids: std::collections::BTreeSet<String> = entries_now
                .into_iter()
                .filter(|e| {
                    e.host_id == target_host
                        && (entry_scope(s, e).recent_key() == scope_key
                            || e.location
                                .as_ref()
                                .and_then(|l| l.directory_root.as_deref().or(l.cwd.as_deref()))
                                == Some(target_cwd.as_str()))
                })
                .map(|e| e.saved.id)
                .collect();
            for ctx in &s.terminal_contexts {
                if s.terminal_host(&ctx.terminal_id) == target_host
                    && (ctx.directory_root.as_deref() == Some(target_cwd.as_str())
                        || ctx.cwd.as_deref() == Some(target_cwd.as_str()))
                {
                    matching_ids.insert(ctx.terminal_id.clone());
                }
            }
            for ctx in &mut s.terminal_contexts {
                if matching_ids.contains(&ctx.terminal_id) {
                    if ctx.ahead > 0 {
                        had_ahead = true;
                        ahead_n = ahead_n.max(ctx.ahead);
                        ctx.ahead = 0;
                    }
                    if ctx.behind > 0 {
                        had_behind = true;
                        behind_n = behind_n.max(ctx.behind);
                        ctx.behind = 0;
                    }
                }
            }
        });
        let msg = if had_ahead && had_behind {
            format!("Synced {target_branch}: pulled {behind_n}, pushed {ahead_n} (Gallery preview)")
        } else if had_behind {
            format!("Pulled {behind_n} into {target_branch} (Gallery preview)")
        } else if had_ahead {
            format!("Pushed {ahead_n} from {target_branch} (Gallery preview)")
        } else {
            format!("{target_branch} is already up to date (Gallery preview)")
        };
        ui.notify(msg);
        return;
    }

    ui.terminal_git_syncing.update(|set| {
        set.insert(key.clone());
    });
    ui.status
        .set(format!("Syncing Git branch {}…", target.branch));
    ui.operation_error.set(String::new());
    ui.review_state.read_cache.with(|cache| {
        cache.invalidate_checkout(
            &target.host_id,
            &target.project_id,
            &target.cwd,
            target.card_id.as_deref(),
        );
    });

    let mut params = json!({
        "project_id": target.project_id,
        "cwd": target.cwd,
        "request_key": canban_types::id(),
    });
    if let Some(cid) = &target.card_id {
        params["card_id"] = json!(cid);
    }
    let project_for_route = target.project_id.clone();

    ui.spawn(async move {
        let result =
            blocking(move || crate::routed_call("git.sync", params, Some(&project_for_route)))
                .await;
        if let Some(mut set) = ui.terminal_git_syncing.try_get() {
            set.remove(&key);
            let _ = ui.terminal_git_syncing.try_set(set);
        }
        let _ = ui.review_state.read_cache.try_with(|cache| {
            cache.invalidate_checkout(
                &target.host_id,
                &target.project_id,
                &target.cwd,
                target.card_id.as_deref(),
            );
        });
        match result {
            Ok(Ok(val)) => {
                ui.notify(format_git_sync_summary(&val, &target.branch));
                if ui.page.try_get() == Some("review".into())
                    && ui.project.try_get().as_deref() == Some(target.project_id.as_str())
                {
                    crate::review::load_refs(ui);
                    let branch = ui.review_state.log_branch.get_untracked();
                    crate::review::git_ops::load_branch_log(ui, branch, false);
                    crate::review::refresh_active_diff(ui, false);
                }
            }
            Ok(Err(e)) => {
                let err = format!("Git sync ({}): {e:#}", target.branch);
                let _ = ui.status.try_set(err.clone());
                let _ = ui.operation_error.try_set(err);
            }
            Err(e) => {
                let err = format!("Git sync ({}): {e}", target.branch);
                let _ = ui.status.try_set(err.clone());
                let _ = ui.operation_error.try_set(err);
            }
        }
    });
}

pub fn sync_current_git(ui: Ui) {
    let target = current(ui).and_then(|e| ui.snapshot.with_untracked(|s| git_sync_target(s, &e)));
    if let Some(target) = target {
        sync_git_target(ui, target);
    } else {
        ui.notify("Git sync unavailable: no registered Git branch checkout for this terminal");
    }
}

pub fn request_remove(ui: Ui, wt_id: String, force: bool) {
    if !crate::backend::is_connected() {
        ui.snapshot
            .update(|s| s.worktrees.retain(|w| w.id != wt_id));
        ui.worktree_remove_confirm.set(None);
        ui.notify("Worktree deleted");
        return;
    }
    let wid = wt_id.clone();
    dispatch_complete(
        ui,
        "worktree.remove",
        json!({"worktree_id": wt_id, "force": force}),
        move |res| match res {
            Ok(_) => {
                ui.worktree_remove_confirm.set(None);
            }
            Err(err) => {
                if !force && err.to_lowercase().contains("uncommitted") {
                    ui.operation_error.set(String::new());
                    ui.worktree_remove_confirm.set(Some(wid));
                }
            }
        },
    );
}

pub async fn menu(ui: Ui, wt: ManagedWorktree, at: Option<Point<Points>>) {
    let wt_shell = wt.clone();
    let wt_diff = wt.clone();
    let wt_promote = wt.id.clone();
    let wt_copy = wt.path.clone();
    let wt_del = wt.id.clone();

    let mut items = vec![
        MenuItem::new(format!("⎇ {} (base: {})", wt.branch, wt.base_branch)).disabled(true),
        MenuItem::new(wt.path.clone()).disabled(true),
        MenuItem::separator(),
        MenuItem::new("Open Shell in Worktree").on_select(move || open_shell(ui, &wt_shell)),
    ];

    for p in ui.profiles.get_untracked() {
        let wt_agent = wt.clone();
        let pid = p.id.clone();
        items.push(
            MenuItem::new(format!("Launch @{} CLI in Worktree", p.id))
                .on_select(move || open_agent(ui, &wt_agent, &pid)),
        );
    }

    items.push(MenuItem::separator());
    items.push(MenuItem::new("Open in Git").on_select(move || open_diff(ui, &wt_diff)));
    items.push(
        MenuItem::new("Promote to Board Card…")
            .on_select(move || ui.worktree_promote.set(Some(wt_promote.clone()))),
    );
    items.push(
        MenuItem::new("Copy Worktree Path").on_select(move || copy_text(ui, wt_copy.clone())),
    );
    items.push(MenuItem::separator());
    items.push(
        MenuItem::new("Delete Worktree…")
            .on_select(move || request_remove(ui, wt_del.clone(), false)),
    );

    if let Some(pos) = at {
        popup_menu_at(items, pos).await;
    } else {
        popup_menu(items).await;
    }
    ui.terminal_focus.update(|v| *v += 1);
}

pub fn modals(ui: Ui) -> impl IntoElement {
    view()
        .child(create_modal(ui))
        .child(promote_modal(ui))
        .child(remove_confirm_modal(ui))
}

fn create_modal(ui: Ui) -> impl IntoElement {
    when(
        move || ui.worktree_modal.get().is_some(),
        move || {
            let initial_project = ui
                .worktree_modal
                .get_untracked()
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    let cur = ui.project.get_untracked();
                    if !cur.is_empty() {
                        Some(cur)
                    } else {
                        ui.snapshot
                            .with_untracked(|s| s.projects.first().map(|p| p.id.clone()))
                    }
                })
                .unwrap_or_default();
            let project_id = signal(initial_project);
            let branch = signal(String::new());
            let base_branch = signal(String::new());
            let sync_base = signal(false);
            let launch_mode = signal("shell".to_owned());
            let profile_id = signal(crate::sessions::default_profile(ui));
            let error = signal(String::new());
            let refs_data = signal(Value::Null);
            let refs_loading = signal(false);
            let refs_ticket = signal(0u64);
            let prev_project = stored(String::new());

            effect(move || {
                let pid = project_id.get();
                let previous = prev_project.replace(pid.clone());
                if !previous.is_empty() && previous != pid {
                    base_branch.set(String::new());
                }
                if pid.is_empty() {
                    refs_data.set(Value::Null);
                    refs_loading.set(false);
                    return;
                }
                if !crate::backend::is_connected() {
                    refs_data.set(ui.review_state.refs_data.get_untracked());
                    refs_loading.set(false);
                    return;
                }
                let offline = untracked(|| {
                    ui.snapshot
                        .with(|s| s.is_host_offline(s.project_host(&pid)))
                });
                if offline {
                    refs_data.set(Value::Null);
                    refs_loading.set(false);
                    return;
                }
                let ticket = refs_ticket.get_untracked() + 1;
                refs_ticket.set(ticket);
                refs_data.set(Value::Null);
                refs_loading.set(true);
                ui.spawn(async move {
                    let result =
                        crate::review::cached_read(ui, "git.refs", json!({"project_id": pid}), pid)
                            .await;
                    if refs_ticket.try_get() != Some(ticket) {
                        return;
                    }
                    let _ = refs_loading.try_set(false);
                    if let Ok(value) = result {
                        let _ = refs_data.try_set(value.data.clone());
                    }
                });
            });

            let close_modal = move || {
                if ui.is_method_pending("worktree.create") {
                    return;
                }
                ui.worktree_modal.set(None);
                ui.terminal_focus.update(|n| *n += 1);
            };

            let submit = move || {
                if ui.is_method_pending("worktree.create") {
                    return;
                }
                let pid = project_id.get_untracked();
                let br = branch.get_untracked().trim().to_owned();
                if pid.is_empty() {
                    error.set("Choose a Project first".into());
                    return;
                }
                if br.is_empty() {
                    error.set("Enter a Git branch name (for example feat/my-branch)".into());
                    return;
                }
                error.set(String::new());
                let base = base_branch.get_untracked().trim().to_owned();
                let sync = sync_base.get_untracked();
                let launch = launch_mode.get_untracked();
                let prof = profile_id.get_untracked();

                if !crate::backend::is_connected() {
                    let proj = ui
                        .snapshot
                        .with_untracked(|s| s.projects.iter().find(|p| p.id == pid).cloned());
                    let fallback_base = refs_data
                        .with_untracked(|refs| effective_default_base(proj.as_ref(), refs))
                        .unwrap_or_else(|| "main".into());
                    let wt = ManagedWorktree {
                        id: format!("wt-{}", canban_types::id()),
                        project_id: pid.clone(),
                        branch: br.clone(),
                        base_branch: if base.is_empty() { fallback_base } else { base },
                        base_commit: Some("0123456789abcdef".into()),
                        path: format!("/preview/worktrees/{br}"),
                        directory: "/preview/source".into(),
                        created_at: canban_types::now(),
                    };
                    let wt_open = wt.clone();
                    batch(|| {
                        ui.snapshot.update(|s| s.worktrees.push(wt));
                        ui.worktree_modal.set(None);
                        ui.navigate("terminals");
                    });
                    if launch != "none" {
                        open_shell(ui, &wt_open);
                    }
                    return;
                }

                dispatch_complete(
                    ui,
                    "worktree.create",
                    json!({
                        "project_id": pid,
                        "branch": br,
                        "base_branch": (!base.is_empty()).then_some(base),
                        "sync_base": sync,
                        "launch": launch,
                        "profile_id": (launch == "agent").then_some(prof),
                    }),
                    move |res| match res {
                        Ok(val) => {
                            batch(|| {
                                ui.worktree_modal.set(None);
                                if let Some(tid) = val["terminal"]["id"].as_str() {
                                    ui.open_terminal(tid.to_owned());
                                } else {
                                    ui.navigate("terminals");
                                }
                            });
                        }
                        Err(e) => {
                            let _ = error.try_set(e);
                        }
                    },
                );
            };
            let submit_enter = submit;
            let proj_anchor = element_ref();

            nsh_base::dialog()
                .animation(canban_ui_kit::motion::modal)
                .close_on_escape(true)
                .close_on_backdrop_press(true)
                .on_cancel(close_modal)
                .child(
                    view()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x00000055))
                        .child(
                            panel()
                                .w(px(440.))
                                .max_w(Percentage(0.95))
                                .border_1()
                                .border_color(BORDER)
                                .gap_3()
                                .child(
                                    view()
                                        .flex_row()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            label("New Managed Branch Worktree")
                                                .font_weight(FontWeight::SEMIBOLD),
                                        )
                                        .child(chrome_button(
                                            Icon::Close,
                                            "Close worktree dialog",
                                            move |_| close_modal(),
                                        )),
                                )
                                .child(muted(
                                    "Create an isolated Git branch worktree managed by Canban without cluttering the Board.",
                                ))
                                .child(
                                    view()
                                        .gap_1p5()
                                        .child(label("Project"))
                                        .child(
                                            canban_ui_kit::press_target()
                                                .element_ref(&proj_anchor)
                                                .h(px(32.))
                                                .px_2p5()
                                                .rounded(px(RADIUS))
                                                .border_1()
                                                .border_color(BORDER)
                                                .bg(INSET)
                                                .hover(|s| s.bg(CARD_HOVER))
                                                .flex_row()
                                                .items_center()
                                                .justify_between()
                                                .child(label(move || {
                                                    let pid = project_id.get();
                                                    ui.snapshot
                                                        .with(|s| {
                                                            s.projects
                                                                .iter()
                                                                .find(|p| p.id == pid)
                                                                .map(|p| s.project_display_name(&p.id))
                                                        })
                                                        .unwrap_or_else(|| "Select Project".into())
                                                }))
                                                .child(icon(Icon::ChevronDown).w(px(13.)).h(px(13.)))
                                                .on_press(move |_: &()| {
                                                    let items_data = ui.snapshot.with_untracked(|s| {
                                                        s.projects
                                                            .iter()
                                                            .map(|p| {
                                                                let offline = s.is_host_offline(
                                                                    s.project_host(&p.id),
                                                                );
                                                                let mut label =
                                                                    s.project_display_name(&p.id);
                                                                if offline {
                                                                    label.push_str(" (offline)");
                                                                }
                                                                (p.id.clone(), label, offline)
                                                            })
                                                            .collect::<Vec<_>>()
                                                    });
                                                    if items_data.is_empty() {
                                                        return;
                                                    }
                                                    ui.spawn(async move {
                                                        let items: Vec<_> = items_data
                                                            .into_iter()
                                                            .map(|(id, label, offline)| {
                                                                MenuItem::new(label)
                                                                    .disabled(offline)
                                                                    .on_select(move || {
                                                                        let _ = project_id
                                                                            .try_set(id.clone());
                                                                    })
                                                            })
                                                            .collect();
                                                        if let Some(b) = proj_anchor.measure().await
                                                        {
                                                            popup_menu_at(
                                                                items,
                                                                point(
                                                                    b.page.x,
                                                                    b.page.y + b.frame.size.height,
                                                                ),
                                                            )
                                                            .await;
                                                        } else {
                                                            popup_menu(items).await;
                                                        }
                                                    });
                                                }),
                                        ),
                                )
                                .child(
                                    view()
                                        .gap_1p5()
                                        .child(label("Branch name"))
                                        .child(
                                            field(branch, "feat/my-spike")
                                                .label("Branch name")
                                                .auto_focus(true)
                                                .on_submit(move |_: &str| submit_enter()),
                                        ),
                                )
                                .child(
                                    view()
                                        .gap_1p5()
                                        .child(label("Base branch (optional)"))
                                        .child({
                                            let base_trigger = canban_ui_kit::press_target()
                                                .w_full()
                                                .h(px(32.))
                                                .px_2p5()
                                                .rounded(px(RADIUS))
                                                .border_1()
                                                .border_color(BORDER)
                                                .bg(INSET)
                                                .hover(|s| s.bg(CARD_HOVER))
                                                .flex_row()
                                                .items_center()
                                                .justify_between()
                                                .gap_2()
                                                .accessibility_label("Choose or type base branch")
                                                .child(
                                                    view()
                                                        .flex_row()
                                                        .items_center()
                                                        .gap_2()
                                                        .min_w(px(0.))
                                                        .child(
                                                            icon(Icon::Branch)
                                                                .w(px(13.))
                                                                .h(px(13.)),
                                                        )
                                                        .child(
                                                            label(move || {
                                                                let chosen = base_branch.get();
                                                                let chosen = chosen.trim();
                                                                if !chosen.is_empty() {
                                                                    return chosen.to_owned();
                                                                }
                                                                let pid = project_id.get();
                                                                let proj = ui.snapshot.with(|s| {
                                                                    s.projects
                                                                        .iter()
                                                                        .find(|p| p.id == pid)
                                                                        .cloned()
                                                                });
                                                                let refs = refs_data.get();
                                                                if let Some(def) =
                                                                    effective_default_base(
                                                                        proj.as_ref(),
                                                                        &refs,
                                                                    )
                                                                {
                                                                    format!("{def} (default)")
                                                                } else if refs_loading.get() {
                                                                    "Detecting default branch…"
                                                                        .into()
                                                                } else {
                                                                    "Repository default branch"
                                                                        .into()
                                                                }
                                                            })
                                                            .font_family(MONO_FONT)
                                                            .text_size(px(12.))
                                                            .truncate()
                                                            .min_w(px(0.)),
                                                        ),
                                                )
                                                .child(dynamic(move || {
                                                    if refs_loading.get() {
                                                        loading_spinner(12., MUTED).into_element()
                                                    } else {
                                                        icon(Icon::ChevronDown)
                                                            .w(px(13.))
                                                            .h(px(13.))
                                                            .into_element()
                                                    }
                                                }));
                                            search_popover(
                                                base_trigger,
                                                move || {
                                                    let pid = project_id.get();
                                                    let proj = ui.snapshot.with(|s| {
                                                        s.projects
                                                            .iter()
                                                            .find(|p| p.id == pid)
                                                            .cloned()
                                                    });
                                                    let refs = refs_data.get();
                                                    let cur = base_branch.get();
                                                    let def = effective_default_base(
                                                        proj.as_ref(),
                                                        &refs,
                                                    );
                                                    let remote = effective_sync_remote(
                                                        proj.as_ref(),
                                                        &refs,
                                                        &cur,
                                                    );
                                                    build_base_branch_options(
                                                        &refs,
                                                        def.as_deref(),
                                                        &remote,
                                                        &cur,
                                                    )
                                                },
                                                move |chosen| {
                                                    let pid = project_id.get_untracked();
                                                    let proj = ui.snapshot.with_untracked(|s| {
                                                        s.projects
                                                            .iter()
                                                            .find(|p| p.id == pid)
                                                            .cloned()
                                                    });
                                                    let def = refs_data.with_untracked(|refs| {
                                                        effective_default_base(proj.as_ref(), refs)
                                                    });
                                                    if chosen == DEFAULT_BASE_ID
                                                        || def.as_deref() == Some(chosen.as_str())
                                                    {
                                                        base_branch.set(String::new());
                                                    } else {
                                                        base_branch.set(chosen);
                                                    }
                                                },
                                            )
                                            .loading(move || refs_loading.get())
                                            .placeholder("Search branches or type ref…")
                                            .empty_text("No matching branches")
                                            .custom_option(|q| {
                                                let trimmed = q.trim();
                                                (!trimmed.is_empty()).then(|| {
                                                    PickerOption::new(
                                                        trimmed.to_owned(),
                                                        format!("Use branch \"{trimmed}\""),
                                                    )
                                                    .detail("custom")
                                                    .icon(Icon::Branch)
                                                    .mono(true)
                                                })
                                            })
                                            .width(392.)
                                            .list_height(260.)
                                        }),
                                )
                                .child({
                                    let sync_remote = computed(move || {
                                        let pid = project_id.get();
                                        let proj = ui.index.with(|idx| idx.projects.get(&pid).cloned());
                                        let refs = refs_data.get();
                                        let base = base_branch.get();
                                        effective_sync_remote(proj.as_ref(), &refs, &base)
                                    });
                                    dynamic(move || {
                                        let remote = sync_remote.get();
                                        checkbox(
                                            format!(
                                                "Sync base branch from `{remote}` before branching"
                                            ),
                                            sync_base,
                                            move |v| sync_base.set(*v),
                                        )
                                    })
                                })
                                .child(
                                    view()
                                        .gap_1p5()
                                        .child(label("Open with"))
                                        .child(
                                            segmented()
                                                .child(compact_segment(
                                                    "Shell",
                                                    Some(Icon::Terminal),
                                                    move || launch_mode.get() == "shell",
                                                    move |_| launch_mode.set("shell".into()),
                                                ))
                                                .child(compact_segment(
                                                    "Agent CLI",
                                                    Some(Icon::Agent),
                                                    move || launch_mode.get() == "agent",
                                                    move |_| launch_mode.set("agent".into()),
                                                ))
                                                .child(compact_segment(
                                                    "Create only",
                                                    None,
                                                    move || launch_mode.get() == "none",
                                                    move |_| launch_mode.set("none".into()),
                                                )),
                                        )
                                        .child(when(
                                            move || launch_mode.get() == "agent",
                                            move || {
                                                let hid = ui.snapshot.with(|s| {
                                                    s.project_host(&project_id.get()).to_owned()
                                                });
                                                let profiles =
                                                    crate::backend::config_for_host(&hid)
                                                        .profiles
                                                        .into_iter()
                                                        .filter(|p| p.enabled)
                                                        .collect::<Vec<_>>();
                                                let profiles = if profiles.is_empty() {
                                                    ui.profiles.get()
                                                } else {
                                                    profiles
                                                };
                                                segmented().children(
                                                    profiles
                                                        .into_iter()
                                                        .map(|p| {
                                                            let id = p.id.clone();
                                                            let check = id.clone();
                                                            compact_segment(
                                                                format!("@{}", p.id),
                                                                None,
                                                                move || profile_id.get() == check,
                                                                move |_| profile_id.set(id.clone()),
                                                            )
                                                            .into_element()
                                                        })
                                                        .collect::<Vec<_>>(),
                                                )
                                            },
                                        )),
                                )
                                .child(when(
                                    move || ui.is_method_pending("worktree.create"),
                                    move || {
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .child(live_dot(ACCENT))
                                            .child(muted(move || {
                                                if sync_base.get() {
                                                    "Syncing base branch, creating Git worktree and starting terminal…"
                                                } else if launch_mode.get() == "none" {
                                                    "Creating Git branch worktree…"
                                                } else {
                                                    "Creating Git branch worktree and starting terminal…"
                                                }
                                            }))
                                    },
                                ))
                                .child(when(
                                    move || !error.get().is_empty(),
                                    move || notice(move || error.get(), Tone::Attention),
                                ))
                                .child(
                                    view()
                                        .flex_row()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            action("Cancel", Variant::Quiet, move |_| close_modal())
                                                .disabled(move || {
                                                    ui.is_method_pending("worktree.create")
                                                }),
                                        )
                                        .child(
                                            action(
                                                move || {
                                                    if ui.is_method_pending("worktree.create") {
                                                        if sync_base.get() {
                                                            "Syncing & creating…"
                                                        } else {
                                                            "Creating…"
                                                        }
                                                    } else {
                                                        "Create Worktree"
                                                    }
                                                },
                                                Variant::Primary,
                                                move |_| submit(),
                                            )
                                            .disabled(move || ui.is_method_pending("worktree.create")),
                                        ),
                                ),
                        ),
                )
        },
    )
}

fn promote_modal(ui: Ui) -> impl IntoElement {
    when(
        move || ui.worktree_promote.get().is_some(),
        move || {
            let wt_id = ui.worktree_promote.get_untracked().unwrap_or_default();
            let wt = ui
                .snapshot
                .with_untracked(|s| s.worktrees.iter().find(|w| w.id == wt_id).cloned());
            let initial_title = wt
                .as_ref()
                .map(|w| w.branch.replace(['/', '-', '_'], " "))
                .unwrap_or_default();
            let branch_label = wt.as_ref().map(|w| w.branch.clone()).unwrap_or_default();
            let title = signal(initial_title);
            let body = signal(String::new());
            let error = signal(String::new());

            let close_modal = move || {
                ui.worktree_promote.set(None);
                ui.terminal_focus.update(|n| *n += 1);
            };

            let submit = move || {
                let t = title.get_untracked().trim().to_owned();
                if t.is_empty() {
                    error.set("Card title is required".into());
                    return;
                }
                let b = body.get_untracked();
                if !crate::backend::is_connected() {
                    ui.worktree_promote.set(None);
                    ui.notify("Promoted worktree to Card");
                    return;
                }
                let wid = wt_id.clone();
                dispatch_complete(
                    ui,
                    "worktree.promote",
                    json!({"worktree_id": wid, "title": t, "body": b}),
                    move |res| match res {
                        Ok(val) => {
                            ui.worktree_promote.set(None);
                            if let Some(cid) = val["id"].as_str() {
                                ui.open_card(cid.to_owned());
                            }
                        }
                        Err(e) => {
                            let _ = error.try_set(e);
                        }
                    },
                );
            };
            let submit_enter = submit.clone();

            nsh_base::dialog()
                .animation(canban_ui_kit::motion::modal)
                .close_on_escape(true)
                .close_on_backdrop_press(true)
                .on_cancel(close_modal)
                .child(
                    view()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x00000055))
                        .child(
                            panel()
                                .w(px(440.))
                                .max_w(Percentage(0.95))
                                .border_1()
                                .border_color(BORDER)
                                .gap_3()
                                .child(
                                    view()
                                        .flex_row()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            label("Promote Worktree to Board Card")
                                                .font_weight(FontWeight::SEMIBOLD),
                                        )
                                        .child(chrome_button(
                                            Icon::Close,
                                            "Close promote dialog",
                                            move |_| close_modal(),
                                        )),
                                )
                                .child(muted(format!(
                                    "Transfer branch `{branch_label}` and all uncommitted files directly to a new Board Card."
                                )))
                                .child(
                                    field(title, "Card title")
                                        .label("Card title")
                                        .auto_focus(true)
                                        .on_submit(move |_: &str| submit_enter()),
                                )
                                .child(
                                    field(body, "Optional task context…").label("Description"),
                                )
                                .child(when(
                                    move || !error.get().is_empty(),
                                    move || notice(move || error.get(), Tone::Attention),
                                ))
                                .child(
                                    view()
                                        .flex_row()
                                        .justify_end()
                                        .gap_2()
                                        .child(action("Cancel", Variant::Quiet, move |_| {
                                            close_modal()
                                        }))
                                        .child(
                                            action(
                                                "Create Card",
                                                Variant::Primary,
                                                move |_| submit(),
                                            )
                                            .disabled(move || {
                                                ui.is_method_pending("worktree.promote")
                                            }),
                                        ),
                                ),
                        ),
                )
        },
    )
}

fn remove_confirm_modal(ui: Ui) -> impl IntoElement {
    when(
        move || ui.worktree_remove_confirm.get().is_some(),
        move || {
            let wt_id = ui
                .worktree_remove_confirm
                .get_untracked()
                .unwrap_or_default();
            let branch = ui
                .snapshot
                .with_untracked(|s| {
                    s.worktrees
                        .iter()
                        .find(|w| w.id == wt_id)
                        .map(|w| w.branch.clone())
                })
                .unwrap_or_else(|| "worktree".into());
            let close_modal = move || {
                ui.worktree_remove_confirm.set(None);
                ui.terminal_focus.update(|n| *n += 1);
            };
            nsh_base::dialog()
                .animation(canban_ui_kit::motion::modal)
                .close_on_escape(true)
                .close_on_backdrop_press(true)
                .on_cancel(close_modal)
                .child(
                    view()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x00000055))
                        .child(
                            panel()
                                .w(px(400.))
                                .max_w(Percentage(0.95))
                                .border_1()
                                .border_color(BORDER)
                                .gap_3()
                                .child(
                                    label("Delete Dirty Worktree?")
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(notice(
                                    format!(
                                        "Worktree `{branch}` has uncommitted changes. Deleting it permanently discards uncommitted files on disk."
                                    ),
                                    Tone::Danger,
                                ))
                                .child(
                                    view()
                                        .flex_row()
                                        .justify_end()
                                        .gap_2()
                                        .child(action("Cancel", Variant::Quiet, move |_| {
                                            close_modal()
                                        }))
                                        .child(
                                            action(
                                                "Force Delete",
                                                Variant::Primary,
                                                move |_| request_remove(ui, wt_id.clone(), true),
                                            )
                                            .disabled(move || {
                                                ui.is_method_pending("worktree.remove")
                                            }),
                                        ),
                                ),
                        ),
                )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::Workflow;
    use serde_json::json;

    fn sample_project(target_branch: Option<&str>, base_branch: Option<&str>) -> Project {
        let mut workflow = Workflow::default();
        workflow.workspace.base_branch = base_branch.map(str::to_owned);
        Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: Default::default(),
            id: "proj-1".into(),
            name: "Fiber".into(),
            directory: "/tmp/fiber".into(),
            workflow,
            target_branch: target_branch.map(str::to_owned),
            created_at: 1,
        }
    }

    #[test]
    fn effective_default_base_and_sync_remote_follow_repo_and_remote_prefixes() {
        let proj = sample_project(Some("lsp-support"), None);
        let refs = json!({
            "default_branch": "develop",
            "remote_name": "origin",
            "sync_remote": "upstream",
            "branches": [
                {"name": "develop", "kind": "local", "short_sha": "aaa1111", "is_current": false, "is_default": true},
                {"name": "lsp-support", "kind": "local", "short_sha": "bbb2222", "is_current": true, "is_default": false},
                {"name": "upstream/develop", "kind": "remote", "short_sha": "aaa1111", "is_current": false, "is_default": false},
                {"name": "origin/develop", "kind": "remote", "short_sha": "ccc3333", "is_current": false, "is_default": false},
            ]
        });

        assert_eq!(
            effective_default_base(Some(&proj), &refs).as_deref(),
            Some("develop")
        );
        let overridden = sample_project(Some("lsp-support"), Some("release"));
        assert_eq!(
            effective_default_base(Some(&overridden), &refs).as_deref(),
            Some("release")
        );
        assert_eq!(effective_sync_remote(Some(&proj), &refs, ""), "upstream");
        assert_eq!(
            effective_sync_remote(Some(&proj), &refs, "origin/develop"),
            "origin"
        );
        assert_eq!(
            effective_sync_remote(Some(&proj), &refs, "upstream/develop"),
            "upstream"
        );

        let opts = build_base_branch_options(&refs, Some("develop"), "upstream", "");
        assert_eq!(opts[0].id, "develop");
        assert_eq!(opts[0].group.as_deref(), Some("Default"));
        assert_eq!(opts[0].detail, "default");
        assert!(opts[0].selected);
        assert!(
            opts.iter()
                .any(|o| o.id == "lsp-support" && o.group.as_deref() == Some("Local Branches"))
        );
        assert!(opts.iter().any(|o| {
            o.id == "upstream/develop"
                && o.group.as_deref() == Some("Remote Branches")
                && o.detail == "default remote"
        }));
    }
}
