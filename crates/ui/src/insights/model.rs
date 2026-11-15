//! Pure local-date aggregation of Kanban completions (merges) and Agent scheduling.
//!
//! Operates directly on `canban_types::Snapshot` with zero extra RPCs or SQLite queries,
//! so multi-host merged snapshots and standalone Gallery fixtures behave identically.

use canban_types::{Card, ExecutionTrigger, Snapshot};
use chrono::{Datelike, Duration, NaiveDate, TimeZone};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RangeWindow {
    Days7,
    #[default]
    Days30,
    Days90,
}

impl RangeWindow {
    pub fn days(self) -> i64 {
        match self {
            Self::Days7 => 7,
            Self::Days30 => 30,
            Self::Days90 => 90,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Days7 => "7D",
            Self::Days30 => "30D",
            Self::Days90 => "90D",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Days7 => "last 7 days",
            Self::Days30 => "last 30 days",
            Self::Days90 => "last 90 days",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeKind {
    Completed,
    Blocked,
    Failed,
    Active,
}

impl OutcomeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "Completed",
            Self::Blocked => "Blocked",
            Self::Failed => "Failed",
            Self::Active => "Running / Queued",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TriggerKind {
    Workflow,
    Mention,
    Interactive,
}

impl TriggerKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Workflow => "Workflow Auto",
            Self::Mention => "@mention Comment",
            Self::Interactive => "Manual / Interactive",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayBucket {
    pub date_key: String,
    pub short_label: String,
    pub weekday_label: &'static str,
    pub created_cards: usize,
    pub merged_cards: usize,
    pub local_merges: usize,
    pub pr_merges: usize,
    pub completed_runs: usize,
    pub blocked_runs: usize,
    pub failed_runs: usize,
    pub active_runs: usize,
}

impl DayBucket {
    pub fn total_runs(&self) -> usize {
        self.completed_runs + self.blocked_runs + self.failed_runs + self.active_runs
    }

    pub fn total_activity(&self) -> usize {
        self.merged_cards + self.total_runs()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeatmapCell {
    pub date_key: String,
    pub short_label: String,
    pub weekday_label: &'static str,
    pub merged_cards: usize,
    pub total_runs: usize,
    pub level: u8,
    pub is_future: bool,
    pub is_today: bool,
    pub in_range: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeatmapWeek {
    pub month_header: Option<String>,
    pub cells: [HeatmapCell; 7],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileStat {
    pub profile_id: String,
    pub total_runs: usize,
    pub completed_runs: usize,
    pub blocked_runs: usize,
    pub failed_runs: usize,
    pub active_runs: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerStat {
    pub kind: TriggerKind,
    pub count: usize,
    pub share_pct: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dashboard {
    pub range: RangeWindow,
    pub today_key: String,
    pub window_start_key: String,
    pub merged_total: usize,
    pub prev_merged_total: usize,
    pub local_merges: usize,
    pub pr_merges: usize,
    pub created_total: usize,
    pub net_wip_delta: isize,
    pub scheduled_total: usize,
    pub prev_scheduled_total: usize,
    pub completed_runs: usize,
    pub blocked_runs: usize,
    pub failed_runs: usize,
    pub active_runs: usize,
    pub success_rate_pct: Option<usize>,
    pub median_lead_time_ms: Option<u64>,
    pub median_queue_wait_ms: Option<u64>,
    pub days: Vec<DayBucket>,
    pub max_daily_kanban: usize,
    pub max_daily_runs: usize,
    pub heatmap_weeks: Vec<HeatmapWeek>,
    pub profile_stats: Vec<ProfileStat>,
    pub trigger_stats: Vec<TriggerStat>,
    pub has_any_history: bool,
}

#[derive(Clone, Debug)]
struct ReconstructedRun {
    profile_id: String,
    trigger: TriggerKind,
    queued_at: Option<u64>,
    started_at: Option<u64>,
    scheduled_at: u64,
    outcome: OutcomeKind,
    cancelled_before_start: bool,
}

fn local_date<Tz: TimeZone>(ms: u64, tz: &Tz) -> NaiveDate {
    let secs = i64::try_from(ms / 1000).unwrap_or(0);
    let nsecs = ((ms % 1000) * 1_000_000) as u32;
    tz.timestamp_opt(secs, nsecs)
        .single()
        .or_else(|| tz.timestamp_opt(secs, 0).single())
        .or_else(|| tz.timestamp_opt(0, 0).single())
        .map(|dt| dt.date_naive())
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
}

fn weekday_short(d: NaiveDate) -> &'static str {
    match d.weekday() {
        chrono::Weekday::Mon => "Mon",
        chrono::Weekday::Tue => "Tue",
        chrono::Weekday::Wed => "Wed",
        chrono::Weekday::Thu => "Thu",
        chrono::Weekday::Fri => "Fri",
        chrono::Weekday::Sat => "Sat",
        chrono::Weekday::Sun => "Sun",
    }
}

fn month_short(month: u32) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "",
    }
}

fn classify_trigger(
    raw_trigger: Option<&str>,
    raw_origin: Option<&str>,
    has_workflow_run: bool,
) -> TriggerKind {
    if let Some(t) = raw_trigger {
        let parsed = ExecutionTrigger::parse(t);
        if parsed.is_comment() {
            return TriggerKind::Mention;
        }
        if matches!(parsed, ExecutionTrigger::Interactive) && !has_workflow_run {
            return TriggerKind::Interactive;
        }
        if parsed.is_automatic() || has_workflow_run {
            return TriggerKind::Workflow;
        }
    }
    match raw_origin.unwrap_or_default() {
        "comment" => TriggerKind::Mention,
        "manual" | "interactive" if !has_workflow_run => TriggerKind::Interactive,
        _ => TriggerKind::Workflow,
    }
}

fn median_u64(mut values: Vec<u64>) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        Some(values[mid])
    } else {
        Some(values[mid - 1] / 2 + values[mid] / 2 + (values[mid - 1] % 2 + values[mid] % 2) / 2)
    }
}

pub fn format_duration_compact(ms: u64) -> String {
    let total_secs = ms / 1000;
    if total_secs < 60 {
        return format!("{total_secs}s");
    }
    let total_mins = total_secs / 60;
    if total_mins < 60 {
        let secs = total_secs % 60;
        if total_mins < 10 && secs > 0 {
            return format!("{total_mins}m {secs}s");
        }
        return format!("{total_mins}m");
    }
    let total_hours = total_mins / 60;
    let mins = total_mins % 60;
    if total_hours < 48 {
        if mins == 0 {
            return format!("{total_hours}h");
        }
        return format!("{total_hours}h {mins}m");
    }
    let days = total_hours / 24;
    let rem_hours = total_hours % 24;
    if rem_hours == 0 {
        format!("{days}d")
    } else {
        format!("{days}d {rem_hours}h")
    }
}

pub fn format_delta_pct(current: usize, previous: usize) -> String {
    if previous == 0 {
        if current == 0 {
            "0% vs prev".into()
        } else {
            format!("+{current} vs prev")
        }
    } else if current >= previous {
        let pct = ((current - previous) * 100 + previous / 2) / previous;
        format!("+{pct}% vs prev")
    } else {
        let pct = ((previous - current) * 100 + previous / 2) / previous;
        format!("-{pct}% vs prev")
    }
}

fn card_is_pr_merge(card: &Card) -> bool {
    card.is_github_pr()
        || card.pull_request.as_ref().is_some_and(|pr| pr.number > 0)
        || card
            .delivery
            .as_ref()
            .is_some_and(|d| d.request_key.starts_with("github:"))
}

fn resolve_merged_at(card: &Card, card_events: Option<&Vec<&canban_types::Event>>) -> u64 {
    if let Some(ts) = card
        .delivery
        .as_ref()
        .map(|d| d.merged_at)
        .filter(|&t| t > 0)
    {
        return ts;
    }
    if let Some(events) = card_events
        && let Some(ts) = events
            .iter()
            .rev()
            .find(|e| {
                matches!(
                    e.kind.as_str(),
                    "review.merged" | "github.pr_merged" | "github.pr_delivered"
                )
            })
            .map(|e| e.created_at)
            .filter(|&t| t > 0)
    {
        return ts;
    }
    card.archived_at.unwrap_or(card.created_at)
}

fn resolve_work_started_at(
    card: &Card,
    merged_at: u64,
    card_events: Option<&Vec<&canban_types::Event>>,
) -> u64 {
    if let Some(events) = card_events {
        for ev in events {
            if ev.created_at == 0 || ev.created_at > merged_at {
                continue;
            }
            let started = match ev.kind.as_str() {
                "execution.queued" | "execution.started" | "workflow.started" => true,
                "card.stage_changed" => {
                    let msg = ev.message.trim();
                    !msg.ends_with("→ backlog") && !msg.ends_with("-> backlog")
                }
                _ => false,
            };
            if started {
                return ev.created_at;
            }
        }
    }
    card.created_at.min(merged_at)
}

fn extract_profile_from_terminal_or_actor<'a>(
    card_id: &str,
    terminal_id: &'a str,
    actor: &'a str,
) -> String {
    if let Some(rest) = terminal_id.strip_prefix(card_id)
        && let Some(profile) = rest.strip_prefix(':')
        && !profile.is_empty()
    {
        return profile.to_owned();
    }
    if let Some((_, profile)) = terminal_id.rsplit_once(':')
        && !profile.is_empty()
    {
        return profile.to_owned();
    }
    if let Some(profile) = actor.strip_prefix("agent:")
        && !profile.is_empty()
    {
        return profile.to_owned();
    }
    "developer".into()
}

fn reconstruct_runs(
    snapshot: &Snapshot,
    in_scope_project: impl Fn(&str) -> bool,
    valid_cards: &BTreeMap<&str, &Card>,
) -> Vec<ReconstructedRun> {
    let mut by_terminal: BTreeMap<String, BTreeMap<u32, ReconstructedRun>> = BTreeMap::new();
    let mut synthetic_seq: u32 = 1_000_000;

    let mut sorted_events: Vec<&canban_types::Event> = snapshot
        .events
        .iter()
        .filter(|ev| {
            in_scope_project(&ev.project_id) && valid_cards.contains_key(ev.card_id.as_str())
        })
        .collect();
    sorted_events.sort_by_key(|ev| (ev.created_at, ev.sequence));

    for ev in sorted_events {
        let relevant = matches!(
            ev.kind.as_str(),
            "execution.queued"
                | "execution.retry_queued"
                | "execution.started"
                | "execution.continued"
                | "execution.reported"
                | "execution.failed"
                | "execution.missing"
        );
        if !relevant {
            continue;
        }
        if ev.kind == "execution.retry_queued"
            && ev.data["queued_for_delivery"].as_bool() == Some(true)
        {
            continue;
        }

        let terminal_id = ev.data["terminal_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                ev.actor
                    .strip_prefix("agent:")
                    .filter(|s| !s.is_empty())
                    .map(|p| format!("{}:{p}", ev.card_id))
            })
            .unwrap_or_else(|| format!("{}:developer", ev.card_id));
        let profile_id =
            extract_profile_from_terminal_or_actor(&ev.card_id, &terminal_id, &ev.actor);
        let explicit_num = ev.data["run_number"].as_u64().unwrap_or(0) as u32;
        let has_wf = ev.data["workflow_run_id"]
            .as_str()
            .is_some_and(|s| !s.is_empty());
        let trigger = classify_trigger(
            ev.data["trigger"].as_str(),
            ev.data["origin"].as_str(),
            has_wf,
        );

        let runs = by_terminal.entry(terminal_id.clone()).or_default();

        match ev.kind.as_str() {
            "execution.queued" | "execution.retry_queued" => {
                let key = if explicit_num > 0 {
                    explicit_num
                } else {
                    synthetic_seq += 1;
                    synthetic_seq
                };
                let entry = runs.entry(key).or_insert_with(|| ReconstructedRun {
                    profile_id,
                    trigger,
                    queued_at: Some(ev.created_at),
                    started_at: None,
                    scheduled_at: ev.created_at,
                    outcome: OutcomeKind::Active,
                    cancelled_before_start: false,
                });
                if entry.queued_at.is_none() {
                    entry.queued_at = Some(ev.created_at);
                }
                if ev.created_at > 0
                    && (entry.scheduled_at == 0 || ev.created_at < entry.scheduled_at)
                {
                    entry.scheduled_at = ev.created_at;
                }
            }
            "execution.started" => {
                let target_key = if explicit_num > 0 && runs.contains_key(&explicit_num) {
                    explicit_num
                } else if explicit_num > 0 {
                    if let Some((&old_k, _)) = runs.iter().find(|(k, r)| {
                        **k >= 1_000_000
                            && r.started_at.is_none()
                            && r.outcome == OutcomeKind::Active
                    }) {
                        let mut moved = runs.remove(&old_k).unwrap();
                        moved.started_at = Some(ev.created_at);
                        runs.insert(explicit_num, moved);
                        continue;
                    }
                    explicit_num
                } else if let Some((&open_k, _)) = runs
                    .iter()
                    .rev()
                    .find(|(_, r)| r.started_at.is_none() && r.outcome == OutcomeKind::Active)
                {
                    open_k
                } else {
                    synthetic_seq += 1;
                    synthetic_seq
                };
                let entry = runs.entry(target_key).or_insert_with(|| ReconstructedRun {
                    profile_id,
                    trigger,
                    queued_at: None,
                    started_at: Some(ev.created_at),
                    scheduled_at: ev.created_at,
                    outcome: OutcomeKind::Active,
                    cancelled_before_start: false,
                });
                if entry.started_at.is_none() {
                    entry.started_at = Some(ev.created_at);
                }
            }
            "execution.continued" => {
                if explicit_num > 0 && !runs.contains_key(&explicit_num) {
                    let pending_key = runs
                        .iter()
                        .find(|(k, r)| {
                            (**k < explicit_num || **k >= 1_000_000)
                                && r.started_at.is_none()
                                && r.outcome == OutcomeKind::Active
                        })
                        .map(|(&k, _)| k);
                    if let Some(old_k) = pending_key {
                        let mut moved = runs.remove(&old_k).unwrap();
                        moved.started_at = Some(ev.created_at);
                        runs.insert(explicit_num, moved);
                        continue;
                    }
                }
                let key = if explicit_num > 0 {
                    explicit_num
                } else {
                    synthetic_seq += 1;
                    synthetic_seq
                };
                let entry = runs.entry(key).or_insert_with(|| ReconstructedRun {
                    profile_id,
                    trigger,
                    queued_at: None,
                    started_at: Some(ev.created_at),
                    scheduled_at: ev.created_at,
                    outcome: OutcomeKind::Active,
                    cancelled_before_start: false,
                });
                if entry.started_at.is_none() {
                    entry.started_at = Some(ev.created_at);
                }
            }
            "execution.reported" => {
                let state = ev.data["state"].as_str().unwrap_or_default();
                let mapped = match state {
                    "completed" | "backlog" => Some(OutcomeKind::Completed),
                    "blocked" => Some(OutcomeKind::Blocked),
                    "failed" => Some(OutcomeKind::Failed),
                    _ => None,
                };
                let target_key = if explicit_num > 0 && runs.contains_key(&explicit_num) {
                    Some(explicit_num)
                } else {
                    runs.iter()
                        .rev()
                        .find(|(_, r)| {
                            r.outcome == OutcomeKind::Active && r.scheduled_at <= ev.created_at
                        })
                        .or_else(|| {
                            runs.iter()
                                .rev()
                                .find(|(_, r)| r.scheduled_at <= ev.created_at)
                        })
                        .map(|(&k, _)| k)
                };
                if let Some(k) = target_key {
                    if let Some(out) = mapped
                        && let Some(r) = runs.get_mut(&k)
                    {
                        r.outcome = out;
                    }
                } else if state != "working" {
                    let key = if explicit_num > 0 {
                        explicit_num
                    } else {
                        synthetic_seq += 1;
                        synthetic_seq
                    };
                    runs.insert(
                        key,
                        ReconstructedRun {
                            profile_id,
                            trigger,
                            queued_at: None,
                            started_at: Some(ev.created_at),
                            scheduled_at: ev.created_at,
                            outcome: mapped.unwrap_or(OutcomeKind::Active),
                            cancelled_before_start: false,
                        },
                    );
                }
            }
            "execution.failed" | "execution.missing" => {
                let target_key = if explicit_num > 0 && runs.contains_key(&explicit_num) {
                    Some(explicit_num)
                } else {
                    runs.iter()
                        .rev()
                        .find(|(_, r)| {
                            r.outcome == OutcomeKind::Active && r.scheduled_at <= ev.created_at
                        })
                        .map(|(&k, _)| k)
                };
                if let Some(k) = target_key
                    && let Some(r) = runs.get_mut(&k)
                    && r.outcome != OutcomeKind::Completed
                {
                    r.outcome = OutcomeKind::Failed;
                }
            }
            _ => {}
        }
    }

    for entry in &snapshot.terminals {
        let Some(agent) = &entry.agent else {
            continue;
        };
        if !in_scope_project(&agent.project_id) || !valid_cards.contains_key(agent.card_id.as_str())
        {
            continue;
        }
        let runs = by_terminal.entry(entry.id.clone()).or_default();
        let key = if agent.number > 0 {
            agent.number
        } else {
            synthetic_seq += 1;
            synthetic_seq
        };
        let is_cancelled_unstarted = matches!(agent.state.as_str(), "cancelled" | "superseded")
            && agent.started_at.is_none();
        let trigger_str = agent.trigger.to_string();
        let trigger = classify_trigger(Some(&trigger_str), None, agent.workflow_run_id.is_some());
        let profile_id = if agent.profile.id.is_empty() {
            extract_profile_from_terminal_or_actor(&agent.card_id, &entry.id, "")
        } else {
            agent.profile.id.clone()
        };
        let scheduled_at = agent
            .started_at
            .or((agent.updated_at > 0).then_some(agent.updated_at))
            .unwrap_or(entry.created_at);

        if let Some(existing) = runs.get_mut(&key) {
            if is_cancelled_unstarted && existing.started_at.is_none() {
                existing.cancelled_before_start = true;
            } else {
                if existing.started_at.is_none() {
                    existing.started_at = agent.started_at;
                }
                if existing.outcome == OutcomeKind::Active {
                    existing.outcome = match agent.state.as_str() {
                        "completed" | "backlog" => OutcomeKind::Completed,
                        "blocked" => OutcomeKind::Blocked,
                        "failed" => OutcomeKind::Failed,
                        _ => OutcomeKind::Active,
                    };
                }
            }
        } else if !is_cancelled_unstarted && scheduled_at > 0 {
            let outcome = match agent.state.as_str() {
                "completed" | "backlog" => OutcomeKind::Completed,
                "blocked" => OutcomeKind::Blocked,
                "failed" => OutcomeKind::Failed,
                "cancelled" | "superseded" => continue,
                _ => OutcomeKind::Active,
            };
            runs.insert(
                key,
                ReconstructedRun {
                    profile_id,
                    trigger,
                    queued_at: None,
                    started_at: agent.started_at,
                    scheduled_at,
                    outcome,
                    cancelled_before_start: false,
                },
            );
        }

        if let Some(pending) = &agent.pending
            && pending.queued_at > 0
        {
            let already_recorded = runs.values().any(|r| {
                r.started_at.is_none()
                    && r.outcome == OutcomeKind::Active
                    && r.queued_at
                        .is_some_and(|q| q.abs_diff(pending.queued_at) <= 2_000)
            });
            if !already_recorded {
                synthetic_seq += 1;
                let p_trigger_str = pending.trigger.to_string();
                let p_trigger = classify_trigger(
                    Some(&p_trigger_str),
                    None,
                    pending.workflow_run_id.is_some(),
                );
                let p_profile = if pending.profile.id.is_empty() {
                    extract_profile_from_terminal_or_actor(&agent.card_id, &entry.id, "")
                } else {
                    pending.profile.id.clone()
                };
                runs.insert(
                    synthetic_seq,
                    ReconstructedRun {
                        profile_id: p_profile,
                        trigger: p_trigger,
                        queued_at: Some(pending.queued_at),
                        started_at: None,
                        scheduled_at: pending.queued_at,
                        outcome: OutcomeKind::Active,
                        cancelled_before_start: false,
                    },
                );
            }
        }
    }

    let mut out: Vec<ReconstructedRun> = by_terminal
        .into_values()
        .flat_map(|m| m.into_values())
        .filter(|r| !r.cancelled_before_start && r.scheduled_at > 0)
        .collect();

    for batch in &snapshot.merge_batches {
        if batch.prompt.trim().is_empty()
            || batch.state == "cancelled"
            || batch.created_at == 0
            || !in_scope_project(&batch.project_id)
        {
            continue;
        }
        let outcome = match batch.state.as_str() {
            "completed" => OutcomeKind::Completed,
            "blocked" => OutcomeKind::Blocked,
            "failed" => OutcomeKind::Failed,
            _ => OutcomeKind::Active,
        };
        let profile_id = if batch.profile_id.trim().is_empty() {
            "merger".into()
        } else {
            batch.profile_id.clone()
        };
        out.push(ReconstructedRun {
            profile_id,
            trigger: TriggerKind::Workflow,
            queued_at: Some(batch.created_at),
            started_at: Some(batch.created_at),
            scheduled_at: batch.created_at,
            outcome,
            cancelled_before_start: false,
        });
    }

    out
}

pub fn compute<Tz: TimeZone>(
    snapshot: &Snapshot,
    scope_project: Option<&str>,
    range: RangeWindow,
    now_ms: u64,
    tz: &Tz,
) -> Dashboard {
    let scope_filter = scope_project.map(str::trim).filter(|s| !s.is_empty());
    let known_projects: BTreeMap<&str, &str> = snapshot
        .projects
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();
    let enforce_known_projects = !known_projects.is_empty();

    let in_scope_project = |pid: &str| -> bool {
        if let Some(target) = scope_filter {
            return pid == target;
        }
        !enforce_known_projects || known_projects.contains_key(pid)
    };

    let today = local_date(now_ms, tz);
    let span_days = range.days();
    let window_start = today - Duration::days(span_days - 1);
    let prev_end = window_start - Duration::days(1);
    let prev_start = prev_end - Duration::days(span_days - 1);

    let current_week_monday =
        today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
    let heatmap_start = current_week_monday - Duration::weeks(11);

    let valid_cards: BTreeMap<&str, &Card> = snapshot
        .cards
        .iter()
        .filter(|c| c.removed_at.is_none() && in_scope_project(&c.project_id))
        .map(|c| (c.id.as_str(), c))
        .collect();

    let mut events_by_card: BTreeMap<&str, Vec<&canban_types::Event>> = BTreeMap::new();
    for ev in &snapshot.events {
        if valid_cards.contains_key(ev.card_id.as_str()) {
            events_by_card
                .entry(ev.card_id.as_str())
                .or_default()
                .push(ev);
        }
    }
    for list in events_by_card.values_mut() {
        list.sort_by_key(|e| (e.created_at, e.sequence));
    }

    let mut daily_created: BTreeMap<NaiveDate, usize> = BTreeMap::new();
    let mut daily_local_merges: BTreeMap<NaiveDate, usize> = BTreeMap::new();
    let mut daily_pr_merges: BTreeMap<NaiveDate, usize> = BTreeMap::new();

    let mut created_total = 0usize;
    let mut merged_total = 0usize;
    let mut prev_merged_total = 0usize;
    let mut local_merges = 0usize;
    let mut pr_merges = 0usize;
    let mut lead_times_ms = Vec::new();
    let mut has_any_history = !valid_cards.is_empty();

    for card in valid_cards.values() {
        if card.created_at > 0 {
            let c_date = local_date(card.created_at, tz);
            *daily_created.entry(c_date).or_default() += 1;
            if c_date >= window_start && c_date <= today {
                created_total += 1;
            }
        }

        // Strict merge-only completion rule ("只统计 merge"):
        // Unmerged archived cards (`archived_at.is_some() && !card.merged()`) are never counted.
        if !card.merged() {
            continue;
        }

        let card_events = events_by_card.get(card.id.as_str());
        let merged_at = resolve_merged_at(card, card_events);
        let m_date = local_date(merged_at, tz);
        let is_pr = card_is_pr_merge(card);
        if is_pr {
            *daily_pr_merges.entry(m_date).or_default() += 1;
        } else {
            *daily_local_merges.entry(m_date).or_default() += 1;
        }

        let started_work_at = resolve_work_started_at(card, merged_at, card_events);
        let lead_time_ms = (merged_at >= started_work_at && merged_at > 0)
            .then(|| merged_at.saturating_sub(started_work_at));

        if m_date >= window_start && m_date <= today {
            merged_total += 1;
            if is_pr {
                pr_merges += 1;
            } else {
                local_merges += 1;
            }
            if let Some(lt) = lead_time_ms {
                lead_times_ms.push(lt);
            }
        } else if m_date >= prev_start && m_date <= prev_end {
            prev_merged_total += 1;
        }
    }

    let runs = reconstruct_runs(snapshot, in_scope_project, &valid_cards);
    if !runs.is_empty() {
        has_any_history = true;
    }

    let mut daily_outcomes: BTreeMap<NaiveDate, [usize; 4]> = BTreeMap::new();
    let mut scheduled_total = 0usize;
    let mut prev_scheduled_total = 0usize;
    let mut completed_runs = 0usize;
    let mut blocked_runs = 0usize;
    let mut failed_runs = 0usize;
    let mut active_runs = 0usize;
    let mut queue_waits_ms = Vec::new();
    let mut profile_map: BTreeMap<String, ProfileStat> = BTreeMap::new();
    let mut trigger_counts: BTreeMap<TriggerKind, usize> = BTreeMap::new();

    for run in &runs {
        let r_date = local_date(run.scheduled_at, tz);
        let slot = daily_outcomes.entry(r_date).or_insert([0; 4]);
        match run.outcome {
            OutcomeKind::Completed => slot[0] += 1,
            OutcomeKind::Blocked => slot[1] += 1,
            OutcomeKind::Failed => slot[2] += 1,
            OutcomeKind::Active => slot[3] += 1,
        }

        if r_date >= window_start && r_date <= today {
            scheduled_total += 1;
            match run.outcome {
                OutcomeKind::Completed => completed_runs += 1,
                OutcomeKind::Blocked => blocked_runs += 1,
                OutcomeKind::Failed => failed_runs += 1,
                OutcomeKind::Active => active_runs += 1,
            }
            if let (Some(q), Some(s)) = (run.queued_at, run.started_at)
                && s >= q
            {
                queue_waits_ms.push(s - q);
            }
            let pstat = profile_map
                .entry(run.profile_id.clone())
                .or_insert_with(|| ProfileStat {
                    profile_id: run.profile_id.clone(),
                    total_runs: 0,
                    completed_runs: 0,
                    blocked_runs: 0,
                    failed_runs: 0,
                    active_runs: 0,
                });
            pstat.total_runs += 1;
            match run.outcome {
                OutcomeKind::Completed => pstat.completed_runs += 1,
                OutcomeKind::Blocked => pstat.blocked_runs += 1,
                OutcomeKind::Failed => pstat.failed_runs += 1,
                OutcomeKind::Active => pstat.active_runs += 1,
            }
            *trigger_counts.entry(run.trigger).or_default() += 1;
        } else if r_date >= prev_start && r_date <= prev_end {
            prev_scheduled_total += 1;
        }
    }

    let mut days = Vec::with_capacity(span_days as usize);
    let mut max_daily_kanban = 0usize;
    let mut max_daily_runs = 0usize;
    for offset in 0..span_days {
        let d = window_start + Duration::days(offset);
        let created_cards = daily_created.get(&d).copied().unwrap_or(0);
        let l_merges = daily_local_merges.get(&d).copied().unwrap_or(0);
        let p_merges = daily_pr_merges.get(&d).copied().unwrap_or(0);
        let merged_cards = l_merges + p_merges;
        let [comp, blk, fail, act] = daily_outcomes.get(&d).copied().unwrap_or([0; 4]);
        let bucket = DayBucket {
            date_key: d.format("%Y-%m-%d").to_string(),
            short_label: d.format("%m-%d").to_string(),
            weekday_label: weekday_short(d),
            created_cards,
            merged_cards,
            local_merges: l_merges,
            pr_merges: p_merges,
            completed_runs: comp,
            blocked_runs: blk,
            failed_runs: fail,
            active_runs: act,
        };
        max_daily_kanban = max_daily_kanban
            .max(bucket.created_cards)
            .max(bucket.merged_cards);
        max_daily_runs = max_daily_runs.max(bucket.total_runs());
        days.push(bucket);
    }

    let mut peak_heatmap_activity = 0usize;
    for w in 0..12 {
        for dow in 0..7 {
            let d = heatmap_start + Duration::days(w * 7 + dow);
            if d > today {
                continue;
            }
            let m = daily_local_merges.get(&d).copied().unwrap_or(0)
                + daily_pr_merges.get(&d).copied().unwrap_or(0);
            let r: usize = daily_outcomes.get(&d).map(|s| s.iter().sum()).unwrap_or(0);
            peak_heatmap_activity = peak_heatmap_activity.max(m + r);
        }
    }
    let scale_peak = peak_heatmap_activity.max(4);
    let mut heatmap_weeks = Vec::with_capacity(12);
    let mut prev_month = None;
    let mut last_header_w: Option<i64> = None;
    for w in 0..12 {
        let week_monday = heatmap_start + Duration::days(w * 7);
        let month_num = week_monday.month();
        let month_header = if prev_month != Some(month_num) {
            prev_month = Some(month_num);
            if last_header_w.is_none_or(|lw| w - lw >= 2) {
                last_header_w = Some(w);
                Some(month_short(month_num).to_owned())
            } else {
                None
            }
        } else {
            None
        };
        let cells = std::array::from_fn(|dow| {
            let d = week_monday + Duration::days(dow as i64);
            let is_future = d > today;
            let merged_cards = if is_future {
                0
            } else {
                daily_local_merges.get(&d).copied().unwrap_or(0)
                    + daily_pr_merges.get(&d).copied().unwrap_or(0)
            };
            let total_runs = if is_future {
                0
            } else {
                daily_outcomes.get(&d).map(|s| s.iter().sum()).unwrap_or(0)
            };
            let activity = merged_cards + total_runs;
            let level = if is_future || activity == 0 {
                0
            } else {
                let ratio = activity as f32 / scale_peak as f32;
                if ratio <= 0.25 {
                    1
                } else if ratio <= 0.50 {
                    2
                } else if ratio <= 0.75 {
                    3
                } else {
                    4
                }
            };
            HeatmapCell {
                date_key: d.format("%Y-%m-%d").to_string(),
                short_label: d.format("%m-%d").to_string(),
                weekday_label: weekday_short(d),
                merged_cards,
                total_runs,
                level,
                is_future,
                is_today: d == today,
                in_range: d >= window_start && d <= today,
            }
        });
        heatmap_weeks.push(HeatmapWeek {
            month_header,
            cells,
        });
    }

    let mut profile_stats: Vec<ProfileStat> = profile_map.into_values().collect();
    profile_stats.sort_by(|a, b| {
        b.total_runs
            .cmp(&a.total_runs)
            .then_with(|| a.profile_id.cmp(&b.profile_id))
    });

    let trigger_stats = [
        TriggerKind::Workflow,
        TriggerKind::Mention,
        TriggerKind::Interactive,
    ]
    .into_iter()
    .map(|kind| {
        let count = trigger_counts.get(&kind).copied().unwrap_or(0);
        let share_pct = (count * 100 + scheduled_total / 2)
            .checked_div(scheduled_total)
            .unwrap_or(0);
        TriggerStat {
            kind,
            count,
            share_pct,
        }
    })
    .collect();

    let resolved_runs = completed_runs + blocked_runs + failed_runs;
    let success_rate_pct =
        (resolved_runs > 0).then(|| (completed_runs * 100 + resolved_runs / 2) / resolved_runs);

    Dashboard {
        range,
        today_key: today.format("%Y-%m-%d").to_string(),
        window_start_key: window_start.format("%Y-%m-%d").to_string(),
        merged_total,
        prev_merged_total,
        local_merges,
        pr_merges,
        created_total,
        net_wip_delta: created_total as isize - merged_total as isize,
        scheduled_total,
        prev_scheduled_total,
        completed_runs,
        blocked_runs,
        failed_runs,
        active_runs,
        success_rate_pct,
        median_lead_time_ms: median_u64(lead_times_ms),
        median_queue_wait_ms: median_u64(queue_waits_ms),
        days,
        max_daily_kanban,
        max_daily_runs,
        heatmap_weeks,
        profile_stats,
        trigger_stats,
        has_any_history,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{Delivery, Event, MergeBatch, MergeBatchItem, Project, Workflow};
    use chrono::FixedOffset;
    use serde_json::json;

    fn tz_plus_8() -> FixedOffset {
        FixedOffset::east_opt(8 * 3600).unwrap()
    }

    fn ts_utc8(y: i32, m: u32, d: u32, hh: u32, mm: u32) -> u64 {
        tz_plus_8()
            .with_ymd_and_hms(y, m, d, hh, mm, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64
    }

    fn sample_project(id: &str, name: &str) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            directory: format!("/tmp/{id}"),
            location: None,
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: BTreeMap::new(),
            tags: Vec::new(),
            workflow: Workflow::default(),
            target_branch: Some("main".into()),
            created_at: 1,
        }
    }

    fn sample_card(id: &str, project_id: &str, number: u64, created_at: u64) -> Card {
        serde_json::from_value(json!({
            "id": id,
            "project_id": project_id,
            "number": number,
            "title": format!("Card #{number}"),
            "body": "",
            "stage": "develop",
            "revision": 1,
            "created_at": created_at,
        }))
        .unwrap()
    }

    #[test]
    fn empty_snapshot_produces_clean_zero_metrics() {
        let snap = Snapshot::default();
        let now = ts_utc8(2026, 11, 15, 18, 0);
        let dash = compute(&snap, None, RangeWindow::Days30, now, &tz_plus_8());
        assert_eq!(dash.merged_total, 0);
        assert_eq!(dash.scheduled_total, 0);
        assert_eq!(dash.created_total, 0);
        assert_eq!(dash.net_wip_delta, 0);
        assert_eq!(dash.success_rate_pct, None);
        assert_eq!(dash.median_lead_time_ms, None);
        assert_eq!(dash.median_queue_wait_ms, None);
        assert_eq!(dash.days.len(), 30);
        assert_eq!(dash.heatmap_weeks.len(), 12);
        assert!(!dash.has_any_history);
    }

    #[test]
    fn counts_only_merged_cards_and_excludes_unmerged_archived_or_removed_cards() {
        let mut snap = Snapshot::default();
        snap.projects.push(sample_project("p1", "Canban"));
        let now = ts_utc8(2026, 11, 15, 20, 0);

        let mut c_local = sample_card("c-local", "p1", 1, ts_utc8(2026, 11, 14, 10, 0));
        c_local.delivery = Some(Delivery {
            target_branch: "main".into(),
            commit: "abc1234".into(),
            request_key: "merge:c-local".into(),
            merged_at: ts_utc8(2026, 11, 15, 11, 0),
        });
        c_local.archived_at = Some(ts_utc8(2026, 11, 15, 11, 0));
        snap.cards.push(c_local);

        let mut c_pr = sample_card("c-pr", "p1", 2, ts_utc8(2026, 11, 13, 9, 0));
        c_pr.delivery = Some(Delivery {
            target_branch: "main".into(),
            commit: "def5678".into(),
            request_key: "github:jjyr/canban#42".into(),
            merged_at: ts_utc8(2026, 11, 15, 0, 30),
        });
        snap.cards.push(c_pr);

        let mut c_archived_unmerged =
            sample_card("c-unmerged", "p1", 3, ts_utc8(2026, 11, 15, 9, 0));
        c_archived_unmerged.archived_at = Some(ts_utc8(2026, 11, 15, 14, 0));
        snap.cards.push(c_archived_unmerged);

        let mut c_removed = sample_card("c-removed", "p1", 4, ts_utc8(2026, 11, 15, 9, 0));
        c_removed.removed_at = Some(ts_utc8(2026, 11, 15, 15, 0));
        c_removed.delivery = Some(Delivery {
            target_branch: "main".into(),
            commit: "000".into(),
            request_key: "merge:removed".into(),
            merged_at: ts_utc8(2026, 11, 15, 12, 0),
        });
        snap.cards.push(c_removed);

        let dash = compute(&snap, None, RangeWindow::Days7, now, &tz_plus_8());
        assert_eq!(dash.merged_total, 2);
        assert_eq!(dash.local_merges, 1);
        assert_eq!(dash.pr_merges, 1);
        assert_eq!(dash.created_total, 3);
        assert_eq!(dash.net_wip_delta, 1);

        let today_bucket = dash.days.last().unwrap();
        assert_eq!(today_bucket.date_key, "2026-11-15");
        assert_eq!(today_bucket.merged_cards, 2);
        assert_eq!(today_bucket.local_merges, 1);
        assert_eq!(today_bucket.pr_merges, 1);
    }

    #[test]
    fn deduplicates_agent_runs_across_queued_continued_interim_working_and_merger_batches() {
        let mut snap = Snapshot::default();
        snap.projects.push(sample_project("p1", "Canban"));
        snap.projects.push(sample_project("p2", "Other"));
        let now = ts_utc8(2026, 11, 15, 20, 0);

        snap.cards
            .push(sample_card("c1", "p1", 10, ts_utc8(2026, 11, 14, 8, 0)));
        snap.cards
            .push(sample_card("c2", "p2", 20, ts_utc8(2026, 11, 15, 8, 0)));

        let mut seq = 0u64;
        let mut push_ev =
            |pid: &str, cid: &str, kind: &str, actor: &str, ts: u64, data: serde_json::Value| {
                seq += 1;
                snap.events.push(Event {
                    id: format!("ev-{seq}"),
                    project_id: pid.into(),
                    card_id: cid.into(),
                    sequence: seq,
                    kind: kind.into(),
                    actor: actor.into(),
                    message: String::new(),
                    data,
                    created_at: ts,
                });
            };

        push_ev(
            "p1",
            "c1",
            "execution.queued",
            "system",
            ts_utc8(2026, 11, 14, 9, 0),
            json!({"terminal_id": "c1:developer", "run_number": 1, "trigger": "workflow", "workflow_run_id": "wf-1"}),
        );
        push_ev(
            "p1",
            "c1",
            "execution.started",
            "system",
            ts_utc8(2026, 11, 14, 9, 2),
            json!({"terminal_id": "c1:developer", "run_number": 1, "trigger": "workflow", "workflow_run_id": "wf-1"}),
        );
        push_ev(
            "p1",
            "c1",
            "execution.reported",
            "agent:developer",
            ts_utc8(2026, 11, 14, 9, 10),
            json!({"state": "working", "workflow_run_id": "wf-1"}),
        );
        push_ev(
            "p1",
            "c1",
            "execution.reported",
            "agent:developer",
            ts_utc8(2026, 11, 14, 9, 25),
            json!({"state": "completed", "workflow_run_id": "wf-1"}),
        );

        push_ev(
            "p1",
            "c1",
            "execution.queued",
            "user",
            ts_utc8(2026, 11, 15, 10, 0),
            json!({"terminal_id": "c1:reviewer", "run_number": 2, "trigger": "comment:ev-99", "origin": "comment"}),
        );
        push_ev(
            "p1",
            "c1",
            "execution.started",
            "system",
            ts_utc8(2026, 11, 15, 10, 1),
            json!({"terminal_id": "c1:reviewer", "run_number": 2}),
        );
        push_ev(
            "p1",
            "c1",
            "execution.reported",
            "agent:reviewer",
            ts_utc8(2026, 11, 15, 10, 15),
            json!({"state": "blocked"}),
        );

        push_ev(
            "p1",
            "c1",
            "execution.queued",
            "user",
            ts_utc8(2026, 11, 15, 11, 0),
            json!({"terminal_id": "c1:developer", "run_number": 2, "trigger": "comment:ev-105", "origin": "comment"}),
        );
        push_ev(
            "p1",
            "c1",
            "execution.continued",
            "system",
            ts_utc8(2026, 11, 15, 11, 1),
            json!({"terminal_id": "c1:developer", "run_number": 3}),
        );
        push_ev(
            "p1",
            "c1",
            "execution.reported",
            "agent:developer",
            ts_utc8(2026, 11, 15, 11, 20),
            json!({"state": "completed"}),
        );

        snap.merge_batches.push(MergeBatch {
            id: "batch:1".into(),
            project_id: "p1".into(),
            profile_id: "merger".into(),
            provider_session_id: None,
            target_branch: "main".into(),
            target_head: "111".into(),
            strategy: Default::default(),
            sync_upstream: true,
            test_command: None,
            worktree: "/tmp/wt".into(),
            state: "completed".into(),
            failure_kind: None,
            conflict_card_number: Some(10),
            conflict_files: vec!["src/lib.rs".into()],
            test_output: None,
            items: vec![MergeBatchItem {
                card_id: "c1".into(),
                card_number: 10,
                workflow_run_id: "wf-1".into(),
                branch: "canban/10".into(),
                worktree: "/tmp/wt10".into(),
                base_head: "000".into(),
                integrated_commit: None,
                evicted: false,
            }],
            prompt: "Resolve conflict in src/lib.rs".into(),
            error: None,
            created_at: ts_utc8(2026, 11, 15, 12, 0),
            updated_at: ts_utc8(2026, 11, 15, 12, 10),
        });

        push_ev(
            "p2",
            "c2",
            "execution.queued",
            "user",
            ts_utc8(2026, 11, 15, 13, 0),
            json!({"terminal_id": "c2:planner", "run_number": 1, "trigger": "interactive", "origin": "manual"}),
        );
        push_ev(
            "p2",
            "c2",
            "execution.started",
            "system",
            ts_utc8(2026, 11, 15, 13, 4),
            json!({"terminal_id": "c2:planner", "run_number": 1}),
        );
        push_ev(
            "p2",
            "c2",
            "execution.failed",
            "system",
            ts_utc8(2026, 11, 15, 13, 8),
            json!({"terminal_id": "c2:planner", "run_number": 1}),
        );

        let all = compute(&snap, None, RangeWindow::Days7, now, &tz_plus_8());
        assert_eq!(all.scheduled_total, 5);
        assert_eq!(all.completed_runs, 3);
        assert_eq!(all.blocked_runs, 1);
        assert_eq!(all.failed_runs, 1);
        assert_eq!(all.active_runs, 0);
        assert_eq!(all.success_rate_pct, Some(60));

        let p1_only = compute(&snap, Some("p1"), RangeWindow::Days7, now, &tz_plus_8());
        assert_eq!(p1_only.scheduled_total, 4);
        assert_eq!(p1_only.completed_runs, 3);
        assert_eq!(p1_only.blocked_runs, 1);
        assert_eq!(p1_only.failed_runs, 0);
        assert_eq!(p1_only.success_rate_pct, Some(75));
        assert_eq!(
            p1_only
                .profile_stats
                .iter()
                .map(|p| (p.profile_id.as_str(), p.total_runs))
                .collect::<Vec<_>>(),
            vec![("developer", 2), ("merger", 1), ("reviewer", 1)]
        );
        assert_eq!(
            p1_only
                .trigger_stats
                .iter()
                .map(|t| (t.kind, t.count))
                .collect::<Vec<_>>(),
            vec![
                (TriggerKind::Workflow, 2),
                (TriggerKind::Mention, 2),
                (TriggerKind::Interactive, 0),
            ]
        );
    }

    #[test]
    fn computes_period_over_period_deltas_and_median_lead_time() {
        let mut snap = Snapshot::default();
        snap.projects.push(sample_project("p1", "Canban"));
        let now = ts_utc8(2026, 11, 15, 18, 0);

        let mut prev_card = sample_card("c-prev", "p1", 1, ts_utc8(2026, 11, 4, 10, 0));
        prev_card.delivery = Some(Delivery {
            target_branch: "main".into(),
            commit: "111".into(),
            request_key: "m1".into(),
            merged_at: ts_utc8(2026, 11, 5, 10, 0),
        });
        snap.cards.push(prev_card);

        for (idx, hours) in [(2u64, 2u64), (3u64, 4u64)] {
            let cid = format!("c-{idx}");
            let created = ts_utc8(2026, 11, 12, 8, 0);
            let work_start = ts_utc8(2026, 11, 12, 10, 0);
            let merged = work_start + hours * 3_600_000;
            let mut c = sample_card(&cid, "p1", idx, created);
            c.delivery = Some(Delivery {
                target_branch: "main".into(),
                commit: format!("sha{idx}"),
                request_key: format!("m{idx}"),
                merged_at: merged,
            });
            snap.cards.push(c);
            snap.events.push(Event {
                id: format!("ev-{idx}"),
                project_id: "p1".into(),
                card_id: cid,
                sequence: 1,
                kind: "card.stage_changed".into(),
                actor: "user".into(),
                message: "backlog → develop".into(),
                data: json!({}),
                created_at: work_start,
            });
        }

        let dash = compute(&snap, None, RangeWindow::Days7, now, &tz_plus_8());
        assert_eq!(dash.merged_total, 2);
        assert_eq!(dash.prev_merged_total, 1);
        assert_eq!(
            format_delta_pct(dash.merged_total, dash.prev_merged_total),
            "+100% vs prev"
        );
        assert_eq!(dash.median_lead_time_ms, Some(3 * 3_600_000));
        assert_eq!(format_duration_compact(3 * 3_600_000), "3h");
    }
}
