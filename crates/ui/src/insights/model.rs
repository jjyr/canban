//! Pure local-date aggregation of time allocation, Agent working duration,
//! Kanban throughput, pipeline automation, and same-progress period comparisons.
//!
//! Operates directly on `canban_types::Snapshot`, `Event`, and persisted
//! `ActivitySegment` / `ActivityBucket` data with zero extra IO.

use canban_types::{
    ActivityBucket, ActivityDetail, ActivityKind, ActivitySegment, Card, CardContent, DwellLane,
    Event, ExecutionTrigger, Profile, Snapshot,
};
use chrono::{Datelike, Duration, NaiveDate, TimeZone, Timelike};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RangeWindow {
    #[default]
    Today,
    Days7,
    Days30,
    Days90,
}

impl RangeWindow {
    pub fn days(self) -> i64 {
        match self {
            Self::Today => 1,
            Self::Days7 => 7,
            Self::Days30 => 30,
            Self::Days90 => 90,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "1D",
            Self::Days7 => "7D",
            Self::Days30 => "30D",
            Self::Days90 => "90D",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Today => "today",
            Self::Days7 => "last 7 days",
            Self::Days30 => "last 30 days",
            Self::Days90 => "last 90 days",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentGroup {
    #[default]
    Profile,
    Model,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeatmapMetric {
    #[default]
    Resolved,
    Dwell,
    Agent,
}

impl HeatmapMetric {
    pub fn label(self) -> &'static str {
        match self {
            Self::Resolved => "Resolved",
            Self::Dwell => "My time",
            Self::Agent => "Agent time",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectSort {
    #[default]
    Time,
    Merged,
    Created,
    Wip,
    Runs,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntakeKind {
    LocalManual,
    GitHubPr,
    GitHubIssue,
    InitialSync,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayBucket {
    pub date_key: String,
    pub short_label: String,
    pub weekday_label: &'static str,
    pub created_cards: usize,
    pub initial_sync_cards: usize,
    pub merged_cards: usize,
    pub local_merges: usize,
    pub pr_merges: usize,
    pub manual_resolutions: usize,
    pub completed_runs: usize,
    pub blocked_runs: usize,
    pub failed_runs: usize,
    pub active_runs: usize,
    pub human_touches: usize,
    pub cycle_p50_ms: Option<u64>,
}

impl DayBucket {
    pub fn total_runs(&self) -> usize {
        self.completed_runs + self.blocked_runs + self.failed_runs + self.active_runs
    }

    pub fn pass_rate_pct(&self) -> Option<usize> {
        let resolved = self.completed_runs + self.blocked_runs + self.failed_runs;
        (resolved > 0).then(|| (self.completed_runs * 100 + resolved / 2) / resolved)
    }

    pub fn total_activity(&self) -> usize {
        self.merged_cards + self.total_runs()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActivityInput {
    pub period_segments: Vec<ActivitySegment>,
    pub baseline_segments: Vec<ActivitySegment>,
    pub range_buckets: Vec<ActivityBucket>,
    pub baseline_buckets: Vec<ActivityBucket>,
    pub annual_buckets: Vec<ActivityBucket>,
    pub loaded: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimeTotals {
    pub dwell_total_ms: u64,
    pub dwell_card_ms: u64,
    pub dwell_terminal_ms: u64,
    pub dwell_other_ms: u64,
    pub prev_dwell_total_ms: u64,
    pub agent_total_ms: u64,
    pub prev_agent_total_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineLaneKind {
    MeCard,
    MeTerminal,
    MeOther,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineBlock {
    pub start_ms: u64,
    pub end_ms: u64,
    pub project_id: Option<String>,
    pub card_id: Option<String>,
    pub terminal_id: Option<String>,
    pub label: String,
    pub open: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineMergeMarker {
    pub timestamp_ms: u64,
    pub project_id: String,
    pub card_id: String,
    pub card_number: u64,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineLane {
    pub kind: TimelineLaneKind,
    pub label: String,
    pub sublabel: Option<String>,
    pub total_ms: u64,
    pub blocks: Vec<TimelineBlock>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimelineData {
    pub day_start_ms: u64,
    pub day_end_ms: u64,
    pub active_until_ms: u64,
    pub lanes: Vec<TimelineLane>,
    pub merges: Vec<TimelineMergeMarker>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayActivityRow {
    pub date_key: String,
    pub short_label: String,
    pub weekday_label: &'static str,
    pub is_today: bool,
    pub dwell_card_ms: u64,
    pub dwell_terminal_ms: u64,
    pub dwell_other_ms: u64,
    pub agent_ms: u64,
    pub resolved_cards: usize,
}

impl DayActivityRow {
    pub fn dwell_total_ms(&self) -> u64 {
        self.dwell_card_ms
            .saturating_add(self.dwell_terminal_ms)
            .saturating_add(self.dwell_other_ms)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutomationFunnel {
    pub created: usize,
    pub agent_touched: usize,
    pub reviewed: usize,
    pub merged: usize,
    pub human_asks: usize,
    pub zero_human_resolved: usize,
    pub prev_zero_human_resolved: usize,
    pub prev_merged: usize,
}

impl AutomationFunnel {
    pub fn zero_human_pct(&self) -> Option<usize> {
        (self.merged > 0).then(|| (self.zero_human_resolved * 100 + self.merged / 2) / self.merged)
    }

    pub fn prev_zero_human_pct(&self) -> Option<usize> {
        (self.prev_merged > 0).then(|| {
            (self.prev_zero_human_resolved * 100 + self.prev_merged / 2) / self.prev_merged
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelAgentStat {
    pub model: String,
    pub providers: BTreeSet<String>,
    pub profiles: BTreeSet<String>,
    pub cards_touched: usize,
    pub total_runs: usize,
    pub completed_runs: usize,
    pub blocked_runs: usize,
    pub failed_runs: usize,
    pub active_runs: usize,
    pub human_asks: usize,
    pub working_ms: u64,
    pub median_queue_wait_ms: Option<u64>,
    pub prev_median_queue_wait_ms: Option<u64>,
}

impl ModelAgentStat {
    pub fn pass_rate_pct(&self) -> Option<usize> {
        let resolved = self.completed_runs + self.blocked_runs + self.failed_runs;
        (resolved > 0).then(|| (self.completed_runs * 100 + resolved / 2) / resolved)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ViewOptions {
    pub range: RangeWindow,
    pub now_ms: u64,
    pub selected: Option<NaiveDate>,
    pub comparison: Comparison,
    pub heatmap_metric: HeatmapMetric,
    pub hourly_metric: ActivityMetric,
    pub agent_group: AgentGroup,
    pub trend_days: i64,
    pub exclude_initial_sync: bool,
}

impl ViewOptions {
    pub fn with_now(now_ms: u64) -> Self {
        Self {
            range: RangeWindow::Today,
            now_ms,
            selected: None,
            comparison: Comparison::Previous,
            heatmap_metric: HeatmapMetric::Resolved,
            hourly_metric: ActivityMetric::Merged,
            agent_group: AgentGroup::Profile,
            trend_days: 14,
            exclude_initial_sync: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityFetchPlan {
    pub period_from: u64,
    pub period_until: u64,
    pub prev_from: u64,
    pub prev_until: u64,
    pub range_boundaries: Vec<u64>,
    pub baseline_boundaries: Vec<u64>,
    pub annual_boundaries: Vec<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PipelineSnapshot {
    pub active_wip: usize,
    pub backlog: usize,
    pub develop: usize,
    pub review: usize,
    pub other_stage: usize,
    pub human_gated: usize,
    pub attention_cards: usize,
    pub merging_now: usize,
    pub merge_batches_period: usize,
    pub conflict_batches_period: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IntakeBreakdown {
    pub local_manual: usize,
    pub github_pr: usize,
    pub github_issue: usize,
    pub initial_sync: usize,
    pub total_raw: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScheduleSummary {
    pub configured: usize,
    pub runs_in_period: usize,
    pub completed: usize,
    pub failed_or_blocked: usize,
    pub active: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeatmapCell {
    pub date_key: String,
    pub short_label: String,
    pub weekday_label: &'static str,
    pub merged_cards: usize,
    pub created_cards: usize,
    pub total_runs: usize,
    pub dwell_ms: u64,
    pub agent_ms: u64,
    pub known: bool,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimingStat {
    pub avg_ms: Option<u64>,
    pub p50_ms: Option<u64>,
    pub samples: usize,
    pub prev_avg_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StageQueueTimings {
    pub develop_stage: TimingStat,
    pub develop_exec: TimingStat,
    pub review_stage: TimingStat,
    pub merge_queue: TimingStat,
    pub mention_reply: TimingStat,
    pub merge_queue_batches: usize,
    pub merge_queue_clean_batches: usize,
    pub merge_queue_conflict_batches: usize,
    pub merge_queue_avg_batch_size: Option<f64>,
    pub merge_queue_conflict_avg_ms: Option<u64>,
}

impl StageQueueTimings {
    pub fn clean_merge_rate_pct(&self) -> Option<usize> {
        (self.merge_queue_batches > 0).then(|| {
            (self.merge_queue_clean_batches * 100 + self.merge_queue_batches / 2)
                / self.merge_queue_batches
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileStat {
    pub card_ids: BTreeSet<String>,
    pub profile_id: String,
    pub provider: String,
    pub model: String,
    pub cards_touched: usize,
    pub median_queue_wait_ms: Option<u64>,
    pub prev_median_queue_wait_ms: Option<u64>,
    pub avg_exec_ms: Option<u64>,
    pub avg_mention_reply_ms: Option<u64>,
    pub working_ms: u64,
    pub total_runs: usize,
    pub overlapping_runs: usize,
    pub completed_runs: usize,
    pub blocked_runs: usize,
    pub failed_runs: usize,
    pub active_runs: usize,
    pub human_asks: usize,
}

impl ProfileStat {
    pub fn pass_rate_pct(&self) -> Option<usize> {
        let resolved = self.completed_runs + self.blocked_runs + self.failed_runs;
        (resolved > 0).then(|| (self.completed_runs * 100 + resolved / 2) / resolved)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerStat {
    pub kind: TriggerKind,
    pub count: usize,
    pub share_pct: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dashboard {
    pub exclude_initial_sync: bool,
    pub created_ids: Vec<String>,
    pub merged_ids: Vec<String>,
    pub selected_date: NaiveDate,
    pub period_label: String,
    pub comparison_label: String,
    pub timezone: String,
    pub baseline_known: bool,
    pub unknown_batch_starts: usize,
    pub previous_created: usize,
    pub reference_created: Option<f64>,
    pub reference_merged: Option<f64>,
    pub reference_days: usize,
    pub project_stats: Vec<ProjectStat>,
    pub providers: usize,
    pub used_profiles: usize,
    pub used_runs: usize,
    pub unknown_providers: usize,
    pub unknown_merge_dates: usize,
    pub hours: Vec<HourBucket>,
    pub efficiency: Efficiency,
    pub timings: StageQueueTimings,
    pub moving_average: Vec<Option<f64>>,
    pub time_totals: TimeTotals,
    pub timeline: TimelineData,
    pub day_rows: Vec<DayActivityRow>,
    pub funnel: AutomationFunnel,
    pub other_dwell_ms: u64,
    pub other_agent_ms: u64,
    pub activity_loaded: bool,
    pub pipeline: PipelineSnapshot,
    pub intake: IntakeBreakdown,
    pub schedules: ScheduleSummary,
    pub human_asks_period: usize,
    pub prev_human_asks_period: usize,
    pub workflow_runs_completed: usize,
    pub annual_start: NaiveDate,
    pub annual_end: NaiveDate,
    pub active_days: usize,
    pub current_streak: usize,
    pub longest_streak: usize,
    pub history_start: Option<NaiveDate>,
    pub range: RangeWindow,
    pub today_key: String,
    pub window_start_key: String,
    pub merged_total: usize,
    pub prev_merged_total: usize,
    pub local_merges: usize,
    pub pr_merges: usize,
    pub manual_resolutions: usize,
    pub created_total: usize,
    pub initial_sync_period: usize,
    pub net_wip_delta: isize,
    pub scheduled_total: usize,
    pub prev_scheduled_total: usize,
    pub completed_runs: usize,
    pub blocked_runs: usize,
    pub failed_runs: usize,
    pub active_runs: usize,
    pub prev_completed_runs: usize,
    pub prev_resolved_runs: usize,
    pub success_rate_pct: Option<usize>,
    pub prev_success_rate_pct: Option<usize>,
    pub median_lead_time_ms: Option<u64>,
    pub prev_median_lead_time_ms: Option<u64>,
    pub median_queue_wait_ms: Option<u64>,
    pub prev_median_queue_wait_ms: Option<u64>,
    pub days: Vec<DayBucket>,
    pub max_daily_kanban: usize,
    pub max_daily_runs: usize,
    pub heatmap_weeks: Vec<HeatmapWeek>,
    pub profile_stats: Vec<ProfileStat>,
    pub model_stats: Vec<ModelAgentStat>,
    pub trigger_stats: Vec<TriggerStat>,
    pub has_any_history: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Comparison {
    #[default]
    Previous,
    LastWeek,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActivityMetric {
    #[default]
    Merged,
    Created,
    Runs,
}
impl ActivityMetric {
    pub fn label(self) -> &'static str {
        match self {
            Self::Merged => "Resolved",
            Self::Created => "Created",
            Self::Runs => "Agent starts",
        }
    }
    pub fn value(self, created: usize, merged: usize, runs: usize) -> usize {
        match self {
            Self::Merged => merged,
            Self::Created => created,
            Self::Runs => runs,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectStat {
    pub id: String,
    pub name: String,
    pub created: usize,
    pub initial_sync: usize,
    pub merged: usize,
    pub wip_backlog: usize,
    pub wip_develop: usize,
    pub wip_review: usize,
    pub wip_gated: usize,
    pub agent_runs: usize,
    pub dwell_ms: u64,
    pub agent_ms: u64,
    pub cards: BTreeSet<String>,
    pub profiles: BTreeSet<String>,
    pub recent_merges: Vec<usize>,
}

impl ProjectStat {
    pub fn total_wip(&self) -> usize {
        self.wip_backlog + self.wip_develop + self.wip_review
    }

    pub fn total_time_ms(&self) -> u64 {
        self.dwell_ms.saturating_add(self.agent_ms)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurationSample {
    pub card_id: String,
    pub number: u64,
    pub title: String,
    pub project: String,
    pub profile: Option<String>,
    pub start: u64,
    pub end: u64,
    pub duration: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Efficiency {
    pub cycle: Vec<DurationSample>,
    pub queue: Vec<DurationSample>,
    pub previous_cycle: Vec<DurationSample>,
    pub previous_queue: Vec<DurationSample>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HourBucket {
    pub hour: u32,
    pub current: usize,
    pub previous: usize,
    pub future: bool,
}

#[derive(Clone, Debug)]
struct ReconstructedRun {
    terminal_id: String,
    run_number: u32,
    card_ids: Vec<String>,
    project_id: String,
    ended_at: Option<u64>,
    profile_id: String,
    explicit_provider: Option<String>,
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

fn mean_u64(values: &[u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let sum: u128 = values.iter().map(|&v| u128::from(v)).sum();
    Some((sum / values.len() as u128) as u64)
}

fn build_timing_stat(current: Vec<u64>, previous: Vec<u64>) -> TimingStat {
    let samples = current.len();
    let avg_ms = mean_u64(&current);
    let p50_ms = median_u64(current);
    let prev_avg_ms = mean_u64(&previous);
    TimingStat {
        avg_ms,
        p50_ms,
        samples,
        prev_avg_ms,
    }
}

fn parse_stage_transition(ev: &Event) -> Option<(String, String)> {
    let from = ev.data["from"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let to = ev.data["to"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let (Some(f), Some(t)) = (from, to) {
        return Some((f.to_ascii_lowercase(), t.to_ascii_lowercase()));
    }
    if let Some((lhs, rhs)) = ev
        .message
        .split_once('→')
        .or_else(|| ev.message.split_once("->"))
    {
        let f = lhs.trim();
        let t = rhs.trim();
        if !f.is_empty() && !t.is_empty() {
            return Some((f.to_ascii_lowercase(), t.to_ascii_lowercase()));
        }
    }
    if let Some(t) = to {
        return Some((String::new(), t.to_ascii_lowercase()));
    }
    None
}

/// Computes total dwell time (accumulating rework visits) and the last exit timestamp
/// for `target_stage` on a single card. `terminal_at` closes an open interval when the
/// card resolves or enters delivery without emitting an explicit stage exit.
fn compute_stage_dwell(
    events: &[&Event],
    target_stage: &str,
    terminal_at: Option<u64>,
) -> Option<(u64, u64)> {
    let mut entered_at: Option<u64> = None;
    let mut total_ms: u64 = 0;
    let mut last_exit_at: Option<u64> = None;

    for ev in events {
        if ev.kind != "card.stage_changed" || ev.created_at == 0 {
            continue;
        }
        let Some((from, to)) = parse_stage_transition(ev) else {
            continue;
        };
        if let Some(start) = entered_at {
            if to != target_stage && ev.created_at >= start {
                total_ms = total_ms.saturating_add(ev.created_at - start);
                last_exit_at = Some(ev.created_at);
                entered_at = None;
            }
        } else if to == target_stage && from != target_stage {
            entered_at = Some(ev.created_at);
        }
    }

    if let (Some(start), Some(end)) = (entered_at, terminal_at)
        && end >= start
    {
        total_ms = total_ms.saturating_add(end - start);
        last_exit_at = Some(end);
    }

    last_exit_at.map(|exit_ts| (total_ms, exit_ts))
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

pub fn format_duration_delta(
    current: Option<u64>,
    previous: Option<u64>,
) -> Option<(String, bool)> {
    let (a, b) = (current?, previous?);
    if a == b {
        return Some(("0s vs prev".into(), true));
    }
    let diff = format_duration_compact(a.abs_diff(b));
    if a < b {
        let pct = ((b - a) * 100 + b / 2) / b.max(1);
        Some((format!("−{diff} (−{pct}%)"), true))
    } else {
        let pct = ((a - b) * 100 + b / 2) / b.max(1);
        Some((format!("+{diff} (+{pct}%)"), false))
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

fn classify_card_intake(card: &Card, card_events: Option<&Vec<&Event>>) -> IntakeKind {
    let created_ev = card_events.and_then(|evs| evs.iter().find(|e| e.kind == "card.created"));
    if let Some(ev) = created_ev {
        if ev.data["initial_sync"].as_bool() == Some(true) {
            return IntakeKind::InitialSync;
        }
        match ev.data["source_kind"].as_str().unwrap_or_default() {
            "github.pr" => return IntakeKind::GitHubPr,
            "github.issue" => return IntakeKind::GitHubIssue,
            _ => {}
        }
    }
    match &card.content {
        CardContent::GitHubPr { .. } => IntakeKind::GitHubPr,
        CardContent::GitHubIssue { .. } => IntakeKind::GitHubIssue,
        CardContent::Local => {
            if card.pull_request.as_ref().is_some_and(|pr| pr.number > 0)
                && card.created_by.is_empty()
            {
                IntakeKind::GitHubPr
            } else {
                IntakeKind::LocalManual
            }
        }
    }
}

fn resolve_merged_at(card: &Card, card_events: Option<&Vec<&Event>>) -> Option<u64> {
    if let Some(ts) = card.resolved_at() {
        return Some(ts);
    }
    if let Some(events) = card_events
        && let Some(ts) = events
            .iter()
            .rev()
            .find(|e| {
                matches!(
                    e.kind.as_str(),
                    "review.merged"
                        | "github.pr_merged"
                        | "github.pr_delivered"
                        | "github.pr.merged"
                        | "card.resolved"
                )
            })
            .map(|e| e.created_at)
            .filter(|&t| t > 0)
    {
        return Some(ts);
    }
    None
}

fn resolve_work_started_at(
    _card: &Card,
    merged_at: u64,
    card_events: Option<&Vec<&Event>>,
) -> Option<u64> {
    card_events?
        .iter()
        .filter(|ev| {
            ev.created_at > 0
                && ev.created_at <= merged_at
                && matches!(
                    ev.kind.as_str(),
                    "execution.started" | "execution.continued"
                )
        })
        .map(|ev| ev.created_at)
        .min()
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
    events_by_card: &BTreeMap<&str, Vec<&Event>>,
) -> Vec<ReconstructedRun> {
    let mut by_terminal: BTreeMap<String, BTreeMap<u32, ReconstructedRun>> = BTreeMap::new();
    let mut synthetic_seq: u32 = 1_000_000;

    let mut sorted_events: Vec<&Event> = events_by_card
        .values()
        .flat_map(|evs| evs.iter().copied())
        .filter(|ev| {
            matches!(
                ev.kind.as_str(),
                "execution.queued"
                    | "execution.retry_queued"
                    | "execution.started"
                    | "execution.continued"
                    | "execution.reported"
                    | "execution.failed"
                    | "execution.missing"
                    | "execution.close_requested"
                    | "execution.idle_closed"
            )
        })
        .collect();
    sorted_events.sort_by_key(|ev| (ev.created_at, ev.sequence));

    for ev in sorted_events {
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
                    terminal_id: terminal_id.clone(),
                    run_number: explicit_num,
                    card_ids: vec![ev.card_id.clone()],
                    project_id: ev.project_id.clone(),
                    ended_at: None,
                    profile_id,
                    explicit_provider: None,
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
                        moved.run_number = explicit_num;
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
                    terminal_id: terminal_id.clone(),
                    run_number: explicit_num,
                    card_ids: vec![ev.card_id.clone()],
                    project_id: ev.project_id.clone(),
                    ended_at: None,
                    profile_id,
                    explicit_provider: None,
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
                        moved.run_number = explicit_num;
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
                    terminal_id: terminal_id.clone(),
                    run_number: explicit_num,
                    card_ids: vec![ev.card_id.clone()],
                    project_id: ev.project_id.clone(),
                    ended_at: None,
                    profile_id,
                    explicit_provider: None,
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
                        r.ended_at = Some(ev.created_at);
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
                            terminal_id: terminal_id.clone(),
                            run_number: explicit_num,
                            card_ids: vec![ev.card_id.clone()],
                            project_id: ev.project_id.clone(),
                            ended_at: None,
                            profile_id,
                            explicit_provider: None,
                            trigger,
                            queued_at: None,
                            started_at: None,
                            scheduled_at: ev.created_at,
                            outcome: mapped.unwrap_or(OutcomeKind::Active),
                            cancelled_before_start: false,
                        },
                    );
                }
            }
            "execution.failed"
            | "execution.missing"
            | "execution.close_requested"
            | "execution.idle_closed" => {
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
                    r.ended_at = Some(ev.created_at);
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
        let explicit_prov = {
            let p = agent.profile.provider_name();
            (p != "Unknown").then_some(p)
        };
        let scheduled_at = agent
            .started_at
            .or((agent.updated_at > 0).then_some(agent.updated_at))
            .unwrap_or(entry.created_at);

        if let Some(existing) = runs.get_mut(&key) {
            if existing.explicit_provider.is_none() {
                existing.explicit_provider = explicit_prov;
            }
            if agent.state != "running" && agent.state != "queued" && agent.updated_at > 0 {
                existing.ended_at = existing.ended_at.or(Some(agent.updated_at));
            }
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
                    terminal_id: entry.id.clone(),
                    run_number: agent.number,
                    card_ids: vec![agent.card_id.clone()],
                    project_id: agent.project_id.clone(),
                    ended_at: matches!(
                        agent.state.as_str(),
                        "completed" | "backlog" | "blocked" | "failed" | "cancelled" | "superseded"
                    )
                    .then_some(agent.updated_at),
                    profile_id,
                    explicit_provider: explicit_prov,
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
                let p_prov = {
                    let p = pending.profile.provider_name();
                    (p != "Unknown").then_some(p)
                };
                runs.insert(
                    synthetic_seq,
                    ReconstructedRun {
                        terminal_id: entry.id.clone(),
                        run_number: 0,
                        card_ids: vec![agent.card_id.clone()],
                        project_id: agent.project_id.clone(),
                        ended_at: matches!(
                            agent.state.as_str(),
                            "completed"
                                | "backlog"
                                | "blocked"
                                | "failed"
                                | "cancelled"
                                | "superseded"
                        )
                        .then_some(agent.updated_at),
                        profile_id: p_profile,
                        explicit_provider: p_prov,
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

    let terminal_providers: BTreeMap<&str, String> = snapshot
        .terminals
        .iter()
        .filter_map(|t| {
            let a = t.agent.as_ref()?;
            let p = a.profile.provider_name();
            (p != "Unknown").then_some((t.id.as_str(), p))
        })
        .collect();

    let mut out: Vec<ReconstructedRun> = by_terminal
        .into_values()
        .flat_map(|m| m.into_values())
        .filter(|r| !r.cancelled_before_start && r.scheduled_at > 0)
        .map(|mut r| {
            if r.explicit_provider.is_none()
                && let Some(tp) = terminal_providers.get(r.terminal_id.as_str())
            {
                r.explicit_provider = Some(tp.clone());
            }
            r
        })
        .collect();

    for batch in &snapshot.merge_batches {
        if snapshot
            .terminals
            .iter()
            .any(|t| t.id == batch.id && t.agent.is_some())
        {
            continue;
        }
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
            terminal_id: batch.id.clone(),
            run_number: 1,
            card_ids: batch
                .items
                .iter()
                .filter(|i| valid_cards.contains_key(i.card_id.as_str()))
                .map(|i| i.card_id.clone())
                .collect(),
            project_id: batch.project_id.clone(),
            ended_at: (!batch.active()).then_some(batch.updated_at),
            profile_id,
            explicit_provider: None,
            trigger: TriggerKind::Workflow,
            queued_at: batch.queued_at,
            started_at: batch.started_at,
            scheduled_at: batch.created_at,
            outcome,
            cancelled_before_start: false,
        });
    }

    out
}

fn boundary<Tz: TimeZone>(date: NaiveDate, time: chrono::NaiveTime, tz: &Tz) -> u64 {
    let mut naive = date.and_time(time);
    for _ in 0..=180 {
        if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
            return dt.timestamp_millis().max(0) as u64;
        }
        naive += Duration::minutes(1);
    }
    0
}

pub fn percentile(samples: &[DurationSample], p: usize) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut values: Vec<_> = samples.iter().map(|s| s.duration).collect();
    values.sort_unstable();
    if p == 50 {
        return median_u64(values);
    }
    Some(
        values[(values.len() * p)
            .div_ceil(100)
            .saturating_sub(1)
            .min(values.len() - 1)],
    )
}

pub fn activity_fetch_plan<Tz: TimeZone>(
    range: RangeWindow,
    now_ms: u64,
    tz: &Tz,
    selected: Option<NaiveDate>,
    comparison: Comparison,
) -> ActivityFetchPlan {
    let now = chrono::DateTime::from_timestamp_millis(now_ms as i64)
        .unwrap()
        .with_timezone(tz);
    let actual_today = now.date_naive();
    let today = selected.unwrap_or(actual_today).min(actual_today);
    let partial = today == actual_today;
    let start = today - Duration::days(range.days() - 1);
    let midnight = chrono::NaiveTime::MIN;
    let period_from = boundary(start, midnight, tz);
    let period_until = if partial {
        now_ms.saturating_add(1)
    } else {
        boundary(today + Duration::days(1), midnight, tz)
    };
    let shift = if range == RangeWindow::Today && comparison == Comparison::LastWeek {
        7
    } else {
        range.days()
    };
    let previous_start = start - Duration::days(shift);
    let previous_date = today - Duration::days(shift);
    let prev_from = boundary(previous_start, midnight, tz);
    let prev_until = if partial {
        boundary(previous_date, now.time(), tz).saturating_add(1)
    } else {
        boundary(previous_date + Duration::days(1), midnight, tz)
    };

    let day_bounds = |d0: NaiveDate, days: i64, end_cap: u64| -> Vec<u64> {
        let mut bounds = Vec::with_capacity(days as usize + 1);
        for i in 0..=days {
            let ts = if i == days {
                end_cap
            } else {
                boundary(d0 + Duration::days(i), midnight, tz)
            };
            if bounds.last().is_none_or(|&prev| ts > prev) {
                bounds.push(ts);
            }
        }
        bounds
    };
    let range_boundaries = day_bounds(start, range.days(), period_until);
    let baseline_boundaries = day_bounds(previous_start, range.days(), prev_until);

    let annual_start = actual_today - Duration::days(364);
    let mut annual_boundaries = Vec::with_capacity(366);
    for i in 0..=365 {
        let ts = if i == 365 {
            now_ms.saturating_add(1)
        } else {
            boundary(annual_start + Duration::days(i), midnight, tz)
        };
        if annual_boundaries.last().is_none_or(|&prev| ts > prev) {
            annual_boundaries.push(ts);
        }
    }

    ActivityFetchPlan {
        period_from,
        period_until,
        prev_from,
        prev_until,
        range_boundaries,
        baseline_boundaries,
        annual_boundaries,
    }
}

pub fn time_totals(range: RangeWindow, activity: &ActivityInput) -> TimeTotals {
    let period_segs: Vec<&ActivitySegment> = activity.period_segments.iter().collect();
    let baseline_segs: Vec<&ActivitySegment> = activity.baseline_segments.iter().collect();
    let range_buckets: Vec<&ActivityBucket> = activity.range_buckets.iter().collect();
    let baseline_buckets: Vec<&ActivityBucket> = activity.baseline_buckets.iter().collect();
    time_totals_filtered(
        range,
        &period_segs,
        &baseline_segs,
        &range_buckets,
        &baseline_buckets,
    )
}

fn time_totals_filtered(
    range: RangeWindow,
    period_segments: &[&ActivitySegment],
    baseline_segments: &[&ActivitySegment],
    range_buckets: &[&ActivityBucket],
    baseline_buckets: &[&ActivityBucket],
) -> TimeTotals {
    let mut totals = TimeTotals::default();
    if range == RangeWindow::Today {
        for seg in period_segments {
            let dur = seg.duration_ms();
            match (&seg.kind, &seg.data) {
                (ActivityKind::Dwell, ActivityDetail::Dwell(d)) => {
                    totals.dwell_total_ms += dur;
                    match d.lane {
                        DwellLane::Card => totals.dwell_card_ms += dur,
                        DwellLane::Terminal => totals.dwell_terminal_ms += dur,
                        DwellLane::Other => totals.dwell_other_ms += dur,
                    }
                }
                (ActivityKind::Agent, ActivityDetail::Agent(_)) => {
                    totals.agent_total_ms += dur;
                }
                _ => {}
            }
        }
        for seg in baseline_segments {
            let dur = seg.duration_ms();
            match seg.kind {
                ActivityKind::Dwell => totals.prev_dwell_total_ms += dur,
                ActivityKind::Agent => totals.prev_agent_total_ms += dur,
            }
        }
    } else {
        for b in range_buckets {
            totals.dwell_card_ms += b.dwell.card;
            totals.dwell_terminal_ms += b.dwell.terminal;
            totals.dwell_other_ms += b.dwell.other;
            totals.dwell_total_ms += b.dwell.total();
            totals.agent_total_ms += b.agent_ms;
        }
        for b in baseline_buckets {
            totals.prev_dwell_total_ms += b.dwell.total();
            totals.prev_agent_total_ms += b.agent_ms;
        }
    }
    totals
}

fn push_timeline_block(blocks: &mut Vec<TimelineBlock>, block: TimelineBlock) {
    if block.end_ms <= block.start_ms {
        return;
    }
    if let Some(prev) = blocks.last_mut()
        && prev.project_id == block.project_id
        && prev.card_id == block.card_id
        && prev.terminal_id == block.terminal_id
        && block.start_ms <= prev.end_ms.saturating_add(5_000)
    {
        prev.end_ms = prev.end_ms.max(block.end_ms);
        prev.open |= block.open;
        return;
    }
    blocks.push(block);
}

fn describe_segment_target(seg: &ActivitySegment, snapshot: &Snapshot, fallback: &str) -> String {
    if let Some(cid) = seg.card_id.as_deref()
        && let Some(card) = snapshot.cards.iter().find(|c| c.id == cid)
    {
        return format!("#{} {}", card.number, card.title);
    }
    if let Some(tid) = seg.terminal_id.as_deref()
        && let Some(term) = snapshot.terminals.iter().find(|t| t.id == tid)
    {
        return term.title();
    }
    if let Some(pid) = seg.project_id.as_deref()
        && let Some(proj) = snapshot.projects.iter().find(|p| p.id == pid)
    {
        return proj.name.clone();
    }
    fallback.to_owned()
}

pub fn timeline_lanes<Tz: TimeZone>(
    snapshot: &Snapshot,
    segments: &[ActivitySegment],
    merges: Vec<TimelineMergeMarker>,
    today: NaiveDate,
    partial_now_ms: Option<u64>,
    tz: &Tz,
    agent_group: AgentGroup,
) -> TimelineData {
    let midnight = chrono::NaiveTime::MIN;
    let day_start_ms = boundary(today, midnight, tz);
    let day_end_ms = boundary(today + Duration::days(1), midnight, tz).max(day_start_ms + 1);
    let active_until_ms =
        partial_now_ms.map_or(day_end_ms, |ms| ms.clamp(day_start_ms, day_end_ms));

    let mut me_card = TimelineLane {
        kind: TimelineLaneKind::MeCard,
        label: "Me · Card".into(),
        sublabel: None,
        total_ms: 0,
        blocks: Vec::new(),
    };
    let mut me_terminal = TimelineLane {
        kind: TimelineLaneKind::MeTerminal,
        label: "Me · Terminal".into(),
        sublabel: None,
        total_ms: 0,
        blocks: Vec::new(),
    };
    let mut me_other = TimelineLane {
        kind: TimelineLaneKind::MeOther,
        label: "Me · Other".into(),
        sublabel: None,
        total_ms: 0,
        blocks: Vec::new(),
    };

    let mut agent_lanes: BTreeMap<String, (Option<String>, Vec<TimelineBlock>, u64)> =
        BTreeMap::new();

    let mut sorted: Vec<&ActivitySegment> = segments.iter().collect();
    sorted.sort_by_key(|s| (s.start_ms, s.end_ms));

    for seg in sorted {
        let start = seg.start_ms.max(day_start_ms);
        let end = seg.end_ms.min(active_until_ms);
        if end <= start {
            continue;
        }
        let dur = end - start;
        match (&seg.kind, &seg.data) {
            (ActivityKind::Dwell, ActivityDetail::Dwell(dwell)) => {
                let target_label = describe_segment_target(seg, snapshot, &dwell.page);
                let block = TimelineBlock {
                    start_ms: start,
                    end_ms: end,
                    project_id: seg.project_id.clone(),
                    card_id: seg.card_id.clone(),
                    terminal_id: seg.terminal_id.clone(),
                    label: target_label,
                    open: false,
                };
                match dwell.lane {
                    DwellLane::Card => {
                        me_card.total_ms += dur;
                        push_timeline_block(&mut me_card.blocks, block);
                    }
                    DwellLane::Terminal => {
                        me_terminal.total_ms += dur;
                        push_timeline_block(&mut me_terminal.blocks, block);
                    }
                    DwellLane::Other => {
                        me_other.total_ms += dur;
                        push_timeline_block(&mut me_other.blocks, block);
                    }
                }
            }
            (ActivityKind::Agent, ActivityDetail::Agent(agent)) => {
                let (lane_key, sublabel) = match agent_group {
                    AgentGroup::Profile => (
                        if agent.profile_id == "manual" {
                            format!("Manual ({})", agent.provider)
                        } else {
                            format!("@{}", agent.profile_id)
                        },
                        Some(agent.provider.clone()),
                    ),
                    AgentGroup::Model => (agent.model.clone(), Some(agent.provider.clone())),
                };
                let target_label = describe_segment_target(seg, snapshot, &lane_key);
                let entry = agent_lanes
                    .entry(lane_key)
                    .or_insert_with(|| (sublabel, Vec::new(), 0));
                entry.2 += dur;
                push_timeline_block(
                    &mut entry.1,
                    TimelineBlock {
                        start_ms: start,
                        end_ms: end,
                        project_id: seg.project_id.clone(),
                        card_id: seg.card_id.clone(),
                        terminal_id: seg.terminal_id.clone(),
                        label: target_label,
                        open: seg.open,
                    },
                );
            }
            _ => {}
        }
    }

    let mut lanes = vec![me_card, me_terminal, me_other];
    let mut agent_vec: Vec<TimelineLane> = agent_lanes
        .into_iter()
        .map(|(label, (sublabel, blocks, total_ms))| TimelineLane {
            kind: TimelineLaneKind::Agent,
            label,
            sublabel,
            total_ms,
            blocks,
        })
        .collect();
    agent_vec.sort_by(|a, b| {
        b.total_ms
            .cmp(&a.total_ms)
            .then_with(|| a.label.cmp(&b.label))
    });
    lanes.extend(agent_vec);

    TimelineData {
        day_start_ms,
        day_end_ms,
        active_until_ms,
        lanes,
        merges,
    }
}

pub fn day_rows<Tz: TimeZone>(
    days: &[DayBucket],
    buckets: &[ActivityBucket],
    actual_today: NaiveDate,
    tz: &Tz,
) -> Vec<DayActivityRow> {
    let mut by_date: BTreeMap<String, &ActivityBucket> = BTreeMap::new();
    for b in buckets {
        let mid = b
            .start_ms
            .saturating_add(b.end_ms.saturating_sub(b.start_ms) / 2);
        by_date.insert(local_date(mid, tz).to_string(), b);
    }
    let today_key = actual_today.to_string();
    days.iter()
        .map(|d| {
            let act = by_date.get(&d.date_key);
            DayActivityRow {
                date_key: d.date_key.clone(),
                short_label: d.short_label.clone(),
                weekday_label: d.weekday_label,
                is_today: d.date_key == today_key,
                dwell_card_ms: act.map_or(0, |b| b.dwell.card),
                dwell_terminal_ms: act.map_or(0, |b| b.dwell.terminal),
                dwell_other_ms: act.map_or(0, |b| b.dwell.other),
                agent_ms: act.map_or(0, |b| b.agent_ms),
                resolved_cards: d.merged_cards,
            }
        })
        .collect()
}

pub fn group_agents_by_model(
    stats: &[ProfileStat],
    prev_queue_by_model: &BTreeMap<String, Option<u64>>,
) -> Vec<ModelAgentStat> {
    let mut grouped: BTreeMap<String, (ModelAgentStat, BTreeSet<String>, Vec<u64>)> =
        BTreeMap::new();
    for p in stats {
        let entry = grouped.entry(p.model.clone()).or_insert_with(|| {
            (
                ModelAgentStat {
                    model: p.model.clone(),
                    providers: BTreeSet::new(),
                    profiles: BTreeSet::new(),
                    cards_touched: 0,
                    total_runs: 0,
                    completed_runs: 0,
                    blocked_runs: 0,
                    failed_runs: 0,
                    active_runs: 0,
                    human_asks: 0,
                    working_ms: 0,
                    median_queue_wait_ms: None,
                    prev_median_queue_wait_ms: prev_queue_by_model.get(&p.model).copied().flatten(),
                },
                BTreeSet::new(),
                Vec::new(),
            )
        });
        let (agg, cards, waits) = entry;
        agg.providers.insert(p.provider.clone());
        agg.profiles.insert(p.profile_id.clone());
        cards.extend(p.card_ids.iter().cloned());
        agg.total_runs += p.total_runs;
        agg.completed_runs += p.completed_runs;
        agg.blocked_runs += p.blocked_runs;
        agg.failed_runs += p.failed_runs;
        agg.active_runs += p.active_runs;
        agg.human_asks += p.human_asks;
        agg.working_ms += p.working_ms;
        if let Some(q) = p.median_queue_wait_ms {
            waits.push(q);
        }
    }
    let mut out: Vec<ModelAgentStat> = grouped
        .into_values()
        .map(|(mut agg, cards, waits)| {
            agg.cards_touched = cards.len();
            agg.median_queue_wait_ms = median_u64(waits);
            agg
        })
        .collect();
    out.sort_by(|a, b| {
        b.working_ms
            .cmp(&a.working_ms)
            .then_with(|| b.total_runs.cmp(&a.total_runs))
            .then_with(|| a.model.cmp(&b.model))
    });
    out
}

#[cfg(test)]
pub fn compute<Tz: TimeZone>(
    snapshot: &Snapshot,
    events: &[Event],
    scope_project: Option<&str>,
    range: RangeWindow,
    now_ms: u64,
    tz: &Tz,
) -> Dashboard {
    let mut opts = ViewOptions::with_now(now_ms);
    opts.range = range;
    compute_dashboard(
        snapshot,
        events,
        &ActivityInput::default(),
        scope_project,
        tz,
        &[],
        opts,
    )
}

pub fn compute_dashboard<Tz: TimeZone>(
    snapshot: &Snapshot,
    events: &[Event],
    activity: &ActivityInput,
    scope_project: Option<&str>,
    tz: &Tz,
    profiles: &[Profile],
    opts: ViewOptions,
) -> Dashboard {
    let ViewOptions {
        range,
        now_ms,
        selected,
        comparison,
        heatmap_metric,
        hourly_metric,
        agent_group,
        trend_days,
        exclude_initial_sync,
    } = opts;
    let scope = scope_project.filter(|s| !s.is_empty());
    let in_scope = |pid: &str| {
        scope.map_or_else(
            || snapshot.projects.is_empty() || snapshot.projects.iter().any(|p| p.id == pid),
            |s| s == pid,
        )
    };
    let now = chrono::DateTime::from_timestamp_millis(now_ms as i64)
        .unwrap()
        .with_timezone(tz);
    let actual_today = now.date_naive();
    let today = selected.unwrap_or(actual_today).min(actual_today);
    let partial = today == actual_today;
    let start = today - Duration::days(range.days() - 1);
    let midnight = chrono::NaiveTime::MIN;
    let from = boundary(start, midnight, tz);
    let until = if partial {
        now_ms.saturating_add(1)
    } else {
        boundary(today + Duration::days(1), midnight, tz)
    };
    let shift = if range == RangeWindow::Today && comparison == Comparison::LastWeek {
        7
    } else {
        range.days()
    };
    let previous_start = start - Duration::days(shift);
    let previous_date = today - Duration::days(shift);
    let prev_from = boundary(previous_start, midnight, tz);
    let prev_until = if partial {
        boundary(previous_date, now.time(), tz).saturating_add(1)
    } else {
        boundary(previous_date + Duration::days(1), midnight, tz)
    };
    let current = |ts: u64| ts > 0 && ts >= from && ts < until;
    let previous = |ts: u64| ts > 0 && ts >= prev_from && ts < prev_until;

    let cur_segments: Vec<&ActivitySegment> = activity
        .period_segments
        .iter()
        .filter(|s| {
            s.end_ms > from
                && s.start_ms < until
                && scope.is_none_or(|pid| s.project_id.as_deref() == Some(pid))
        })
        .collect();
    let prev_segments: Vec<&ActivitySegment> = activity
        .baseline_segments
        .iter()
        .filter(|s| {
            s.end_ms > prev_from
                && s.start_ms < prev_until
                && scope.is_none_or(|pid| s.project_id.as_deref() == Some(pid))
        })
        .collect();
    let cur_buckets: Vec<&ActivityBucket> = activity
        .range_buckets
        .iter()
        .filter(|b| b.end_ms > from && b.start_ms < until)
        .collect();
    let prev_buckets: Vec<&ActivityBucket> = activity
        .baseline_buckets
        .iter()
        .filter(|b| b.end_ms > prev_from && b.start_ms < prev_until)
        .collect();

    let valid_cards: BTreeMap<&str, &Card> = snapshot
        .cards
        .iter()
        .filter(|c| c.removed_at.is_none() && in_scope(&c.project_id))
        .map(|c| (c.id.as_str(), c))
        .collect();

    let mut events_by_card: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
    for ev in events
        .iter()
        .filter(|e| e.created_at <= now_ms && valid_cards.contains_key(e.card_id.as_str()))
    {
        events_by_card.entry(&ev.card_id).or_default().push(ev);
    }
    for card_events in events_by_card.values_mut() {
        card_events.sort_by_key(|e| (e.created_at, e.sequence));
    }

    let activity_start = activity
        .annual_buckets
        .iter()
        .filter(|b| b.dwell.total() > 0 || b.agent_ms > 0)
        .map(|b| local_date(b.start_ms, tz))
        .chain(
            activity
                .range_buckets
                .iter()
                .filter(|b| b.dwell.total() > 0 || b.agent_ms > 0)
                .map(|b| local_date(b.start_ms, tz)),
        )
        .chain(
            activity
                .period_segments
                .iter()
                .filter(|s| s.duration_ms() > 0)
                .map(|s| local_date(s.start_ms, tz)),
        )
        .min();

    let history_start = valid_cards
        .values()
        .map(|c| c.created_at)
        .chain(
            snapshot
                .projects
                .iter()
                .filter(|p| in_scope(&p.id))
                .map(|p| p.created_at),
        )
        .chain(events_by_card.values().flatten().map(|e| e.created_at))
        .filter(|t| *t > 0 && *t <= now_ms)
        .min()
        .map(|t| local_date(t, tz))
        .into_iter()
        .chain(activity_start)
        .min();

    let mut projects: BTreeMap<String, ProjectStat> = snapshot
        .projects
        .iter()
        .filter(|p| in_scope(&p.id))
        .map(|p| {
            (
                p.id.clone(),
                ProjectStat {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    ..Default::default()
                },
            )
        })
        .collect();

    let mut pipeline = PipelineSnapshot {
        merging_now: snapshot
            .merging_cards
            .iter()
            .filter(|cid| valid_cards.contains_key(cid.as_str()))
            .count(),
        ..Default::default()
    };
    let mut intake = IntakeBreakdown::default();

    let mut daily_created = BTreeMap::<NaiveDate, usize>::new();
    let mut daily_initial_sync = BTreeMap::<NaiveDate, usize>::new();
    let mut daily_local = BTreeMap::<NaiveDate, usize>::new();
    let mut daily_pr = BTreeMap::<NaiveDate, usize>::new();
    let mut daily_manual = BTreeMap::<NaiveDate, usize>::new();
    let mut daily_runs = BTreeMap::<NaiveDate, [usize; 4]>::new();
    let mut daily_cycles = BTreeMap::<NaiveDate, Vec<u64>>::new();
    let mut daily_touches = BTreeMap::<NaiveDate, usize>::new();

    let mut efficiency = Efficiency::default();
    let mut created_ids = Vec::new();
    let mut merged_ids = Vec::new();
    let mut created_total = 0;
    let mut initial_sync_period = 0;
    let mut previous_created = 0;
    let mut merged_total = 0;
    let mut prev_merged_total = 0;
    let mut local_merges = 0;
    let mut pr_merges = 0;
    let mut manual_resolutions = 0;
    let mut unknown_merge_dates = 0;
    let mut created_times = Vec::new();
    let mut merge_times = Vec::new();
    let mut start_times = Vec::new();
    let mut human_asks_by_profile_period = BTreeMap::<String, usize>::new();
    let mut human_asks_period = 0usize;
    let mut prev_human_asks_period = 0usize;
    let mut reviewed_cards: BTreeSet<String> = BTreeSet::new();
    let mut zero_human_resolved = 0usize;
    let mut prev_zero_human_resolved = 0usize;
    let mut merges_on_selected_day: Vec<TimelineMergeMarker> = Vec::new();

    let mut project_merges: BTreeMap<String, BTreeMap<NaiveDate, usize>> = BTreeMap::new();

    let workflow_runs_by_id: BTreeMap<&str, &canban_types::WorkflowRun> = snapshot
        .workflow_runs
        .iter()
        .filter(|w| in_scope(&w.project_id))
        .map(|w| (w.id.as_str(), w))
        .collect();
    let merge_batches_by_id: BTreeMap<&str, &canban_types::MergeBatch> = snapshot
        .merge_batches
        .iter()
        .filter(|b| in_scope(&b.project_id))
        .map(|b| (b.id.as_str(), b))
        .collect();
    let questions_by_id: BTreeMap<&str, &canban_types::Question> = snapshot
        .questions
        .iter()
        .filter(|q| in_scope(&q.project_id))
        .map(|q| (q.id.as_str(), q))
        .collect();
    let mut comment_created_by_id: BTreeMap<&str, u64> = BTreeMap::new();

    let mut develop_stage_cur = Vec::new();
    let mut develop_stage_prev = Vec::new();
    let mut review_stage_cur = Vec::new();
    let mut review_stage_prev = Vec::new();
    let mut merge_queue_cur = Vec::new();
    let mut merge_queue_prev = Vec::new();
    let mut mention_cur = Vec::new();
    let mut mention_prev = Vec::new();
    let mut mention_by_profile_period: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    // batch_id -> (batch_size, had_conflict)
    let mut mq_batches_period: BTreeMap<String, (usize, bool)> = BTreeMap::new();
    let mut mq_conflict_durations: Vec<u64> = Vec::new();

    for (&cid, evs) in &events_by_card {
        for ev in evs {
            if ev.kind == "comment.created" && ev.created_at > 0 {
                comment_created_by_id.insert(ev.id.as_str(), ev.created_at);
            }
            if matches!(
                ev.kind.as_str(),
                "execution.input_required" | "review.merge_conflict"
            ) && ev.created_at > 0
            {
                let d = local_date(ev.created_at, tz);
                *daily_touches.entry(d).or_default() += 1;
                if current(ev.created_at) {
                    human_asks_period += 1;
                    if ev.kind == "execution.input_required" {
                        let tid = ev.data["terminal_id"].as_str().unwrap_or_default();
                        let prof = extract_profile_from_terminal_or_actor(cid, tid, &ev.actor);
                        *human_asks_by_profile_period.entry(prof).or_default() += 1;
                    }
                }
                if previous(ev.created_at) {
                    prev_human_asks_period += 1;
                }
            }
        }
    }

    for evs in events_by_card.values() {
        for ev in evs {
            if ev.kind != "comment.replied" || ev.created_at == 0 || ev.created_at > now_ms {
                continue;
            }
            let question = ev
                .data
                .get("question_id")
                .and_then(|v| v.as_str())
                .and_then(|qid| questions_by_id.get(qid).copied());
            let start_ts = question
                .map(|q| q.created_at)
                .filter(|&t| t > 0)
                .or_else(|| {
                    ev.data
                        .get("reply_to")
                        .and_then(|v| v.as_str())
                        .and_then(|cid| comment_created_by_id.get(cid).copied())
                });
            if let Some(s) = start_ts
                && s > 0
                && s <= ev.created_at
            {
                let dur = ev.created_at - s;
                if current(ev.created_at) {
                    mention_cur.push(dur);
                    let prof = ev
                        .actor
                        .strip_prefix("agent:")
                        .filter(|p| !p.is_empty())
                        .or_else(|| {
                            question
                                .map(|q| q.profile_id.as_str())
                                .filter(|p| !p.is_empty())
                        });
                    if let Some(p) = prof {
                        mention_by_profile_period
                            .entry(p.to_owned())
                            .or_default()
                            .push(dur);
                    }
                }
                if previous(ev.created_at) {
                    mention_prev.push(dur);
                }
            }
        }
    }

    for card in valid_cards.values() {
        let p = projects
            .entry(card.project_id.clone())
            .or_insert_with(|| ProjectStat {
                id: card.project_id.clone(),
                name: card.project_id.clone(),
                ..Default::default()
            });

        if card.active() && !card.resolved() {
            pipeline.active_wip += 1;
            match card.stage.as_str() {
                "backlog" => {
                    pipeline.backlog += 1;
                    p.wip_backlog += 1;
                }
                "develop" => {
                    pipeline.develop += 1;
                    p.wip_develop += 1;
                }
                "review" => {
                    pipeline.review += 1;
                    p.wip_review += 1;
                }
                _ => {
                    pipeline.other_stage += 1;
                    p.wip_backlog += 1;
                }
            }
            if card.human_gate.is_some() {
                pipeline.human_gated += 1;
                p.wip_gated += 1;
            }
            if card
                .attention
                .as_deref()
                .is_some_and(|a| !a.trim().is_empty())
            {
                pipeline.attention_cards += 1;
            }
        }

        let card_evs = events_by_card.get(card.id.as_str());
        let intake_kind = classify_card_intake(card, card_evs);
        let is_init_sync = intake_kind == IntakeKind::InitialSync;

        if card.created_at > 0 && card.created_at <= now_ms {
            let cd = local_date(card.created_at, tz);
            if is_init_sync {
                *daily_initial_sync.entry(cd).or_default() += 1;
            }
            let counts_as_created = !exclude_initial_sync || !is_init_sync;
            if counts_as_created {
                *daily_created.entry(cd).or_default() += 1;
                created_times.push(card.created_at);
            }
            if current(card.created_at) {
                intake.total_raw += 1;
                match intake_kind {
                    IntakeKind::LocalManual => intake.local_manual += 1,
                    IntakeKind::GitHubPr => intake.github_pr += 1,
                    IntakeKind::GitHubIssue => intake.github_issue += 1,
                    IntakeKind::InitialSync => {
                        intake.initial_sync += 1;
                        initial_sync_period += 1;
                        p.initial_sync += 1;
                    }
                }
                if counts_as_created {
                    created_ids.push(card.id.clone());
                    created_total += 1;
                    p.created += 1;
                    p.cards.insert(card.id.clone());
                }
            }
            if counts_as_created && previous(card.created_at) {
                previous_created += 1;
            }
        }
        let resolved_ts = if card.resolved() {
            resolve_merged_at(card, card_evs).filter(|&t| t <= now_ms)
        } else {
            None
        };

        let mut card_mq_queued_at: Option<u64> = None;
        if let Some(evs) = card_evs {
            for ev in evs {
                if ev.kind != "review.merged" || ev.created_at == 0 || ev.created_at > now_ms {
                    continue;
                }
                let batch_id = ev
                    .data
                    .get("batch_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty());
                let explicit_q = ev
                    .data
                    .get("queued_at")
                    .and_then(|v| v.as_u64())
                    .filter(|&t| t > 0);
                if batch_id.is_none() && explicit_q.is_none() {
                    continue;
                }
                let wf_run = ev
                    .data
                    .get("workflow_run_id")
                    .and_then(|v| v.as_str())
                    .and_then(|wid| workflow_runs_by_id.get(wid).copied());
                let batch_opt = batch_id.and_then(|bid| merge_batches_by_id.get(bid).copied());
                let queued_at = explicit_q
                    .or_else(|| {
                        wf_run.and_then(|w| {
                            w.outputs
                                .values()
                                .find_map(|v| v.get("queued_at").and_then(|q| q.as_u64()))
                                .filter(|&t| t > 0)
                        })
                    })
                    .or_else(|| batch_opt.map(|b| b.created_at).filter(|&t| t > 0))
                    .or_else(|| {
                        wf_run.and_then(|w| {
                            w.history
                                .iter()
                                .filter_map(|h| h["at"].as_u64())
                                .filter(|&t| t > 0 && t <= ev.created_at)
                                .max()
                        })
                    });
                if let Some(q) = queued_at
                    && q <= ev.created_at
                {
                    card_mq_queued_at = Some(q);
                    let dur = ev.created_at - q;
                    if current(ev.created_at) {
                        merge_queue_cur.push(dur);
                    }
                    if previous(ev.created_at) {
                        merge_queue_prev.push(dur);
                    }
                }
                if let Some(bid) = batch_id
                    && current(ev.created_at)
                {
                    let bsize = ev
                        .data
                        .get("batch_size")
                        .and_then(|v| v.as_u64())
                        .map(|n| n as usize)
                        .filter(|&n| n > 0)
                        .or_else(|| batch_opt.map(|b| b.items.len().max(1)))
                        .unwrap_or(1);
                    let had_conflict = batch_opt.is_some_and(|b| {
                        b.failure_kind.is_some()
                            || !b.conflict_files.is_empty()
                            || !b.prompt.trim().is_empty()
                    });
                    let entry = mq_batches_period
                        .entry(bid.to_owned())
                        .or_insert((bsize, had_conflict));
                    entry.0 = entry.0.max(bsize);
                    entry.1 |= had_conflict;
                }
            }

            if !card.resolved() && card.stage == "review" && card.created_at < until {
                reviewed_cards.insert(card.id.clone());
            }
            if evs.iter().any(|ev| {
                ev.kind == "card.stage_changed"
                    && current(ev.created_at)
                    && parse_stage_transition(ev).is_some_and(|(_, to)| to == "review")
            }) {
                reviewed_cards.insert(card.id.clone());
            }
            if let Some((dur, exit_ts)) = compute_stage_dwell(evs, "develop", resolved_ts) {
                if current(exit_ts) {
                    develop_stage_cur.push(dur);
                }
                if previous(exit_ts) {
                    develop_stage_prev.push(dur);
                }
            }
            let review_terminal = card_mq_queued_at.or(resolved_ts);
            if let Some((dur, exit_ts)) = compute_stage_dwell(evs, "review", review_terminal) {
                if current(exit_ts) {
                    review_stage_cur.push(dur);
                }
                if previous(exit_ts) {
                    review_stage_prev.push(dur);
                }
            }
        }

        if !card.resolved() {
            continue;
        }
        let Some(merged_at) = resolve_merged_at(card, card_evs) else {
            unknown_merge_dates += 1;
            continue;
        };
        if merged_at > now_ms {
            continue;
        }
        merge_times.push(merged_at);
        let d = local_date(merged_at, tz);
        let is_pr = card_is_pr_merge(card);
        if card.merged() {
            *if is_pr {
                daily_pr.entry(d).or_default()
            } else {
                daily_local.entry(d).or_default()
            } += 1;
        } else {
            *daily_manual.entry(d).or_default() += 1;
        }
        *project_merges
            .entry(card.project_id.clone())
            .or_default()
            .entry(d)
            .or_default() += 1;
        let had_human_ask = card_evs.is_some_and(|evs| {
            evs.iter().any(|e| {
                matches!(
                    e.kind.as_str(),
                    "execution.input_required" | "review.merge_conflict"
                )
            })
        });
        if current(merged_at) {
            merged_ids.push(card.id.clone());
            merged_total += 1;
            p.merged += 1;
            p.cards.insert(card.id.clone());
            if !had_human_ask {
                zero_human_resolved += 1;
            }
            if d == today {
                merges_on_selected_day.push(TimelineMergeMarker {
                    timestamp_ms: merged_at,
                    project_id: card.project_id.clone(),
                    card_id: card.id.clone(),
                    card_number: card.number,
                    title: card.title.clone(),
                });
            }
            if !card.merged() {
                manual_resolutions += 1;
            } else {
                reviewed_cards.insert(card.id.clone());
                if is_pr {
                    pr_merges += 1;
                } else {
                    local_merges += 1;
                }
            }
        }
        if previous(merged_at) {
            prev_merged_total += 1;
            if !had_human_ask {
                prev_zero_human_resolved += 1;
            }
        }
        if let Some(began) = resolve_work_started_at(card, merged_at, card_evs) {
            let dur = merged_at - began;
            daily_cycles.entry(d).or_default().push(dur);
            let sample = DurationSample {
                card_id: card.id.clone(),
                number: card.number,
                title: card.title.clone(),
                project: p.name.clone(),
                profile: None,
                start: began,
                end: merged_at,
                duration: dur,
            };
            if current(merged_at) {
                efficiency.cycle.push(sample.clone());
            }
            if previous(merged_at) {
                efficiency.previous_cycle.push(sample);
            }
        }
    }

    for batch in &snapshot.merge_batches {
        if !in_scope(&batch.project_id) || batch.created_at == 0 || batch.state == "cancelled" {
            continue;
        }
        let ts = batch.started_at.unwrap_or(batch.created_at);
        if current(ts) {
            pipeline.merge_batches_period += 1;
            let had_conflict = batch.failure_kind.as_deref() == Some("conflict")
                || batch.failure_kind.is_some()
                || !batch.conflict_files.is_empty()
                || !batch.prompt.trim().is_empty();
            if had_conflict {
                pipeline.conflict_batches_period += 1;
                if !batch.active() && batch.updated_at >= batch.created_at {
                    mq_conflict_durations.push(batch.updated_at - batch.created_at);
                }
            }
            let bsize = batch.items.len().max(1);
            let entry = mq_batches_period
                .entry(batch.id.clone())
                .or_insert((bsize, had_conflict));
            entry.0 = entry.0.max(bsize);
            entry.1 |= had_conflict;
        }
    }

    let workflow_runs_completed = snapshot
        .workflow_runs
        .iter()
        .filter(|w| in_scope(&w.project_id) && w.state == "completed" && current(w.updated_at))
        .count();

    let mut schedules = ScheduleSummary {
        configured: snapshot
            .schedule_states
            .iter()
            .filter(|st| st.project_id.is_empty() || in_scope(&st.project_id))
            .count(),
        ..Default::default()
    };
    for sr in &snapshot.scheduled_runs {
        if !sr.project_id.is_empty() && !in_scope(&sr.project_id) {
            continue;
        }
        let ts = sr.started_at.unwrap_or(sr.scheduled_at);
        if current(ts) {
            schedules.runs_in_period += 1;
            match sr.state.as_str() {
                "completed" => schedules.completed += 1,
                "failed" | "blocked" => schedules.failed_or_blocked += 1,
                "running" | "starting" | "queued" => schedules.active += 1,
                _ => {}
            }
        }
    }

    let mut runs = reconstruct_runs(snapshot, in_scope, &valid_cards, &events_by_card);
    runs.retain(|r| r.scheduled_at <= now_ms);
    let mut scheduled_total = 0;
    let mut prev_scheduled_total = 0;
    let mut outcomes = [0usize; 4];
    let mut prev_outcomes = [0usize; 4];
    let mut profile_map: BTreeMap<String, ProfileStat> = BTreeMap::new();
    let mut profile_waits: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut prev_profile_waits: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut model_waits: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut prev_model_waits: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut profile_execs: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut develop_exec_cur: Vec<u64> = Vec::new();
    let mut develop_exec_prev: Vec<u64> = Vec::new();
    let mut trigger_counts: BTreeMap<TriggerKind, usize> = BTreeMap::new();
    let mut agent_touched_cards: BTreeSet<String> = BTreeSet::new();
    let mut used_runs = 0;

    for run in &runs {
        let Some(began) = run.started_at.filter(|t| *t > 0 && *t <= now_ms) else {
            continue;
        };
        let index = match run.outcome {
            OutcomeKind::Completed => 0,
            OutcomeKind::Blocked => 1,
            OutcomeKind::Failed => 2,
            OutcomeKind::Active => 3,
        };
        daily_runs.entry(local_date(began, tz)).or_default()[index] += 1;
        start_times.push(began);
        if previous(began) {
            prev_scheduled_total += 1;
            prev_outcomes[index] += 1;
        }
        let overlaps = began < until && run.ended_at.unwrap_or(now_ms) >= from;
        if current(began) {
            scheduled_total += 1;
            outcomes[index] += 1;
            *trigger_counts.entry(run.trigger).or_default() += 1;
            if let Some(p) = projects.get_mut(&run.project_id) {
                p.agent_runs += 1;
            }
        }
        let provider = run
            .explicit_provider
            .clone()
            .or_else(|| {
                profiles
                    .iter()
                    .find(|p| p.id == run.profile_id)
                    .map(Profile::provider_name)
            })
            .or_else(|| {
                snapshot
                    .terminals
                    .iter()
                    .filter_map(|t| t.agent.as_ref())
                    .find(|a| a.project_id == run.project_id && a.profile.id == run.profile_id)
                    .map(|a| a.profile.provider_name())
            })
            .unwrap_or_else(|| "Unknown".into());
        let model = snapshot
            .terminals
            .iter()
            .filter_map(|t| t.agent.as_ref())
            .find(|a| a.project_id == run.project_id && a.profile.id == run.profile_id)
            .map(|a| a.profile.effective_model())
            .or_else(|| {
                profiles
                    .iter()
                    .find(|p| p.id == run.profile_id)
                    .map(Profile::effective_model)
            })
            .unwrap_or_else(|| "default".into());
        let group_key = format!("{}:{}", run.profile_id, provider.to_ascii_lowercase());

        if overlaps {
            used_runs += 1;
            let stat = profile_map
                .entry(group_key.clone())
                .or_insert_with(|| ProfileStat {
                    card_ids: BTreeSet::new(),
                    profile_id: run.profile_id.clone(),
                    provider: provider.clone(),
                    model: model.clone(),
                    cards_touched: 0,
                    median_queue_wait_ms: None,
                    prev_median_queue_wait_ms: None,
                    avg_exec_ms: None,
                    avg_mention_reply_ms: None,
                    working_ms: 0,
                    total_runs: 0,
                    overlapping_runs: 0,
                    completed_runs: 0,
                    blocked_runs: 0,
                    failed_runs: 0,
                    active_runs: 0,
                    human_asks: 0,
                });
            stat.overlapping_runs += 1;
            if current(began) {
                stat.total_runs += 1;
                match index {
                    0 => stat.completed_runs += 1,
                    1 => stat.blocked_runs += 1,
                    2 => stat.failed_runs += 1,
                    _ => stat.active_runs += 1,
                }
            }
            for id in &run.card_ids {
                stat.card_ids.insert(id.clone());
                agent_touched_cards.insert(id.clone());
                if let Some(p) = projects.get_mut(&run.project_id) {
                    p.cards.insert(id.clone());
                    p.profiles.insert(run.profile_id.clone());
                }
            }
        }
        if run.outcome != OutcomeKind::Active
            && let Some(ended) = run.ended_at.filter(|&e| e >= began && e <= now_ms)
        {
            let exec_dur = ended - began;
            if current(ended) {
                profile_execs
                    .entry(group_key.clone())
                    .or_default()
                    .push(exec_dur);
                if run.profile_id == "developer" {
                    develop_exec_cur.push(exec_dur);
                }
            }
            if previous(ended) && run.profile_id == "developer" {
                develop_exec_prev.push(exec_dur);
            }
        }
        if let Some(queued) = run.queued_at.filter(|q| *q > 0 && *q <= began) {
            let card = run
                .card_ids
                .iter()
                .find_map(|id| valid_cards.get(id.as_str()));
            let sample = DurationSample {
                card_id: card.map(|c| c.id.clone()).unwrap_or_default(),
                number: card.map(|c| c.number).unwrap_or(0),
                title: card
                    .map(|c| c.title.clone())
                    .unwrap_or_else(|| "Merge batch".into()),
                project: projects
                    .get(&run.project_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| run.project_id.clone()),
                profile: Some(run.profile_id.clone()),
                start: queued,
                end: began,
                duration: began - queued,
            };
            if current(began) {
                profile_waits
                    .entry(group_key.clone())
                    .or_default()
                    .push(sample.duration);
                model_waits
                    .entry(model.clone())
                    .or_default()
                    .push(sample.duration);
                efficiency.queue.push(sample.clone());
            }
            if previous(began) {
                prev_profile_waits
                    .entry(group_key)
                    .or_default()
                    .push(sample.duration);
                prev_model_waits
                    .entry(model)
                    .or_default()
                    .push(sample.duration);
                efficiency.previous_queue.push(sample);
            }
        }
    }

    let mut add_agent_work = |profile_id: &str, provider: &str, model: &str, ms: u64| {
        if ms == 0 {
            return;
        }
        let gkey = format!("{}:{}", profile_id, provider.to_ascii_lowercase());
        let stat = profile_map.entry(gkey).or_insert_with(|| ProfileStat {
            card_ids: BTreeSet::new(),
            profile_id: profile_id.to_owned(),
            provider: provider.to_owned(),
            model: model.to_owned(),
            cards_touched: 0,
            median_queue_wait_ms: None,
            prev_median_queue_wait_ms: None,
            avg_exec_ms: None,
            avg_mention_reply_ms: None,
            working_ms: 0,
            total_runs: 0,
            overlapping_runs: 0,
            completed_runs: 0,
            blocked_runs: 0,
            failed_runs: 0,
            active_runs: 0,
            human_asks: 0,
        });
        if stat.model == "default" && model != "default" && model != "unknown" {
            stat.model = model.to_owned();
        }
        stat.working_ms = stat.working_ms.saturating_add(ms);
    };
    if range == RangeWindow::Today {
        for seg in &cur_segments {
            if let (ActivityKind::Agent, ActivityDetail::Agent(agent)) = (&seg.kind, &seg.data) {
                add_agent_work(
                    &agent.profile_id,
                    &agent.provider,
                    &agent.model,
                    seg.duration_ms(),
                );
            }
        }
    } else {
        for bucket in &cur_buckets {
            for a in &bucket.by_agent {
                add_agent_work(&a.profile_id, &a.provider, &a.model, a.ms);
            }
        }
    }

    let mut assigned_ask_profiles = BTreeSet::new();
    let mut profile_stats: Vec<_> = profile_map
        .into_iter()
        .map(|(gkey, mut p)| {
            p.cards_touched = p.card_ids.len();
            p.median_queue_wait_ms = profile_waits.remove(&gkey).and_then(median_u64);
            p.prev_median_queue_wait_ms = prev_profile_waits.remove(&gkey).and_then(median_u64);
            p.avg_exec_ms = profile_execs.remove(&gkey).and_then(|vals| mean_u64(&vals));
            if assigned_ask_profiles.insert(p.profile_id.clone()) {
                p.human_asks = human_asks_by_profile_period
                    .get(&p.profile_id)
                    .copied()
                    .unwrap_or(0);
                p.avg_mention_reply_ms = mention_by_profile_period
                    .get(&p.profile_id)
                    .and_then(|vals| mean_u64(vals));
            }
            p
        })
        .collect();
    profile_stats.sort_by(|a, b| {
        b.working_ms
            .cmp(&a.working_ms)
            .then_with(|| b.total_runs.cmp(&a.total_runs))
            .then_with(|| b.overlapping_runs.cmp(&a.overlapping_runs))
            .then_with(|| a.profile_id.cmp(&b.profile_id))
    });

    let prev_queue_by_model: BTreeMap<String, Option<u64>> = prev_model_waits
        .into_iter()
        .map(|(m, waits)| (m, median_u64(waits)))
        .collect();
    let mut model_stats = group_agents_by_model(&profile_stats, &prev_queue_by_model);
    for m in &mut model_stats {
        m.median_queue_wait_ms = model_waits.remove(&m.model).and_then(median_u64);
    }

    let providers = profile_stats
        .iter()
        .filter(|p| p.provider != "Unknown")
        .map(|p| &p.provider)
        .collect::<BTreeSet<_>>()
        .len();
    let unknown_providers = profile_stats
        .iter()
        .filter(|p| p.provider == "Unknown")
        .count();
    let used_profiles = profile_stats
        .iter()
        .map(|p| &p.profile_id)
        .collect::<BTreeSet<_>>()
        .len();
    let merged_on = |d: NaiveDate| {
        daily_local.get(&d).copied().unwrap_or(0)
            + daily_pr.get(&d).copied().unwrap_or(0)
            + daily_manual.get(&d).copied().unwrap_or(0)
    };

    let chart_days = if range == RangeWindow::Today {
        if trend_days == 30 { 30 } else { 14 }
    } else {
        range.days()
    };
    let chart_start = today - Duration::days(chart_days - 1);
    let mut moving_average = Vec::new();
    let days: Vec<_> = (0..chart_days)
        .map(|i| {
            let d = chart_start + Duration::days(i);
            let known = history_start.is_some_and(|h| d - Duration::days(6) >= h);
            moving_average.push((d < actual_today && known).then(|| {
                (0..7)
                    .map(|j| merged_on(d - Duration::days(j)))
                    .sum::<usize>() as f64
                    / 7.
            }));
            let o = daily_runs.get(&d).copied().unwrap_or_default();
            let day_cyc = daily_cycles.get(&d).cloned().and_then(median_u64);
            DayBucket {
                date_key: d.to_string(),
                short_label: d.format("%m-%d").to_string(),
                weekday_label: weekday_short(d),
                created_cards: daily_created.get(&d).copied().unwrap_or(0),
                initial_sync_cards: daily_initial_sync.get(&d).copied().unwrap_or(0),
                merged_cards: merged_on(d),
                local_merges: daily_local.get(&d).copied().unwrap_or(0),
                pr_merges: daily_pr.get(&d).copied().unwrap_or(0),
                manual_resolutions: daily_manual.get(&d).copied().unwrap_or(0),
                completed_runs: o[0],
                blocked_runs: o[1],
                failed_runs: o[2],
                active_runs: o[3],
                human_touches: daily_touches.get(&d).copied().unwrap_or(0),
                cycle_p50_ms: day_cyc,
            }
        })
        .collect();

    let mut other_dwell_ms = 0u64;
    let mut other_agent_ms = 0u64;
    if range == RangeWindow::Today {
        for seg in &cur_segments {
            let dur = seg.duration_ms();
            if dur == 0 {
                continue;
            }
            let pid = seg.project_id.as_deref().filter(|s| !s.is_empty());
            match seg.kind {
                ActivityKind::Dwell => {
                    if let Some(p) = pid.and_then(|id| projects.get_mut(id)) {
                        p.dwell_ms = p.dwell_ms.saturating_add(dur);
                    } else if scope.is_none() {
                        other_dwell_ms = other_dwell_ms.saturating_add(dur);
                    }
                }
                ActivityKind::Agent => {
                    if let Some(p) = pid.and_then(|id| projects.get_mut(id)) {
                        p.agent_ms = p.agent_ms.saturating_add(dur);
                    } else if scope.is_none() {
                        other_agent_ms = other_agent_ms.saturating_add(dur);
                    }
                }
            }
        }
    } else {
        for b in &cur_buckets {
            let mut matched_dwell = 0u64;
            let mut matched_agent = 0u64;
            for (pid, pt) in &b.by_project {
                if let Some(p) = projects.get_mut(pid) {
                    p.dwell_ms = p.dwell_ms.saturating_add(pt.dwell_ms);
                    p.agent_ms = p.agent_ms.saturating_add(pt.agent_ms);
                    matched_dwell = matched_dwell.saturating_add(pt.dwell_ms);
                    matched_agent = matched_agent.saturating_add(pt.agent_ms);
                }
            }
            if scope.is_none() {
                other_dwell_ms =
                    other_dwell_ms.saturating_add(b.dwell.total().saturating_sub(matched_dwell));
                other_agent_ms =
                    other_agent_ms.saturating_add(b.agent_ms.saturating_sub(matched_agent));
            }
        }
    }

    for p in projects.values_mut() {
        p.recent_merges = (0..7)
            .map(|i| {
                project_merges
                    .get(&p.id)
                    .and_then(|v| v.get(&(today - Duration::days(6 - i))))
                    .copied()
                    .unwrap_or(0)
            })
            .collect();
    }
    let mut project_stats: Vec<_> = projects
        .into_values()
        .filter(|p| {
            !p.cards.is_empty()
                || p.total_wip() > 0
                || p.initial_sync > 0
                || p.dwell_ms > 0
                || p.agent_ms > 0
        })
        .collect();
    project_stats.sort_by(|a, b| {
        b.total_time_ms()
            .cmp(&a.total_time_ms())
            .then_with(|| b.merged.cmp(&a.merged))
            .then_with(|| b.created.cmp(&a.created))
            .then_with(|| b.total_wip().cmp(&a.total_wip()))
            .then_with(|| a.name.cmp(&b.name))
    });

    let time_totals = time_totals_filtered(
        range,
        &cur_segments,
        &prev_segments,
        &cur_buckets,
        &prev_buckets,
    );
    merges_on_selected_day.sort_by_key(|m| m.timestamp_ms);
    let timeline = if range == RangeWindow::Today {
        let owned_cur: Vec<ActivitySegment> = cur_segments.iter().map(|s| (*s).clone()).collect();
        timeline_lanes(
            snapshot,
            &owned_cur,
            merges_on_selected_day,
            today,
            partial.then_some(now_ms),
            tz,
            agent_group,
        )
    } else {
        TimelineData::default()
    };
    let day_rows = day_rows(&days, &activity.range_buckets, actual_today, tz);
    let funnel = AutomationFunnel {
        created: created_total,
        agent_touched: agent_touched_cards.len(),
        reviewed: reviewed_cards.len(),
        merged: merged_total,
        human_asks: human_asks_period,
        zero_human_resolved,
        prev_zero_human_resolved,
        prev_merged: prev_merged_total,
    };

    let hourly = |times: &[u64], date: NaiveDate, cutoff: u64, hour: u32| {
        times
            .iter()
            .filter(|ts| {
                let dt = chrono::DateTime::from_timestamp_millis(**ts as i64)
                    .unwrap()
                    .with_timezone(tz);
                **ts < cutoff && dt.date_naive() == date && dt.hour() <= hour
            })
            .count()
    };
    let times = match hourly_metric {
        ActivityMetric::Created => &created_times,
        ActivityMetric::Merged => &merge_times,
        ActivityMetric::Runs => &start_times,
    };
    let hours = (0..24)
        .map(|hour| HourBucket {
            hour,
            current: hourly(times, today, until, hour),
            previous: hourly(times, previous_date, prev_until, hour),
            future: partial && hour > now.hour(),
        })
        .collect();

    let mut reference_created = 0;
    let mut reference_merged = 0;
    let mut reference_days = 0;
    for i in 1..=7 {
        let d = today - Duration::days(i);
        if !history_start.is_some_and(|h| d >= h) {
            continue;
        }
        let a = boundary(d, midnight, tz);
        let b = if partial {
            boundary(d, now.time(), tz).saturating_add(1)
        } else {
            boundary(d + Duration::days(1), midnight, tz)
        };
        reference_created += created_times.iter().filter(|t| **t >= a && **t < b).count();
        reference_merged += merge_times.iter().filter(|t| **t >= a && **t < b).count();
        reference_days += 1;
    }

    let annual_start = actual_today - Duration::days(364);
    let heatmap_start =
        annual_start - Duration::days(i64::from(annual_start.weekday().num_days_from_monday()));
    let heatmap_end =
        actual_today + Duration::days(6 - i64::from(actual_today.weekday().num_days_from_monday()));
    let num_hm_weeks = (heatmap_end - heatmap_start).num_days() / 7 + 1;

    let mut annual_dwell_by_day: BTreeMap<NaiveDate, u64> = BTreeMap::new();
    let mut annual_agent_by_day: BTreeMap<NaiveDate, u64> = BTreeMap::new();
    for b in &activity.annual_buckets {
        let mid = b
            .start_ms
            .saturating_add(b.end_ms.saturating_sub(b.start_ms) / 2);
        let d = local_date(mid, tz);
        *annual_dwell_by_day.entry(d).or_default() += b.dwell.total();
        *annual_agent_by_day.entry(d).or_default() += b.agent_ms;
    }
    let activity_on = |d: NaiveDate| -> usize {
        match heatmap_metric {
            HeatmapMetric::Resolved => merged_on(d),
            HeatmapMetric::Dwell => annual_dwell_by_day
                .get(&d)
                .copied()
                .map(|ms| {
                    if ms == 0 {
                        0
                    } else {
                        ms.div_ceil(60_000) as usize
                    }
                })
                .unwrap_or(0),
            HeatmapMetric::Agent => annual_agent_by_day
                .get(&d)
                .copied()
                .map(|ms| {
                    if ms == 0 {
                        0
                    } else {
                        ms.div_ceil(60_000) as usize
                    }
                })
                .unwrap_or(0),
        }
    };
    let min_peak = match heatmap_metric {
        HeatmapMetric::Resolved => 4,
        HeatmapMetric::Dwell | HeatmapMetric::Agent => 30,
    };
    let peak = (0..365)
        .map(|i| activity_on(annual_start + Duration::days(i)))
        .max()
        .unwrap_or(0)
        .max(min_peak);
    let mut active_days = 0;
    let mut streak = 0;
    let mut longest_streak = 0;
    for i in 0..365 {
        let d = annual_start + Duration::days(i);
        if activity_on(d) > 0 {
            active_days += 1;
            streak += 1;
            longest_streak = longest_streak.max(streak);
        } else if d != actual_today {
            streak = 0;
        }
    }
    let mut previous_month = None;
    let heatmap_weeks = (0..num_hm_weeks)
        .map(|w| {
            let monday = heatmap_start + Duration::weeks(w);
            let header = (previous_month != Some(monday.month()))
                .then(|| month_short(monday.month()).to_owned());
            previous_month = Some(monday.month());
            HeatmapWeek {
                month_header: header,
                cells: std::array::from_fn(|dow| {
                    let d = monday + Duration::days(dow as i64);
                    let valid = d >= annual_start && d <= actual_today;
                    let known = valid && history_start.is_some_and(|h| d >= h);
                    let a = activity_on(d);
                    HeatmapCell {
                        date_key: d.to_string(),
                        short_label: d.format("%m-%d").to_string(),
                        weekday_label: weekday_short(d),
                        merged_cards: merged_on(d),
                        created_cards: daily_created.get(&d).copied().unwrap_or(0),
                        total_runs: daily_runs.get(&d).map(|v| v.iter().sum()).unwrap_or(0),
                        dwell_ms: annual_dwell_by_day.get(&d).copied().unwrap_or(0),
                        agent_ms: annual_agent_by_day.get(&d).copied().unwrap_or(0),
                        known,
                        level: if !known || a == 0 {
                            0
                        } else {
                            ((a * 4).div_ceil(peak)).min(4) as u8
                        },
                        is_future: !valid,
                        is_today: d == actual_today,
                        in_range: d >= start && d <= today,
                    }
                }),
            }
        })
        .collect();

    let resolved = outcomes[0] + outcomes[1] + outcomes[2];
    let prev_resolved = prev_outcomes[0] + prev_outcomes[1] + prev_outcomes[2];
    let trigger_stats = [
        TriggerKind::Workflow,
        TriggerKind::Mention,
        TriggerKind::Interactive,
    ]
    .into_iter()
    .map(|kind| {
        let count = trigger_counts.get(&kind).copied().unwrap_or(0);
        TriggerStat {
            kind,
            count,
            share_pct: (count * 100).checked_div(scheduled_total).unwrap_or(0),
        }
    })
    .collect();

    efficiency
        .cycle
        .sort_by_key(|s| std::cmp::Reverse(s.duration));
    efficiency
        .queue
        .sort_by_key(|s| std::cmp::Reverse(s.duration));

    let median_lead_time_ms = percentile(&efficiency.cycle, 50);
    let prev_median_lead_time_ms = percentile(&efficiency.previous_cycle, 50);
    let median_queue_wait_ms = percentile(&efficiency.queue, 50);
    let prev_median_queue_wait_ms = percentile(&efficiency.previous_queue, 50);

    let merge_queue_batches = mq_batches_period.len();
    let merge_queue_conflict_batches = mq_batches_period.values().filter(|(_, c)| *c).count();
    let merge_queue_clean_batches =
        merge_queue_batches.saturating_sub(merge_queue_conflict_batches);
    let merge_queue_avg_batch_size = (merge_queue_batches > 0).then(|| {
        mq_batches_period.values().map(|(sz, _)| *sz).sum::<usize>() as f64
            / merge_queue_batches as f64
    });
    let timings = StageQueueTimings {
        develop_stage: build_timing_stat(develop_stage_cur, develop_stage_prev),
        develop_exec: build_timing_stat(develop_exec_cur, develop_exec_prev),
        review_stage: build_timing_stat(review_stage_cur, review_stage_prev),
        merge_queue: build_timing_stat(merge_queue_cur, merge_queue_prev),
        mention_reply: build_timing_stat(mention_cur, mention_prev),
        merge_queue_batches,
        merge_queue_clean_batches,
        merge_queue_conflict_batches,
        merge_queue_avg_batch_size,
        merge_queue_conflict_avg_ms: mean_u64(&mq_conflict_durations),
    };

    Dashboard {
        exclude_initial_sync,
        baseline_known: history_start.is_some_and(|h| h <= previous_start),
        unknown_batch_starts: snapshot
            .merge_batches
            .iter()
            .filter(|b| in_scope(&b.project_id) && !b.prompt.is_empty() && b.started_at.is_none())
            .count(),
        created_ids,
        merged_ids,
        selected_date: today,
        period_label: if start == today {
            format!(
                "{} ({}) · {}{}",
                today,
                weekday_short(today),
                if partial {
                    now.fixed_offset().format("%H:%M").to_string()
                } else {
                    "24:00".into()
                },
                if partial { " · In progress" } else { "" }
            )
        } else {
            format!(
                "{} – {} {}{}",
                start,
                today,
                if partial {
                    now.fixed_offset().format("%H:%M").to_string()
                } else {
                    "24:00".into()
                },
                if partial { " · In progress" } else { "" }
            )
        },
        comparison_label: if previous_start == previous_date {
            format!(
                "{} ({}) {}",
                previous_date,
                weekday_short(previous_date),
                if partial {
                    format!("· through {}", now.fixed_offset().format("%H:%M"))
                } else {
                    "· full day".into()
                }
            )
        } else {
            format!(
                "{} – {} {}",
                previous_start,
                previous_date,
                if partial {
                    format!("· through {}", now.fixed_offset().format("%H:%M"))
                } else {
                    "· full days".into()
                }
            )
        },
        timezone: now.fixed_offset().format("%Z (UTC%:z)").to_string(),
        previous_created,
        reference_created: (reference_days > 0)
            .then(|| reference_created as f64 / reference_days as f64),
        reference_merged: (reference_days > 0)
            .then(|| reference_merged as f64 / reference_days as f64),
        reference_days,
        project_stats,
        providers,
        used_profiles,
        used_runs,
        unknown_providers,
        unknown_merge_dates,
        hours,
        moving_average,
        time_totals,
        timeline,
        day_rows,
        funnel,
        other_dwell_ms,
        other_agent_ms,
        activity_loaded: activity.loaded,
        pipeline,
        intake,
        schedules,
        human_asks_period,
        prev_human_asks_period,
        workflow_runs_completed,
        annual_start,
        annual_end: actual_today,
        active_days,
        current_streak: streak,
        longest_streak,
        history_start,
        range,
        today_key: today.to_string(),
        window_start_key: start.to_string(),
        merged_total,
        prev_merged_total,
        local_merges,
        pr_merges,
        manual_resolutions,
        created_total,
        initial_sync_period,
        net_wip_delta: created_total as isize - merged_total as isize,
        scheduled_total,
        prev_scheduled_total,
        completed_runs: outcomes[0],
        blocked_runs: outcomes[1],
        failed_runs: outcomes[2],
        active_runs: outcomes[3],
        prev_completed_runs: prev_outcomes[0],
        prev_resolved_runs: prev_resolved,
        success_rate_pct: (resolved > 0).then(|| (outcomes[0] * 100 + resolved / 2) / resolved),
        prev_success_rate_pct: (prev_resolved > 0)
            .then(|| (prev_outcomes[0] * 100 + prev_resolved / 2) / prev_resolved),
        median_lead_time_ms,
        prev_median_lead_time_ms,
        median_queue_wait_ms,
        prev_median_queue_wait_ms,
        efficiency,
        timings,
        max_daily_kanban: days
            .iter()
            .map(|d| d.created_cards.max(d.merged_cards))
            .max()
            .unwrap_or(0),
        max_daily_runs: days.iter().map(DayBucket::total_runs).max().unwrap_or(0),
        days,
        heatmap_weeks,
        profile_stats,
        model_stats,
        trigger_stats,
        has_any_history: !valid_cards.is_empty()
            || !runs.is_empty()
            || !activity.period_segments.is_empty()
            || activity
                .range_buckets
                .iter()
                .any(|b| b.dwell.total() > 0 || b.agent_ms > 0)
            || activity
                .annual_buckets
                .iter()
                .any(|b| b.dwell.total() > 0 || b.agent_ms > 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{
        Delivery, Event, MergeBatch, MergeBatchItem, Project, TerminalEntry, Workflow,
    };
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
        let dash = compute(&snap, &[], None, RangeWindow::Days30, now, &tz_plus_8());
        assert_eq!(dash.merged_total, 0);
        assert_eq!(dash.scheduled_total, 0);
        assert_eq!(dash.created_total, 0);
        assert_eq!(dash.net_wip_delta, 0);
        assert_eq!(dash.success_rate_pct, None);
        assert_eq!(dash.median_lead_time_ms, None);
        assert_eq!(dash.median_queue_wait_ms, None);
        assert_eq!(dash.days.len(), 30);
        assert_eq!(dash.heatmap_weeks.len(), 53);
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

        let mut c_manual = sample_card("c-manual", "p1", 5, ts_utc8(2026, 11, 15, 10, 0));
        c_manual.resolution = Some(canban_types::CardResolution {
            resolved_at: ts_utc8(2026, 11, 15, 13, 0),
            kind: "manual".into(),
            reason: Some("Duplicate".into()),
            actor: "user".into(),
        });
        snap.cards.push(c_manual);

        let dash = compute(&snap, &[], None, RangeWindow::Days7, now, &tz_plus_8());
        assert_eq!(dash.merged_total, 3);
        assert_eq!(dash.local_merges, 1);
        assert_eq!(dash.pr_merges, 1);
        assert_eq!(dash.manual_resolutions, 1);
        assert_eq!(dash.created_total, 4);
        assert_eq!(dash.net_wip_delta, 1);
        assert_eq!(dash.pipeline.active_wip, 0);

        let today_bucket = dash.days.last().unwrap();
        assert_eq!(today_bucket.date_key, "2026-11-15");
        assert_eq!(today_bucket.merged_cards, 3);
        assert_eq!(today_bucket.local_merges, 1);
        assert_eq!(today_bucket.pr_merges, 1);
        assert_eq!(today_bucket.manual_resolutions, 1);
    }

    #[test]
    fn excludes_initial_sync_cards_by_default_and_can_include_when_toggled() {
        let mut snap = Snapshot::default();
        snap.projects.push(sample_project("p1", "Fiber"));
        let now = ts_utc8(2026, 11, 18, 18, 0);

        let c_organic = sample_card("c-org", "p1", 1, ts_utc8(2026, 11, 18, 10, 0));
        let c_sync = sample_card("c-sync", "p1", 2, ts_utc8(2026, 11, 18, 10, 5));
        snap.cards.extend([c_organic, c_sync]);
        let events = vec![Event {
            id: "ev-sync".into(),
            project_id: "p1".into(),
            card_id: "c-sync".into(),
            sequence: 1,
            kind: "card.created".into(),
            actor: "github".into(),
            message: "Card created".into(),
            data: json!({"initial_sync": true, "source_kind": "github.issue", "origin": "github"}),
            created_at: ts_utc8(2026, 11, 18, 10, 5),
        }];

        let default_dash = compute(&snap, &events, None, RangeWindow::Today, now, &tz_plus_8());
        assert_eq!(default_dash.created_total, 1);
        assert_eq!(default_dash.initial_sync_period, 1);
        assert_eq!(default_dash.intake.initial_sync, 1);
        assert_eq!(default_dash.intake.local_manual, 1);

        let mut opts = ViewOptions::with_now(now);
        opts.hourly_metric = ActivityMetric::Created;
        opts.exclude_initial_sync = false;
        let inclusive = compute_dashboard(
            &snap,
            &events,
            &ActivityInput::default(),
            None,
            &tz_plus_8(),
            &[],
            opts,
        );
        assert_eq!(inclusive.created_total, 2);
    }

    #[test]
    fn separates_queue_wait_per_provider_and_computes_weekly_wow_trajectory() {
        let mut snap = Snapshot::default();
        snap.projects.push(sample_project("p1", "Canban"));
        let now = ts_utc8(2026, 11, 18, 18, 0);

        let mut c_prev = sample_card("c-prev", "p1", 1, ts_utc8(2026, 11, 10, 9, 0));
        c_prev.delivery = Some(Delivery {
            target_branch: "main".into(),
            commit: "prev1".into(),
            request_key: "merge:prev".into(),
            merged_at: ts_utc8(2026, 11, 10, 12, 0),
        });
        snap.cards.push(c_prev);
        let mut events = vec![
            Event {
                id: "ep-1".into(),
                project_id: "p1".into(),
                card_id: "c-prev".into(),
                sequence: 1,
                kind: "execution.started".into(),
                actor: "system".into(),
                message: String::new(),
                data: json!({"terminal_id": "c-prev:merger", "run_number": 1}),
                created_at: ts_utc8(2026, 11, 10, 10, 0),
            },
            Event {
                id: "ep-2".into(),
                project_id: "p1".into(),
                card_id: "c-prev".into(),
                sequence: 2,
                kind: "execution.input_required".into(),
                actor: "herdr".into(),
                message: String::new(),
                data: json!({"terminal_id": "c-prev:merger"}),
                created_at: ts_utc8(2026, 11, 10, 10, 30),
            },
            Event {
                id: "ep-3".into(),
                project_id: "p1".into(),
                card_id: "c-prev".into(),
                sequence: 3,
                kind: "execution.reported".into(),
                actor: "agent:merger".into(),
                message: String::new(),
                data: json!({"terminal_id": "c-prev:merger", "run_number": 1, "state": "completed"}),
                created_at: ts_utc8(2026, 11, 10, 11, 50),
            },
        ];

        for (idx, d, prov, q_sec) in [(2u64, 17u32, "codex", 10u64), (3u64, 18u32, "agy", 40u64)] {
            let cid = format!("c-{idx}");
            let mut c = sample_card(&cid, "p1", idx, ts_utc8(2026, 11, d, 9, 0));
            c.delivery = Some(Delivery {
                target_branch: "main".into(),
                commit: format!("sha{idx}"),
                request_key: format!("merge:{cid}"),
                merged_at: ts_utc8(2026, 11, d, 10, 30),
            });
            snap.cards.push(c);
            let tid = format!("{cid}:merger");
            let q_at = ts_utc8(2026, 11, d, 10, 0) - q_sec * 1_000;
            let s_at = ts_utc8(2026, 11, d, 10, 0);
            events.extend([
                Event {
                    id: format!("eq-{idx}"),
                    project_id: "p1".into(),
                    card_id: cid.clone(),
                    sequence: 1,
                    kind: "execution.queued".into(),
                    actor: "system".into(),
                    message: String::new(),
                    data: json!({"terminal_id": tid, "run_number": 1}),
                    created_at: q_at,
                },
                Event {
                    id: format!("es-{idx}"),
                    project_id: "p1".into(),
                    card_id: cid.clone(),
                    sequence: 2,
                    kind: "execution.started".into(),
                    actor: "system".into(),
                    message: String::new(),
                    data: json!({"terminal_id": tid, "run_number": 1}),
                    created_at: s_at,
                },
                Event {
                    id: format!("er-{idx}"),
                    project_id: "p1".into(),
                    card_id: cid.clone(),
                    sequence: 3,
                    kind: "execution.reported".into(),
                    actor: "agent:merger".into(),
                    message: String::new(),
                    data: json!({"terminal_id": tid, "run_number": 1, "state": "completed"}),
                    created_at: s_at + 20 * 60_000,
                },
            ]);
            let entry: TerminalEntry = serde_json::from_value(json!({
                "id": tid,
                "number": idx,
                "name": "merger",
                "created_at": q_at,
                "origin_project_id": "p1",
                "agent": {
                    "project_id": "p1",
                    "card_id": cid,
                    "number": 1,
                    "stage": "review",
                    "state": "completed",
                    "started_at": s_at,
                    "updated_at": s_at + 20 * 60_000,
                    "completed_at": s_at + 20 * 60_000,
                    "connection_state": "closed",
                    "profile": {
                        "id": "merger",
                        "preset": prov,
                        "argv": [prov],
                        "exit": {"type": "manual"},
                        "role_instruction": "",
                        "session_instruction": ""
                    }
                }
            }))
            .unwrap();
            snap.terminals.push(entry);
        }

        let dash = compute(&snap, &events, None, RangeWindow::Days7, now, &tz_plus_8());
        assert_eq!(dash.profile_stats.len(), 2);
        let codex_row = dash
            .profile_stats
            .iter()
            .find(|p| p.provider == "codex")
            .unwrap();
        let agy_row = dash
            .profile_stats
            .iter()
            .find(|p| p.provider == "agy")
            .unwrap();
        assert_eq!(codex_row.median_queue_wait_ms, Some(10_000));
        assert_eq!(agy_row.median_queue_wait_ms, Some(40_000));
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
        let mut events = Vec::new();
        let mut push_ev =
            |pid: &str, cid: &str, kind: &str, actor: &str, ts: u64, data: serde_json::Value| {
                seq += 1;
                events.push(Event {
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
            queued_at: Some(ts_utc8(2026, 11, 15, 11, 59)),
            started_at: Some(ts_utc8(2026, 11, 15, 12, 0)),
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

        let all = compute(&snap, &events, None, RangeWindow::Days7, now, &tz_plus_8());
        assert_eq!(all.scheduled_total, 5);
        assert_eq!(all.completed_runs, 3);
        assert_eq!(all.blocked_runs, 1);
        assert_eq!(all.failed_runs, 1);
        assert_eq!(all.active_runs, 0);
        assert_eq!(all.success_rate_pct, Some(60));

        let p1_only = compute(
            &snap,
            &events,
            Some("p1"),
            RangeWindow::Days7,
            now,
            &tz_plus_8(),
        );
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
        let mut events = Vec::new();

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
            events.push(Event {
                id: format!("ev-{idx}"),
                project_id: "p1".into(),
                card_id: cid,
                sequence: 1,
                kind: "execution.started".into(),
                actor: "system".into(),
                message: "Agent started".into(),
                data: json!({}),
                created_at: work_start,
            });
        }

        let dash = compute(&snap, &events, None, RangeWindow::Days7, now, &tz_plus_8());
        assert_eq!(dash.merged_total, 2);
        assert_eq!(dash.prev_merged_total, 1);
        assert_eq!(
            format_delta_pct(dash.merged_total, dash.prev_merged_total),
            "+100% vs prev"
        );
        assert_eq!(dash.median_lead_time_ms, Some(3 * 3_600_000));
        assert_eq!(format_duration_compact(3 * 3_600_000), "3h");
    }

    fn event(events: &mut Vec<Event>, card: &str, profile: &str, kind: &str, at: u64, number: u32) {
        events.push(Event {
            id: format!("event-{}", events.len()),
            project_id: "p1".into(),
            card_id: card.into(),
            sequence: events.len() as u64 + 1,
            kind: kind.into(),
            actor: format!("agent:{profile}"),
            message: String::new(),
            data: json!({"terminal_id":format!("{card}:{profile}"),"run_number":number}),
            created_at: at,
        });
    }

    fn merged(s: &mut Snapshot, id: &str, at: u64) {
        let mut c = sample_card(id, "p1", s.cards.len() as u64 + 1, at - 60_000);
        c.delivery = Some(Delivery {
            target_branch: "main".into(),
            commit: id.into(),
            request_key: format!("merge:{id}"),
            merged_at: at,
        });
        s.cards.push(c);
    }

    #[test]
    fn today_compares_matching_local_time_and_historical_days_are_complete() {
        let tz = tz_plus_8();
        let now = ts_utc8(2026, 11, 17, 15, 30);
        let mut s = Snapshot::default();
        s.projects.push(sample_project("p1", "Canban"));
        for (id, d, h) in [
            ("today", 17, 10),
            ("yesterday-am", 16, 10),
            ("yesterday-pm", 16, 20),
            ("last-week-am", 10, 9),
            ("last-week-pm", 10, 18),
        ] {
            merged(&mut s, id, ts_utc8(2026, 11, d, h, 0));
        }
        merged(&mut s, "future", ts_utc8(2026, 11, 17, 20, 0));
        let d = compute(&s, &[], None, RangeWindow::Today, now, &tz);
        assert_eq!(
            (d.created_total, d.merged_total, d.prev_merged_total),
            (1, 1, 1)
        );
        assert_eq!(d.hours[15].current, 1);
        assert!(d.hours[16].future);
        assert_eq!(d.reference_days, 7);
        let mut opts_long = ViewOptions::with_now(now);
        opts_long.trend_days = 30;
        let longer_trend = compute_dashboard(
            &s,
            &[],
            &ActivityInput::default(),
            None,
            &tz,
            &[],
            opts_long,
        );
        assert_eq!(longer_trend.days.len(), 30);
        assert_eq!(longer_trend.merged_total, d.merged_total);
        assert_eq!(longer_trend.prev_merged_total, d.prev_merged_total);
        let mut opts_week = ViewOptions::with_now(now);
        opts_week.comparison = Comparison::LastWeek;
        let week = compute_dashboard(
            &s,
            &[],
            &ActivityInput::default(),
            None,
            &tz,
            &[],
            opts_week,
        );
        assert_eq!(week.prev_merged_total, 1);
        let mut opts_hist = ViewOptions::with_now(now);
        opts_hist.selected = Some(chrono::NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
        let historical = compute_dashboard(
            &s,
            &[],
            &ActivityInput::default(),
            None,
            &tz,
            &[],
            opts_hist,
        );
        assert_eq!(historical.merged_total, 2);
        assert!(!historical.hours.iter().any(|h| h.future));
        assert_eq!(historical.hours[23].current, 2);
        assert_eq!(
            d.heatmap_weeks,
            historical
                .heatmap_weeks
                .iter()
                .cloned()
                .map(|mut w| {
                    for c in &mut w.cells {
                        c.in_range = c.date_key == d.today_key;
                    }
                    w
                })
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn midnight_and_timezone_boundaries_do_not_use_rolling_twenty_four_hours() {
        let now = ts_utc8(2026, 11, 17, 0, 0);
        let mut s = Snapshot::default();
        s.projects.push(sample_project("p1", "Canban"));
        merged(&mut s, "last-minute", ts_utc8(2026, 11, 16, 23, 59));
        merged(&mut s, "midnight", now);
        let d = compute(&s, &[], None, RangeWindow::Today, now, &tz_plus_8());
        assert_eq!(d.merged_ids, vec!["midnight"]);
        assert_eq!(d.prev_merged_total, 0);
        let utc = compute(&s, &[], None, RangeWindow::Today, now, &chrono::Utc);
        assert_eq!(utc.today_key, "2026-11-16");
        assert_eq!(utc.merged_total, 2);
    }

    #[test]
    fn annual_grid_has_exactly_365_valid_days_and_separate_metric_units() {
        let now = ts_utc8(2028, 3, 1, 18, 0);
        let mut s = Snapshot::default();
        s.projects.push(sample_project("p1", "Canban"));
        merged(&mut s, "leap", ts_utc8(2028, 2, 29, 12, 0));
        let mut events = Vec::new();
        event(
            &mut events,
            "leap",
            "codex",
            "execution.started",
            ts_utc8(2028, 2, 29, 11, 0),
            1,
        );
        let d = compute(&s, &events, None, RangeWindow::Days90, now, &tz_plus_8());
        let valid: Vec<_> = d
            .heatmap_weeks
            .iter()
            .flat_map(|w| &w.cells)
            .filter(|c| !c.is_future)
            .collect();
        assert_eq!(valid.len(), 365);
        assert_eq!(valid.first().unwrap().date_key, "2027-03-03");
        let leap = valid.iter().find(|c| c.date_key == "2028-02-29").unwrap();
        assert_eq!((leap.merged_cards, leap.total_runs, leap.level), (1, 1, 1));
        assert_eq!(d.current_streak, 1);
        assert_eq!(d.moving_average.last(), Some(&None));
    }

    #[test]
    fn agent_usage_includes_cross_midnight_but_not_unstarted_or_report_only_tasks() {
        let now = ts_utc8(2026, 11, 17, 12, 0);
        let mut s = Snapshot::default();
        s.projects.push(sample_project("p1", "Canban"));
        s.cards
            .push(sample_card("c1", "p1", 1, ts_utc8(2026, 11, 15, 10, 0)));
        let mut events = Vec::new();
        event(
            &mut events,
            "c1",
            "codex",
            "execution.started",
            ts_utc8(2026, 11, 16, 23, 0),
            1,
        );
        event(
            &mut events,
            "c1",
            "reviewer",
            "execution.started",
            ts_utc8(2026, 11, 17, 9, 0),
            2,
        );
        event(
            &mut events,
            "c1",
            "pending",
            "execution.queued",
            ts_utc8(2026, 11, 17, 10, 0),
            3,
        );
        event(
            &mut events,
            "c1",
            "unobserved",
            "execution.reported",
            ts_utc8(2026, 11, 17, 11, 0),
            4,
        );
        events.last_mut().unwrap().data["state"] = json!("completed");
        events.push(events[1].clone());
        let profiles = vec![
            Profile {
                id: "codex".into(),
                preset: Some("codex".into()),
                ..Default::default()
            },
            Profile {
                id: "reviewer".into(),
                preset: Some("claude".into()),
                ..Default::default()
            },
        ];
        let d = compute_dashboard(
            &s,
            &events,
            &ActivityInput::default(),
            None,
            &tz_plus_8(),
            &profiles,
            ViewOptions::with_now(now),
        );
        assert_eq!(
            (d.used_profiles, d.providers, d.used_runs, d.scheduled_total),
            (2, 2, 2, 1)
        );
        assert_eq!(d.project_stats[0].cards.len(), 1);
        assert_eq!(d.profile_stats.len(), 2);
        event(
            &mut events,
            "c1",
            "codex",
            "execution.close_requested",
            ts_utc8(2026, 11, 16, 23, 50),
            1,
        );
        let d = compute(&s, &events, None, RangeWindow::Today, now, &tz_plus_8());
        assert_eq!((d.used_profiles, d.used_runs), (1, 1));
    }

    #[test]
    fn missing_merge_or_execution_time_is_never_replaced_by_creation_or_archive() {
        let now = ts_utc8(2026, 11, 17, 18, 0);
        let mut s = Snapshot::default();
        s.projects.push(sample_project("p1", "Canban"));
        merged(&mut s, "unknown", now - 60_000);
        s.cards[0].delivery.as_mut().unwrap().merged_at = 0;
        s.cards[0].archived_at = Some(now - 60_000);
        merged(&mut s, "known-merge", now - 120_000);
        let mut events = Vec::new();
        event(
            &mut events,
            "known-merge",
            "developer",
            "execution.queued",
            now - 600_000,
            1,
        );
        let d = compute(&s, &events, None, RangeWindow::Today, now, &tz_plus_8());
        assert_eq!(d.unknown_merge_dates, 1);
        assert_eq!(d.merged_total, 1);
        assert!(d.efficiency.cycle.is_empty());
        event(
            &mut events,
            "known-merge",
            "developer",
            "execution.started",
            now - 300_000,
            1,
        );
        let d = compute(&s, &events, None, RangeWindow::Today, now, &tz_plus_8());
        assert_eq!(d.efficiency.cycle[0].duration, 180_000);
        assert_eq!(d.efficiency.queue[0].duration, 300_000);
    }

    #[test]
    fn unknown_baseline_is_distinct_from_zero_and_percentiles_have_samples() {
        let now = ts_utc8(2026, 11, 17, 15, 0);
        let mut s = Snapshot::default();
        let mut project = sample_project("p1", "New workspace");
        project.created_at = now - 3_600_000;
        s.projects.push(project);
        merged(&mut s, "new", now - 60_000);
        let d = compute(&s, &[], None, RangeWindow::Today, now, &tz_plus_8());
        assert!(!d.baseline_known);
        assert_eq!(d.reference_merged, None);
        assert_eq!(format_delta_pct(2, 0), "+2 vs prev");
        assert_eq!(percentile(&[], 90), None);
        let samples = (1..=10)
            .map(|n| DurationSample {
                card_id: n.to_string(),
                number: n,
                title: String::new(),
                project: String::new(),
                profile: None,
                start: 0,
                end: n,
                duration: n,
            })
            .collect::<Vec<_>>();
        assert_eq!(percentile(&samples, 50), Some(5));
        assert_eq!(percentile(&samples, 90), Some(9));
    }

    #[cfg(feature = "local-host")]
    #[test]
    fn local_dst_boundaries_follow_civil_days() {
        if std::env::var_os("CANBAN_INSIGHTS_DST_CHILD").is_none() {
            let child = canban_exec::raw_command(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "insights::model::tests::local_dst_boundaries_follow_civil_days",
                    "--nocapture",
                ])
                .env("TZ", "America/New_York")
                .env("CANBAN_INSIGHTS_DST_CHILD", "1")
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "{}",
                String::from_utf8_lossy(&child.stderr)
            );
            return;
        }
        let spring = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        let fall = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let span = |d| {
            boundary(
                d + Duration::days(1),
                chrono::NaiveTime::MIN,
                &chrono::Local,
            ) - boundary(d, chrono::NaiveTime::MIN, &chrono::Local)
        };
        assert_eq!(span(spring), 23 * 3_600_000);
        assert_eq!(span(fall), 25 * 3_600_000);
    }

    #[test]
    fn stage_queue_and_mention_timings_aggregate_cleanly() {
        let now = ts_utc8(2026, 11, 17, 18, 0);
        let min = 60_000u64;
        let mut s = Snapshot::default();
        s.projects.push(sample_project("p1", "Canban"));

        // Card 1: backlog -> develop (20m) -> review (10m) -> develop rework (10m) -> review (15m) -> merge queue (5m)
        // Total develop dwell = 30m, review dwell = 25m (excludes 5m merge queue), merge queue = 5m.
        let c1_merged = ts_utc8(2026, 11, 17, 15, 0);
        merged(&mut s, "c1", c1_merged);
        let mut seq = 1u64;
        let mut events = Vec::new();
        let mut push_ev = |card_id: &str,
                           kind: &str,
                           actor: &str,
                           msg: &str,
                           data: serde_json::Value,
                           at: u64| {
            let id = format!("ev-{seq}");
            events.push(Event {
                id: id.clone(),
                project_id: "p1".into(),
                card_id: card_id.into(),
                sequence: seq,
                kind: kind.into(),
                actor: actor.into(),
                message: msg.into(),
                data,
                created_at: at,
            });
            seq += 1;
            id
        };

        push_ev(
            "c1",
            "card.stage_changed",
            "user",
            "backlog → develop",
            json!({"from": "backlog", "to": "develop"}),
            c1_merged - 60 * min,
        );
        push_ev(
            "c1",
            "card.stage_changed",
            "system",
            "develop → review",
            json!({"from": "develop", "to": "review"}),
            c1_merged - 40 * min,
        );
        push_ev(
            "c1",
            "card.stage_changed",
            "user",
            "review → develop",
            json!({"from": "review", "to": "develop"}),
            c1_merged - 30 * min,
        );
        push_ev(
            "c1",
            "card.stage_changed",
            "system",
            "develop → review",
            json!({"from": "develop", "to": "review"}),
            c1_merged - 20 * min,
        );
        push_ev(
            "c1",
            "review.merged",
            "system",
            "Merged batch into main",
            json!({
                "batch_id": "b-clean",
                "batch_size": 2,
                "queued_at": c1_merged - 5 * min,
            }),
            c1_merged,
        );

        // Mention question + reply (2m) for @reviewer
        let comment_id = push_ev(
            "c1",
            "comment.created",
            "user",
            "@reviewer check this",
            json!({"body": "@reviewer check this"}),
            c1_merged - 12 * min,
        );
        s.questions.push(canban_types::Question {
            id: "q1".into(),
            project_id: "p1".into(),
            card_id: "c1".into(),
            profile_id: "reviewer".into(),
            comment_id: comment_id.clone(),
            body: "@reviewer check this".into(),
            terminal_id: Some("c1:reviewer".into()),
            state: "replied".into(),
            error: None,
            reply_path: None,
            created_at: c1_merged - 12 * min,
        });
        push_ev(
            "c1",
            "comment.replied",
            "agent:reviewer",
            "Looks good",
            json!({"question_id": "q1", "reply_to": comment_id}),
            c1_merged - 10 * min,
        );

        // Developer execution (8m) + Reviewer execution (2m)
        push_ev(
            "c1",
            "execution.started",
            "system",
            "Started @developer",
            json!({"terminal_id": "c1:developer", "run_number": 1, "trigger": "workflow"}),
            c1_merged - 58 * min,
        );
        push_ev(
            "c1",
            "execution.reported",
            "agent:developer",
            "Done",
            json!({"terminal_id": "c1:developer", "run_number": 1, "state": "completed"}),
            c1_merged - 50 * min,
        );
        push_ev(
            "c1",
            "execution.started",
            "system",
            "Started @reviewer",
            json!({"terminal_id": "c1:reviewer", "run_number": 1, "trigger": "comment:rev"}),
            c1_merged - 12 * min,
        );
        push_ev(
            "c1",
            "execution.reported",
            "agent:reviewer",
            "Done",
            json!({"terminal_id": "c1:reviewer", "run_number": 1, "state": "completed"}),
            c1_merged - 10 * min,
        );

        // Also add a conflict merge batch in period (6m resolve)
        s.merge_batches.push(MergeBatch {
            id: "b-conflict".into(),
            project_id: "p1".into(),
            profile_id: "merger".into(),
            provider_session_id: None,
            target_branch: "main".into(),
            target_head: "abc".into(),
            strategy: Default::default(),
            sync_upstream: true,
            test_command: None,
            worktree: "/tmp/wt".into(),
            state: "completed".into(),
            failure_kind: Some("conflict".into()),
            conflict_card_number: Some(1),
            conflict_files: vec!["src/lib.rs".into()],
            test_output: None,
            items: vec![MergeBatchItem {
                card_id: "c1".into(),
                card_number: 1,
                workflow_run_id: "wf1".into(),
                branch: "b1".into(),
                worktree: "/tmp/wt1".into(),
                base_head: "def".into(),
                integrated_commit: None,
                evicted: false,
            }],
            prompt: "Resolve conflict".into(),
            error: None,
            queued_at: Some(c1_merged - 10 * min),
            started_at: Some(c1_merged - 9 * min),
            created_at: c1_merged - 10 * min,
            updated_at: c1_merged - 4 * min,
        });

        let d = compute(&s, &events, None, RangeWindow::Today, now, &tz_plus_8());
        assert_eq!(d.timings.develop_stage.avg_ms, Some(30 * min));
        assert_eq!(d.timings.develop_stage.samples, 1);
        assert_eq!(d.timings.develop_exec.avg_ms, Some(8 * min));
        assert_eq!(d.timings.review_stage.avg_ms, Some(25 * min));
        assert_eq!(d.timings.merge_queue.avg_ms, Some(5 * min));
        assert_eq!(d.timings.mention_reply.avg_ms, Some(2 * min));
        assert_eq!(d.timings.merge_queue_batches, 2);
        assert_eq!(d.timings.merge_queue_clean_batches, 1);
        assert_eq!(d.timings.merge_queue_conflict_batches, 1);
        assert_eq!(d.timings.clean_merge_rate_pct(), Some(50));
        assert_eq!(d.timings.merge_queue_avg_batch_size, Some(1.5));
        assert_eq!(d.timings.merge_queue_conflict_avg_ms, Some(6 * min));

        let rev = d
            .profile_stats
            .iter()
            .find(|p| p.profile_id == "reviewer")
            .expect("reviewer profile");
        assert_eq!(rev.avg_exec_ms, Some(2 * min));
        assert_eq!(rev.avg_mention_reply_ms, Some(2 * min));
    }

    #[test]
    fn activity_totals_timeline_day_rows_and_model_grouping() {
        use canban_types::{
            ActivityBucket, ActivityDetail, ActivityKind, ActivitySegment, AgentBucketStat,
            AgentWorkData, DwellBucketTotals, DwellData, DwellLane, ProjectActivityTotals,
        };

        let tz = tz_plus_8();
        let now = ts_utc8(2026, 11, 21, 16, 0);
        let mut s = Snapshot::default();
        s.projects.push(sample_project("p1", "Canban"));
        s.projects.push(sample_project("p2", "Herdr"));
        let mut c1 = sample_card("c1", "p1", 1, ts_utc8(2026, 11, 21, 8, 30));
        c1.delivery = Some(Delivery {
            target_branch: "main".into(),
            commit: "abc".into(),
            request_key: "merge:c1".into(),
            merged_at: ts_utc8(2026, 11, 21, 11, 0),
        });
        s.cards.push(c1);
        // Historical resolved card resting in "review" must not inflate today's funnel
        let mut old_resolved = sample_card("c0", "p1", 99, ts_utc8(2026, 11, 1, 8, 0));
        old_resolved.stage = "review".into();
        old_resolved.delivery = Some(Delivery {
            target_branch: "main".into(),
            commit: "old".into(),
            request_key: "merge:c0".into(),
            merged_at: ts_utc8(2026, 11, 1, 10, 0),
        });
        s.cards.push(old_resolved);

        let events = vec![
            Event {
                id: "ev-q".into(),
                project_id: "p1".into(),
                card_id: "c1".into(),
                sequence: 1,
                kind: "execution.queued".into(),
                actor: "system".into(),
                message: String::new(),
                data: json!({"terminal_id": "c1:developer", "run_number": 1, "trigger": "workflow", "provider": "Claude"}),
                created_at: ts_utc8(2026, 11, 21, 9, 5),
            },
            Event {
                id: "ev-s".into(),
                project_id: "p1".into(),
                card_id: "c1".into(),
                sequence: 2,
                kind: "execution.started".into(),
                actor: "system".into(),
                message: String::new(),
                data: json!({"terminal_id": "c1:developer", "run_number": 1, "trigger": "workflow"}),
                created_at: ts_utc8(2026, 11, 21, 9, 10),
            },
        ];

        let profiles = vec![
            Profile {
                id: "developer".into(),
                preset: Some("claude".into()),
                model: Some("claude-sonnet-4".into()),
                ..Default::default()
            },
            Profile {
                id: "reviewer".into(),
                preset: Some("claude".into()),
                model: Some("claude-sonnet-4".into()),
                ..Default::default()
            },
        ];

        let act = ActivityInput {
            loaded: true,
            period_segments: vec![
                // Split checkpoint blocks in same Card -> should merge in timeline
                ActivitySegment {
                    id: Some(1),
                    kind: ActivityKind::Dwell,
                    start_ms: ts_utc8(2026, 11, 21, 9, 0),
                    end_ms: ts_utc8(2026, 11, 21, 9, 5),
                    project_id: Some("p1".into()),
                    card_id: Some("c1".into()),
                    terminal_id: None,
                    open: false,
                    data: ActivityDetail::Dwell(DwellData {
                        lane: DwellLane::Card,
                        page: "card".into(),
                    }),
                },
                ActivitySegment {
                    id: Some(2),
                    kind: ActivityKind::Dwell,
                    start_ms: ts_utc8(2026, 11, 21, 9, 5),
                    end_ms: ts_utc8(2026, 11, 21, 9, 20),
                    project_id: Some("p1".into()),
                    card_id: Some("c1".into()),
                    terminal_id: None,
                    open: false,
                    data: ActivityDetail::Dwell(DwellData {
                        lane: DwellLane::Card,
                        page: "card".into(),
                    }),
                },
                ActivitySegment {
                    id: Some(3),
                    kind: ActivityKind::Dwell,
                    start_ms: ts_utc8(2026, 11, 21, 10, 0),
                    end_ms: ts_utc8(2026, 11, 21, 10, 15),
                    project_id: Some("p2".into()),
                    card_id: None,
                    terminal_id: Some("term-2".into()),
                    open: false,
                    data: ActivityDetail::Dwell(DwellData {
                        lane: DwellLane::Terminal,
                        page: "terminals".into(),
                    }),
                },
                ActivitySegment {
                    id: Some(4),
                    kind: ActivityKind::Agent,
                    start_ms: ts_utc8(2026, 11, 21, 9, 10),
                    end_ms: ts_utc8(2026, 11, 21, 9, 40),
                    project_id: Some("p1".into()),
                    card_id: Some("c1".into()),
                    terminal_id: Some("c1:developer".into()),
                    open: false,
                    data: ActivityDetail::Agent(AgentWorkData {
                        profile_id: "developer".into(),
                        provider: "claude".into(),
                        model: "claude-sonnet-4".into(),
                        engine: None,
                        stage: Some("develop".into()),
                        run_number: Some(1),
                        trigger: None,
                        manual: false,
                    }),
                },
                ActivitySegment {
                    id: Some(5),
                    kind: ActivityKind::Agent,
                    start_ms: ts_utc8(2026, 11, 21, 10, 10),
                    end_ms: ts_utc8(2026, 11, 21, 10, 20),
                    project_id: Some("p1".into()),
                    card_id: Some("c1".into()),
                    terminal_id: Some("c1:reviewer".into()),
                    open: true,
                    data: ActivityDetail::Agent(AgentWorkData {
                        profile_id: "reviewer".into(),
                        provider: "claude".into(),
                        model: "claude-sonnet-4".into(),
                        engine: None,
                        stage: Some("review".into()),
                        run_number: Some(1),
                        trigger: None,
                        manual: false,
                    }),
                },
            ],
            baseline_segments: vec![ActivitySegment {
                id: Some(6),
                kind: ActivityKind::Dwell,
                start_ms: ts_utc8(2026, 11, 20, 9, 0),
                end_ms: ts_utc8(2026, 11, 20, 9, 10),
                project_id: Some("p1".into()),
                card_id: Some("c1".into()),
                terminal_id: None,
                open: false,
                data: ActivityDetail::Dwell(DwellData {
                    lane: DwellLane::Card,
                    page: "card".into(),
                }),
            }],
            range_buckets: vec![
                // Outside 7D window — must be ignored when computing 7D totals
                ActivityBucket {
                    start_ms: ts_utc8(2026, 10, 1, 0, 0),
                    end_ms: ts_utc8(2026, 10, 2, 0, 0),
                    dwell: DwellBucketTotals {
                        card: 99 * 60_000,
                        terminal: 0,
                        other: 0,
                    },
                    agent_ms: 88 * 60_000,
                    by_project: BTreeMap::from([(
                        "p1".into(),
                        ProjectActivityTotals {
                            dwell_ms: 99 * 60_000,
                            agent_ms: 88 * 60_000,
                        },
                    )]),
                    by_agent: vec![AgentBucketStat {
                        profile_id: "developer".into(),
                        provider: "claude".into(),
                        model: "claude-sonnet-4".into(),
                        ms: 88 * 60_000,
                    }],
                },
                ActivityBucket {
                    start_ms: ts_utc8(2026, 11, 21, 0, 0),
                    end_ms: ts_utc8(2026, 11, 21, 16, 0),
                    dwell: DwellBucketTotals {
                        card: 20 * 60_000,
                        terminal: 15 * 60_000,
                        other: 0,
                    },
                    agent_ms: 40 * 60_000,
                    by_project: BTreeMap::from([(
                        "p1".into(),
                        ProjectActivityTotals {
                            dwell_ms: 20 * 60_000,
                            agent_ms: 40 * 60_000,
                        },
                    )]),
                    by_agent: vec![
                        AgentBucketStat {
                            profile_id: "developer".into(),
                            provider: "claude".into(),
                            model: "claude-sonnet-4".into(),
                            ms: 30 * 60_000,
                        },
                        AgentBucketStat {
                            profile_id: "reviewer".into(),
                            provider: "claude".into(),
                            model: "claude-sonnet-4".into(),
                            ms: 10 * 60_000,
                        },
                    ],
                },
            ],
            baseline_buckets: vec![ActivityBucket {
                start_ms: ts_utc8(2026, 10, 1, 0, 0),
                end_ms: ts_utc8(2026, 10, 2, 0, 0),
                dwell: DwellBucketTotals {
                    card: 77 * 60_000,
                    terminal: 0,
                    other: 0,
                },
                agent_ms: 66 * 60_000,
                ..Default::default()
            }],
            annual_buckets: Vec::new(),
        };

        let opts = ViewOptions::with_now(now);
        let d1 = compute_dashboard(&s, &events, &act, None, &tz, &profiles, opts);
        assert_eq!(d1.time_totals.dwell_total_ms, 35 * 60_000);
        assert_eq!(d1.time_totals.dwell_card_ms, 20 * 60_000);
        assert_eq!(d1.time_totals.dwell_terminal_ms, 15 * 60_000);
        assert_eq!(d1.time_totals.prev_dwell_total_ms, 10 * 60_000);
        assert_eq!(d1.time_totals.agent_total_ms, 40 * 60_000);
        assert_eq!(d1.funnel.reviewed, 1);
        assert_eq!(d1.profile_stats.len(), 2);
        let dev = d1
            .profile_stats
            .iter()
            .find(|p| p.profile_id == "developer")
            .unwrap();
        assert_eq!(dev.total_runs, 1);
        assert_eq!(dev.working_ms, 30 * 60_000);

        // Timeline lanes: 3 Me lanes + 2 Agent lanes (@developer, @reviewer)
        assert_eq!(d1.timeline.lanes.len(), 5);
        assert_eq!(d1.timeline.lanes[0].blocks.len(), 1); // merged checkpoint blocks
        assert_eq!(d1.timeline.lanes[0].total_ms, 20 * 60_000);
        assert_eq!(d1.timeline.merges.len(), 1);
        assert_eq!(d1.timeline.merges[0].card_number, 1);

        // Model grouping collapses developer + reviewer into claude-sonnet-4 (40m)
        let models = group_agents_by_model(&d1.profile_stats, &BTreeMap::new());
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "claude-sonnet-4");
        assert_eq!(models[0].working_ms, 40 * 60_000);
        assert_eq!(d1.model_stats.len(), 1);
        assert_eq!(d1.model_stats[0].model, "claude-sonnet-4");
        assert_eq!(d1.model_stats[0].working_ms, 40 * 60_000);
        assert!(d1.has_any_history);

        // Model timeline merges into 1 agent lane
        let mut opts_model = opts;
        opts_model.agent_group = AgentGroup::Model;
        let d_model = compute_dashboard(&s, &events, &act, None, &tz, &profiles, opts_model);
        assert_eq!(d_model.timeline.lanes.len(), 4);
        assert_eq!(d_model.timeline.lanes[3].label, "claude-sonnet-4");

        // 7D day_rows & totals filter out the out-of-window October bucket cleanly
        let mut opts_7d = opts;
        opts_7d.range = RangeWindow::Days7;
        let d7 = compute_dashboard(&s, &events, &act, None, &tz, &profiles, opts_7d);
        assert_eq!(d7.day_rows.len(), 7);
        assert_eq!(d7.time_totals.dwell_total_ms, 35 * 60_000);
        assert_eq!(d7.time_totals.prev_dwell_total_ms, 0);
        assert_eq!(d7.time_totals.agent_total_ms, 40 * 60_000);
        let p1 = d7.project_stats.iter().find(|p| p.id == "p1").unwrap();
        assert_eq!(p1.dwell_ms, 20 * 60_000);
        assert_eq!(p1.agent_ms, 40 * 60_000);
        let today_row = d7.day_rows.last().unwrap();
        assert!(today_row.is_today);
        assert_eq!(today_row.dwell_total_ms(), 35 * 60_000);
        assert_eq!(today_row.agent_ms, 40 * 60_000);
        assert_eq!(today_row.resolved_cards, 1);
        assert_eq!(d7.funnel.created, 1);
        assert_eq!(d7.funnel.reviewed, 1);
        assert_eq!(d7.funnel.merged, 1);
        assert_eq!(d7.funnel.zero_human_pct(), Some(100));
    }
}
