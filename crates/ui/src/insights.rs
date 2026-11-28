//! Insights cockpit: Projects and Machines combo charts (dual-color Time bars + Tasks line)
//! and descending horizontal bar ranking for Agents.

pub mod model;

use crate::{
    drafts::{capture_scroll, scroll_position},
    presenter::*,
};
use canban_types::{ActivityBucket, ActivitySegment, AgentBucketStat, DwellBucketTotals};
use canban_ui_kit::{tokens::*, *};
use model::{
    ActivityFetchPlan, ActivityInput, ActivityMetric, AgentGroup, ChartPoint, Comparison,
    Dashboard, HeatmapMetric, HostStat, InsightsMetric, ProjectSort, ProjectStat, RangeWindow,
    ViewOptions, format_duration_compact,
};
use nsh_core::{
    element::{AdaptiveColor, ImageSrc, rgb},
    prelude::*,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

const ALL_PROJECTS_OPTION: &str = "__all__";
const ALL_HOSTS_OPTION: &str = "__all_hosts__";

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
    pub metric: Signal<InsightsMetric>,
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
            range: signal(RangeWindow::Days14),
            metric: signal(InsightsMetric::Time),
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
            if matches!(
                ui.insights.range.get_untracked(),
                RangeWindow::Today | RangeWindow::Days7
            ) {
                set_range(ui, RangeWindow::Days14);
            }
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
        "metric-time" => ui.insights.metric.set(InsightsMetric::Time),
        "metric-tasks" => ui.insights.metric.set(InsightsMetric::Tasks),
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
                RangeWindow::Days14,
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

const CARD_CHART_H: f32 = 68.0;
const INSIGHTS_TOOLTIP_DELAY_MS: f64 = 120.0;

fn section_wrap() -> View {
    view()
        .w_full()
        .pt_5()
        .border_t_1()
        .border_color(BORDER)
        .gap_3()
}

fn heading(title: &str) -> Text {
    label(title.to_owned())
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(14.))
}

fn insights_tip(value: impl Into<String>) -> nsh_base::TooltipElement {
    tooltip_box()
        .delay(INSIGHTS_TOOLTIP_DELAY_MS)
        .child(note(value))
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

fn palette_color(index: usize) -> AdaptiveColor {
    match index % 6 {
        0 => ACCENT.resolve(),
        1 => SUCCESS.resolve(),
        2 => ATTENTION.resolve(),
        3 => HIGHLIGHT.resolve(),
        4 => fixed_color(0x22b8cf),
        _ => fixed_color(0xe64980),
    }
}

fn host_color(index: usize, offline: bool) -> AdaptiveColor {
    if offline {
        return alpha_tint(MUTED, 0.55);
    }
    palette_color(index)
}

fn agent_time_color() -> AdaptiveColor {
    ACCENT.resolve()
}

fn me_time_color() -> AdaptiveColor {
    alpha_tint(MUTED, 0.58)
}

fn tasks_line_color() -> AdaptiveColor {
    SUCCESS.resolve()
}

fn hsla_hex(color: AdaptiveColor) -> String {
    let rgb = color.dark.to_rgb();
    format!(
        "#{:02x}{:02x}{:02x}",
        (rgb.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb.b.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn time_breakdown_inline(agent_ms: u64, dwell_ms: u64) -> View {
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .flex_shrink_0()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(
                    view()
                        .w(px(6.))
                        .h(px(6.))
                        .rounded(px(1.5))
                        .bg(agent_time_color()),
                )
                .child(
                    text(format!("Agent {}", format_duration_compact(agent_ms)))
                        .font_family(MONO_FONT)
                        .text_size(px(11.))
                        .text_color(MUTED),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(
                    view()
                        .w(px(6.))
                        .h(px(6.))
                        .rounded(px(1.5))
                        .bg(me_time_color()),
                )
                .child(
                    text(format!("Me {}", format_duration_compact(dwell_ms)))
                        .font_family(MONO_FONT)
                        .text_size(px(11.))
                        .text_color(MUTED),
                ),
        )
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
                .tooltip(insights_tip(tip))
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

/// Full-width dual-color Time stacked bars (Agent + Me) with overlaid Tasks polyline + dots.
fn render_combo_chart(points: &[ChartPoint]) -> View {
    if points.is_empty() {
        return view().w_full().h(px(CARD_CHART_H));
    }

    let n = points.len();
    let max_ms = points.iter().map(ChartPoint::total_ms).max().unwrap_or(0);
    let max_tasks = points.iter().map(|p| p.tasks).max().unwrap_or(0);

    let pad_top = 8.0f32;
    let pad_bot = 4.0f32;
    let usable_h = CARD_CHART_H - pad_top - pad_bot;

    let svg_w = 280.0f32;
    let svg_h = CARD_CHART_H;

    let task_bottom_px = |tasks: u64| -> f32 {
        if max_tasks == 0 {
            pad_bot
        } else {
            pad_bot + (tasks as f32 / max_tasks as f32).clamp(0.0, 1.0) * usable_h
        }
    };

    // Layer 1: Dual-color stacked Time bars (full width, equal columns)
    let bar_cols: Vec<AnyElement> = points
        .iter()
        .map(|pt| {
            let tot = pt.total_ms();
            let bar_el = if tot == 0 || max_ms == 0 {
                view()
                    .w_full()
                    .max_w(px(22.))
                    .h(px(2.))
                    .rounded(px(1.))
                    .bg(alpha_tint(BORDER, 0.35))
            } else {
                let h = ((tot as f32 / max_ms as f32) * usable_h).clamp(3.0, usable_h);
                let mut stack = view()
                    .w_full()
                    .max_w(px(22.))
                    .h(px(h))
                    .rounded(px(2.5))
                    .overflow_hidden();
                if pt.dwell_ms > 0 {
                    stack = stack.child(
                        view()
                            .w_full()
                            .flex_grow(pt.dwell_ms as f32)
                            .basis(px(0.))
                            .min_h(px(2.))
                            .bg(me_time_color()),
                    );
                }
                if pt.agent_ms > 0 {
                    stack = stack.child(
                        view()
                            .w_full()
                            .flex_grow(pt.agent_ms as f32)
                            .basis(px(0.))
                            .min_h(px(2.))
                            .bg(agent_time_color()),
                    );
                }
                stack
            };
            view()
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .px(px(2.))
                .pb(px(pad_bot))
                .justify_end()
                .items_center()
                .child(bar_el)
                .into_element()
        })
        .collect();

    // Layer 2: Smooth SVG polyline for Tasks (when any task exists)
    let line_overlay = if max_tasks > 0 && n >= 2 {
        use std::fmt::Write;
        let mut path_d = String::new();
        for (i, pt) in points.iter().enumerate() {
            let x = (i as f32 + 0.5) / n as f32 * svg_w;
            let y = svg_h - task_bottom_px(pt.tasks);
            let cmd = if i == 0 { 'M' } else { 'L' };
            let _ = write!(path_d, "{cmd}{x:.1} {y:.1} ");
        }
        let line_hex = hsla_hex(tasks_line_color());
        let svg_markup = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}" preserveAspectRatio="none"><path d="{path_d}" fill="none" stroke="{line_hex}" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"#
        );
        svg(ImageSrc::svg_markup(&svg_markup))
            .absolute()
            .inset_0()
            .w_full()
            .h_full()
            .pointer_events(PointerEvents::None)
            .into_element()
    } else {
        view().into_element()
    };

    // Layer 3: Crisp circular dots aligned to exact column centers + 120ms hover tooltips
    let hit_cols: Vec<AnyElement> = points
        .iter()
        .map(|pt| {
            let tot = pt.total_ms();
            let tip = format!(
                "{} · Total {} (Agent {} · Me {}) · {} {}",
                pt.label,
                format_duration_compact(tot),
                format_duration_compact(pt.agent_ms),
                format_duration_compact(pt.dwell_ms),
                pt.tasks,
                if pt.tasks == 1 { "task" } else { "tasks" }
            );
            let mut col = view()
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .relative()
                .items_center()
                .rounded(px(3.))
                .hover(|s| s.bg(alpha_tint(TEXT, 0.05)))
                .tooltip(insights_tip(tip));
            if max_tasks > 0 {
                let y_bot = (task_bottom_px(pt.tasks) - 2.5).max(0.0);
                col = col.child(
                    view()
                        .absolute()
                        .bottom(px(y_bot))
                        .w(px(5.))
                        .h(px(5.))
                        .rounded_full()
                        .bg(tasks_line_color())
                        .opacity(if pt.tasks > 0 { 1.0 } else { 0.42 })
                        .pointer_events(PointerEvents::None),
                );
            }
            col.into_element()
        })
        .collect();

    view()
        .w_full()
        .h(px(CARD_CHART_H))
        .relative()
        // Subtle top & baseline grid hairlines
        .child(
            view()
                .absolute()
                .left_0()
                .right_0()
                .top(px(pad_top))
                .h(px(1.))
                .bg(BORDER)
                .opacity(0.28)
                .pointer_events(PointerEvents::None),
        )
        .child(
            view()
                .absolute()
                .left_0()
                .right_0()
                .bottom(px(pad_bot))
                .h(px(1.))
                .bg(BORDER)
                .opacity(0.50)
                .pointer_events(PointerEvents::None),
        )
        // Bars
        .child(
            view()
                .w_full()
                .h_full()
                .flex_row()
                .items_end()
                .children(bar_cols),
        )
        // Tasks SVG line
        .child(line_overlay)
        // Dots + hover hit targets
        .child(
            view()
                .absolute()
                .inset_0()
                .flex_row()
                .items_stretch()
                .children(hit_cols),
        )
}

fn render_two_col_grid(narrow: bool, cards: Vec<AnyElement>) -> View {
    if narrow || cards.len() <= 1 {
        return view().w_full().gap_3().children(cards);
    }
    let mut rows = Vec::new();
    let mut iter = cards.into_iter();
    while let Some(a) = iter.next() {
        let row_el = if let Some(b) = iter.next() {
            view()
                .w_full()
                .flex_row()
                .items_stretch()
                .gap_3()
                .child(a)
                .child(b)
                .into_element()
        } else {
            view()
                .w_full()
                .flex_row()
                .items_stretch()
                .gap_3()
                .child(a)
                .child(view().flex_1().min_w(px(280.)))
                .into_element()
        };
        rows.push(row_el);
    }
    view().w_full().gap_3().children(rows)
}

fn render_summary_bar(ui: Ui, dash: &Dashboard) -> View {
    let t = &dash.time_totals;
    let total_ms = t.total_active_ms();
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

    let mut left = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2p5()
        .child(
            mono_val(format!("Total {}", format_duration_compact(total_ms)), 12.5)
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(time_breakdown_inline(t.agent_total_ms, t.dwell_total_ms))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(
                    view()
                        .w(px(6.))
                        .h(px(6.))
                        .rounded_full()
                        .bg(tasks_line_color()),
                )
                .child(mono_val(
                    format!(
                        "{} {}",
                        dash.scheduled_total,
                        if dash.scheduled_total == 1 {
                            "task"
                        } else {
                            "tasks"
                        }
                    ),
                    12.,
                )),
        )
        .child(note("·"))
        .child(
            text(format!("{} merged", dash.merged_total))
                .font_family(MONO_FONT)
                .text_size(px(12.))
                .text_color(if dash.merged_total > 0 {
                    SUCCESS
                } else {
                    MUTED
                }),
        );

    if let Some(msg) = notice_text {
        left = left.child(note(format!("· {msg}")));
    }
    if has_host_filter {
        left = left.child(compact_action(
            "Clear Host filter",
            Variant::Quiet,
            move |_| {
                ui.insights.scope_host.set(String::new());
            },
        ));
    }
    if has_project_filter {
        left = left.child(compact_action(
            "Clear Project filter",
            Variant::Quiet,
            move |_| {
                ui.insights.scope_project.set(String::new());
            },
        ));
    }

    let prior = match dash.range {
        RangeWindow::Days7 => "vs prior 7d",
        RangeWindow::Days14 => "vs prior 14d",
        RangeWindow::Days30 => "vs prior 30d",
        RangeWindow::Days90 => "vs prior 90d",
        RangeWindow::Today => "1D",
    };

    view()
        .w_full()
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_2()
        .child(left)
        .child(
            view()
                .tooltip(insights_tip(format!(
                    "{} · {}",
                    dash.comparison_label, dash.timezone
                )))
                .child(note(prior)),
        )
}

fn render_project_card(
    ui: Ui,
    p: &ProjectStat,
    total_all_ms: u64,
    dot_color: AdaptiveColor,
    is_selected: bool,
) -> impl IntoElement {
    let pid = p.id.clone();
    let total_ms = p.total_time_ms();
    let total_wip = p.total_wip();
    let share_pct = if total_all_ms > 0 {
        ((total_ms as u128 * 100 + total_all_ms as u128 / 2) / total_all_ms as u128) as usize
    } else {
        0
    };

    let start_lbl = p
        .chart_points
        .first()
        .map(|pt| pt.label.clone())
        .unwrap_or_default();
    let end_lbl = p
        .chart_points
        .last()
        .map(|pt| pt.label.clone())
        .unwrap_or_default();

    let card_body = view()
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
                                .bg(dot_color)
                                .flex_shrink_0(),
                        )
                        .child(
                            label(p.name.clone())
                                .text_size(px(13.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(TEXT)
                                .truncate()
                                .min_w(px(0.)),
                        )
                        .child({
                            let has_badge = p.host_badge.is_some();
                            let b = p.host_badge.clone().unwrap_or_default();
                            when(move || has_badge, move || note(b.clone()).flex_shrink_0())
                        }),
                )
                .child(
                    text(if total_all_ms > 0 {
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
        // Row 2: Compact summary stats
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_baseline()
                .gap_2p5()
                .child(
                    text(format_duration_compact(total_ms))
                        .font_family(MONO_FONT)
                        .text_size(px(14.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(TEXT),
                )
                .child(
                    text(format!(
                        "{} {}",
                        p.agent_runs,
                        if p.agent_runs == 1 { "task" } else { "tasks" }
                    ))
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .text_color(TEXT),
                )
                .child(
                    text(format!("{} merged", p.merged))
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .text_color(if p.merged > 0 { SUCCESS } else { MUTED }),
                )
                .child(
                    text(format!("{total_wip} WIP"))
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .text_color(MUTED),
                ),
        )
        // Row 3: Full-width combo chart (Dual-color Time bars + Tasks line)
        .child(render_combo_chart(&p.chart_points))
        // Row 4: Date range + Agent/Me breakdown
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    text(start_lbl)
                        .font_family(MONO_FONT)
                        .text_size(px(10.5))
                        .text_color(MUTED),
                )
                .child(time_breakdown_inline(p.agent_ms, p.dwell_ms))
                .child(
                    text(end_lbl)
                        .font_family(MONO_FONT)
                        .text_size(px(10.5))
                        .text_color(MUTED),
                ),
        );

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
        .accessibility_label(format!("Insights Project: {}", p.name))
        .on_press(move |_: &()| {
            let cur = ui.insights.scope_project.get_untracked();
            if cur == pid {
                ui.insights.scope_project.set(String::new());
            } else {
                ui.insights.scope_project.set(pid.clone());
            }
        })
        .child(card_body)
}

fn render_projects(ui: Ui, dash: &Dashboard, project_order: &[String], narrow: bool) -> View {
    let active_scope = ui.insights.scope_project.get();
    let mut projects = dash.project_stats.clone();
    projects.sort_by(|a, b| {
        b.total_time_ms()
            .cmp(&a.total_time_ms())
            .then_with(|| b.agent_runs.cmp(&a.agent_runs))
            .then_with(|| b.merged.cmp(&a.merged))
            .then_with(|| a.name.cmp(&b.name))
    });

    let other_total = dash.other_dwell_ms.saturating_add(dash.other_agent_ms);
    let mut split_segments: Vec<(String, u64, AdaptiveColor)> = projects
        .iter()
        .filter_map(|p| {
            let v = p.total_time_ms();
            (v > 0).then(|| (p.name.clone(), v, project_color(Some(&p.id), project_order)))
        })
        .collect();
    if active_scope.is_empty() && other_total > 0 {
        split_segments.push(("Other".into(), other_total, alpha_tint(MUTED, 0.55)));
    }
    let total_all_ms: u64 = split_segments.iter().map(|(_, v, _)| *v).sum();

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
                    .child(heading("Projects"))
                    .child(mono_val(projects.len().to_string(), 11.5).text_color(MUTED)),
            )
            .child(if total_all_ms > 0 {
                note(format!("Total {}", format_duration_compact(total_all_ms))).into_element()
            } else {
                view().into_element()
            }),
    );

    if !split_segments.is_empty() {
        sec = sec.child(render_proportional_bar(&split_segments));
    }

    if projects.is_empty() {
        sec = sec.child(note("No project activity"));
    } else {
        let cards: Vec<AnyElement> = projects
            .iter()
            .map(|p| {
                let is_sel = active_scope == p.id;
                let dot_color = project_color(Some(&p.id), project_order);
                render_project_card(ui, p, total_all_ms, dot_color, is_sel).into_element()
            })
            .collect();
        sec = sec.child(render_two_col_grid(narrow, cards));
    }
    sec
}

fn render_machine_card(
    ui: Ui,
    h: &HostStat,
    total_all_ms: u64,
    is_selected: bool,
) -> impl IntoElement {
    let hid = h.id.clone();
    let total_ms = h.total_time_ms();
    let share_pct = if total_all_ms > 0 {
        ((total_ms as u128 * 100 + total_all_ms as u128 / 2) / total_all_ms as u128) as usize
    } else {
        0
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

    let mut meta_row = view()
        .flex_row()
        .flex_wrap()
        .items_baseline()
        .gap_2p5()
        .child(
            text(format_duration_compact(total_ms))
                .font_family(MONO_FONT)
                .text_size(px(14.))
                .font_weight(FontWeight::BOLD)
                .text_color(TEXT),
        )
        .child(
            text(format!(
                "{} {}",
                h.agent_runs,
                if h.agent_runs == 1 { "task" } else { "tasks" }
            ))
            .font_family(MONO_FONT)
            .text_size(px(11.5))
            .text_color(TEXT),
        )
        .child(
            text(format!("{} merged", h.merged))
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(if h.merged > 0 { SUCCESS } else { MUTED }),
        )
        .child(
            text(format!("{} WIP", h.wip))
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

    let card_body = view()
        .w_full()
        .gap_2p5()
        // Row 1: Machine identity + share %
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
                    text(if total_all_ms > 0 {
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
        // Row 2: Compact summary stats
        .child(meta_row)
        // Row 3: Full-width combo chart (Dual-color Time bars + Tasks line)
        .child(render_combo_chart(&h.chart_points))
        // Row 4: Top projects + Agent/Me breakdown
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .child(note(top_projects_str).flex_1().min_w(px(0.)).truncate())
                .child(time_breakdown_inline(h.agent_ms, h.dwell_ms)),
        );

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
    let mut hosts = dash.host_stats.clone();
    hosts.sort_by(|a, b| {
        b.total_time_ms()
            .cmp(&a.total_time_ms())
            .then_with(|| b.agent_runs.cmp(&a.agent_runs))
            .then_with(|| (!a.is_local).cmp(&!b.is_local))
            .then_with(|| a.name.cmp(&b.name))
    });

    let total_all_ms: u64 = hosts.iter().map(HostStat::total_time_ms).sum();
    let online_count = hosts.iter().filter(|h| !h.offline).count();

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
                        mono_val(format!("{online_count}/{} online", hosts.len()), 11.5)
                            .text_color(MUTED),
                    ),
            )
            .child(if total_all_ms > 0 {
                note(format!("Total {}", format_duration_compact(total_all_ms))).into_element()
            } else {
                view().into_element()
            }),
    );

    if hosts.len() >= 2 && total_all_ms > 0 {
        let split: Vec<(String, u64, AdaptiveColor)> = hosts
            .iter()
            .enumerate()
            .filter_map(|(idx, h)| {
                let v = h.total_time_ms();
                (v > 0).then(|| (h.name.clone(), v, host_color(idx, h.offline)))
            })
            .collect();
        if !split.is_empty() {
            sec = sec.child(render_proportional_bar(&split));
        }
    }

    let cards: Vec<AnyElement> = hosts
        .iter()
        .map(|h| {
            let is_sel = active_host == h.id;
            render_machine_card(ui, h, total_all_ms, is_sel).into_element()
        })
        .collect();
    sec.child(render_two_col_grid(narrow, cards))
}

struct AgentRankRow {
    title: String,
    subtitle: String,
    working_ms: u64,
    tasks: usize,
    cards: usize,
    avg_task_ms: Option<u64>,
    color: AdaptiveColor,
}

fn render_agents(ui: Ui, dash: &Dashboard, narrow: bool) -> View {
    let agent_group = ui.insights.agent_group.get();

    let mut rows: Vec<AgentRankRow> = match agent_group {
        AgentGroup::Profile => dash
            .profile_stats
            .iter()
            .map(|p| {
                let title = if p.profile_id == "manual" {
                    "Manual".to_owned()
                } else {
                    format!("@{}", p.profile_id)
                };
                let avg_task_ms = p.avg_exec_ms.or_else(|| {
                    (p.total_runs > 0 && p.working_ms > 0)
                        .then(|| p.working_ms / p.total_runs as u64)
                });
                AgentRankRow {
                    title,
                    subtitle: format!("{} · {}", p.provider, p.model),
                    working_ms: p.working_ms,
                    tasks: p.total_runs,
                    cards: p.cards_touched,
                    avg_task_ms,
                    color: ACCENT.resolve(),
                }
            })
            .collect(),
        AgentGroup::Model => dash
            .model_stats
            .iter()
            .map(|m| {
                let providers = if m.providers.is_empty() {
                    "—".into()
                } else {
                    m.providers.iter().cloned().collect::<Vec<_>>().join(", ")
                };
                let avg_task_ms = (m.total_runs > 0 && m.working_ms > 0)
                    .then(|| m.working_ms / m.total_runs as u64);
                AgentRankRow {
                    title: m.model.clone(),
                    subtitle: format!(
                        "{providers} · {} {}",
                        m.profiles.len(),
                        if m.profiles.len() == 1 {
                            "profile"
                        } else {
                            "profiles"
                        }
                    ),
                    working_ms: m.working_ms,
                    tasks: m.total_runs,
                    cards: m.cards_touched,
                    avg_task_ms,
                    color: ACCENT.resolve(),
                }
            })
            .collect(),
    };

    rows.sort_by(|a, b| {
        b.working_ms
            .cmp(&a.working_ms)
            .then_with(|| b.tasks.cmp(&a.tasks))
            .then_with(|| a.title.cmp(&b.title))
    });
    for (idx, r) in rows.iter_mut().enumerate() {
        r.color = palette_color(idx);
    }

    let total_ms: u64 = rows.iter().map(|r| r.working_ms).sum();
    let max_ms: u64 = rows.iter().map(|r| r.working_ms).max().unwrap_or(0);

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
                    .child(heading("Agents"))
                    .child(mono_val(rows.len().to_string(), 11.5).text_color(MUTED)),
            )
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

    if rows.is_empty() {
        return sec.child(note("No agent tasks"));
    }

    let list_rows: Vec<AnyElement> = rows
        .into_iter()
        .map(|r| {
            let ratio = if r.working_ms == 0 || max_ms == 0 {
                0.0
            } else {
                (r.working_ms as f32 / max_ms as f32).clamp(0.03, 1.0)
            };
            let share_pct = if total_ms > 0 {
                ((r.working_ms as u128 * 100 + total_ms as u128 / 2) / total_ms as u128) as usize
            } else {
                0
            };
            let time_txt = if r.working_ms > 0 {
                format_duration_compact(r.working_ms)
            } else {
                "0m".into()
            };
            let tasks_txt = format!(
                "{} {}",
                r.tasks,
                if r.tasks == 1 { "task" } else { "tasks" }
            );
            let avg_txt = r
                .avg_task_ms
                .map(|ms| format!(" · avg {}/task", format_duration_compact(ms)))
                .unwrap_or_default();
            let tip = format!(
                "{} ({}) · {time_txt} · {tasks_txt} · {} {}{avg_txt}",
                r.title,
                r.subtitle,
                r.cards,
                if r.cards == 1 { "card" } else { "cards" },
            );

            let bar = view()
                .flex_1()
                .min_w(px(100.))
                .h(px(8.))
                .rounded(px(4.))
                .bg(INSET)
                .overflow_hidden()
                .flex_row()
                .child(
                    view()
                        .flex_grow(ratio)
                        .basis(px(0.))
                        .h_full()
                        .rounded(px(4.))
                        .bg(r.color),
                )
                .child(
                    view()
                        .flex_grow((1.0 - ratio).max(0.0))
                        .basis(px(0.))
                        .h_full(),
                );

            let mut item = view()
                .w_full()
                .py_2()
                .px_1()
                .border_b_1()
                .border_color(alpha_tint(BORDER, 0.45))
                .hover(|s| s.bg(CARD_HOVER))
                .flex_row()
                .items_center()
                .gap_3()
                .tooltip(insights_tip(tip))
                .child(
                    view()
                        .w(px(148.))
                        .flex_shrink_0()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            view()
                                .w(px(7.))
                                .h(px(7.))
                                .rounded(px(3.5))
                                .bg(r.color)
                                .flex_shrink_0(),
                        )
                        .child(
                            label(r.title)
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .flex_1()
                                .min_w(px(0.)),
                        ),
                );
            if !narrow {
                item = item.child(note(r.subtitle).w(px(172.)).flex_shrink_0().truncate());
            }
            item.child(bar)
                .child(
                    view()
                        .w(px(64.))
                        .flex_shrink_0()
                        .flex_row()
                        .justify_end()
                        .child(mono_val(time_txt, 12.).font_weight(FontWeight::MEDIUM)),
                )
                .child(
                    view()
                        .w(px(72.))
                        .flex_shrink_0()
                        .flex_row()
                        .justify_end()
                        .child(
                            text(tasks_txt)
                                .font_family(MONO_FONT)
                                .text_size(px(11.5))
                                .text_color(MUTED),
                        ),
                )
                .child(
                    view()
                        .w(px(40.))
                        .flex_shrink_0()
                        .flex_row()
                        .justify_end()
                        .child(
                            text(if total_ms > 0 {
                                format!("{share_pct}%")
                            } else {
                                "—".into()
                            })
                            .font_family(MONO_FONT)
                            .text_size(px(11.5))
                            .text_color(MUTED),
                        ),
                )
                .into_element()
        })
        .collect();

    sec = sec.child(view().w_full().children(list_rows));
    sec
}

fn render_unavailable_hosts(ui: Ui) -> Option<View> {
    let live_unavailable = ui.insights.unavailable_hosts.get();
    let offline = ui.snapshot.with_untracked(|s| {
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
    if offline.is_empty() {
        None
    } else {
        Some(
            view()
                .w_full()
                .pt_3()
                .border_t_1()
                .border_color(BORDER)
                .child(note(format!("Unavailable hosts: {offline}"))),
        )
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
    let skel_card = || {
        view()
            .flex_1()
            .min_w(px(280.))
            .p_4()
            .rounded(px(10.))
            .bg(SURFACE)
            .gap_2p5()
            .child(skeleton_bar(0.42, 14.))
            .child(skeleton_bar(0.68, 13.))
            .child(skeleton_bar(1.0, CARD_CHART_H))
            .child(skeleton_bar(0.75, 11.))
            .into_element()
    };

    let skel_section = |title: &'static str| {
        section_wrap()
            .child(heading(title))
            .child(skeleton_bar(1.0, 8.))
            .child(render_two_col_grid(narrow, vec![skel_card(), skel_card()]))
    };

    let agents_skel = section_wrap().child(heading("Agents")).child(
        view()
            .gap_2p5()
            .child(skeleton_bar(0.98, 20.))
            .child(skeleton_bar(0.86, 20.))
            .child(skeleton_bar(0.74, 20.)),
    );

    view()
        .gap_5()
        .min_w(px(0.))
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .justify_between()
                .child(skeleton_bar(0.48, 14.))
                .child(skeleton_bar(0.12, 12.)),
        )
        .child(skel_section("Projects"))
        .child(skel_section("Machines"))
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
                                    .child(render_summary_bar(ui, &dash))
                                    .child(render_projects(ui, &dash, &project_order, narrow));

                                if !dash.host_stats.is_empty() {
                                    body = body.child(render_machines(ui, &dash, narrow));
                                }

                                body = body.child(render_agents(ui, &dash, narrow));

                                if let Some(unavail) = render_unavailable_hosts(ui) {
                                    body = body.child(unavail);
                                }

                                body.into_element()
                            })),
                    ),
                ),
        )
}
