//! Pure, clock-injected 4-tier Daily Briefing projection engine.
//!
//! Signal tiers:
//! - Tier 1 (`needs_you`, 1:1 with `actionable_count` and the left-rail Inbox badge):
//!   Blocked/errored agents (card or standalone terminal), review/merge gates, explicit unread
//!   agent replies/escalations, failed scheduled runs, and unread direct GitHub blockers
//!   (live CI failure on your PR, `CHANGES_REQUESTED`, unread `review_requested`/`mention`/`assign`).
//! - Tier 2 (`github_stacks` direct digest, `schedule_pulse`/`schedule_stacks`, `open_cards`,
//!   `agent_terminals`, `plain_shells`): today's direct personal work counted in Today progress.
//! - Tier 3 (`github_watching`): passive repository watch feeds (`subscribed`, `ci_activity`,
//!   today-updated untouched synced cards) — excluded from rail badge, Today progress, and Defer rest.
//! - Tier 4 (`earlier_github`, `carryover_cards`): older unread GitHub items and previous cards.
use crate::{OverviewRow, overview_at};
use canban_types::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MS_PER_SEC: u64 = 1_000;
const SECS_PER_DAY: i64 = 86_400;

/// Compute `[day_start_ms, next_cutoff_ms)` in UTC epoch milliseconds for a given
/// `now_ms`, local timezone offset (`tz_offset_secs`), and configurable daily cutoff
/// (`cutoff_minutes` in `0..1440`, default `0` = `00:00` local natural day).
pub fn day_window_bounds(now_ms: u64, tz_offset_secs: i64, cutoff_minutes: u16) -> (u64, u64) {
    let cutoff_s = i64::from(cutoff_minutes.min(1439)) * 60;
    let now_s = (now_ms / MS_PER_SEC) as i64;
    let local_s = now_s.saturating_add(tz_offset_secs);
    let shifted = local_s.saturating_sub(cutoff_s);
    let day_index = shifted.div_euclid(SECS_PER_DAY);
    let day_start_local = day_index
        .saturating_mul(SECS_PER_DAY)
        .saturating_add(cutoff_s);
    let day_start_utc_s = day_start_local.saturating_sub(tz_offset_secs).max(0) as u64;
    let day_start_ms = day_start_utc_s.saturating_mul(MS_PER_SEC);
    let next_cutoff_ms = day_start_ms.saturating_add((SECS_PER_DAY as u64) * MS_PER_SEC);
    (day_start_ms, next_cutoff_ms)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferPreset {
    pub id: &'static str,
    pub label: String,
    pub until_ms: u64,
}

/// Deterministic defer targets relative to `now_ms`, local timezone offset, and cutoff.
pub fn defer_presets(now_ms: u64, tz_offset_secs: i64, cutoff_minutes: u16) -> Vec<DeferPreset> {
    let (_, next_cutoff_ms) = day_window_bounds(now_ms, tz_offset_secs, cutoff_minutes);
    let cutoff_h = cutoff_minutes / 60;
    let cutoff_m = cutoff_minutes % 60;
    let in_2h = now_ms.saturating_add(2 * 3600 * MS_PER_SEC);

    let now_s = (now_ms / MS_PER_SEC) as i64;
    let local_s = now_s.saturating_add(tz_offset_secs);
    let local_day = local_s.div_euclid(SECS_PER_DAY);
    let tonight_local_s = local_day * SECS_PER_DAY + 20 * 3600;
    let tonight_utc_ms = if tonight_local_s > local_s + 1800 {
        ((tonight_local_s - tz_offset_secs).max(0) as u64) * MS_PER_SEC
    } else {
        (((local_day + 1) * SECS_PER_DAY + 20 * 3600 - tz_offset_secs).max(0) as u64) * MS_PER_SEC
    };
    let tomorrow_9_utc_ms =
        (((local_day + 1) * SECS_PER_DAY + 9 * 3600 - tz_offset_secs).max(0) as u64) * MS_PER_SEC;
    // 1970-01-01 was Thursday (weekday 3 if Mon=0..Sun=6).
    let weekday = (local_day + 3).rem_euclid(7);
    let days_to_monday = if weekday == 0 { 7 } else { 7 - weekday };
    let next_monday_9_utc_ms =
        (((local_day + days_to_monday) * SECS_PER_DAY + 9 * 3600 - tz_offset_secs).max(0) as u64)
            * MS_PER_SEC;

    vec![
        DeferPreset {
            id: "next_cutoff",
            label: format!("Next cutoff ({cutoff_h:02}:{cutoff_m:02})"),
            until_ms: next_cutoff_ms,
        },
        DeferPreset {
            id: "2h",
            label: "In 2 hours".into(),
            until_ms: in_2h,
        },
        DeferPreset {
            id: "tonight",
            label: "Tonight 20:00".into(),
            until_ms: tonight_utc_ms,
        },
        DeferPreset {
            id: "tomorrow_9",
            label: "Tomorrow 09:00".into(),
            until_ms: tomorrow_9_utc_ms,
        },
        DeferPreset {
            id: "next_monday",
            label: "Next Monday 09:00".into(),
            until_ms: next_monday_9_utc_ms,
        },
    ]
}

/// Deterministic zero-dependency RFC3339 timestamp parser (UTC epoch milliseconds).
pub fn parse_rfc3339_ms(raw: &str) -> Option<u64> {
    let s = raw.trim();
    if s.len() < 19 {
        return None;
    }
    let b = s.as_bytes();
    if b[4] != b'-'
        || b[7] != b'-'
        || (b[10] != b'T' && b[10] != b't' && b[10] != b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: i64 = s[11..13].parse().ok()?;
    let minute: i64 = s[14..16].parse().ok()?;
    let second: i64 = s[17..19].parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let rest = &s[19..];
    let tz_part = if let Some(after_dot) = rest.strip_prefix('.') {
        let digits = after_dot.bytes().take_while(u8::is_ascii_digit).count();
        &after_dot[digits..]
    } else {
        rest
    };
    let tz_offset_secs: i64 = if tz_part.is_empty() || tz_part.eq_ignore_ascii_case("Z") {
        0
    } else if tz_part.len() >= 6 && (tz_part.starts_with('+') || tz_part.starts_with('-')) {
        let sign: i64 = if tz_part.starts_with('-') { -1 } else { 1 };
        let th: i64 = tz_part[1..3].parse().ok()?;
        let tm: i64 = tz_part[4..6].parse().ok()?;
        sign * (th * 3600 + tm * 60)
    } else {
        return None;
    };
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - tz_offset_secs;
    u64::try_from(secs).ok().map(|s| s.saturating_mul(1_000))
}

/// Deterministic UTC RFC3339 formatter (`YYYY-MM-DDTHH:MM:SSZ`) with zero external dependencies.
pub fn format_rfc3339_utc_ms(ms: u64) -> String {
    let total_secs = (ms / 1_000) as i64;
    let days = total_secs.div_euclid(86_400);
    let rem = total_secs.rem_euclid(86_400);
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn is_github_reason_review(reason: &str) -> bool {
    matches!(
        reason.to_ascii_lowercase().as_str(),
        "review_requested" | "review requested" | "changes_requested"
    )
}

fn is_github_reason_mention(reason: &str) -> bool {
    matches!(
        reason.to_ascii_lowercase().as_str(),
        "mention" | "team_mention" | "@mention" | "team mention"
    )
}

fn is_github_reason_assign(reason: &str) -> bool {
    matches!(reason.to_ascii_lowercase().as_str(), "assign" | "assigned")
}

fn is_github_reason_watching(reason: &str) -> bool {
    matches!(
        reason.to_ascii_lowercase().as_str(),
        "subscribed" | "ci_activity" | "ci activity" | "manual" | "synced"
    )
}

/// Lightweight GitHub notification input fed from `github.browse(kind="notifications")`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubInboxNotification {
    pub thread_id: String,
    pub repository: String,
    pub kind: String,
    pub number: Option<u64>,
    pub title: String,
    pub reason: String,
    pub unread: bool,
    pub updated_at_ms: u64,
    pub url: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub linked_card_id: Option<String>,
    #[serde(default)]
    pub linked_card_number: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpcomingScheduleItem {
    pub schedule_id: String,
    pub schedule_name: String,
    pub project_id: String,
    pub project_name: String,
    pub next_run_at: u64,
    pub cron: String,
    pub creates_card: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShippedTodayItem {
    pub key: String,
    pub is_pr: bool,
    pub title: String,
    pub repository: Option<String>,
    pub pr_number: Option<u64>,
    pub card_id: Option<String>,
    pub card_number: Option<u64>,
    pub project_name: String,
    pub author: Option<String>,
    pub url: Option<String>,
    pub shipped_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagePulseCount {
    pub stage_id: String,
    pub stage_name: String,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectPulseItem {
    pub project_id: String,
    pub project_name: String,
    pub host_label: Option<String>,
    pub stage_counts: Vec<StagePulseCount>,
    pub open_pr_count: usize,
    pub live_agent_count: usize,
    pub attention_count: usize,
    pub merged_today_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulePulseItem {
    pub run_id: String,
    pub schedule_id: String,
    pub schedule_name: String,
    pub project_id: String,
    pub project_name: String,
    pub state: String,
    pub scheduled_at: u64,
    pub completed_at: Option<u64>,
    pub creates_card: bool,
    pub card_id: Option<String>,
    pub card_number: Option<u64>,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub report_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NeedsYouKind {
    Card {
        card_id: String,
        card_number: u64,
        card_title: String,
        project_id: String,
        project_name: String,
        host_label: Option<String>,
        terminal_id: Option<String>,
        agent_engine: Option<String>,
        pr_number: Option<u64>,
        pr_checks_summary: Option<String>,
        pr_ci_state: Option<String>,
        branch: Option<String>,
        headline: String,
        detail: Option<String>,
        body: Option<String>,
        actor: Option<String>,
        is_review: bool,
        is_blocked: bool,
        unread_seq: u64,
        unread_count: usize,
        can_quick_merge: bool,
    },
    ScheduleFailed {
        run_id: String,
        schedule_id: String,
        schedule_name: String,
        project_id: String,
        project_name: String,
        host_label: Option<String>,
        state: String,
        summary: Option<String>,
        error: Option<String>,
        report_path: Option<String>,
        terminal_id: Option<String>,
    },
    Terminal {
        terminal: Box<InboxTerminalWork>,
    },
    GitHub {
        item: Box<GitHubStackItem>,
        project_id: Option<String>,
        project_name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeedsYouItem {
    pub key: String,
    pub watermark: Option<u64>,
    pub timestamp_ms: u64,
    pub kind: NeedsYouKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubStackItem {
    pub key: String,
    pub watermark: Option<u64>,
    pub repository: String,
    pub kind: String,
    pub number: Option<u64>,
    pub title: String,
    pub subtitle: String,
    pub body: Option<String>,
    pub author: Option<String>,
    pub reason: String,
    pub pr_state: Option<String>,
    pub ci_state: Option<String>,
    pub ci_passed: u32,
    pub ci_total: u32,
    pub failed_checks: Vec<String>,
    pub labels: Vec<String>,
    pub comments_count: Option<u32>,
    pub url: String,
    pub timestamp_ms: u64,
    pub thread_id: Option<String>,
    pub card_id: Option<String>,
    pub card_number: Option<u64>,
    pub read_sequence: u64,
    pub is_ci_failure: bool,
    pub is_actionable: bool,
    pub is_synced_card: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubProjectStack {
    pub stack_id: String,
    pub repository: String,
    pub project_id: Option<String>,
    pub display_name: String,
    pub items: Vec<GitHubStackItem>,
    pub authors: Vec<String>,
    pub preview: String,
    pub latest_at: u64,
    pub has_ci_failure: bool,
    pub ci_failed_count: usize,
    pub review_count: usize,
    pub mention_count: usize,
    pub synced_card_count: usize,
    pub watching_count: usize,
    pub actionable_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleStackCard {
    pub key: String,
    pub watermark: Option<u64>,
    pub run_id: String,
    pub card_id: String,
    pub card_number: u64,
    pub card_title: String,
    pub card_body: String,
    pub stage: String,
    pub project_id: String,
    pub project_name: String,
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleCardStack {
    pub stack_id: String,
    pub schedule_id: String,
    pub schedule_name: String,
    pub project_id: String,
    pub project_name: String,
    pub cards: Vec<ScheduleStackCard>,
    pub latest_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxCardWork {
    pub key: String,
    pub watermark: Option<u64>,
    pub card_id: String,
    pub card_number: u64,
    pub card_title: String,
    pub stage: String,
    pub project_id: String,
    pub project_name: String,
    pub branch: Option<String>,
    pub host_label: Option<String>,
    pub pr_summary: Option<String>,
    pub pr_ci_state: Option<String>,
    pub last_activity_at: u64,
    pub pinned_today: bool,
    pub has_live_agent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxTerminalWork {
    pub key: String,
    pub terminal_id: String,
    pub title: String,
    pub agent_name: Option<String>,
    pub agent_engine: Option<String>,
    pub status: HerdrStatus,
    pub agent_turn: Option<u64>,
    pub waiting_title: Option<String>,
    pub status_detail: Option<String>,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub host_label: String,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
    pub additions: u32,
    pub deletions: u32,
    pub ahead: u32,
    pub behind: u32,
    pub card_id: Option<String>,
    pub card_number: Option<u64>,
    pub card_title: Option<String>,
    pub shell_command: Option<String>,
    pub shell_exit_code: Option<i32>,
    pub started_at: Option<u64>,
    pub timestamp_ms: u64,
    pub is_agent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredInboxItem {
    pub key: String,
    pub title: String,
    pub context: String,
    pub kind_label: &'static str,
    pub defer_until: u64,
    pub card_id: Option<String>,
    pub terminal_id: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoneInboxItem {
    pub key: String,
    pub title: String,
    pub context: String,
    pub kind_label: &'static str,
    pub completed_at: u64,
    pub card_id: Option<String>,
    pub can_restore: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DailyInbox {
    pub day_start_ms: u64,
    pub next_cutoff_ms: u64,
    pub cutoff_minutes: u16,
    /// Strictly 1:1 with `needs_you.len()` (left-rail Inbox badge & KPI #1).
    pub actionable_count: usize,
    pub completed_today: usize,
    pub remaining_today: usize,
    pub total_today: usize,
    pub has_projects: bool,
    pub schedule_pulse: Vec<SchedulePulseItem>,
    pub upcoming_schedules: Vec<UpcomingScheduleItem>,
    pub project_pulses: Vec<ProjectPulseItem>,
    pub shipped_today: Vec<ShippedTodayItem>,
    /// Total merged PRs and resolved Cards over the preceding 7 full day windows (`[day_start - 7d, day_start)`).
    pub shipped_prev_7d_total: usize,
    /// Tier 1: All items currently demanding direct user action (1:1 with rail badge).
    pub needs_you: Vec<NeedsYouItem>,
    /// Tier 2: Today's direct non-blocking GitHub updates grouped by project/repo.
    pub github_stacks: Vec<GitHubProjectStack>,
    /// Tier 3: Passive repository watch feeds (`subscribed`, `ci_activity`, today synced cards)
    /// — strictly excluded from rail badge, Today progress, and Defer rest.
    pub github_watching: Vec<GitHubProjectStack>,
    pub schedule_stacks: Vec<ScheduleCardStack>,
    pub open_cards: Vec<InboxCardWork>,
    pub agent_terminals: Vec<InboxTerminalWork>,
    pub plain_shells: Vec<InboxTerminalWork>,
    /// Tier 4: Older non-actionable GitHub items (`< day_start_ms`) & carry-over cards.
    pub earlier_github: Vec<GitHubStackItem>,
    pub carryover_cards: Vec<InboxCardWork>,
    pub deferred_items: Vec<DeferredInboxItem>,
    pub done_today_items: Vec<DoneInboxItem>,
}

impl DailyInbox {
    /// Collect all currently active Tier 1 & Tier 2 triageable item keys for "Defer rest".
    /// Ongoing non-blocked terminals, passive `github_watching`, and `Earlier` are excluded.
    pub fn remaining_triage_keys(&self) -> Vec<(String, Option<u64>)> {
        let mut out = Vec::new();
        for item in &self.needs_you {
            out.push((item.key.clone(), item.watermark));
        }
        for stack in &self.github_stacks {
            for item in &stack.items {
                out.push((item.key.clone(), item.watermark));
            }
        }
        for stack in &self.schedule_stacks {
            for card in &stack.cards {
                out.push((card.key.clone(), card.watermark));
            }
        }
        for card in &self.open_cards {
            out.push((card.key.clone(), card.watermark));
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TriageDisposition {
    Active,
    Deferred(u64),
    DismissedToday,
    DismissedOlder,
    PinnedToday,
}

fn classify_triage(
    triage_map: &BTreeMap<&str, &InboxEntry>,
    key: &str,
    now_ms: u64,
    day_start_ms: u64,
    watermark: Option<u64>,
) -> TriageDisposition {
    let Some(entry) = triage_map.get(key) else {
        return TriageDisposition::Active;
    };
    match entry.state.as_str() {
        "deferred" => {
            if let Some(until) = entry.defer_until
                && until > now_ms
            {
                return TriageDisposition::Deferred(until);
            }
            TriageDisposition::Active
        }
        "dismissed" => {
            let superseded = watermark.is_some_and(|w| entry.watermark.is_some_and(|ew| w > ew));
            if superseded {
                TriageDisposition::Active
            } else if entry.updated_at >= day_start_ms {
                TriageDisposition::DismissedToday
            } else {
                TriageDisposition::DismissedOlder
            }
        }
        "pinned_today" => {
            if entry.day_start_ms == Some(day_start_ms) || entry.updated_at >= day_start_ms {
                TriageDisposition::PinnedToday
            } else {
                TriageDisposition::Active
            }
        }
        _ => TriageDisposition::Active,
    }
}

fn is_github_reminder(r: &Reminder) -> bool {
    r.event
        .as_ref()
        .is_some_and(|m| m.kind.starts_with("github."))
}

fn project_host_label(snapshot: &Snapshot, project_id: &str) -> Option<String> {
    if !snapshot.has_remote_hosts() {
        return None;
    }
    let hid = snapshot.project_host(project_id);
    if hid == "local" {
        None
    } else {
        Some(snapshot.host_display_name(hid).to_owned())
    }
}

fn terminal_host_label(snapshot: &Snapshot, terminal_id: &str) -> String {
    let hid = snapshot
        .entity_hosts
        .get(terminal_id)
        .map(String::as_str)
        .unwrap_or("local");
    if hid == "local" {
        "local".into()
    } else {
        snapshot.host_display_name(hid).to_owned()
    }
}

/// True when an `OverviewRow` genuinely demands direct user action right now
/// (excludes passive interactive `idle`/`done` card agents with no gate or attention).
fn row_needs_action_now(row: &OverviewRow) -> bool {
    if !row.actionable() {
        return false;
    }
    if row.card.human_gate.is_some() || row.card.attention.is_some() {
        return true;
    }
    if row.reasons.iter().any(|r| {
        matches!(
            r.source.as_str(),
            "review" | "human_gate" | "attention" | "recovery" | "delivery"
        )
    }) {
        return true;
    }
    if let Some(a) = row.agent()
        && (a.state == "blocked"
            || a.state == "failed"
            || a.lifecycle.waiting.is_some()
            || a.trigger.is_automatic())
    {
        return true;
    }
    false
}

fn push_into_gh_map(
    map: &mut BTreeMap<String, GitHubProjectStack>,
    stack_key: String,
    project_id: Option<String>,
    display_name: String,
    repository: String,
    item: GitHubStackItem,
) {
    let ts = item.timestamp_ms;
    let is_ci_failure = item.is_ci_failure;
    let author = item.author.clone();
    let stack = map
        .entry(stack_key.clone())
        .or_insert_with(|| GitHubProjectStack {
            stack_id: format!("github-stack:{stack_key}"),
            repository,
            project_id,
            display_name,
            items: Vec::new(),
            authors: Vec::new(),
            preview: String::new(),
            latest_at: ts,
            has_ci_failure: false,
            ci_failed_count: 0,
            review_count: 0,
            mention_count: 0,
            synced_card_count: 0,
            watching_count: 0,
            actionable_count: 0,
        });
    stack.latest_at = stack.latest_at.max(ts);
    stack.has_ci_failure |= is_ci_failure;
    if let Some(a) = author
        && !a.is_empty()
        && !stack.authors.contains(&a)
    {
        stack.authors.push(a);
    }
    stack.items.push(item);
}

fn finalize_gh_stacks(map: BTreeMap<String, GitHubProjectStack>) -> Vec<GitHubProjectStack> {
    let mut stacks: Vec<GitHubProjectStack> = map
        .into_values()
        .map(|mut stack| {
            stack.items.sort_by(|a, b| {
                let a_watch = is_github_reason_watching(&a.reason);
                let b_watch = is_github_reason_watching(&b.reason);
                b.is_ci_failure
                    .cmp(&a.is_ci_failure)
                    .then_with(|| b.is_actionable.cmp(&a.is_actionable))
                    .then_with(|| a_watch.cmp(&b_watch))
                    .then_with(|| b.timestamp_ms.cmp(&a.timestamp_ms))
                    .then_with(|| a.key.cmp(&b.key))
            });
            stack.ci_failed_count = stack.items.iter().filter(|i| i.is_ci_failure).count();
            stack.review_count = stack
                .items
                .iter()
                .filter(|i| !i.is_ci_failure && is_github_reason_review(&i.reason))
                .count();
            stack.mention_count = stack
                .items
                .iter()
                .filter(|i| !i.is_ci_failure && is_github_reason_mention(&i.reason))
                .count();
            stack.synced_card_count = stack.items.iter().filter(|i| i.is_synced_card).count();
            stack.watching_count = stack
                .items
                .iter()
                .filter(|i| {
                    !i.is_ci_failure && !i.is_actionable && is_github_reason_watching(&i.reason)
                })
                .count();
            stack.actionable_count = stack.items.iter().filter(|i| i.is_actionable).count();
            stack.has_ci_failure = stack.ci_failed_count > 0;
            stack.preview = stack
                .items
                .iter()
                .take(3)
                .map(|i| match i.number {
                    Some(n) if i.is_ci_failure => format!("#{n} CI failed"),
                    Some(n) => format!("#{n} {}", i.title),
                    None => i.title.clone(),
                })
                .collect::<Vec<_>>()
                .join(" · ");
            stack
        })
        .collect();
    stacks.sort_by(|a, b| {
        b.has_ci_failure
            .cmp(&a.has_ci_failure)
            .then_with(|| b.actionable_count.cmp(&a.actionable_count))
            .then_with(|| b.latest_at.cmp(&a.latest_at))
            .then_with(|| a.stack_id.cmp(&b.stack_id))
    });
    stacks
}

/// Pure, deterministic Daily Briefing projection from `Snapshot` + cached GitHub notifications.
pub fn build_daily_inbox(
    snapshot: &Snapshot,
    github_notifications: &[GitHubInboxNotification],
    now_ms: u64,
    tz_offset_secs: i64,
    cutoff_minutes: u16,
) -> DailyInbox {
    let (day_start_ms, next_cutoff_ms) = day_window_bounds(now_ms, tz_offset_secs, cutoff_minutes);
    let triage_map: BTreeMap<&str, &InboxEntry> = snapshot
        .inbox_entries
        .iter()
        .map(|e| (e.id.as_str(), e))
        .collect();
    let project_names: BTreeMap<&str, String> = snapshot
        .projects
        .iter()
        .map(|p| (p.id.as_str(), p.name.clone()))
        .collect();
    let stage_names: BTreeMap<(&str, &str), String> = snapshot
        .projects
        .iter()
        .flat_map(|p| {
            p.workflow.stages.iter().map(move |st| {
                let label = if st.name.trim().is_empty() {
                    st.id.clone()
                } else {
                    st.name.clone()
                };
                ((p.id.as_str(), st.id.as_str()), label)
            })
        })
        .collect();
    let cards_by_id: BTreeMap<&str, &Card> = snapshot
        .cards
        .iter()
        .filter(|c| c.removed_at.is_none())
        .map(|c| (c.id.as_str(), c))
        .collect();
    let repo_to_project: BTreeMap<String, (&str, &str)> = snapshot
        .projects
        .iter()
        .filter_map(|p| {
            let repo = p
                .github
                .as_ref()
                .map(|g| g.repository.as_str())
                .or_else(|| p.detected_github.as_ref().map(|d| d.repository.as_str()))?;
            Some((repo.to_ascii_lowercase(), (p.id.as_str(), p.name.as_str())))
        })
        .collect();
    let gh_issue_updated_ms: BTreeMap<(&str, u64), u64> = snapshot
        .github_issues
        .iter()
        .filter_map(|gi| {
            let ts = parse_rfc3339_ms(&gi.updated_at)?;
            Some(((gi.project_id.as_str(), gi.source.issue_id), ts))
        })
        .collect();
    let gh_issue_by_repo_num: BTreeMap<(String, u64), &GitHubIssueSnapshot> = snapshot
        .github_issues
        .iter()
        .map(|gi| {
            (
                (gi.source.repository.to_ascii_lowercase(), gi.source.number),
                gi,
            )
        })
        .collect();
    let pr_by_repo_num: BTreeMap<(String, u64), &GitHubPr> = snapshot
        .cards
        .iter()
        .filter(|c| c.removed_at.is_none())
        .filter_map(|c| {
            let pr = c.pull_request.as_ref()?;
            Some(((pr.repository.to_ascii_lowercase(), pr.number), pr))
        })
        .collect();

    let mut inbox = DailyInbox {
        day_start_ms,
        next_cutoff_ms,
        cutoff_minutes,
        ..Default::default()
    };

    let mut handled_card_ids: BTreeSet<String> = BTreeSet::new();
    let mut handled_terminal_ids: BTreeSet<String> = BTreeSet::new();
    let mut deferred_keys: BTreeSet<String> = BTreeSet::new();
    let mut done_keys: BTreeSet<String> = BTreeSet::new();

    // 1. Today's Scheduled Runs + Failed Runs (latest failed per schedule if not superseded by a pass).
    let mut schedule_stacks_map: BTreeMap<String, ScheduleCardStack> = BTreeMap::new();
    let mut scheduled_card_ids_today: BTreeSet<String> = BTreeSet::new();
    let mut today_runs: Vec<&ScheduledRun> = snapshot
        .scheduled_runs
        .iter()
        .filter(|r| {
            let ts = r.completed_at.unwrap_or(r.scheduled_at).max(r.scheduled_at);
            ts >= day_start_ms && ts < next_cutoff_ms
        })
        .collect();
    today_runs.sort_by(|a, b| b.scheduled_at.cmp(&a.scheduled_at).then(a.id.cmp(&b.id)));

    // Track whether a schedule already succeeded more recently today so stale earlier failures don't stay urgent.
    let mut passed_schedules: BTreeSet<(&str, &str)> = BTreeSet::new();
    for run in &today_runs {
        if matches!(run.state.as_str(), "completed" | "succeeded") {
            passed_schedules.insert((run.project_id.as_str(), run.schedule_id.as_str()));
        }
    }

    for run in today_runs {
        let proj_name = if is_global_project_id(&run.project_id) {
            "Global".into()
        } else {
            project_names
                .get(run.project_id.as_str())
                .cloned()
                .unwrap_or_else(|| run.project_id.clone())
        };
        inbox.schedule_pulse.push(SchedulePulseItem {
            run_id: run.id.clone(),
            schedule_id: run.schedule_id.clone(),
            schedule_name: run.workflow_name.clone(),
            project_id: run.project_id.clone(),
            project_name: proj_name.clone(),
            state: run.state.clone(),
            scheduled_at: run.scheduled_at,
            completed_at: run.completed_at,
            creates_card: run.creates_card,
            card_id: run.card_id.clone(),
            card_number: run.card_number,
            summary: run.summary.clone(),
            error: run.error.clone(),
            report_path: run.report_path.clone(),
        });

        let sched_key = format!("schedule:{}", run.id);
        let ts = run
            .completed_at
            .unwrap_or(run.scheduled_at)
            .max(run.scheduled_at);
        if matches!(run.state.as_str(), "failed" | "blocked")
            && !passed_schedules.contains(&(run.project_id.as_str(), run.schedule_id.as_str()))
        {
            match classify_triage(&triage_map, &sched_key, now_ms, day_start_ms, Some(ts)) {
                TriageDisposition::Active | TriageDisposition::PinnedToday => {
                    if let Some(tid) = &run.terminal_id {
                        handled_terminal_ids.insert(tid.clone());
                    }
                    inbox.needs_you.push(NeedsYouItem {
                        key: sched_key,
                        watermark: Some(ts),
                        timestamp_ms: ts,
                        kind: NeedsYouKind::ScheduleFailed {
                            run_id: run.id.clone(),
                            schedule_id: run.schedule_id.clone(),
                            schedule_name: run.workflow_name.clone(),
                            project_id: run.project_id.clone(),
                            project_name: proj_name.clone(),
                            host_label: project_host_label(snapshot, &run.project_id),
                            state: run.state.clone(),
                            summary: run.summary.clone(),
                            error: run.error.clone(),
                            report_path: run.report_path.clone(),
                            terminal_id: run.terminal_id.clone(),
                        },
                    });
                }
                TriageDisposition::Deferred(until) => {
                    if deferred_keys.insert(sched_key.clone()) {
                        inbox.deferred_items.push(DeferredInboxItem {
                            key: sched_key,
                            title: format!("Schedule · {} ({})", run.workflow_name, run.state),
                            context: proj_name,
                            kind_label: "Schedule",
                            defer_until: until,
                            card_id: run.card_id.clone(),
                            terminal_id: run.terminal_id.clone(),
                            url: None,
                        });
                    }
                }
                TriageDisposition::DismissedToday => {
                    if done_keys.insert(sched_key.clone()) {
                        inbox.done_today_items.push(DoneInboxItem {
                            key: sched_key.clone(),
                            title: format!("Schedule · {}", run.workflow_name),
                            context: proj_name,
                            kind_label: "Schedule",
                            completed_at: triage_map
                                .get(sched_key.as_str())
                                .map(|e| e.updated_at)
                                .unwrap_or(ts),
                            card_id: run.card_id.clone(),
                            can_restore: true,
                        });
                    }
                }
                TriageDisposition::DismissedOlder => {}
            }
        } else if run.creates_card
            && let Some(cid) = run.card_id.as_deref()
            && let Some(card) = cards_by_id.get(cid)
            && card.active()
            && !card.resolved()
        {
            let card_key = format!("schedule-card:{}", card.id);
            let wm = snapshot
                .card_summaries
                .get(&card.id)
                .map(|s| s.last_event_seq)
                .unwrap_or(0);
            match classify_triage(&triage_map, &card_key, now_ms, day_start_ms, Some(wm)) {
                TriageDisposition::Active | TriageDisposition::PinnedToday => {
                    scheduled_card_ids_today.insert(card.id.clone());
                    let entry = schedule_stacks_map
                        .entry(run.schedule_id.clone())
                        .or_insert_with(|| ScheduleCardStack {
                            stack_id: format!("schedule-stack:{}", run.schedule_id),
                            schedule_id: run.schedule_id.clone(),
                            schedule_name: run.workflow_name.clone(),
                            project_id: run.project_id.clone(),
                            project_name: proj_name.clone(),
                            cards: Vec::new(),
                            latest_at: ts,
                        });
                    entry.latest_at = entry.latest_at.max(ts);
                    entry.cards.push(ScheduleStackCard {
                        key: card_key,
                        watermark: Some(wm),
                        run_id: run.id.clone(),
                        card_id: card.id.clone(),
                        card_number: card.number,
                        card_title: card.title.clone(),
                        card_body: card.body.clone(),
                        stage: card.stage.clone(),
                        project_id: card.project_id.clone(),
                        project_name: proj_name,
                        created_at: card.created_at,
                    });
                }
                TriageDisposition::Deferred(until) => {
                    scheduled_card_ids_today.insert(card.id.clone());
                    if deferred_keys.insert(card_key.clone()) {
                        inbox.deferred_items.push(DeferredInboxItem {
                            key: card_key,
                            title: format!("#{} {}", card.number, card.title),
                            context: format!("{} · {}", run.workflow_name, proj_name),
                            kind_label: "Scheduled Card",
                            defer_until: until,
                            card_id: Some(card.id.clone()),
                            terminal_id: None,
                            url: None,
                        });
                    }
                }
                TriageDisposition::DismissedToday => {
                    scheduled_card_ids_today.insert(card.id.clone());
                    if done_keys.insert(card_key.clone()) {
                        inbox.done_today_items.push(DoneInboxItem {
                            key: card_key.clone(),
                            title: format!("#{} {}", card.number, card.title),
                            context: format!("{} · {}", run.workflow_name, proj_name),
                            kind_label: "Scheduled Card",
                            completed_at: triage_map
                                .get(card_key.as_str())
                                .map(|e| e.updated_at)
                                .unwrap_or(ts),
                            card_id: Some(card.id.clone()),
                            can_restore: true,
                        });
                    }
                }
                TriageDisposition::DismissedOlder => {
                    scheduled_card_ids_today.insert(card.id.clone());
                }
            }
        }
    }

    // 2. Tier 1 Cards: human gates, attention, blocked/stalled agents, explicit unread replies/escalations.
    let overview_rows = overview_at(snapshot, now_ms);
    let mut actionable_by_card: BTreeMap<&str, &OverviewRow> = BTreeMap::new();
    for row in &overview_rows {
        if row.card.active() && row_needs_action_now(row) {
            actionable_by_card
                .entry(row.card.id.as_str())
                .or_insert(row);
        }
    }

    let mut non_gh_unread: BTreeMap<&str, Vec<&Reminder>> = BTreeMap::new();
    let mut gh_card_reminders: Vec<&Reminder> = Vec::new();
    for rem in &snapshot.reminders {
        if is_github_reminder(rem) {
            if rem.unread {
                gh_card_reminders.push(rem);
            }
        } else if rem.unread && rem.audible_when_unread {
            // Only explicit agent replies and blocked/failed/backlog escalations enter Tier 1;
            // routine silent stage `completed` reports never inflate the left-rail badge.
            non_gh_unread
                .entry(rem.card_id.as_str())
                .or_default()
                .push(rem);
        }
    }

    let mut needs_you_card_ids: BTreeSet<&str> = BTreeSet::new();
    for cid in actionable_by_card.keys() {
        needs_you_card_ids.insert(*cid);
    }
    for cid in non_gh_unread.keys() {
        if cards_by_id.get(cid).is_some_and(|c| c.active()) {
            needs_you_card_ids.insert(*cid);
        }
    }

    for cid in needs_you_card_ids {
        let Some(card) = cards_by_id.get(cid) else {
            continue;
        };
        let proj_name = project_names
            .get(card.project_id.as_str())
            .cloned()
            .unwrap_or_else(|| card.project_id.clone());
        let summary = snapshot.card_summaries.get(cid);
        let max_seq = summary.map(|s| s.last_event_seq).unwrap_or(0);
        let row = actionable_by_card.get(cid).copied();
        let unread_list = non_gh_unread.get(cid).cloned().unwrap_or_default();
        let latest_unread = unread_list
            .iter()
            .max_by_key(|r| (r.sequence, r.created_at))
            .copied();

        let (headline, detail, body, actor, agent_engine, is_review, is_blocked, terminal_id, ts) =
            if let Some(row) = row {
                let first_reason = row.reasons.first();
                let is_review = row.reasons.iter().any(|r| r.source == "review");
                let is_blocked = row.reasons.iter().any(|r| r.source == "runtime")
                    || row.agent().is_some_and(|a| a.state == "blocked");
                let headline = if is_review {
                    "Waiting for review approval".into()
                } else if is_blocked {
                    "Waiting for your reply".into()
                } else if let Some(r) = first_reason {
                    r.message.clone()
                } else {
                    "Needs your attention".into()
                };
                let detail = first_reason.map(|r| r.message.clone());
                let body = latest_unread
                    .and_then(|r| r.event.as_ref().and_then(|m| m.body.clone()))
                    .or_else(|| {
                        row.agent()
                            .and_then(|a| a.lifecycle.waiting.as_ref())
                            .map(|w| w.message.clone())
                    });
                let actor = row
                    .agent()
                    .map(|a| format!("@{}", a.profile.id))
                    .or_else(|| {
                        latest_unread.and_then(|r| r.event.as_ref().map(|m| m.actor.clone()))
                    });
                let agent_engine = row
                    .terminal
                    .as_ref()
                    .and_then(|t| t.active_agent_engine().map(str::to_owned))
                    .or_else(|| {
                        row.agent().map(|a| {
                            a.profile
                                .preset
                                .clone()
                                .unwrap_or_else(|| a.profile.id.clone())
                        })
                    });
                let ts = latest_unread
                    .map(|r| r.created_at)
                    .or_else(|| row.agent().map(|a| a.updated_at))
                    .or_else(|| summary.map(|s| s.last_activity_at))
                    .unwrap_or(card.created_at);
                (
                    headline,
                    detail,
                    body,
                    actor,
                    agent_engine,
                    is_review,
                    is_blocked,
                    row.outcome_terminal_id().map(str::to_owned),
                    ts,
                )
            } else {
                let rem = latest_unread.unwrap();
                let actor = rem.event.as_ref().map(|m| m.actor.clone());
                let body = rem
                    .event
                    .as_ref()
                    .and_then(|m| m.body.clone().or_else(|| m.summary.clone()));
                (
                    rem.message.clone(),
                    None,
                    body,
                    actor,
                    None,
                    false,
                    false,
                    None,
                    rem.created_at,
                )
            };

        let key = format!("card:{}", card.id);
        let wm = Some(max_seq.max(ts));
        match classify_triage(&triage_map, &key, now_ms, day_start_ms, wm) {
            TriageDisposition::Active | TriageDisposition::PinnedToday => {
                handled_card_ids.insert(card.id.clone());
                if let Some(tid) = &terminal_id {
                    handled_terminal_ids.insert(tid.clone());
                }
                let can_quick_merge = card.worktree.is_some()
                    && !card.resolved()
                    && !card.is_github_pr()
                    && !snapshot.merging_cards.contains(&card.id);
                let pr_number = card.pull_request.as_ref().map(|p| p.number);
                let pr_checks_summary = card
                    .pull_request
                    .as_ref()
                    .map(|p| p.checks.clone())
                    .filter(|s| !s.is_empty());
                let pr_ci_state = card
                    .pull_request
                    .as_ref()
                    .and_then(|p| p.checks_summary.as_ref().map(|c| c.state.clone()));
                inbox.needs_you.push(NeedsYouItem {
                    key,
                    watermark: wm,
                    timestamp_ms: ts,
                    kind: NeedsYouKind::Card {
                        card_id: card.id.clone(),
                        card_number: card.number,
                        card_title: card.title.clone(),
                        project_id: card.project_id.clone(),
                        project_name: proj_name,
                        host_label: project_host_label(snapshot, &card.project_id),
                        terminal_id,
                        agent_engine,
                        pr_number,
                        pr_checks_summary,
                        pr_ci_state,
                        branch: card.branch.clone(),
                        headline,
                        detail,
                        body,
                        actor,
                        is_review,
                        is_blocked,
                        unread_seq: latest_unread.map(|r| r.sequence).unwrap_or(max_seq),
                        unread_count: unread_list.len(),
                        can_quick_merge,
                    },
                });
            }
            TriageDisposition::Deferred(until) => {
                handled_card_ids.insert(card.id.clone());
                if deferred_keys.insert(key.clone()) {
                    inbox.deferred_items.push(DeferredInboxItem {
                        key,
                        title: format!("#{} {}", card.number, card.title),
                        context: proj_name,
                        kind_label: "Card",
                        defer_until: until,
                        card_id: Some(card.id.clone()),
                        terminal_id,
                        url: None,
                    });
                }
            }
            TriageDisposition::DismissedToday => {
                handled_card_ids.insert(card.id.clone());
                if done_keys.insert(key.clone()) {
                    inbox.done_today_items.push(DoneInboxItem {
                        key: key.clone(),
                        title: format!("#{} {}", card.number, card.title),
                        context: proj_name,
                        kind_label: "Card",
                        completed_at: triage_map
                            .get(key.as_str())
                            .map(|e| e.updated_at)
                            .unwrap_or(ts),
                        card_id: Some(card.id.clone()),
                        can_restore: true,
                    });
                }
            }
            TriageDisposition::DismissedOlder => {
                handled_card_ids.insert(card.id.clone());
            }
        }
    }

    // Deduplicate schedule_stacks against cards already surfaced in Tier 1 `needs_you`.
    for stack in schedule_stacks_map.values_mut() {
        stack
            .cards
            .retain(|c| !handled_card_ids.contains(&c.card_id));
    }
    inbox.schedule_stacks = schedule_stacks_map
        .into_values()
        .filter(|s| !s.cards.is_empty())
        .collect();
    inbox.schedule_stacks.sort_by(|a, b| {
        b.latest_at
            .cmp(&a.latest_at)
            .then(a.stack_id.cmp(&b.stack_id))
    });

    // 3. GitHub Signal Classification:
    //    - Tier 1 (`needs_you`): unread direct blockers (`CI failed`, `CHANGES_REQUESTED`,
    //      unread `review_requested`, `@mention`, `team_mention`, `assign`).
    //    - Tier 2 (`gh_direct_map` -> `github_stacks`): today's direct participating updates.
    //    - Tier 3 (`gh_watching_map` -> `github_watching`): today's passive `subscribed`/`ci_activity`
    //      & untouched today-updated synced cards (excluded from badge & Today progress).
    //    - Tier 4 (`earlier_github`): older non-actionable GitHub items (`< day_start_ms`).
    let mut gh_direct_map: BTreeMap<String, GitHubProjectStack> = BTreeMap::new();
    let mut gh_watching_map: BTreeMap<String, GitHubProjectStack> = BTreeMap::new();
    let mut seen_gh_targets: BTreeSet<(String, u64)> = BTreeSet::new();

    // 3a. Live GitHub notifications from `github.browse(kind="notifications")`.
    for notif in github_notifications {
        let repo_lower = notif.repository.to_ascii_lowercase();
        if let Some(num) = notif.number {
            seen_gh_targets.insert((repo_lower.clone(), num));
        }
        let key = format!("github:thread:{}", notif.thread_id);
        let wm = Some(notif.updated_at_ms);
        let (project_id, display_name) = repo_to_project
            .get(&repo_lower)
            .map(|(pid, pname)| (Some((*pid).to_owned()), (*pname).to_owned()))
            .unwrap_or_else(|| (None, notif.repository.clone()));

        let is_direct_blocker = notif.unread
            && (is_github_reason_review(&notif.reason)
                || is_github_reason_mention(&notif.reason)
                || is_github_reason_assign(&notif.reason));
        let is_watching = is_github_reason_watching(&notif.reason);

        let disp = classify_triage(&triage_map, &key, now_ms, day_start_ms, wm);
        match disp {
            TriageDisposition::Active | TriageDisposition::PinnedToday => {
                let kind_prefix = match notif.kind.as_str() {
                    "pr" => "PR",
                    "commit" => "Commit",
                    _ => "Issue",
                };
                let subtitle = match notif.number {
                    Some(n) => format!("{kind_prefix} #{n} · {}", notif.reason),
                    None => notif.reason.clone(),
                };
                let matched_issue = notif
                    .number
                    .and_then(|n| gh_issue_by_repo_num.get(&(repo_lower.clone(), n)).copied());
                let matched_pr = notif
                    .number
                    .and_then(|n| pr_by_repo_num.get(&(repo_lower.clone(), n)).copied());
                let checks = matched_pr
                    .and_then(|p| p.checks_summary.as_ref())
                    .or_else(|| matched_issue.and_then(|gi| gi.pr_checks.as_ref()));
                let ci_state = checks
                    .map(|c| c.state.clone())
                    .filter(|s| !s.is_empty() && s != "unknown")
                    .or_else(|| {
                        matched_pr
                            .map(|p| p.checks.clone())
                            .filter(|s| !s.is_empty() && s != "unknown")
                    });
                let ci_passed = checks.map(|c| c.passed).unwrap_or(0);
                let ci_total = checks.map(|c| c.total).unwrap_or(0);
                let failed_checks: Vec<String> = checks
                    .map(|c| c.failed_checks.iter().map(|f| f.name.clone()).collect())
                    .unwrap_or_default();
                let is_ci_failure = ci_state
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case("failure"));
                let author = notif.author.clone().or_else(|| {
                    matched_issue
                        .map(|gi| gi.author_login.clone())
                        .filter(|s| !s.is_empty())
                });
                let labels = matched_issue
                    .map(|gi| gi.labels.clone())
                    .unwrap_or_default();
                let pr_state = matched_pr
                    .map(|p| {
                        if p.merged {
                            "merged".to_owned()
                        } else {
                            p.state.to_ascii_lowercase()
                        }
                    })
                    .or_else(|| {
                        matched_issue
                            .map(|gi| gi.state.to_ascii_lowercase())
                            .filter(|s| !s.is_empty())
                    });
                let item = GitHubStackItem {
                    key: key.clone(),
                    watermark: wm,
                    repository: notif.repository.clone(),
                    kind: notif.kind.clone(),
                    number: notif.number,
                    title: notif.title.clone(),
                    subtitle,
                    body: notif.body.clone().or_else(|| {
                        matched_issue
                            .map(|gi| gi.body.clone())
                            .filter(|b| !b.trim().is_empty())
                    }),
                    author,
                    reason: notif.reason.clone(),
                    pr_state,
                    ci_state,
                    ci_passed,
                    ci_total,
                    failed_checks,
                    labels,
                    comments_count: None,
                    url: notif.url.clone(),
                    timestamp_ms: notif.updated_at_ms,
                    thread_id: Some(notif.thread_id.clone()),
                    card_id: notif.linked_card_id.clone(),
                    card_number: notif.linked_card_number,
                    read_sequence: 0,
                    is_ci_failure,
                    is_actionable: is_direct_blocker,
                    is_synced_card: false,
                };
                let is_today =
                    disp == TriageDisposition::PinnedToday || notif.updated_at_ms >= day_start_ms;

                if is_direct_blocker && is_today {
                    // Promote Tier 1 unread direct GitHub blocker directly into `Needs Action`!
                    inbox.needs_you.push(NeedsYouItem {
                        key,
                        watermark: wm,
                        timestamp_ms: notif.updated_at_ms,
                        kind: NeedsYouKind::GitHub {
                            item: Box::new(item),
                            project_id,
                            project_name: display_name,
                        },
                    });
                } else if !is_today {
                    inbox.earlier_github.push(item);
                } else if is_watching {
                    let stack_key = project_id
                        .clone()
                        .unwrap_or_else(|| format!("repo:{repo_lower}"));
                    push_into_gh_map(
                        &mut gh_watching_map,
                        format!("watch:{stack_key}"),
                        project_id,
                        display_name,
                        notif.repository.clone(),
                        item,
                    );
                } else {
                    let stack_key = project_id
                        .clone()
                        .unwrap_or_else(|| format!("repo:{repo_lower}"));
                    push_into_gh_map(
                        &mut gh_direct_map,
                        stack_key,
                        project_id,
                        display_name,
                        notif.repository.clone(),
                        item,
                    );
                }
            }
            TriageDisposition::Deferred(until) => {
                if deferred_keys.insert(key.clone()) {
                    inbox.deferred_items.push(DeferredInboxItem {
                        key,
                        title: notif.title.clone(),
                        context: format!("GitHub · {display_name}"),
                        kind_label: "GitHub",
                        defer_until: until,
                        card_id: notif.linked_card_id.clone(),
                        terminal_id: None,
                        url: Some(notif.url.clone()),
                    });
                }
            }
            TriageDisposition::DismissedToday => {
                if done_keys.insert(key.clone()) {
                    inbox.done_today_items.push(DoneInboxItem {
                        key: key.clone(),
                        title: notif.title.clone(),
                        context: format!("GitHub · {display_name}"),
                        kind_label: "GitHub",
                        completed_at: triage_map
                            .get(key.as_str())
                            .map(|e| e.updated_at)
                            .unwrap_or(notif.updated_at_ms),
                        card_id: notif.linked_card_id.clone(),
                        can_restore: true,
                    });
                }
            }
            TriageDisposition::DismissedOlder => {}
        }
    }

    // 3b. Unread GitHub PR Card reminders for YOUR OWN published PRs (`card.pull_request.is_some()`),
    //     collapsed to 1 row per PR and deduplicated against `handled_card_ids`.
    let mut pr_rems_by_card: BTreeMap<&str, Vec<&Reminder>> = BTreeMap::new();
    for rem in gh_card_reminders {
        let Some(card) = cards_by_id.get(rem.card_id.as_str()) else {
            continue;
        };
        if !card.active() || card.pull_request.is_none() || handled_card_ids.contains(&card.id) {
            continue;
        }
        pr_rems_by_card
            .entry(card.id.as_str())
            .or_default()
            .push(rem);
    }
    for (card_id, mut card_rems) in pr_rems_by_card {
        let Some(card) = cards_by_id.get(card_id) else {
            continue;
        };
        let live_ci_failing = card.pull_request.as_ref().is_some_and(|p| {
            p.checks_summary
                .as_ref()
                .is_some_and(|c| c.state == "failure")
                || p.checks.eq_ignore_ascii_case("failure")
        });
        card_rems.sort_by(|a, b| {
            let prio = |r: &Reminder| match r
                .event
                .as_ref()
                .map(|m| (m.kind.as_str(), m.state.as_deref()))
            {
                Some(("github.pr.ci_failed", _)) if live_ci_failing => 0u8,
                Some(("github.pr.reviewed", Some("CHANGES_REQUESTED"))) => 1,
                Some(("github.pr.reviewed", _)) => 2,
                Some(("github.pr.commented", _)) => 3,
                _ => 4,
            };
            prio(a)
                .cmp(&prio(b))
                .then_with(|| b.created_at.cmp(&a.created_at))
                .then_with(|| b.sequence.cmp(&a.sequence))
        });
        let rem = card_rems[0];
        let extra_count = card_rems.len().saturating_sub(1);
        let max_seq = card_rems
            .iter()
            .map(|r| r.sequence)
            .max()
            .unwrap_or(rem.sequence);
        let max_ts = card_rems
            .iter()
            .map(|r| r.created_at)
            .max()
            .unwrap_or(rem.created_at);
        let meta = rem.event.as_ref();
        let repo = meta
            .and_then(|m| m.repository.clone())
            .or_else(|| card.pull_request.as_ref().map(|p| p.repository.clone()))
            .or_else(|| card.github_issue().map(|s| s.repository.clone()))
            .unwrap_or_else(|| {
                project_names
                    .get(card.project_id.as_str())
                    .cloned()
                    .unwrap_or_else(|| card.project_id.clone())
            });
        let pr_num = meta
            .and_then(|m| m.pr_number)
            .or_else(|| card.pull_request.as_ref().map(|p| p.number))
            .or_else(|| card.github_issue().map(|s| s.number));
        let is_ci_failure =
            live_ci_failing && meta.is_some_and(|m| m.kind == "github.pr.ci_failed");
        let is_changes_requested = meta.is_some_and(|m| {
            m.kind == "github.pr.reviewed" && m.state.as_deref() == Some("CHANGES_REQUESTED")
        });
        let is_actionable = is_ci_failure || is_changes_requested;
        if !is_ci_failure
            && let Some(n) = pr_num
            && seen_gh_targets.contains(&(repo.to_ascii_lowercase(), n))
        {
            continue;
        }
        let key = format!("github:pr:{}", card.id);
        let wm = Some(max_seq.max(max_ts));
        let proj_name = project_names
            .get(card.project_id.as_str())
            .cloned()
            .unwrap_or_else(|| repo.clone());

        let disp = classify_triage(&triage_map, &key, now_ms, day_start_ms, wm);
        match disp {
            TriageDisposition::Active | TriageDisposition::PinnedToday => {
                let author = meta
                    .map(|m| {
                        m.actor
                            .strip_prefix("github:")
                            .unwrap_or(&m.actor)
                            .to_owned()
                    })
                    .filter(|a| !a.is_empty() && a != "github" && a != "system");
                let url = meta
                    .and_then(|m| m.action_url.clone())
                    .or_else(|| card.pull_request.as_ref().map(|p| p.url.clone()))
                    .or_else(|| card.github_issue().map(|s| s.url.clone()))
                    .unwrap_or_default();
                let mut subtitle = match (pr_num, meta.map(|m| m.kind.as_str())) {
                    (Some(n), Some("github.pr.ci_failed")) => format!("PR #{n} · CI failed"),
                    (Some(n), Some("github.pr.ci_succeeded")) => format!("PR #{n} · CI passed"),
                    (Some(n), Some("github.pr.reviewed")) if is_changes_requested => {
                        format!("PR #{n} · Changes requested")
                    }
                    (Some(n), Some("github.pr.reviewed")) => format!("PR #{n} · Review"),
                    (Some(n), Some("github.pr.commented")) => format!("PR #{n} · Comment"),
                    (Some(n), _) => format!("PR #{n} · Update"),
                    (None, _) => rem.message.clone(),
                };
                if extra_count > 0 {
                    subtitle.push_str(&format!(" (+{extra_count})"));
                }
                let reason = if is_changes_requested {
                    "changes_requested".into()
                } else {
                    meta.map(|m| m.kind.clone()).unwrap_or_default()
                };
                let matched_issue = pr_num.and_then(|n| {
                    gh_issue_by_repo_num
                        .get(&(repo.to_ascii_lowercase(), n))
                        .copied()
                });
                let checks = card
                    .pull_request
                    .as_ref()
                    .and_then(|p| p.checks_summary.as_ref())
                    .or_else(|| matched_issue.and_then(|gi| gi.pr_checks.as_ref()));
                let ci_state = checks
                    .map(|c| c.state.clone())
                    .filter(|s| !s.is_empty() && s != "unknown")
                    .or_else(|| {
                        card.pull_request
                            .as_ref()
                            .map(|p| p.checks.clone())
                            .filter(|s| !s.is_empty() && s != "unknown")
                    });
                let ci_passed = checks.map(|c| c.passed).unwrap_or(0);
                let ci_total = checks.map(|c| c.total).unwrap_or(0);
                let mut failed_checks = meta.map(|m| m.failed_checks.clone()).unwrap_or_default();
                if failed_checks.is_empty()
                    && let Some(c) = checks
                {
                    failed_checks = c.failed_checks.iter().map(|f| f.name.clone()).collect();
                }
                let author = author.or_else(|| {
                    matched_issue
                        .map(|gi| gi.author_login.clone())
                        .filter(|s| !s.is_empty())
                });
                let labels = matched_issue
                    .map(|gi| gi.labels.clone())
                    .unwrap_or_default();
                let pr_state = card
                    .pull_request
                    .as_ref()
                    .map(|p| {
                        if p.merged {
                            "merged".to_owned()
                        } else {
                            p.state.to_ascii_lowercase()
                        }
                    })
                    .or_else(|| {
                        matched_issue
                            .map(|gi| gi.state.to_ascii_lowercase())
                            .filter(|s| !s.is_empty())
                    });
                let item = GitHubStackItem {
                    key: key.clone(),
                    watermark: wm,
                    repository: repo.clone(),
                    kind: "pr".into(),
                    number: pr_num,
                    title: card.title.clone(),
                    subtitle,
                    body: meta.and_then(|m| m.body.clone().or_else(|| m.summary.clone())),
                    author,
                    reason,
                    pr_state,
                    ci_state,
                    ci_passed,
                    ci_total,
                    failed_checks,
                    labels,
                    comments_count: None,
                    url,
                    timestamp_ms: max_ts,
                    thread_id: None,
                    card_id: Some(card.id.clone()),
                    card_number: Some(card.number),
                    read_sequence: max_seq,
                    is_ci_failure,
                    is_actionable,
                    is_synced_card: false,
                };
                let is_today = disp == TriageDisposition::PinnedToday
                    || is_ci_failure
                    || max_ts >= day_start_ms;
                if is_actionable && is_today {
                    inbox.needs_you.push(NeedsYouItem {
                        key,
                        watermark: wm,
                        timestamp_ms: max_ts,
                        kind: NeedsYouKind::GitHub {
                            item: Box::new(item),
                            project_id: Some(card.project_id.clone()),
                            project_name: proj_name,
                        },
                    });
                } else if !is_today {
                    inbox.earlier_github.push(item);
                } else {
                    push_into_gh_map(
                        &mut gh_direct_map,
                        card.project_id.clone(),
                        Some(card.project_id.clone()),
                        proj_name,
                        repo,
                        item,
                    );
                }
            }
            TriageDisposition::Deferred(until) => {
                if deferred_keys.insert(key.clone()) {
                    inbox.deferred_items.push(DeferredInboxItem {
                        key,
                        title: format!("#{} {}", card.number, card.title),
                        context: format!("GitHub · {proj_name}"),
                        kind_label: "GitHub",
                        defer_until: until,
                        card_id: Some(card.id.clone()),
                        terminal_id: None,
                        url: meta.and_then(|m| m.action_url.clone()),
                    });
                }
            }
            TriageDisposition::DismissedToday => {
                if done_keys.insert(key.clone()) {
                    inbox.done_today_items.push(DoneInboxItem {
                        key: key.clone(),
                        title: format!("#{} {}", card.number, card.title),
                        context: format!("GitHub · {proj_name}"),
                        kind_label: "GitHub",
                        completed_at: triage_map
                            .get(key.as_str())
                            .map(|e| e.updated_at)
                            .unwrap_or(max_ts),
                        card_id: Some(card.id.clone()),
                        can_restore: true,
                    });
                }
            }
            TriageDisposition::DismissedOlder => {}
        }
    }

    // 4. Today's Open Work (unresolved Cards genuinely worked on today or pinned today)
    //    & Untouched Today-Updated Synced GitHub Cards (-> Tier 3 `github_watching`)
    //    & Carry-over Cards (-> Tier 4 `carryover_cards`).
    let live_agent_cards: BTreeSet<&str> = snapshot
        .terminals
        .iter()
        .filter_map(|t| t.agent.as_ref())
        .filter(|a| !a.finished())
        .map(|a| a.card_id.as_str())
        .collect();

    for card in &snapshot.cards {
        if !card.active() {
            continue;
        }
        let proj_name = project_names
            .get(card.project_id.as_str())
            .cloned()
            .unwrap_or_else(|| card.project_id.clone());

        if card.resolved() {
            if let Some(res_at) = card.resolved_at()
                && res_at >= day_start_ms
                && res_at < next_cutoff_ms
            {
                let key = format!("resolved-card:{}", card.id);
                if done_keys.insert(key.clone()) {
                    inbox.done_today_items.push(DoneInboxItem {
                        key,
                        title: format!("#{} {}", card.number, card.title),
                        context: proj_name,
                        kind_label: "Resolved Card",
                        completed_at: res_at,
                        card_id: Some(card.id.clone()),
                        can_restore: false,
                    });
                }
            }
            continue;
        }
        if handled_card_ids.contains(&card.id) || scheduled_card_ids_today.contains(&card.id) {
            continue;
        }
        let summary = snapshot.card_summaries.get(&card.id);
        let gh_source = card.github_issue();
        let is_github_synced = gh_source.is_some();
        let local_user_act = summary.map(|s| s.last_user_activity_at).unwrap_or(0);
        let local_created_today = !is_github_synced && card.created_at >= day_start_ms;
        let has_local_work_today = local_user_act >= day_start_ms
            || live_agent_cards.contains(card.id.as_str())
            || local_created_today;
        let gh_real_ts = gh_source
            .and_then(|s| {
                gh_issue_updated_ms
                    .get(&(card.project_id.as_str(), s.issue_id))
                    .copied()
            })
            .unwrap_or(0);
        let last_act = if is_github_synced && !has_local_work_today {
            gh_real_ts.max(local_user_act)
        } else {
            summary
                .map(|s| s.last_activity_at.max(s.last_user_activity_at))
                .unwrap_or(0)
                .max(if is_github_synced {
                    gh_real_ts
                } else {
                    card.created_at
                })
        };
        let max_seq = summary.map(|s| s.last_event_seq).unwrap_or(0);
        let key = format!("card:{}", card.id);
        let wm = Some(max_seq.max(last_act));
        let disp = classify_triage(&triage_map, &key, now_ms, day_start_ms, wm);

        let pr_summary = card
            .pull_request
            .as_ref()
            .map(|pr| format!("PR #{}", pr.number));
        let pr_ci_state = card
            .pull_request
            .as_ref()
            .and_then(|pr| pr.checks_summary.as_ref().map(|c| c.state.clone()));
        let card_stage_label = stage_names
            .get(&(card.project_id.as_str(), card.stage.as_str()))
            .cloned()
            .unwrap_or_else(|| card.stage.clone());
        let card_branch = card
            .branch
            .clone()
            .or_else(|| card.workspace.as_ref().and_then(|w| w.branch.clone()));

        match disp {
            TriageDisposition::Deferred(until) => {
                if deferred_keys.insert(key.clone()) {
                    inbox.deferred_items.push(DeferredInboxItem {
                        key,
                        title: format!("#{} {}", card.number, card.title),
                        context: proj_name,
                        kind_label: "Card",
                        defer_until: until,
                        card_id: Some(card.id.clone()),
                        terminal_id: None,
                        url: None,
                    });
                }
            }
            TriageDisposition::DismissedToday => {
                if done_keys.insert(key.clone()) {
                    inbox.done_today_items.push(DoneInboxItem {
                        key: key.clone(),
                        title: format!("#{} {}", card.number, card.title),
                        context: proj_name,
                        kind_label: "Card",
                        completed_at: triage_map
                            .get(key.as_str())
                            .map(|e| e.updated_at)
                            .unwrap_or(last_act),
                        card_id: Some(card.id.clone()),
                        can_restore: true,
                    });
                }
            }
            TriageDisposition::DismissedOlder => {}
            TriageDisposition::PinnedToday => {
                inbox.open_cards.push(InboxCardWork {
                    key,
                    watermark: wm,
                    card_id: card.id.clone(),
                    card_number: card.number,
                    card_title: card.title.clone(),
                    stage: card_stage_label,
                    project_id: card.project_id.clone(),
                    project_name: proj_name,
                    branch: card_branch,
                    host_label: project_host_label(snapshot, &card.project_id),
                    pr_summary,
                    pr_ci_state,
                    last_activity_at: last_act,
                    pinned_today: true,
                    has_live_agent: live_agent_cards.contains(card.id.as_str()),
                });
            }
            TriageDisposition::Active => {
                if let Some(src) = gh_source
                    && !has_local_work_today
                {
                    if gh_real_ts >= day_start_ms
                        && !seen_gh_targets
                            .contains(&(src.repository.to_ascii_lowercase(), src.number))
                    {
                        seen_gh_targets.insert((src.repository.to_ascii_lowercase(), src.number));
                        let is_pr = card.is_github_pr();
                        let subtitle = if is_pr {
                            format!("Synced PR #{} · {}", src.number, card.stage)
                        } else {
                            format!("Synced Issue #{} · {}", src.number, card.stage)
                        };
                        let matched_issue = gh_issue_by_repo_num
                            .get(&(src.repository.to_ascii_lowercase(), src.number))
                            .copied();
                        let checks = card
                            .pull_request
                            .as_ref()
                            .and_then(|p| p.checks_summary.as_ref())
                            .or_else(|| matched_issue.and_then(|gi| gi.pr_checks.as_ref()));
                        let ci_state = checks
                            .map(|c| c.state.clone())
                            .filter(|s| !s.is_empty() && s != "unknown")
                            .or_else(|| {
                                card.pull_request
                                    .as_ref()
                                    .map(|p| p.checks.clone())
                                    .filter(|s| !s.is_empty() && s != "unknown")
                            });
                        let ci_passed = checks.map(|c| c.passed).unwrap_or(0);
                        let ci_total = checks.map(|c| c.total).unwrap_or(0);
                        let failed_checks: Vec<String> = checks
                            .map(|c| c.failed_checks.iter().map(|f| f.name.clone()).collect())
                            .unwrap_or_default();
                        let is_ci_failure = ci_state
                            .as_deref()
                            .is_some_and(|s| s.eq_ignore_ascii_case("failure"));
                        let author = matched_issue
                            .map(|gi| gi.author_login.clone())
                            .filter(|s| !s.is_empty());
                        let labels = matched_issue
                            .map(|gi| gi.labels.clone())
                            .unwrap_or_default();
                        let pr_state = card
                            .pull_request
                            .as_ref()
                            .map(|p| {
                                if p.merged {
                                    "merged".to_owned()
                                } else {
                                    p.state.to_ascii_lowercase()
                                }
                            })
                            .or_else(|| {
                                matched_issue
                                    .map(|gi| gi.state.to_ascii_lowercase())
                                    .filter(|s| !s.is_empty())
                            });
                        let item = GitHubStackItem {
                            key,
                            watermark: wm,
                            repository: src.repository.clone(),
                            kind: if is_pr { "pr".into() } else { "issue".into() },
                            number: Some(src.number),
                            title: card.title.clone(),
                            subtitle,
                            body: (!card.body.trim().is_empty()).then(|| card.body.clone()),
                            author,
                            reason: "synced".into(),
                            pr_state,
                            ci_state,
                            ci_passed,
                            ci_total,
                            failed_checks,
                            labels,
                            comments_count: None,
                            url: src.url.clone(),
                            timestamp_ms: gh_real_ts,
                            thread_id: None,
                            card_id: Some(card.id.clone()),
                            card_number: Some(card.number),
                            read_sequence: 0,
                            is_ci_failure,
                            is_actionable: false,
                            is_synced_card: true,
                        };
                        // Route untouched synced external cards to Tier 3 (`github_watching`)
                        // so syncing an OSS repo never pollutes Today's progress or badge.
                        push_into_gh_map(
                            &mut gh_watching_map,
                            format!("watch:{}", card.project_id),
                            Some(card.project_id.clone()),
                            proj_name,
                            src.repository.clone(),
                            item,
                        );
                    } else {
                        inbox.carryover_cards.push(InboxCardWork {
                            key,
                            watermark: wm,
                            card_id: card.id.clone(),
                            card_number: card.number,
                            card_title: card.title.clone(),
                            stage: card_stage_label,
                            project_id: card.project_id.clone(),
                            project_name: proj_name,
                            branch: card_branch,
                            host_label: project_host_label(snapshot, &card.project_id),
                            pr_summary,
                            pr_ci_state,
                            last_activity_at: last_act,
                            pinned_today: false,
                            has_live_agent: false,
                        });
                    }
                    continue;
                }

                let item = InboxCardWork {
                    key,
                    watermark: wm,
                    card_id: card.id.clone(),
                    card_number: card.number,
                    card_title: card.title.clone(),
                    stage: card_stage_label,
                    project_id: card.project_id.clone(),
                    project_name: proj_name,
                    branch: card_branch,
                    host_label: project_host_label(snapshot, &card.project_id),
                    pr_summary,
                    pr_ci_state,
                    last_activity_at: last_act,
                    pinned_today: false,
                    has_live_agent: live_agent_cards.contains(card.id.as_str()),
                };
                if has_local_work_today {
                    inbox.open_cards.push(item);
                } else {
                    inbox.carryover_cards.push(item);
                }
            }
        }
    }

    inbox.github_stacks = finalize_gh_stacks(gh_direct_map);
    inbox.github_watching = finalize_gh_stacks(gh_watching_map);
    inbox.earlier_github.sort_by(|a, b| {
        b.timestamp_ms
            .cmp(&a.timestamp_ms)
            .then_with(|| a.key.cmp(&b.key))
    });
    inbox.open_cards.sort_by(|a, b| {
        b.pinned_today
            .cmp(&a.pinned_today)
            .then_with(|| a.project_name.cmp(&b.project_name))
            .then_with(|| b.last_activity_at.cmp(&a.last_activity_at))
            .then_with(|| a.card_id.cmp(&b.card_id))
    });
    inbox.carryover_cards.sort_by(|a, b| {
        b.last_activity_at
            .cmp(&a.last_activity_at)
            .then_with(|| a.card_id.cmp(&b.card_id))
    });

    // 5. Unclosed Terminals with structured Git/Turn telemetry:
    //    - Blocked / Error standalone Agent Terminals promote to Tier 1 (`needs_you`)
    //      so `actionable_count == needs_you.len()` (strict 1:1 with left-rail badge!).
    //    - Active/Done Agent Terminals from today go to `agent_terminals`.
    //    - Plain shells opened today go to `plain_shells`.
    let ctx_by_term: BTreeMap<&str, &TerminalContext> = snapshot
        .terminal_contexts
        .iter()
        .map(|c| (c.terminal_id.as_str(), c))
        .collect();

    for entry in &snapshot.terminals {
        let status = entry.herdr_status();
        if matches!(status, HerdrStatus::Ended | HerdrStatus::Closing) {
            continue;
        }
        if handled_terminal_ids.contains(&entry.id) {
            continue;
        }
        let is_live = if let Some(a) = &entry.agent {
            !a.finished() && (a.terminal.is_some() || a.observed.is_some())
        } else {
            snapshot.terminal_is_live(entry)
        };
        if !is_live {
            continue;
        }
        let is_agent = entry.has_active_agent() || entry.agent.is_some();
        let ts = entry
            .agent
            .as_ref()
            .map(|a| {
                a.lifecycle
                    .last_interaction_at
                    .unwrap_or(0)
                    .max(a.updated_at)
                    .max(entry.created_at)
            })
            .unwrap_or(entry.created_at);
        let is_urgent_term = is_agent
            && (matches!(status, HerdrStatus::Blocked | HerdrStatus::Error)
                || (status == HerdrStatus::Disconnected && ts >= day_start_ms));
        // Only include terminals active/touched today OR currently Blocked/Working/Error.
        let relevant_today = ts >= day_start_ms
            || (is_agent
                && matches!(
                    status,
                    HerdrStatus::Blocked | HerdrStatus::Working | HerdrStatus::Error
                ));
        if !relevant_today {
            continue;
        }
        let key = format!("terminal:{}", entry.id);
        let ctx = ctx_by_term.get(entry.id.as_str()).copied();
        let card_id = entry
            .agent
            .as_ref()
            .map(|a| a.card_id.clone())
            .or_else(|| ctx.and_then(|c| c.card_id.clone()))
            .or_else(|| entry.origin_card_id.clone());
        if let Some(cid) = &card_id
            && handled_card_ids.contains(cid)
            && is_urgent_term
        {
            continue;
        }
        let card = card_id
            .as_deref()
            .and_then(|cid| cards_by_id.get(cid).copied());
        let project_id = snapshot
            .terminal_project_id(entry)
            .map(str::to_owned)
            .or_else(|| card.map(|c| c.project_id.clone()));
        let project_name = project_id
            .as_deref()
            .and_then(|pid| project_names.get(pid).cloned());
        let host_label = terminal_host_label(snapshot, &entry.id);
        let cwd = ctx
            .and_then(|c| c.cwd.clone())
            .or_else(|| entry.agent.as_ref().and_then(|a| a.directory.clone()))
            .or_else(|| card.and_then(|c| c.worktree.clone()));
        let branch = ctx.and_then(|c| c.branch.clone()).or_else(|| {
            card.and_then(|c| {
                c.branch
                    .clone()
                    .or_else(|| c.workspace.as_ref().and_then(|w| w.branch.clone()))
            })
        });
        let agent_engine = entry.active_agent_engine().map(str::to_owned);
        let agent_name = entry
            .agent
            .as_ref()
            .map(|a| format!("@{}", a.profile.id))
            .or_else(|| agent_engine.as_deref().map(friendly_agent_engine));
        let agent_turn = entry
            .runtime
            .as_ref()
            .and_then(|r| r.agent_turn)
            .or_else(|| {
                entry
                    .agent
                    .as_ref()
                    .and_then(|a| a.observed.as_ref().and_then(|v| v["turn"].as_u64()))
            });
        let wait = entry.runtime_wait();
        let waiting_title = wait
            .map(|w| w.title.clone())
            .filter(|s| !s.trim().is_empty());
        let status_detail = wait
            .map(|w| {
                if w.message.trim().is_empty() {
                    w.title.clone()
                } else {
                    w.message.clone()
                }
            })
            .or_else(|| entry.agent.as_ref().and_then(|a| a.error.clone()))
            .or_else(|| entry.runtime.as_ref().and_then(|r| r.error.clone()));
        let shell_cmd = entry.shell_command();
        let context_label = project_name
            .clone()
            .unwrap_or_else(|| cwd.clone().unwrap_or_else(|| host_label.clone()));
        let started_at = entry
            .agent
            .as_ref()
            .and_then(|a| a.started_at)
            .or(Some(entry.created_at));
        let term_wm = Some(ts);

        match classify_triage(&triage_map, &key, now_ms, day_start_ms, term_wm) {
            TriageDisposition::Active | TriageDisposition::PinnedToday => {
                let work = InboxTerminalWork {
                    key: key.clone(),
                    terminal_id: entry.id.clone(),
                    title: entry.title(),
                    agent_name,
                    agent_engine,
                    status,
                    agent_turn,
                    waiting_title,
                    status_detail,
                    project_id,
                    project_name,
                    host_label,
                    cwd,
                    branch,
                    dirty: ctx.is_some_and(|c| c.dirty),
                    additions: ctx.map(|c| c.additions).unwrap_or(0),
                    deletions: ctx.map(|c| c.deletions).unwrap_or(0),
                    ahead: ctx.map(|c| c.ahead).unwrap_or(0),
                    behind: ctx.map(|c| c.behind).unwrap_or(0),
                    card_id,
                    card_number: card.map(|c| c.number),
                    card_title: card.map(|c| c.title.clone()),
                    shell_command: shell_cmd.map(|c| c.label.clone()).filter(|s| !s.is_empty()),
                    shell_exit_code: shell_cmd.and_then(|c| c.exit_code),
                    started_at,
                    timestamp_ms: ts,
                    is_agent,
                };
                if is_urgent_term {
                    // Promote blocked/errored standalone agent terminals directly into Tier 1 (`Needs Action`).
                    inbox.needs_you.push(NeedsYouItem {
                        key,
                        watermark: term_wm,
                        timestamp_ms: ts,
                        kind: NeedsYouKind::Terminal {
                            terminal: Box::new(work),
                        },
                    });
                } else if is_agent {
                    inbox.agent_terminals.push(work);
                } else {
                    inbox.plain_shells.push(work);
                }
            }
            TriageDisposition::Deferred(until) => {
                if deferred_keys.insert(key.clone()) {
                    inbox.deferred_items.push(DeferredInboxItem {
                        key,
                        title: entry.title(),
                        context: context_label,
                        kind_label: if is_agent { "Agent Terminal" } else { "Shell" },
                        defer_until: until,
                        card_id,
                        terminal_id: Some(entry.id.clone()),
                        url: None,
                    });
                }
            }
            TriageDisposition::DismissedToday => {
                if done_keys.insert(key.clone()) {
                    inbox.done_today_items.push(DoneInboxItem {
                        key: key.clone(),
                        title: entry.title(),
                        context: context_label,
                        kind_label: if is_agent { "Agent Terminal" } else { "Shell" },
                        completed_at: triage_map
                            .get(key.as_str())
                            .map(|e| e.updated_at)
                            .unwrap_or(ts),
                        card_id,
                        can_restore: true,
                    });
                }
            }
            TriageDisposition::DismissedOlder => {}
        }
    }

    inbox.needs_you.sort_by(|a, b| {
        let rank = |k: &NeedsYouKind| match k {
            NeedsYouKind::Card {
                is_blocked: true, ..
            }
            | NeedsYouKind::Terminal { .. } => 0u8,
            NeedsYouKind::ScheduleFailed { .. } => 1,
            NeedsYouKind::GitHub { item, .. } if item.is_ci_failure => 2,
            NeedsYouKind::Card { .. } => 3,
            NeedsYouKind::GitHub { .. } => 4,
        };
        rank(&a.kind)
            .cmp(&rank(&b.kind))
            .then_with(|| b.timestamp_ms.cmp(&a.timestamp_ms))
            .then_with(|| a.key.cmp(&b.key))
    });
    inbox.agent_terminals.sort_by(|a, b| {
        let rank = |s: HerdrStatus| match s {
            HerdrStatus::Working | HerdrStatus::Starting => 0,
            HerdrStatus::Done => 1,
            _ => 2,
        };
        rank(a.status)
            .cmp(&rank(b.status))
            .then_with(|| b.timestamp_ms.cmp(&a.timestamp_ms))
            .then_with(|| a.terminal_id.cmp(&b.terminal_id))
    });
    inbox.plain_shells.sort_by(|a, b| {
        b.timestamp_ms
            .cmp(&a.timestamp_ms)
            .then_with(|| a.terminal_id.cmp(&b.terminal_id))
    });
    inbox
        .deferred_items
        .sort_by(|a, b| a.defer_until.cmp(&b.defer_until).then(a.key.cmp(&b.key)));
    inbox
        .done_today_items
        .sort_by(|a, b| b.completed_at.cmp(&a.completed_at).then(a.key.cmp(&b.key)));

    // 6. Upcoming schedules remaining today (`schedule_states` with `next_run_at` in `[now_ms, next_cutoff_ms)`).
    for st in &snapshot.schedule_states {
        if !st.enabled {
            continue;
        }
        let Some(next_at) = st.next_run_at else {
            continue;
        };
        if next_at >= now_ms && next_at < next_cutoff_ms {
            let proj_name = if is_global_project_id(&st.project_id) {
                "Global".into()
            } else {
                project_names
                    .get(st.project_id.as_str())
                    .cloned()
                    .unwrap_or_else(|| st.project_id.clone())
            };
            inbox.upcoming_schedules.push(UpcomingScheduleItem {
                schedule_id: st.id.clone(),
                schedule_name: st.workflow_name.clone(),
                project_id: st.project_id.clone(),
                project_name: proj_name,
                next_run_at: next_at,
                cron: st.cron.clone(),
                creates_card: st.creates_card,
            });
        }
    }
    inbox.upcoming_schedules.sort_by(|a, b| {
        a.next_run_at
            .cmp(&b.next_run_at)
            .then_with(|| a.schedule_id.cmp(&b.schedule_id))
    });

    // 7. Shipped Today: merged PRs (delivery PRs, synced PRs, or archived-after-merge cards) + resolved Cards today,
    // plus preceding 7-day baseline count (`[day_start - 7d, day_start)`).
    let prev_7d_start_ms = day_start_ms.saturating_sub(7 * 86_400 * MS_PER_SEC);
    let mut seen_prev_7d_prs: BTreeSet<(String, u64)> = BTreeSet::new();
    let mut seen_prev_7d_cards: BTreeSet<String> = BTreeSet::new();
    let mut seen_shipped_prs: BTreeSet<(String, u64)> = BTreeSet::new();
    let mut seen_shipped_cards: BTreeSet<String> = BTreeSet::new();
    let mut merged_today_by_project: BTreeMap<String, usize> = BTreeMap::new();

    for card in &snapshot.cards {
        if card.removed_at.is_some() {
            continue;
        }
        let proj_name = project_names
            .get(card.project_id.as_str())
            .cloned()
            .unwrap_or_else(|| card.project_id.clone());
        let delivery_pr = card.pull_request.as_ref().filter(|p| p.merged);
        let gh_src = card.github_issue();
        let matched_issue = gh_src.and_then(|s| {
            gh_issue_by_repo_num
                .get(&(s.repository.to_ascii_lowercase(), s.number))
                .copied()
        });
        let synced_pr_merged = card.is_github_pr()
            && matched_issue.is_some_and(|gi| gi.state.eq_ignore_ascii_case("merged"));
        let res_ts = card
            .resolved_at()
            .or_else(|| card.delivery.as_ref().map(|d| d.merged_at))
            .or_else(|| {
                if synced_pr_merged {
                    matched_issue.and_then(|gi| parse_rfc3339_ms(&gi.updated_at))
                } else {
                    None
                }
            });
        let Some(shipped_at) = res_ts else {
            continue;
        };
        if shipped_at >= prev_7d_start_ms && shipped_at < day_start_ms {
            if let Some(pr) = delivery_pr {
                let repo_key = (pr.repository.to_ascii_lowercase(), pr.number);
                if seen_prev_7d_prs.insert(repo_key) {
                    seen_prev_7d_cards.insert(card.id.clone());
                    inbox.shipped_prev_7d_total += 1;
                }
            } else if synced_pr_merged && let Some(src) = gh_src {
                let repo_key = (src.repository.to_ascii_lowercase(), src.number);
                if seen_prev_7d_prs.insert(repo_key) {
                    seen_prev_7d_cards.insert(card.id.clone());
                    inbox.shipped_prev_7d_total += 1;
                }
            } else if card.resolved() && seen_prev_7d_cards.insert(card.id.clone()) {
                inbox.shipped_prev_7d_total += 1;
            }
            continue;
        }
        if shipped_at < day_start_ms || shipped_at >= next_cutoff_ms {
            continue;
        }
        if let Some(pr) = delivery_pr {
            let repo_key = (pr.repository.to_ascii_lowercase(), pr.number);
            if seen_shipped_prs.insert(repo_key) {
                seen_shipped_cards.insert(card.id.clone());
                *merged_today_by_project
                    .entry(card.project_id.clone())
                    .or_default() += 1;
                inbox.shipped_today.push(ShippedTodayItem {
                    key: format!("shipped:pr:{}:{}", pr.repository, pr.number),
                    is_pr: true,
                    title: card.title.clone(),
                    repository: Some(pr.repository.clone()),
                    pr_number: Some(pr.number),
                    card_id: Some(card.id.clone()),
                    card_number: Some(card.number),
                    project_name: proj_name,
                    author: matched_issue
                        .map(|gi| gi.author_login.clone())
                        .filter(|s| !s.is_empty())
                        .or_else(|| snapshot.github.login.clone()),
                    url: Some(pr.url.clone()),
                    shipped_at,
                });
            }
        } else if synced_pr_merged && let Some(src) = gh_src {
            let repo_key = (src.repository.to_ascii_lowercase(), src.number);
            if seen_shipped_prs.insert(repo_key) {
                seen_shipped_cards.insert(card.id.clone());
                *merged_today_by_project
                    .entry(card.project_id.clone())
                    .or_default() += 1;
                inbox.shipped_today.push(ShippedTodayItem {
                    key: format!("shipped:pr:{}:{}", src.repository, src.number),
                    is_pr: true,
                    title: card.title.clone(),
                    repository: Some(src.repository.clone()),
                    pr_number: Some(src.number),
                    card_id: Some(card.id.clone()),
                    card_number: Some(card.number),
                    project_name: proj_name,
                    author: matched_issue
                        .map(|gi| gi.author_login.clone())
                        .filter(|s| !s.is_empty()),
                    url: Some(src.url.clone()),
                    shipped_at,
                });
            }
        } else if card.resolved() && seen_shipped_cards.insert(card.id.clone()) {
            *merged_today_by_project
                .entry(card.project_id.clone())
                .or_default() += 1;
            inbox.shipped_today.push(ShippedTodayItem {
                key: format!("shipped:card:{}", card.id),
                is_pr: false,
                title: card.title.clone(),
                repository: None,
                pr_number: None,
                card_id: Some(card.id.clone()),
                card_number: Some(card.number),
                project_name: proj_name,
                author: None,
                url: None,
                shipped_at,
            });
        }
    }
    // Also capture merged PRs from `snapshot.github_issues` that had no active local Card.
    for gi in &snapshot.github_issues {
        if !gi.state.eq_ignore_ascii_case("merged") {
            continue;
        }
        let Some(ts) = parse_rfc3339_ms(&gi.updated_at) else {
            continue;
        };
        if ts >= prev_7d_start_ms && ts < day_start_ms {
            let repo_key = (gi.source.repository.to_ascii_lowercase(), gi.source.number);
            if seen_prev_7d_prs.insert(repo_key) {
                inbox.shipped_prev_7d_total += 1;
            }
            continue;
        }
        if ts < day_start_ms || ts >= next_cutoff_ms {
            continue;
        }
        let repo_key = (gi.source.repository.to_ascii_lowercase(), gi.source.number);
        if seen_shipped_prs.insert(repo_key) {
            let proj_name = project_names
                .get(gi.project_id.as_str())
                .cloned()
                .unwrap_or_else(|| gi.source.repository.clone());
            *merged_today_by_project
                .entry(gi.project_id.clone())
                .or_default() += 1;
            inbox.shipped_today.push(ShippedTodayItem {
                key: format!("shipped:pr:{}:{}", gi.source.repository, gi.source.number),
                is_pr: true,
                title: gi.title.clone(),
                repository: Some(gi.source.repository.clone()),
                pr_number: Some(gi.source.number),
                card_id: None,
                card_number: None,
                project_name: proj_name,
                author: (!gi.author_login.is_empty()).then(|| gi.author_login.clone()),
                url: Some(gi.source.url.clone()),
                shipped_at: ts,
            });
        }
    }
    inbox.shipped_today.sort_by(|a, b| {
        b.shipped_at
            .cmp(&a.shipped_at)
            .then_with(|| a.key.cmp(&b.key))
    });

    // 8. Active Project Pulse (projects with active WIP, open PRs, live agents, attention, or shipped today).
    let mut attention_by_proj: BTreeMap<&str, usize> = BTreeMap::new();
    for ny in &inbox.needs_you {
        let pid = match &ny.kind {
            NeedsYouKind::Card { project_id, .. }
            | NeedsYouKind::ScheduleFailed { project_id, .. } => Some(project_id.as_str()),
            NeedsYouKind::GitHub { project_id, .. } => project_id.as_deref(),
            NeedsYouKind::Terminal { terminal } => terminal.project_id.as_deref(),
        };
        if let Some(p) = pid {
            *attention_by_proj.entry(p).or_default() += 1;
        }
    }
    let mut live_agents_by_proj: BTreeMap<&str, usize> = BTreeMap::new();
    for t in &inbox.agent_terminals {
        if let Some(p) = t.project_id.as_deref() {
            *live_agents_by_proj.entry(p).or_default() += 1;
        }
    }
    inbox.has_projects = !snapshot.projects.is_empty();
    for proj in &snapshot.projects {
        let mut counts_by_stage: BTreeMap<&str, usize> = BTreeMap::new();
        let mut open_pr_count = 0usize;
        for c in snapshot
            .cards
            .iter()
            .filter(|c| c.active() && !c.resolved() && c.project_id == proj.id)
        {
            *counts_by_stage.entry(c.stage.as_str()).or_default() += 1;
            if c.pull_request
                .as_ref()
                .is_some_and(|p| !p.merged && p.state == "open")
            {
                open_pr_count += 1;
            }
        }
        let mut stage_counts = Vec::new();
        let mut seen_stages: BTreeSet<&str> = BTreeSet::new();
        for (idx, st) in proj.workflow.stages.iter().enumerate() {
            seen_stages.insert(st.id.as_str());
            let is_backlog = st.id.eq_ignore_ascii_case("backlog")
                || (idx == 0 && !st.execute && proj.workflow.stages.len() > 1);
            if is_backlog {
                continue;
            }
            if let Some(&count) = counts_by_stage.get(st.id.as_str())
                && count > 0
            {
                let stage_name = if st.name.trim().is_empty() {
                    st.id.clone()
                } else {
                    st.name.clone()
                };
                stage_counts.push(StagePulseCount {
                    stage_id: st.id.clone(),
                    stage_name,
                    count,
                });
            }
        }
        for (stage_id, count) in counts_by_stage {
            if count > 0
                && !seen_stages.contains(stage_id)
                && !stage_id.eq_ignore_ascii_case("backlog")
            {
                stage_counts.push(StagePulseCount {
                    stage_id: stage_id.to_owned(),
                    stage_name: stage_id.to_owned(),
                    count,
                });
            }
        }
        let live_agent_count = live_agents_by_proj
            .get(proj.id.as_str())
            .copied()
            .unwrap_or(0);
        let attention_count = attention_by_proj
            .get(proj.id.as_str())
            .copied()
            .unwrap_or(0);
        let merged_today_count = merged_today_by_project
            .get(proj.id.as_str())
            .copied()
            .unwrap_or(0);
        if !stage_counts.is_empty()
            || open_pr_count > 0
            || live_agent_count > 0
            || attention_count > 0
            || merged_today_count > 0
        {
            inbox.project_pulses.push(ProjectPulseItem {
                project_id: proj.id.clone(),
                project_name: proj.name.clone(),
                host_label: project_host_label(snapshot, &proj.id),
                stage_counts,
                open_pr_count,
                live_agent_count,
                attention_count,
                merged_today_count,
            });
        }
    }
    inbox.project_pulses.sort_by(|a, b| {
        let a_stages: usize = a.stage_counts.iter().map(|s| s.count).sum();
        let b_stages: usize = b.stage_counts.iter().map(|s| s.count).sum();
        b.attention_count
            .cmp(&a.attention_count)
            .then_with(|| b.live_agent_count.cmp(&a.live_agent_count))
            .then_with(|| (b_stages + b.open_pr_count).cmp(&(a_stages + a.open_pr_count)))
            .then_with(|| a.project_name.cmp(&b.project_name))
    });

    // Strictly 1:1 invariant: rail badge == `Needs Action` section count!
    inbox.actionable_count = inbox.needs_you.len();

    // Today triage count covers Tier 1 + Tier 2 triageable work (ongoing non-blocked terminals
    // and passive `github_watching` are excluded so open terminals never look "unfinished").
    let gh_direct_count: usize = inbox.github_stacks.iter().map(|s| s.items.len()).sum();
    let sched_card_count: usize = inbox.schedule_stacks.iter().map(|s| s.cards.len()).sum();
    inbox.remaining_today =
        inbox.needs_you.len() + gh_direct_count + sched_card_count + inbox.open_cards.len();
    inbox.completed_today = inbox.done_today_items.len();
    inbox.total_today = inbox.remaining_today + inbox.completed_today;
    inbox
}
