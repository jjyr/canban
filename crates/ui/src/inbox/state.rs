//! Window-local Daily Inbox state, lazy terminal/GitHub previews, inline replies, and optimistic triage.
use crate::presenter::*;
use canban_notifications::{
    DailyInbox, GitHubInboxNotification, GitHubProjectStack, GitHubStackItem, InboxTerminalWork,
    defer_presets,
};
use canban_types::{GitHubReference, InboxEntry};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const MAX_DETAIL_CACHE: usize = 128;
const MAX_TERM_PREVIEW_CACHE: usize = 48;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitHubItemDetail {
    pub body: String,
    pub author_login: Option<String>,
    pub author_avatar_url: Option<String>,
    pub state: String,
    pub merged: bool,
    pub draft: bool,
    pub head_ref: Option<String>,
    pub base_ref: Option<String>,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub changed_files: Option<u32>,
    pub comments_count: Option<u32>,
    pub labels: Vec<String>,
    pub ci_state: Option<String>,
    pub ci_passed: u32,
    pub ci_total: u32,
    pub failed_checks: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplyTarget {
    Card {
        card_id: String,
        label: String,
    },
    Terminal {
        terminal_id: String,
        label: String,
    },
    GitHub {
        key: String,
        repository: String,
        kind: String,
        number: u64,
        url: String,
        label: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalPreviewState {
    pub lines: Vec<String>,
    pub source: String,
    pub cwd: Option<String>,
    pub loading: bool,
    pub fetched_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndoBatch {
    pub label: String,
    pub keys: Vec<String>,
    pub epoch: u64,
}

#[derive(Clone, Copy)]
pub struct State {
    pub drawer: Signal<String>,
    pub schedule_expanded: Signal<bool>,
    pub expanded_stacks: Signal<BTreeSet<String>>,
    pub expanded_terminals: Signal<BTreeSet<String>>,
    pub expanded_github_items: Signal<BTreeSet<String>>,
    pub github_notifications: Signal<Vec<GitHubInboxNotification>>,
    pub github_loading: Signal<bool>,
    pub github_error: Signal<String>,
    pub github_item_details: Signal<BTreeMap<String, GitHubItemDetail>>,
    pub github_item_loading: Signal<BTreeSet<String>>,
    pub terminal_previews: Signal<BTreeMap<String, TerminalPreviewState>>,
    pub reply_target: Signal<Option<ReplyTarget>>,
    pub reply_draft: Signal<String>,
    pub reply_sending: Signal<bool>,
    pub undo_banner: Signal<Option<UndoBatch>>,
    pub undo_epoch: Signal<u64>,
    pub gh_epoch: Signal<u64>,
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub fn new() -> Self {
        Self {
            drawer: signal("today".into()),
            schedule_expanded: signal(false),
            expanded_stacks: signal(BTreeSet::new()),
            expanded_terminals: signal(BTreeSet::new()),
            expanded_github_items: signal(BTreeSet::new()),
            github_notifications: signal(Vec::new()),
            github_loading: signal(false),
            github_error: signal(String::new()),
            github_item_details: signal(BTreeMap::new()),
            github_item_loading: signal(BTreeSet::new()),
            terminal_previews: signal(BTreeMap::new()),
            reply_target: signal(None),
            reply_draft: signal(String::new()),
            reply_sending: signal(false),
            undo_banner: signal(None),
            undo_epoch: signal(0),
            gh_epoch: signal(0),
        }
    }
}

pub fn local_tz_offset_secs() -> i64 {
    i64::from(chrono::Local::now().offset().local_minus_utc())
}

pub fn cutoff_minutes_from_utility(v: &Value) -> u16 {
    v["preferences"]["inbox_cutoff_minutes"]
        .as_u64()
        .filter(|&m| m < 1440)
        .map(|m| m as u16)
        .unwrap_or(0)
}

pub fn format_cutoff(minutes: u16) -> String {
    let h = minutes / 60;
    let m = minutes % 60;
    format!("{h:02}:{m:02}")
}

pub fn init(ui: Ui) {
    // Hydrate GitHub notifications on startup/connect AND whenever the user visits Inbox or gh state changes,
    // so the left-rail Inbox badge accurately reflects unread GitHub mentions/reviews immediately.
    effect(move || {
        let _ = ui.inbox.gh_epoch.get();
        let on_inbox = ui.page.get() == "inbox";
        let gh_connected = ui
            .github
            .gh_status
            .with(|s| s["state"].as_str() == Some("connected"));
        if crate::backend::is_connected() && (on_inbox || gh_connected) {
            untracked(|| {
                let empty = ui.inbox.github_notifications.with_untracked(Vec::is_empty);
                if on_inbox || empty {
                    refresh_github_notifications(ui, false);
                }
            });
        }
    });

    // Prefetch structured PR/Issue metadata (avatar, CI, comment count, labels) for visible
    // Daily Briefing GitHub items (bounded, cached by AppServer).
    effect(move || {
        if ui.page.get() != "inbox" || !crate::backend::is_connected() {
            return;
        }
        let inbox = ui.daily_inbox.get();
        let expanded = ui.inbox.expanded_stacks.get();
        untracked(|| {
            prefetch_visible_github_details(ui, &inbox, &expanded);
        });
    });
}

fn parse_rfc3339_ms(raw: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .and_then(|dt| u64::try_from(dt.timestamp_millis()).ok())
        .unwrap_or(0)
}

fn friendly_gh_reason(raw: &str) -> String {
    match raw {
        "review_requested" => "Review requested".into(),
        "mention" => "@mention".into(),
        "team_mention" => "Team mention".into(),
        "assign" => "Assigned".into(),
        "author" => "Author".into(),
        "comment" => "Comment".into(),
        "ci_activity" => "CI activity".into(),
        "state_change" => "State change".into(),
        "subscribed" => "Subscribed".into(),
        other if !other.is_empty() => other.replace('_', " "),
        _ => "Notification".into(),
    }
}

pub fn parse_github_browse_notifications(payload: &Value) -> Vec<GitHubInboxNotification> {
    let Some(rows) = payload["items"].as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for row in rows {
        let item = &row["item"];
        let thread_id = item["id"].as_str().unwrap_or("").trim();
        if thread_id.is_empty() {
            continue;
        }
        let repo = item["repository"]["full_name"]
            .as_str()
            .or_else(|| row["reference"]["repository"].as_str())
            .unwrap_or("")
            .trim();
        if repo.is_empty() {
            continue;
        }
        let title = item["subject"]["title"]
            .as_str()
            .unwrap_or("GitHub notification")
            .trim()
            .to_owned();
        let kind = row["reference"]["kind"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| match item["subject"]["type"].as_str() {
                Some("PullRequest") => "pr".into(),
                Some("Commit") => "commit".into(),
                _ => "issue".into(),
            });
        let number = row["reference"]["number"].as_u64();
        let url = row["reference"]["url"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("https://github.com/{repo}"));
        let reason = friendly_gh_reason(item["reason"].as_str().unwrap_or(""));
        let unread = item["unread"].as_bool().unwrap_or(true);
        let updated_at_ms = item["updated_at"]
            .as_str()
            .map(parse_rfc3339_ms)
            .unwrap_or(0);
        let author = item["subject"]["latest_comment_author"]
            .as_str()
            .or_else(|| item["author"]["login"].as_str())
            .map(str::to_owned);
        let linked = row["cards"].as_array().and_then(|arr| {
            arr.iter()
                .find(|c| c["archived"] != true)
                .or_else(|| arr.first())
        });
        let linked_card_id = linked.and_then(|c| c["id"].as_str()).map(str::to_owned);
        let linked_card_number = linked.and_then(|c| c["number"].as_u64());
        out.push(GitHubInboxNotification {
            thread_id: thread_id.to_owned(),
            repository: repo.to_owned(),
            kind,
            number,
            title,
            reason,
            unread,
            updated_at_ms,
            url,
            author,
            body: None,
            linked_card_id,
            linked_card_number,
        });
    }
    out
}

pub fn refresh_github_notifications(ui: Ui, refresh: bool) {
    let Some(backend) = crate::backend::get() else {
        return;
    };
    if ui.inbox.github_loading.get_untracked() {
        return;
    }
    ui.inbox.github_loading.set(true);
    ui.inbox.github_error.set(String::new());
    ui.spawn(async move {
        let res = blocking(move || {
            backend.call_local(
                "github.browse",
                json!({
                    "kind": "notifications",
                    "state": "inbox",
                    "relation": "all",
                    "page": 1,
                    "refresh": refresh,
                }),
            )
        })
        .await;
        let _ = ui.inbox.github_loading.try_set(false);
        match res {
            Ok(Ok(val)) => {
                let parsed = parse_github_browse_notifications(&val);
                let _ = ui.inbox.github_notifications.try_set(parsed);
            }
            Ok(Err(e)) => {
                let msg = format!("{e:#}");
                // Keep quiet when gh CLI simply isn't configured yet; UI shows a gentle connect hint.
                let _ = ui.inbox.github_error.try_set(msg);
            }
            Err(e) => {
                let _ = ui.inbox.github_error.try_set(e.to_string());
            }
        }
    });
}

fn parse_github_item_detail(overview: &Value, checks: Option<&Value>) -> GitHubItemDetail {
    let data = &overview["data"];
    let body = data["body"]
        .as_str()
        .map(str::trim)
        .unwrap_or("")
        .to_owned();
    let author_login = data["user"]["login"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let author_avatar_url = data["user"]["avatar_url"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let state = data["state"].as_str().unwrap_or("").to_owned();
    let merged = data["merged"].as_bool().unwrap_or(false)
        || data["merged_at"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty());
    let draft = data["draft"].as_bool().unwrap_or(false);
    let head_ref = data["head"]["ref"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let base_ref = data["base"]["ref"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let additions = data["additions"].as_u64().map(|n| n as u32);
    let deletions = data["deletions"].as_u64().map(|n| n as u32);
    let changed_files = data["changed_files"].as_u64().map(|n| n as u32);
    let comments_count = match (data["comments"].as_u64(), data["review_comments"].as_u64()) {
        (Some(c), Some(rc)) => Some((c + rc) as u32),
        (Some(c), None) => Some(c as u32),
        (None, Some(rc)) => Some(rc as u32),
        (None, None) => None,
    };
    let labels = data["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["name"].as_str().map(str::trim))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();

    let (ci_state, ci_passed, ci_total, failed_checks) =
        if let Some(runs) = checks.and_then(|c| c["data"]["check_runs"].as_array()) {
            let mut total = 0u32;
            let mut passed = 0u32;
            let mut running = 0u32;
            let mut failed = Vec::new();
            for run in runs {
                total += 1;
                let status = run["status"].as_str().unwrap_or("");
                let conclusion = run["conclusion"].as_str().unwrap_or("");
                if status != "completed" {
                    running += 1;
                } else if matches!(conclusion, "success" | "neutral" | "skipped") {
                    passed += 1;
                } else {
                    let name = run["name"].as_str().unwrap_or("check").to_owned();
                    failed.push(name);
                }
            }
            let st = if !failed.is_empty() {
                Some("failure".into())
            } else if running > 0 {
                Some("running".into())
            } else if total > 0 {
                Some("success".into())
            } else {
                Some("none".into())
            };
            (st, passed, total, failed)
        } else {
            (None, 0, 0, Vec::new())
        };

    GitHubItemDetail {
        body,
        author_login,
        author_avatar_url,
        state,
        merged,
        draft,
        head_ref,
        base_ref,
        additions,
        deletions,
        changed_files,
        comments_count,
        labels,
        ci_state,
        ci_passed,
        ci_total,
        failed_checks,
    }
}

pub fn prefetch_visible_github_details(
    ui: Ui,
    inbox: &DailyInbox,
    expanded_stacks: &BTreeSet<String>,
) {
    let mut budget = 14usize;
    let mut try_load = |item: &GitHubStackItem| {
        if budget == 0 || item.number.is_none() {
            return;
        }
        let cached = ui
            .inbox
            .github_item_details
            .with_untracked(|m| m.contains_key(&item.key));
        let loading = ui
            .inbox
            .github_item_loading
            .with_untracked(|s| s.contains(&item.key));
        if !cached && !loading {
            budget -= 1;
            load_github_item_detail(ui, item);
        }
    };
    for ny in &inbox.needs_you {
        if let canban_notifications::NeedsYouKind::GitHub { item, .. } = &ny.kind {
            try_load(item);
        }
    }
    for stack in &inbox.github_stacks {
        for item in &stack.items {
            try_load(item);
        }
    }
    for stack in &inbox.github_watching {
        if expanded_stacks.contains(&stack.stack_id) {
            for item in stack.items.iter().take(12) {
                try_load(item);
            }
        }
    }
    if expanded_stacks.contains("earlier-carryover-stack") {
        for item in inbox.earlier_github.iter().take(10) {
            try_load(item);
        }
    }
}

pub fn toggle_github_item(ui: Ui, item: &GitHubStackItem) {
    let key = item.key.clone();
    let was_open = ui
        .inbox
        .expanded_github_items
        .with_untracked(|s| s.contains(&key));
    if was_open {
        ui.inbox.expanded_github_items.update(|s| {
            s.remove(&key);
        });
    } else {
        ui.inbox.expanded_github_items.update(|s| {
            s.insert(key);
        });
        load_github_item_detail(ui, item);
    }
}

pub fn collapse_github_item(ui: Ui, key: &str) {
    ui.inbox.expanded_github_items.update(|s| {
        s.remove(key);
    });
}

pub fn load_github_item_detail(ui: Ui, item: &GitHubStackItem) {
    if ui
        .inbox
        .github_item_details
        .with_untracked(|m| m.contains_key(&item.key))
    {
        return;
    }
    if ui
        .inbox
        .github_item_loading
        .with_untracked(|s| s.contains(&item.key))
    {
        return;
    }
    let Some(number) = item.number else {
        return;
    };
    let Some(backend) = crate::backend::get() else {
        return;
    };
    let key = item.key.clone();
    let is_pr = item.kind == "pr";
    let reference = GitHubReference {
        repository: item.repository.clone(),
        kind: item.kind.clone(),
        number: Some(number),
        sha: None,
        url: item.url.clone(),
    };
    ui.inbox.github_item_loading.update(|s| {
        s.insert(key.clone());
    });
    ui.spawn(async move {
        let res = blocking(move || {
            let overview = backend.call_local(
                "github.detail",
                json!({
                    "reference": reference,
                    "section": "overview",
                    "refresh": false,
                }),
            )?;
            let checks = if is_pr
                && let Some(sha) = overview["data"]["head"]["sha"].as_str()
                && sha.len() == 40
                && sha.bytes().all(|b| b.is_ascii_hexdigit())
            {
                backend
                    .call_local(
                        "github.detail",
                        json!({
                            "reference": reference,
                            "section": "checks",
                            "head": sha,
                            "refresh": false,
                        }),
                    )
                    .ok()
            } else {
                None
            };
            Ok::<_, anyhow::Error>(parse_github_item_detail(&overview, checks.as_ref()))
        })
        .await;
        ui.inbox.github_item_loading.update(|s| {
            s.remove(&key);
        });
        if let Ok(Ok(detail)) = res {
            ui.inbox.github_item_details.update(|m| {
                if m.len() >= MAX_DETAIL_CACHE
                    && let Some(first) = m.keys().next().cloned()
                {
                    m.remove(&first);
                }
                m.insert(key, detail);
            });
        }
    });
}

pub fn prefetch_agent_terminal_previews(ui: Ui) {
    let inbox = ui.daily_inbox.get_untracked();
    for term in inbox.agent_terminals.iter().take(12) {
        let should_fetch = ui.inbox.terminal_previews.with_untracked(|m| {
            m.get(&term.terminal_id).is_none_or(|p| {
                !p.loading && canban_types::now().saturating_sub(p.fetched_at) > 15_000
            })
        });
        if should_fetch {
            fetch_terminal_preview(ui, term.terminal_id.clone(), 16);
        }
    }
}

pub fn fetch_terminal_preview(ui: Ui, terminal_id: String, lines: u64) {
    let Some(backend) = crate::backend::get() else {
        return;
    };
    ui.inbox.terminal_previews.update(|m| {
        if m.len() >= MAX_TERM_PREVIEW_CACHE
            && let Some(first) = m.keys().next().cloned()
        {
            m.remove(&first);
        }
        let entry = m
            .entry(terminal_id.clone())
            .or_insert(TerminalPreviewState {
                lines: Vec::new(),
                source: String::new(),
                cwd: None,
                loading: true,
                fetched_at: 0,
            });
        entry.loading = true;
    });
    let tid = terminal_id.clone();
    ui.spawn(async move {
        let res = blocking(move || {
            backend.call_routed(
                "terminal.preview",
                json!({ "terminal_id": tid, "lines": lines }),
                None,
            )
        })
        .await;
        let now_ms = canban_types::now();
        ui.inbox.terminal_previews.update(|m| {
            let entry = m.entry(terminal_id).or_insert(TerminalPreviewState {
                lines: Vec::new(),
                source: String::new(),
                cwd: None,
                loading: false,
                fetched_at: now_ms,
            });
            entry.loading = false;
            entry.fetched_at = now_ms;
            if let Ok(Ok(val)) = res {
                entry.lines = val["lines"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                entry.source = val["source"].as_str().unwrap_or("").to_owned();
                entry.cwd = val["cwd"].as_str().map(str::to_owned);
            }
        });
    });
}

fn apply_inbox_entries_local(ui: Ui, patches: &[Value]) -> Vec<InboxEntry> {
    let mut prev = Vec::new();
    let now_ms = canban_types::now();
    ui.snapshot.update(|s| {
        prev = s.inbox_entries.clone();
        for patch in patches {
            let Some(id) = patch["id"].as_str() else {
                continue;
            };
            let state = patch["state"].as_str().unwrap_or("");
            s.inbox_entries.retain(|e| e.id != id);
            if state != "active" {
                s.inbox_entries.push(InboxEntry {
                    id: id.to_owned(),
                    state: state.to_owned(),
                    defer_until: patch["defer_until"].as_u64(),
                    day_start_ms: patch["day_start_ms"].as_u64(),
                    watermark: patch["watermark"].as_u64(),
                    updated_at: now_ms,
                });
            }
        }
    });
    prev
}

fn show_undo_banner(ui: Ui, label: String, keys: Vec<String>) {
    if keys.is_empty() {
        return;
    }
    let epoch = ui.inbox.undo_epoch.get_untracked().wrapping_add(1);
    batch(|| {
        ui.inbox.undo_epoch.set(epoch);
        ui.inbox
            .undo_banner
            .set(Some(UndoBatch { label, keys, epoch }));
    });
    set_timeout(std::time::Duration::from_secs(6), move || {
        if ui.inbox.undo_epoch.try_get() == Some(epoch) {
            let _ = ui.inbox.undo_banner.try_set(None);
        }
    });
}

pub fn triage_items(ui: Ui, patches: Vec<Value>, undo_label: Option<String>) {
    if patches.is_empty() {
        return;
    }
    let keys: Vec<String> = patches
        .iter()
        .filter_map(|p| p["id"].as_str().map(str::to_owned))
        .collect();
    let prev = apply_inbox_entries_local(ui, &patches);
    if let Some(label) = undo_label {
        show_undo_banner(ui, label, keys);
    }
    if !crate::backend::is_connected() {
        return;
    }
    dispatch_complete(
        ui,
        "inbox.triage",
        json!({ "items": patches }),
        move |res| {
            if res.is_err() {
                ui.snapshot.update(|s| s.inbox_entries = prev);
            }
        },
    );
}

pub fn undo_last(ui: Ui) {
    let Some(batch_undo) = ui.inbox.undo_banner.get_untracked() else {
        return;
    };
    ui.inbox.undo_banner.set(None);
    let patches: Vec<Value> = batch_undo
        .keys
        .into_iter()
        .map(|id| json!({ "id": id, "state": "active" }))
        .collect();
    triage_items(ui, patches, None);
    ui.notify("Undone");
}

pub fn restore_item(ui: Ui, key: String) {
    triage_items(ui, vec![json!({ "id": key, "state": "active" })], None);
    ui.notify("Restored to Today");
}

pub fn pin_to_today(ui: Ui, key: String, day_start_ms: u64, watermark: Option<u64>) {
    triage_items(
        ui,
        vec![json!({
            "id": key,
            "state": "pinned_today",
            "day_start_ms": day_start_ms,
            "watermark": watermark,
        })],
        Some("Added to Today".into()),
    );
}

pub fn dismiss_items(ui: Ui, items: Vec<(String, Option<u64>)>, label: String) {
    let day_start = ui.daily_inbox.get_untracked().day_start_ms;
    let patches: Vec<Value> = items
        .into_iter()
        .map(|(id, wm)| {
            json!({
                "id": id,
                "state": "dismissed",
                "day_start_ms": day_start,
                "watermark": wm,
            })
        })
        .collect();
    triage_items(ui, patches, Some(label));
}

pub fn open_defer_menu(ui: Ui, items: Vec<(String, Option<u64>)>, subject: String) {
    if items.is_empty() {
        return;
    }
    use nsh_core::element::desktop::{MenuItem, popup_menu};
    let cutoff = cutoff_minutes_from_utility(&ui.utility.get_untracked());
    let presets = defer_presets(canban_types::now(), local_tz_offset_secs(), cutoff);
    ui.spawn(async move {
        popup_menu(
            presets
                .into_iter()
                .map(|preset| {
                    let items = items.clone();
                    let subject = subject.clone();
                    let label_text = preset.label.clone();
                    let until = preset.until_ms;
                    MenuItem::new(preset.label).on_select(move || {
                        let patches: Vec<Value> = items
                            .iter()
                            .map(|(id, wm)| {
                                json!({
                                    "id": id,
                                    "state": "deferred",
                                    "defer_until": until,
                                    "watermark": wm,
                                })
                            })
                            .collect();
                        triage_items(
                            ui,
                            patches,
                            Some(format!("Deferred \"{subject}\" → {label_text}")),
                        );
                    })
                })
                .collect(),
        )
        .await;
    });
}

pub fn defer_remaining_all(ui: Ui) {
    let inbox = ui.daily_inbox.get_untracked();
    let items = inbox.remaining_triage_keys();
    if items.is_empty() {
        ui.notify("All caught up for today");
        return;
    }
    let count = items.len();
    open_defer_menu(ui, items, format!("remaining {count} items"));
}

pub fn open_cutoff_menu(ui: Ui) {
    use nsh_core::element::desktop::{MenuItem, popup_menu};
    let options: &[(u16, &str)] = &[
        (0, "00:00 · Calendar day (default)"),
        (4 * 60, "04:00 · Late-night cutoff"),
        (6 * 60, "06:00 · Early morning cutoff"),
        (9 * 60, "09:00 · Workday start"),
        (10 * 60, "10:00 · Late morning cutoff"),
    ];
    let current = cutoff_minutes_from_utility(&ui.utility.get_untracked());
    ui.spawn(async move {
        popup_menu(
            options
                .iter()
                .map(|&(mins, label_text)| {
                    let prefix = if mins == current { "✓ " } else { "   " };
                    MenuItem::new(format!("{prefix}{label_text}")).on_select(move || {
                        crate::utilities::configure_preferences(
                            ui,
                            json!({ "inbox_cutoff_minutes": mins }),
                        );
                        ui.notify(format!("Day cutoff set to {}", format_cutoff(mins)));
                    })
                })
                .collect(),
        )
        .await;
    });
}

pub fn dismiss_github_item(ui: Ui, item: GitHubStackItem) {
    if let Some(tid) = item.thread_id.clone()
        && crate::backend::is_connected()
    {
        dispatch_complete(
            ui,
            "github.notification.done",
            json!({ "thread_id": tid }),
            |_| {},
        );
    }
    if let Some(cid) = item.card_id.clone()
        && item.read_sequence > 0
    {
        crate::overview::mark_card_read(ui, cid, item.read_sequence);
    }
    dismiss_items(
        ui,
        vec![(item.key, item.watermark)],
        format!("Marked read · {}", item.title),
    );
}

pub fn dismiss_github_stack(ui: Ui, stack: GitHubProjectStack) {
    let mut keys = Vec::new();
    for item in stack.items {
        if let Some(tid) = item.thread_id
            && crate::backend::is_connected()
        {
            dispatch_complete(
                ui,
                "github.notification.done",
                json!({ "thread_id": tid }),
                |_| {},
            );
        }
        if let Some(cid) = item.card_id
            && item.read_sequence > 0
        {
            crate::overview::mark_card_read(ui, cid, item.read_sequence);
        }
        keys.push((item.key, item.watermark));
    }
    let count = keys.len();
    dismiss_items(
        ui,
        keys,
        format!("Cleared GitHub · {} ({count})", stack.display_name),
    );
}

pub fn close_terminal_from_inbox(ui: Ui, terminal_id: String, title: String) {
    if !crate::backend::is_connected() {
        dismiss_items(
            ui,
            vec![(format!("terminal:{terminal_id}"), None)],
            format!("Closed {title}"),
        );
        return;
    }
    ui.closing_terminals.update(|s| {
        s.insert(terminal_id.clone());
    });
    let tid = terminal_id.clone();
    dispatch_complete(
        ui,
        "terminal.close",
        json!({ "terminal_id": terminal_id }),
        move |res| {
            ui.closing_terminals.update(|s| {
                s.remove(&tid);
            });
            if res.is_ok() {
                ui.notify(format!("Closed {title}"));
            }
        },
    );
}

pub fn close_all_plain_shells(ui: Ui, shells: Vec<InboxTerminalWork>) {
    let count = shells.len();
    for shell in shells {
        close_terminal_from_inbox(ui, shell.terminal_id, shell.title);
    }
    ui.notify(format!("Closing {count} plain shells"));
}

pub fn toggle_stack(ui: Ui, id: String) {
    ui.inbox.expanded_stacks.update(|set| {
        if !set.remove(&id) {
            set.insert(id);
        }
    });
}

pub fn start_inline_reply(ui: Ui, target: ReplyTarget) {
    let same = ui.inbox.reply_target.get_untracked().as_ref() == Some(&target);
    batch(|| {
        if same {
            ui.inbox.reply_target.set(None);
            ui.inbox.reply_draft.set(String::new());
        } else {
            ui.inbox.reply_target.set(Some(target));
            ui.inbox.reply_draft.set(String::new());
        }
    });
}

pub fn submit_inline_reply(ui: Ui) {
    let Some(target) = ui.inbox.reply_target.get_untracked() else {
        return;
    };
    let text = ui.inbox.reply_draft.get_untracked().trim().to_owned();
    if text.is_empty() || ui.inbox.reply_sending.get_untracked() {
        return;
    }
    if !crate::backend::is_connected() {
        batch(|| {
            ui.inbox.reply_target.set(None);
            ui.inbox.reply_draft.set(String::new());
        });
        ui.notify("Fixture: reply sent");
        return;
    }
    ui.inbox.reply_sending.set(true);
    match target {
        ReplyTarget::Card { card_id, .. } => {
            dispatch_complete(
                ui,
                "comment.add",
                json!({ "card_id": card_id, "body": text }),
                move |res| {
                    ui.inbox.reply_sending.set(false);
                    if res.is_ok() {
                        batch(|| {
                            ui.inbox.reply_target.set(None);
                            ui.inbox.reply_draft.set(String::new());
                        });
                        ui.notify("Reply sent");
                    }
                },
            );
        }
        ReplyTarget::Terminal { terminal_id, .. } => {
            let tid_refresh = terminal_id.clone();
            dispatch_complete(
                ui,
                "terminal.send",
                json!({ "terminal_id": terminal_id, "text": text, "submit": true }),
                move |res| {
                    ui.inbox.reply_sending.set(false);
                    if res.is_ok() {
                        batch(|| {
                            ui.inbox.reply_target.set(None);
                            ui.inbox.reply_draft.set(String::new());
                        });
                        ui.notify("Sent to terminal");
                        set_timeout(std::time::Duration::from_millis(600), move || {
                            fetch_terminal_preview(ui, tid_refresh, 16);
                        });
                    }
                },
            );
        }
        ReplyTarget::GitHub {
            repository,
            kind,
            number,
            url,
            ..
        } => {
            let reference = GitHubReference {
                repository,
                kind,
                number: Some(number),
                sha: None,
                url,
            };
            dispatch_complete(
                ui,
                "github.remote.comment",
                json!({ "reference": reference, "body": text }),
                move |res| {
                    ui.inbox.reply_sending.set(false);
                    if res.is_ok() {
                        batch(|| {
                            ui.inbox.reply_target.set(None);
                            ui.inbox.reply_draft.set(String::new());
                        });
                        ui.notify("GitHub comment posted");
                    }
                },
            );
        }
    }
}

pub fn format_relative(ts_ms: u64, now_ms: u64) -> String {
    if ts_ms == 0 {
        return String::new();
    }
    let elapsed_s = now_ms.saturating_sub(ts_ms) / 1000;
    if elapsed_s < 60 {
        "Just now".into()
    } else if elapsed_s < 3600 {
        format!("{}m ago", elapsed_s / 60)
    } else if elapsed_s < 86_400 {
        format!("{}h ago", elapsed_s / 3600)
    } else {
        format!("{}d ago", elapsed_s / 86_400)
    }
}

pub fn format_clock_hm(ts_ms: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ts_ms as i64)
        .single()
        .map(|dt| dt.format("%H:%M").to_string())
        .unwrap_or_default()
}

pub fn format_defer_until(ts_ms: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ts_ms as i64)
        .single()
        .map(|dt| dt.format("%b %d %H:%M").to_string())
        .unwrap_or_default()
}

/// Left-rail Inbox badge count: only items requiring direct action right now.
pub fn current_inbox_count(inbox: &DailyInbox) -> usize {
    inbox.actionable_count
}
