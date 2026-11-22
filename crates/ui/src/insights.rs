//! Timeline-first Insights cockpit: personal dwell and Agent working timeline,
//! KPIs, automation funnel, Project and Profile/Model Agent breakdowns,
//! 365-day activity heatmap, and always-visible timing percentiles & stage metrics.

pub mod model;

use crate::{
    drafts::{capture_scroll, scroll_position},
    presenter::*,
};
use canban_types::{ActivityBucket, ActivitySegment, AgentBucketStat, DwellBucketTotals};
use canban_ui_kit::{tokens::*, *};
use model::{
    ActivityFetchPlan, ActivityInput, ActivityMetric, AgentGroup, Comparison, Dashboard,
    DayActivityRow, DayBucket, DurationSample, HeatmapMetric, ModelAgentStat, ProfileStat,
    ProjectSort, RangeWindow, TimelineLaneKind, ViewOptions, format_duration_compact,
    format_duration_delta, percentile,
};
use nsh_core::{
    element::{AdaptiveColor, ImageSrc, rgb},
    prelude::*,
};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

const ALL_PROJECTS_OPTION: &str = "__all__";
const CHART_HEIGHT: f32 = 148.0;
const TIMING_PAGE_SIZE: usize = 10;

#[derive(Clone, PartialEq, Eq)]
pub struct ActivityCacheKey {
    pub scope: String,
    pub range: RangeWindow,
    pub selected_date: Option<chrono::NaiveDate>,
    pub comparison: Comparison,
    pub want_annual: bool,
    pub visit_seq: u64,
}

#[derive(Clone, PartialEq)]
pub struct LoadedInsights {
    pub scope: String,
    pub range: RangeWindow,
    pub selected_date: Option<chrono::NaiveDate>,
    pub comparison: Comparison,
    pub events: Rc<Vec<canban_types::Event>>,
    pub activity: Rc<ActivityInput>,
    pub annual_loaded: bool,
    pub now_ms: u64,
}

#[derive(Clone, Copy)]
pub struct State {
    /// Empty string means `All Projects`; otherwise holds a `Project.id`.
    pub scope_project: Signal<String>,
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
    pub events_key: Signal<Option<(String, u64)>>,
    pub activity: Signal<Rc<ActivityInput>>,
    pub activity_key: Signal<Option<ActivityCacheKey>>,
    pub annual_activity_key: Signal<Option<(String, String, u64)>>,
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

fn fetch_events_for_scope(scope: &str) -> Vec<canban_types::Event> {
    let parse_events = |val: serde_json::Value| -> Vec<canban_types::Event> {
        val["events"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| serde_json::from_value::<canban_types::Event>(v.clone()).ok())
                    .collect()
            })
            .unwrap_or_default()
    };
    if !scope.is_empty() {
        crate::routed_call(
            "insights.events",
            serde_json::json!({"project_id": scope}),
            Some(scope),
        )
        .map(parse_events)
        .unwrap_or_default()
    } else {
        let mut all = Vec::new();
        for (_host, res) in crate::call_all_hosts("insights.events", serde_json::json!({})) {
            if let Ok(val) = res {
                all.extend(parse_events(val));
            }
        }
        all
    }
}

fn fetch_merged_segments(from_ms: u64, to_ms: u64, scope: &str) -> Vec<ActivitySegment> {
    if to_ms <= from_ms {
        return Vec::new();
    }
    let mut params = serde_json::json!({ "from_ms": from_ms, "to_ms": to_ms });
    if !scope.is_empty() {
        params["project_id"] = serde_json::json!(scope);
    }
    let mut segments = Vec::new();
    for (_host, res) in crate::call_all_hosts("insights.activity", params) {
        if let Ok(val) = res
            && let Ok(parsed) = serde_json::from_value::<Vec<ActivitySegment>>(
                val.get("segments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            )
        {
            segments.extend(parsed);
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

fn fetch_merged_buckets(boundaries: &[u64], scope: &str) -> Vec<ActivityBucket> {
    if boundaries.len() < 2 {
        return Vec::new();
    }
    let mut params = serde_json::json!({ "boundaries": boundaries });
    if !scope.is_empty() {
        params["project_id"] = serde_json::json!(scope);
    }
    let mut per_host = Vec::new();
    for (_host, res) in crate::call_all_hosts("insights.activity_buckets", params) {
        if let Ok(val) = res
            && let Ok(parsed) = serde_json::from_value::<Vec<ActivityBucket>>(
                val.get("buckets")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            )
        {
            per_host.push(parsed);
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
    let range = ui.insights.range.get_untracked();
    let selected_date = ui.insights.selected_date.get_untracked();
    let comparison = ui.insights.comparison.get_untracked();
    let heatmap_metric = ui.insights.heatmap_metric.get_untracked();
    let want_annual = heatmap_metric != HeatmapMetric::Resolved;
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

    let events_cache_key = (scope.clone(), visit_seq);
    let need_events = ui.insights.events_key.get_untracked().as_ref() != Some(&events_cache_key);
    let key = ActivityCacheKey {
        scope: scope.clone(),
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

    let annual_cache_key = (scope.clone(), actual_today_key, visit_seq);
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
        let pending_for_req = pending_dwell.clone();
        let fetched = blocking(
            move || -> ((bool, u64), Option<Vec<canban_types::Event>>, ActivityInput) {
                let flush_res = crate::activity::flush_batch_blocking(&pending_for_req);
                let fetched_events = need_events.then(|| fetch_events_for_scope(&scope_req));
                let mut input = ActivityInput {
                    loaded: true,
                    annual_buckets: existing_annual,
                    ..Default::default()
                };
                if range == RangeWindow::Today {
                    input.period_segments =
                        fetch_merged_segments(plan.period_from, plan.period_until, &scope_req);
                    input.baseline_segments =
                        fetch_merged_segments(plan.prev_from, plan.prev_until, &scope_req);
                } else {
                    input.range_buckets = fetch_merged_buckets(&plan.range_boundaries, &scope_req);
                    input.baseline_buckets =
                        fetch_merged_buckets(&plan.baseline_boundaries, &scope_req);
                }
                if fetch_annual {
                    input.annual_buckets =
                        fetch_merged_buckets(&plan.annual_boundaries, &scope_req);
                }
                (flush_res, fetched_events, input)
            },
        )
        .await;

        let (flush_ok, flush_recorded) = match &fetched {
            Ok((res, _, _)) => *res,
            Err(_) => (false, 0),
        };
        crate::activity::complete_insights_flush(ui, pending_dwell, flush_ok, flush_recorded);

        if ui.insights.request_seq.get_untracked() != seq {
            return;
        }
        match fetched {
            Ok((_, maybe_events, input)) => {
                let events_rc = maybe_events
                    .map(Rc::new)
                    .or(existing_events)
                    .unwrap_or_else(|| Rc::new(Vec::new()));
                let activity_rc = Rc::new(input);
                let annual_loaded = want_annual || annual_cached;
                batch(move || {
                    ui.insights.events.set(events_rc.clone());
                    ui.insights.events_key.set(Some(events_cache_key));
                    ui.insights.activity.set(activity_rc.clone());
                    ui.insights.activity_key.set(Some(key));
                    if fetch_annual {
                        ui.insights.annual_activity_key.set(Some(annual_cache_key));
                    }
                    ui.insights.loaded_data.set(Some(LoadedInsights {
                        scope,
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

pub fn chrome_bar(ui: Ui) -> View {
    let scope_label = computed(move || {
        let scope = ui.insights.scope_project.get();
        if scope.is_empty() {
            return "All Projects".to_owned();
        }
        ui.snapshot.with(|s| {
            s.projects
                .iter()
                .find(|p| p.id == scope)
                .map(|p| p.name.clone())
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
                    .max_w(px(160.))
                    .truncate(),
            )
            .child(icon(Icon::ChevronDown).w(px(11.)).h(px(11.))),
        move || {
            let cur = ui.insights.scope_project.get();
            ui.snapshot.with(|s| {
                let total_merged = s
                    .cards
                    .iter()
                    .filter(|c| c.removed_at.is_none() && c.resolved())
                    .count();
                let mut opts = Vec::with_capacity(s.projects.len() + 1);
                opts.push(
                    PickerOption::new(ALL_PROJECTS_OPTION, "All Projects")
                        .detail(format!("{total_merged} resolved"))
                        .icon(Icon::Projects)
                        .selected(cur.is_empty()),
                );
                for p in &s.projects {
                    let p_merged = s
                        .cards
                        .iter()
                        .filter(|c| c.project_id == p.id && c.removed_at.is_none() && c.resolved())
                        .count();
                    opts.push(
                        PickerOption::new(p.id.clone(), p.name.clone())
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
    .width(280.);

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

fn card_surface() -> View {
    view()
        .p_4()
        .gap_3()
        .bg(SURFACE)
        .rounded(px(RADIUS))
        .border_1()
        .border_color(BORDER)
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

fn render_compare_subbar(ui: Ui, dash: &Dashboard) -> View {
    let notice_text = if dash.history_start.is_none_or(|h| h > dash.selected_date) {
        Some("No retained history for this period; unavailable counts are shown as —.")
    } else if dash
        .history_start
        .is_some_and(|h| h.to_string() > dash.window_start_key)
    {
        Some("Partial history: the selected period starts before retained records.")
    } else {
        None
    };

    let left = if let Some(msg) = notice_text {
        note(msg).into_element()
    } else if !ui.insights.scope_project.get().is_empty() {
        compact_action("Clear Project filter", Variant::Quiet, move |_| {
            ui.insights.scope_project.set(String::new());
        })
        .into_element()
    } else {
        view().into_element()
    };

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
                "Comparing against {} · {}",
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
        .child(left)
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

fn format_dur_delta(current: u64, previous: u64, baseline_known: bool, act_known: bool) -> String {
    if !baseline_known || !act_known {
        return String::new();
    }
    if current == previous {
        return "0s".into();
    }
    let diff = format_duration_compact(current.abs_diff(previous));
    if current > previous {
        format!("▲{diff}")
    } else {
        format!("▼{diff}")
    }
}

fn kpi_strip_item(
    title: &'static str,
    value: String,
    delta: String,
    delta_positive: Option<bool>,
    tip: String,
) -> View {
    let mut val_row = view().flex_row().items_baseline().gap_2().child(
        text(value)
            .font_family(MONO_FONT)
            .text_size(px(22.))
            .font_weight(FontWeight::BOLD)
            .text_color(TEXT),
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
        .min_w(px(150.))
        .py_2()
        .px_3()
        .gap_1()
        .tooltip(tooltip_box().child(note(tip)))
        .child(muted(title).text_size(px(11.5)))
        .child(val_row)
}

fn render_summary_and_kpis(ui: Ui, dash: &Dashboard, narrow: bool) -> View {
    let known = dash.history_start.is_some_and(|d| d <= dash.selected_date);
    let act_known = known
        && (dash.activity_loaded
            || dash.time_totals.dwell_total_ms > 0
            || dash.time_totals.agent_total_ms > 0);
    let t = &dash.time_totals;

    let my_time_hero = if act_known {
        format_duration_compact(t.dwell_total_ms)
    } else {
        "—".into()
    };
    let agent_hero = if act_known {
        format_duration_compact(t.agent_total_ms)
    } else {
        "—".into()
    };
    let zero_human_txt = dash
        .funnel
        .zero_human_pct()
        .map(|p| format!("{p}% zero-human"))
        .unwrap_or_else(|| "— zero-human".into());

    let summary_sentence = text(format!(
        "{}: {} resolved ({} local, {} PR) · My time {} · Agent working {} · {}",
        dash.period_label,
        if known {
            dash.merged_total.to_string()
        } else {
            "—".into()
        },
        dash.local_merges,
        dash.pr_merges,
        my_time_hero,
        agent_hero,
        zero_human_txt
    ))
    .text_size(px(13.))
    .text_color(TEXT);

    let sync_suffix = if dash.initial_sync_period > 0 && dash.exclude_initial_sync {
        format!(" (+{} sync muted)", dash.initial_sync_period)
    } else {
        String::new()
    };
    let net_sign = if dash.net_wip_delta > 0 { "+" } else { "" };
    let pace_txt = if dash.range == RangeWindow::Today {
        dash.reference_merged
            .map(|n| format!("7D pace {n:.1}/d"))
            .unwrap_or_else(|| format!("{} active WIP", dash.pipeline.active_wip))
    } else {
        format!(
            "Prev {} · {} active WIP",
            if dash.baseline_known {
                dash.prev_merged_total.to_string()
            } else {
                "—".into()
            },
            dash.pipeline.active_wip
        )
    };
    let resolved_tip = format!(
        "{} local · {} PR · {} manual · +{} organic{} · net WIP {net_sign}{} · {pace_txt} · Click to inspect resolved Cards",
        dash.local_merges,
        dash.pr_merges,
        dash.manual_resolutions,
        dash.created_total,
        sync_suffix,
        dash.net_wip_delta
    );
    let (res_delta, res_pos) = format_count_delta(
        dash.merged_total,
        dash.prev_merged_total,
        dash.baseline_known,
    );
    let resolved_active = ui.insights.drilldown.get() == "merged";
    let resolved_cell = press_target()
        .flex_1()
        .min_w(px(150.))
        .rounded(px(6.))
        .bg(if resolved_active {
            SELECTED.resolve()
        } else {
            AdaptiveColor::TRANSPARENT
        })
        .hover(|s| s.bg(CARD_HOVER))
        .accessibility_label("Resolved KPI")
        .on_press(move |_: &()| intent(ui, "cards-merged"))
        .child(kpi_strip_item(
            "Resolved",
            if known {
                dash.merged_total.to_string()
            } else {
                "—".into()
            },
            res_delta,
            (!res_pos || dash.merged_total > dash.prev_merged_total).then_some(res_pos),
            resolved_tip,
        ));

    let total_merges = dash.local_merges + dash.pr_merges;
    let prev_merges = dash.prev_local_merges + dash.prev_pr_merges;
    let (merge_delta, merge_pos) =
        format_count_delta(total_merges, prev_merges, dash.baseline_known);
    let clean_pct_str = dash
        .timings
        .clean_merge_rate_pct()
        .map(|p| format!("{p}% clean"))
        .unwrap_or_else(|| "— clean".into());
    let merge_tip = if dash.timings.merge_queue_batches > 0 {
        format!(
            "{} local · {} PR · {} queue batches · {}",
            dash.local_merges, dash.pr_merges, dash.timings.merge_queue_batches, clean_pct_str
        )
    } else {
        format!(
            "{} local · {} PR · WIP {}/{}/{} (B/D/R)",
            dash.local_merges,
            dash.pr_merges,
            dash.pipeline.backlog,
            dash.pipeline.develop,
            dash.pipeline.review
        )
    };
    let merges_cell = kpi_strip_item(
        "Merges (Local / PR)",
        if known {
            format!("{total_merges} ({}/{})", dash.local_merges, dash.pr_merges)
        } else {
            "—".into()
        },
        merge_delta,
        (total_merges != prev_merges).then_some(merge_pos),
        merge_tip,
    );

    let my_time_delta = format_pct_delta(
        t.dwell_total_ms,
        t.prev_dwell_total_ms,
        dash.baseline_known,
        act_known,
    );
    let my_time_tip = format!(
        "Card {} · Terminal {} · Other {} · Active window dwell (5m idle cutoff)",
        format_duration_compact(t.dwell_card_ms),
        format_duration_compact(t.dwell_terminal_ms),
        format_duration_compact(t.dwell_other_ms),
    );
    let my_time_cell = kpi_strip_item("My time", my_time_hero, my_time_delta, None, my_time_tip);

    let pass_str = dash
        .success_rate_pct
        .map(|p| format!("{p}% pass"))
        .unwrap_or_else(|| "No runs".into());
    let agent_delta = format_dur_delta(
        t.agent_total_ms,
        t.prev_agent_total_ms,
        dash.baseline_known,
        act_known,
    );
    let agent_tip = format!(
        "{} runs ({} completed · {} active · {} blocked · {} failed) · {} · {} profiles · {} human asks",
        dash.scheduled_total,
        dash.completed_runs,
        dash.active_runs,
        dash.blocked_runs,
        dash.failed_runs,
        pass_str,
        dash.used_profiles,
        dash.human_asks_period
    );
    let agent_cell = kpi_strip_item("Agent working", agent_hero, agent_delta, None, agent_tip);

    let v_sep = || {
        view()
            .w(px(1.))
            .my_2()
            .bg(BORDER)
            .opacity(0.65)
            .flex_shrink_0()
    };

    let strip = if narrow {
        view()
            .pb_2()
            .border_b_1()
            .border_color(BORDER)
            .gap_1()
            .child(
                view()
                    .flex_row()
                    .items_stretch()
                    .child(resolved_cell)
                    .child(v_sep())
                    .child(merges_cell),
            )
            .child(
                view()
                    .flex_row()
                    .items_stretch()
                    .child(my_time_cell)
                    .child(v_sep())
                    .child(agent_cell),
            )
    } else {
        view()
            .pb_2()
            .border_b_1()
            .border_color(BORDER)
            .flex_row()
            .items_stretch()
            .child(resolved_cell)
            .child(v_sep())
            .child(merges_cell)
            .child(v_sep())
            .child(my_time_cell)
            .child(v_sep())
            .child(agent_cell)
    };

    view().gap_2p5().child(summary_sentence).child(strip)
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

    let mut footer = view().flex_row().flex_wrap().items_center().gap_1p5();
    if dash.project_stats.is_empty() {
        footer = footer.child(note(
            "Color = Project · Hover for card & duration · Click to open",
        ));
    } else {
        footer = footer.child(note("Color = Project ("));
        for (idx, p) in dash.project_stats.iter().take(6).enumerate() {
            if idx > 0 {
                footer = footer.child(note(" "));
            }
            footer = footer.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(note(p.name.clone()))
                    .child(
                        view()
                            .w(px(8.))
                            .h(px(8.))
                            .rounded(px(2.))
                            .bg(project_color(Some(&p.id), project_order)),
                    ),
            );
        }
        footer = footer.child(note(") · Hover for card & duration · Click to open"));
    }

    let has_any = tl.lanes.iter().any(|l| !l.blocks.is_empty()) || !tl.merges.is_empty();
    let mut panel = card_surface()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(heading("Timeline"))
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
        .child(merges_row)
        .child(footer);

    if !has_any {
        panel = panel.child(note(
            "No dwell segments, Agent working runs, or merges recorded on this day yet.",
        ));
    }
    panel
}

fn render_timeline_multi_day(ui: Ui, dash: &Dashboard) -> View {
    let max_dwell = dash
        .day_rows
        .iter()
        .map(DayActivityRow::dwell_total_ms)
        .max()
        .unwrap_or(1)
        .max(1);
    let max_agent = dash
        .day_rows
        .iter()
        .map(|r| r.agent_ms)
        .max()
        .unwrap_or(1)
        .max(1);

    let rows: Vec<AnyElement> = dash
        .day_rows
        .iter()
        .rev()
        .map(|d| {
            let dwell = d.dwell_total_ms();
            let dwell_ratio = if dwell == 0 {
                0.0
            } else {
                (dwell as f32 / max_dwell as f32).clamp(0.04, 1.0)
            };
            let agent_ratio = if d.agent_ms == 0 {
                0.0
            } else {
                (d.agent_ms as f32 / max_agent as f32).clamp(0.04, 1.0)
            };
            let dwell_bar = if dwell == 0 {
                view().flex_1().h(px(8.)).rounded(px(3.)).bg(INSET)
            } else {
                let segs: [(usize, AdaptiveColor); 3] = [
                    (
                        d.dwell_card_ms.min(u64::from(u32::MAX)) as usize,
                        ACCENT.resolve(),
                    ),
                    (
                        d.dwell_terminal_ms.min(u64::from(u32::MAX)) as usize,
                        SUCCESS.resolve(),
                    ),
                    (
                        d.dwell_other_ms.min(u64::from(u32::MAX)) as usize,
                        alpha_tint(MUTED, 0.55),
                    ),
                ];
                let filled = view()
                    .flex_grow(dwell_ratio)
                    .basis(px(0.))
                    .h_full()
                    .rounded(px(3.))
                    .overflow_hidden()
                    .flex_row()
                    .gap(px(1.))
                    .children(
                        segs.into_iter()
                            .filter(|(v, _)| *v > 0)
                            .map(|(v, c)| {
                                view()
                                    .flex_grow(v as f32)
                                    .basis(px(0.))
                                    .h_full()
                                    .bg(c)
                                    .into_element()
                            })
                            .collect::<Vec<_>>(),
                    );
                view()
                    .flex_1()
                    .h(px(8.))
                    .rounded(px(3.))
                    .bg(INSET)
                    .overflow_hidden()
                    .flex_row()
                    .child(filled)
                    .child(
                        view()
                            .flex_grow((1.0 - dwell_ratio).max(0.0))
                            .basis(px(0.))
                            .h_full(),
                    )
            };

            let agent_bar = view()
                .flex_1()
                .h(px(8.))
                .rounded(px(3.))
                .bg(INSET)
                .overflow_hidden()
                .flex_row()
                .child(
                    view()
                        .flex_grow(agent_ratio)
                        .basis(px(0.))
                        .h_full()
                        .rounded(px(3.))
                        .bg(alpha_tint(SUCCESS, 0.85)),
                )
                .child(
                    view()
                        .flex_grow((1.0 - agent_ratio).max(0.0))
                        .basis(px(0.))
                        .h_full(),
                );

            let date_parsed = chrono::NaiveDate::parse_from_str(&d.date_key, "%Y-%m-%d").ok();
            let tip = format!(
                "{} ({}) · My time {} (Card {} / Terminal {} / Other {}) · Agent {} · {} resolved",
                d.date_key,
                d.weekday_label,
                format_duration_compact(dwell),
                format_duration_compact(d.dwell_card_ms),
                format_duration_compact(d.dwell_terminal_ms),
                format_duration_compact(d.dwell_other_ms),
                format_duration_compact(d.agent_ms),
                d.resolved_cards
            );

            view()
                .w_full()
                .tooltip(tooltip_box().child(note(tip)))
                .child(
                    press_target()
                        .w_full()
                        .p_2()
                        .rounded(px(5.))
                        .bg(if d.is_today {
                            SELECTED.resolve()
                        } else {
                            AdaptiveColor::TRANSPARENT
                        })
                        .hover(|s| s.bg(CARD_HOVER))
                        .on_press(move |_: &()| {
                            if let Some(date) = date_parsed {
                                select_date(ui, Some(date));
                                set_range(ui, RangeWindow::Today);
                            }
                        })
                        .child(
                            view()
                                .w_full()
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .child(
                                    mono_val(
                                        format!("{} {}", d.short_label, d.weekday_label),
                                        11.5,
                                    )
                                    .w(px(82.))
                                    .flex_shrink_0(),
                                )
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(80.))
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(dwell_bar)
                                        .child(
                                            mono_val(format_duration_compact(dwell), 11.)
                                                .w(px(54.))
                                                .flex_shrink_0(),
                                        ),
                                )
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(80.))
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(agent_bar)
                                        .child(
                                            mono_val(format_duration_compact(d.agent_ms), 11.)
                                                .w(px(54.))
                                                .flex_shrink_0(),
                                        ),
                                )
                                .child(
                                    mono_val(format!("{} res", d.resolved_cards), 11.5)
                                        .w(px(52.))
                                        .flex_shrink_0(),
                                ),
                        ),
                )
                .into_element()
        })
        .collect();

    let header_cols = view()
        .w_full()
        .px_2()
        .flex_row()
        .items_center()
        .gap_3()
        .child(note("Date").w(px(82.)).flex_shrink_0())
        .child(note("My time (Card / Terminal / Other)").flex_1())
        .child(note("Agent working").flex_1())
        .child(note("Resolved").w(px(52.)).flex_shrink_0());

    let list_body = if dash.day_rows.len() > 14 {
        scroll()
            .max_h(px(340.))
            .child(view().gap_1().children(rows))
            .into_element()
    } else {
        view().gap_1().children(rows).into_element()
    };

    card_surface()
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .gap_0p5()
                        .child(heading("Daily activity"))
                        .child(note("Click any day to inspect its 1D activity timeline")),
                )
                .child(
                    row()
                        .child(legend_swatch(ACCENT, "Card"))
                        .child(legend_swatch(SUCCESS, "Terminal"))
                        .child(legend_swatch(move || alpha_tint(MUTED, 0.55), "Other"))
                        .child(legend_swatch(
                            move || alpha_tint(SUCCESS, 0.85),
                            "Agent working",
                        )),
                ),
        )
        .child(header_cols)
        .child(list_body)
}

fn render_timeline(ui: Ui, dash: &Dashboard, project_order: &[String]) -> View {
    if dash.range == RangeWindow::Today {
        render_timeline_1d(ui, dash, project_order)
    } else {
        render_timeline_multi_day(ui, dash)
    }
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

fn hsla_hex(color: AdaptiveColor) -> String {
    let rgb = color.dark.to_rgb();
    format!(
        "#{:02x}{:02x}{:02x}",
        (rgb.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb.b.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn render_hourly_output(ui: Ui, dash: &Dashboard) -> View {
    let raw_max = dash
        .hours
        .iter()
        .filter(|h| !h.future)
        .map(|h| h.current.max(h.previous))
        .max()
        .unwrap_or(0);
    let top_tick = nice_ceil_tick(raw_max);

    let svg_w = 240.0f32;
    let svg_h = 120.0f32;
    let pad_y = 6.0f32;
    let plot_h = svg_h - pad_y * 2.0;
    let point_xy = |idx: usize, val: usize| -> (f32, f32) {
        let x = (idx as f32 + 0.5) / 24.0 * svg_w;
        let y = (svg_h - pad_y) - (val as f32 / top_tick as f32).clamp(0.0, 1.0) * plot_h;
        (x, y)
    };

    let mut base_pts = Vec::with_capacity(24);
    let mut cur_pts = Vec::with_capacity(24);
    for (idx, h) in dash.hours.iter().enumerate() {
        base_pts.push(point_xy(idx, h.previous));
        if !h.future {
            cur_pts.push(point_xy(idx, h.current));
        }
    }

    let polyline_path = |pts: &[(f32, f32)]| -> String {
        let mut d = String::new();
        for (i, &(x, y)) in pts.iter().enumerate() {
            use std::fmt::Write;
            let cmd = if i == 0 { 'M' } else { 'L' };
            let _ = write!(d, "{cmd}{x:.1} {y:.1} ");
        }
        d
    };

    let base_hex = hsla_hex(MUTED.resolve());
    let accent_hex = hsla_hex(ACCENT.resolve());
    let base_d = polyline_path(&base_pts);
    let cur_d = polyline_path(&cur_pts);
    let area_d = if let (Some(&(fx, _)), Some(&(lx, _))) = (cur_pts.first(), cur_pts.last()) {
        let y_bot = svg_h - pad_y;
        format!("{cur_d}L{lx:.1} {y_bot:.1} L{fx:.1} {y_bot:.1} Z")
    } else {
        String::new()
    };
    let end_dot = cur_pts
        .last()
        .map(|&(lx, ly)| {
            format!(r#"<circle cx="{lx:.1}" cy="{ly:.1}" r="2.8" fill="{accent_hex}"/>"#)
        })
        .unwrap_or_default();

    let svg_markup = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}" preserveAspectRatio="none"><path d="{base_d}" fill="none" stroke="{base_hex}" stroke-opacity="0.55" stroke-width="1.6" stroke-dasharray="4 3" stroke-linecap="round" stroke-linejoin="round"/><path d="{area_d}" fill="{accent_hex}" fill-opacity="0.10"/><path d="{cur_d}" fill="none" stroke="{accent_hex}" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"/>{end_dot}</svg>"#
    );

    let hover_cols = dash
        .hours
        .iter()
        .map(|h| {
            let title = format!(
                "{:02}:00 · cumulative {} / baseline {}{}",
                h.hour,
                h.current,
                h.previous,
                if h.future { " · future" } else { "" }
            );
            view()
                .flex_1()
                .min_w(px(0.))
                .h(px(CHART_HEIGHT))
                .tooltip(tooltip_box().child(note(title)))
                .into_element()
        })
        .collect::<Vec<_>>();

    let curve_frame = view()
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
                    svg(ImageSrc::svg_markup(&svg_markup))
                        .w_full()
                        .h_full()
                        .pointer_events(PointerEvents::None),
                )
                .child(view().absolute().inset_0().flex_row().children(hover_cols)),
        );

    let hourly_metric = ui.insights.hourly_metric.get();
    card_surface()
        .flex_1()
        .min_w(px(280.))
        .justify_between()
        .child(
            view()
                .gap_2p5()
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(heading("Output (cumulative)"))
                        .child(
                            row()
                                .child(legend_swatch(ACCENT, "Selected"))
                                .child(legend_swatch(move || alpha_tint(MUTED, 0.55), "Baseline")),
                        ),
                )
                .child(curve_frame)
                .child(
                    view()
                        .pl(px(28.))
                        .flex_row()
                        .justify_between()
                        .child(note("00:00"))
                        .child(note("06:00"))
                        .child(note("12:00"))
                        .child(note("18:00"))
                        .child(note("24:00")),
                ),
        )
        .child(
            row()
                .child(choice(
                    ui,
                    "Resolved",
                    "hourly-merged",
                    hourly_metric == ActivityMetric::Merged,
                ))
                .child(choice(
                    ui,
                    "Created",
                    "hourly-created",
                    hourly_metric == ActivityMetric::Created,
                ))
                .child(choice(
                    ui,
                    "Agent starts",
                    "hourly-runs",
                    hourly_metric == ActivityMetric::Runs,
                )),
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

    card_surface()
        .flex_1()
        .min_w(px(280.))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .gap_0p5()
                        .child(heading("Daily output"))
                        .child(note("Created / Resolved · click bar for 1D")),
                )
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

fn render_automation_funnel(dash: &Dashboard) -> View {
    let f = &dash.funnel;
    let peak = [f.created, f.agent_touched, f.reviewed, f.merged]
        .into_iter()
        .max()
        .unwrap_or(1)
        .max(1);

    let step_row = |name: &'static str, count: usize, color: AdaptiveColor| {
        let ratio = if count == 0 {
            0.0
        } else {
            (count as f32 / peak as f32).clamp(0.05, 1.0)
        };
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(label(name).text_size(px(12.)).w(px(64.)).truncate())
            .child(
                view()
                    .flex_1()
                    .min_w(px(48.))
                    .h(px(8.))
                    .rounded(px(3.))
                    .bg(INSET)
                    .overflow_hidden()
                    .flex_row()
                    .child(
                        view()
                            .flex_grow(ratio.max(0.0))
                            .basis(px(0.))
                            .h_full()
                            .rounded(px(3.))
                            .bg(color),
                    )
                    .child(
                        view()
                            .flex_grow((1.0 - ratio).max(0.0))
                            .basis(px(0.))
                            .h_full(),
                    ),
            )
            .child(mono_val(count.to_string(), 11.5).w(px(32.)))
    };

    let asks_line = match f.human_asks_per_resolved() {
        Some(rate) => format!("Human asks: {} · {rate:.1}/card", f.human_asks),
        None => format!("Human asks: {} · —/card", f.human_asks),
    };

    let (zh_val, zh_color) = match (f.zero_human_pct(), f.prev_zero_human_pct()) {
        (Some(cur), Some(prev)) => {
            let diff = cur as isize - prev as isize;
            if diff > 0 {
                (format!("{cur}% ▲{diff}pt"), SUCCESS)
            } else if diff < 0 {
                (format!("{cur}% ▼{}pt", -diff), ATTENTION)
            } else {
                (format!("{cur}% 0pt"), MUTED)
            }
        }
        (Some(cur), None) => (format!("{cur}%"), TEXT),
        _ => ("—".into(), MUTED),
    };

    card_surface()
        .flex_1()
        .min_w(px(260.))
        .justify_between()
        .child(
            view().gap_3().child(heading("Automation")).child(
                view()
                    .gap_2()
                    .child(step_row("Created", f.created, alpha_tint(ACCENT, 0.55)))
                    .child(step_row("Agent", f.agent_touched, ACCENT.resolve()))
                    .child(step_row("Review", f.reviewed, ATTENTION.resolve()))
                    .child(step_row("Merged", f.merged, SUCCESS.resolve())),
            ),
        )
        .child(
            view()
                .pt_2p5()
                .border_t_1()
                .border_color(BORDER)
                .gap_1()
                .child(note(asks_line))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(note("Zero-human completion:"))
                        .child(
                            text(zh_val)
                                .font_family(MONO_FONT)
                                .text_size(px(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(zh_color),
                        ),
                ),
        )
}

fn project_sort_label(sort: ProjectSort) -> &'static str {
    match sort {
        ProjectSort::Time => "Time",
        ProjectSort::Merged => "Merged",
        ProjectSort::Created => "Created",
        ProjectSort::Wip => "WIP",
        ProjectSort::Runs => "Runs",
    }
}

fn render_projects(ui: Ui, dash: &Dashboard, project_order: &[String]) -> View {
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

    let max_metric: u64 = projects
        .iter()
        .map(|p| match sort_mode {
            ProjectSort::Time => p.total_time_ms(),
            ProjectSort::Merged => p.merged as u64,
            ProjectSort::Created => p.created as u64,
            ProjectSort::Wip => p.total_wip() as u64,
            ProjectSort::Runs => p.agent_runs as u64,
        })
        .max()
        .unwrap_or(1)
        .max(1);

    let mut rows: Vec<AnyElement> = projects
        .into_iter()
        .map(|p| {
            let pid = p.id.clone();
            let is_scoped = active_scope == pid;
            let dot_color = project_color(Some(&p.id), project_order);
            let total_wip = p.total_wip();

            let (ratio, fill_color) = match sort_mode {
                ProjectSort::Time => {
                    let tot = p.total_time_ms();
                    let r = if tot == 0 {
                        0.0
                    } else {
                        (tot as f32 / max_metric as f32).clamp(0.05, 1.0)
                    };
                    (r, ACCENT.resolve())
                }
                ProjectSort::Merged => (
                    if p.merged == 0 {
                        0.0
                    } else {
                        (p.merged as f32 / max_metric as f32).clamp(0.05, 1.0)
                    },
                    SUCCESS.resolve(),
                ),
                ProjectSort::Created => (
                    if p.created == 0 {
                        0.0
                    } else {
                        (p.created as f32 / max_metric as f32).clamp(0.05, 1.0)
                    },
                    ACCENT.resolve(),
                ),
                ProjectSort::Wip => (
                    if total_wip == 0 {
                        0.0
                    } else {
                        (total_wip as f32 / max_metric as f32).clamp(0.05, 1.0)
                    },
                    ATTENTION.resolve(),
                ),
                ProjectSort::Runs => (
                    if p.agent_runs == 0 {
                        0.0
                    } else {
                        (p.agent_runs as f32 / max_metric as f32).clamp(0.05, 1.0)
                    },
                    alpha_tint(ACCENT, 0.8),
                ),
            };

            let bar_track = view()
                .w(px(64.))
                .flex_shrink_0()
                .h(px(6.))
                .rounded(px(3.))
                .bg(INSET)
                .overflow_hidden()
                .flex_row()
                .child(
                    view()
                        .flex_grow(ratio.max(0.0))
                        .basis(px(0.))
                        .h_full()
                        .rounded(px(3.))
                        .bg(fill_color),
                )
                .child(
                    view()
                        .flex_grow((1.0 - ratio).max(0.0))
                        .basis(px(0.))
                        .h_full(),
                );

            let right_sub = match sort_mode {
                ProjectSort::Time | ProjectSort::Merged => {
                    format!(
                        "{} {}",
                        p.merged,
                        if p.merged == 1 { "card" } else { "cards" }
                    )
                }
                ProjectSort::Created => format!("+{} created", p.created),
                ProjectSort::Wip => format!("{total_wip} wip"),
                ProjectSort::Runs => format!("{} runs", p.agent_runs),
            };

            let tip = format!(
                "{} · Me {} · Agent {} · {} resolved · +{} created · WIP {total_wip} · {} runs",
                p.name,
                format_duration_compact(p.dwell_ms),
                format_duration_compact(p.agent_ms),
                p.merged,
                p.created,
                p.agent_runs
            );

            view()
                .w_full()
                .tooltip(tooltip_box().child(note(tip)))
                .child(
                    press_target()
                        .w_full()
                        .px_2()
                        .py_1p5()
                        .rounded(px(6.))
                        .bg(if is_scoped {
                            SELECTED.resolve()
                        } else {
                            AdaptiveColor::TRANSPARENT
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
                        .child(
                            view()
                                .w_full()
                                .gap_0p5()
                                .child(
                                    view()
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
                                                .gap_1p5()
                                                .child(
                                                    view()
                                                        .w(px(7.))
                                                        .h(px(7.))
                                                        .rounded(px(3.5))
                                                        .bg(dot_color)
                                                        .flex_shrink_0(),
                                                )
                                                .child(
                                                    label(p.name.clone())
                                                        .text_size(px(12.5))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .truncate()
                                                        .min_w(px(0.)),
                                                ),
                                        )
                                        .child(
                                            view()
                                                .flex_row()
                                                .items_center()
                                                .gap_2()
                                                .flex_shrink_0()
                                                .child(note(format!(
                                                    "Me {}",
                                                    format_duration_compact(p.dwell_ms)
                                                )))
                                                .child(bar_track),
                                        ),
                                )
                                .child(
                                    view()
                                        .pl(px(13.))
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .child(note(format!(
                                            "Agent {}",
                                            format_duration_compact(p.agent_ms)
                                        )))
                                        .child(note(right_sub)),
                                ),
                        ),
                )
                .into_element()
        })
        .collect();

    if active_scope.is_empty() && (dash.other_dwell_ms > 0 || dash.other_agent_ms > 0) {
        rows.push(
            view()
                .w_full()
                .px_2()
                .py_1()
                .flex_row()
                .items_center()
                .justify_between()
                .child(note("Other"))
                .child(note(format!(
                    "Me {} · Agent {}",
                    format_duration_compact(dash.other_dwell_ms),
                    format_duration_compact(dash.other_agent_ms)
                )))
                .into_element(),
        );
    }

    let sort_open = signal(false);
    let sort_options = [
        (ProjectSort::Time, "Time", "sort-time"),
        (ProjectSort::Merged, "Merged", "sort-merged"),
        (ProjectSort::Created, "Created", "sort-created"),
        (ProjectSort::Wip, "WIP", "sort-wip"),
        (ProjectSort::Runs, "Runs", "sort-runs"),
    ];
    let sort_popover = nsh_base::popover()
        .open(move || Some(sort_open.get()))
        .placement(Placement::Top)
        .align(Align::Start)
        .offset(px(4.))
        .margin(px(8.))
        .popup_class(canban_ui_kit::motion::POPUP_CLASS)
        .popup_starting_class(canban_ui_kit::motion::POPUP_STARTING_CLASS)
        .popup_ending_class(canban_ui_kit::motion::POPUP_ENDING_CLASS)
        .on_open_change(move |e: &nsh_base::PopoverEvent| {
            if let nsh_base::PopoverEvent::OpenChange(next, _) = e {
                sort_open.set(*next);
            }
        })
        .trigger(
            press_target()
                .h(px(24.))
                .px_2()
                .rounded(px(5.))
                .bg(INSET)
                .border_1()
                .border_color(BORDER)
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.border_color(ACCENT))
                .flex_row()
                .items_center()
                .gap_1p5()
                .accessibility_label(format!("Sort Projects: {}", project_sort_label(sort_mode)))
                .child(note(format!("Sort: {}", project_sort_label(sort_mode))))
                .child(icon(Icon::ChevronDown).w(px(11.)).h(px(11.))),
        )
        .child(
            view()
                .w(px(136.))
                .p_1p5()
                .gap_0p5()
                .bg(SURFACE)
                .rounded(px(8.))
                .border_1()
                .border_color(BORDER)
                .shadow_lg()
                .children(
                    sort_options
                        .into_iter()
                        .map(|(mode, title, op)| {
                            let is_sel = sort_mode == mode;
                            press_target()
                                .w_full()
                                .px_2()
                                .py_1()
                                .rounded(px(5.))
                                .bg(if is_sel {
                                    SELECTED.resolve()
                                } else {
                                    AdaptiveColor::TRANSPARENT
                                })
                                .hover(|s| s.bg(CARD_HOVER))
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .accessibility_label(title)
                                .on_press(move |_: &()| {
                                    intent(ui, op);
                                    sort_open.set(false);
                                })
                                .child(label(title).text_size(px(12.)))
                                .child(if is_sel {
                                    icon(Icon::Check).w(px(12.)).h(px(12.)).into_element()
                                } else {
                                    view().into_element()
                                })
                                .into_element()
                        })
                        .collect::<Vec<_>>(),
                ),
        );

    let mut body = view().gap_1p5().child(heading("Projects"));
    if rows.is_empty() {
        body = body.child(note("No Project activity or active WIP in this period."));
    } else {
        body = body.children(rows);
    }

    card_surface()
        .flex_1()
        .min_w(px(260.))
        .justify_between()
        .child(body)
        .child(view().pt_1().items_start().child(sort_popover))
}

struct AgentRowSpec {
    name: String,
    runs: usize,
    working_ms: u64,
    max_work_ms: u64,
    pass_pct: Option<usize>,
    active_runs: usize,
    asks: usize,
    queue_p50_ms: Option<u64>,
    prev_queue_p50_ms: Option<u64>,
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
    let q_str = spec
        .queue_p50_ms
        .map(format_duration_compact)
        .unwrap_or_else(|| "—".into());
    let q_delta = format_duration_delta(spec.queue_p50_ms, spec.prev_queue_p50_ms)
        .map(|(txt, _)| format!(" {txt}"))
        .unwrap_or_default();

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
        .child(
            mono_val(format!("{q_str}{q_delta}"), 11.5)
                .w(px(130.))
                .flex_shrink_0(),
        )
}

fn render_agents(ui: Ui, dash: &Dashboard) -> View {
    let agent_group = ui.insights.agent_group.get();
    let mut panel = card_surface().child(
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .child(heading("Agents"))
            .child(
                row()
                    .child(note("Group:"))
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
        .pb_1()
        .border_b_1()
        .border_color(BORDER)
        .flex_row()
        .items_center()
        .gap_3()
        .child(note(first_col).flex_1().min_w(px(120.)))
        .child(note("Runs").w(px(48.)).flex_shrink_0())
        .child(note("Working").flex_1().min_w(px(140.)))
        .child(note("Pass").w(px(54.)).flex_shrink_0())
        .child(note("Asks").w(px(44.)).flex_shrink_0())
        .child(note("Queue p50").w(px(130.)).flex_shrink_0());

    panel = panel.child(header_row);

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
                panel = panel.child(render_profile_agent_row(p, max_work));
            }
            if dash.profile_stats.is_empty() {
                panel = panel.child(note(
                    "No Agent execution or working time observed in this period.",
                ));
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
                panel = panel.child(render_model_agent_row(m, max_work));
            }
            if dash.model_stats.is_empty() {
                panel = panel.child(note(
                    "No Agent execution or working time observed in this period.",
                ));
            }
        }
    }

    panel
}

fn render_profile_agent_row(p: &ProfileStat, max_work: u64) -> View {
    let title = if p.profile_id == "manual" {
        "Manual".to_owned()
    } else {
        format!("@{}", p.profile_id)
    };
    let exec_str = p
        .avg_exec_ms
        .map(format_duration_compact)
        .unwrap_or_else(|| "—".into());
    let reply_suffix = p
        .avg_mention_reply_ms
        .map(|ms| format!(" · reply avg {}", format_duration_compact(ms)))
        .unwrap_or_default();
    let tooltip = format!(
        "{title} ({} · {}) · {} completed · {} active · {} blocked · {} failed · {} Cards · exec avg {exec_str}{reply_suffix}",
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
        working_ms: p.working_ms,
        max_work_ms: max_work,
        pass_pct: p.pass_rate_pct(),
        active_runs: p.active_runs,
        asks: p.human_asks,
        queue_p50_ms: p.median_queue_wait_ms,
        prev_queue_p50_ms: p.prev_median_queue_wait_ms,
        tooltip,
    })
}

fn render_model_agent_row(m: &ModelAgentStat, max_work: u64) -> View {
    let providers = if m.providers.is_empty() {
        "—".into()
    } else {
        m.providers.iter().cloned().collect::<Vec<_>>().join(", ")
    };
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
        working_ms: m.working_ms,
        max_work_ms: max_work,
        pass_pct: m.pass_rate_pct(),
        active_runs: m.active_runs,
        asks: m.human_asks,
        queue_p50_ms: m.median_queue_wait_ms,
        prev_queue_p50_ms: m.prev_median_queue_wait_ms,
        tooltip,
    })
}

fn heatmap_level_bg(level: u8) -> AdaptiveColor {
    match level {
        0 => INSET.resolve(),
        1 => alpha_tint(SUCCESS, 0.28),
        2 => alpha_tint(SUCCESS, 0.50),
        3 => alpha_tint(SUCCESS, 0.75),
        _ => SUCCESS.resolve(),
    }
}

fn render_heatmap_panel(ui: Ui, dash: &Dashboard) -> View {
    let cell_size = 11.;
    let gap = 2.5;
    let headers = dash
        .heatmap_weeks
        .iter()
        .map(|w| {
            view()
                .w(px(cell_size))
                .h(px(14.))
                .flex_shrink_0()
                .child(
                    note(w.month_header.clone().unwrap_or_default())
                        .w(px(30.))
                        .flex_shrink_0()
                        .text_size(px(10.)),
                )
                .into_element()
        })
        .collect::<Vec<_>>();
    let columns = dash
        .heatmap_weeks
        .iter()
        .map(|w| {
            view()
                .gap(px(gap))
                .children(
                    w.cells
                        .iter()
                        .map(|c| {
                            if c.is_future {
                                return view().w(px(cell_size)).h(px(cell_size)).into_element();
                            }
                            let date =
                                chrono::NaiveDate::parse_from_str(&c.date_key, "%Y-%m-%d").unwrap();
                            let title = if c.known {
                                format!(
                                    "{} · {} resolved · My time {} · Agent {} · {} runs",
                                    c.date_key,
                                    c.merged_cards,
                                    format_duration_compact(c.dwell_ms),
                                    format_duration_compact(c.agent_ms),
                                    c.total_runs
                                )
                            } else {
                                format!("{} · no retained history", c.date_key)
                            };
                            view()
                                .w(px(cell_size))
                                .h(px(cell_size))
                                .tooltip(tooltip_box().child(note(title.clone())))
                                .child(
                                    press_target()
                                        .w(px(cell_size))
                                        .h(px(cell_size))
                                        .min_w(px(cell_size))
                                        .min_h(px(cell_size))
                                        .rounded(px(2.))
                                        .bg(heatmap_level_bg(c.level))
                                        .border_1()
                                        .border_color(if c.is_today {
                                            TEXT
                                        } else if c.in_range {
                                            ACCENT
                                        } else {
                                            CLEAR
                                        })
                                        .opacity(if c.known { 1. } else { 0.25 })
                                        .hover(|s| s.border_color(TEXT))
                                        .accessibility_label(title)
                                        .on_press(move |_: &()| {
                                            select_date(ui, Some(date));
                                            set_range(ui, RangeWindow::Today);
                                        }),
                                )
                                .into_element()
                        })
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
        .collect::<Vec<_>>();
    let grid_height = 14.0 + 4.0 + 7.0 * cell_size + 6.0 * gap;
    let grid = view()
        .min_w(px(dash.heatmap_weeks.len() as f32 * (cell_size + gap) + 28.))
        .h(px(grid_height))
        .gap(px(4.))
        .child(view().flex_row().pl(px(28.)).gap(px(gap)).children(headers))
        .child(
            view()
                .flex_row()
                .child(
                    view().w(px(28.)).gap(px(gap)).children(
                        ["Mon", "", "Wed", "", "Fri", "", ""]
                            .into_iter()
                            .map(|d| {
                                view()
                                    .h(px(cell_size))
                                    .justify_center()
                                    .child(note(d).text_size(px(9.)))
                                    .into_element()
                            })
                            .collect::<Vec<_>>(),
                    ),
                )
                .child(view().flex_row().gap(px(gap)).children(columns)),
        );

    let hm_metric = ui.insights.heatmap_metric.get();
    let hm_spinner = view()
        .w(px(14.))
        .h(px(14.))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .child(dynamic(move || {
            if ui.insights.heatmap_loading.get() {
                loading_spinner(11., MUTED).into_element()
            } else {
                view().w(px(11.)).h(px(11.)).into_element()
            }
        }));

    card_surface()
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(heading("Past year"))
                        .child(
                            row()
                                .child(choice(
                                    ui,
                                    "Resolved",
                                    "metric-merged",
                                    hm_metric == HeatmapMetric::Resolved,
                                ))
                                .child(choice(
                                    ui,
                                    "My time",
                                    "metric-dwell",
                                    hm_metric == HeatmapMetric::Dwell,
                                ))
                                .child(choice(
                                    ui,
                                    "Agent time",
                                    "metric-agent",
                                    hm_metric == HeatmapMetric::Agent,
                                )),
                        )
                        .child(hm_spinner),
                )
                .child(note(format!(
                    "Streak {}d · Longest {}d",
                    dash.current_streak, dash.longest_streak
                ))),
        )
        .child(
            scroll()
                .horizontal()
                .w_full()
                .h(px(grid_height + 2.0))
                .content_offset(Point::new(px(10_000.), px(0.)))
                .child(grid),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(note(format!(
                    "{} active days · Click a tile to inspect",
                    dash.active_days
                )))
                .child(
                    row()
                        .child(note("Less"))
                        .children(
                            (0..=4)
                                .map(|n| {
                                    view()
                                        .w(px(9.))
                                        .h(px(9.))
                                        .rounded(px(2.))
                                        .bg(heatmap_level_bg(n))
                                        .into_element()
                                })
                                .collect::<Vec<_>>(),
                        )
                        .child(note("More")),
                ),
        )
}

fn render_percentile_row(
    ui: Ui,
    title: &'static str,
    samples: &[DurationSample],
    prev_samples: &[DurationSample],
    tab_id: &'static str,
    op: &'static str,
    active_tab: &str,
) -> View {
    let p50 = percentile(samples, 50);
    let p90 = percentile(samples, 90);
    let prev_p50 = percentile(prev_samples, 50);
    let p50_txt = p50
        .map(format_duration_compact)
        .unwrap_or_else(|| "—".into());
    let p90_txt = p90
        .map(format_duration_compact)
        .unwrap_or_else(|| "—".into());
    let prev_txt = prev_p50
        .map(format_duration_compact)
        .unwrap_or_else(|| "—".into());
    let (baseline_txt, baseline_color) = match format_duration_delta(p50, prev_p50) {
        Some((delta_txt, faster)) => (
            format!("{prev_txt} ({delta_txt})"),
            if faster { SUCCESS } else { MUTED },
        ),
        None => (prev_txt, MUTED),
    };
    let is_open = active_tab == tab_id;
    let inspect_label = if is_open { "Hide ▾" } else { "Inspect ▸" };

    view()
        .w_full()
        .py_1p5()
        .px_2()
        .border_b_1()
        .border_color(alpha_tint(BORDER, 0.45))
        .flex_row()
        .items_center()
        .gap_3()
        .child(
            label(title)
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .flex_1()
                .min_w(px(150.)),
        )
        .child(mono_val(p50_txt, 12.).w(px(64.)).flex_shrink_0())
        .child(mono_val(p90_txt, 12.).w(px(64.)).flex_shrink_0())
        .child(
            text(baseline_txt)
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(baseline_color)
                .w(px(148.))
                .flex_shrink_0(),
        )
        .child(
            mono_val(samples.len().to_string(), 11.5)
                .w(px(58.))
                .flex_shrink_0(),
        )
        .child(
            view().w(px(88.)).flex_shrink_0().items_end().child(
                compact_action(
                    inspect_label,
                    if is_open {
                        Variant::Secondary
                    } else {
                        Variant::Quiet
                    },
                    move |_| intent(ui, op),
                )
                .accessibility_label(format!("{inspect_label} {title}")),
            ),
        )
}

fn render_timings(ui: Ui, dash: &Dashboard) -> View {
    let tab = ui.insights.timing_tab.get();
    let samples: &[DurationSample] = match tab.as_str() {
        "cycle" => &dash.efficiency.cycle,
        "queue" => &dash.efficiency.queue,
        _ => &[],
    };

    let mut panel = card_surface().child(heading("Timings")).child(
        view()
            .gap_0p5()
            .child(
                view()
                    .px_2()
                    .pb_1()
                    .border_b_1()
                    .border_color(BORDER)
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(note("Metric").flex_1().min_w(px(150.)))
                    .child(note("p50").w(px(64.)).flex_shrink_0())
                    .child(note("p90").w(px(64.)).flex_shrink_0())
                    .child(note("vs Baseline").w(px(148.)).flex_shrink_0())
                    .child(note("Samples").w(px(58.)).flex_shrink_0())
                    .child(view().w(px(88.)).flex_shrink_0()),
            )
            .child(render_percentile_row(
                ui,
                "Execution → resolution",
                &dash.efficiency.cycle,
                &dash.efficiency.previous_cycle,
                "cycle",
                "timing-cycle",
                &tab,
            ))
            .child(render_percentile_row(
                ui,
                "Queue → start",
                &dash.efficiency.queue,
                &dash.efficiency.previous_queue,
                "queue",
                "timing-queue",
                &tab,
            )),
    );

    if !tab.is_empty() {
        let total = samples.len();
        let total_pages = total.div_ceil(TIMING_PAGE_SIZE).max(1);
        let page = ui.insights.timing_page.get().min(total_pages - 1);
        let start = page * TIMING_PAGE_SIZE;
        let slice = &samples[start..total.min(start + TIMING_PAGE_SIZE)];

        let mut drill = view().p_2p5().gap_1p5().rounded(px(6.)).bg(INSET).child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(note(format!(
                    "{} · slowest-first ({} samples)",
                    if tab == "cycle" {
                        "Execution → resolution"
                    } else {
                        "Queue → start"
                    },
                    total
                )))
                .child(
                    row()
                        .child(
                            compact_action("Prev", Variant::Quiet, move |_| {
                                intent(ui, "timing-prev")
                            })
                            .disabled(move || page == 0),
                        )
                        .child(mono_val(format!("{}/{total_pages}", page + 1), 11.))
                        .child(
                            compact_action("Next", Variant::Quiet, move |_| {
                                intent(ui, "timing-next")
                            })
                            .disabled(move || page + 1 >= total_pages),
                        ),
                ),
        );

        if slice.is_empty() {
            drill = drill.child(note("No samples in this period."));
        } else {
            for s in slice {
                let cid = s.card_id.clone();
                let profile_str = s
                    .profile
                    .as_deref()
                    .map(|p| format!(" · @{p}"))
                    .unwrap_or_default();
                let row_view = view()
                    .w_full()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        label(if s.number > 0 {
                            format!("#{} {}", s.number, s.title)
                        } else {
                            s.title.clone()
                        })
                        .text_size(px(12.))
                        .truncate(),
                    )
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(note(format!("{}{}", s.project, profile_str)))
                            .child(mono_val(format_duration_compact(s.duration), 11.5)),
                    );
                if cid.is_empty() {
                    drill = drill.child(view().px_2().py_1().child(row_view));
                } else {
                    drill = drill.child(
                        press_target()
                            .w_full()
                            .px_2()
                            .py_1()
                            .rounded(px(4.))
                            .hover(|st| st.bg(CARD_HOVER))
                            .on_press(move |_: &()| ui.open_card(cid.clone()))
                            .child(row_view),
                    );
                }
            }
        }
        panel = panel.child(drill);
    }

    panel
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
    let mut panel = card_surface().child(
        view()
            .flex_row()
            .items_center()
            .justify_between()
            .child(heading(&format!("{} {} Cards", ids.len(), label_kind)))
            .child(compact_action(
                "Close Card list",
                Variant::Quiet,
                move |_| intent(ui, "cards-close"),
            )),
    );
    for id in ids.iter().take(100) {
        if let Some(card) = ui.index.with(|idx| idx.cards.get(id).cloned()) {
            let id = id.clone();
            panel = panel.child(
                press_target()
                    .w_full()
                    .p_2()
                    .rounded(px(5.))
                    .hover(|s| s.bg(CARD_HOVER))
                    .on_press(move |_: &()| ui.open_card(id.clone()))
                    .child(label(format!("#{} {}", card.number, card.title)).truncate()),
            );
        }
    }
    if ids.len() > 100 {
        panel = panel.child(note(
            "Showing the first 100 Cards. Select a shorter range or a Project to narrow the list.",
        ));
    }
    panel
}

fn render_footer(ui: Ui, dash: &Dashboard) -> View {
    let intake = &dash.intake;
    let excl = dash.exclude_initial_sync;
    let offline = ui.snapshot.with(|s| {
        s.host_summaries
            .values()
            .filter(|h| h.is_offline())
            .map(|h| {
                format!(
                    "{} (last sync: {})",
                    h.name,
                    h.last_synced_at
                        .map(|t| {
                            chrono::DateTime::from_timestamp_millis(t as i64)
                                .unwrap()
                                .with_timezone(&chrono::Local)
                                .format("%m-%d %H:%M")
                                .to_string()
                        })
                        .unwrap_or_else(|| "unknown".into())
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
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
        .pt_2()
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
        view().gap_1p5().child(row_view).child(note(format!(
            "Offline hosts: {offline}; cached data may be stale."
        )))
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
            .min_w(px(150.))
            .py_2()
            .px_3()
            .gap_1p5()
            .child(skeleton_bar(0.45, 11.))
            .child(skeleton_bar(0.60, 24.))
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
            .pb_2()
            .border_b_1()
            .border_color(BORDER)
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
            .pb_2()
            .border_b_1()
            .border_color(BORDER)
            .flex_row()
            .child(kpi_col())
            .child(v_sep())
            .child(kpi_col())
            .child(v_sep())
            .child(kpi_col())
            .child(v_sep())
            .child(kpi_col())
    };

    let timeline_skel = card_surface()
        .h(px(220.))
        .justify_between()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(heading("Timeline"))
                .child(skeleton_bar(0.10, 20.)),
        )
        .child(
            view()
                .gap_2()
                .child(skeleton_bar(0.96, 18.))
                .child(skeleton_bar(0.92, 18.))
                .child(skeleton_bar(0.88, 18.))
                .child(skeleton_bar(0.95, 18.))
                .child(skeleton_bar(0.84, 18.)),
        )
        .child(skeleton_bar(0.48, 12.));

    let overview_a = card_surface()
        .flex_1()
        .min_w(px(260.))
        .h(px(248.))
        .justify_between()
        .child(heading("Output (cumulative)"))
        .child(skeleton_bar(1.0, CHART_HEIGHT))
        .child(skeleton_bar(0.65, 22.));
    let overview_b = card_surface()
        .flex_1()
        .min_w(px(260.))
        .h(px(248.))
        .justify_between()
        .child(heading("Automation"))
        .child(
            view()
                .gap_2p5()
                .child(skeleton_bar(0.95, 12.))
                .child(skeleton_bar(0.85, 12.))
                .child(skeleton_bar(0.72, 12.))
                .child(skeleton_bar(0.64, 12.)),
        )
        .child(
            view()
                .gap_1p5()
                .child(skeleton_bar(0.62, 12.))
                .child(skeleton_bar(0.76, 12.)),
        );
    let overview_c = card_surface()
        .flex_1()
        .min_w(px(260.))
        .h(px(248.))
        .justify_between()
        .child(heading("Projects"))
        .child(
            view()
                .gap_3()
                .child(skeleton_bar(0.94, 28.))
                .child(skeleton_bar(0.86, 28.))
                .child(skeleton_bar(0.55, 14.)),
        )
        .child(skeleton_bar(0.36, 22.));

    let agents_skel = card_surface()
        .h(px(176.))
        .justify_between()
        .child(heading("Agents"))
        .child(
            view()
                .gap_2p5()
                .child(skeleton_bar(1.0, 18.))
                .child(skeleton_bar(0.96, 18.))
                .child(skeleton_bar(0.90, 18.)),
        );

    let heatmap_skel = card_surface()
        .h(px(188.))
        .justify_between()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(heading("Past year"))
                .child(skeleton_bar(0.24, 14.)),
        )
        .child(skeleton_bar(1.0, 110.))
        .child(skeleton_bar(0.32, 12.));

    let timings_skel = card_surface()
        .h(px(124.))
        .justify_between()
        .child(heading("Timings"))
        .child(
            view()
                .gap_2()
                .child(skeleton_bar(0.98, 22.))
                .child(skeleton_bar(0.94, 22.)),
        );

    let intake_skel = view()
        .w_full()
        .h(px(28.))
        .pt_2()
        .border_t_1()
        .border_color(BORDER)
        .flex_row()
        .items_center()
        .justify_between()
        .child(skeleton_bar(0.38, 12.))
        .child(skeleton_bar(0.24, 12.));

    view()
        .gap_4()
        .min_w(px(0.))
        .child(
            view()
                .h(px(24.))
                .items_end()
                .justify_center()
                .child(skeleton_bar(0.28, 12.)),
        )
        .child(
            view()
                .gap_2p5()
                .child(skeleton_bar(0.64, 16.))
                .child(kpi_strip),
        )
        .child(timeline_skel)
        .child(overview_row(narrow, overview_a, overview_b, overview_c))
        .child(agents_skel)
        .child(heatmap_skel)
        .child(timings_skel)
        .child(intake_skel)
}

fn overview_row(narrow: bool, a: View, b: View, c: View) -> View {
    if narrow {
        view().gap_4().min_w(px(0.)).child(a).child(b).child(c)
    } else {
        view()
            .flex_row()
            .items_stretch()
            .gap_4()
            .min_w(px(0.))
            .child(a)
            .child(b)
            .child(c)
    }
}

fn dashboard(ui: Ui) -> Option<Dashboard> {
    let profiles = ui.all_profiles.get();
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
            model::compute_dashboard(
                s,
                &loaded.events,
                &loaded.activity,
                (!loaded.scope.is_empty()).then_some(loaded.scope.as_str()),
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
                model::compute_dashboard(
                    s,
                    evs,
                    act,
                    (!scope.is_empty()).then_some(scope.as_str()),
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
    effect(move || {
        let page = ui.page.get();
        let _scope = ui.insights.scope_project.get();
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
                    view()
                        .w_full()
                        .max_w(px(1240.))
                        .min_w(px(0.))
                        .p_5()
                        .gap_4()
                        .child(dynamic(move || {
                            let narrow = ui.narrow.get();
                            let Some(dash) = dashboard.get() else {
                                return render_loading_skeleton(narrow).into_element();
                            };
                            let project_order: Vec<String> =
                                dash.project_stats.iter().map(|p| p.id.clone()).collect();

                            let mut body = view()
                                .gap_4()
                                .min_w(px(0.))
                                .child(render_summary_and_kpis(ui, &dash, narrow));

                            if !ui.insights.drilldown.get().is_empty() {
                                body = body.child(render_card_drilldown(ui, &dash));
                            }

                            body = body.child(render_timeline(ui, &dash, &project_order));

                            let output_panel = if dash.range == RangeWindow::Today {
                                render_hourly_output(ui, &dash)
                            } else {
                                render_daily_output(ui, &dash)
                            };
                            body = body.child(overview_row(
                                narrow,
                                output_panel,
                                render_automation_funnel(&dash),
                                render_projects(ui, &dash, &project_order),
                            ));

                            body = body
                                .child(render_agents(ui, &dash))
                                .child(render_heatmap_panel(ui, &dash))
                                .child(render_timings(ui, &dash))
                                .child(render_footer(ui, &dash));

                            view()
                                .gap_3()
                                .min_w(px(0.))
                                .child(render_compare_subbar(ui, &dash))
                                .child(body)
                                .into_element()
                        })),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{
        ActivityDetail, ActivityKind, ActivitySegment, AgentWorkData, Card, Delivery, DwellData,
        DwellLane, Event, Profile, Project, Snapshot, Workflow,
    };
    use nsh_core::{
        ApproxTextEngine, Core, EventKind, EventPayload, MountBatch, MountItem, NodeId,
        element::JsEvent, geometry::size,
    };
    use serde_json::json;
    use std::collections::{BTreeMap, HashMap, HashSet};

    struct Tracker {
        alive: HashSet<NodeId>,
        parents: HashMap<NodeId, NodeId>,
        texts: HashMap<NodeId, String>,
        labels: HashMap<String, NodeId>,
    }

    impl Tracker {
        fn new() -> Self {
            Self {
                alive: HashSet::new(),
                parents: HashMap::new(),
                texts: HashMap::new(),
                labels: HashMap::new(),
            }
        }

        fn apply(&mut self, batch: &MountBatch) {
            for item in &batch.items {
                match item {
                    MountItem::Create { id, .. } => {
                        self.alive.insert(*id);
                    }
                    MountItem::Insert { parent, child, .. } => {
                        self.parents.insert(*child, *parent);
                    }
                    MountItem::Remove { id } => {
                        self.parents.remove(id);
                    }
                    MountItem::Delete { id } => {
                        self.alive.remove(id);
                        self.parents.remove(id);
                        self.texts.remove(id);
                    }
                    MountItem::Text { id, text, .. } => {
                        self.texts.insert(*id, text.text.to_string());
                    }
                    MountItem::ViewProps { id, props } => {
                        if let Some(lbl) = &props.accessibility.label {
                            self.labels.insert(lbl.to_string(), *id);
                        }
                    }
                    _ => {}
                }
            }
        }

        fn is_mounted(&self, mut id: NodeId) -> bool {
            if !self.alive.contains(&id) {
                return false;
            }
            let mut guard = 0;
            while id != nsh_core::ROOT_ID {
                let Some(&p) = self.parents.get(&id) else {
                    return false;
                };
                id = p;
                guard += 1;
                if guard > 256 {
                    return false;
                }
            }
            true
        }

        fn has_mounted_text(&self, needle: &str) -> bool {
            self.texts
                .iter()
                .any(|(id, t)| t == needle && self.is_mounted(*id))
        }

        fn has_mounted_substring(&self, needle: &str) -> bool {
            self.texts
                .iter()
                .any(|(id, t)| t.contains(needle) && self.is_mounted(*id))
        }
    }

    fn pump(core: &mut Core<ApproxTextEngine>, tree: &mut Tracker) {
        for _ in 0..8 {
            let had_batch = if let Some(batch) = core.commit() {
                tree.apply(&batch);
                true
            } else {
                false
            };
            let mut events = Vec::new();
            core.take_events(&mut events);
            let mut dispatched = false;
            for ev in events {
                match ev {
                    JsEvent::Event(id, kind, payload) => {
                        core.component_event(id, kind, &payload);
                        dispatched = true;
                    }
                    JsEvent::LayoutListener {
                        host,
                        id,
                        listener,
                        frame,
                    } => {
                        core.component_layout(host, id, &listener, frame);
                        dispatched = true;
                    }
                    _ => {}
                }
            }
            if !had_batch && !dispatched {
                break;
            }
        }
    }

    fn build_test_ui(now_ms: u64, loading: bool) -> Ui {
        canban_ui_kit::tokens::install_theme();
        let ui = Ui::new();
        ui.insights.anchor_ms.set(Some(now_ms));

        ui.all_profiles.set(vec![
            Profile {
                id: "developer".into(),
                preset: Some("codex".into()),
                model: Some("gpt-5.4".into()),
                ..Default::default()
            },
            Profile {
                id: "planner".into(),
                preset: Some("codex".into()),
                model: Some("gpt-5.4".into()),
                ..Default::default()
            },
            Profile {
                id: "reviewer".into(),
                preset: Some("claude".into()),
                model: Some("claude-sonnet-4".into()),
                ..Default::default()
            },
        ]);

        let mut snap = Snapshot::default();
        snap.projects.push(Project {
            id: "p1".into(),
            name: "Canban".into(),
            directory: "/tmp/canban".into(),
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
        });
        let mut card: Card = serde_json::from_value(json!({
            "id": "card-41",
            "project_id": "p1",
            "number": 41,
            "title": "Insights statistics dashboard",
            "body": "",
            "stage": "review",
            "revision": 2,
            "created_at": now_ms - 7_200_000,
        }))
        .unwrap();
        card.delivery = Some(Delivery {
            target_branch: "main".into(),
            commit: "abc41".into(),
            request_key: "merge:card-41".into(),
            merged_at: now_ms - 1_800_000,
        });
        snap.cards.push(card);
        let events = vec![
            Event {
                id: "ev-1".into(),
                project_id: "p1".into(),
                card_id: "card-41".into(),
                sequence: 1,
                kind: "execution.queued".into(),
                actor: "system".into(),
                message: String::new(),
                data: json!({"terminal_id": "card-41:developer", "run_number": 1, "trigger": "workflow", "workflow_run_id": "wf-41"}),
                created_at: now_ms - 5_400_000,
            },
            Event {
                id: "ev-1s".into(),
                project_id: "p1".into(),
                card_id: "card-41".into(),
                sequence: 2,
                kind: "execution.started".into(),
                actor: "system".into(),
                message: String::new(),
                data: json!({"terminal_id": "card-41:developer", "run_number": 1, "trigger": "workflow", "workflow_run_id": "wf-41"}),
                created_at: now_ms - 5_340_000,
            },
            Event {
                id: "ev-2".into(),
                project_id: "p1".into(),
                card_id: "card-41".into(),
                sequence: 3,
                kind: "execution.reported".into(),
                actor: "agent:developer".into(),
                message: String::new(),
                data: json!({"state": "completed", "workflow_run_id": "wf-41"}),
                created_at: now_ms - 2_400_000,
            },
        ];

        let activity = ActivityInput {
            loaded: true,
            period_segments: vec![
                ActivitySegment {
                    id: Some(1),
                    kind: ActivityKind::Dwell,
                    start_ms: now_ms - 3_600_000,
                    end_ms: now_ms - 2_400_000,
                    project_id: Some("p1".into()),
                    card_id: Some("card-41".into()),
                    terminal_id: None,
                    open: false,
                    data: ActivityDetail::Dwell(DwellData {
                        lane: DwellLane::Card,
                        page: "card".into(),
                    }),
                },
                ActivitySegment {
                    id: Some(2),
                    kind: ActivityKind::Agent,
                    start_ms: now_ms - 3_000_000,
                    end_ms: now_ms - 1_200_000,
                    project_id: Some("p1".into()),
                    card_id: Some("card-41".into()),
                    terminal_id: Some("card-41:developer".into()),
                    open: false,
                    data: ActivityDetail::Agent(AgentWorkData {
                        profile_id: "developer".into(),
                        provider: "codex".into(),
                        model: "gpt-5.4".into(),
                        engine: Some("codex".into()),
                        stage: Some("develop".into()),
                        run_number: Some(1),
                        trigger: Some("workflow".into()),
                        manual: false,
                    }),
                },
            ],
            ..Default::default()
        };

        ui.snapshot.set(snap);
        ui.insights.events.set(Rc::new(events));
        ui.insights.activity.set(Rc::new(activity));
        ui.insights.preview_loading.set(loading);
        ui.insights.loading.set(loading);
        ui.project.set("p1".into());
        ui.page.set("insights".into());
        ui
    }

    #[test]
    fn insights_route_mounts_timeline_first_cockpit_and_switches_agent_groups_and_ranges() {
        #[component]
        fn InsightsHarness() -> impl IntoElement {
            let ui = build_test_ui(1_794_744_000_000u64, false);
            view()
                .w(px(1400.))
                .h(px(3600.))
                .child(crate::frame::screen(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1400.), px(3600.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<InsightsHarness>());
        let mut tree = Tracker::new();
        pump(&mut core, &mut tree);

        assert!(tree.has_mounted_text("Insights"));
        assert!(tree.has_mounted_text("Resolved"));
        assert!(tree.has_mounted_text("Merges (Local / PR)"));
        assert!(tree.has_mounted_text("My time"));
        assert!(tree.has_mounted_text("Agent working"));
        assert!(tree.has_mounted_text("Timeline"));
        assert!(tree.has_mounted_text("Output (cumulative)"));
        assert!(tree.has_mounted_text("Automation"));
        assert!(tree.has_mounted_text("Projects"));
        assert!(tree.has_mounted_text("Agents"));
        assert!(tree.has_mounted_text("Past year"));
        assert!(tree.has_mounted_text("Timings"));
        assert!(tree.has_mounted_text("Execution → resolution"));
        assert!(tree.has_mounted_text("Queue → start"));
        assert!(tree.has_mounted_substring("Human asks:"));
        assert!(tree.has_mounted_substring("Intake  Local"));
        assert!(tree.has_mounted_substring("Streak "));

        // Removed blocks must stay absent
        assert!(!tree.has_mounted_text("Trigger sources"));
        assert!(!tree.has_mounted_substring("Merge queue efficiency"));
        assert!(!tree.has_mounted_text("▸ Efficiency & timing percentiles"));
        assert!(!tree.has_mounted_text("Develop"));
        assert!(!tree.has_mounted_text("Daily"));
        assert!(!tree.has_mounted_text("24"));

        // Toggle [24h] timeline window
        let btn_24h = *tree.labels.get("24h").expect("24h timeline toggle");
        assert!(core.component_event(btn_24h, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("24"));

        // Toggle [Inspect ▸] timing drawer
        let btn_inspect = *tree
            .labels
            .get("Inspect ▸ Execution → resolution")
            .expect("Inspect cycle button");
        assert!(core.component_event(btn_inspect, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("Hide ▾"));
        assert!(tree.has_mounted_substring("slowest-first"));

        // Switch Agents grouping to Model and back to Profile
        let btn_model = *tree.labels.get("Model").expect("Model group button");
        assert!(core.component_event(btn_model, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("gpt-5.4"));

        let btn_profile = *tree.labels.get("Profile").expect("Profile group button");
        assert!(core.component_event(btn_profile, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("@developer"));

        // Switch range to 7D via segmented button and back to 1D
        let btn_7d = *tree.labels.get("7D").expect("7D segment button");
        assert!(core.component_event(btn_7d, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("Daily activity"));
        assert!(tree.has_mounted_text("Daily output"));
        assert!(!tree.has_mounted_text("Output (cumulative)"));

        let btn_1d = *tree.labels.get("1D").expect("1D segment button");
        assert!(core.component_event(btn_1d, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("Output (cumulative)"));

        // Switch heatmap metric to My time
        let btn_my_time = *tree
            .labels
            .get("My time")
            .expect("My time heatmap metric button");
        assert!(core.component_event(btn_my_time, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("Past year"));

        // Manual refresh button is mounted in Chrome and triggers refresh intent cleanly
        let btn_refresh = *tree
            .labels
            .get("Refresh Insights")
            .expect("Refresh Insights chrome button");
        assert!(core.component_event(btn_refresh, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("Timeline"));
    }

    #[test]
    fn insights_loading_skeleton_renders_fixed_height_sections() {
        #[component]
        fn LoadingHarness() -> impl IntoElement {
            let ui = build_test_ui(1_794_744_000_000u64, true);
            view()
                .w(px(1400.))
                .h(px(3600.))
                .child(crate::frame::screen(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1400.), px(3600.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<LoadingHarness>());
        let mut tree = Tracker::new();
        pump(&mut core, &mut tree);

        assert!(tree.has_mounted_text("Insights"));
        assert!(tree.has_mounted_text("Timeline"));
        assert!(tree.has_mounted_text("Output (cumulative)"));
        assert!(tree.has_mounted_text("Automation"));
        assert!(tree.has_mounted_text("Projects"));
        assert!(tree.has_mounted_text("Agents"));
        assert!(tree.has_mounted_text("Past year"));
        assert!(tree.has_mounted_text("Timings"));
        assert!(!tree.has_mounted_text("Execution → resolution"));
    }

    #[test]
    fn insights_empty_state_omits_empty_project_parentheses() {
        #[component]
        fn EmptyHarness() -> impl IntoElement {
            let ui = build_test_ui(1_794_744_000_000u64, false);
            ui.snapshot.set(Snapshot::default());
            ui.insights.events.set(Rc::new(Vec::new()));
            ui.insights.activity.set(Rc::new(ActivityInput {
                loaded: true,
                ..Default::default()
            }));
            ui.project.set(ALL_PROJECTS_OPTION.into());
            view()
                .w(px(1400.))
                .h(px(3600.))
                .child(crate::frame::screen(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1400.), px(3600.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<EmptyHarness>());
        let mut tree = Tracker::new();
        pump(&mut core, &mut tree);

        assert!(
            tree.has_mounted_text("Color = Project · Hover for card & duration · Click to open")
        );
        assert!(!tree.has_mounted_substring("Color = Project ("));
    }
}
