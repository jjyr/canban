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
    ProjectSort, RangeWindow, TimelineLaneKind, ViewOptions, format_delta_pct,
    format_duration_compact, format_duration_delta, percentile,
};
use nsh_core::{
    element::{AdaptiveColor, rgb},
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
    pub clock_minute: u64,
    pub generation: u64,
    pub want_annual: bool,
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
    pub scroll: Signal<Option<ElementRef>>,
    pub annual_width: Signal<f32>,
    pub timeline_width: Signal<f32>,
    pub project_sort: Signal<ProjectSort>,
    pub drilldown: Signal<String>,
    pub timing_tab: Signal<String>,
    pub timing_page: Signal<usize>,
    /// Optional fixed timestamp (ms) for deterministic Gallery/test rendering; `None` uses live clock.
    pub anchor_ms: Signal<Option<u64>>,
    pub events: Signal<Rc<Vec<canban_types::Event>>>,
    pub events_loading: Signal<bool>,
    pub events_key: Signal<Option<(String, u64)>>,
    pub activity: Signal<Rc<ActivityInput>>,
    pub activity_loading: Signal<bool>,
    pub activity_key: Signal<Option<ActivityCacheKey>>,
    pub annual_activity_key: Signal<Option<(String, String)>>,
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
            scroll: signal(None),
            annual_width: signal(0.),
            timeline_width: signal(0.),
            project_sort: signal(ProjectSort::Time),
            drilldown: signal(String::new()),
            timing_tab: signal(String::new()),
            timing_page: signal(0),
            anchor_ms: signal(None),
            events: signal(Rc::new(Vec::new())),
            events_loading: signal(false),
            events_key: signal(None),
            activity: signal(Rc::new(ActivityInput::default())),
            activity_loading: signal(false),
            activity_key: signal(None),
            annual_activity_key: signal(None),
        }
    }

    pub fn timeline_width_px(&self) -> f32 {
        let w = self.timeline_width.get();
        if w > 20.0 { w } else { 900.0 }
    }
}

fn has_backend() -> bool {
    crate::backend::is_connected()
}

pub fn refresh_events(ui: Ui) {
    if !has_backend() || ui.page.get_untracked() != "insights" {
        return;
    }
    let scope = ui.insights.scope_project.get_untracked();
    let generation = ui.index.with_untracked(|idx| idx.generation);
    let key = (scope.clone(), generation);
    if ui.insights.events_key.get_untracked().as_ref() == Some(&key)
        || ui.insights.events_loading.get_untracked()
    {
        return;
    }
    ui.insights.events_loading.set(true);
    ui.spawn(async move {
        let scope_req = scope.clone();
        let fetched = blocking(move || -> Vec<canban_types::Event> {
            let parse_events = |val: serde_json::Value| -> Vec<canban_types::Event> {
                val["events"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| {
                                serde_json::from_value::<canban_types::Event>(v.clone()).ok()
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            };
            if !scope_req.is_empty() {
                crate::routed_call(
                    "insights.events",
                    serde_json::json!({"project_id": scope_req}),
                    Some(scope_req.as_str()),
                )
                .map(parse_events)
                .unwrap_or_default()
            } else {
                let mut all = Vec::new();
                for (_host, res) in crate::call_all_hosts("insights.events", serde_json::json!({}))
                {
                    if let Ok(val) = res {
                        all.extend(parse_events(val));
                    }
                }
                all
            }
        })
        .await;
        ui.insights.events_loading.set(false);
        if let Ok(events) = fetched {
            batch(move || {
                ui.insights.events.set(Rc::new(events));
                ui.insights.events_key.set(Some(key));
            });
            let current_scope = ui.insights.scope_project.get_untracked();
            let current_gen = ui.index.with_untracked(|idx| idx.generation);
            if ui.page.get_untracked() == "insights"
                && (current_scope != scope || current_gen != generation)
            {
                refresh_events(ui);
            }
        }
    });
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

pub fn refresh_activity(ui: Ui) {
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
    let generation = ui.index.with_untracked(|idx| idx.generation);
    let key = ActivityCacheKey {
        scope: scope.clone(),
        range,
        selected_date,
        comparison,
        clock_minute: now / 60_000,
        generation,
        want_annual,
    };
    if ui.insights.activity_key.get_untracked().as_ref() == Some(&key)
        || ui.insights.activity_loading.get_untracked()
    {
        return;
    }

    let annual_cache_key = (scope.clone(), actual_today_key);
    let annual_cached =
        ui.insights.annual_activity_key.get_untracked().as_ref() == Some(&annual_cache_key);
    let fetch_annual = want_annual && !annual_cached;
    let existing_annual = if annual_cached {
        ui.insights
            .activity
            .with_untracked(|a| a.annual_buckets.clone())
    } else {
        Vec::new()
    };

    let plan: ActivityFetchPlan =
        model::activity_fetch_plan(range, now, &chrono::Local, selected_date, comparison);

    ui.insights.activity_loading.set(true);
    ui.spawn(async move {
        let scope_req = scope.clone();
        let fetched = blocking(move || -> ActivityInput {
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
                input.annual_buckets = fetch_merged_buckets(&plan.annual_boundaries, &scope_req);
            }
            input
        })
        .await;

        ui.insights.activity_loading.set(false);
        if let Ok(input) = fetched {
            batch(move || {
                ui.insights.activity.set(Rc::new(input));
                ui.insights.activity_key.set(Some(key));
                if fetch_annual {
                    ui.insights.annual_activity_key.set(Some(annual_cache_key));
                }
            });
            let cur_now = ui
                .insights
                .anchor_ms
                .get_untracked()
                .unwrap_or_else(canban_types::now);
            let cur_want_annual =
                ui.insights.heatmap_metric.get_untracked() != HeatmapMetric::Resolved;
            if ui.page.get_untracked() == "insights"
                && (ui.insights.scope_project.get_untracked() != scope
                    || ui.insights.range.get_untracked() != range
                    || ui.insights.selected_date.get_untracked() != selected_date
                    || ui.insights.comparison.get_untracked() != comparison
                    || cur_want_annual != want_annual
                    || cur_now / 60_000 != now / 60_000
                    || ui.index.with_untracked(|idx| idx.generation) != generation)
            {
                refresh_activity(ui);
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

pub fn select_date(ui: Ui, date: Option<chrono::NaiveDate>) {
    let actual = chrono::DateTime::from_timestamp_millis(now_ms(ui) as i64)
        .unwrap()
        .with_timezone(&chrono::Local)
        .date_naive();
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
            let actual = chrono::DateTime::from_timestamp_millis(now_ms(ui) as i64)
                .unwrap()
                .with_timezone(&chrono::Local)
                .date_naive();
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

    let scope_picker = search_popover(
        press_target()
            .h(px(28.))
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
                .child(scope_picker)
                .child(range_bar),
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

fn segmented_bar(segments: &[(usize, AdaptiveColor)]) -> View {
    let total: usize = segments.iter().map(|(v, _)| *v).sum();
    if total == 0 {
        return view().w_full().h(px(6.)).rounded(px(3.)).bg(INSET);
    }
    view()
        .w_full()
        .h(px(6.))
        .rounded(px(3.))
        .overflow_hidden()
        .bg(INSET)
        .flex_row()
        .gap(px(1.))
        .children(
            segments
                .iter()
                .filter(|(v, _)| *v > 0)
                .map(|&(v, col)| view().flex_grow(v as f32).h_full().bg(col).into_element())
                .collect::<Vec<_>>(),
        )
}

struct KpiSpec {
    mark: Icon,
    mark_tint: ThemeColor,
    eyebrow: &'static str,
    hero: String,
    pill_text: Option<(String, Tone)>,
    bar_segments: Vec<(usize, AdaptiveColor)>,
    sub_primary: String,
    sub_secondary: String,
}

fn kpi_card(spec: KpiSpec) -> View {
    let KpiSpec {
        mark,
        mark_tint,
        eyebrow,
        hero,
        pill_text,
        bar_segments,
        sub_primary,
        sub_secondary,
    } = spec;
    let mut header = view()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .child(icon_tinted(mark, mark_tint).w(px(14.)).h(px(14.)))
                .child(
                    text(eyebrow)
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(MUTED),
                ),
        );
    if let Some((pt, tone)) = pill_text {
        header = header.child(status(pt, tone));
    }

    card_surface()
        .flex_1()
        .min_w(px(200.))
        .gap_2()
        .child(header)
        .child(
            text(hero)
                .font_family(MONO_FONT)
                .text_size(px(23.))
                .font_weight(FontWeight::BOLD)
                .text_color(TEXT),
        )
        .child(segmented_bar(&bar_segments))
        .child(
            view()
                .gap_0p5()
                .child(label(sub_primary).text_size(px(CAPTION_SIZE)).truncate())
                .child(muted(sub_secondary).text_size(px(11.5)).truncate()),
        )
}

fn render_period_subbar(ui: Ui, dash: &Dashboard) -> View {
    let actual = dash.annual_end;
    let dash_date = dash.selected_date;

    let mut top = view()
        .p_3()
        .gap_2()
        .bg(SURFACE)
        .rounded(px(RADIUS))
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    row()
                        .child(chrome_button(Icon::Back, "Previous day", move |_| {
                            intent(ui, "previous-day")
                        }))
                        .child(label(dash.period_label.clone()).font_weight(FontWeight::MEDIUM))
                        .child(chrome_button_disabled(
                            Icon::Forward,
                            "Next day",
                            move || dash_date >= actual,
                            move |_| intent(ui, "next-day"),
                        ))
                        .child(compact_action("Today", Variant::Quiet, move |_| {
                            intent(ui, "today")
                        }))
                        .child(note(format!(
                            "vs {} · {}",
                            dash.comparison_label, dash.timezone
                        ))),
                )
                .child(
                    row()
                        .child(choice(
                            ui,
                            "Previous period",
                            "compare-previous",
                            ui.insights.comparison.get() == Comparison::Previous,
                        ))
                        .child(if dash.range == RangeWindow::Today {
                            choice(
                                ui,
                                "Last week, same day",
                                "compare-week",
                                ui.insights.comparison.get() == Comparison::LastWeek,
                            )
                            .into_element()
                        } else {
                            view().into_element()
                        }),
                ),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    row()
                        .child(
                            field(ui.insights.date_text, "YYYY-MM-DD")
                                .label("Insights date")
                                .w(px(136.)),
                        )
                        .child(
                            compact_action("Apply date", Variant::Quiet, move |_| {
                                intent(ui, "apply-date")
                            })
                            .disabled(move || {
                                chrono::NaiveDate::parse_from_str(
                                    &ui.insights.date_text.get(),
                                    "%Y-%m-%d",
                                )
                                .ok()
                                .is_none_or(|d| d > actual)
                            }),
                        )
                        .child(compact_action(
                            format!("Created Cards ({})", dash.created_total),
                            if ui.insights.drilldown.get() == "created" {
                                Variant::Secondary
                            } else {
                                Variant::Quiet
                            },
                            move |_| intent(ui, "cards-created"),
                        ))
                        .child(compact_action(
                            format!("Resolved Cards ({})", dash.merged_total),
                            if ui.insights.drilldown.get() == "merged" {
                                Variant::Secondary
                            } else {
                                Variant::Quiet
                            },
                            move |_| intent(ui, "cards-merged"),
                        )),
                )
                .child(if !ui.insights.scope_project.get().is_empty() {
                    compact_action("Clear Project filter", Variant::Quiet, move |_| {
                        ui.insights.scope_project.set(String::new());
                    })
                    .into_element()
                } else {
                    view().into_element()
                }),
        );

    if dash.history_start.is_none_or(|h| h > dash.selected_date) {
        top = top.child(note(
            "No retained history for this period; unavailable counts are shown as —.",
        ));
    } else if dash
        .history_start
        .is_some_and(|h| h.to_string() > dash.window_start_key)
    {
        top = top.child(note(
            "Partial history: the selected period starts before retained records.",
        ));
    }
    top
}

fn non_merge_breakdown(count: usize) -> String {
    if count > 0 {
        format!(" · {count} manual/agent")
    } else {
        String::new()
    }
}

fn format_time_delta_pill(current: u64, previous: u64, baseline_known: bool) -> (String, Tone) {
    if !baseline_known {
        return ("No baseline".into(), Tone::Neutral);
    }
    if current == previous {
        return ("0s vs prev".into(), Tone::Neutral);
    }
    let diff = format_duration_compact(current.abs_diff(previous));
    let sign = if current > previous { "+" } else { "−" };
    (format!("{sign}{diff} vs prev"), Tone::Neutral)
}

fn render_summary_and_kpis(dash: &Dashboard, narrow: bool) -> View {
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

    let delivery_pill = if dash.baseline_known {
        format_delta_pct(dash.merged_total, dash.prev_merged_total)
    } else {
        "Insufficient history".into()
    };
    let sync_suffix = if dash.initial_sync_period > 0 && dash.exclude_initial_sync {
        format!(" (+{} sync muted)", dash.initial_sync_period)
    } else {
        String::new()
    };
    let net_sign = if dash.net_wip_delta > 0 { "+" } else { "" };

    let total_merges = dash.local_merges + dash.pr_merges;
    let clean_pct_str = dash
        .timings
        .clean_merge_rate_pct()
        .map(|p| format!("{p}% clean"))
        .unwrap_or_else(|| "— clean".into());
    let merge_sub_secondary = if dash.timings.merge_queue_batches > 0 {
        format!(
            "{} queue batches · {}",
            dash.timings.merge_queue_batches, clean_pct_str
        )
    } else {
        format!(
            "WIP {}/{}/{} (B/D/R)",
            dash.pipeline.backlog, dash.pipeline.develop, dash.pipeline.review
        )
    };

    let pass_str = dash
        .success_rate_pct
        .map(|p| format!("{p}% pass"))
        .unwrap_or_else(|| "No runs".into());

    let dwell_pill =
        format_time_delta_pill(t.dwell_total_ms, t.prev_dwell_total_ms, dash.baseline_known);
    let agent_pill =
        format_time_delta_pill(t.agent_total_ms, t.prev_agent_total_ms, dash.baseline_known);

    let cards = [
        KpiSpec {
            mark: Icon::Check,
            mark_tint: SUCCESS,
            eyebrow: "RESOLVED",
            hero: if known {
                dash.merged_total.to_string()
            } else {
                "—".into()
            },
            pill_text: Some((delivery_pill, Tone::Neutral)),
            bar_segments: vec![
                (dash.local_merges, ACCENT.resolve()),
                (dash.pr_merges, SUCCESS.resolve()),
                (dash.manual_resolutions, alpha_tint(MUTED, 0.55)),
            ],
            sub_primary: format!(
                "+{} organic{} · net WIP {net_sign}{}",
                dash.created_total, sync_suffix, dash.net_wip_delta
            ),
            sub_secondary: if dash.range == RangeWindow::Today {
                dash.reference_merged
                    .map(|n| format!("7D pace: {n:.1}/day ({}d)", dash.reference_days))
                    .unwrap_or_else(|| format!("{} active WIP", dash.pipeline.active_wip))
            } else {
                format!(
                    "Prev resolved: {} · {} active WIP",
                    if dash.baseline_known {
                        dash.prev_merged_total.to_string()
                    } else {
                        "—".into()
                    },
                    dash.pipeline.active_wip
                )
            },
        },
        KpiSpec {
            mark: Icon::GitMerge,
            mark_tint: ACCENT,
            eyebrow: "MERGES (LOCAL / PR)",
            hero: if known {
                format!("{} / {}", dash.local_merges, dash.pr_merges)
            } else {
                "—".into()
            },
            pill_text: Some((format!("{total_merges} merges"), Tone::Neutral)),
            bar_segments: vec![
                (dash.local_merges, ACCENT.resolve()),
                (dash.pr_merges, SUCCESS.resolve()),
            ],
            sub_primary: format!(
                "{} local · {} PR{}",
                dash.local_merges,
                dash.pr_merges,
                non_merge_breakdown(dash.manual_resolutions)
            ),
            sub_secondary: merge_sub_secondary,
        },
        KpiSpec {
            mark: Icon::Clock,
            mark_tint: ACCENT,
            eyebrow: "MY TIME",
            hero: my_time_hero,
            pill_text: Some(dwell_pill),
            bar_segments: vec![
                (
                    t.dwell_card_ms.min(u64::from(u32::MAX)) as usize,
                    ACCENT.resolve(),
                ),
                (
                    t.dwell_terminal_ms.min(u64::from(u32::MAX)) as usize,
                    SUCCESS.resolve(),
                ),
                (
                    t.dwell_other_ms.min(u64::from(u32::MAX)) as usize,
                    alpha_tint(MUTED, 0.55),
                ),
            ],
            sub_primary: format!(
                "Card {} · Terminal {} · Other {}",
                format_duration_compact(t.dwell_card_ms),
                format_duration_compact(t.dwell_terminal_ms),
                format_duration_compact(t.dwell_other_ms),
            ),
            sub_secondary: "Active window dwell (5m idle cutoff)".into(),
        },
        KpiSpec {
            mark: Icon::Agent,
            mark_tint: SUCCESS,
            eyebrow: "AGENT WORKING",
            hero: agent_hero,
            pill_text: Some(agent_pill),
            bar_segments: vec![
                (dash.completed_runs, SUCCESS.resolve()),
                (dash.active_runs, ACCENT.resolve()),
                (dash.blocked_runs, ATTENTION.resolve()),
                (dash.failed_runs, DANGER.resolve()),
            ],
            sub_primary: format!(
                "{} runs · {} · {} profiles",
                dash.scheduled_total, pass_str, dash.used_profiles
            ),
            sub_secondary: format!(
                "{} human asks · {} active now",
                dash.human_asks_period, dash.active_runs
            ),
        },
    ]
    .into_iter()
    .map(kpi_card)
    .collect::<Vec<_>>();

    let mut iter = cards.into_iter();
    let grid = if narrow {
        view()
            .gap_3()
            .child(
                view()
                    .flex_row()
                    .gap_3()
                    .children(iter.by_ref().take(2).collect::<Vec<_>>()),
            )
            .child(view().flex_row().gap_3().children(iter.collect::<Vec<_>>()))
    } else {
        view().flex_row().gap_3().children(iter.collect::<Vec<_>>())
    };

    view().gap_2p5().child(summary_sentence).child(grid)
}

fn format_hhmm(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "—".into())
}

fn render_timeline_1d(ui: Ui, dash: &Dashboard, project_order: &[String]) -> View {
    let tl = &dash.timeline;
    let total_span = tl.day_end_ms.saturating_sub(tl.day_start_ms).max(1) as f64;
    let track_w = ui.insights.timeline_width_px();

    let project_legend = dash
        .project_stats
        .iter()
        .take(6)
        .map(|p| legend_swatch(project_color(Some(&p.id), project_order), &p.name).into_element())
        .collect::<Vec<_>>();

    let mut header_legend = row().children(project_legend);
    if !tl.merges.is_empty() {
        header_legend = header_legend.child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .child(view().w(px(8.)).h(px(8.)).rounded(px(4.)).bg(SUCCESS))
                .child(muted("Merges").text_size(px(11.5))),
        );
    }

    let axis_row = view()
        .flex_row()
        .items_center()
        .gap_2()
        .child(view().w(px(140.)).flex_shrink_0())
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .on_layout(move |b: &Bounds<Points>| {
                    let w = b.size.width.0;
                    if w > 20.0 && (ui.insights.timeline_width.get_untracked() - w).abs() > 0.5 {
                        ui.insights.timeline_width.set(w);
                    }
                })
                .flex_row()
                .justify_between()
                .children(
                    [
                        "00:00", "04:00", "08:00", "12:00", "16:00", "20:00", "24:00",
                    ]
                    .into_iter()
                    .map(|t| {
                        text(t)
                            .font_family(MONO_FONT)
                            .text_size(px(10.))
                            .text_color(MUTED)
                            .into_element()
                    })
                    .collect::<Vec<_>>(),
                ),
        )
        .child(view().w(px(58.)).flex_shrink_0());

    let active_ratio =
        ((tl.active_until_ms.saturating_sub(tl.day_start_ms)) as f64 / total_span).clamp(0.0, 1.0);
    let now_x = (active_ratio as f32) * track_w;
    let is_partial_day = tl.active_until_ms < tl.day_end_ms;

    let lane_views = tl
        .lanes
        .iter()
        .map(|lane| {
            let mut track = view()
                .flex_1()
                .min_w(px(0.))
                .h(px(22.))
                .rounded(px(4.))
                .bg(INSET)
                .relative()
                .overflow_hidden();

            for idx in 1..6 {
                let gx = (idx as f32 / 6.0) * track_w;
                track = track.child(
                    view()
                        .absolute()
                        .left(px(gx))
                        .top_0()
                        .bottom_0()
                        .w(px(1.))
                        .bg(BORDER)
                        .opacity(0.35)
                        .pointer_events(PointerEvents::None),
                );
            }

            if is_partial_day {
                let future_w = (track_w - now_x).max(0.0);
                track = track
                    .child(
                        view()
                            .absolute()
                            .left(px(now_x))
                            .top_0()
                            .bottom_0()
                            .w(px(future_w))
                            .bg(BACKGROUND)
                            .opacity(0.45)
                            .pointer_events(PointerEvents::None),
                    )
                    .child(
                        view()
                            .absolute()
                            .left(px(now_x))
                            .top_0()
                            .bottom_0()
                            .w(px(1.))
                            .bg(ACCENT)
                            .opacity(0.65)
                            .pointer_events(PointerEvents::None),
                    );
            }

            for b in &lane.blocks {
                let left_ratio = ((b.start_ms.saturating_sub(tl.day_start_ms)) as f64 / total_span)
                    .clamp(0.0, 1.0) as f32;
                let width_ratio = ((b.end_ms.saturating_sub(b.start_ms)) as f64 / total_span)
                    .clamp(0.0, 1.0) as f32;
                let left_px = (left_ratio * track_w).min((track_w - 3.0).max(0.0));
                let width_px = (width_ratio * track_w)
                    .max(3.0)
                    .min((track_w - left_px).max(3.0));
                let dur = b.end_ms.saturating_sub(b.start_ms);
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

                track = track.child(
                    view()
                        .absolute()
                        .left(px(left_px))
                        .top(px(2.))
                        .bottom(px(2.))
                        .w(px(width_px))
                        .tooltip(tooltip_box().child(note(tip)))
                        .child(block),
                );
            }

            let left_label = if let Some(sub) = lane.sublabel.as_deref() {
                view()
                    .w(px(140.))
                    .flex_shrink_0()
                    .gap(px(1.))
                    .child(label(lane.label.clone()).text_size(px(12.)).truncate())
                    .child(note(sub).text_size(px(10.5)).truncate())
            } else {
                view()
                    .w(px(140.))
                    .flex_shrink_0()
                    .child(label(lane.label.clone()).text_size(px(12.)).truncate())
            };

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
                    .w(px(58.)),
                )
                .into_element()
        })
        .collect::<Vec<_>>();

    let mut merges_track = view()
        .flex_1()
        .min_w(px(0.))
        .h(px(18.))
        .rounded(px(4.))
        .bg(INSET)
        .relative()
        .overflow_hidden();
    for m in &tl.merges {
        let ratio = ((m.timestamp_ms.saturating_sub(tl.day_start_ms)) as f64 / total_span)
            .clamp(0.0, 1.0) as f32;
        let x = ((ratio * track_w) - 4.0).max(0.0);
        let tip = format!(
            "#{} {} · merged at {}",
            m.card_number,
            m.title,
            format_hhmm(m.timestamp_ms)
        );
        let cid = m.card_id.clone();
        merges_track = merges_track.child(
            view()
                .absolute()
                .left(px(x))
                .top(px(5.))
                .w(px(8.))
                .h(px(8.))
                .tooltip(tooltip_box().child(note(tip)))
                .child(
                    press_target()
                        .w(px(8.))
                        .h(px(8.))
                        .rounded(px(4.))
                        .bg(SUCCESS)
                        .hover(|s| s.border_1().border_color(TEXT))
                        .on_press(move |_: &()| ui.open_card(cid.clone())),
                ),
        );
    }
    let merges_row = view()
        .flex_row()
        .items_center()
        .gap_2()
        .child(note("Merges").w(px(140.)).flex_shrink_0())
        .child(merges_track)
        .child(mono_val(tl.merges.len().to_string(), 11.5).w(px(58.)));

    let has_any = tl.lanes.iter().any(|l| !l.blocks.is_empty()) || !tl.merges.is_empty();
    let mut panel = card_surface()
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
                        .child(heading("Activity timeline"))
                        .child(note(
                            "Hourly swimlanes across personal focus, Agents, and Card merges · click a block to jump in",
                        )),
                )
                .child(header_legend),
        )
        .child(axis_row)
        .children(lane_views)
        .child(merges_row);

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
                    .h_full()
                    .rounded(px(3.))
                    .overflow_hidden()
                    .flex_row()
                    .gap(px(1.))
                    .children(
                        segs.into_iter()
                            .filter(|(v, _)| *v > 0)
                            .map(|(v, c)| view().flex_grow(v as f32).h_full().bg(c).into_element())
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
                    .child(view().flex_grow((1.0 - dwell_ratio).max(0.0)).h_full())
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
                        .h_full()
                        .rounded(px(3.))
                        .bg(alpha_tint(SUCCESS, 0.85)),
                )
                .child(view().flex_grow((1.0 - agent_ratio).max(0.0)).h_full());

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

fn render_hourly_output(ui: Ui, dash: &Dashboard) -> View {
    let raw_max = dash
        .hours
        .iter()
        .filter(|h| !h.future)
        .map(|h| h.current.max(h.previous))
        .max()
        .unwrap_or(0);
    let top_tick = nice_ceil_tick(raw_max);
    let usable_h = CHART_HEIGHT - 8.0;

    let cols = dash
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
            let prev_h = if h.previous == 0 {
                0.0
            } else {
                (h.previous as f32 / top_tick as f32 * usable_h).max(2.5)
            };
            let cur_h = if h.current == 0 {
                0.0
            } else {
                (h.current as f32 / top_tick as f32 * usable_h).max(2.5)
            };
            view()
                .flex_1()
                .min_w(px(0.))
                .h(px(CHART_HEIGHT))
                .justify_end()
                .relative()
                .tooltip(tooltip_box().child(note(title)))
                .child(if h.future {
                    view().into_element()
                } else {
                    view()
                        .w_full()
                        .flex_row()
                        .gap(px(1.))
                        .items_end()
                        .child(
                            view()
                                .flex_1()
                                .h(px(prev_h))
                                .rounded(px(1.5))
                                .bg(alpha_tint(MUTED, 0.45)),
                        )
                        .child(view().flex_1().h(px(cur_h)).rounded(px(1.5)).bg(ACCENT))
                        .into_element()
                })
                .into_element()
        })
        .collect::<Vec<_>>();

    let hourly_metric = ui.insights.hourly_metric.get();
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
                        .child(heading("Hourly output"))
                        .child(note("Cumulative pace vs baseline")),
                )
                .child(
                    row()
                        .child(legend_swatch(move || alpha_tint(MUTED, 0.45), "Baseline"))
                        .child(legend_swatch(ACCENT, "Selected day")),
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
        .child(plot_grid_frame(top_tick, 2.0, cols))
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
            .child(label(name).text_size(px(12.)).w(px(104.)).truncate())
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
                            .h_full()
                            .rounded(px(3.))
                            .bg(color),
                    )
                    .child(view().flex_grow((1.0 - ratio).max(0.0)).h_full()),
            )
            .child(mono_val(count.to_string(), 11.5).w(px(36.)))
    };

    let zero_human_delta = match (f.zero_human_pct(), f.prev_zero_human_pct()) {
        (Some(cur), Some(prev)) => {
            let diff = cur as isize - prev as isize;
            Some((
                format!(
                    "{cur}% ({}{diff}% vs prev)",
                    if diff >= 0 { "+" } else { "" }
                ),
                if diff >= 0 {
                    Tone::Success
                } else {
                    Tone::Attention
                },
            ))
        }
        (Some(cur), None) => Some((format!("{cur}%"), Tone::Neutral)),
        _ => None,
    };

    let mut summary_row = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_2()
        .child(note("Zero-human resolution"));
    if let Some((txt, tone)) = zero_human_delta {
        summary_row = summary_row.child(status(txt, tone));
    } else {
        summary_row = summary_row.child(mono_val("—", 11.5));
    }

    let pipe = &dash.pipeline;
    card_surface()
        .flex_1()
        .min_w(px(280.))
        .child(
            view()
                .gap_0p5()
                .child(heading("Automation funnel"))
                .child(note("Period flow & live WIP gates")),
        )
        .child(
            view()
                .gap_2()
                .child(step_row("Created", f.created, alpha_tint(ACCENT, 0.55)))
                .child(step_row("Agent touched", f.agent_touched, ACCENT.resolve()))
                .child(step_row("Reviewed", f.reviewed, ATTENTION.resolve()))
                .child(step_row("Merged", f.merged, SUCCESS.resolve())),
        )
        .child(view().w_full().h(px(1.)).bg(BORDER))
        .child(
            view()
                .gap_1p5()
                .child(summary_row)
                .child(note(format!(
                    "Human asks: {} · WIP: {} (B {} / D {} / R {})",
                    f.human_asks, pipe.active_wip, pipe.backlog, pipe.develop, pipe.review
                )))
                .child(note(format!(
                    "Human gated: {} · Merging now: {} · Attention: {}",
                    pipe.human_gated, pipe.merging_now, pipe.attention_cards
                ))),
        )
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

            let bar_track = match sort_mode {
                ProjectSort::Time => {
                    let tot = p.total_time_ms();
                    let ratio = if tot == 0 {
                        0.0
                    } else {
                        (tot as f32 / max_metric as f32).clamp(0.05, 1.0)
                    };
                    let segs: [(usize, AdaptiveColor); 2] = [
                        (
                            p.dwell_ms.min(u64::from(u32::MAX)) as usize,
                            ACCENT.resolve(),
                        ),
                        (
                            p.agent_ms.min(u64::from(u32::MAX)) as usize,
                            SUCCESS.resolve(),
                        ),
                    ];
                    let filled = view()
                        .flex_grow(ratio)
                        .h_full()
                        .rounded(px(3.5))
                        .overflow_hidden()
                        .flex_row()
                        .gap(px(1.))
                        .children(
                            segs.into_iter()
                                .filter(|(v, _)| *v > 0)
                                .map(|(v, col)| {
                                    view().flex_grow(v as f32).h_full().bg(col).into_element()
                                })
                                .collect::<Vec<_>>(),
                        );
                    view()
                        .flex_1()
                        .min_w(px(64.))
                        .h(px(7.))
                        .rounded(px(3.5))
                        .bg(INSET)
                        .overflow_hidden()
                        .flex_row()
                        .child(filled)
                        .child(view().flex_grow((1.0 - ratio).max(0.0)).h_full())
                }
                ProjectSort::Wip => {
                    let ratio = if total_wip == 0 {
                        0.0
                    } else {
                        (total_wip as f32 / max_metric as f32).clamp(0.05, 1.0)
                    };
                    let wip_segs: [(usize, AdaptiveColor); 3] = [
                        (p.wip_backlog, alpha_tint(MUTED, 0.55)),
                        (p.wip_develop, ACCENT.resolve()),
                        (p.wip_review, SUCCESS.resolve()),
                    ];
                    let filled = view()
                        .flex_grow(ratio)
                        .h_full()
                        .rounded(px(3.5))
                        .overflow_hidden()
                        .flex_row()
                        .gap(px(1.))
                        .children(
                            wip_segs
                                .into_iter()
                                .filter(|(v, _)| *v > 0)
                                .map(|(v, col)| {
                                    view().flex_grow(v as f32).h_full().bg(col).into_element()
                                })
                                .collect::<Vec<_>>(),
                        );
                    view()
                        .flex_1()
                        .min_w(px(64.))
                        .h(px(7.))
                        .rounded(px(3.5))
                        .bg(INSET)
                        .overflow_hidden()
                        .flex_row()
                        .child(filled)
                        .child(view().flex_grow((1.0 - ratio).max(0.0)).h_full())
                }
                _ => {
                    let (val, fill_color) = match sort_mode {
                        ProjectSort::Merged => (p.merged as u64, SUCCESS.resolve()),
                        ProjectSort::Created => (p.created as u64, ACCENT.resolve()),
                        ProjectSort::Runs => (p.agent_runs as u64, alpha_tint(ACCENT, 0.8)),
                        ProjectSort::Time | ProjectSort::Wip => unreachable!(),
                    };
                    let ratio = if val == 0 {
                        0.0
                    } else {
                        (val as f32 / max_metric as f32).clamp(0.04, 1.0)
                    };
                    view()
                        .flex_1()
                        .min_w(px(64.))
                        .h(px(7.))
                        .rounded(px(3.5))
                        .bg(INSET)
                        .overflow_hidden()
                        .flex_row()
                        .child(
                            view()
                                .flex_grow(ratio.max(0.0))
                                .h_full()
                                .rounded(px(3.5))
                                .bg(fill_color),
                        )
                        .child(view().flex_grow((1.0 - ratio).max(0.0)).h_full())
                }
            };

            let tip = format!(
                "{} · My {} · Agent {} · {} resolved · +{} created · WIP {total_wip} · {} runs",
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
                        .p_2()
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
                                .gap_1()
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
                                                .items_center()
                                                .gap_1p5()
                                                .child(
                                                    view()
                                                        .w(px(8.))
                                                        .h(px(8.))
                                                        .rounded(px(4.))
                                                        .bg(dot_color),
                                                )
                                                .child(
                                                    label(p.name.clone())
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .max_w(px(140.))
                                                        .truncate(),
                                                ),
                                        )
                                        .child(note(format!(
                                            "My {} · Agent {} · {} res",
                                            format_duration_compact(p.dwell_ms),
                                            format_duration_compact(p.agent_ms),
                                            p.merged
                                        ))),
                                )
                                .child(
                                    view()
                                        .w_full()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(bar_track)
                                        .child(
                                            mono_val(
                                                match sort_mode {
                                                    ProjectSort::Time => {
                                                        format_duration_compact(p.total_time_ms())
                                                    }
                                                    ProjectSort::Merged => {
                                                        format!("{} res", p.merged)
                                                    }
                                                    ProjectSort::Created => {
                                                        format!("+{}", p.created)
                                                    }
                                                    ProjectSort::Wip => format!("{total_wip} wip"),
                                                    ProjectSort::Runs => {
                                                        format!("{} runs", p.agent_runs)
                                                    }
                                                },
                                                11.,
                                            )
                                            .w(px(54.)),
                                        ),
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
                .child(note("Other / unattributed"))
                .child(note(format!(
                    "My {} · Agent {}",
                    format_duration_compact(dash.other_dwell_ms),
                    format_duration_compact(dash.other_agent_ms)
                )))
                .into_element(),
        );
    }

    let mut panel = card_surface().flex_1().min_w(px(280.)).child(
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .child(heading("Projects"))
            .child(
                row()
                    .child(choice(
                        ui,
                        "Time",
                        "sort-time",
                        sort_mode == ProjectSort::Time,
                    ))
                    .child(choice(
                        ui,
                        "Resolved",
                        "sort-merged",
                        sort_mode == ProjectSort::Merged,
                    ))
                    .child(choice(
                        ui,
                        "Created",
                        "sort-created",
                        sort_mode == ProjectSort::Created,
                    ))
                    .child(choice(ui, "WIP", "sort-wip", sort_mode == ProjectSort::Wip))
                    .child(choice(
                        ui,
                        "Runs",
                        "sort-runs",
                        sort_mode == ProjectSort::Runs,
                    )),
            ),
    );
    if rows.is_empty() {
        panel = panel.child(note("No Project activity or active WIP in this period."));
    }
    panel.children(rows)
}

struct AgentBarSpec {
    runs: usize,
    max_runs: usize,
    working_ms: u64,
    max_work_ms: u64,
    completed: usize,
    active: usize,
    blocked: usize,
    failed: usize,
}

fn render_agent_stacked_bar(spec: AgentBarSpec) -> View {
    let AgentBarSpec {
        runs,
        max_runs,
        working_ms,
        max_work_ms,
        completed,
        active,
        blocked,
        failed,
    } = spec;
    let ratio = if runs > 0 && max_runs > 0 {
        (runs as f32 / max_runs as f32).clamp(0.05, 1.0)
    } else if working_ms > 0 && max_work_ms > 0 {
        (working_ms as f32 / max_work_ms as f32).clamp(0.05, 1.0)
    } else {
        0.0
    };
    let segs: [(usize, AdaptiveColor); 4] = [
        (completed, SUCCESS.resolve()),
        (active, ACCENT.resolve()),
        (blocked, ATTENTION.resolve()),
        (failed, DANGER.resolve()),
    ];
    let filled_total = completed + active + blocked + failed;
    let filled = if filled_total == 0 {
        view()
            .flex_grow(ratio)
            .h_full()
            .rounded(px(3.5))
            .bg(if working_ms > 0 {
                ACCENT.resolve()
            } else {
                alpha_tint(MUTED, 0.45)
            })
    } else {
        view()
            .flex_grow(ratio)
            .h_full()
            .rounded(px(3.5))
            .overflow_hidden()
            .flex_row()
            .gap(px(1.))
            .children(
                segs.into_iter()
                    .filter(|(v, _)| *v > 0)
                    .map(|(v, col)| view().flex_grow(v as f32).h_full().bg(col).into_element())
                    .collect::<Vec<_>>(),
            )
    };

    view()
        .w_full()
        .h(px(8.))
        .rounded(px(4.))
        .bg(BACKGROUND)
        .overflow_hidden()
        .flex_row()
        .tooltip(tooltip_box().child(note(format!(
            "{completed} completed · {active} active · {blocked} blocked · {failed} failed"
        ))))
        .child(filled)
        .child(view().flex_grow((1.0 - ratio).max(0.0)).h_full())
}

fn format_q_delta(cur: Option<u64>, prev: Option<u64>) -> String {
    format_duration_delta(cur, prev)
        .map(|(txt, _)| format!(" ({txt})"))
        .unwrap_or_default()
}

fn render_agents(ui: Ui, dash: &Dashboard) -> View {
    let agent_group = ui.insights.agent_group.get();
    let mut panel = card_surface()
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(heading("Agents used"))
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
        )
        .child(
            row()
                .child(legend_swatch(SUCCESS, "Completed"))
                .child(legend_swatch(ACCENT, "Active"))
                .child(legend_swatch(ATTENTION, "Blocked"))
                .child(legend_swatch(DANGER, "Failed")),
        );

    match agent_group {
        AgentGroup::Profile => {
            let max_runs = dash
                .profile_stats
                .iter()
                .map(|p| p.total_runs)
                .max()
                .unwrap_or(0);
            let max_work = dash
                .profile_stats
                .iter()
                .map(|p| p.working_ms)
                .max()
                .unwrap_or(1)
                .max(1);
            for p in &dash.profile_stats {
                panel = panel.child(render_profile_agent_row(p, max_runs, max_work));
            }
            if dash.profile_stats.is_empty() {
                panel = panel.child(note(
                    "No Agent execution or working time observed in this period.",
                ));
            }
        }
        AgentGroup::Model => {
            let max_runs = dash
                .model_stats
                .iter()
                .map(|m| m.total_runs)
                .max()
                .unwrap_or(0);
            let max_work = dash
                .model_stats
                .iter()
                .map(|m| m.working_ms)
                .max()
                .unwrap_or(1)
                .max(1);
            for m in &dash.model_stats {
                panel = panel.child(render_model_agent_row(m, max_runs, max_work));
            }
            if dash.model_stats.is_empty() {
                panel = panel.child(note(
                    "No Agent execution or working time observed in this period.",
                ));
            }
        }
    }

    let trigger_color = |kind: model::TriggerKind| match kind {
        model::TriggerKind::Workflow => ACCENT.resolve(),
        model::TriggerKind::Mention => SUCCESS.resolve(),
        model::TriggerKind::Interactive => ATTENTION.resolve(),
    };
    let trigger_segments: Vec<(usize, AdaptiveColor)> = dash
        .trigger_stats
        .iter()
        .map(|s| (s.count, trigger_color(s.kind)))
        .collect();
    let trigger_legend = row().children(
        dash.trigger_stats
            .iter()
            .map(|s| {
                let col = trigger_color(s.kind);
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(view().w(px(7.)).h(px(7.)).rounded(px(2.)).bg(col))
                    .child(note(format!(
                        "{}: {} ({}%)",
                        s.kind.label(),
                        s.count,
                        s.share_pct
                    )))
                    .into_element()
            })
            .collect::<Vec<_>>(),
    );

    panel
        .child(view().w_full().h(px(1.)).bg(BORDER))
        .child(
            view()
                .gap_1p5()
                .child(note("Trigger sources"))
                .child(segmented_bar(&trigger_segments))
                .child(trigger_legend),
        )
        .child(note(
            "Working duration reflects live and historical Herdr working intervals. Queue p50 compares against the baseline period.",
        ))
}

fn render_profile_agent_row(p: &ProfileStat, max_runs: usize, max_work: u64) -> View {
    let pass_pct = p.pass_rate_pct();
    let pass_tone = match pass_pct {
        Some(pct) if pct >= 85 => Tone::Success,
        Some(pct) if pct < 60 => Tone::Attention,
        _ => Tone::Neutral,
    };
    let pass_txt = pass_pct
        .map(|pct| format!("{pct}% pass"))
        .unwrap_or_else(|| {
            if p.active_runs > 0 {
                "active".into()
            } else {
                "—".into()
            }
        });
    let exec_str = p
        .avg_exec_ms
        .map(format_duration_compact)
        .unwrap_or_else(|| "—".into());
    let q_str = p
        .median_queue_wait_ms
        .map(format_duration_compact)
        .unwrap_or_else(|| "—".into());
    let q_delta = format_q_delta(p.median_queue_wait_ms, p.prev_median_queue_wait_ms);
    let reply_suffix = p
        .avg_mention_reply_ms
        .map(|ms| format!(" · reply avg {}", format_duration_compact(ms)))
        .unwrap_or_default();
    let title = if p.profile_id == "manual" {
        "Manual".to_owned()
    } else {
        format!("@{}", p.profile_id)
    };

    view()
        .w_full()
        .p_2p5()
        .gap_1p5()
        .rounded(px(6.))
        .bg(INSET)
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
                        .items_center()
                        .gap_1p5()
                        .child(label(title).font_weight(FontWeight::MEDIUM).truncate())
                        .child(note(format!("({} · {})", p.provider, p.model))),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(mono_val(
                            format!(
                                "{} · {} runs",
                                format_duration_compact(p.working_ms),
                                p.total_runs
                            ),
                            11.5,
                        ))
                        .child(status(pass_txt, pass_tone)),
                ),
        )
        .child(render_agent_stacked_bar(AgentBarSpec {
            runs: p.total_runs,
            max_runs,
            working_ms: p.working_ms,
            max_work_ms: max_work,
            completed: p.completed_runs,
            active: p.active_runs,
            blocked: p.blocked_runs,
            failed: p.failed_runs,
        }))
        .child(note(format!(
            "{} Cards · Q p50 {q_str}{q_delta} · {} asks · exec avg {exec_str}{reply_suffix}",
            p.cards_touched, p.human_asks
        )))
}

fn render_model_agent_row(m: &ModelAgentStat, max_runs: usize, max_work: u64) -> View {
    let pass_pct = m.pass_rate_pct();
    let pass_tone = match pass_pct {
        Some(pct) if pct >= 85 => Tone::Success,
        Some(pct) if pct < 60 => Tone::Attention,
        _ => Tone::Neutral,
    };
    let pass_txt = pass_pct
        .map(|pct| format!("{pct}% pass"))
        .unwrap_or_else(|| {
            if m.active_runs > 0 {
                "active".into()
            } else {
                "—".into()
            }
        });
    let providers = if m.providers.is_empty() {
        "—".into()
    } else {
        m.providers.iter().cloned().collect::<Vec<_>>().join(", ")
    };
    let q_str = m
        .median_queue_wait_ms
        .map(format_duration_compact)
        .unwrap_or_else(|| "—".into());
    let q_delta = format_q_delta(m.median_queue_wait_ms, m.prev_median_queue_wait_ms);

    view()
        .w_full()
        .p_2p5()
        .gap_1p5()
        .rounded(px(6.))
        .bg(INSET)
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
                        .items_center()
                        .gap_1p5()
                        .child(
                            label(m.model.clone())
                                .font_weight(FontWeight::MEDIUM)
                                .truncate(),
                        )
                        .child(note(format!(
                            "({} · {} profiles)",
                            providers,
                            m.profiles.len()
                        ))),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(mono_val(
                            format!(
                                "{} · {} runs",
                                format_duration_compact(m.working_ms),
                                m.total_runs
                            ),
                            11.5,
                        ))
                        .child(status(pass_txt, pass_tone)),
                ),
        )
        .child(render_agent_stacked_bar(AgentBarSpec {
            runs: m.total_runs,
            max_runs,
            working_ms: m.working_ms,
            max_work_ms: max_work,
            completed: m.completed_runs,
            active: m.active_runs,
            blocked: m.blocked_runs,
            failed: m.failed_runs,
        }))
        .child(note(format!(
            "{} Cards · Q p50 {q_str}{q_delta} · {} asks",
            m.cards_touched, m.human_asks
        )))
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
    let annual_scroll = element_ref();
    let cell_size = 14.;
    let gap = 3.;
    let headers = dash
        .heatmap_weeks
        .iter()
        .map(|w| {
            view()
                .w(px(cell_size))
                .h(px(16.))
                .flex_shrink_0()
                .child(
                    note(w.month_header.clone().unwrap_or_default())
                        .w(px(32.))
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
                                        .rounded(px(3.))
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
    let grid = view()
        .min_w(px(dash.heatmap_weeks.len() as f32 * (cell_size + gap) + 32.))
        .gap_1()
        .child(view().flex_row().pl(px(32.)).gap(px(gap)).children(headers))
        .child(
            view()
                .flex_row()
                .child(
                    view().w(px(32.)).gap(px(gap)).children(
                        ["Mon", "", "Wed", "", "Fri", "", "Sun"]
                            .into_iter()
                            .map(|d| {
                                view()
                                    .h(px(cell_size))
                                    .child(note(d).text_size(px(9.)))
                                    .into_element()
                            })
                            .collect::<Vec<_>>(),
                    ),
                )
                .child(view().flex_row().gap(px(gap)).children(columns)),
        );

    let hm_metric = ui.insights.heatmap_metric.get();
    card_surface()
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(heading("Yearly activity"))
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
                ),
        )
        .child(note(format!(
            "{} – {} · Last 365 days · {}",
            dash.annual_start,
            dash.annual_end,
            hm_metric.label()
        )))
        .child(
            scroll()
                .horizontal()
                .w_full()
                .element_ref(&annual_scroll)
                .content_offset(Point::new(px(10_000.), px(0.)))
                .on_layout(move |bounds: &Bounds<Points>| {
                    let width = bounds.size.width.0;
                    if (ui.insights.annual_width.get_untracked() - width).abs() > 0.5 {
                        ui.insights.annual_width.set(width);
                        annual_scroll.scroll_to_end(false);
                    }
                })
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
                    "{} active days · Current streak: {}d · Longest: {}d",
                    dash.active_days, dash.current_streak, dash.longest_streak
                )))
                .child(
                    row()
                        .child(note("Less"))
                        .children(
                            (0..=4)
                                .map(|n| {
                                    view()
                                        .w(px(10.))
                                        .h(px(10.))
                                        .rounded(px(2.))
                                        .bg(heatmap_level_bg(n))
                                        .into_element()
                                })
                                .collect::<Vec<_>>(),
                        )
                        .child(note("More")),
                ),
        )
        .child(note(
            "Select a tile to inspect that date. Faded tiles precede retained history. Project scope applies.",
        ))
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
    let delta = format_duration_delta(p50, prev_p50);
    let inspect_label = if active_tab == tab_id {
        "Hide samples"
    } else {
        "Inspect"
    };

    let delta_view = if let Some((txt, faster)) = delta {
        status(txt, if faster { Tone::Success } else { Tone::Neutral }).into_element()
    } else {
        mono_val("—", 11.5).into_element()
    };

    view()
        .w_full()
        .py_1p5()
        .px_2()
        .rounded(px(5.))
        .bg(INSET)
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(
            label(title)
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .w(px(180.)),
        )
        .child(mono_val(p50_txt, 12.).w(px(64.)))
        .child(mono_val(p90_txt, 12.).w(px(64.)))
        .child(mono_val(prev_txt, 11.5).w(px(86.)))
        .child(view().w(px(120.)).child(delta_view))
        .child(mono_val(samples.len().to_string(), 11.5).w(px(64.)))
        .child(view().flex_1().items_end().child(compact_action(
            inspect_label,
            if active_tab == tab_id {
                Variant::Secondary
            } else {
                Variant::Quiet
            },
            move |_| intent(ui, op),
        )))
}

fn render_timings(ui: Ui, dash: &Dashboard) -> View {
    let tab = ui.insights.timing_tab.get();
    let samples: &[DurationSample] = match tab.as_str() {
        "cycle" => &dash.efficiency.cycle,
        "queue" => &dash.efficiency.queue,
        _ => &[],
    };

    let t = &dash.timings;
    let max_avg = [
        t.develop_stage.avg_ms,
        t.review_stage.avg_ms,
        t.merge_queue.avg_ms,
        t.mention_reply.avg_ms,
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or(1)
    .max(1);

    let stage_row = |title: &'static str,
                     stat: &model::TimingStat,
                     color: AdaptiveColor,
                     extra_detail: Option<String>| {
        let ratio = stat
            .avg_ms
            .map(|ms| (ms as f32 / max_avg as f32).clamp(0.05, 1.0))
            .unwrap_or(0.0);
        let avg_txt = stat
            .avg_ms
            .map(format_duration_compact)
            .unwrap_or_else(|| "—".into());
        let p50_txt = stat
            .p50_ms
            .map(format_duration_compact)
            .unwrap_or_else(|| "—".into());
        let delta = format_duration_delta(stat.avg_ms, stat.prev_avg_ms);
        let mut detail = format!("p50 {p50_txt} · n={}", stat.samples);
        if let Some(extra) = extra_detail {
            detail.push_str(" · ");
            detail.push_str(&extra);
        }

        let mut header =
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .child(view().w(px(7.)).h(px(7.)).rounded(px(2.)).bg(color))
                        .child(
                            label(title)
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM),
                        ),
                )
                .child(view().flex_row().items_center().gap_1p5().child(
                    mono_val(format!("avg {avg_txt}"), 11.5).font_weight(FontWeight::SEMIBOLD),
                ));
        if let Some((delta_txt, faster)) = delta {
            header = header.child(status(
                delta_txt,
                if faster { Tone::Success } else { Tone::Neutral },
            ));
        }

        view()
            .flex_1()
            .min_w(px(220.))
            .gap_1()
            .child(header)
            .child(
                view()
                    .w_full()
                    .h(px(7.))
                    .rounded(px(3.5))
                    .bg(INSET)
                    .overflow_hidden()
                    .flex_row()
                    .child(
                        view()
                            .flex_grow(ratio.max(0.0))
                            .h_full()
                            .rounded(px(3.5))
                            .bg(color),
                    )
                    .child(view().flex_grow((1.0 - ratio).max(0.0)).h_full()),
            )
            .child(note(detail))
    };

    let dev_exec_detail = t.develop_exec.avg_ms.map(|ms| {
        format!(
            "Agent exec avg {} (n={})",
            format_duration_compact(ms),
            t.develop_exec.samples
        )
    });

    let clean_pill = t
        .clean_merge_rate_pct()
        .map(|pct| {
            let tone = if pct >= 85 {
                Tone::Success
            } else if pct < 60 {
                Tone::Attention
            } else {
                Tone::Neutral
            };
            status(format!("{pct}% clean"), tone)
        })
        .unwrap_or_else(|| status("No batches", Tone::Neutral));

    let batch_size_str = t
        .merge_queue_avg_batch_size
        .map(|sz| format!("{sz:.1} cards/batch"))
        .unwrap_or_else(|| "— cards/batch".into());
    let conflict_str = t
        .merge_queue_conflict_avg_ms
        .map(|ms| format!(" · Conflict resolve avg {}", format_duration_compact(ms)))
        .unwrap_or_default();

    let mut panel = card_surface()
        .child(view().gap_0p5().child(heading("Timings")).child(note(
            "Delivery and queue percentiles plus stage averages and merge queue efficiency",
        )))
        .child(
            view()
                .gap_1p5()
                .child(
                    view()
                        .px_2()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(note("Metric").w(px(180.)))
                        .child(note("p50").w(px(64.)))
                        .child(note("p90").w(px(64.)))
                        .child(note("Baseline p50").w(px(86.)))
                        .child(note("Δ").w(px(120.)))
                        .child(note("Samples").w(px(64.)))
                        .child(view().flex_1()),
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
        .child(view().w_full().h(px(1.)).bg(BORDER))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap_4()
                .child(stage_row(
                    "Develop",
                    &t.develop_stage,
                    ACCENT.resolve(),
                    dev_exec_detail,
                ))
                .child(stage_row(
                    "Review",
                    &t.review_stage,
                    SUCCESS.resolve(),
                    None,
                ))
                .child(stage_row(
                    "Merge Queue",
                    &t.merge_queue,
                    ATTENTION.resolve(),
                    None,
                ))
                .child(stage_row(
                    "@mention Reply",
                    &t.mention_reply,
                    alpha_tint(ACCENT, 0.7),
                    None,
                )),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(note(format!(
                    "Merge queue efficiency: {} batches ({} clean · {} conflicts) · Avg {batch_size_str}{conflict_str}",
                    t.merge_queue_batches,
                    t.merge_queue_clean_batches,
                    t.merge_queue_conflict_batches
                )))
                .child(clean_pill),
        )
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

    view()
        .gap_2()
        .child(
            view()
                .p_3()
                .bg(SURFACE)
                .rounded(px(RADIUS))
                .border_1()
                .border_color(BORDER)
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(note(format!(
                    "Intake: {} local · {} GitHub PR · {} GitHub Issue · {} initial sync",
                    intake.local_manual,
                    intake.github_pr,
                    intake.github_issue,
                    intake.initial_sync
                )))
                .child(choice(
                    ui,
                    if excl {
                        "Exclude initial sync"
                    } else {
                        "Include initial sync"
                    },
                    "toggle-initial-sync",
                    excl,
                )),
        )
        .child(note(format!(
            "Retained history from {} · {} resolved Cards have no reliable resolution date and are excluded from date totals. {} Agent batches have no retained launch timestamp.{}",
            dash.history_start
                .map(|d| d.to_string())
                .unwrap_or_else(|| "—".into()),
            dash.unknown_merge_dates,
            dash.unknown_batch_starts,
            if offline.is_empty() {
                String::new()
            } else {
                format!(" Offline hosts: {offline}; cached data may be stale.")
            }
        )))
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

fn dashboard(ui: Ui) -> Dashboard {
    let scope = ui.insights.scope_project.get();
    let range = ui.insights.range.get();
    let now = now_ms(ui);
    let profiles = ui.all_profiles.get();
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
    ui.insights.activity.with(|act| {
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
    })
}

pub fn screen(ui: Ui) -> impl IntoElement {
    effect(move || {
        let page = ui.page.get();
        let _scope = ui.insights.scope_project.get();
        let _range = ui.insights.range.get();
        let _date = ui.insights.selected_date.get();
        let _cmp = ui.insights.comparison.get();
        let _hm = ui.insights.heatmap_metric.get();
        let _clock = ui.utility_clock.get();
        let _gen = ui.index.with(|idx| idx.generation);
        if page == "insights" {
            refresh_events(ui);
            refresh_activity(ui);
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
                            let dash = dashboard.get();
                            let narrow = ui.narrow.get();
                            let project_order: Vec<String> =
                                dash.project_stats.iter().map(|p| p.id.clone()).collect();

                            let mut content = view()
                                .gap_4()
                                .min_w(px(0.))
                                .child(render_period_subbar(ui, &dash))
                                .child(render_summary_and_kpis(&dash, narrow));

                            if !ui.insights.drilldown.get().is_empty() {
                                content = content.child(render_card_drilldown(ui, &dash));
                            }

                            content = content.child(render_timeline(ui, &dash, &project_order));

                            let output_panel = if dash.range == RangeWindow::Today {
                                render_hourly_output(ui, &dash)
                            } else {
                                render_daily_output(ui, &dash)
                            };
                            content = content.child(overview_row(
                                narrow,
                                output_panel,
                                render_automation_funnel(&dash),
                                render_projects(ui, &dash, &project_order),
                            ));

                            content = content
                                .child(render_agents(ui, &dash))
                                .child(render_heatmap_panel(ui, &dash))
                                .child(render_timings(ui, &dash))
                                .child(render_footer(ui, &dash));

                            content.into_element()
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

    #[test]
    fn insights_route_mounts_timeline_first_cockpit_and_switches_agent_groups_and_ranges() {
        #[component]
        fn InsightsHarness() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let now_ms = 1_794_744_000_000u64;
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
                    id: "ev-2".into(),
                    project_id: "p1".into(),
                    card_id: "card-41".into(),
                    sequence: 2,
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
            ui.project.set("p1".into());
            ui.page.set("insights".into());

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
        assert!(tree.has_mounted_text("RESOLVED"));
        assert!(tree.has_mounted_text("MERGES (LOCAL / PR)"));
        assert!(tree.has_mounted_text("MY TIME"));
        assert!(tree.has_mounted_text("AGENT WORKING"));
        assert!(tree.has_mounted_text("Activity timeline"));
        assert!(tree.has_mounted_text("Hourly output"));
        assert!(tree.has_mounted_text("Automation funnel"));
        assert!(tree.has_mounted_text("Projects"));
        assert!(tree.has_mounted_text("Agents used"));
        assert!(tree.has_mounted_text("Yearly activity"));
        assert!(tree.has_mounted_text("Timings"));
        assert!(tree.has_mounted_text("Execution → resolution"));
        assert!(tree.has_mounted_text("Queue → start"));
        assert!(!tree.has_mounted_text("▸ Efficiency & timing percentiles"));
        assert!(!tree.has_mounted_text("Daily"));

        // Switch Agents grouping to Model
        let btn_model = *tree.labels.get("Model").expect("Model group button");
        assert!(core.component_event(btn_model, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("gpt-5.4"));

        // Switch range to 7D via segmented button
        let btn_7d = *tree.labels.get("7D").expect("7D segment button");
        assert!(core.component_event(btn_7d, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("Daily activity"));
        assert!(tree.has_mounted_text("Daily output"));
        assert!(!tree.has_mounted_text("Hourly output"));

        // Switch heatmap metric to My time
        let btn_my_time = *tree
            .labels
            .get("My time")
            .expect("My time heatmap metric button");
        assert!(core.component_event(btn_my_time, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("Yearly activity"));
    }
}
