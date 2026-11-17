//! Insights dashboard: daily Kanban merge throughput, Agent scheduling outcomes,
//! Calendar-day comparisons, Project/Agent breakdowns, annual cadence and cycle samples.

pub mod model;

use crate::{
    drafts::{capture_scroll, scroll_position},
    presenter::*,
};
use canban_ui_kit::{tokens::*, *};
use model::{
    ActivityMetric, Comparison, Dashboard, DayBucket, RangeWindow, format_delta_pct,
    format_duration_compact,
};
use nsh_core::{element::AdaptiveColor, prelude::*};

const ALL_PROJECTS_OPTION: &str = "__all__";

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
    pub scroll: Signal<Option<ElementRef>>,
    pub annual_width: Signal<f32>,
    pub efficiency_open: Signal<bool>,
    pub queue_samples: Signal<bool>,
    pub sample_page: Signal<usize>,
    pub provider_group: Signal<bool>,
    pub sort_created: Signal<bool>,
    pub drilldown: Signal<String>,
    /// Optional fixed timestamp (ms) for deterministic Gallery/test rendering; `None` uses live clock.
    pub anchor_ms: Signal<Option<u64>>,
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
            scroll: signal(None),
            annual_width: signal(0.),
            efficiency_open: signal(false),
            queue_samples: signal(false),
            sample_page: signal(0),
            provider_group: signal(false),
            sort_created: signal(false),
            drilldown: signal(String::new()),
            anchor_ms: signal(None),
        }
    }
}

pub fn set_range(ui: Ui, next: RangeWindow) {
    ui.insights.range.set(next);
    if next != RangeWindow::Today {
        ui.insights.comparison.set(Comparison::Previous);
    }
    ui.insights.sample_page.set(0);
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
    let c = token.resolve().dark.alpha(alpha);
    AdaptiveColor { light: c, dark: c }
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
                    .filter(|c| c.removed_at.is_none() && c.merged())
                    .count();
                let mut opts = Vec::with_capacity(s.projects.len() + 1);
                opts.push(
                    PickerOption::new(ALL_PROJECTS_OPTION, "All Projects")
                        .detail(format!("{total_merged} merged"))
                        .icon(Icon::Projects)
                        .selected(cur.is_empty()),
                );
                for p in &s.projects {
                    let p_merged = s
                        .cards
                        .iter()
                        .filter(|c| c.project_id == p.id && c.removed_at.is_none() && c.merged())
                        .count();
                    opts.push(
                        PickerOption::new(p.id.clone(), p.name.clone())
                            .group("Projects")
                            .detail(format!("{p_merged} merged"))
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

const CHART_HEIGHT: f32 = 148.0;

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

struct KpiSpec {
    mark: Icon,
    mark_tint: ThemeColor,
    eyebrow: &'static str,
    hero: String,
    pill_text: Option<(String, Tone)>,
    sub_primary: String,
    sub_secondary: String,
    extra_bar: Option<AnyElement>,
}

fn kpi_card(spec: KpiSpec) -> View {
    let KpiSpec {
        mark,
        mark_tint,
        eyebrow,
        hero,
        pill_text,
        sub_primary,
        sub_secondary,
        extra_bar,
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

    let mut card = card_surface()
        .flex_1()
        .min_w(px(200.))
        .gap_2()
        .child(header)
        .child(
            text(hero)
                .font_family(MONO_FONT)
                .text_size(px(24.))
                .font_weight(FontWeight::BOLD)
                .text_color(TEXT),
        );
    if let Some(bar) = extra_bar {
        card = card.child(bar);
    }
    card.child(
        view()
            .gap_0p5()
            .child(label(sub_primary).text_size(px(CAPTION_SIZE)))
            .child(muted(sub_secondary).text_size(px(11.5))),
    )
}

fn render_kpis(dash: &Dashboard, narrow: bool) -> View {
    let known = dash.history_start.is_some_and(|d| d <= dash.selected_date);
    let ref_line = |value: Option<f64>| {
        value
            .map(|n| {
                format!(
                    "Previous 7 days: {n:.1} / day ({} days)",
                    dash.reference_days
                )
            })
            .unwrap_or_else(|| "Previous 7 days: insufficient history".into())
    };
    let cards = [
        KpiSpec {
            mark: Icon::Board,
            mark_tint: ACCENT,
            eyebrow: "CREATED CARDS",
            hero: if known {
                dash.created_total.to_string()
            } else {
                "—".into()
            },
            pill_text: Some((
                if dash.baseline_known {
                    format_delta_pct(dash.created_total, dash.previous_created)
                } else {
                    "Insufficient history".into()
                },
                Tone::Neutral,
            )),
            sub_primary: format!(
                "Baseline: {}",
                if dash.baseline_known {
                    dash.previous_created.to_string()
                } else {
                    "—".into()
                }
            ),
            sub_secondary: if dash.range == RangeWindow::Today {
                ref_line(dash.reference_created)
            } else {
                "Created within the selected period".into()
            },
            extra_bar: None,
        },
        KpiSpec {
            mark: Icon::GitMerge,
            mark_tint: SUCCESS,
            eyebrow: "MERGED CARDS",
            hero: if known {
                dash.merged_total.to_string()
            } else {
                "—".into()
            },
            pill_text: Some((
                if dash.baseline_known {
                    format_delta_pct(dash.merged_total, dash.prev_merged_total)
                } else {
                    "Insufficient history".into()
                },
                Tone::Neutral,
            )),
            sub_primary: format!("{} local · {} PR", dash.local_merges, dash.pr_merges),
            sub_secondary: if dash.range == RangeWindow::Today {
                ref_line(dash.reference_merged)
            } else {
                "Unique merged Cards in the period".into()
            },
            extra_bar: None,
        },
        KpiSpec {
            mark: Icon::Projects,
            mark_tint: ACCENT,
            eyebrow: "CONTRIBUTING PROJECTS",
            hero: if known {
                dash.project_stats.len().to_string()
            } else {
                "—".into()
            },
            pill_text: None,
            sub_primary: "Created, merged or executed".into(),
            sub_secondary: "Unique Projects · breakdown below".into(),
            extra_bar: None,
        },
        KpiSpec {
            mark: Icon::Agent,
            mark_tint: ACCENT,
            eyebrow: "AGENTS USED",
            hero: if known {
                format!("{} profiles", dash.used_profiles)
            } else {
                "—".into()
            },
            pill_text: None,
            sub_primary: format!(
                "{} known providers · {} unknown",
                dash.providers, dash.unknown_providers
            ),
            sub_secondary: format!(
                "{} new starts · {} overlapping runs",
                dash.scheduled_total, dash.used_runs
            ),
            extra_bar: None,
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

fn render_kanban_flow_chart(ui: Ui, dash: &Dashboard) -> View {
    let top_tick = nice_ceil_tick(dash.max_daily_kanban);
    let n = dash.days.len();
    let col_gap = if n > 45 {
        1.0
    } else if n > 14 {
        3.0
    } else {
        10.0
    };
    let bar_max_w = if n > 45 {
        4.0
    } else if n > 14 {
        8.0
    } else {
        20.0
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
                "Merged: {} ({} local · {} PR)",
                day.merged_cards, day.local_merges, day.pr_merges
            );
            let avg = dash.moving_average[day_index];
            let tip_created = format!(
                "Created: {} · 7D mean: {}{}",
                day.created_cards,
                avg.map(|v| format!("{v:.1}")).unwrap_or_else(|| "—".into()),
                if day.date_key == dash.annual_end.to_string() {
                    " · In progress"
                } else {
                    ""
                }
            );

            view()
                .flex_1()
                .min_w(px(0.))
                .h(px(CHART_HEIGHT))
                .rounded(px(4.))
                .hover(|s| s.bg(CARD_HOVER))
                .justify_end()
                .items_center()
                .px(px(if n > 45 { 0.0 } else { 1.0 }))
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
                )
                .into_element()
        })
        .collect::<Vec<_>>();

    let ticks = x_axis_ticks(&dash.days);

    card_surface()
        .child(if dash.range == RangeWindow::Today { row()
            .child(choice(ui,"Recent 14 days","trend-14",ui.insights.trend_days.get() == 14))
            .child(choice(ui,"Recent 30 days","trend-30",ui.insights.trend_days.get() == 30)).into_element()
        } else { view().into_element() })
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
                        .child(
                            label("Recent daily flow")
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(14.)),
                        )
                        .child(
                            muted("Daily created / merged · line: 7-day mean of complete days · today is partial")
                                .text_size(px(11.5)),
                        ),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .child(legend_swatch(move || alpha_tint(ACCENT, 0.45), "Created"))
                        .child(legend_swatch(SUCCESS, "Merged")),
                ),
        )
        .child(
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
                ),
        )
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

fn now_ms(ui: Ui) -> u64 {
    ui.insights.anchor_ms.get().unwrap_or_else(|| {
        // The shared clock schedules refreshes; fresh snapshots must use the actual wall time.
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
    ui.insights.sample_page.set(0);
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
        "trend-14" => ui.insights.trend_days.set(14),
        "trend-30" => ui.insights.trend_days.set(30),
        "efficiency" => ui.insights.efficiency_open.update(|v| *v = !*v),
        "samples-cycle" | "samples-queue" => {
            ui.insights.queue_samples.set(operation == "samples-queue");
            ui.insights.sample_page.set(0);
            ui.insights.efficiency_open.set(true);
        }
        "samples-next" => {
            let d = dashboard(ui);
            let count = if ui.insights.queue_samples.get() {
                d.efficiency.queue.len()
            } else {
                d.efficiency.cycle.len()
            };
            ui.insights
                .sample_page
                .update(|p| *p = (*p + 1).min(count.div_ceil(20).saturating_sub(1)));
        }
        "samples-previous" => ui.insights.sample_page.update(|p| *p = p.saturating_sub(1)),
        "group-provider" => ui.insights.provider_group.set(true),
        "group-profile" => ui.insights.provider_group.set(false),
        "sort-created" => ui.insights.sort_created.set(true),
        "sort-merged" => ui.insights.sort_created.set(false),
        "cards-created" => ui.insights.drilldown.set("created".into()),
        "cards-merged" => ui.insights.drilldown.set("merged".into()),
        "cards-close" => ui.insights.drilldown.set(String::new()),
        _ => {}
    }
}

fn heading(title: &str) -> Text {
    label(title.to_owned())
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(14.))
}
fn note(value: impl Into<String>) -> Text {
    muted(value.into()).text_size(px(11.5))
}
fn row() -> View {
    view().flex_row().flex_wrap().items_center().gap_3()
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
            "Merged",
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

fn render_period(ui: Ui, dash: &Dashboard) -> View {
    let actual = dash.annual_end;
    let dash_date = dash.selected_date;
    let valid_date =
        chrono::NaiveDate::parse_from_str(&ui.insights.date_text.get(), "%Y-%m-%d").ok();
    view()
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
                .child(note(dash.timezone.clone())),
        )
        .child(
            row()
                .child(
                    field(ui.insights.date_text, "YYYY-MM-DD")
                        .label("Insights date")
                        .w(px(140.)),
                )
                .child(
                    compact_action("Apply date", Variant::Quiet, move |_| {
                        intent(ui, "apply-date")
                    })
                    .disabled(valid_date.is_none_or(|d| d > actual)),
                )
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
        )
        .child(note(format!("Baseline: {}", dash.comparison_label)))
        .child(
            if dash.history_start.is_none_or(|h| h > dash.selected_date) {
                note("No retained history for this period; unavailable counts are shown as —.")
                    .into_element()
            } else if dash
                .history_start
                .is_some_and(|h| h.to_string() > dash.window_start_key)
            {
                note("Partial history: the selected period starts before retained records.")
                    .into_element()
            } else {
                view().into_element()
            },
        )
}

fn render_hourly(ui: Ui, dash: &Dashboard) -> View {
    let maximum = dash
        .hours
        .iter()
        .filter(|h| !h.future)
        .map(|h| h.current.max(h.previous))
        .max()
        .unwrap_or(0)
        .max(1);
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
                        .flex_row()
                        .gap(px(1.))
                        .items_end()
                        .child(
                            view()
                                .flex_1()
                                .h(px(h.previous as f32 / maximum as f32 * (CHART_HEIGHT - 8.)))
                                .bg(BORDER),
                        )
                        .child(
                            view()
                                .flex_1()
                                .h(px(h.current as f32 / maximum as f32 * (CHART_HEIGHT - 8.)))
                                .bg(ACCENT),
                        )
                        .into_element()
                })
                .into_element()
        })
        .collect::<Vec<_>>();
    card_surface()
        .child(heading("Hourly comparison"))
        .child(note(
            "Cumulative selected metric · current day / baseline · future hours left blank",
        ))
        .child(metric_choices(ui, false))
        .child(
            view()
                .flex_row()
                .items_end()
                .gap_1()
                .child(chart_y_axis(maximum))
                .children(cols),
        )
        .child(
            row()
                .justify_between()
                .child(note("00:00"))
                .child(note("06:00"))
                .child(note("12:00"))
                .child(note("18:00"))
                .child(note("24:00")),
        )
}

fn render_projects(ui: Ui, dash: &Dashboard) -> View {
    let mut projects = dash.project_stats.clone();
    if ui.insights.sort_created.get() {
        projects.sort_by_key(|p| std::cmp::Reverse(p.created));
    }
    let rows = projects
        .into_iter()
        .map(|p| {
            let pid = p.id.clone();
            let share = if dash.merged_total > 0 {
                format!("{:.1}%", p.merged as f64 * 100. / dash.merged_total as f64)
            } else {
                "—".into()
            };
            let spark = p
                .recent_merges
                .iter()
                .map(|v| match v {
                    0 => '·',
                    1 => '▁',
                    2..=3 => '▃',
                    4..=6 => '▅',
                    _ => '█',
                })
                .collect::<String>();
            press_target()
                .w_full()
                .p_2()
                .rounded(px(6.))
                .hover(|s| s.bg(CARD_HOVER))
                .accessibility_label(format!("Insights Project: {}", p.name))
                .on_press(move |_: &()| {
                    ui.insights.scope_project.set(pid.clone());
                    ui.insights.sample_page.set(0);
                })
                .child(
                    row()
                        .child(label(p.name).w(px(160.)).truncate())
                        .child(note(format!("{} created", p.created)))
                        .child(note(format!("{} merged ({share})", p.merged)))
                        .child(note(format!(
                            "{} active Cards · {} profiles",
                            p.cards.len(),
                            p.profiles.len()
                        )))
                        .child(
                            text(spark)
                                .font_family(MONO_FONT)
                                .text_color(SUCCESS)
                                .tooltip(
                                    tooltip_box().child(note("Daily merges in the last 7 days")),
                                ),
                        ),
                )
                .into_element()
        })
        .collect::<Vec<_>>();
    let mut panel = card_surface().child(
        row()
            .justify_between()
            .child(heading("Project contributions"))
            .child(
                row()
                    .child(choice(
                        ui,
                        "Sort by merged",
                        "sort-merged",
                        !ui.insights.sort_created.get(),
                    ))
                    .child(choice(
                        ui,
                        "Sort by created",
                        "sort-created",
                        ui.insights.sort_created.get(),
                    )),
            ),
    );
    if rows.is_empty() {
        panel = panel.child(note("No Project activity in this period."));
    }
    panel.children(rows).child(note("Active Cards are unique Cards created, merged or executed in this period. Click a Project to scope Insights."))
}

fn render_agents(ui: Ui, dash: &Dashboard) -> View {
    let provider_group = ui.insights.provider_group.get();
    let mut panel = card_surface().child(
        row().justify_between().child(heading("Agents used")).child(
            row()
                .child(choice(ui, "Profile", "group-profile", !provider_group))
                .child(choice(ui, "Provider", "group-provider", provider_group)),
        ),
    );
    if provider_group {
        let mut grouped = std::collections::BTreeMap::<
            String,
            (usize, usize, std::collections::BTreeSet<String>),
        >::new();
        for p in &dash.profile_stats {
            let g = grouped.entry(p.provider.clone()).or_default();
            g.0 += 1;
            g.1 += p.total_runs;
            g.2.extend(p.card_ids.iter().cloned());
        }
        for (name, (profiles, starts, cards)) in grouped {
            panel = panel.child(row().child(label(name).w(px(150.)).truncate()).child(note(
                format!(
                    "{profiles} profiles · {starts} new starts · {} Cards touched",
                    cards.len()
                ),
            )));
        }
    } else {
        for p in &dash.profile_stats {
            panel = panel.child(
                row()
                    .child(label(format!("@{}", p.profile_id)).w(px(150.)).truncate())
                    .child(note(p.provider.clone()))
                    .child(note(format!(
                        "{} starts · {} Cards",
                        p.total_runs, p.cards_touched
                    )))
                    .child(note(format!(
                        "Queue p50: {}",
                        p.median_queue_wait_ms
                            .map(format_duration_compact)
                            .unwrap_or_else(|| "—".into())
                    ))),
            );
        }
    }
    if dash.profile_stats.is_empty() {
        panel = panel.child(note("No Agent execution observed in this period."));
    }
    let triggers = dash
        .trigger_stats
        .iter()
        .map(|s| format!("{}: {}", s.kind.label(), s.count))
        .collect::<Vec<_>>()
        .join(" · ");
    panel.child(note(triggers)).child(note("Includes cross-day executions; starts count only launches in the period. Cards can involve several Agents. Provider uses retained run configuration, or current Profile configuration when unavailable."))
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
                                    "{} · {} created · {} merged · {} Agent starts",
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
    card_surface().child(row().justify_between().child(heading("Yearly activity")).child(metric_choices(ui,true)))
        .child(note(format!("{} – {} · Last 365 days · {}",dash.annual_start,dash.annual_end,ui.insights.metric.get().label())))
        .child(scroll().horizontal().w_full().element_ref(&annual_scroll)
            .content_offset(Point::new(px(10_000.),px(0.)))
            .on_layout(move |bounds: &Bounds<Points>| {
                let width = bounds.size.width.0;
                if (ui.insights.annual_width.get_untracked() - width).abs() > 0.5 {
                    ui.insights.annual_width.set(width);
                    annual_scroll.scroll_to_end(false);
                }
            }).child(grid))
        .child(row().justify_between().child(note(format!("{} active days · Current streak: {}d · Longest: {}d",dash.active_days,dash.current_streak,dash.longest_streak)))
            .child(row().child(note("Less")).children((0..=4).map(|n| view().w(px(10.)).h(px(10.)).rounded(px(2.)).bg(heatmap_level_bg(n)).into_element()).collect::<Vec<_>>()).child(note("More"))))
        .child(note("Select a tile to inspect that date. Faded tiles precede retained history. Project scope applies; the annual range stays fixed."))
}

fn duration_text(value: Option<u64>) -> String {
    value
        .map(format_duration_compact)
        .unwrap_or_else(|| "—".into())
}
fn render_efficiency(ui: Ui, dash: &Dashboard) -> View {
    let open = ui.insights.efficiency_open.get();
    let mut panel = card_surface().child(
        press_target()
            .w_full()
            .on_press(move |_: &()| intent(ui, "efficiency"))
            .accessibility_label("Toggle efficiency details")
            .child(
                row()
                    .justify_between()
                    .child(heading(if open {
                        "▾ Efficiency & timing"
                    } else {
                        "▸ Efficiency & timing"
                    }))
                    .child(note(format!(
                        "Cycle p50: {} · Queue p50: {}",
                        duration_text(dash.median_lead_time_ms),
                        duration_text(dash.median_queue_wait_ms)
                    ))),
            ),
    );
    if !open {
        return panel;
    }
    panel = panel
        .child(note(format!("Baseline: {}", dash.comparison_label)))
        .child(
            row()
                .child(label("Metric").w(px(205.)))
                .child(note("Current / Baseline / Change / Samples")),
        );
    for (title, current, previous, p) in [
        (
            "Execution → merge, p50",
            &dash.efficiency.cycle,
            &dash.efficiency.previous_cycle,
            50,
        ),
        (
            "Execution → merge, p90",
            &dash.efficiency.cycle,
            &dash.efficiency.previous_cycle,
            90,
        ),
        (
            "Queue → start, p50",
            &dash.efficiency.queue,
            &dash.efficiency.previous_queue,
            50,
        ),
        (
            "Queue → start, p90",
            &dash.efficiency.queue,
            &dash.efficiency.previous_queue,
            90,
        ),
    ] {
        let value = model::percentile(current, p);
        let baseline = model::percentile(previous, p);
        let delta = match (value, baseline) {
            (Some(a), Some(b)) => format!(
                "{}{}",
                if a >= b { "+" } else { "−" },
                format_duration_compact(a.abs_diff(b))
            ),
            _ => "—".into(),
        };
        let op = if title.starts_with("Queue") {
            "samples-queue"
        } else {
            "samples-cycle"
        };
        panel = panel.child(
            press_target()
                .w_full()
                .p_2()
                .hover(|s| s.bg(CARD_HOVER))
                .on_press(move |_: &()| intent(ui, op))
                .accessibility_label(format!("Inspect {title}"))
                .child(row().child(label(title).w(px(205.))).child(note(format!(
                    "{} / {} / {} / n={} vs {}",
                    duration_text(value),
                    duration_text(baseline),
                    delta,
                    current.len(),
                    previous.len()
                )))),
        );
    }
    panel = panel.child(note("Cycle includes elapsed waiting between first execution and merge. Queue samples belong to runs started in the selected period. Missing timestamps are excluded; small samples are descriptive."));
    let queue = ui.insights.queue_samples.get();
    let samples = if queue {
        &dash.efficiency.queue
    } else {
        &dash.efficiency.cycle
    };
    let pages = samples.len().div_ceil(20).max(1);
    let page = ui.insights.sample_page.get().min(pages - 1);
    panel = panel
        .child(
            row()
                .child(choice(ui, "Merged Card samples", "samples-cycle", !queue))
                .child(choice(ui, "Queue samples", "samples-queue", queue)),
        )
        .child(note(format!(
            "{} valid samples · longest duration first",
            samples.len()
        )));
    for sample in samples.iter().skip(page * 20).take(20) {
        let id = sample.card_id.clone();
        let begin = chrono::DateTime::from_timestamp_millis(sample.start as i64)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%m-%d %H:%M")
            .to_string();
        let end = chrono::DateTime::from_timestamp_millis(sample.end as i64)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%m-%d %H:%M")
            .to_string();
        panel = panel.child(
            press_target()
                .w_full()
                .p_2()
                .rounded(px(5.))
                .hover(|s| s.bg(CARD_HOVER))
                .disabled(id.is_empty())
                .accessibility_label(format!("Open timing sample #{}", sample.number))
                .on_press(move |_: &()| ui.open_card(id.clone()))
                .child(
                    view()
                        .gap_1()
                        .child(
                            label(format!(
                                "#{} {} · {}",
                                sample.number, sample.title, sample.project
                            ))
                            .truncate(),
                        )
                        .child(note(format!(
                            "{} → {} · {}{}",
                            begin,
                            end,
                            format_duration_compact(sample.duration),
                            sample
                                .profile
                                .as_ref()
                                .map(|p| format!(" · @{p}"))
                                .unwrap_or_default()
                        ))),
                ),
        );
    }
    if samples.is_empty() {
        panel = panel.child(note(
            "No samples with reliable start and end timestamps in this period.",
        ));
    }
    panel.child(
        row()
            .child(
                compact_action("Previous samples", Variant::Quiet, move |_| {
                    intent(ui, "samples-previous")
                })
                .disabled(page == 0),
            )
            .child(note(format!("Page {} / {pages}", page + 1)))
            .child(
                compact_action("Next samples", Variant::Quiet, move |_| {
                    intent(ui, "samples-next")
                })
                .disabled(page + 1 >= pages),
            ),
    )
}

fn render_card_drilldown(ui: Ui, dash: &Dashboard) -> View {
    let filter = ui.insights.drilldown.get();
    let mut panel = view().gap_2().child(
        row()
            .child(compact_action(
                "View created Cards",
                Variant::Quiet,
                move |_| intent(ui, "cards-created"),
            ))
            .child(compact_action(
                "View merged Cards",
                Variant::Quiet,
                move |_| intent(ui, "cards-merged"),
            )),
    );
    if filter.is_empty() {
        return panel;
    }
    let ids = if filter == "created" {
        &dash.created_ids
    } else {
        &dash.merged_ids
    };
    panel = panel.child(
        row()
            .child(note(format!("{} {} Cards", ids.len(), filter)))
            .child(compact_action(
                "Close Card list",
                Variant::Quiet,
                move |_| intent(ui, "cards-close"),
            )),
    );
    for id in ids.iter().take(100) {
        if let Some(card) = ui
            .snapshot
            .with(|s| s.cards.iter().find(|c| &c.id == id).cloned())
        {
            let id = id.clone();
            panel = panel.child(
                press_target()
                    .w_full()
                    .p_2()
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

fn dashboard(ui: Ui) -> Dashboard {
    let scope = ui.insights.scope_project.get();
    let range = ui.insights.range.get();
    let now = now_ms(ui);
    let profiles = ui.all_profiles.get();
    ui.snapshot.with(|s| {
        model::compute_view(
            s,
            (!scope.is_empty()).then_some(scope.as_str()),
            range,
            now,
            &chrono::Local,
            ui.insights.selected_date.get(),
            ui.insights.comparison.get(),
            ui.insights.metric.get(),
            ui.insights.hourly_metric.get(),
            ui.insights.trend_days.get(),
            &profiles,
        )
    })
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let dashboard = computed(move || dashboard(ui));
    let scroll_ref = element_ref();
    ui.insights.scroll.set(Some(scroll_ref));
    let scroll_key = "insights:dashboard".to_owned();
    let capture_key = scroll_key.clone();
    view().flex_1().min_h(px(0.)).min_w(px(0.)).bg(BACKGROUND)
        .child(scroll().scrollbar(false).flex_1().min_h(px(0.))
            .element_ref(&scroll_ref).content_offset(scroll_position(ui,&scroll_key)).on_scroll(move |e: &ScrollEvent| capture_scroll(ui,capture_key.clone(),e))
            .child(view().w_full().max_w(px(1180.)).min_w(px(0.)).p_5().gap_4().child(dynamic(move || {
                let dash = dashboard.get();
                let mut content = view().gap_4().min_w(px(0.)).child(render_period(ui,&dash)).child(render_kpis(&dash,ui.narrow.get()))
                    .child(render_card_drilldown(ui,&dash));
                if dash.range == RangeWindow::Today { content = content.child(render_hourly(ui,&dash)); }
                content = content.child(render_kanban_flow_chart(ui,&dash)).child(render_projects(ui,&dash)).child(render_agents(ui,&dash))
                    .child(render_heatmap_panel(ui,&dash)).child(render_efficiency(ui,&dash));
                let offline = ui.snapshot.with(|s| s.host_summaries.values().filter(|h| h.is_offline())
                    .map(|h| format!("{} (last sync: {})",h.name,h.last_synced_at.map(|t| chrono::DateTime::from_timestamp_millis(t as i64).unwrap().with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string()).unwrap_or_else(|| "unknown".into())))
                    .collect::<Vec<_>>().join(", "));
                content.child(note(format!("Retained history from {} · {} merged Cards have no reliable merge date and are excluded from date totals. {} Agent batches have no retained launch timestamp. Removed history may be unavailable.{}",
                    dash.history_start.map(|d| d.to_string()).unwrap_or_else(|| "—".into()),dash.unknown_merge_dates,dash.unknown_batch_starts,
                    if offline.is_empty() { String::new() } else { format!(" Offline hosts: {offline}; cached data may be stale.") }))).into_element()
            }))))
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
    fn insights_route_mounts_charts_and_switches_ranges() {
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
            snap.events.push(Event {
                id: "ev-1".into(),
                project_id: "p1".into(),
                card_id: "card-41".into(),
                sequence: 1,
                kind: "execution.queued".into(),
                actor: "system".into(),
                message: String::new(),
                data: json!({"terminal_id": "card-41:developer", "run_number": 1, "trigger": "workflow", "workflow_run_id": "wf-41"}),
                created_at: now_ms - 5_400_000,
            });
            snap.events.push(Event {
                id: "ev-2".into(),
                project_id: "p1".into(),
                card_id: "card-41".into(),
                sequence: 2,
                kind: "execution.reported".into(),
                actor: "agent:developer".into(),
                message: String::new(),
                data: json!({"state": "completed", "workflow_run_id": "wf-41"}),
                created_at: now_ms - 2_400_000,
            });

            ui.snapshot.set(snap);
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
        assert!(tree.has_mounted_text("Recent daily flow"));
        assert!(tree.has_mounted_text("Hourly comparison"));
        assert!(tree.has_mounted_text("Yearly activity"));
        assert!(tree.has_mounted_text("Agents used"));
        assert!(!tree.has_mounted_text("Daily Ledger"));
        assert!(!tree.has_mounted_text("Recent activity"));
        assert!(!tree.has_mounted_text("RUN RESULTS"));
        let toggle = *tree.labels.get("Toggle efficiency details").unwrap();
        assert!(core.component_event(toggle, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(
            tree.has_mounted_text("Execution → merge, p90"),
            "Mounted texts after expand: {:?}",
            tree.texts
                .iter()
                .filter(|(id, _)| tree.is_mounted(**id))
                .map(|(_, t)| t)
                .collect::<Vec<_>>()
        );
        assert!(tree.has_mounted_text("Queue → start, p90"));
        let sample_tab = *tree.labels.get("Queue samples").unwrap();
        assert!(core.component_event(sample_tab, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(
            tree.has_mounted_text(
                "No samples with reliable start and end timestamps in this period."
            )
        );

        // Switch range to 7D via segmented button
        let btn_7d = *tree.labels.get("7D").expect("7D segment button");
        assert!(core.component_event(btn_7d, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("Yearly activity"));
        assert!(!tree.has_mounted_text("Hourly comparison"));
    }
}
