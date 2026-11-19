//! Multi-dimensional Insights cockpit: delivery throughput, Kanban WIP pipeline,
//! Agent execution quality, automation health, and Day-over-Day / Week-over-Week
//! personal efficiency trajectories.

pub mod model;

use crate::{
    drafts::{capture_scroll, scroll_position},
    presenter::*,
};
use canban_ui_kit::{tokens::*, *};
use model::{
    ActivityMetric, Comparison, Dashboard, DayBucket, ProjectSort, RangeWindow, TrendCadence,
    WeekBucket, format_delta_pct, format_duration_compact, format_duration_delta,
};
use nsh_core::{element::AdaptiveColor, prelude::*};

const ALL_PROJECTS_OPTION: &str = "__all__";
const CHART_HEIGHT: f32 = 148.0;

#[derive(Clone, Copy)]
pub struct State {
    /// Empty string means `All Projects`; otherwise holds a `Project.id`.
    pub scope_project: Signal<String>,
    pub range: Signal<RangeWindow>,
    pub selected_date: Signal<Option<chrono::NaiveDate>>,
    pub date_text: Signal<String>,
    pub comparison: Signal<Comparison>,
    pub metric: Signal<ActivityMetric>,
    pub hourly_metric: Signal<ActivityMetric>,
    pub trend_days: Signal<i64>,
    pub trend_weeks: Signal<i64>,
    pub cadence: Signal<TrendCadence>,
    pub exclude_initial_sync: Signal<bool>,
    pub scroll: Signal<Option<ElementRef>>,
    pub annual_width: Signal<f32>,
    pub provider_group: Signal<bool>,
    pub project_sort: Signal<ProjectSort>,
    pub drilldown: Signal<String>,
    /// Optional fixed timestamp (ms) for deterministic Gallery/test rendering; `None` uses live clock.
    pub anchor_ms: Signal<Option<u64>>,
    pub events: Signal<std::rc::Rc<Vec<canban_types::Event>>>,
    pub events_loading: Signal<bool>,
    pub events_key: Signal<Option<(String, u64)>>,
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
            metric: signal(ActivityMetric::Merged),
            hourly_metric: signal(ActivityMetric::Merged),
            trend_days: signal(14),
            trend_weeks: signal(8),
            cadence: signal(TrendCadence::Daily),
            exclude_initial_sync: signal(true),
            scroll: signal(None),
            annual_width: signal(0.),
            provider_group: signal(false),
            project_sort: signal(ProjectSort::Merged),
            drilldown: signal(String::new()),
            anchor_ms: signal(None),
            events: signal(std::rc::Rc::new(Vec::new())),
            events_loading: signal(false),
            events_key: signal(None),
        }
    }
}

pub fn refresh_events(ui: Ui) {
    if crate::APP.get().is_none() || ui.page.get_untracked() != "insights" {
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
                ui.insights.events.set(std::rc::Rc::new(events));
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

fn now_ms(ui: Ui) -> u64 {
    ui.insights.anchor_ms.get().unwrap_or_else(|| {
        ui.runtime_clock.get();
        canban_types::now()
    })
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
        "metric-merged" => ui.insights.metric.set(ActivityMetric::Merged),
        "metric-created" => ui.insights.metric.set(ActivityMetric::Created),
        "metric-runs" => ui.insights.metric.set(ActivityMetric::Runs),
        "hourly-merged" => ui.insights.hourly_metric.set(ActivityMetric::Merged),
        "hourly-created" => ui.insights.hourly_metric.set(ActivityMetric::Created),
        "hourly-runs" => ui.insights.hourly_metric.set(ActivityMetric::Runs),
        "cadence-daily" => ui.insights.cadence.set(TrendCadence::Daily),
        "cadence-weekly" => ui.insights.cadence.set(TrendCadence::Weekly),
        "trend-14" => {
            ui.insights.cadence.set(TrendCadence::Daily);
            ui.insights.trend_days.set(14);
        }
        "trend-30" => {
            ui.insights.cadence.set(TrendCadence::Daily);
            ui.insights.trend_days.set(30);
        }
        "weeks-8" => {
            ui.insights.cadence.set(TrendCadence::Weekly);
            ui.insights.trend_weeks.set(8);
        }
        "weeks-12" => {
            ui.insights.cadence.set(TrendCadence::Weekly);
            ui.insights.trend_weeks.set(12);
        }
        "toggle-initial-sync" => ui.insights.exclude_initial_sync.update(|v| *v = !*v),
        "group-provider" => ui.insights.provider_group.set(true),
        "group-profile" => ui.insights.provider_group.set(false),
        "sort-created" => ui.insights.project_sort.set(ProjectSort::Created),
        "sort-merged" => ui.insights.project_sort.set(ProjectSort::Merged),
        "sort-wip" => ui.insights.project_sort.set(ProjectSort::Wip),
        "sort-runs" => ui.insights.project_sort.set(ProjectSort::Runs),
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

    let cadence_bar = view()
        .flex_row()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(7.))
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .child(
            compact_segment(
                "Daily",
                None,
                move || ui.insights.cadence.get() == TrendCadence::Daily,
                move |_| intent(ui, "cadence-daily"),
            )
            .min_h(px(22.))
            .px_2(),
        )
        .child(
            compact_segment(
                "Weekly",
                None,
                move || ui.insights.cadence.get() == TrendCadence::Weekly,
                move |_| intent(ui, "cadence-weekly"),
            )
            .min_h(px(22.))
            .px_2(),
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
                .child(range_bar)
                .child(cadence_bar),
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

fn legend_swatch(color: impl IntoReactive<AdaptiveColor> + 'static, title: &'static str) -> View {
    view()
        .flex_row()
        .items_center()
        .gap_1p5()
        .child(view().w(px(8.)).h(px(8.)).rounded(px(2.)).bg(color))
        .child(muted(title).text_size(px(11.5)))
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

fn metric_choices(ui: Ui, annual: bool) -> View {
    let metric = if annual {
        ui.insights.metric.get()
    } else {
        ui.insights.hourly_metric.get()
    };
    row()
        .child(choice(
            ui,
            "Resolved",
            if annual {
                "metric-merged"
            } else {
                "hourly-merged"
            },
            metric == ActivityMetric::Merged,
        ))
        .child(choice(
            ui,
            "Created",
            if annual {
                "metric-created"
            } else {
                "hourly-created"
            },
            metric == ActivityMetric::Created,
        ))
        .child(choice(
            ui,
            "Agent starts",
            if annual { "metric-runs" } else { "hourly-runs" },
            metric == ActivityMetric::Runs,
        ))
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
    let valid_date =
        chrono::NaiveDate::parse_from_str(&ui.insights.date_text.get(), "%Y-%m-%d").ok();
    let excl_sync = ui.insights.exclude_initial_sync.get();

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
                        })
                        .child(choice(
                            ui,
                            if excl_sync {
                                "Organic intake (excl. sync)"
                            } else {
                                "All intake (incl. sync)"
                            },
                            "toggle-initial-sync",
                            excl_sync,
                        )),
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
                            .disabled(valid_date.is_none_or(|d| d > actual)),
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

fn render_kpis(dash: &Dashboard, narrow: bool) -> View {
    let known = dash.history_start.is_some_and(|d| d <= dash.selected_date);
    let ref_line = |value: Option<f64>| {
        value
            .map(|n| format!("7D pace: {n:.1}/day ({}d)", dash.reference_days))
            .unwrap_or_else(|| "7D pace: insufficient history".into())
    };

    // 1. Delivery Throughput
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

    // 2. Pipeline & WIP Health
    let net_sign = if dash.net_wip_delta > 0 { "+" } else { "" };
    let wip_tone = if dash.pipeline.human_gated > 0 || dash.pipeline.conflict_batches_period > 0 {
        Tone::Attention
    } else {
        Tone::Neutral
    };
    let wip_pill = if dash.pipeline.human_gated > 0 {
        format!("{} gated", dash.pipeline.human_gated)
    } else {
        format!("Net {net_sign}{}", dash.net_wip_delta)
    };

    // 3. Agent Execution Quality
    let pass_str = dash
        .success_rate_pct
        .map(|p| format!("{p}% pass"))
        .unwrap_or_else(|| "No resolved".into());
    let pass_tone = match dash.success_rate_pct {
        Some(p) if p >= 85 => Tone::Success,
        Some(p) if p < 60 => Tone::Attention,
        _ => Tone::Neutral,
    };

    // 4. Cycle Efficiency & Automation
    let cyc_hero = if !known {
        "—".into()
    } else {
        dash.median_lead_time_ms
            .map(|ms| format!("{} p50", format_duration_compact(ms)))
            .unwrap_or_else(|| "— p50".into())
    };
    let cyc_pill = format_duration_delta(dash.median_lead_time_ms, dash.prev_median_lead_time_ms)
        .map(|(txt, faster)| (txt, if faster { Tone::Success } else { Tone::Neutral }))
        .unwrap_or_else(|| ("Cycle time".into(), Tone::Neutral));
    let wf_share = dash.trigger_stats.first().map(|t| t.share_pct).unwrap_or(0);
    let touches_per_card = if dash.merged_total > 0 {
        format!(
            "{:.1} asks/resolution",
            dash.human_asks_period as f64 / dash.merged_total as f64
        )
    } else {
        format!("{} human asks", dash.human_asks_period)
    };

    let cards = [
        KpiSpec {
            mark: Icon::GitMerge,
            mark_tint: SUCCESS,
            eyebrow: "RESOLUTION THROUGHPUT",
            hero: if known {
                format!("{} resolved", dash.merged_total)
            } else {
                "—".into()
            },
            pill_text: Some((delivery_pill, Tone::Neutral)),
            bar_segments: vec![
                (dash.local_merges, SUCCESS.resolve()),
                (dash.pr_merges, ACCENT.resolve()),
                (dash.manual_resolutions, ATTENTION.resolve()),
            ],
            sub_primary: format!(
                "{} local merges · {} PR merges{} · +{} created{}",
                dash.local_merges,
                dash.pr_merges,
                non_merge_breakdown(dash.manual_resolutions),
                dash.created_total,
                sync_suffix
            ),
            sub_secondary: if dash.range == RangeWindow::Today {
                format!(
                    "Prev: {} resolved · {}",
                    if dash.baseline_known {
                        dash.prev_merged_total.to_string()
                    } else {
                        "—".into()
                    },
                    ref_line(dash.reference_merged)
                )
            } else {
                format!(
                    "Prev period: {} resolved · {} created",
                    if dash.baseline_known {
                        dash.prev_merged_total.to_string()
                    } else {
                        "—".into()
                    },
                    if dash.baseline_known {
                        dash.previous_created.to_string()
                    } else {
                        "—".into()
                    }
                )
            },
        },
        KpiSpec {
            mark: Icon::Board,
            mark_tint: ACCENT,
            eyebrow: "PIPELINE & WIP HEALTH",
            hero: if known {
                format!("{} active WIP", dash.pipeline.active_wip)
            } else {
                "—".into()
            },
            pill_text: Some((wip_pill, wip_tone)),
            bar_segments: vec![
                (dash.pipeline.backlog, alpha_tint(MUTED, 0.55)),
                (dash.pipeline.develop, ACCENT.resolve()),
                (dash.pipeline.review, SUCCESS.resolve()),
            ],
            sub_primary: format!(
                "Backlog {} · Develop {} · Review {}",
                dash.pipeline.backlog, dash.pipeline.develop, dash.pipeline.review
            ),
            sub_secondary: format!(
                "{} Projects active · {} merge conflicts",
                dash.project_stats.len(),
                dash.pipeline.conflict_batches_period
            ),
        },
        KpiSpec {
            mark: Icon::Agent,
            mark_tint: ACCENT,
            eyebrow: "AGENT EXECUTION QUALITY",
            hero: if known {
                format!("{} starts", dash.scheduled_total)
            } else {
                "—".into()
            },
            pill_text: Some((pass_str, pass_tone)),
            bar_segments: vec![
                (dash.completed_runs, SUCCESS.resolve()),
                (dash.active_runs, ACCENT.resolve()),
                (dash.blocked_runs, ATTENTION.resolve()),
                (dash.failed_runs, DANGER.resolve()),
            ],
            sub_primary: format!(
                "{} ok · {} blk · {} fail · {} active",
                dash.completed_runs, dash.blocked_runs, dash.failed_runs, dash.active_runs
            ),
            sub_secondary: format!(
                "{} profiles · {} providers · {} overlapping",
                dash.used_profiles, dash.providers, dash.used_runs
            ),
        },
        KpiSpec {
            mark: Icon::Zap,
            mark_tint: ATTENTION,
            eyebrow: "EFFICIENCY & AUTOMATION",
            hero: cyc_hero,
            pill_text: Some(cyc_pill),
            bar_segments: vec![
                (wf_share.max(1), ACCENT.resolve()),
                (100usize.saturating_sub(wf_share), alpha_tint(MUTED, 0.35)),
            ],
            sub_primary: format!(
                "Queue p50: {} · Auto WF: {}%",
                dash.median_queue_wait_ms
                    .map(format_duration_compact)
                    .unwrap_or_else(|| "—".into()),
                wf_share
            ),
            sub_secondary: if dash.schedules.configured > 0 || dash.schedules.runs_in_period > 0 {
                format!(
                    "{} · Sched: {}/{} ok",
                    touches_per_card, dash.schedules.completed, dash.schedules.runs_in_period
                )
            } else {
                format!(
                    "{} · {} WF runs completed",
                    touches_per_card, dash.workflow_runs_completed
                )
            },
        },
    ]
    .into_iter()
    .map(kpi_card)
    .collect::<Vec<_>>();

    let mut iter = cards.into_iter();
    if narrow {
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

fn render_daily_velocity_chart(ui: Ui, dash: &Dashboard) -> View {
    let top_tick = nice_ceil_tick(dash.max_daily_kanban);
    let n = dash.days.len();
    let col_gap = if n > 45 {
        1.0
    } else if n > 14 {
        3.0
    } else {
        8.0
    };
    let bar_max_w = if n > 45 {
        4.0
    } else if n > 14 {
        8.0
    } else {
        18.0
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
            let tip_title = format!("{} ({})", day.date_key, day.weekday_label);
            let tip_merged = format!(
                "Resolved: {} ({} local merges · {} PR merges{}) · Cycle p50: {}",
                day.merged_cards,
                day.local_merges,
                day.pr_merges,
                non_merge_breakdown(day.manual_resolutions),
                day.cycle_p50_ms
                    .map(format_duration_compact)
                    .unwrap_or_else(|| "—".into())
            );
            let avg = dash.moving_average[day_index];
            let sync_note = if day.initial_sync_cards > 0 && dash.exclude_initial_sync {
                format!(" (+{} sync muted)", day.initial_sync_cards)
            } else {
                String::new()
            };
            let tip_created = format!(
                "Created: {}{} · Runs: {} · 7D mean: {}{}",
                day.created_cards,
                sync_note,
                day.total_runs(),
                avg.map(|v| format!("{v:.1}")).unwrap_or_else(|| "—".into()),
                if day.date_key == dash.annual_end.to_string() {
                    " · In progress"
                } else {
                    ""
                }
            );
            let date_parsed = chrono::NaiveDate::parse_from_str(&day.date_key, "%Y-%m-%d").ok();

            view()
                .flex_1()
                .min_w(px(0.))
                .h(px(CHART_HEIGHT))
                .tooltip(
                    tooltip_box()
                        .gap_0p5()
                        .child(
                            text(tip_title)
                                .font_family(MONO_FONT)
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(TEXT),
                        )
                        .child(label(tip_merged).text_size(px(11.5)).text_color(SUCCESS))
                        .child(muted(tip_created).text_size(px(11.5))),
                )
                .child(
                    press_target()
                        .w_full()
                        .h_full()
                        .rounded(px(4.))
                        .hover(|s| s.bg(CARD_HOVER))
                        .justify_end()
                        .items_center()
                        .px(px(if n > 45 { 0.0 } else { 1.0 }))
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
        .min_w(px(320.))
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
                        .child(heading("Recent daily flow"))
                        .child(note(
                            "Daily created / resolved · amber tick: 7-day complete mean · click bar to inspect day",
                        )),
                )
                .child(
                    row()
                        .child(choice(ui, "Daily", "cadence-daily", true))
                        .child(choice(ui, "Weekly", "cadence-weekly", false)),
                ),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(if dash.range == RangeWindow::Today {
                    row()
                        .child(choice(
                            ui,
                            "Recent 14 days",
                            "trend-14",
                            ui.insights.trend_days.get() == 14,
                        ))
                        .child(choice(
                            ui,
                            "Recent 30 days",
                            "trend-30",
                            ui.insights.trend_days.get() == 30,
                        ))
                        .into_element()
                } else {
                    note(format!("Showing {} days in selected range", dash.days.len())).into_element()
                })
                .child(
                    row()
                        .child(legend_swatch(move || alpha_tint(ACCENT, 0.45), "Created"))
                        .child(legend_swatch(SUCCESS, "Resolved"))
                        .child(legend_swatch(ATTENTION, "7D mean")),
                ),
        )
        .child(plot_grid_frame(top_tick, col_gap, columns))
        .child(
            view()
                .pl(px(28.))
                .flex_row()
                .justify_between()
                .children(
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

fn render_weekly_velocity_chart(ui: Ui, dash: &Dashboard) -> View {
    let top_tick = nice_ceil_tick(dash.max_weekly_kanban);
    let usable_h = CHART_HEIGHT - 8.0;

    let columns = dash
        .weeks
        .iter()
        .enumerate()
        .map(|(w_idx, wk)| {
            let c_h = if wk.created_cards == 0 {
                0.0
            } else {
                (wk.created_cards as f32 / top_tick as f32 * usable_h).max(3.0)
            };
            let m_h = if wk.merged_cards == 0 {
                0.0
            } else {
                (wk.merged_cards as f32 / top_tick as f32 * usable_h).max(3.0)
            };
            let proj_h = wk
                .projected_merges
                .filter(|&p| p > wk.merged_cards)
                .map(|p| (p as f32 / top_tick as f32 * usable_h).min(usable_h));
            let avg = dash.weekly_moving_average[w_idx];

            let tip_title = format!(
                "{} ({}..{}){}",
                wk.short_label,
                wk.week_start.format("%m-%d"),
                wk.week_end.format("%m-%d"),
                if wk.is_partial { " · In progress" } else { "" }
            );
            let proj_str = wk
                .projected_merges
                .map(|p| format!(" · Pace ~{p}/wk"))
                .unwrap_or_default();
            let tip_merged = format!(
                "Resolved: {} ({:.1}/day{}) · Cycle p50: {}",
                wk.merged_cards,
                wk.merges_per_day(),
                proj_str,
                wk.cycle_p50_ms
                    .map(format_duration_compact)
                    .unwrap_or_else(|| "—".into())
            );
            let tip_runs = format!(
                "Created: {} · Runs: {} (Pass: {}) · Human asks: {}",
                wk.created_cards,
                wk.total_runs,
                wk.pass_rate_pct
                    .map(|p| format!("{p}%"))
                    .unwrap_or_else(|| "—".into()),
                wk.human_touches
            );
            let target_end = wk.week_end;

            view()
                .flex_1()
                .min_w(px(0.))
                .h(px(CHART_HEIGHT))
                .tooltip(
                    tooltip_box()
                        .gap_0p5()
                        .child(
                            text(tip_title)
                                .font_family(MONO_FONT)
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(TEXT),
                        )
                        .child(label(tip_merged).text_size(px(11.5)).text_color(SUCCESS))
                        .child(muted(tip_runs).text_size(px(11.5))),
                )
                .child(
                    press_target()
                        .w_full()
                        .h_full()
                        .rounded(px(4.))
                        .hover(|s| s.bg(CARD_HOVER))
                        .justify_end()
                        .items_center()
                        .px_1()
                        .on_press(move |_: &()| {
                            select_date(ui, Some(target_end));
                            set_range(ui, RangeWindow::Days7);
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
                                .gap(px(3.))
                                .child(
                                    view()
                                        .flex_1()
                                        .max_w(px(20.))
                                        .h(px(c_h))
                                        .rounded(px(2.))
                                        .bg(move || alpha_tint(ACCENT, 0.45)),
                                )
                                .child(
                                    view()
                                        .flex_1()
                                        .max_w(px(20.))
                                        .h(px(proj_h.unwrap_or(m_h)))
                                        .justify_end()
                                        .rounded(px(2.))
                                        .border_1()
                                        .border_color(if proj_h.is_some() {
                                            SUCCESS
                                        } else {
                                            CLEAR
                                        })
                                        .bg(if proj_h.is_some() {
                                            alpha_tint(SUCCESS, 0.16)
                                        } else {
                                            AdaptiveColor::TRANSPARENT
                                        })
                                        .child(
                                            view().w_full().h(px(m_h)).rounded(px(2.)).bg(SUCCESS),
                                        ),
                                ),
                        ),
                )
                .into_element()
        })
        .collect::<Vec<_>>();

    card_surface()
        .flex_1()
        .min_w(px(320.))
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
                        .child(heading("Weekly velocity comparison (WoW)"))
                        .child(note(
                            "ISO weeks (Mon–Sun) · outlined cap: full-week projection · click week to inspect 7D",
                        )),
                )
                .child(
                    row()
                        .child(choice(ui, "Daily", "cadence-daily", false))
                        .child(choice(ui, "Weekly", "cadence-weekly", true)),
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
                        .child(choice(
                            ui,
                            "8 weeks",
                            "weeks-8",
                            ui.insights.trend_weeks.get() == 8,
                        ))
                        .child(choice(
                            ui,
                            "12 weeks",
                            "weeks-12",
                            ui.insights.trend_weeks.get() == 12,
                        )),
                )
                .child(
                    row()
                        .child(legend_swatch(move || alpha_tint(ACCENT, 0.45), "Created"))
                        .child(legend_swatch(SUCCESS, "Resolved"))
                        .child(legend_swatch(ATTENTION, "4W mean")),
                ),
        )
        .child(plot_grid_frame(top_tick, 8.0, columns))
        .child(
            view()
                .pl(px(28.))
                .flex_row()
                .justify_between()
                .children(
                    dash.weeks
                        .iter()
                        .map(|w| {
                            text(format!(
                                "{}{}",
                                w.short_label,
                                if w.is_partial { "*" } else { "" }
                            ))
                            .font_family(MONO_FONT)
                            .text_size(px(10.5))
                            .text_color(MUTED)
                            .into_element()
                        })
                        .collect::<Vec<_>>(),
                ),
        )
}

fn render_efficiency_trajectory_panel(ui: Ui, dash: &Dashboard) -> View {
    let is_weekly = ui.insights.cadence.get() == TrendCadence::Weekly;
    let wow = &dash.wow;

    // Top summary box: Same-progress DoD (when Daily) + Same-progress WoW
    let wow_merged_delta = if wow.baseline_known {
        format_delta_pct(wow.current_merged, wow.previous_merged)
    } else {
        "insufficient prior week".into()
    };
    let wow_cyc_delta = format_duration_delta(wow.current_cycle_p50_ms, wow.previous_cycle_p50_ms)
        .map(|(s, _)| s)
        .unwrap_or_else(|| "—".into());
    let wow_pass = match (wow.current_pass_pct, wow.previous_pass_pct) {
        (Some(c), Some(p)) => {
            let diff = c as isize - p as isize;
            format!("{c}% ({}{diff}% vs prev)", if diff >= 0 { "+" } else { "" })
        }
        (Some(c), None) => format!("{c}%"),
        _ => "—".into(),
    };
    let wow_touches = match (
        wow.current_touches_per_merge,
        wow.previous_touches_per_merge,
    ) {
        (Some(c), Some(p)) => {
            let diff = c - p;
            format!(
                "{c:.2}/card ({}{diff:.2})",
                if diff >= 0.0 { "+" } else { "" }
            )
        }
        (Some(c), None) => format!("{c:.2}/card"),
        _ => "—".into(),
    };

    let summary_box = view()
        .p_2p5()
        .gap_1p5()
        .rounded(px(6.))
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_1()
                .child(
                    label(format!(
                        "Same-progress WoW: {} vs {}",
                        wow.current_label, wow.previous_label
                    ))
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD),
                )
                .child(status(wow_merged_delta, Tone::Success)),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap_3()
                .child(note(format!(
                    "Resolved: {} vs {}",
                    wow.current_merged, wow.previous_merged
                )))
                .child(note(format!(
                    "Cycle p50: {} ({})",
                    wow.current_cycle_p50_ms
                        .map(format_duration_compact)
                        .unwrap_or_else(|| "—".into()),
                    wow_cyc_delta
                )))
                .child(note(format!("Pass: {wow_pass}")))
                .child(note(format!("Human asks: {wow_touches}"))),
        );

    // Recent rows: either last 6 weeks or last 7 days with cycle/pass/velocity bars
    let rows: Vec<AnyElement> = if is_weekly {
        let recent_weeks: Vec<&WeekBucket> = dash
            .weeks
            .iter()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let max_cyc = recent_weeks
            .iter()
            .filter_map(|w| w.cycle_p50_ms)
            .max()
            .unwrap_or(1)
            .max(1);
        recent_weeks
            .into_iter()
            .map(|wk| {
                let ratio = wk
                    .cycle_p50_ms
                    .map(|ms| (ms as f32 / max_cyc as f32).clamp(0.08, 1.0))
                    .unwrap_or(0.0);
                let cyc_txt = wk
                    .cycle_p50_ms
                    .map(format_duration_compact)
                    .unwrap_or_else(|| "—".into());
                let pass_txt = wk
                    .pass_rate_pct
                    .map(|p| format!("{p}%"))
                    .unwrap_or_else(|| "—".into());
                let asks_txt = wk
                    .touches_per_merge()
                    .map(|t| format!("{t:.1}/res"))
                    .unwrap_or_else(|| format!("{}a", wk.human_touches));
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .py_0p5()
                    .child(
                        mono_val(
                            format!(
                                "{}{}",
                                wk.short_label,
                                if wk.is_partial { "*" } else { " " }
                            ),
                            11.5,
                        )
                        .w(px(36.)),
                    )
                    .child(
                        mono_val(
                            format!("{} ({:.1}/d)", wk.merged_cards, wk.merges_per_day()),
                            11.5,
                        )
                        .w(px(92.)),
                    )
                    .child(
                        view()
                            .flex_1()
                            .min_w(px(50.))
                            .h(px(7.))
                            .rounded(px(3.))
                            .bg(INSET)
                            .overflow_hidden()
                            .flex_row()
                            .child(
                                view()
                                    .flex_grow(ratio.max(0.0))
                                    .h_full()
                                    .rounded(px(3.))
                                    .bg(ACCENT),
                            )
                            .child(view().flex_grow((1.0 - ratio).max(0.0)).h_full()),
                    )
                    .child(mono_val(cyc_txt, 11.5).w(px(54.)))
                    .child(note(format!("Pass {pass_txt} · {asks_txt}")).w(px(116.)))
                    .into_element()
            })
            .collect()
    } else {
        let recent_days: Vec<&DayBucket> = dash
            .days
            .iter()
            .rev()
            .take(7)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let max_cyc = recent_days
            .iter()
            .filter_map(|d| d.cycle_p50_ms)
            .max()
            .unwrap_or(1)
            .max(1);
        recent_days
            .into_iter()
            .map(|d| {
                let ratio = d
                    .cycle_p50_ms
                    .map(|ms| (ms as f32 / max_cyc as f32).clamp(0.08, 1.0))
                    .unwrap_or(0.0);
                let cyc_txt = d
                    .cycle_p50_ms
                    .map(format_duration_compact)
                    .unwrap_or_else(|| "—".into());
                let pass_txt = d
                    .pass_rate_pct()
                    .map(|p| format!("{p}%"))
                    .unwrap_or_else(|| "—".into());
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .py_0p5()
                    .child(
                        mono_val(format!("{} {}", d.short_label, d.weekday_label), 11.5).w(px(74.)),
                    )
                    .child(
                        mono_val(format!("{} / {}", d.merged_cards, d.total_runs()), 11.5)
                            .w(px(68.)),
                    )
                    .child(
                        view()
                            .flex_1()
                            .min_w(px(44.))
                            .h(px(7.))
                            .rounded(px(3.))
                            .bg(INSET)
                            .overflow_hidden()
                            .flex_row()
                            .child(
                                view()
                                    .flex_grow(ratio.max(0.0))
                                    .h_full()
                                    .rounded(px(3.))
                                    .bg(ACCENT),
                            )
                            .child(view().flex_grow((1.0 - ratio).max(0.0)).h_full()),
                    )
                    .child(mono_val(cyc_txt, 11.5).w(px(54.)))
                    .child(note(format!("Pass {pass_txt} · {}a", d.human_touches)).w(px(102.)))
                    .into_element()
            })
            .collect()
    };

    card_surface()
        .flex_1()
        .min_w(px(320.))
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
                        .child(heading("Personal & Agent efficiency trajectory"))
                        .child(note(
                            "Shorter cycle p50 + higher pass% + fewer human asks = higher leverage",
                        )),
                ),
        )
        .child(summary_box)
        .child(
            view()
                .gap_1()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(note(if is_weekly {
                            "Recent 6 weeks (resolutions · cycle p50 bar · pass% · asks/resolution)"
                        } else {
                            "Recent 7 days (resolutions/runs · cycle p50 bar · pass% · asks)"
                        })),
                )
                .children(rows),
        )
}

fn render_hourly(ui: Ui, dash: &Dashboard) -> View {
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

    card_surface()
        .flex_1()
        .min_w(px(320.))
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
                        .child(heading("Hourly comparison"))
                        .child(note(
                            "Cumulative selected metric · current day vs baseline · future hours blank",
                        )),
                )
                .child(
                    row()
                        .child(legend_swatch(move || alpha_tint(MUTED, 0.45), "Baseline"))
                        .child(legend_swatch(ACCENT, "Selected day")),
                ),
        )
        .child(metric_choices(ui, false))
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

fn render_stage_and_queue_timings(dash: &Dashboard) -> View {
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

    let timing_row = |title: &'static str,
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

    card_surface()
        .flex_1()
        .min_w(px(320.))
        .child(
            view()
                .gap_0p5()
                .child(heading("Stage & queue timings"))
                .child(note(
                    "Average Develop, Review, Merge Queue & @mention reply durations (with p50 median)",
                )),
        )
        .child(
            view()
                .gap_2p5()
                .child(timing_row(
                    "Develop",
                    &t.develop_stage,
                    ACCENT.resolve(),
                    dev_exec_detail,
                ))
                .child(timing_row(
                    "Review",
                    &t.review_stage,
                    SUCCESS.resolve(),
                    None,
                ))
                .child(timing_row(
                    "Merge Queue",
                    &t.merge_queue,
                    ATTENTION.resolve(),
                    None,
                ))
                .child(timing_row(
                    "@mention Reply",
                    &t.mention_reply,
                    alpha_tint(ACCENT, 0.7),
                    None,
                )),
        )
        .child(view().w_full().h(px(1.)).bg(BORDER))
        .child(
            view()
                .gap_1()
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            label("Merge queue efficiency")
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM),
                        )
                        .child(clean_pill),
                )
                .child(note(format!(
                    "{} batches ({} clean · {} conflicts) · Avg {batch_size_str}{conflict_str}",
                    t.merge_queue_batches,
                    t.merge_queue_clean_batches,
                    t.merge_queue_conflict_batches
                ))),
        )
}

fn render_intake_and_funnel(dash: &Dashboard) -> View {
    let intake = &dash.intake;
    let denom = intake.total_raw.max(1);
    let bar_row = |name: &'static str, count: usize, color: AdaptiveColor, muted_flag: bool| {
        let pct = (count * 100 + denom / 2) / denom;
        let ratio = if intake.total_raw == 0 || count == 0 {
            0.0
        } else {
            (count as f32 / denom as f32).clamp(0.04, 1.0)
        };
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .opacity(if muted_flag { 0.65 } else { 1.0 })
            .child(label(name).text_size(px(12.)).w(px(120.)).truncate())
            .child(
                view()
                    .flex_1()
                    .min_w(px(48.))
                    .h(px(7.))
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
            .child(mono_val(format!("{count} ({pct}%)"), 11.5).w(px(78.)))
    };

    let pipe = &dash.pipeline;
    card_surface()
        .flex_1()
        .min_w(px(320.))
        .child(
            view()
                .gap_0p5()
                .child(heading("Intake sources & pipeline gates"))
                .child(note(
                    "Period card origins + live board bottlenecks, merge conflicts & human waits",
                )),
        )
        .child(
            view()
                .gap_1p5()
                .child(bar_row(
                    "Local manual",
                    intake.local_manual,
                    ACCENT.resolve(),
                    false,
                ))
                .child(bar_row(
                    "GitHub PR",
                    intake.github_pr,
                    SUCCESS.resolve(),
                    false,
                ))
                .child(bar_row(
                    "GitHub Issue",
                    intake.github_issue,
                    alpha_tint(ACCENT, 0.65),
                    false,
                ))
                .child(bar_row(
                    if dash.exclude_initial_sync {
                        "Initial sync (muted)"
                    } else {
                        "Initial sync"
                    },
                    intake.initial_sync,
                    alpha_tint(MUTED, 0.55),
                    dash.exclude_initial_sync,
                )),
        )
        .child(view().w_full().h(px(1.)).bg(BORDER))
        .child(
            view()
                .gap_1()
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .justify_between()
                        .gap_2()
                        .child(note(format!(
                            "Active Board WIP: {} (Backlog {} · Develop {} · Review {})",
                            pipe.active_wip, pipe.backlog, pipe.develop, pipe.review
                        )))
                        .child(note(format!("Merging now: {}", pipe.merging_now))),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .justify_between()
                        .gap_2()
                        .child(note(format!(
                            "Human gates: {} · Attention cards: {}",
                            pipe.human_gated, pipe.attention_cards
                        )))
                        .child(note(format!(
                            "Merge batches: {} ({} conflicts) · Human asks: {}",
                            pipe.merge_batches_period,
                            pipe.conflict_batches_period,
                            dash.human_asks_period
                        ))),
                ),
        )
}

fn render_projects(ui: Ui, dash: &Dashboard) -> View {
    let sort_mode = ui.insights.project_sort.get();
    let active_scope = ui.insights.scope_project.get();
    let mut projects = dash.project_stats.clone();
    match sort_mode {
        ProjectSort::Merged => {}
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

    let max_metric = projects
        .iter()
        .map(|p| match sort_mode {
            ProjectSort::Merged => p.merged,
            ProjectSort::Created => p.created,
            ProjectSort::Wip => p.total_wip(),
            ProjectSort::Runs => p.agent_runs,
        })
        .max()
        .unwrap_or(1)
        .max(1);

    let rows = projects
        .into_iter()
        .map(|p| {
            let pid = p.id.clone();
            let is_scoped = active_scope == pid;
            let share = if dash.merged_total > 0 {
                format!("{:.0}%", p.merged as f64 * 100. / dash.merged_total as f64)
            } else {
                "—".into()
            };
            let sync_tag = if p.initial_sync > 0 && dash.exclude_initial_sync {
                format!(" (+{}s)", p.initial_sync)
            } else {
                String::new()
            };
            let total_wip = p.total_wip();

            let bar_track = match sort_mode {
                ProjectSort::Wip => {
                    let ratio = if total_wip == 0 {
                        0.0
                    } else {
                        (total_wip as f32 / max_metric as f32).clamp(0.05, 1.0)
                    };
                    let wip_segs: Vec<(usize, AdaptiveColor)> = vec![
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
                        ProjectSort::Merged => (p.merged, SUCCESS.resolve()),
                        ProjectSort::Created => (p.created, ACCENT.resolve()),
                        ProjectSort::Runs => (p.agent_runs, alpha_tint(ACCENT, 0.8)),
                        ProjectSort::Wip => unreachable!(),
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
                "{} · +{} created{} · {} resolved ({share}) · WIP {total_wip} (Backlog {} / Develop {} / Review {}) · {} agent runs",
                p.name,
                p.created,
                sync_tag,
                p.merged,
                p.wip_backlog,
                p.wip_develop,
                p.wip_review,
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
                                .gap_1p5()
                                .child(
                                    view()
                                        .flex_row()
                                        .flex_wrap()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .child(
                                            label(p.name)
                                                .font_weight(FontWeight::MEDIUM)
                                                .max_w(px(170.))
                                                .truncate(),
                                        )
                                        .child(
                                            view()
                                                .flex_row()
                                                .flex_wrap()
                                                .items_center()
                                                .gap_2()
                                                .child(mono_val(
                                                    format!(
                                                        "+{}{} / {} ({share})",
                                                        p.created, sync_tag, p.merged
                                                    ),
                                                    11.5,
                                                ))
                                                .child(note(format!(
                                                    "WIP {}/{}/{} · {} runs",
                                                    p.wip_backlog,
                                                    p.wip_develop,
                                                    p.wip_review,
                                                    p.agent_runs
                                                ))),
                                        ),
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
                                                    ProjectSort::Merged => share,
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
                                            .w(px(52.)),
                                        ),
                                ),
                        ),
                )
                .into_element()
        })
        .collect::<Vec<_>>();

    let mut panel = card_surface().flex_1().min_w(px(320.)).child(
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .child(heading("Project contributions"))
            .child(
                row()
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
    if sort_mode == ProjectSort::Wip {
        panel = panel.child(
            row()
                .child(legend_swatch(move || alpha_tint(MUTED, 0.55), "Backlog"))
                .child(legend_swatch(ACCENT, "Develop"))
                .child(legend_swatch(SUCCESS, "Review")),
        );
    }
    if rows.is_empty() {
        panel = panel.child(note("No Project activity or active WIP in this period."));
    }
    panel.children(rows).child(note(
        "Bar reflects active sort metric (+Created / Resolved share · WIP Backlog/Develop/Review · Agent runs). Click row to scope/unscope.",
    ))
}

fn render_agent_stacked_bar(
    starts: usize,
    max_starts: usize,
    completed: usize,
    active: usize,
    blocked: usize,
    failed: usize,
) -> View {
    let ratio = if starts == 0 || max_starts == 0 {
        0.0
    } else {
        (starts as f32 / max_starts as f32).clamp(0.05, 1.0)
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
            .bg(alpha_tint(MUTED, 0.45))
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

fn render_agents(ui: Ui, dash: &Dashboard) -> View {
    let provider_group = ui.insights.provider_group.get();
    let mut panel = card_surface()
        .flex_1()
        .min_w(px(320.))
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
                        .child(choice(ui, "Profile", "group-profile", !provider_group))
                        .child(choice(ui, "Provider", "group-provider", provider_group)),
                ),
        )
        .child(
            row()
                .child(legend_swatch(SUCCESS, "Completed"))
                .child(legend_swatch(ACCENT, "Active"))
                .child(legend_swatch(ATTENTION, "Blocked"))
                .child(legend_swatch(DANGER, "Failed")),
        );

    if provider_group {
        struct ProvAgg {
            profiles: usize,
            starts: usize,
            completed: usize,
            active: usize,
            blocked: usize,
            failed: usize,
            asks: usize,
            cards: std::collections::BTreeSet<String>,
            exec_sum: u128,
            exec_cnt: usize,
        }
        let mut grouped = std::collections::BTreeMap::<String, ProvAgg>::new();
        for p in &dash.profile_stats {
            let g = grouped.entry(p.provider.clone()).or_insert(ProvAgg {
                profiles: 0,
                starts: 0,
                completed: 0,
                active: 0,
                blocked: 0,
                failed: 0,
                asks: 0,
                cards: std::collections::BTreeSet::new(),
                exec_sum: 0,
                exec_cnt: 0,
            });
            g.profiles += 1;
            g.starts += p.total_runs;
            g.completed += p.completed_runs;
            g.active += p.active_runs;
            g.blocked += p.blocked_runs;
            g.failed += p.failed_runs;
            g.asks += p.human_asks;
            g.cards.extend(p.card_ids.iter().cloned());
            if let Some(ms) = p.avg_exec_ms {
                let w = (p.completed_runs + p.blocked_runs + p.failed_runs).max(1);
                g.exec_sum += u128::from(ms) * w as u128;
                g.exec_cnt += w;
            }
        }
        let mut entries: Vec<(String, ProvAgg)> = grouped.into_iter().collect();
        entries.sort_by(|a, b| b.1.starts.cmp(&a.1.starts).then_with(|| a.0.cmp(&b.0)));
        let max_starts = entries
            .iter()
            .map(|(_, g)| g.starts)
            .max()
            .unwrap_or(1)
            .max(1);
        for (name, g) in entries {
            let resolved = g.completed + g.blocked + g.failed;
            let pass_pct = (resolved > 0).then(|| (g.completed * 100 + resolved / 2) / resolved);
            let pass_tone = match pass_pct {
                Some(pct) if pct >= 85 => Tone::Success,
                Some(pct) if pct < 60 => Tone::Attention,
                _ => Tone::Neutral,
            };
            let pass_txt = pass_pct
                .map(|pct| format!("{pct}% pass"))
                .unwrap_or_else(|| "active".into());
            let avg_exec = if g.exec_cnt > 0 {
                format_duration_compact((g.exec_sum / g.exec_cnt as u128) as u64)
            } else {
                "—".into()
            };
            panel = panel.child(
                view()
                    .w_full()
                    .p_2()
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
                                    .child(label(name).font_weight(FontWeight::MEDIUM).truncate())
                                    .child(note(format!("({} roles)", g.profiles))),
                            )
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_1p5()
                                    .child(mono_val(format!("{} starts", g.starts), 11.5))
                                    .child(status(pass_txt, pass_tone)),
                            ),
                    )
                    .child(render_agent_stacked_bar(
                        g.starts,
                        max_starts,
                        g.completed,
                        g.active,
                        g.blocked,
                        g.failed,
                    ))
                    .child(note(format!(
                        "{} Cards · exec avg {avg_exec} · {} asks",
                        g.cards.len(),
                        g.asks
                    ))),
            );
        }
    } else {
        let max_starts = dash
            .profile_stats
            .iter()
            .map(|p| p.total_runs)
            .max()
            .unwrap_or(1)
            .max(1);
        for p in &dash.profile_stats {
            let pass_pct = p.pass_rate_pct();
            let pass_tone = match pass_pct {
                Some(pct) if pct >= 85 => Tone::Success,
                Some(pct) if pct < 60 => Tone::Attention,
                _ => Tone::Neutral,
            };
            let pass_txt = pass_pct
                .map(|pct| format!("{pct}% pass"))
                .unwrap_or_else(|| "active".into());
            let exec_str = p
                .avg_exec_ms
                .map(format_duration_compact)
                .unwrap_or_else(|| "—".into());
            let q_str = p
                .median_queue_wait_ms
                .map(format_duration_compact)
                .unwrap_or_else(|| "—".into());
            let reply_suffix = p
                .avg_mention_reply_ms
                .map(|ms| format!(" · reply avg {}", format_duration_compact(ms)))
                .unwrap_or_default();

            panel = panel.child(
                view()
                    .w_full()
                    .p_2()
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
                                        label(format!("@{}", p.profile_id))
                                            .font_weight(FontWeight::MEDIUM)
                                            .truncate(),
                                    )
                                    .child(note(format!("({})", p.provider))),
                            )
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_1p5()
                                    .child(mono_val(format!("{} starts", p.total_runs), 11.5))
                                    .child(status(pass_txt, pass_tone)),
                            ),
                    )
                    .child(render_agent_stacked_bar(
                        p.total_runs,
                        max_starts,
                        p.completed_runs,
                        p.active_runs,
                        p.blocked_runs,
                        p.failed_runs,
                    ))
                    .child(note(format!(
                        "{} Cards · exec avg {exec_str} · Q p50 {q_str} · {} asks{reply_suffix}",
                        p.cards_touched, p.human_asks
                    ))),
            );
        }
    }
    if dash.profile_stats.is_empty() {
        panel = panel.child(note("No Agent execution observed in this period."));
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
            "Bar length reflects starts in period; segments show outcome mix. Queue p50 is computed per Role + Provider.",
        ))
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
                                    "{} · {} created · {} resolved · {} Agent starts",
                                    c.date_key, c.created_cards, c.merged_cards, c.total_runs
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

    card_surface()
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(heading("Yearly activity"))
                .child(metric_choices(ui, true)),
        )
        .child(note(format!(
            "{} – {} · Last 365 days · {}",
            dash.annual_start,
            dash.annual_end,
            ui.insights.metric.get().label()
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
            "Select a tile to inspect that date. Faded tiles precede retained history. Project scope applies; the annual range stays fixed.",
        ))
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

fn two_col_or_stack(narrow: bool, left: View, right: View) -> View {
    if narrow {
        view().gap_4().min_w(px(0.)).child(left).child(right)
    } else {
        view()
            .flex_row()
            .items_stretch()
            .gap_4()
            .min_w(px(0.))
            .child(left)
            .child(right)
    }
}

fn dashboard(ui: Ui) -> Dashboard {
    let scope = ui.insights.scope_project.get();
    let range = ui.insights.range.get();
    let now = now_ms(ui);
    let profiles = ui.all_profiles.get();
    let selected = ui.insights.selected_date.get();
    let comparison = ui.insights.comparison.get();
    let metric = ui.insights.metric.get();
    let hourly_metric = ui.insights.hourly_metric.get();
    let trend_days = ui.insights.trend_days.get();
    let trend_weeks = ui.insights.trend_weeks.get();
    let excl_sync = ui.insights.exclude_initial_sync.get();
    ui.insights.events.with(|evs| {
        ui.snapshot.with(|s| {
            model::compute_full(
                s,
                evs,
                (!scope.is_empty()).then_some(scope.as_str()),
                range,
                now,
                &chrono::Local,
                selected,
                comparison,
                metric,
                hourly_metric,
                trend_days,
                trend_weeks,
                excl_sync,
                &profiles,
            )
        })
    })
}

pub fn screen(ui: Ui) -> impl IntoElement {
    effect(move || {
        let page = ui.page.get();
        let _scope = ui.insights.scope_project.get();
        let _gen = ui.index.with(|idx| idx.generation);
        if page == "insights" {
            refresh_events(ui);
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
                            let cadence = ui.insights.cadence.get();

                            let mut content = view()
                                .gap_4()
                                .min_w(px(0.))
                                .child(render_period_subbar(ui, &dash))
                                .child(render_kpis(&dash, narrow));

                            if !ui.insights.drilldown.get().is_empty() {
                                content = content.child(render_card_drilldown(ui, &dash));
                            }

                            // Row 2: Output Velocity (Daily or Weekly) + Personal/Agent Efficiency Trajectory
                            let velocity_panel = match cadence {
                                TrendCadence::Daily => render_daily_velocity_chart(ui, &dash),
                                TrendCadence::Weekly => render_weekly_velocity_chart(ui, &dash),
                            };
                            content = content.child(two_col_or_stack(
                                narrow,
                                velocity_panel,
                                render_efficiency_trajectory_panel(ui, &dash),
                            ));

                            // Row 2.5: For 1D -> full-width Hourly cumulative pace
                            if dash.range == RangeWindow::Today {
                                content = content.child(render_hourly(ui, &dash));
                            }

                            // Row 3: Stage & Queue Timings + Intake sources & pipeline gates
                            content = content.child(two_col_or_stack(
                                narrow,
                                render_stage_and_queue_timings(&dash),
                                render_intake_and_funnel(&dash),
                            ));

                            // Row 4: Project multi-dimensional matrix + Agent Role/Provider matrix
                            content = content.child(two_col_or_stack(
                                narrow,
                                render_projects(ui, &dash),
                                render_agents(ui, &dash),
                            ));

                            // Row 5: Full-width 365-day cadence heatmap
                            content = content.child(render_heatmap_panel(ui, &dash));

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
                                                    chrono::DateTime::from_timestamp_millis(
                                                        t as i64,
                                                    )
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
                            content
                                .child(note(format!(
                                    "Retained history from {} · {} resolved Cards have no reliable resolution date and are excluded from date totals. {} Agent batches have no retained launch timestamp. Removed history may be unavailable.{}",
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
                                .into_element()
                        })),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{Card, Delivery, Event, Project, Snapshot, Workflow};
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
    fn insights_route_mounts_cockpit_switches_weekly_cadence_and_ranges() {
        #[component]
        fn InsightsHarness() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let now_ms = 1_794_744_000_000u64;
            ui.insights.anchor_ms.set(Some(now_ms));

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

            ui.snapshot.set(snap);
            ui.insights.events.set(std::rc::Rc::new(events));
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
        assert!(tree.has_mounted_text("RESOLUTION THROUGHPUT"));
        assert!(tree.has_mounted_text("PIPELINE & WIP HEALTH"));
        assert!(tree.has_mounted_text("AGENT EXECUTION QUALITY"));
        assert!(tree.has_mounted_text("EFFICIENCY & AUTOMATION"));
        assert!(tree.has_mounted_text("Recent daily flow"));
        assert!(tree.has_mounted_text("Personal & Agent efficiency trajectory"));
        assert!(tree.has_mounted_text("Hourly comparison"));
        assert!(tree.has_mounted_text("Stage & queue timings"));
        assert!(tree.has_mounted_text("Merge queue efficiency"));
        assert!(tree.has_mounted_text("Intake sources & pipeline gates"));
        assert!(tree.has_mounted_text("Yearly activity"));
        assert!(tree.has_mounted_text("Agents used"));
        assert!(tree.has_mounted_text("Trigger sources"));
        assert!(!tree.has_mounted_text("▸ Efficiency & timing percentiles"));
        assert!(!tree.has_mounted_text("▾ Efficiency & timing percentiles"));

        // Switch cadence to Weekly
        let btn_weekly = *tree.labels.get("Weekly").expect("Weekly cadence button");
        assert!(core.component_event(btn_weekly, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("Weekly velocity comparison (WoW)"));

        // Switch Agents grouping to Provider
        let btn_provider = *tree.labels.get("Provider").expect("Provider group button");
        assert!(core.component_event(btn_provider, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("Agents used"));

        // Switch range to 7D via segmented button
        let btn_7d = *tree.labels.get("7D").expect("7D segment button");
        assert!(core.component_event(btn_7d, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("Yearly activity"));
        assert!(tree.has_mounted_text("Stage & queue timings"));
        assert!(!tree.has_mounted_text("Hourly comparison"));
    }
}
