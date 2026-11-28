//! Efficiency-focused Insights cockpit: Agent Leverage & Active Time KPIs,
//! Projects efficiency matrix, Machines workload & parallelism cards,
//! Card flow (pipeline + delivery + daily trend / 1D timeline), and Agents table.

pub mod model;

use crate::{
    drafts::{capture_scroll, scroll_position},
    presenter::*,
};
use canban_types::{ActivityBucket, ActivitySegment, AgentBucketStat, DwellBucketTotals};
use canban_ui_kit::{tokens::*, *};
use model::{
    ActivityFetchPlan, ActivityInput, ActivityMetric, AgentGroup, Comparison, Dashboard, DayBucket,
    HeatmapMetric, HostStat, ModelAgentStat, ProfileStat, ProjectSort, RangeWindow,
    TimelineLaneKind, ViewOptions, format_duration_compact,
};
use nsh_core::{
    element::{AdaptiveColor, rgb},
    prelude::*,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

const ALL_PROJECTS_OPTION: &str = "__all__";
const ALL_HOSTS_OPTION: &str = "__all_hosts__";
const CHART_HEIGHT: f32 = 148.0;

#[derive(Clone, PartialEq, Eq)]
pub struct ActivityCacheKey {
    pub scope: String,
    pub scope_host: String,
    pub connected_hosts: Vec<String>,
    pub range: RangeWindow,
    pub selected_date: Option<chrono::NaiveDate>,
    pub comparison: Comparison,
    pub want_annual: bool,
    pub visit_seq: u64,
}

#[derive(Clone, PartialEq)]
pub struct LoadedInsights {
    pub scope: String,
    pub scope_host: String,
    pub range: RangeWindow,
    pub selected_date: Option<chrono::NaiveDate>,
    pub comparison: Comparison,
    pub events: Rc<Vec<canban_types::Event>>,
    pub activity: Rc<ActivityInput>,
    pub annual_loaded: bool,
    pub now_ms: u64,
}

type EventsCacheKey = (String, String, Vec<String>, u64);
type AnnualCacheKey = (String, String, String, Vec<String>, u64);
type FetchedInsightsBatch = (
    (bool, u64),
    Option<Vec<canban_types::Event>>,
    ActivityInput,
    Vec<(String, String)>,
);

#[derive(Clone, Copy)]
pub struct State {
    /// Empty string means `All Projects`; otherwise holds a `Project.id`.
    pub scope_project: Signal<String>,
    /// Empty string means `All Hosts`; otherwise holds `"local"` or a remote host ID.
    pub scope_host: Signal<String>,
    /// Skipped offline or unavailable hosts recorded during the latest Insights load.
    pub unavailable_hosts: Signal<Vec<(String, String)>>,
    pub range: Signal<RangeWindow>,
    pub selected_date: Signal<Option<chrono::NaiveDate>>,
    pub date_text: Signal<String>,
    pub comparison: Signal<Comparison>,
    pub heatmap_metric: Signal<HeatmapMetric>,
    pub hourly_metric: Signal<ActivityMetric>,
    pub agent_group: Signal<AgentGroup>,
    pub trend_days: Signal<i64>,
    pub exclude_initial_sync: Signal<bool>,
    pub timeline_full_24h: Signal<bool>,
    pub scroll: Signal<Option<ElementRef>>,
    pub project_sort: Signal<ProjectSort>,
    pub drilldown: Signal<String>,
    pub timing_tab: Signal<String>,
    pub timing_page: Signal<usize>,
    /// Optional fixed timestamp (ms) for deterministic Gallery/test rendering; `None` uses live clock.
    pub anchor_ms: Signal<Option<u64>>,
    pub events: Signal<Rc<Vec<canban_types::Event>>>,
    pub events_key: Signal<Option<EventsCacheKey>>,
    pub activity: Signal<Rc<ActivityInput>>,
    pub activity_key: Signal<Option<ActivityCacheKey>>,
    pub annual_activity_key: Signal<Option<AnnualCacheKey>>,
    pub manual_refresh_seq: Signal<u64>,
    pub request_seq: Signal<u64>,
    pub loading: Signal<bool>,
    pub heatmap_loading: Signal<bool>,
    pub loaded_data: Signal<Option<LoadedInsights>>,
    pub preview_loading: Signal<bool>,
    inflight_key: Signal<Option<ActivityCacheKey>>,
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub fn new() -> Self {
        Self {
            scope_project: signal(String::new()),
            scope_host: signal(String::new()),
            unavailable_hosts: signal(Vec::new()),
            range: signal(RangeWindow::Today),
            selected_date: signal(None),
            date_text: signal(String::new()),
            comparison: signal(Comparison::Previous),
            heatmap_metric: signal(HeatmapMetric::Resolved),
            hourly_metric: signal(ActivityMetric::Merged),
            agent_group: signal(AgentGroup::Profile),
            trend_days: signal(14),
            exclude_initial_sync: signal(true),
            timeline_full_24h: signal(false),
            scroll: signal(None),
            project_sort: signal(ProjectSort::Time),
            drilldown: signal(String::new()),
            timing_tab: signal(String::new()),
            timing_page: signal(0),
            anchor_ms: signal(None),
            events: signal(Rc::new(Vec::new())),
            events_key: signal(None),
            activity: signal(Rc::new(ActivityInput::default())),
            activity_key: signal(None),
            annual_activity_key: signal(None),
            manual_refresh_seq: signal(0),
            request_seq: signal(0),
            loading: signal(false),
            heatmap_loading: signal(false),
            loaded_data: signal(None),
            preview_loading: signal(false),
            inflight_key: signal(None),
        }
    }
}

fn has_backend() -> bool {
    crate::backend::is_connected()
}

pub(crate) fn connected_hosts_key(hosts: &[canban_types::HostSummary]) -> Vec<String> {
    let mut ids: Vec<String> = hosts
        .iter()
        .filter(|h| h.is_local || h.connection.is_connected())
        .map(|h| h.id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn is_host_connected(host_id: &str, hosts: &[canban_types::HostSummary]) -> bool {
    host_id.is_empty()
        || host_id == "local"
        || hosts
            .iter()
            .any(|h| h.id == host_id && h.connection.is_connected())
}

fn collect_skipped_offline_hosts(
    snap: &canban_types::Snapshot,
    hosts: &[canban_types::HostSummary],
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for h in hosts {
        if !h.is_local && !h.connection.is_connected() && seen.insert(h.name.clone()) {
            out.push((h.name.clone(), h.connection.short_status().to_owned()));
        }
    }
    for (id, h) in &snap.host_summaries {
        if id != "local"
            && h.is_offline()
            && !hosts.iter().any(|live| &live.id == id)
            && seen.insert(h.name.clone())
        {
            out.push((h.name.clone(), h.connection.short_status().to_owned()));
        }
    }
    out
}

fn host_unavailable_entry(
    host_id: &str,
    snap: &canban_types::Snapshot,
    hosts: &[canban_types::HostSummary],
) -> (String, String) {
    if let Some(h) = hosts
        .iter()
        .find(|h| h.id == host_id)
        .or_else(|| snap.host_summaries.get(host_id))
    {
        (h.name.clone(), h.connection.short_status().to_owned())
    } else {
        (
            snap.host_display_name(host_id).to_owned(),
            "offline".to_owned(),
        )
    }
}

fn build_project_remaps(snap: &canban_types::Snapshot) -> HashMap<(String, String), String> {
    let mut out = HashMap::new();
    for p in &snap.projects {
        if let Some((hid, raw)) = canban_types::split_qualified_host_id(&p.id) {
            out.insert((hid.to_owned(), raw.to_owned()), p.id.clone());
        }
    }
    out
}

fn remap_project_id(
    remaps: &HashMap<(String, String), String>,
    host_id: &str,
    raw_pid: &str,
) -> String {
    if raw_pid.is_empty() || canban_types::split_qualified_host_id(raw_pid).is_some() {
        return raw_pid.to_owned();
    }
    remaps
        .get(&(host_id.to_owned(), raw_pid.to_owned()))
        .cloned()
        .unwrap_or_else(|| raw_pid.to_owned())
}

fn fanout_insights_rpc(
    method: &str,
    params: serde_json::Value,
    scope_project: &str,
    scope_host: &str,
    owner_host: &str,
) -> Vec<(String, anyhow::Result<serde_json::Value>)> {
    if !scope_project.is_empty() {
        vec![(
            owner_host.to_owned(),
            crate::routed_call(method, params, Some(scope_project)),
        )]
    } else if !scope_host.is_empty() {
        let mut p = params;
        p["host_id"] = serde_json::json!(scope_host);
        vec![(scope_host.to_owned(), crate::routed_call(method, p, None))]
    } else {
        crate::call_all_hosts(method, params)
    }
}

fn fetch_events_for_scope(
    scope: &str,
    scope_host: &str,
    owner_host: &str,
    remaps: &HashMap<(String, String), String>,
    snap: &canban_types::Snapshot,
    hosts: &[canban_types::HostSummary],
    failed_hosts: &mut Vec<(String, String)>,
) -> Vec<canban_types::Event> {
    let mut params = serde_json::json!({});
    if !scope.is_empty() {
        params["project_id"] = serde_json::json!(scope);
    }
    let mut all = Vec::new();
    for (hid, res) in fanout_insights_rpc("insights.events", params, scope, scope_host, owner_host)
    {
        match res {
            Ok(val) => {
                if let Some(arr) = val["events"].as_array() {
                    for v in arr {
                        if let Ok(mut ev) = serde_json::from_value::<canban_types::Event>(v.clone())
                        {
                            ev.project_id = remap_project_id(remaps, &hid, &ev.project_id);
                            all.push(ev);
                        }
                    }
                }
            }
            Err(_) => {
                failed_hosts.push((
                    snap.host_display_name(&hid).to_owned(),
                    host_unavailable_entry(&hid, snap, hosts).1,
                ));
            }
        }
    }
    all
}

struct InsightsFetchCtx<'a> {
    scope: &'a str,
    scope_host: &'a str,
    owner_host: &'a str,
    remaps: &'a HashMap<(String, String), String>,
    snap: &'a canban_types::Snapshot,
    hosts: &'a [canban_types::HostSummary],
}

fn fetch_merged_segments(
    from_ms: u64,
    to_ms: u64,
    ctx: &InsightsFetchCtx<'_>,
    failed_hosts: &mut Vec<(String, String)>,
    host_totals: Option<&mut BTreeMap<String, (u64, u64)>>,
) -> Vec<ActivitySegment> {
    if to_ms <= from_ms {
        return Vec::new();
    }
    let mut params = serde_json::json!({ "from_ms": from_ms, "to_ms": to_ms });
    if !ctx.scope.is_empty() {
        params["project_id"] = serde_json::json!(ctx.scope);
    }
    let mut host_totals = host_totals;
    let mut segments = Vec::new();
    for (hid, res) in fanout_insights_rpc(
        "insights.activity",
        params,
        ctx.scope,
        ctx.scope_host,
        ctx.owner_host,
    ) {
        match res {
            Ok(val) => {
                if let Ok(mut parsed) = serde_json::from_value::<Vec<ActivitySegment>>(
                    val.get("segments")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                ) {
                    let mut dwell_sum = 0u64;
                    let mut agent_sum = 0u64;
                    for seg in &mut parsed {
                        if let Some(ref mut pid) = seg.project_id {
                            *pid = remap_project_id(ctx.remaps, &hid, pid);
                        }
                        let dur = seg.duration_ms();
                        match seg.kind {
                            canban_types::ActivityKind::Dwell => {
                                dwell_sum = dwell_sum.saturating_add(dur);
                            }
                            canban_types::ActivityKind::Agent => {
                                agent_sum = agent_sum.saturating_add(dur);
                            }
                        }
                    }
                    if let Some(totals) = host_totals.as_deref_mut() {
                        let e = totals.entry(hid.clone()).or_default();
                        e.0 = e.0.saturating_add(dwell_sum);
                        e.1 = e.1.saturating_add(agent_sum);
                    }
                    segments.extend(parsed);
                }
            }
            Err(_) => {
                failed_hosts.push((
                    ctx.snap.host_display_name(&hid).to_owned(),
                    host_unavailable_entry(&hid, ctx.snap, ctx.hosts).1,
                ));
            }
        }
    }
    segments.sort_by_key(|s| (s.start_ms, s.end_ms));
    segments
}

fn merge_bucket_lists(lists: Vec<Vec<ActivityBucket>>, boundaries: &[u64]) -> Vec<ActivityBucket> {
    let n = boundaries.len().saturating_sub(1);
    let mut merged: Vec<ActivityBucket> = boundaries
        .windows(2)
        .map(|w| ActivityBucket {
            start_ms: w[0],
            end_ms: w[1],
            dwell: DwellBucketTotals::default(),
            agent_ms: 0,
            by_project: BTreeMap::new(),
            by_agent: Vec::new(),
        })
        .collect();
    let mut agent_maps: Vec<HashMap<(String, String, String), u64>> = vec![HashMap::new(); n];

    for list in lists {
        for (idx, b) in list.into_iter().take(n).enumerate() {
            let dst = &mut merged[idx];
            dst.dwell.card = dst.dwell.card.saturating_add(b.dwell.card);
            dst.dwell.terminal = dst.dwell.terminal.saturating_add(b.dwell.terminal);
            dst.dwell.other = dst.dwell.other.saturating_add(b.dwell.other);
            dst.agent_ms = dst.agent_ms.saturating_add(b.agent_ms);
            for (pid, pt) in b.by_project {
                let entry = dst.by_project.entry(pid).or_default();
                entry.dwell_ms = entry.dwell_ms.saturating_add(pt.dwell_ms);
                entry.agent_ms = entry.agent_ms.saturating_add(pt.agent_ms);
            }
            for a in b.by_agent {
                *agent_maps[idx]
                    .entry((a.profile_id, a.provider, a.model))
                    .or_default() += a.ms;
            }
        }
    }

    for (dst, map) in merged.iter_mut().zip(agent_maps) {
        let mut stats: Vec<AgentBucketStat> = map
            .into_iter()
            .map(|((profile_id, provider, model), ms)| AgentBucketStat {
                profile_id,
                provider,
                model,
                ms,
            })
            .collect();
        stats.sort_by(|a, b| {
            b.ms.cmp(&a.ms)
                .then_with(|| a.profile_id.cmp(&b.profile_id))
                .then_with(|| a.provider.cmp(&b.provider))
                .then_with(|| a.model.cmp(&b.model))
        });
        dst.by_agent = stats;
    }

    merged
}

fn fetch_merged_buckets(
    boundaries: &[u64],
    ctx: &InsightsFetchCtx<'_>,
    failed_hosts: &mut Vec<(String, String)>,
    host_totals: Option<&mut BTreeMap<String, (u64, u64)>>,
) -> Vec<ActivityBucket> {
    if boundaries.len() < 2 {
        return Vec::new();
    }
    let mut params = serde_json::json!({ "boundaries": boundaries });
    if !ctx.scope.is_empty() {
        params["project_id"] = serde_json::json!(ctx.scope);
    }
    let mut host_totals = host_totals;
    let mut per_host = Vec::new();
    for (hid, res) in fanout_insights_rpc(
        "insights.activity_buckets",
        params,
        ctx.scope,
        ctx.scope_host,
        ctx.owner_host,
    ) {
        match res {
            Ok(val) => {
                if let Ok(mut parsed) = serde_json::from_value::<Vec<ActivityBucket>>(
                    val.get("buckets")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                ) {
                    let mut dwell_sum = 0u64;
                    let mut agent_sum = 0u64;
                    for b in &mut parsed {
                        dwell_sum = dwell_sum.saturating_add(b.dwell.total());
                        agent_sum = agent_sum.saturating_add(b.agent_ms);
                        let raw_map = std::mem::take(&mut b.by_project);
                        for (raw_pid, pt) in raw_map {
                            let mapped = remap_project_id(ctx.remaps, &hid, &raw_pid);
                            let entry = b.by_project.entry(mapped).or_default();
                            entry.dwell_ms = entry.dwell_ms.saturating_add(pt.dwell_ms);
                            entry.agent_ms = entry.agent_ms.saturating_add(pt.agent_ms);
                        }
                    }
                    if let Some(totals) = host_totals.as_deref_mut() {
                        let e = totals.entry(hid.clone()).or_default();
                        e.0 = e.0.saturating_add(dwell_sum);
                        e.1 = e.1.saturating_add(agent_sum);
                    }
                    per_host.push(parsed);
                }
            }
            Err(_) => {
                failed_hosts.push((
                    ctx.snap.host_display_name(&hid).to_owned(),
                    host_unavailable_entry(&hid, ctx.snap, ctx.hosts).1,
                ));
            }
        }
    }
    merge_bucket_lists(per_host, boundaries)
}

/// Coordinated non-blocking loader flushing pending dwell and fetching events,
/// period/baseline activity, and optional annual buckets in a single background
/// pass with sequence fencing.
pub fn refresh_data(ui: Ui, visit_seq: u64) {
    if !has_backend() || ui.page.get_untracked() != "insights" {
        return;
    }
    let scope = ui.insights.scope_project.get_untracked();
    let scope_host = ui.insights.scope_host.get_untracked();
    let hosts = ui.hosts.get_untracked();
    let snap = ui.snapshot.get_untracked();
    let connected_hosts = connected_hosts_key(&hosts);
    let range = ui.insights.range.get_untracked();
    let selected_date = ui.insights.selected_date.get_untracked();
    let comparison = ui.insights.comparison.get_untracked();
    let want_annual = false;
    let now = ui
        .insights
        .anchor_ms
        .get_untracked()
        .unwrap_or_else(canban_types::now);
    let actual_today_key = chrono::DateTime::from_timestamp_millis(now as i64)
        .unwrap()
        .with_timezone(&chrono::Local)
        .date_naive()
        .to_string();

    let events_cache_key = (
        scope.clone(),
        scope_host.clone(),
        connected_hosts.clone(),
        visit_seq,
    );
    let need_events = ui.insights.events_key.get_untracked().as_ref() != Some(&events_cache_key);
    let key = ActivityCacheKey {
        scope: scope.clone(),
        scope_host: scope_host.clone(),
        connected_hosts: connected_hosts.clone(),
        range,
        selected_date,
        comparison,
        want_annual,
        visit_seq,
    };

    if !need_events && ui.insights.activity_key.get_untracked().as_ref() == Some(&key) {
        return;
    }
    if ui.insights.loading.get_untracked()
        && ui.insights.inflight_key.get_untracked().as_ref() == Some(&key)
    {
        return;
    }

    let owner_host = if !scope.is_empty() {
        snap.project_host(&scope).to_owned()
    } else if !scope_host.is_empty() {
        scope_host.clone()
    } else {
        String::new()
    };

    // Strict offline-host skip: if scoped specifically to an offline host (or project on one),
    // skip RPCs completely without blocking, erroring, or retrying.
    if !owner_host.is_empty() && !is_host_connected(&owner_host, &hosts) {
        let entry = host_unavailable_entry(&owner_host, &snap, &hosts);
        let empty_events = Rc::new(Vec::new());
        let empty_activity = Rc::new(ActivityInput {
            loaded: true,
            ..Default::default()
        });
        batch(move || {
            ui.insights.unavailable_hosts.set(vec![entry]);
            ui.insights.events.set(empty_events.clone());
            ui.insights.events_key.set(Some(events_cache_key));
            ui.insights.activity.set(empty_activity.clone());
            ui.insights.activity_key.set(Some(key));
            ui.insights.loaded_data.set(Some(LoadedInsights {
                scope,
                scope_host,
                range,
                selected_date,
                comparison,
                events: empty_events,
                activity: empty_activity,
                annual_loaded: want_annual,
                now_ms: now,
            }));
            ui.insights.inflight_key.set(None);
            ui.insights.loading.set(false);
            ui.insights.heatmap_loading.set(false);
        });
        return;
    }

    let skipped_offline = if scope.is_empty() && scope_host.is_empty() {
        collect_skipped_offline_hosts(&snap, &hosts)
    } else {
        Vec::new()
    };

    let annual_cache_key = (
        scope.clone(),
        scope_host.clone(),
        actual_today_key,
        connected_hosts,
        visit_seq,
    );
    let annual_cached =
        ui.insights.annual_activity_key.get_untracked().as_ref() == Some(&annual_cache_key);
    let fetch_annual = want_annual && !annual_cached;
    let existing_events = if need_events {
        None
    } else {
        Some(ui.insights.events.get_untracked())
    };
    let existing_annual = if annual_cached {
        ui.insights
            .activity
            .with_untracked(|a| a.annual_buckets.clone())
    } else {
        Vec::new()
    };

    let pending_dwell = crate::activity::take_pending_for_insights(ui);
    let plan: ActivityFetchPlan =
        model::activity_fetch_plan(range, now, &chrono::Local, selected_date, comparison);
    let seq = ui.insights.request_seq.get_untracked().wrapping_add(1);
    let inflight = key.clone();
    batch(move || {
        ui.insights.request_seq.set(seq);
        ui.insights.inflight_key.set(Some(inflight));
        ui.insights.loading.set(true);
        ui.insights.heatmap_loading.set(fetch_annual);
    });

    ui.spawn(async move {
        let scope_req = scope.clone();
        let scope_host_req = scope_host.clone();
        let owner_host_req = owner_host;
        let remaps = build_project_remaps(&snap);
        let pending_for_req = pending_dwell.clone();
        let mut unavailable = skipped_offline;
        let fetched = blocking(move || -> FetchedInsightsBatch {
            let flush_res = crate::activity::flush_batch_blocking(&pending_for_req);
            let mut failed = Vec::new();
            let ctx = InsightsFetchCtx {
                scope: &scope_req,
                scope_host: &scope_host_req,
                owner_host: &owner_host_req,
                remaps: &remaps,
                snap: &snap,
                hosts: &hosts,
            };
            let fetched_events = need_events.then(|| {
                fetch_events_for_scope(
                    ctx.scope,
                    ctx.scope_host,
                    ctx.owner_host,
                    ctx.remaps,
                    ctx.snap,
                    ctx.hosts,
                    &mut failed,
                )
            });
            let mut host_totals: BTreeMap<String, (u64, u64)> = BTreeMap::new();
            let mut input = ActivityInput {
                loaded: true,
                annual_buckets: existing_annual,
                ..Default::default()
            };
            if range == RangeWindow::Today {
                input.period_segments = fetch_merged_segments(
                    plan.period_from,
                    plan.period_until,
                    &ctx,
                    &mut failed,
                    Some(&mut host_totals),
                );
                input.baseline_segments =
                    fetch_merged_segments(plan.prev_from, plan.prev_until, &ctx, &mut failed, None);
            } else {
                input.range_buckets = fetch_merged_buckets(
                    &plan.range_boundaries,
                    &ctx,
                    &mut failed,
                    Some(&mut host_totals),
                );
                input.baseline_buckets =
                    fetch_merged_buckets(&plan.baseline_boundaries, &ctx, &mut failed, None);
            }
            if fetch_annual {
                input.annual_buckets =
                    fetch_merged_buckets(&plan.annual_boundaries, &ctx, &mut failed, None);
            }
            input.host_totals = host_totals;
            (flush_res, fetched_events, input, failed)
        })
        .await;

        let (flush_ok, flush_recorded) = match &fetched {
            Ok((res, _, _, _)) => *res,
            Err(_) => (false, 0),
        };
        crate::activity::complete_insights_flush(ui, pending_dwell, flush_ok, flush_recorded);

        if ui.insights.request_seq.get_untracked() != seq {
            return;
        }
        match fetched {
            Ok((_, maybe_events, input, failed)) => {
                let mut seen_names: BTreeSet<String> =
                    unavailable.iter().map(|(n, _)| n.clone()).collect();
                for (name, reason) in failed {
                    if seen_names.insert(name.clone()) {
                        unavailable.push((name, reason));
                    }
                }
                let events_rc = maybe_events
                    .map(Rc::new)
                    .or(existing_events)
                    .unwrap_or_else(|| Rc::new(Vec::new()));
                let activity_rc = Rc::new(input);
                let annual_loaded = want_annual || annual_cached;
                batch(move || {
                    ui.insights.unavailable_hosts.set(unavailable);
                    ui.insights.events.set(events_rc.clone());
                    ui.insights.events_key.set(Some(events_cache_key));
                    ui.insights.activity.set(activity_rc.clone());
                    ui.insights.activity_key.set(Some(key));
                    if fetch_annual {
                        ui.insights.annual_activity_key.set(Some(annual_cache_key));
                    }
                    ui.insights.loaded_data.set(Some(LoadedInsights {
                        scope,
                        scope_host,
                        range,
                        selected_date,
                        comparison,
                        events: events_rc,
                        activity: activity_rc,
                        annual_loaded,
                        now_ms: now,
                    }));
                    ui.insights.inflight_key.set(None);
                    ui.insights.loading.set(false);
                    ui.insights.heatmap_loading.set(false);
                });
            }
            Err(_) => {
                batch(move || {
                    ui.insights.inflight_key.set(None);
                    ui.insights.loading.set(false);
                    ui.insights.heatmap_loading.set(false);
                });
            }
        }
    });
}

pub fn set_range(ui: Ui, next: RangeWindow) {
    ui.insights.range.set(next);
    if next != RangeWindow::Today {
        ui.insights.comparison.set(Comparison::Previous);
    }
    ui.insights.drilldown.set(String::new());
}

fn nice_ceil_tick(max_val: usize) -> usize {
    if max_val <= 2 {
        2
    } else if max_val <= 4 {
        4
    } else if max_val <= 6 {
        6
    } else if max_val <= 8 {
        8
    } else if max_val <= 10 {
        10
    } else {
        let step = if max_val <= 25 {
            5
        } else if max_val <= 60 {
            10
        } else {
            20
        };
        max_val.div_ceil(step) * step
    }
}

fn alpha_tint(token: ThemeColor, alpha: f32) -> AdaptiveColor {
    let resolved = token.resolve();
    AdaptiveColor {
        light: resolved.light.alpha(alpha),
        dark: resolved.dark.alpha(alpha),
    }
}

fn fixed_color(hex: u32) -> AdaptiveColor {
    let c = rgb(hex).to_hsla();
    AdaptiveColor { light: c, dark: c }
}

fn project_color(project_id: Option<&str>, project_order: &[String]) -> AdaptiveColor {
    let Some(pid) = project_id.filter(|s| !s.is_empty()) else {
        return alpha_tint(MUTED, 0.72);
    };
    let idx = project_order
        .iter()
        .position(|p| p == pid)
        .unwrap_or(project_order.len());
    match idx % 6 {
        0 => ACCENT.resolve(),
        1 => SUCCESS.resolve(),
        2 => ATTENTION.resolve(),
        3 => HIGHLIGHT.resolve(),
        4 => fixed_color(0x22b8cf),
        _ => fixed_color(0xe64980),
    }
}

fn now_ms(ui: Ui) -> u64 {
    if let Some(fixed) = ui.insights.anchor_ms.get() {
        fixed
    } else {
        let _ = ui.utility_clock.get();
        canban_types::now()
    }
}

fn current_today(ui: Ui) -> chrono::NaiveDate {
    chrono::DateTime::from_timestamp_millis(now_ms(ui) as i64)
        .unwrap()
        .with_timezone(&chrono::Local)
        .date_naive()
}

pub fn select_date(ui: Ui, date: Option<chrono::NaiveDate>) {
    let actual = current_today(ui);
    if date.is_some_and(|d| d > actual) {
        return;
    }
    ui.insights.selected_date.set(date);
    ui.scrolls.with(|s| {
        s.insert("insights:dashboard".into(), Point::default());
    });
    if let Some(scroll) = ui.insights.scroll.get_untracked() {
        scroll.scroll_to(Point::default(), false);
    }
    ui.insights
        .date_text
        .set(date.map(|d| d.to_string()).unwrap_or_default());
    ui.insights.drilldown.set(String::new());
}

/// Shared intents for pointer controls and searchable Command Palette actions.
pub fn intent(ui: Ui, operation: &str) {
    use chrono::Duration;
    match operation {
        "today" => {
            select_date(ui, None);
            set_range(ui, RangeWindow::Today);
        }
        "previous-day" | "next-day" => {
            let actual = current_today(ui);
            let day = ui.insights.selected_date.get().unwrap_or(actual);
            select_date(
                ui,
                Some(day + Duration::days(if operation == "previous-day" { -1 } else { 1 })),
            );
        }
        "apply-date" => {
            if let Ok(date) =
                chrono::NaiveDate::parse_from_str(&ui.insights.date_text.get(), "%Y-%m-%d")
            {
                select_date(ui, Some(date));
            } else {
                ui.notify("Use a date in YYYY-MM-DD format");
            }
        }
        "compare-previous" => ui.insights.comparison.set(Comparison::Previous),
        "compare-week" => ui.insights.comparison.set(Comparison::LastWeek),
        "metric-merged" => ui.insights.heatmap_metric.set(HeatmapMetric::Resolved),
        "metric-dwell" => ui.insights.heatmap_metric.set(HeatmapMetric::Dwell),
        "metric-agent" => ui.insights.heatmap_metric.set(HeatmapMetric::Agent),
        "hourly-merged" => ui.insights.hourly_metric.set(ActivityMetric::Merged),
        "hourly-created" => ui.insights.hourly_metric.set(ActivityMetric::Created),
        "hourly-runs" => ui.insights.hourly_metric.set(ActivityMetric::Runs),
        "group-profile" => ui.insights.agent_group.set(AgentGroup::Profile),
        "group-model" => ui.insights.agent_group.set(AgentGroup::Model),
        "sort-time" => ui.insights.project_sort.set(ProjectSort::Time),
        "sort-leverage" => ui.insights.project_sort.set(ProjectSort::Leverage),
        "sort-created" => ui.insights.project_sort.set(ProjectSort::Created),
        "sort-merged" => ui.insights.project_sort.set(ProjectSort::Merged),
        "sort-wip" => ui.insights.project_sort.set(ProjectSort::Wip),
        "sort-runs" => ui.insights.project_sort.set(ProjectSort::Runs),
        "toggle-initial-sync" => ui.insights.exclude_initial_sync.update(|v| *v = !*v),
        "toggle-timeline-24h" => ui.insights.timeline_full_24h.update(|v| *v = !*v),
        "cards-created" => ui.insights.drilldown.update(|cur| {
            *cur = if cur == "created" {
                String::new()
            } else {
                "created".into()
            };
        }),
        "cards-merged" => ui.insights.drilldown.update(|cur| {
            *cur = if cur == "merged" {
                String::new()
            } else {
                "merged".into()
            };
        }),
        "cards-close" => ui.insights.drilldown.set(String::new()),
        "timing-cycle" => {
            ui.insights.timing_tab.update(|cur| {
                *cur = if cur == "cycle" {
                    String::new()
                } else {
                    "cycle".into()
                };
            });
            ui.insights.timing_page.set(0);
        }
        "timing-queue" => {
            ui.insights.timing_tab.update(|cur| {
                *cur = if cur == "queue" {
                    String::new()
                } else {
                    "queue".into()
                };
            });
            ui.insights.timing_page.set(0);
        }
        "timing-prev" => ui.insights.timing_page.update(|p| *p = p.saturating_sub(1)),
        "timing-next" => ui.insights.timing_page.update(|p| *p += 1),
        "refresh" => ui
            .insights
            .manual_refresh_seq
            .update(|v| *v = v.wrapping_add(1)),
        _ => {}
    }
}

fn build_insights_host_picker(ui: Ui, host_label: Memo<String>) -> impl IntoElement {
    search_popover(
        press_target()
            .h(px(26.))
            .px_2p5()
            .rounded(px(6.))
            .border_1()
            .border_color(BORDER)
            .bg(INSET)
            .hover(|s| s.bg(CARD_HOVER))
            .focus(|s| s.border_color(ACCENT))
            .active(|s| s.opacity(0.75))
            .flex_row()
            .items_center()
            .gap_1p5()
            .accessibility_label(move || Some(format!("Host scope: {}", host_label.get()).into()))
            .child(icon(Icon::Server).w(px(13.)).h(px(13.)))
            .child(
                label(move || host_label.get())
                    .text_size(px(CAPTION_SIZE))
                    .font_weight(FontWeight::MEDIUM)
                    .max_w(px(140.))
                    .truncate(),
            )
            .child(icon(Icon::ChevronDown).w(px(11.)).h(px(11.))),
        move || {
            let cur = ui.insights.scope_host.get();
            ui.snapshot.with(|s| {
                let mut opts = Vec::new();
                opts.push(
                    PickerOption::new(ALL_HOSTS_OPTION, "All Hosts")
                        .detail(format!("{} hosts", s.host_summaries.len().max(1)))
                        .icon(Icon::Server)
                        .selected(cur.is_empty()),
                );
                let mut hosts: Vec<(&str, &str, bool)> = Vec::new();
                hosts.push(("local", s.host_display_name("local"), false));
                let mut remotes: Vec<_> = s
                    .host_summaries
                    .iter()
                    .filter(|(id, _)| id.as_str() != "local")
                    .map(|(id, h)| (id.as_str(), h.name.as_str(), s.is_host_offline(id)))
                    .collect();
                remotes.sort_by_key(|a| a.1.to_ascii_lowercase());
                hosts.extend(remotes);
                for (hid, name, offline) in hosts {
                    let count = s
                        .projects
                        .iter()
                        .filter(|p| s.project_host(&p.id) == hid)
                        .count();
                    let opt_label = if offline {
                        format!("{name} (offline)")
                    } else {
                        name.to_owned()
                    };
                    opts.push(
                        PickerOption::new(hid, opt_label)
                            .group("Machines")
                            .detail(format!("{count} projects"))
                            .icon(Icon::Server)
                            .selected(cur == hid),
                    );
                }
                opts
            })
        },
        move |selected_host| {
            let next = if selected_host == ALL_HOSTS_OPTION {
                String::new()
            } else {
                selected_host
            };
            ui.insights.scope_host.set(next.clone());
            let cur_proj = ui.insights.scope_project.get_untracked();
            if !cur_proj.is_empty() && !next.is_empty() {
                let proj_host = ui
                    .snapshot
                    .with_untracked(|s| s.project_host(&cur_proj).to_owned());
                if proj_host != next {
                    ui.insights.scope_project.set(String::new());
                }
            }
        },
    )
    .placeholder("Filter Insights by Machine…")
    .width(240.)
}

pub fn chrome_bar(ui: Ui) -> View {
    let has_remote_hosts = computed(move || ui.snapshot.with(|s| s.has_remote_hosts()));
    let host_label = computed(move || {
        let sh = ui.insights.scope_host.get();
        if sh.is_empty() {
            "All Hosts".to_owned()
        } else {
            ui.snapshot.with(|s| {
                let name = s.host_display_name(&sh);
                if s.is_host_offline(&sh) {
                    format!("{name} (offline)")
                } else {
                    name.to_owned()
                }
            })
        }
    });
    let scope_label = computed(move || {
        let scope = ui.insights.scope_project.get();
        if scope.is_empty() {
            return "All Projects".to_owned();
        }
        ui.snapshot.with(|s| {
            s.projects
                .iter()
                .find(|p| p.id == scope)
                .map(|p| s.project_display_name(&p.id))
                .unwrap_or_else(|| "All Projects".to_owned())
        })
    });

    let date_label = computed(move || {
        let actual = current_today(ui);
        let selected = ui.insights.selected_date.get();
        let today = selected.unwrap_or(actual).min(actual);
        let range = ui.insights.range.get();
        if range == RangeWindow::Today {
            format!("{today} ({})", today.format("%a"))
        } else {
            let start = today - chrono::Duration::days(range.days() - 1);
            format!("{} – {}", start.format("%b %-d"), today.format("%b %-d"))
        }
    });

    let at_latest_date = computed(move || {
        let actual = current_today(ui);
        ui.insights.selected_date.get().is_none_or(|d| d >= actual)
    });

    let date_popover_open = signal(false);
    let date_input = element_ref();
    let date_picker = nsh_base::popover()
        .open(move || Some(date_popover_open.get()))
        .placement(Placement::Bottom)
        .align(Align::Start)
        .offset(px(4.))
        .margin(px(8.))
        .popup_class(canban_ui_kit::motion::POPUP_CLASS)
        .popup_starting_class(canban_ui_kit::motion::POPUP_STARTING_CLASS)
        .popup_ending_class(canban_ui_kit::motion::POPUP_ENDING_CLASS)
        .on_open_change(move |e: &nsh_base::PopoverEvent| {
            if let nsh_base::PopoverEvent::OpenChange(next, _) = e {
                if *next {
                    let active = ui
                        .insights
                        .selected_date
                        .get_untracked()
                        .unwrap_or_else(|| current_today(ui));
                    ui.insights.date_text.set(active.to_string());
                }
                date_popover_open.set(*next);
            }
        })
        .on_open(move || date_input.focus())
        .trigger(
            press_target()
                .h(px(26.))
                .px_2()
                .rounded(px(5.))
                .hover(|s| s.bg(INSET))
                .focus(|s| s.border_1().border_color(ACCENT))
                .active(|s| s.opacity(0.75))
                .flex_row()
                .items_center()
                .accessibility_label(move || Some(format!("Date: {}", date_label.get()).into()))
                .child(
                    label(move || date_label.get())
                        .font_family(MONO_FONT)
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM),
                ),
        )
        .child({
            let submit_date = move || {
                intent(ui, "apply-date");
                date_popover_open.set(false);
            };
            view()
                .w(px(232.))
                .p_3()
                .gap_2()
                .bg(SURFACE)
                .rounded(px(8.))
                .border_1()
                .border_color(BORDER)
                .shadow_lg()
                .child(
                    label("Jump to date")
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .child(
                            field(ui.insights.date_text, "YYYY-MM-DD")
                                .element_ref(&date_input)
                                .label("Insights date")
                                .min_h(px(28.))
                                .px_2()
                                .py_1()
                                .text_size(px(12.))
                                .flex_1()
                                .on_submit(move |_: &str| submit_date()),
                        )
                        .child(
                            compact_action("Apply", Variant::Primary, move |_| submit_date())
                                .disabled(move || {
                                    let actual = current_today(ui);
                                    chrono::NaiveDate::parse_from_str(
                                        &ui.insights.date_text.get(),
                                        "%Y-%m-%d",
                                    )
                                    .ok()
                                    .is_none_or(|d| d > actual)
                                }),
                        ),
                )
        });

    let scope_picker = search_popover(
        press_target()
            .h(px(26.))
            .px_2p5()
            .rounded(px(6.))
            .border_1()
            .border_color(BORDER)
            .bg(INSET)
            .hover(|s| s.bg(CARD_HOVER))
            .focus(|s| s.border_color(ACCENT))
            .active(|s| s.opacity(0.75))
            .flex_row()
            .items_center()
            .gap_1p5()
            .accessibility_label(move || {
                Some(format!("Project scope: {}", scope_label.get()).into())
            })
            .child(icon(Icon::Projects).w(px(13.)).h(px(13.)))
            .child(
                label(move || scope_label.get())
                    .text_size(px(CAPTION_SIZE))
                    .font_weight(FontWeight::MEDIUM)
                    .max_w(px(180.))
                    .truncate(),
            )
            .child(icon(Icon::ChevronDown).w(px(11.)).h(px(11.))),
        move || {
            let cur = ui.insights.scope_project.get();
            let host_filter = ui.insights.scope_host.get();
            ui.snapshot.with(|s| {
                let total_merged = s
                    .cards
                    .iter()
                    .filter(|c| {
                        c.removed_at.is_none()
                            && c.resolved()
                            && (host_filter.is_empty()
                                || s.project_host(&c.project_id) == host_filter)
                    })
                    .count();
                let mut opts = Vec::with_capacity(s.projects.len() + 1);
                opts.push(
                    PickerOption::new(ALL_PROJECTS_OPTION, "All Projects")
                        .detail(format!("{total_merged} resolved"))
                        .icon(Icon::Projects)
                        .selected(cur.is_empty()),
                );
                for p in &s.projects {
                    if !host_filter.is_empty() && s.project_host(&p.id) != host_filter {
                        continue;
                    }
                    let p_merged = s
                        .cards
                        .iter()
                        .filter(|c| c.project_id == p.id && c.removed_at.is_none() && c.resolved())
                        .count();
                    let offline = s.is_host_offline(s.project_host(&p.id));
                    let mut pname = s.project_display_name(&p.id);
                    if offline {
                        pname.push_str(" (offline)");
                    }
                    opts.push(
                        PickerOption::new(p.id.clone(), pname)
                            .group("Projects")
                            .detail(format!("{p_merged} resolved"))
                            .icon(Icon::Folder)
                            .selected(cur == p.id),
                    );
                }
                opts
            })
        },
        move |selected_id| {
            if selected_id == ALL_PROJECTS_OPTION {
                ui.insights.scope_project.set(String::new());
            } else {
                ui.insights.scope_project.set(selected_id);
            }
        },
    )
    .placeholder("Filter Insights by Project…")
    .width(300.);

    let range_bar = view()
        .flex_row()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(7.))
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .children(
            [
                RangeWindow::Today,
                RangeWindow::Days7,
                RangeWindow::Days30,
                RangeWindow::Days90,
            ]
            .into_iter()
            .map(|rw| {
                compact_segment(
                    rw.label(),
                    None,
                    move || ui.insights.range.get() == rw,
                    move |_| set_range(ui, rw),
                )
                .min_h(px(22.))
                .px_2()
                .into_element()
            })
            .collect::<Vec<_>>(),
        );

    let spinner_slot = view()
        .w(px(16.))
        .h(px(16.))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .child(dynamic(move || {
            if ui.insights.loading.get() || ui.insights.preview_loading.get() {
                loading_spinner(12., MUTED).into_element()
            } else {
                view().w(px(12.)).h(px(12.)).into_element()
            }
        }));

    view()
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .flex_row()
        .items_center()
        .gap_2()
        .pl_2()
        .window_drag(true)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w(px(0.))
                .window_drag(false)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .child(icon_tinted(Icon::Insights, ACCENT).w(px(15.)).h(px(15.)))
                        .child(
                            label("Insights")
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(13.5)),
                        ),
                )
                .child(view().w(px(1.)).h(px(14.)).bg(BORDER))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(chrome_button(Icon::Back, "Previous day", move |_| {
                            intent(ui, "previous-day")
                        }))
                        .child(date_picker)
                        .child(chrome_button_disabled(
                            Icon::Forward,
                            "Next day",
                            move || at_latest_date.get(),
                            move |_| intent(ui, "next-day"),
                        ))
                        .child(
                            compact_action("Today", Variant::Quiet, move |_| intent(ui, "today"))
                                .accessibility_label("Today"),
                        ),
                )
                .child(view().w(px(1.)).h(px(14.)).bg(BORDER))
                .child(range_bar)
                .child(view().w(px(1.)).h(px(14.)).bg(BORDER))
                .child(when(
                    move || has_remote_hosts.get(),
                    move || build_insights_host_picker(ui, host_label),
                ))
                .child(scope_picker)
                .child(chrome_button(
                    Icon::Refresh,
                    "Refresh Insights",
                    move |_| intent(ui, "refresh"),
                ))
                .child(spinner_slot),
        )
        .child(view().flex_1().h_full().window_drag(true))
}

fn section_wrap() -> View {
    view()
        .w_full()
        .pt_5()
        .border_t_1()
        .border_color(BORDER)
        .gap_3()
}

fn legend_swatch(color: impl IntoReactive<AdaptiveColor> + 'static, title: &str) -> View {
    view()
        .flex_row()
        .items_center()
        .gap_1p5()
        .child(view().w(px(8.)).h(px(8.)).rounded(px(2.)).bg(color))
        .child(muted(title.to_owned()).text_size(px(11.5)))
}

fn heading(title: &str) -> Text {
    label(title.to_owned())
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(14.))
}

fn note(value: impl Into<String>) -> Text {
    muted(value.into()).text_size(px(11.5))
}

fn mono_val(value: impl Into<String>, size: f32) -> Text {
    text(value.into())
        .font_family(MONO_FONT)
        .text_size(px(size))
        .text_color(TEXT)
}

fn row() -> View {
    view().flex_row().flex_wrap().items_center().gap_2p5()
}

fn choice(ui: Ui, title: &'static str, op: &'static str, selected: bool) -> impl IntoElement {
    compact_action(
        title,
        if selected {
            Variant::Secondary
        } else {
            Variant::Quiet
        },
        move |_| intent(ui, op),
    )
    .accessibility_label(title)
}

fn format_leverage(val: Option<f64>) -> String {
    match val {
        Some(v) if v >= 10.0 => format!("{v:.0}×"),
        Some(v) => format!("{v:.1}×"),
        None => "—".into(),
    }
}

fn host_color(index: usize, offline: bool) -> AdaptiveColor {
    if offline {
        return alpha_tint(MUTED, 0.55);
    }
    match index % 6 {
        0 => ACCENT.resolve(),
        1 => SUCCESS.resolve(),
        2 => ATTENTION.resolve(),
        3 => HIGHLIGHT.resolve(),
        4 => fixed_color(0x22b8cf),
        _ => fixed_color(0xe64980),
    }
}

fn render_proportional_bar(segments: &[(String, u64, AdaptiveColor)]) -> View {
    let total: u64 = segments.iter().map(|(_, v, _)| *v).sum();
    if total == 0 {
        return view();
    }

    let bar_items: Vec<AnyElement> = segments
        .iter()
        .filter(|(_, v, _)| *v > 0)
        .map(|(name, v, color)| {
            let pct = ((*v as u128 * 100 + total as u128 / 2) / total as u128) as usize;
            let tip = format!("{name} · {} ({pct}%)", format_duration_compact(*v));
            view()
                .flex_grow((*v as f32).max(1.0))
                .basis(px(0.))
                .min_w(px(3.))
                .h_full()
                .bg(*color)
                .tooltip(tooltip_box().child(note(tip)))
                .into_element()
        })
        .collect();

    let legend_items: Vec<AnyElement> = segments
        .iter()
        .filter(|(_, v, _)| *v > 0)
        .take(7)
        .map(|(name, v, color)| {
            let pct = ((*v as u128 * 100 + total as u128 / 2) / total as u128) as usize;
            let c = *color;
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .child(view().w(px(7.)).h(px(7.)).rounded(px(3.5)).bg(c))
                .child(label(name.clone()).text_size(px(11.5)).text_color(TEXT))
                .child(
                    text(format!("{pct}%"))
                        .font_family(MONO_FONT)
                        .text_size(px(11.))
                        .text_color(MUTED),
                )
                .into_element()
        })
        .collect();

    view()
        .w_full()
        .gap_2()
        .child(
            view()
                .w_full()
                .h(px(8.))
                .rounded(px(4.))
                .bg(INSET)
                .overflow_hidden()
                .flex_row()
                .gap(px(1.5))
                .children(bar_items),
        )
        .child(
            view()
                .w_full()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_3()
                .children(legend_items),
        )
}

fn render_compare_subbar(ui: Ui, dash: &Dashboard) -> View {
    let notice_text = if dash.history_start.is_none_or(|h| h > dash.selected_date) {
        Some("No retained history")
    } else if dash
        .history_start
        .is_some_and(|h| h.to_string() > dash.window_start_key)
    {
        Some("Partial history")
    } else {
        None
    };

    let has_project_filter = !ui.insights.scope_project.get().is_empty();
    let has_host_filter = !ui.insights.scope_host.get().is_empty();

    let mut left_row = view().flex_row().flex_wrap().items_center().gap_2();
    if let Some(msg) = notice_text {
        left_row = left_row.child(note(msg));
    }
    if has_host_filter {
        left_row = left_row.child(compact_action(
            "Clear Host filter",
            Variant::Quiet,
            move |_| {
                ui.insights.scope_host.set(String::new());
            },
        ));
    }
    if has_project_filter {
        left_row = left_row.child(compact_action(
            "Clear Project filter",
            Variant::Quiet,
            move |_| {
                ui.insights.scope_project.set(String::new());
            },
        ));
    }

    let right = if dash.range == RangeWindow::Today {
        let prev_label = if dash.selected_date == dash.annual_end {
            "Yesterday so far"
        } else {
            "Previous day"
        };
        view()
            .flex_row()
            .items_center()
            .gap_1p5()
            .child(note("Compare:"))
            .child(choice(
                ui,
                prev_label,
                "compare-previous",
                ui.insights.comparison.get() == Comparison::Previous,
            ))
            .child(choice(
                ui,
                "Last week",
                "compare-week",
                ui.insights.comparison.get() == Comparison::LastWeek,
            ))
            .into_element()
    } else {
        let prior = match dash.range {
            RangeWindow::Days7 => "vs prior 7d",
            RangeWindow::Days30 => "vs prior 30d",
            RangeWindow::Days90 => "vs prior 90d",
            RangeWindow::Today => "",
        };
        view()
            .tooltip(tooltip_box().child(note(format!(
                "{} · {}",
                dash.comparison_label, dash.timezone
            ))))
            .child(note(prior))
            .into_element()
    };

    view()
        .w_full()
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_2()
        .child(left_row)
        .child(right)
}

fn format_count_delta(current: usize, previous: usize, baseline_known: bool) -> (String, bool) {
    if !baseline_known {
        return (String::new(), true);
    }
    if current > previous {
        (format!("▲{}", current - previous), true)
    } else if current < previous {
        (format!("▼{}", previous - current), false)
    } else {
        ("0".into(), true)
    }
}

fn format_pct_delta(current: u64, previous: u64, baseline_known: bool, act_known: bool) -> String {
    if !baseline_known || !act_known {
        return String::new();
    }
    if previous == 0 {
        if current == 0 {
            return "0%".into();
        }
        return format!("▲{}", format_duration_compact(current));
    }
    if current == previous {
        return "0%".into();
    }
    let diff = current.abs_diff(previous);
    let pct = (diff * 100 + previous / 2) / previous;
    if current > previous {
        format!("▲{pct}%")
    } else {
        format!("▼{pct}%")
    }
}

fn format_leverage_delta(
    current: Option<f64>,
    previous: Option<f64>,
    baseline_known: bool,
    act_known: bool,
) -> (String, Option<bool>) {
    if !baseline_known || !act_known {
        return (String::new(), None);
    }
    let (Some(c), Some(p)) = (current, previous) else {
        return (String::new(), None);
    };
    let diff = c - p;
    if diff.abs() < 0.05 {
        ("0.0×".into(), None)
    } else if diff > 0.0 {
        (format!("▲{diff:.1}×"), Some(true))
    } else {
        (format!("▼{:.1}×", -diff), Some(false))
    }
}

fn kpi_cell(
    title: &'static str,
    value: String,
    value_color: ThemeColor,
    delta: String,
    delta_positive: Option<bool>,
    sub: String,
    tip: String,
) -> View {
    let mut val_row = view().flex_row().items_baseline().gap_2().child(
        text(value)
            .font_family(MONO_FONT)
            .text_size(px(24.))
            .font_weight(FontWeight::BOLD)
            .text_color(value_color),
    );
    if !delta.is_empty() {
        let color = match delta_positive {
            Some(true) => SUCCESS,
            Some(false) => ATTENTION,
            None => MUTED,
        };
        val_row = val_row.child(
            text(delta)
                .font_family(MONO_FONT)
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color),
        );
    }
    view()
        .flex_1()
        .min_w(px(160.))
        .py_2()
        .px_3()
        .gap_1()
        .tooltip(tooltip_box().child(note(tip)))
        .child(
            muted(title)
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(val_row)
        .child(
            text(sub)
                .font_family(MONO_FONT)
                .text_size(px(11.))
                .text_color(MUTED)
                .truncate(),
        )
}

fn render_kpis(ui: Ui, dash: &Dashboard, narrow: bool) -> View {
    let known = dash.history_start.is_some_and(|d| d <= dash.selected_date);
    let act_known = known
        && (dash.activity_loaded
            || dash.time_totals.dwell_total_ms > 0
            || dash.time_totals.agent_total_ms > 0);
    let t = &dash.time_totals;

    let my_time_str = if act_known {
        format_duration_compact(t.dwell_total_ms)
    } else {
        "—".into()
    };
    let agent_time_str = if act_known {
        format_duration_compact(t.agent_total_ms)
    } else {
        "—".into()
    };

    // 1. AGENT LEVERAGE
    let lev_val = if act_known {
        format_leverage(t.leverage())
    } else {
        "—".into()
    };
    let lev_color = match t.leverage() {
        Some(v) if act_known && v >= 2.0 => SUCCESS,
        _ => TEXT,
    };
    let (lev_delta, lev_pos) = format_leverage_delta(
        t.leverage(),
        t.prev_leverage(),
        dash.baseline_known,
        act_known,
    );
    let parallel_suffix = if dash.peak_parallel > 1 {
        format!(" · peak {}∥", dash.peak_parallel)
    } else {
        String::new()
    };
    let lev_sub = format!("Agent {agent_time_str} / Me {my_time_str}{parallel_suffix}");
    let lev_tip = format!(
        "Agent {agent_time_str} · Me {my_time_str} · Peak {} parallel",
        dash.peak_parallel
    );
    let leverage_cell = kpi_cell(
        "AGENT LEVERAGE",
        lev_val,
        lev_color,
        lev_delta,
        lev_pos,
        lev_sub,
        lev_tip,
    );

    // 2. CARDS
    let (res_delta, res_pos) = format_count_delta(
        dash.merged_total,
        dash.prev_merged_total,
        dash.baseline_known,
    );
    let net_sign = if dash.net_wip_delta > 0 { "+" } else { "" };
    let cards_sub = format!(
        "+{} in · net {net_sign}{} · {} WIP",
        dash.created_total, dash.net_wip_delta, dash.pipeline.active_wip
    );
    let cards_tip = format!(
        "{} resolved ({} PR · {} local · {} manual) · +{} created · {} WIP (B {} / D {} / R {})",
        dash.merged_total,
        dash.pr_merges,
        dash.local_merges,
        dash.manual_resolutions,
        dash.created_total,
        dash.pipeline.active_wip,
        dash.pipeline.backlog,
        dash.pipeline.develop,
        dash.pipeline.review,
    );
    let resolved_active = ui.insights.drilldown.get() == "merged";
    let cards_cell = press_target()
        .flex_1()
        .min_w(px(160.))
        .rounded(px(6.))
        .bg(if resolved_active {
            SELECTED.resolve()
        } else {
            AdaptiveColor::TRANSPARENT
        })
        .hover(|s| s.bg(CARD_HOVER))
        .accessibility_label("Cards KPI")
        .on_press(move |_: &()| intent(ui, "cards-merged"))
        .child(kpi_cell(
            "CARDS DONE",
            if known {
                dash.merged_total.to_string()
            } else {
                "—".into()
            },
            TEXT,
            res_delta,
            (dash.merged_total != dash.prev_merged_total).then_some(res_pos),
            cards_sub,
            cards_tip,
        ));

    // 3. AGENT PASS RATE
    let pass_val = dash
        .success_rate_pct
        .map(|p| format!("{p}%"))
        .unwrap_or_else(|| "—".into());
    let pass_color = match dash.success_rate_pct {
        Some(p) if p >= 85 => SUCCESS,
        Some(p) if p < 60 => ATTENTION,
        _ => TEXT,
    };
    let (pass_delta, pass_pos) = match (
        dash.success_rate_pct,
        dash.prev_success_rate_pct,
        dash.baseline_known,
    ) {
        (Some(c), Some(p), true) => {
            let diff = c as isize - p as isize;
            if diff > 0 {
                (format!("▲{diff}pt"), Some(true))
            } else if diff < 0 {
                (format!("▼{}pt", -diff), Some(false))
            } else {
                ("0pt".into(), None)
            }
        }
        _ => (String::new(), None),
    };
    let resolved_runs = dash.completed_runs + dash.blocked_runs + dash.failed_runs;
    let pass_sub = if resolved_runs > 0 {
        format!(
            "{}/{} runs · {} blocked · {} failed",
            dash.completed_runs, resolved_runs, dash.blocked_runs, dash.failed_runs
        )
    } else if dash.active_runs > 0 {
        format!("{} active runs", dash.active_runs)
    } else {
        "0 runs".into()
    };
    let pass_tip = format!(
        "{} total runs · {} completed · {} blocked · {} failed · {} active · {} asks",
        dash.scheduled_total,
        dash.completed_runs,
        dash.blocked_runs,
        dash.failed_runs,
        dash.active_runs,
        dash.human_asks_period,
    );
    let pass_cell = kpi_cell(
        "AGENT PASS RATE",
        pass_val,
        pass_color,
        pass_delta,
        pass_pos,
        pass_sub,
        pass_tip,
    );

    // 4. ACTIVE TIME
    let active_val = if act_known {
        format_duration_compact(t.total_active_ms())
    } else {
        "—".into()
    };
    let active_delta = format_pct_delta(
        t.total_active_ms(),
        t.prev_total_active_ms(),
        dash.baseline_known,
        act_known,
    );
    let active_sub = format!("Me {my_time_str} · Agent {agent_time_str}");
    let active_tip = format!(
        "Card {} · Terminal {} · Other {} · Agent {}",
        format_duration_compact(t.dwell_card_ms),
        format_duration_compact(t.dwell_terminal_ms),
        format_duration_compact(t.dwell_other_ms),
        agent_time_str,
    );
    let active_cell = kpi_cell(
        "ACTIVE TIME",
        active_val,
        TEXT,
        active_delta,
        None,
        active_sub,
        active_tip,
    );

    let v_sep = || {
        view()
            .w(px(1.))
            .my_2()
            .bg(BORDER)
            .opacity(0.65)
            .flex_shrink_0()
    };

    if narrow {
        view()
            .pb_3()
            .gap_1()
            .child(
                view()
                    .flex_row()
                    .items_stretch()
                    .child(leverage_cell)
                    .child(v_sep())
                    .child(cards_cell),
            )
            .child(
                view()
                    .flex_row()
                    .items_stretch()
                    .child(pass_cell)
                    .child(v_sep())
                    .child(active_cell),
            )
    } else {
        view()
            .pb_3()
            .flex_row()
            .items_stretch()
            .child(leverage_cell)
            .child(v_sep())
            .child(cards_cell)
            .child(v_sep())
            .child(pass_cell)
            .child(v_sep())
            .child(active_cell)
    }
}

fn format_hhmm(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "—".into())
}

fn render_timeline_track_backdrop(
    tick_count: usize,
    active_ratio: Option<f32>,
    is_all_future: bool,
) -> View {
    let mut backdrop = view()
        .absolute()
        .inset_0()
        .pointer_events(PointerEvents::None);

    if tick_count >= 2 {
        backdrop = backdrop.child(
            view()
                .absolute()
                .inset_0()
                .flex_row()
                .justify_between()
                .children((0..tick_count).map(|i| {
                    if i == 0 || i + 1 == tick_count {
                        view().w(px(1.)).h_full().into_element()
                    } else {
                        view()
                            .w(px(1.))
                            .h_full()
                            .bg(BORDER)
                            .opacity(0.30)
                            .into_element()
                    }
                })),
        );
    }

    if is_all_future {
        backdrop = backdrop.child(view().w_full().h_full().bg(BACKGROUND).opacity(0.45));
    } else if let Some(ratio) = active_ratio {
        backdrop = backdrop.child(
            view()
                .absolute()
                .inset_0()
                .flex_row()
                .child(view().flex_grow(ratio).basis(px(0.)).h_full())
                .child(view().w(px(1.)).h_full().bg(ACCENT).opacity(0.65))
                .child(
                    view()
                        .flex_grow((1.0 - ratio).max(0.0))
                        .basis(px(0.))
                        .h_full()
                        .bg(BACKGROUND)
                        .opacity(0.45),
                ),
        );
    }
    backdrop
}

fn render_timeline_1d(ui: Ui, dash: &Dashboard, project_order: &[String]) -> View {
    let tl = &dash.timeline;
    let full_24h = ui.insights.timeline_full_24h.get();
    let (start_h, end_h) = if full_24h {
        (0u32, 24u32)
    } else {
        tl.visible_hour_window()
    };
    let win_start_ms = (tl.day_start_ms + u64::from(start_h) * 3_600_000).min(tl.day_end_ms);
    let win_end_ms = if end_h >= 24 {
        tl.day_end_ms
    } else {
        (tl.day_start_ms + u64::from(end_h) * 3_600_000).min(tl.day_end_ms)
    }
    .max(win_start_ms.saturating_add(1));
    let span = (win_end_ms.saturating_sub(win_start_ms)).max(1) as f64;

    let span_h = end_h.saturating_sub(start_h);
    let step = if span_h <= 12 {
        1usize
    } else if span_h.is_multiple_of(3) {
        3usize
    } else if span_h.is_multiple_of(2) {
        2usize
    } else {
        1usize
    };
    let ticks: Vec<u32> = (start_h..=end_h).step_by(step).collect();
    let tick_count = ticks.len();

    let is_all_future = tl.active_until_ms <= win_start_ms;
    let active_ratio = (!is_all_future && tl.active_until_ms < win_end_ms).then(|| {
        ((tl.active_until_ms.saturating_sub(win_start_ms)) as f64 / span).clamp(0.0, 1.0) as f32
    });

    let axis_row = view()
        .flex_row()
        .items_center()
        .gap_2()
        .child(view().w(px(168.)).flex_shrink_0())
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .flex_row()
                .justify_between()
                .children(
                    ticks
                        .into_iter()
                        .map(|h| {
                            text(h.to_string())
                                .font_family(MONO_FONT)
                                .text_size(px(10.))
                                .text_color(MUTED)
                                .into_element()
                        })
                        .collect::<Vec<_>>(),
                ),
        )
        .child(view().w(px(54.)).flex_shrink_0());

    let mut seen_first_agent = false;
    let lane_views = tl
        .lanes
        .iter()
        .map(|lane| {
            let mut cursor = 0.0f32;
            let mut blocks_children: Vec<AnyElement> = Vec::new();
            for b in &lane.blocks {
                if b.end_ms <= win_start_ms || b.start_ms >= win_end_ms {
                    continue;
                }
                let s_ms = b.start_ms.max(win_start_ms);
                let e_ms = b.end_ms.min(win_end_ms);
                let start_r = ((s_ms - win_start_ms) as f64 / span).clamp(0.0, 1.0) as f32;
                let end_r = ((e_ms - win_start_ms) as f64 / span).clamp(0.0, 1.0) as f32;
                let gap = (start_r - cursor).max(0.0);
                if gap > 0.0 {
                    blocks_children
                        .push(view().flex_grow(gap).basis(px(0.)).h_full().into_element());
                    cursor += gap;
                }
                let block_r = (end_r - cursor).max(0.0);
                if block_r <= 0.0 && e_ms <= s_ms {
                    continue;
                }

                let dur = b.active_ms;
                let color = project_color(b.project_id.as_deref(), project_order);
                let tip = format!(
                    "{} · {} · {}–{} ({}){}",
                    lane.label,
                    b.label,
                    format_hhmm(b.start_ms),
                    format_hhmm(b.end_ms),
                    format_duration_compact(dur),
                    if b.open { " · Live" } else { "" }
                );
                let lane_kind = lane.kind;
                let cid = b.card_id.clone();
                let tid = b.terminal_id.clone();
                let clickable = cid.is_some() || tid.is_some();

                let mut block = press_target()
                    .w_full()
                    .h_full()
                    .rounded(px(3.))
                    .bg(color)
                    .hover(|s| s.opacity(0.85));
                if b.open {
                    block = block.border_1().border_color(TEXT);
                }
                if clickable {
                    block = block.on_press(move |_: &()| match lane_kind {
                        TimelineLaneKind::MeTerminal => {
                            if let Some(t) = tid.clone() {
                                ui.open_terminal(t);
                            } else if let Some(c) = cid.clone() {
                                ui.open_card(c);
                            }
                        }
                        _ => {
                            if let Some(c) = cid.clone() {
                                ui.open_card(c);
                            } else if let Some(t) = tid.clone() {
                                ui.open_terminal(t);
                            }
                        }
                    });
                }

                blocks_children.push(
                    view()
                        .flex_grow(block_r.max(0.002))
                        .basis(px(0.))
                        .min_w(px(3.))
                        .h_full()
                        .tooltip(tooltip_box().child(note(tip)))
                        .child(block)
                        .into_element(),
                );
                cursor += block_r;
            }
            let tail = (1.0 - cursor).max(0.0);
            if tail > 0.0 || blocks_children.is_empty() {
                blocks_children.push(
                    view()
                        .flex_grow(tail.max(0.001))
                        .basis(px(0.))
                        .h_full()
                        .into_element(),
                );
            }

            let track = view()
                .flex_1()
                .min_w(px(0.))
                .h(px(20.))
                .rounded(px(4.))
                .bg(INSET)
                .relative()
                .overflow_hidden()
                .child(render_timeline_track_backdrop(
                    tick_count,
                    active_ratio,
                    is_all_future,
                ))
                .child(
                    view()
                        .w_full()
                        .h_full()
                        .py(px(2.))
                        .flex_row()
                        .items_center()
                        .children(blocks_children),
                );

            let (group_txt, item_txt) = match lane.kind {
                TimelineLaneKind::MeCard => ("Me", "Card"),
                TimelineLaneKind::MeTerminal => ("", "Terminal"),
                TimelineLaneKind::MeOther => ("", "Other"),
                TimelineLaneKind::Agent => {
                    let g = if !seen_first_agent {
                        seen_first_agent = true;
                        "Agent"
                    } else {
                        ""
                    };
                    (g, lane.label.as_str())
                }
            };

            let mut left_label = view()
                .w(px(168.))
                .flex_shrink_0()
                .flex_row()
                .items_center()
                .gap_1p5()
                .child(
                    muted(group_txt)
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .w(px(46.))
                        .flex_shrink_0(),
                )
                .child(
                    label(item_txt.to_owned())
                        .text_size(px(12.))
                        .truncate()
                        .flex_1()
                        .min_w(px(0.)),
                );
            if let Some(sub) = lane.sublabel.as_deref() {
                left_label = left_label
                    .tooltip(tooltip_box().child(note(format!("{} ({sub})", lane.label))));
            }

            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(left_label)
                .child(track)
                .child(
                    mono_val(
                        if lane.total_ms > 0 {
                            format_duration_compact(lane.total_ms)
                        } else {
                            "—".into()
                        },
                        11.5,
                    )
                    .w(px(54.))
                    .flex_shrink_0(),
                )
                .into_element()
        })
        .collect::<Vec<_>>();

    let mut merge_cursor = 0.0f32;
    let mut merge_children: Vec<AnyElement> = Vec::new();
    for m in &tl.merges {
        if m.timestamp_ms < win_start_ms || m.timestamp_ms > win_end_ms {
            continue;
        }
        let ratio =
            ((m.timestamp_ms.saturating_sub(win_start_ms)) as f64 / span).clamp(0.0, 1.0) as f32;
        let gap = (ratio - merge_cursor).max(0.0);
        if gap > 0.0 {
            merge_children.push(view().flex_grow(gap).basis(px(0.)).h_full().into_element());
            merge_cursor += gap;
        }
        let tip = format!(
            "#{} {} · merged at {}",
            m.card_number,
            m.title,
            format_hhmm(m.timestamp_ms)
        );
        let cid = m.card_id.clone();
        merge_children.push(
            view()
                .w(px(8.))
                .h(px(8.))
                .flex_shrink_0()
                .tooltip(tooltip_box().child(note(tip)))
                .child(
                    press_target()
                        .w(px(8.))
                        .h(px(8.))
                        .rounded(px(4.))
                        .bg(SUCCESS)
                        .hover(|s| s.border_1().border_color(TEXT))
                        .on_press(move |_: &()| ui.open_card(cid.clone())),
                )
                .into_element(),
        );
    }
    let merge_tail = (1.0 - merge_cursor).max(0.0);
    if merge_tail > 0.0 || merge_children.is_empty() {
        merge_children.push(
            view()
                .flex_grow(merge_tail.max(0.001))
                .basis(px(0.))
                .h_full()
                .into_element(),
        );
    }

    let merges_track = view()
        .flex_1()
        .min_w(px(0.))
        .h(px(18.))
        .rounded(px(4.))
        .bg(INSET)
        .relative()
        .overflow_hidden()
        .child(render_timeline_track_backdrop(
            tick_count,
            active_ratio,
            is_all_future,
        ))
        .child(
            view()
                .w_full()
                .h_full()
                .flex_row()
                .items_center()
                .children(merge_children),
        );

    let merges_row = view()
        .flex_row()
        .items_center()
        .gap_2()
        .child(
            muted("Merges")
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .w(px(168.))
                .flex_shrink_0(),
        )
        .child(merges_track)
        .child(
            mono_val(tl.merges.len().to_string(), 11.5)
                .w(px(54.))
                .flex_shrink_0(),
        );

    let has_any = tl.lanes.iter().any(|l| !l.blocks.is_empty()) || !tl.merges.is_empty();
    let mut panel = view()
        .w_full()
        .pt_2()
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(note("Day timeline"))
                .child(
                    compact_action(
                        "24h",
                        if full_24h {
                            Variant::Secondary
                        } else {
                            Variant::Quiet
                        },
                        move |_| intent(ui, "toggle-timeline-24h"),
                    )
                    .accessibility_label("24h"),
                ),
        )
        .child(axis_row)
        .children(lane_views)
        .child(merges_row);

    if !has_any {
        panel = panel.child(note("No activity"));
    }
    panel
}

fn x_axis_ticks(days: &[DayBucket]) -> Vec<String> {
    let n = days.len();
    if n == 0 {
        return Vec::new();
    }
    if n <= 7 {
        return days.iter().map(|d| d.short_label.clone()).collect();
    }
    let slots = 6usize;
    (0..slots)
        .map(|i| {
            let idx = i * (n - 1) / (slots - 1);
            days[idx].short_label.clone()
        })
        .collect()
}

fn chart_y_axis(top_tick: usize) -> View {
    let mid_tick = top_tick / 2;
    view()
        .w(px(28.))
        .h(px(CHART_HEIGHT))
        .flex_shrink_0()
        .justify_between()
        .items_end()
        .pr_1p5()
        .child(
            text(top_tick.to_string())
                .font_family(MONO_FONT)
                .text_size(px(10.5))
                .text_color(MUTED),
        )
        .child(
            text(mid_tick.to_string())
                .font_family(MONO_FONT)
                .text_size(px(10.5))
                .text_color(MUTED),
        )
        .child(
            text("0")
                .font_family(MONO_FONT)
                .text_size(px(10.5))
                .text_color(MUTED),
        )
}

fn plot_grid_frame(top_tick: usize, col_gap: f32, columns: Vec<AnyElement>) -> View {
    view()
        .flex_row()
        .items_center()
        .child(chart_y_axis(top_tick))
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .h(px(CHART_HEIGHT))
                .relative()
                .child(
                    view()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(4.))
                        .h(px(1.))
                        .bg(BORDER)
                        .opacity(0.45)
                        .pointer_events(PointerEvents::None),
                )
                .child(
                    view()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(CHART_HEIGHT / 2.0))
                        .h(px(1.))
                        .bg(BORDER)
                        .opacity(0.45)
                        .pointer_events(PointerEvents::None),
                )
                .child(
                    view()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(1.))
                        .bg(BORDER)
                        .pointer_events(PointerEvents::None),
                )
                .child(
                    view()
                        .w_full()
                        .h_full()
                        .flex_row()
                        .items_end()
                        .gap(px(col_gap))
                        .children(columns),
                ),
        )
}

fn render_daily_output(ui: Ui, dash: &Dashboard) -> View {
    let top_tick = nice_ceil_tick(dash.max_daily_kanban);
    let n = dash.days.len();
    let col_gap = if n > 45 {
        1.0
    } else if n > 14 {
        2.0
    } else {
        6.0
    };
    let bar_max_w = if n > 45 {
        4.0
    } else if n > 14 {
        7.0
    } else {
        14.0
    };
    let pair_gap = if n > 45 { 1.0 } else { 2.0 };
    let usable_h = CHART_HEIGHT - 8.0;

    let columns = dash
        .days
        .iter()
        .enumerate()
        .map(|(day_index, day)| {
            let c_h = if day.created_cards == 0 {
                0.0
            } else {
                (day.created_cards as f32 / top_tick as f32 * usable_h).max(3.0)
            };
            let m_h = if day.merged_cards == 0 {
                0.0
            } else {
                (day.merged_cards as f32 / top_tick as f32 * usable_h).max(3.0)
            };
            let avg = dash.moving_average[day_index];
            let tip = format!(
                "{} ({}) · Resolved {} ({} local / {} PR) · Created {} · 7D mean {}",
                day.date_key,
                day.weekday_label,
                day.merged_cards,
                day.local_merges,
                day.pr_merges,
                day.created_cards,
                avg.map(|v| format!("{v:.1}")).unwrap_or_else(|| "—".into())
            );
            let date_parsed = chrono::NaiveDate::parse_from_str(&day.date_key, "%Y-%m-%d").ok();

            view()
                .flex_1()
                .min_w(px(0.))
                .h(px(CHART_HEIGHT))
                .tooltip(tooltip_box().child(note(tip)))
                .child(
                    press_target()
                        .w_full()
                        .h_full()
                        .rounded(px(4.))
                        .hover(|s| s.bg(CARD_HOVER))
                        .justify_end()
                        .items_center()
                        .on_press(move |_: &()| {
                            if let Some(d) = date_parsed {
                                select_date(ui, Some(d));
                                set_range(ui, RangeWindow::Today);
                            }
                        })
                        .relative()
                        .child(if let Some(avg) = avg {
                            view()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom(px((avg as f32 / top_tick as f32 * usable_h).min(usable_h)))
                                .h(px(2.))
                                .bg(ATTENTION)
                                .pointer_events(PointerEvents::None)
                                .into_element()
                        } else {
                            view().into_element()
                        })
                        .child(
                            view()
                                .w_full()
                                .flex_row()
                                .items_end()
                                .justify_center()
                                .gap(px(pair_gap))
                                .child(
                                    view()
                                        .flex_1()
                                        .max_w(px(bar_max_w))
                                        .h(px(c_h))
                                        .rounded(px(2.))
                                        .bg(move || alpha_tint(ACCENT, 0.45)),
                                )
                                .child(
                                    view()
                                        .flex_1()
                                        .max_w(px(bar_max_w))
                                        .h(px(m_h))
                                        .rounded(px(2.))
                                        .bg(SUCCESS),
                                ),
                        ),
                )
                .into_element()
        })
        .collect::<Vec<_>>();

    let ticks = x_axis_ticks(&dash.days);

    view()
        .w_full()
        .pt_2()
        .gap_2()
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(note("Daily created vs resolved"))
                .child(
                    row()
                        .child(legend_swatch(move || alpha_tint(ACCENT, 0.45), "Created"))
                        .child(legend_swatch(SUCCESS, "Resolved"))
                        .child(legend_swatch(ATTENTION, "7D mean")),
                ),
        )
        .child(plot_grid_frame(top_tick, col_gap, columns))
        .child(
            view().pl(px(28.)).flex_row().justify_between().children(
                ticks
                    .into_iter()
                    .map(|t| {
                        text(t)
                            .font_family(MONO_FONT)
                            .text_size(px(10.5))
                            .text_color(MUTED)
                            .into_element()
                    })
                    .collect::<Vec<_>>(),
            ),
        )
}

fn render_mini_sparkline(values: &[u64], max_val: u64, color: AdaptiveColor, height: f32) -> View {
    let cap = max_val.max(1);
    let bars: Vec<AnyElement> = values
        .iter()
        .map(|&v| {
            let h = if v == 0 {
                2.0
            } else {
                ((v as f32 / cap as f32) * height).clamp(3.0, height)
            };
            let bg = if v == 0 {
                alpha_tint(BORDER, 0.45)
            } else {
                color
            };
            view()
                .flex_1()
                .min_w(px(3.))
                .h(px(h))
                .rounded(px(1.5))
                .bg(bg)
                .into_element()
        })
        .collect();
    view()
        .h(px(height))
        .flex_row()
        .items_end()
        .gap(px(2.))
        .children(bars)
}

fn render_projects(ui: Ui, dash: &Dashboard, project_order: &[String], narrow: bool) -> View {
    let sort_mode = ui.insights.project_sort.get();
    let active_scope = ui.insights.scope_project.get();
    let mut projects = dash.project_stats.clone();
    match sort_mode {
        ProjectSort::Time => {
            projects.sort_by(|a, b| {
                b.total_time_ms()
                    .cmp(&a.total_time_ms())
                    .then_with(|| b.merged.cmp(&a.merged))
                    .then_with(|| a.name.cmp(&b.name))
            });
        }
        ProjectSort::Leverage => {
            projects.sort_by(|a, b| {
                let la = a.leverage().unwrap_or(-1.0);
                let lb = b.leverage().unwrap_or(-1.0);
                lb.total_cmp(&la)
                    .then_with(|| b.agent_ms.cmp(&a.agent_ms))
                    .then_with(|| a.name.cmp(&b.name))
            });
        }
        ProjectSort::Merged => {
            projects.sort_by(|a, b| {
                b.merged
                    .cmp(&a.merged)
                    .then_with(|| b.total_time_ms().cmp(&a.total_time_ms()))
                    .then_with(|| a.name.cmp(&b.name))
            });
        }
        ProjectSort::Created => {
            projects.sort_by(|a, b| {
                b.created
                    .cmp(&a.created)
                    .then_with(|| b.merged.cmp(&a.merged))
                    .then_with(|| a.name.cmp(&b.name))
            });
        }
        ProjectSort::Wip => {
            projects.sort_by(|a, b| {
                b.total_wip()
                    .cmp(&a.total_wip())
                    .then_with(|| b.merged.cmp(&a.merged))
                    .then_with(|| a.name.cmp(&b.name))
            });
        }
        ProjectSort::Runs => {
            projects.sort_by(|a, b| {
                b.agent_runs
                    .cmp(&a.agent_runs)
                    .then_with(|| b.merged.cmp(&a.merged))
                    .then_with(|| a.name.cmp(&b.name))
            });
        }
    }

    let mut split_segments: Vec<(String, u64, AdaptiveColor)> = projects
        .iter()
        .filter(|p| p.total_time_ms() > 0)
        .map(|p| {
            (
                p.name.clone(),
                p.total_time_ms(),
                project_color(Some(&p.id), project_order),
            )
        })
        .collect();
    let other_total = dash.other_dwell_ms.saturating_add(dash.other_agent_ms);
    if active_scope.is_empty() && other_total > 0 {
        split_segments.push(("Other".into(), other_total, alpha_tint(MUTED, 0.55)));
    }

    let spark_max = projects
        .iter()
        .flat_map(|p| p.recent_merges.iter().copied())
        .max()
        .unwrap_or(1)
        .max(1) as u64;

    let mut rows: Vec<AnyElement> = projects
        .into_iter()
        .map(|p| {
            let pid = p.id.clone();
            let is_scoped = active_scope == pid;
            let dot_color = project_color(Some(&p.id), project_order);
            let total_wip = p.total_wip();
            let lev = p.leverage();
            let lev_color = match lev {
                Some(v) if v >= 2.0 => SUCCESS,
                Some(_) => TEXT,
                None => MUTED,
            };
            let (pass_txt, pass_color) = match p.pass_rate_pct() {
                Some(pct) if pct >= 85 => (format!("{pct}%"), SUCCESS),
                Some(pct) if pct < 60 => (format!("{pct}%"), ATTENTION),
                Some(pct) => (format!("{pct}%"), TEXT),
                None if p.active_runs > 0 => ("run".into(), ACCENT),
                None => ("—".into(), MUTED),
            };

            let display_title = if let Some(ref badge_txt) = p.host_badge {
                format!("{} · {badge_txt}", p.name)
            } else {
                p.name.clone()
            };
            let tip = format!(
                "{display_title} · Me {} · Agent {} · Leverage {} · ✓{} done · +{} in · {} WIP (B {} / D {} / R {}) · {} runs · {} asks",
                format_duration_compact(p.dwell_ms),
                format_duration_compact(p.agent_ms),
                format_leverage(lev),
                p.merged,
                p.created,
                total_wip,
                p.wip_backlog,
                p.wip_develop,
                p.wip_review,
                p.agent_runs,
                p.human_asks,
            );

            let spark_vals: Vec<u64> = p.recent_merges.iter().map(|&n| n as u64).collect();
            let cards_cell = view()
                .w(px(148.))
                .flex_shrink_0()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    text(format!("+{}", p.created))
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .text_color(MUTED)
                        .w(px(30.)),
                )
                .child(
                    text(format!("✓{}", p.merged))
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .font_weight(if p.merged > 0 {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(if p.merged > 0 { SUCCESS } else { MUTED })
                        .w(px(36.)),
                )
                .child(
                    text(format!("{total_wip} wip"))
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .text_color(if p.wip_gated > 0 { ATTENTION } else { MUTED })
                        .flex_1(),
                );

            let mut row_content = view()
                .w_full()
                .flex_row()
                .items_center()
                .gap_3()
                .child(
                    view()
                        .flex_1()
                        .min_w(px(140.))
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            view()
                                .w(px(8.))
                                .h(px(8.))
                                .rounded(px(4.))
                                .bg(dot_color)
                                .flex_shrink_0(),
                        )
                        .child(
                            label(p.name.clone())
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .min_w(px(0.)),
                        )
                        .child({
                            let has_badge = p.host_badge.is_some();
                            let b = p.host_badge.clone().unwrap_or_default();
                            when(
                                move || has_badge,
                                move || {
                                    muted(b.clone())
                                        .text_size(px(11.))
                                        .flex_shrink_0()
                                },
                            )
                        }),
                )
                .child(
                    mono_val(
                        if p.dwell_ms > 0 {
                            format_duration_compact(p.dwell_ms)
                        } else {
                            "—".into()
                        },
                        11.5,
                    )
                    .w(px(68.))
                    .flex_shrink_0(),
                )
                .child(
                    mono_val(
                        if p.agent_ms > 0 {
                            format_duration_compact(p.agent_ms)
                        } else {
                            "—".into()
                        },
                        11.5,
                    )
                    .w(px(68.))
                    .flex_shrink_0(),
                )
                .child(
                    text(format_leverage(lev))
                        .font_family(MONO_FONT)
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(lev_color)
                        .w(px(68.))
                        .flex_shrink_0(),
                )
                .child(cards_cell)
                .child(
                    text(pass_txt)
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .text_color(pass_color)
                        .w(px(52.))
                        .flex_shrink_0(),
                );

            if !narrow {
                row_content = row_content.child(
                    view().w(px(62.)).flex_shrink_0().child(render_mini_sparkline(
                        &spark_vals,
                        spark_max,
                        alpha_tint(SUCCESS, 0.85),
                        16.0,
                    )),
                );
            }

            view()
                .w_full()
                .tooltip(tooltip_box().child(note(tip)))
                .child(
                    press_target()
                        .w_full()
                        .px_2()
                        .py_2()
                        .border_b_1()
                        .border_color(alpha_tint(BORDER, 0.45))
                        .bg(if is_scoped {
                            SELECTED.resolve()
                        } else {
                            AdaptiveColor::TRANSPARENT
                        })
                        .hover(|s| s.bg(CARD_HOVER))
                        .accessibility_label(format!("Insights Project: {display_title}"))
                        .on_press(move |_: &()| {
                            let cur = ui.insights.scope_project.get_untracked();
                            if cur == pid {
                                ui.insights.scope_project.set(String::new());
                            } else {
                                ui.insights.scope_project.set(pid.clone());
                            }
                        })
                        .child(row_content),
                )
                .into_element()
        })
        .collect();

    if active_scope.is_empty() && other_total > 0 {
        let mut other_row = view()
            .w_full()
            .px_2()
            .py_1p5()
            .flex_row()
            .items_center()
            .gap_3()
            .child(note("Other").flex_1().min_w(px(140.)))
            .child(
                note(if dash.other_dwell_ms > 0 {
                    format_duration_compact(dash.other_dwell_ms)
                } else {
                    "—".into()
                })
                .w(px(68.))
                .flex_shrink_0(),
            )
            .child(
                note(if dash.other_agent_ms > 0 {
                    format_duration_compact(dash.other_agent_ms)
                } else {
                    "—".into()
                })
                .w(px(68.))
                .flex_shrink_0(),
            )
            .child(note("—").w(px(68.)).flex_shrink_0())
            .child(note("—").w(px(148.)).flex_shrink_0())
            .child(note("—").w(px(52.)).flex_shrink_0());
        if !narrow {
            other_row = other_row.child(view().w(px(62.)).flex_shrink_0());
        }
        rows.push(other_row.into_element());
    }

    let mut header_row = view()
        .w_full()
        .px_2()
        .pb_1p5()
        .border_b_1()
        .border_color(BORDER)
        .flex_row()
        .items_center()
        .gap_3()
        .child(note("Project").flex_1().min_w(px(140.)))
        .child(note("My time").w(px(68.)).flex_shrink_0())
        .child(note("Agent").w(px(68.)).flex_shrink_0())
        .child(note("Leverage").w(px(68.)).flex_shrink_0())
        .child(note("Cards").w(px(148.)).flex_shrink_0())
        .child(note("Pass").w(px(52.)).flex_shrink_0());
    if !narrow {
        header_row = header_row.child(note("7D").w(px(62.)).flex_shrink_0());
    }

    let sort_controls = row()
        .child(choice(
            ui,
            "Time",
            "sort-time",
            sort_mode == ProjectSort::Time,
        ))
        .child(choice(
            ui,
            "Leverage",
            "sort-leverage",
            sort_mode == ProjectSort::Leverage,
        ))
        .child(choice(
            ui,
            "Merged",
            "sort-merged",
            sort_mode == ProjectSort::Merged,
        ))
        .child(choice(ui, "WIP", "sort-wip", sort_mode == ProjectSort::Wip))
        .child(choice(
            ui,
            "Runs",
            "sort-runs",
            sort_mode == ProjectSort::Runs,
        ));

    let mut sec = section_wrap().child(
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                view()
                    .flex_row()
                    .items_baseline()
                    .gap_2()
                    .child(heading("Projects"))
                    .child(mono_val(dash.project_stats.len().to_string(), 11.5).text_color(MUTED)),
            )
            .child(sort_controls),
    );

    if !split_segments.is_empty() {
        sec = sec.child(render_proportional_bar(&split_segments));
    }

    if rows.is_empty() {
        sec = sec.child(note("No project activity"));
    } else {
        sec = sec.child(view().w_full().child(header_row).children(rows));
    }
    sec
}

fn render_machine_card(
    ui: Ui,
    h: &HostStat,
    total_all_hosts_ms: u64,
    accent_color: AdaptiveColor,
    is_selected: bool,
) -> impl IntoElement {
    let hid = h.id.clone();
    let tot = h.total_time_ms();
    let share_pct = if total_all_hosts_ms > 0 {
        ((tot as u128 * 100 + total_all_hosts_ms as u128 / 2) / total_all_hosts_ms as u128) as usize
    } else {
        0
    };
    let lev = h.leverage();
    let lev_color = match lev {
        Some(v) if v >= 2.0 => SUCCESS,
        Some(_) => TEXT,
        None => MUTED,
    };
    let (pass_txt, pass_color) = match h.pass_rate_pct() {
        Some(pct) if pct >= 85 => (format!("{pct}% pass"), SUCCESS),
        Some(pct) if pct < 60 => (format!("{pct}% pass"), ATTENTION),
        Some(pct) => (format!("{pct}% pass"), TEXT),
        None if h.active_runs > 0 => (format!("{} active", h.active_runs), ACCENT),
        None => ("— pass".into(), MUTED),
    };

    let status_dot_color = if h.offline {
        MUTED.resolve()
    } else {
        SUCCESS.resolve()
    };

    let top_projects_str = if h.top_projects.is_empty() {
        "—".into()
    } else {
        h.top_projects
            .iter()
            .map(|(name, ms)| {
                if *ms > 0 {
                    format!("{name} {}", format_duration_compact(*ms))
                } else {
                    name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" · ")
    };

    let spark_max = h.load_bars.iter().copied().max().unwrap_or(1).max(1);
    let has_spark = h.load_bars.iter().any(|&v| v > 0);

    let mut meta_row = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2p5()
        .child(
            text(format!("✓{} done", h.merged))
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(if h.merged > 0 { SUCCESS } else { MUTED }),
        )
        .child(
            text(format!("+{} in", h.created))
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(MUTED),
        )
        .child(
            text(format!("{} WIP", h.wip))
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(MUTED),
        )
        .child(
            text(pass_txt)
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(pass_color),
        )
        .child(
            text(format!("{} runs", h.agent_runs))
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(MUTED),
        );
    if h.peak_parallel > 1 {
        meta_row = meta_row.child(
            text(format!("peak {}∥", h.peak_parallel))
                .font_family(MONO_FONT)
                .text_size(px(11.))
                .text_color(MUTED),
        );
    }

    let mut card_body = view()
        .w_full()
        .gap_2p5()
        // Row 1: Identity + share %
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            view()
                                .w(px(8.))
                                .h(px(8.))
                                .rounded(px(4.))
                                .bg(status_dot_color)
                                .flex_shrink_0(),
                        )
                        .child(
                            label(h.name.clone())
                                .text_size(px(13.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if h.offline { MUTED } else { TEXT })
                                .truncate()
                                .min_w(px(0.)),
                        )
                        .child({
                            let tag = if h.offline {
                                Some("offline")
                            } else if h.is_local {
                                Some("local")
                            } else {
                                None
                            };
                            if let Some(t) = tag {
                                note(t).flex_shrink_0().into_element()
                            } else {
                                view().into_element()
                            }
                        }),
                )
                .child(
                    text(if total_all_hosts_ms > 0 {
                        format!("{share_pct}%")
                    } else {
                        "—".into()
                    })
                    .font_family(MONO_FONT)
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if is_selected { ACCENT } else { MUTED })
                    .flex_shrink_0(),
                ),
        )
        // Row 2: Agent & Me time + Leverage
        .child(
            view()
                .w_full()
                .flex_row()
                .items_baseline()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .items_baseline()
                        .gap_3()
                        .child(
                            text(format!("Agent {}", format_duration_compact(h.agent_ms)))
                                .font_family(MONO_FONT)
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT),
                        )
                        .child(
                            text(format!("Me {}", format_duration_compact(h.dwell_ms)))
                                .font_family(MONO_FONT)
                                .text_size(px(12.))
                                .text_color(MUTED),
                        ),
                )
                .child(
                    text(format_leverage(lev))
                        .font_family(MONO_FONT)
                        .text_size(px(15.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(lev_color)
                        .flex_shrink_0(),
                ),
        )
        // Row 3: Cards + Pass% + Runs + optional peak parallel
        .child(meta_row);

    // Row 4: Mini load sparkline + Top projects
    let mut bottom_row = view()
        .w_full()
        .pt_1p5()
        .border_t_1()
        .border_color(alpha_tint(BORDER, 0.45))
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3();
    if has_spark {
        bottom_row = bottom_row.child(view().w(px(96.)).flex_shrink_0().child(
            render_mini_sparkline(&h.load_bars, spark_max, accent_color, 18.0),
        ));
    }
    bottom_row = bottom_row.child(note(top_projects_str).flex_1().min_w(px(0.)).truncate());
    card_body = card_body.child(bottom_row);

    press_target()
        .flex_1()
        .min_w(px(280.))
        .p_4()
        .rounded(px(10.))
        .bg(if is_selected {
            SELECTED.resolve()
        } else {
            SURFACE.resolve()
        })
        .hover(|s| s.bg(CARD_HOVER))
        .accessibility_label(format!("Insights Host: {}", h.name))
        .on_press(move |_: &()| {
            let cur = ui.insights.scope_host.get_untracked();
            if cur == hid {
                ui.insights.scope_host.set(String::new());
            } else {
                ui.insights.scope_host.set(hid.clone());
                let cur_proj = ui.insights.scope_project.get_untracked();
                if !cur_proj.is_empty() {
                    let ph = ui
                        .snapshot
                        .with_untracked(|s| s.project_host(&cur_proj).to_owned());
                    if ph != hid {
                        ui.insights.scope_project.set(String::new());
                    }
                }
            }
        })
        .child(card_body)
}

fn render_machines(ui: Ui, dash: &Dashboard, narrow: bool) -> View {
    let active_host = ui.insights.scope_host.get();
    let total_all_hosts_ms: u64 = dash.host_stats.iter().map(HostStat::total_time_ms).sum();
    let online_count = dash.host_stats.iter().filter(|h| !h.offline).count();

    let mut sec = section_wrap().child(
        view()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                view()
                    .flex_row()
                    .items_baseline()
                    .gap_2()
                    .child(heading("Machines"))
                    .child(
                        mono_val(
                            format!("{online_count}/{} online", dash.host_stats.len()),
                            11.5,
                        )
                        .text_color(MUTED),
                    ),
            )
            .child(if total_all_hosts_ms > 0 {
                note(format!(
                    "Total {}",
                    format_duration_compact(total_all_hosts_ms)
                ))
                .into_element()
            } else {
                view().into_element()
            }),
    );

    if dash.host_stats.len() >= 2 && total_all_hosts_ms > 0 {
        let split: Vec<(String, u64, AdaptiveColor)> = dash
            .host_stats
            .iter()
            .enumerate()
            .filter(|(_, h)| h.total_time_ms() > 0)
            .map(|(idx, h)| {
                (
                    h.name.clone(),
                    h.total_time_ms(),
                    host_color(idx, h.offline),
                )
            })
            .collect();
        if !split.is_empty() {
            sec = sec.child(render_proportional_bar(&split));
        }
    }

    if narrow || dash.host_stats.len() == 1 {
        let cards: Vec<AnyElement> = dash
            .host_stats
            .iter()
            .enumerate()
            .map(|(idx, h)| {
                let is_sel = active_host == h.id;
                render_machine_card(
                    ui,
                    h,
                    total_all_hosts_ms,
                    host_color(idx, h.offline),
                    is_sel,
                )
                .into_element()
            })
            .collect();
        sec = sec.child(view().w_full().gap_3().children(cards));
    } else {
        let mut grid_rows: Vec<AnyElement> = Vec::new();
        let mut iter = dash.host_stats.iter().enumerate();
        while let Some((idx_a, a)) = iter.next() {
            let card_a = render_machine_card(
                ui,
                a,
                total_all_hosts_ms,
                host_color(idx_a, a.offline),
                active_host == a.id,
            );
            let row_el = if let Some((idx_b, b)) = iter.next() {
                let card_b = render_machine_card(
                    ui,
                    b,
                    total_all_hosts_ms,
                    host_color(idx_b, b.offline),
                    active_host == b.id,
                );
                view()
                    .w_full()
                    .flex_row()
                    .items_stretch()
                    .gap_3()
                    .child(card_a)
                    .child(card_b)
                    .into_element()
            } else {
                view()
                    .w_full()
                    .flex_row()
                    .items_stretch()
                    .gap_3()
                    .child(card_a)
                    .child(view().flex_1().min_w(px(280.)))
                    .into_element()
            };
            grid_rows.push(row_el);
        }
        sec = sec.child(view().w_full().gap_3().children(grid_rows));
    }

    sec
}

fn render_card_drilldown(ui: Ui, dash: &Dashboard) -> View {
    let filter = ui.insights.drilldown.get();
    if filter.is_empty() {
        return view();
    }
    let ids = if filter == "created" {
        &dash.created_ids
    } else {
        &dash.merged_ids
    };
    let label_kind = if filter == "merged" {
        "resolved"
    } else {
        &filter
    };
    let mut panel = view()
        .w_full()
        .p_3()
        .gap_1p5()
        .rounded(px(8.))
        .bg(SURFACE)
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    label(format!("{} {} Cards", ids.len(), label_kind))
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(compact_action("Close", Variant::Quiet, move |_| {
                    intent(ui, "cards-close")
                })),
        );
    for id in ids.iter().take(100) {
        if let Some(card) = ui.index.with(|idx| idx.cards.get(id).cloned()) {
            let proj_name = ui
                .snapshot
                .with(|s| s.project_display_name(&card.project_id));
            let id = id.clone();
            panel = panel.child(
                press_target()
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .rounded(px(5.))
                    .hover(|s| s.bg(CARD_HOVER))
                    .on_press(move |_: &()| ui.open_card(id.clone()))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                label(format!("#{} {}", card.number, card.title))
                                    .text_size(px(12.5))
                                    .truncate()
                                    .flex_1()
                                    .min_w(px(0.)),
                            )
                            .child(note(proj_name).flex_shrink_0()),
                    ),
            );
        }
    }
    if ids.len() > 100 {
        panel = panel.child(note("First 100 cards"));
    }
    panel
}

fn render_cards(ui: Ui, dash: &Dashboard, project_order: &[String]) -> View {
    let drill = ui.insights.drilldown.get();
    let p = &dash.pipeline;

    let mut stage_row = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_3()
        .child(
            view()
                .flex_row()
                .items_baseline()
                .gap_1()
                .child(note("Backlog"))
                .child(mono_val(p.backlog.to_string(), 12.)),
        )
        .child(
            view()
                .flex_row()
                .items_baseline()
                .gap_1()
                .child(note("Develop"))
                .child(
                    mono_val(p.develop.to_string(), 12.).text_color(if p.develop > 0 {
                        ACCENT
                    } else {
                        TEXT
                    }),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_baseline()
                .gap_1()
                .child(note("Review"))
                .child(mono_val(p.review.to_string(), 12.)),
        );
    if p.human_gated > 0 {
        stage_row = stage_row.child(
            view()
                .flex_row()
                .items_baseline()
                .gap_1()
                .child(note("Blocked"))
                .child(mono_val(p.human_gated.to_string(), 12.).text_color(ATTENTION)),
        );
    }
    if p.merging_now > 0 {
        stage_row = stage_row.child(
            view()
                .flex_row()
                .items_baseline()
                .gap_1()
                .child(note("Merging"))
                .child(mono_val(p.merging_now.to_string(), 12.).text_color(SUCCESS)),
        );
    }

    let mut delivery_row = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_3()
        .child(note(format!("PR {}", dash.pr_merges)))
        .child(note(format!("Local {}", dash.local_merges)))
        .child(note(format!("Manual {}", dash.manual_resolutions)));
    if let Some(zh) = dash.funnel.zero_human_pct() {
        delivery_row = delivery_row.child(
            text(format!("{zh}% zero-human"))
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(if zh >= 70 { SUCCESS } else { MUTED }),
        );
    }

    let mut sec = section_wrap()
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(heading("Cards"))
                .child(
                    row()
                        .child(
                            compact_action(
                                format!("Resolved ({})", dash.merged_total),
                                if drill == "merged" {
                                    Variant::Secondary
                                } else {
                                    Variant::Quiet
                                },
                                move |_| intent(ui, "cards-merged"),
                            )
                            .accessibility_label("Resolved Cards"),
                        )
                        .child(
                            compact_action(
                                format!("Created ({})", dash.created_total),
                                if drill == "created" {
                                    Variant::Secondary
                                } else {
                                    Variant::Quiet
                                },
                                move |_| intent(ui, "cards-created"),
                            )
                            .accessibility_label("Created Cards"),
                        ),
                ),
        )
        .child(
            view()
                .w_full()
                .pb_2p5()
                .border_b_1()
                .border_color(alpha_tint(BORDER, 0.45))
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_3()
                .child(stage_row)
                .child(delivery_row),
        );

    if !drill.is_empty() {
        sec = sec.child(render_card_drilldown(ui, dash));
    }

    if dash.range == RangeWindow::Today {
        sec = sec.child(render_timeline_1d(ui, dash, project_order));
    } else {
        sec = sec.child(render_daily_output(ui, dash));
    }

    sec
}

struct AgentRowSpec {
    name: String,
    runs: usize,
    cards: usize,
    working_ms: u64,
    max_work_ms: u64,
    pass_pct: Option<usize>,
    active_runs: usize,
    asks: usize,
    avg_run_ms: Option<u64>,
    tooltip: String,
}

fn render_agent_table_row(spec: AgentRowSpec) -> View {
    let work_ratio = if spec.working_ms > 0 && spec.max_work_ms > 0 {
        (spec.working_ms as f32 / spec.max_work_ms as f32).clamp(0.04, 1.0)
    } else {
        0.0
    };
    let (pass_txt, pass_color) = match spec.pass_pct {
        Some(pct) if pct >= 85 => (format!("{pct}%"), SUCCESS),
        Some(pct) if pct < 60 => (format!("{pct}%"), ATTENTION),
        Some(pct) => (format!("{pct}%"), TEXT),
        None if spec.active_runs > 0 => ("active".into(), ACCENT),
        None => ("—".into(), MUTED),
    };
    let avg_str = spec
        .avg_run_ms
        .map(format_duration_compact)
        .unwrap_or_else(|| "—".into());

    view()
        .w_full()
        .py_1p5()
        .px_2()
        .border_b_1()
        .border_color(alpha_tint(BORDER, 0.45))
        .hover(|s| s.bg(CARD_HOVER))
        .flex_row()
        .items_center()
        .gap_3()
        .tooltip(tooltip_box().child(note(spec.tooltip)))
        .child(
            label(spec.name)
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .flex_1()
                .min_w(px(120.))
                .truncate(),
        )
        .child(
            mono_val(spec.runs.to_string(), 12.)
                .w(px(48.))
                .flex_shrink_0(),
        )
        .child(
            mono_val(spec.cards.to_string(), 12.)
                .w(px(48.))
                .flex_shrink_0(),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(140.))
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    view()
                        .flex_1()
                        .min_w(px(56.))
                        .h(px(6.))
                        .rounded(px(3.))
                        .bg(INSET)
                        .overflow_hidden()
                        .flex_row()
                        .child(
                            view()
                                .flex_grow(work_ratio)
                                .basis(px(0.))
                                .h_full()
                                .rounded(px(3.))
                                .bg(ACCENT),
                        )
                        .child(
                            view()
                                .flex_grow((1.0 - work_ratio).max(0.0))
                                .basis(px(0.))
                                .h_full(),
                        ),
                )
                .child(
                    mono_val(format_duration_compact(spec.working_ms), 11.5)
                        .w(px(54.))
                        .flex_shrink_0(),
                ),
        )
        .child(
            text(pass_txt)
                .font_family(MONO_FONT)
                .text_size(px(12.))
                .text_color(pass_color)
                .w(px(54.))
                .flex_shrink_0(),
        )
        .child(
            mono_val(spec.asks.to_string(), 12.)
                .w(px(44.))
                .flex_shrink_0(),
        )
        .child(mono_val(avg_str, 11.5).w(px(68.)).flex_shrink_0())
}

fn render_agents(ui: Ui, dash: &Dashboard) -> View {
    let agent_group = ui.insights.agent_group.get();
    let mut panel = section_wrap().child(
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .child(heading("Agents"))
            .child(
                row()
                    .child(choice(
                        ui,
                        "Profile",
                        "group-profile",
                        agent_group == AgentGroup::Profile,
                    ))
                    .child(choice(
                        ui,
                        "Model",
                        "group-model",
                        agent_group == AgentGroup::Model,
                    )),
            ),
    );

    let first_col = match agent_group {
        AgentGroup::Profile => "Profile",
        AgentGroup::Model => "Model",
    };

    let header_row = view()
        .w_full()
        .px_2()
        .pb_1p5()
        .border_b_1()
        .border_color(BORDER)
        .flex_row()
        .items_center()
        .gap_3()
        .child(note(first_col).flex_1().min_w(px(120.)))
        .child(note("Runs").w(px(48.)).flex_shrink_0())
        .child(note("Cards").w(px(48.)).flex_shrink_0())
        .child(note("Working").flex_1().min_w(px(140.)))
        .child(note("Pass").w(px(54.)).flex_shrink_0())
        .child(note("Asks").w(px(44.)).flex_shrink_0())
        .child(note("Avg/run").w(px(68.)).flex_shrink_0());

    let mut table = view().w_full().child(header_row);

    match agent_group {
        AgentGroup::Profile => {
            let max_work = dash
                .profile_stats
                .iter()
                .map(|p| p.working_ms)
                .max()
                .unwrap_or(1)
                .max(1);
            for p in &dash.profile_stats {
                table = table.child(render_profile_agent_row(p, max_work));
            }
            if dash.profile_stats.is_empty() {
                table = table.child(view().py_2().px_2().child(note("No agent runs")));
            }
        }
        AgentGroup::Model => {
            let max_work = dash
                .model_stats
                .iter()
                .map(|m| m.working_ms)
                .max()
                .unwrap_or(1)
                .max(1);
            for m in &dash.model_stats {
                table = table.child(render_model_agent_row(m, max_work));
            }
            if dash.model_stats.is_empty() {
                table = table.child(view().py_2().px_2().child(note("No agent runs")));
            }
        }
    }

    panel = panel.child(table);
    panel
}

fn render_profile_agent_row(p: &ProfileStat, max_work: u64) -> View {
    let title = if p.profile_id == "manual" {
        "Manual".to_owned()
    } else {
        format!("@{}", p.profile_id)
    };
    let avg_run_ms = p.avg_exec_ms.or_else(|| {
        (p.total_runs > 0 && p.working_ms > 0).then(|| p.working_ms / p.total_runs as u64)
    });
    let exec_str = avg_run_ms
        .map(format_duration_compact)
        .unwrap_or_else(|| "—".into());
    let tooltip = format!(
        "{title} ({} · {}) · {} completed · {} active · {} blocked · {} failed · {} Cards · avg {exec_str}",
        p.provider,
        p.model,
        p.completed_runs,
        p.active_runs,
        p.blocked_runs,
        p.failed_runs,
        p.cards_touched
    );
    render_agent_table_row(AgentRowSpec {
        name: title,
        runs: p.total_runs,
        cards: p.cards_touched,
        working_ms: p.working_ms,
        max_work_ms: max_work,
        pass_pct: p.pass_rate_pct(),
        active_runs: p.active_runs,
        asks: p.human_asks,
        avg_run_ms,
        tooltip,
    })
}

fn render_model_agent_row(m: &ModelAgentStat, max_work: u64) -> View {
    let providers = if m.providers.is_empty() {
        "—".into()
    } else {
        m.providers.iter().cloned().collect::<Vec<_>>().join(", ")
    };
    let avg_run_ms =
        (m.total_runs > 0 && m.working_ms > 0).then(|| m.working_ms / m.total_runs as u64);
    let tooltip = format!(
        "{} ({providers} · {} profiles) · {} completed · {} active · {} blocked · {} failed · {} Cards",
        m.model,
        m.profiles.len(),
        m.completed_runs,
        m.active_runs,
        m.blocked_runs,
        m.failed_runs,
        m.cards_touched
    );
    render_agent_table_row(AgentRowSpec {
        name: m.model.clone(),
        runs: m.total_runs,
        cards: m.cards_touched,
        working_ms: m.working_ms,
        max_work_ms: max_work,
        pass_pct: m.pass_rate_pct(),
        active_runs: m.active_runs,
        asks: m.human_asks,
        avg_run_ms,
        tooltip,
    })
}

fn render_footer(ui: Ui, dash: &Dashboard) -> View {
    let intake = &dash.intake;
    let excl = dash.exclude_initial_sync;
    let live_unavailable = ui.insights.unavailable_hosts.get();
    let offline = ui.snapshot.with(|s| {
        let mut entries: Vec<String> = Vec::new();
        let mut seen = BTreeSet::new();
        for (name, reason) in &live_unavailable {
            if seen.insert(name.clone()) {
                entries.push(format!("{name} ({reason})"));
            }
        }
        for h in s.host_summaries.values().filter(|h| h.is_offline()) {
            if seen.insert(h.name.clone()) {
                entries.push(format!("{} ({})", h.name, h.connection.short_status()));
            }
        }
        entries.join(", ")
    });

    let toggle_box = nsh_base::checkbox()
        .checked(move || ui.insights.exclude_initial_sync.get())
        .on_change(move |_: &bool| intent(ui, "toggle-initial-sync"))
        .cursor_pointer()
        .px_1()
        .py_0p5()
        .rounded(px(4.))
        .hover(|s| s.bg(INSET))
        .focus(|s| s.border_1().border_color(ACCENT))
        .accessibility_label("Exclude initial sync")
        .child(mono_val(if excl { "☑" } else { "☐" }, 12.));

    let row_view = view()
        .w_full()
        .pt_3()
        .border_t_1()
        .border_color(BORDER)
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_2()
        .child(note(format!(
            "Intake  Local {} · GitHub PR {} · Issue {}",
            intake.local_manual, intake.github_pr, intake.github_issue
        )))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(note(format!(
                    "(Initial sync {} {}",
                    intake.initial_sync,
                    if excl { "excluded" } else { "included" }
                )))
                .child(toggle_box)
                .child(note(")")),
        );

    if offline.is_empty() {
        row_view
    } else {
        view()
            .gap_1p5()
            .child(row_view)
            .child(note(format!("Unavailable hosts: {offline}")))
    }
}

fn skeleton_bar(width_pct: f32, height: f32) -> View {
    view()
        .h(px(height))
        .w(Percentage(width_pct))
        .rounded(px(4.))
        .bg(BORDER)
        .opacity(0.45)
}

fn render_loading_skeleton(narrow: bool) -> View {
    let kpi_col = || {
        view()
            .flex_1()
            .min_w(px(160.))
            .py_2()
            .px_3()
            .gap_1p5()
            .child(skeleton_bar(0.48, 10.))
            .child(skeleton_bar(0.56, 24.))
            .child(skeleton_bar(0.74, 10.))
    };
    let v_sep = || {
        view()
            .w(px(1.))
            .my_2()
            .bg(BORDER)
            .opacity(0.65)
            .flex_shrink_0()
    };

    let kpi_strip = if narrow {
        view()
            .pb_3()
            .gap_1()
            .child(
                view()
                    .flex_row()
                    .child(kpi_col())
                    .child(v_sep())
                    .child(kpi_col()),
            )
            .child(
                view()
                    .flex_row()
                    .child(kpi_col())
                    .child(v_sep())
                    .child(kpi_col()),
            )
    } else {
        view()
            .pb_3()
            .flex_row()
            .child(kpi_col())
            .child(v_sep())
            .child(kpi_col())
            .child(v_sep())
            .child(kpi_col())
            .child(v_sep())
            .child(kpi_col())
    };

    let projects_skel = section_wrap()
        .child(heading("Projects"))
        .child(skeleton_bar(1.0, 8.))
        .child(
            view()
                .gap_2p5()
                .child(skeleton_bar(0.98, 22.))
                .child(skeleton_bar(0.94, 22.))
                .child(skeleton_bar(0.90, 22.)),
        );

    let machine_skel_card = || {
        view()
            .flex_1()
            .min_w(px(280.))
            .p_4()
            .rounded(px(10.))
            .bg(SURFACE)
            .gap_2p5()
            .child(skeleton_bar(0.45, 14.))
            .child(skeleton_bar(0.70, 16.))
            .child(skeleton_bar(0.85, 12.))
            .child(skeleton_bar(0.60, 14.))
    };
    let machines_skel = section_wrap().child(heading("Machines")).child(if narrow {
        view()
            .gap_3()
            .child(machine_skel_card())
            .child(machine_skel_card())
    } else {
        view()
            .flex_row()
            .gap_3()
            .child(machine_skel_card())
            .child(machine_skel_card())
    });

    let cards_skel = section_wrap()
        .child(heading("Cards"))
        .child(skeleton_bar(0.68, 14.))
        .child(skeleton_bar(1.0, CHART_HEIGHT));

    let agents_skel = section_wrap().child(heading("Agents")).child(
        view()
            .gap_2()
            .child(skeleton_bar(0.98, 18.))
            .child(skeleton_bar(0.92, 18.)),
    );

    view()
        .gap_4()
        .min_w(px(0.))
        .child(
            view()
                .h(px(24.))
                .items_end()
                .justify_center()
                .child(skeleton_bar(0.24, 12.)),
        )
        .child(kpi_strip)
        .child(projects_skel)
        .child(machines_skel)
        .child(cards_skel)
        .child(agents_skel)
}

fn merged_insights_profiles(ui: Ui) -> Vec<canban_types::Profile> {
    let mut out = ui.all_profiles.get();
    let mut seen: BTreeSet<String> = out.iter().map(|p| p.id.clone()).collect();
    let host_ids: Vec<String> = ui
        .snapshot
        .with(|s| s.host_summaries.keys().cloned().collect());
    for hid in host_ids {
        for p in crate::backend::config_for_host(&hid).profiles {
            if seen.insert(p.id.clone()) {
                out.push(p);
            }
        }
    }
    out
}

fn dashboard(ui: Ui) -> Option<Dashboard> {
    let profiles = merged_insights_profiles(ui);
    if has_backend() {
        let loaded = ui.insights.loaded_data.get()?;
        let effective_heatmap = if loaded.annual_loaded {
            ui.insights.heatmap_metric.get()
        } else {
            HeatmapMetric::Resolved
        };
        let now = ui.insights.anchor_ms.get().unwrap_or(loaded.now_ms);
        let opts = ViewOptions {
            range: loaded.range,
            now_ms: now,
            selected: loaded.selected_date,
            comparison: loaded.comparison,
            heatmap_metric: effective_heatmap,
            hourly_metric: ui.insights.hourly_metric.get(),
            agent_group: ui.insights.agent_group.get(),
            trend_days: ui.insights.trend_days.get(),
            exclude_initial_sync: ui.insights.exclude_initial_sync.get(),
        };
        return Some(ui.snapshot.with(|s| {
            model::compute_dashboard_scoped(
                s,
                &loaded.events,
                &loaded.activity,
                (!loaded.scope.is_empty()).then_some(loaded.scope.as_str()),
                (!loaded.scope_host.is_empty()).then_some(loaded.scope_host.as_str()),
                &chrono::Local,
                &profiles,
                opts,
            )
        }));
    }

    if ui.insights.preview_loading.get() {
        return None;
    }

    let scope = ui.insights.scope_project.get();
    let scope_host = ui.insights.scope_host.get();
    let range = ui.insights.range.get();
    let now = now_ms(ui);
    let opts = ViewOptions {
        range,
        now_ms: now,
        selected: ui.insights.selected_date.get(),
        comparison: ui.insights.comparison.get(),
        heatmap_metric: ui.insights.heatmap_metric.get(),
        hourly_metric: ui.insights.hourly_metric.get(),
        agent_group: ui.insights.agent_group.get(),
        trend_days: ui.insights.trend_days.get(),
        exclude_initial_sync: ui.insights.exclude_initial_sync.get(),
    };
    Some(ui.insights.activity.with(|act| {
        ui.insights.events.with(|evs| {
            ui.snapshot.with(|s| {
                model::compute_dashboard_scoped(
                    s,
                    evs,
                    act,
                    (!scope.is_empty()).then_some(scope.as_str()),
                    (!scope_host.is_empty()).then_some(scope_host.as_str()),
                    &chrono::Local,
                    &profiles,
                    opts,
                )
            })
        })
    }))
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let visit = stored((false, 0u64));
    let connected_hosts = computed(move || connected_hosts_key(&ui.hosts.get()));
    effect(move || {
        let page = ui.page.get();
        let _scope = ui.insights.scope_project.get();
        let _scope_host = ui.insights.scope_host.get();
        let _connected = connected_hosts.get();
        let _range = ui.insights.range.get();
        let _date = ui.insights.selected_date.get();
        let _cmp = ui.insights.comparison.get();
        let _hm = ui.insights.heatmap_metric.get();
        let manual = ui.insights.manual_refresh_seq.get();
        let is_insights = page == "insights";
        let enter_seq = visit.with(|(was_insights, seq)| {
            if is_insights && !*was_insights {
                *seq = seq.wrapping_add(1);
            }
            *was_insights = is_insights;
            *seq
        });
        if is_insights {
            refresh_data(ui, enter_seq.wrapping_add(manual));
        }
    });
    let dashboard = computed(move || dashboard(ui));
    let scroll_ref = element_ref();
    ui.insights.scroll.set(Some(scroll_ref));
    let scroll_key = "insights:dashboard".to_owned();
    let capture_key = scroll_key.clone();

    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .bg(BACKGROUND)
        .child(
            scroll()
                .scrollbar(false)
                .flex_1()
                .min_h(px(0.))
                .element_ref(&scroll_ref)
                .content_offset(scroll_position(ui, &scroll_key))
                .on_scroll(move |e: &ScrollEvent| capture_scroll(ui, capture_key.clone(), e))
                .child(
                    view().w_full().items_center().px_6().py_5().child(
                        view()
                            .w_full()
                            .max_w(px(1240.))
                            .min_w(px(0.))
                            .gap_4()
                            .child(dynamic(move || {
                                let narrow = ui.narrow.get();
                                let Some(dash) = dashboard.get() else {
                                    return render_loading_skeleton(narrow).into_element();
                                };
                                let project_order: Vec<String> =
                                    dash.project_stats.iter().map(|p| p.id.clone()).collect();

                                let mut body = view()
                                    .gap_5()
                                    .min_w(px(0.))
                                    .child(render_kpis(ui, &dash, narrow))
                                    .child(render_projects(ui, &dash, &project_order, narrow));

                                if !dash.host_stats.is_empty() {
                                    body = body.child(render_machines(ui, &dash, narrow));
                                }

                                body = body
                                    .child(render_cards(ui, &dash, &project_order))
                                    .child(render_agents(ui, &dash))
                                    .child(render_footer(ui, &dash));

                                view()
                                    .gap_3()
                                    .min_w(px(0.))
                                    .child(render_compare_subbar(ui, &dash))
                                    .child(body)
                                    .into_element()
                            })),
                    ),
                ),
        )
}
