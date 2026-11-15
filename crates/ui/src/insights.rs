//! Insights dashboard: daily Kanban merge throughput, Agent scheduling outcomes,
//! 12-week cadence heatmap, and role/trigger breakdown.

pub mod model;

use crate::{
    drafts::{capture_scroll, scroll_position},
    presenter::*,
};
use canban_ui_kit::{tokens::*, *};
use model::{
    Dashboard, DayBucket, HeatmapWeek, ProfileStat, RangeWindow, TriggerKind, TriggerStat,
    format_delta_pct, format_duration_compact,
};
use nsh_core::{element::AdaptiveColor, prelude::*};

const ALL_PROJECTS_OPTION: &str = "__all__";

#[derive(Clone, Copy)]
pub struct State {
    /// Empty string means `All Projects`; otherwise holds a `Project.id`.
    pub scope_project: Signal<String>,
    pub range: Signal<RangeWindow>,
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
            range: signal(RangeWindow::Days30),
            anchor_ms: signal(None),
        }
    }
}

pub fn set_range(ui: Ui, next: RangeWindow) {
    ui.insights.range.set(next);
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
            [RangeWindow::Days7, RangeWindow::Days30, RangeWindow::Days90]
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
    let delta_merges = format_delta_pct(dash.merged_total, dash.prev_merged_total);
    let merge_tone = if dash.merged_total >= dash.prev_merged_total {
        Tone::Success
    } else {
        Tone::Neutral
    };
    let wip_str = if dash.net_wip_delta > 0 {
        format!("+{}", dash.net_wip_delta)
    } else {
        dash.net_wip_delta.to_string()
    };
    let c1 = kpi_card(KpiSpec {
        mark: Icon::GitMerge,
        mark_tint: SUCCESS,
        eyebrow: "MERGED CARDS",
        hero: dash.merged_total.to_string(),
        pill_text: Some((delta_merges, merge_tone)),
        sub_primary: format!("{} local · {} PR", dash.local_merges, dash.pr_merges),
        sub_secondary: format!("{} created · Net WIP {wip_str}", dash.created_total),
        extra_bar: None,
    });

    let delta_runs = format_delta_pct(dash.scheduled_total, dash.prev_scheduled_total);
    let runs_tone = if dash.scheduled_total > 0 {
        Tone::Info
    } else {
        Tone::Neutral
    };
    let c2 = kpi_card(KpiSpec {
        mark: Icon::Agent,
        mark_tint: ACCENT,
        eyebrow: "SCHEDULED RUNS",
        hero: dash.scheduled_total.to_string(),
        pill_text: Some((delta_runs, runs_tone)),
        sub_primary: format!(
            "{} completed · {} active",
            dash.completed_runs, dash.active_runs
        ),
        sub_secondary: format!(
            "{} blocked · {} failed",
            dash.blocked_runs, dash.failed_runs
        ),
        extra_bar: None,
    });

    let rate_hero = dash
        .success_rate_pct
        .map(|p| format!("{p}%"))
        .unwrap_or_else(|| "—".into());
    let rate_tone = match dash.success_rate_pct {
        Some(p) if p >= 80 => Tone::Success,
        Some(p) if p >= 50 => Tone::Attention,
        Some(_) => Tone::Danger,
        None => Tone::Neutral,
    };
    let total_r = dash.scheduled_total.max(1);
    let outcome_bar = view()
        .w_full()
        .h(px(6.))
        .rounded_full()
        .bg(INSET)
        .overflow_hidden()
        .flex_row()
        .children(if dash.scheduled_total == 0 {
            Vec::new()
        } else {
            [
                (dash.completed_runs, SUCCESS),
                (dash.blocked_runs, ATTENTION),
                (dash.failed_runs, DANGER),
                (dash.active_runs, ACCENT),
            ]
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, col)| {
                let frac = (n as f32 / total_r as f32).clamp(0.03, 1.0);
                view().h_full().w(relative(frac)).bg(col).into_element()
            })
            .collect::<Vec<_>>()
        })
        .into_element();
    let resolved_cnt = dash.completed_runs + dash.blocked_runs + dash.failed_runs;
    let c3 = kpi_card(KpiSpec {
        mark: Icon::Check,
        mark_tint: SUCCESS,
        eyebrow: "RUN SUCCESS RATE",
        hero: rate_hero,
        pill_text: Some((format!("{resolved_cnt} resolved"), rate_tone)),
        sub_primary: "Completed ÷ resolved runs".into(),
        sub_secondary: if dash.active_runs > 0 {
            format!("Excludes {} in-flight active runs", dash.active_runs)
        } else {
            format!("Across {}", dash.range.description())
        },
        extra_bar: Some(outcome_bar),
    });

    let lead_hero = dash
        .median_lead_time_ms
        .map(format_duration_compact)
        .unwrap_or_else(|| "—".into());
    let queue_str = dash
        .median_queue_wait_ms
        .map(format_duration_compact)
        .unwrap_or_else(|| "—".into());
    let c4 = kpi_card(KpiSpec {
        mark: Icon::Trigger,
        mark_tint: ATTENTION,
        eyebrow: "CYCLE & QUEUE",
        hero: lead_hero,
        pill_text: Some(("p50 lead".into(), Tone::Neutral)),
        sub_primary: "Execution start → Merge median".into(),
        sub_secondary: format!("Queue wait p50: {queue_str}"),
        extra_bar: None,
    });

    if narrow {
        view()
            .gap_3()
            .child(view().flex_row().gap_3().child(c1).child(c2))
            .child(view().flex_row().gap_3().child(c3).child(c4))
    } else {
        view()
            .flex_row()
            .gap_3()
            .child(c1)
            .child(c2)
            .child(c3)
            .child(c4)
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

fn render_kanban_flow_chart(dash: &Dashboard) -> View {
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
        .map(|day| {
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
            let tip_created = format!("Created: {}", day.created_cards);

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
                            label("Daily Kanban Flow")
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(14.)),
                        )
                        .child(
                            muted("Cards created vs merged per local day · only merged Cards count as completed")
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

fn render_agent_scheduling_chart(dash: &Dashboard) -> View {
    let top_tick = nice_ceil_tick(dash.max_daily_runs);
    let n = dash.days.len();
    let col_gap = if n > 45 {
        1.5
    } else if n > 14 {
        3.5
    } else {
        12.0
    };
    let bar_max_w = if n > 45 {
        7.0
    } else if n > 14 {
        14.0
    } else {
        34.0
    };
    let usable_h = CHART_HEIGHT - 8.0;

    let columns = dash
        .days
        .iter()
        .map(|day| {
            let total = day.total_runs();
            let total_h = if total == 0 {
                0.0
            } else {
                (total as f32 / top_tick as f32 * usable_h).max(3.0)
            };

            let segments = if total == 0 {
                Vec::new()
            } else {
                [
                    (day.active_runs, ACCENT),
                    (day.failed_runs, DANGER),
                    (day.blocked_runs, ATTENTION),
                    (day.completed_runs, SUCCESS),
                ]
                .into_iter()
                .filter(|(cnt, _)| *cnt > 0)
                .map(|(cnt, color)| {
                    let seg_h = (cnt as f32 / total as f32 * total_h).max(2.0);
                    view().w_full().h(px(seg_h)).bg(color).into_element()
                })
                .collect::<Vec<_>>()
            };

            let tip_title = format!("{} ({}) · {} runs", day.date_key, day.weekday_label, total);
            let tip_breakdown = format!(
                "{} ok · {} blocked · {} failed · {} active",
                day.completed_runs, day.blocked_runs, day.failed_runs, day.active_runs
            );

            view()
                .flex_1()
                .min_w(px(0.))
                .h(px(CHART_HEIGHT))
                .rounded(px(4.))
                .hover(|s| s.bg(CARD_HOVER))
                .justify_end()
                .items_center()
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
                        .child(muted(tip_breakdown).text_size(px(11.5))),
                )
                .child(
                    view()
                        .w_full()
                        .max_w(px(bar_max_w))
                        .h(px(total_h))
                        .rounded(px(2.))
                        .overflow_hidden()
                        .justify_end()
                        .children(segments),
                )
                .into_element()
        })
        .collect::<Vec<_>>();

    let ticks = x_axis_ticks(&dash.days);

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
                        .child(
                            label("Daily Agent Scheduling")
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(14.)),
                        )
                        .child(
                            muted("Scheduled Agent runs stacked by execution outcome")
                                .text_size(px(11.5)),
                        ),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_3()
                        .child(legend_swatch(SUCCESS, "Completed"))
                        .child(legend_swatch(ATTENTION, "Blocked"))
                        .child(legend_swatch(DANGER, "Failed"))
                        .child(legend_swatch(ACCENT, "Active")),
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

fn heatmap_level_bg(level: u8) -> AdaptiveColor {
    match level {
        0 => INSET.resolve(),
        1 => alpha_tint(SUCCESS, 0.28),
        2 => alpha_tint(SUCCESS, 0.50),
        3 => alpha_tint(SUCCESS, 0.75),
        _ => SUCCESS.resolve(),
    }
}

fn render_heatmap_panel(weeks: &[HeatmapWeek]) -> View {
    let cell_size = 15.0;
    let cell_gap = 3.0;

    let month_headers = weeks
        .iter()
        .map(|w| {
            view()
                .w(px(cell_size))
                .h(px(14.))
                .child(if let Some(m) = &w.month_header {
                    text(m.clone())
                        .w(px(28.))
                        .flex_shrink_0()
                        .text_size(px(10.))
                        .text_color(MUTED)
                        .into_element()
                } else {
                    view().into_element()
                })
                .into_element()
        })
        .collect::<Vec<_>>();

    let weekday_labels = ["Mon", "", "Wed", "", "Fri", "", "Sun"]
        .into_iter()
        .map(|lbl| {
            view()
                .h(px(cell_size))
                .justify_center()
                .pr_1p5()
                .child(
                    text(lbl)
                        .font_family(MONO_FONT)
                        .text_size(px(10.))
                        .text_color(MUTED),
                )
                .into_element()
        })
        .collect::<Vec<_>>();

    let week_cols = weeks
        .iter()
        .map(|w| {
            let cells = w
                .cells
                .iter()
                .map(|cell| {
                    if cell.is_future {
                        return view()
                            .w(px(cell_size))
                            .h(px(cell_size))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(BORDER)
                            .opacity(0.25)
                            .into_element();
                    }
                    let lvl = cell.level;
                    let is_today = cell.is_today;
                    let tip_head = format!(
                        "{} ({}){}",
                        cell.date_key,
                        cell.weekday_label,
                        if is_today { " · Today" } else { "" }
                    );
                    let tip_body = format!(
                        "{} merged · {} scheduled runs",
                        cell.merged_cards, cell.total_runs
                    );

                    view()
                        .w(px(cell_size))
                        .h(px(cell_size))
                        .rounded(px(3.))
                        .bg(move || heatmap_level_bg(lvl))
                        .border_1()
                        .border_color(if is_today { TEXT } else { CLEAR })
                        .hover(|s| s.border_color(ACCENT))
                        .tooltip(
                            tooltip_box()
                                .gap_0p5()
                                .child(
                                    text(tip_head)
                                        .font_family(MONO_FONT)
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(TEXT),
                                )
                                .child(muted(tip_body).text_size(px(11.5))),
                        )
                        .into_element()
                })
                .collect::<Vec<_>>();

            view().gap(px(cell_gap)).children(cells).into_element()
        })
        .collect::<Vec<_>>();

    let legend_cells = (0u8..=4)
        .map(|lvl| {
            view()
                .w(px(10.))
                .h(px(10.))
                .rounded(px(2.))
                .bg(move || heatmap_level_bg(lvl))
                .into_element()
        })
        .collect::<Vec<_>>();

    card_surface()
        .flex_shrink_0()
        .justify_between()
        .child(
            view()
                .gap_0p5()
                .child(
                    label("Activity Cadence · 12 Weeks")
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(14.)),
                )
                .child(
                    muted("Daily merges + Agent runs across the past 12 weeks").text_size(px(11.5)),
                ),
        )
        .child(
            view()
                .gap_1()
                .child(
                    view()
                        .pl(px(30.))
                        .flex_row()
                        .gap(px(cell_gap))
                        .children(month_headers),
                )
                .child(
                    view()
                        .flex_row()
                        .items_start()
                        .child(view().gap(px(cell_gap)).children(weekday_labels))
                        .child(view().flex_row().gap(px(cell_gap)).children(week_cols)),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .pt_1()
                .child(muted("Hover any day square for totals").text_size(px(11.)))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .flex_shrink_0()
                        .child(muted("Less").text_size(px(10.5)))
                        .children(legend_cells)
                        .child(muted("More").text_size(px(10.5))),
                ),
        )
}

fn render_roles_and_triggers_panel(
    ui: Ui,
    profiles: &[ProfileStat],
    triggers: &[TriggerStat],
) -> View {
    let all_cfg_profiles = ui.all_profiles.get();
    let max_role_runs = profiles.first().map(|p| p.total_runs).unwrap_or(1).max(1);

    let role_rows = if profiles.is_empty() {
        vec![
            muted("No scheduled Agent runs in this time range.")
                .text_size(px(CAPTION_SIZE))
                .into_element(),
        ]
    } else {
        profiles
            .iter()
            .take(6)
            .map(|p| {
                let mark = all_cfg_profiles
                    .iter()
                    .find(|cfg_p| cfg_p.id == p.profile_id)
                    .map(|cfg_p| crate::profiles::provider(cfg_p).0)
                    .unwrap_or_else(|| crate::profiles::agent_icon(&p.profile_id));
                let resolved = p.completed_runs + p.blocked_runs + p.failed_runs;
                let rate_txt = (p.completed_runs * 100 + resolved / 2)
                    .checked_div(resolved)
                    .map(|pct| format!(" · {pct}% ok"))
                    .unwrap_or_default();
                let bar_frac = (p.total_runs as f32 / max_role_runs as f32).clamp(0.04, 1.0);
                let total_p = p.total_runs.max(1);
                let segments = [
                    (p.completed_runs, SUCCESS),
                    (p.blocked_runs, ATTENTION),
                    (p.failed_runs, DANGER),
                    (p.active_runs, ACCENT),
                ]
                .into_iter()
                .filter(|(c, _)| *c > 0)
                .map(|(c, col)| {
                    view()
                        .h_full()
                        .w(relative(c as f32 / total_p as f32))
                        .bg(col)
                        .into_element()
                })
                .collect::<Vec<_>>();

                view()
                    .gap_1()
                    .child(
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
                                    .min_w(px(0.))
                                    .child(icon_colored(mark, 14.))
                                    .child(
                                        text(format!("@{}", p.profile_id))
                                            .font_family(MONO_FONT)
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(TEXT)
                                            .truncate(),
                                    ),
                            )
                            .child(
                                text(format!("{} runs{rate_txt}", p.total_runs))
                                    .font_family(MONO_FONT)
                                    .text_size(px(11.5))
                                    .text_color(MUTED),
                            ),
                    )
                    .child(
                        view()
                            .w_full()
                            .h(px(6.))
                            .rounded_full()
                            .bg(INSET)
                            .overflow_hidden()
                            .child(
                                view()
                                    .h_full()
                                    .w(relative(bar_frac))
                                    .rounded_full()
                                    .overflow_hidden()
                                    .flex_row()
                                    .children(segments),
                            ),
                    )
                    .into_element()
            })
            .collect::<Vec<_>>()
    };

    let trigger_rows = triggers
        .iter()
        .map(|t| {
            let trig_icon = match t.kind {
                TriggerKind::Workflow => Icon::Trigger,
                TriggerKind::Mention => Icon::MessageSquare,
                TriggerKind::Interactive => Icon::Terminal,
            };
            view()
                .flex_row()
                .items_center()
                .gap_2p5()
                .child(
                    view()
                        .w(px(156.))
                        .flex_shrink_0()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .child(icon(trig_icon).w(px(13.)).h(px(13.)))
                        .child(label(t.kind.label()).text_size(px(12.)).truncate()),
                )
                .child(
                    view()
                        .flex_1()
                        .h(px(6.))
                        .rounded_full()
                        .bg(INSET)
                        .overflow_hidden()
                        .child(if t.count > 0 {
                            view()
                                .h_full()
                                .w(relative((t.share_pct as f32 / 100.0).clamp(0.04, 1.0)))
                                .rounded_full()
                                .bg(ACCENT)
                                .into_element()
                        } else {
                            view().into_element()
                        }),
                )
                .child(
                    view().w(px(76.)).flex_shrink_0().items_end().child(
                        text(format!("{} ({}%)", t.count, t.share_pct))
                            .font_family(MONO_FONT)
                            .text_size(px(11.5))
                            .text_color(MUTED),
                    ),
                )
                .into_element()
        })
        .collect::<Vec<_>>();

    card_surface()
        .flex_1()
        .min_w(px(0.))
        .gap_3()
        .child(
            view()
                .gap_0p5()
                .child(
                    label("Agent Roles & Scheduling Triggers")
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(14.)),
                )
                .child(muted("Throughput by @identity and dispatch origin").text_size(px(11.5))),
        )
        .child(view().gap_2().children(role_rows))
        .child(view().h(px(1.)).bg(BORDER))
        .child(view().gap_1p5().children(trigger_rows))
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let dashboard = computed(move || {
        let scope = ui.insights.scope_project.get();
        let range = ui.insights.range.get();
        let now_ms = ui
            .insights
            .anchor_ms
            .get()
            .unwrap_or_else(canban_types::now);
        ui.snapshot.with(|s| {
            model::compute(
                s,
                (!scope.is_empty()).then_some(scope.as_str()),
                range,
                now_ms,
                &chrono::Local,
            )
        })
    });

    let scroll_ref = element_ref();
    let scroll_key = "insights:dashboard".to_owned();
    let scroll_capture_key = scroll_key.clone();

    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .relative()
        .bg(BACKGROUND)
        .child(
            scroll()
                .element_ref(&scroll_ref)
                .scrollbar(false)
                .flex_1()
                .min_h(px(0.))
                .content_offset(scroll_position(ui, &scroll_key))
                .on_scroll(move |e: &ScrollEvent| capture_scroll(ui, scroll_capture_key.clone(), e))
                .child(
                    view()
                        .w_full()
                        .max_w(px(1180.))
                        .min_w(px(0.))
                        .p_5()
                        .gap_4()
                        .child(dynamic(move || {
                            let dash = dashboard.get();
                            let narrow = ui.narrow.get();

                            let bottom_row = if narrow {
                                view()
                                    .gap_4()
                                    .child(render_heatmap_panel(&dash.heatmap_weeks))
                                    .child(render_roles_and_triggers_panel(
                                        ui,
                                        &dash.profile_stats,
                                        &dash.trigger_stats,
                                    ))
                            } else {
                                view()
                                    .flex_row()
                                    .items_stretch()
                                    .gap_4()
                                    .child(render_heatmap_panel(&dash.heatmap_weeks))
                                    .child(render_roles_and_triggers_panel(
                                        ui,
                                        &dash.profile_stats,
                                        &dash.trigger_stats,
                                    ))
                            };

                            view()
                                .gap_4()
                                .min_w(px(0.))
                                .child(render_kpis(&dash, narrow))
                                .child(render_kanban_flow_chart(&dash))
                                .child(render_agent_scheduling_chart(&dash))
                                .child(bottom_row)
                                .into_element()
                        })),
                ),
        )
        .child(
            vertical_scrollbar(scroll_ref)
                .absolute()
                .top(px(6.))
                .bottom(px(6.))
                .right(px(4.)),
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
                .h(px(900.))
                .child(crate::frame::screen(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1400.), px(900.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<InsightsHarness>());
        let mut tree = Tracker::new();
        pump(&mut core, &mut tree);

        assert!(tree.has_mounted_text("Insights"));
        assert!(tree.has_mounted_text("Daily Kanban Flow"));
        assert!(tree.has_mounted_text("Daily Agent Scheduling"));
        assert!(tree.has_mounted_text("Activity Cadence · 12 Weeks"));
        assert!(tree.has_mounted_text("Agent Roles & Scheduling Triggers"));
        assert!(!tree.has_mounted_text("Daily Ledger"));

        // Switch range to 7D via segmented button
        let btn_7d = *tree.labels.get("7D").expect("7D segment button");
        assert!(core.component_event(btn_7d, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(tree.has_mounted_text("Across last 7 days"));
    }
}
