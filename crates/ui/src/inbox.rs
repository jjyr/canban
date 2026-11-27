//! Daily Inbox: Google-Inbox-style single-page workspace with automatic project/source stacks,
//! inline triage, live Agent Terminal status/output preview, Schedule pulse, and End-of-Day Defer.
pub mod state;
pub use state::{
    State, cutoff_minutes_from_utility, defer_remaining_all, init, local_tz_offset_secs,
};

use crate::presenter::*;
use canban_notifications::{
    DailyInbox, DeferredInboxItem, DoneInboxItem, GitHubProjectStack, GitHubStackItem,
    InboxCardWork, InboxTerminalWork, NeedsYouItem, NeedsYouKind, ScheduleCardStack,
    SchedulePulseItem,
};
use canban_types::{GitHubReference, HerdrStatus};
use canban_ui_kit::{compact_home, tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;
use state::*;

fn engine_icon(engine: Option<&str>) -> Icon {
    match engine
        .map(|e| e.strip_prefix("herdr:").unwrap_or(e))
        .unwrap_or("")
    {
        "codex" => Icon::Codex,
        "claude" => Icon::Claude,
        "agy" | "antigravity" => Icon::Agy,
        "gemini" => Icon::Gemini,
        "opencode" => Icon::OpenCode,
        "cursor" | "cursor-agent" => Icon::Cursor,
        "pi" => Icon::Pi,
        "omp" => Icon::Omp,
        "copilot" => Icon::Copilot,
        "droid" => Icon::Droid,
        "amp" => Icon::Amp,
        "kimi" => Icon::Kimi,
        "kiro" | "kiro-cli" => Icon::Kiro,
        "qwen" => Icon::Qwen,
        "devin" => Icon::Devin,
        "cline" => Icon::Cline,
        "grok" => Icon::Grok,
        "hermes" => Icon::Hermes,
        "kilo" => Icon::Kilo,
        "qodercli" | "qoder" => Icon::Qoder,
        "letta" => Icon::Letta,
        "mastracode" | "mastra" => Icon::Mastra,
        "maki" => Icon::Maki,
        "muse" => Icon::Muse,
        _ => Icon::Agent,
    }
}

fn terminal_status_visual(status: HerdrStatus) -> (&'static str, Tone, bool) {
    match status {
        HerdrStatus::Blocked => ("等待输入", Tone::Attention, false),
        HerdrStatus::Done => ("已完成未关", Tone::Success, false),
        HerdrStatus::Working => ("运行中", Tone::Info, true),
        HerdrStatus::Starting | HerdrStatus::Queued => ("启动中", Tone::Info, true),
        HerdrStatus::Idle => ("空闲等待", Tone::Neutral, false),
        HerdrStatus::Error => ("运行出错", Tone::Danger, false),
        HerdrStatus::Disconnected => ("已断开", Tone::Danger, false),
        _ => ("打开中", Tone::Neutral, false),
    }
}

fn shorten_path(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    compact_home(path, &home)
}

fn stacked_deck_lip() -> View {
    view()
        .mx_2p5()
        .h(px(4.))
        .bg(INSET)
        .border_l_1()
        .border_r_1()
        .border_b_1()
        .border_color(BORDER)
        .rounded_b(px(6.))
}

fn section_header(
    title: impl Into<String>,
    count: usize,
    tone: Tone,
    trailing: Option<AnyElement>,
) -> View {
    let mut row = view()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .pt_1()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(status_dot(match tone {
                    Tone::Attention => ATTENTION,
                    Tone::Danger => DANGER,
                    Tone::Success => SUCCESS,
                    Tone::Info => ACCENT,
                    Tone::Highlight => HIGHLIGHT,
                    Tone::Neutral => MUTED,
                }))
                .child(
                    label(title.into())
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(TEXT),
                )
                .child(badge(count.to_string())),
        );
    if let Some(el) = trailing {
        row = row.child(el);
    }
    row
}

fn inline_reply_box(ui: Ui, expected: ReplyTarget) -> impl IntoElement {
    let is_open = computed(move || {
        ui.inbox
            .reply_target
            .with(|t| t.as_ref() == Some(&expected))
    });
    when(
        move || is_open.get(),
        move || {
            let placeholder = match ui.inbox.reply_target.get_untracked() {
                Some(ReplyTarget::Terminal { ref label, .. }) => {
                    format!("直接回复到 {label}（回车发送）…")
                }
                Some(ReplyTarget::GitHub { ref label, .. }) => {
                    format!("回复到 GitHub {label}…")
                }
                Some(ReplyTarget::Card { ref label, .. }) => {
                    format!("回复到 {label}（支持 @agent）…")
                }
                None => "输入回复内容…".into(),
            };
            view()
                .mt_2()
                .pt_2()
                .border_t_1()
                .border_color(BORDER)
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    field(ui.inbox.reply_draft, &placeholder)
                        .flex_1()
                        .min_w(px(0.))
                        .on_submit(move |_: &str| submit_inline_reply(ui)),
                )
                .child(
                    compact_action("发送", Variant::Primary, move |_| submit_inline_reply(ui))
                        .disabled(move || {
                            ui.inbox.reply_sending.get()
                                || ui.inbox.reply_draft.with(|d| d.trim().is_empty())
                        }),
                )
                .child(compact_action("取消", Variant::Quiet, move |_| {
                    ui.inbox.reply_target.set(None);
                }))
        },
    )
}

/// Top 42pt unified chrome bar for the Daily Inbox route.
pub fn chrome_bar(ui: Ui) -> impl IntoElement {
    let summary = computed(move || {
        let inbox = ui.daily_inbox.get();
        (
            inbox.completed_today,
            inbox.remaining_today,
            inbox.total_today,
            inbox.deferred_items.len(),
            inbox.cutoff_minutes,
        )
    });
    let drawer = ui.inbox.drawer;

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
                .child(icon_tinted(Icon::Inbox, ACCENT).w(px(15.)).h(px(15.)))
                .child(
                    label("Inbox")
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(13.5)),
                )
                .child(dynamic(move || {
                    let (done, remaining, total, _, _) = summary.get();
                    let ratio = if total == 0 {
                        1.0_f32
                    } else {
                        (done as f32 / total as f32).clamp(0.0, 1.0)
                    };
                    let fill_w = (ratio * 58.0).round();
                    let all_clear = remaining == 0;
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .pl_1()
                        .child(
                            view()
                                .w(px(58.))
                                .h(px(5.))
                                .rounded_full()
                                .bg(INSET)
                                .border_1()
                                .border_color(BORDER)
                                .overflow_hidden()
                                .child(view().w(px(fill_w)).h_full().bg(if all_clear {
                                    SUCCESS
                                } else {
                                    ACCENT
                                })),
                        )
                        .child(
                            muted(if all_clear {
                                format!("已清零 · {done}/{total}")
                            } else {
                                format!("{done}/{total}")
                            })
                            .text_size(px(11.5))
                            .font_family(MONO_FONT),
                        )
                        .into_element()
                })),
        )
        .child(view().flex_1().h_full().window_drag(true))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .flex_shrink_0()
                .window_drag(false)
                .child(dynamic(move || {
                    let (done, _, _, deferred, _) = summary.get();
                    let deferred_lbl = if deferred > 0 {
                        format!("已推迟 {deferred}")
                    } else {
                        "已推迟".into()
                    };
                    let done_lbl = if done > 0 {
                        format!("已完成 {done}")
                    } else {
                        "已完成".into()
                    };
                    segmented()
                        .child(compact_segment(
                            "今天",
                            None,
                            move || drawer.get() == "today",
                            move |_| drawer.set("today".into()),
                        ))
                        .child(compact_segment(
                            deferred_lbl,
                            None,
                            move || drawer.get() == "deferred",
                            move |_| drawer.set("deferred".into()),
                        ))
                        .child(compact_segment(
                            done_lbl,
                            None,
                            move || drawer.get() == "done",
                            move |_| drawer.set("done".into()),
                        ))
                        .into_element()
                }))
                .child(when(
                    move || summary.get().1 > 0 && drawer.get() == "today",
                    move || {
                        compact_action("推迟剩余全部 ▾", Variant::Secondary, move |_| {
                            defer_remaining_all(ui)
                        })
                    },
                ))
                .child(compact_action(
                    move || format!("日切 {} ▾", format_cutoff(summary.get().4)),
                    Variant::Quiet,
                    move |_| open_cutoff_menu(ui),
                ))
                .child(chrome_button_disabled(
                    Icon::Refresh,
                    "刷新 GitHub 通知与终端预览",
                    move || ui.inbox.github_loading.get(),
                    move |_| {
                        refresh_github_notifications(ui, true);
                        prefetch_agent_terminal_previews(ui);
                    },
                )),
        )
}

fn render_schedule_pulse(ui: Ui, runs: &[SchedulePulseItem]) -> Option<View> {
    if runs.is_empty() {
        return None;
    }
    let mut pills = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_1p5()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .pr_1()
                .child(icon_tinted(Icon::Clock, MUTED).w(px(12.5)).h(px(12.5)))
                .child(muted("今日定时").text_size(px(11.5))),
        );

    for run in runs.iter().take(8) {
        let time_str = format_clock_hm(run.scheduled_at);
        let (mark, tone) = match run.state.as_str() {
            "completed" | "succeeded" => ("✓", Tone::Success),
            "failed" => ("✗", Tone::Danger),
            "blocked" => ("!", Tone::Attention),
            "running" | "starting" | "queued" => ("●", Tone::Info),
            _ => ("·", Tone::Neutral),
        };
        let extra = if run.creates_card && run.card_number.is_some() {
            format!(" #{}", run.card_number.unwrap_or(0))
        } else {
            String::new()
        };
        let label_text = format!("{mark} {time_str} {}{extra}", run.schedule_name);
        let pid = run.project_id.clone();
        let sid = run.schedule_id.clone();
        let card_id = run.card_id.clone();
        pills = pills.child(status_button(
            label_text,
            tone,
            matches!(run.state.as_str(), "running" | "starting"),
            None,
            true,
            move |_| {
                if let Some(cid) = &card_id {
                    ui.open_card(cid.clone());
                } else {
                    crate::schedules::open(ui, Some(pid.clone()), Some(sid.clone()));
                }
            },
        ));
    }

    Some(
        view()
            .px_3()
            .py_2()
            .bg(SURFACE)
            .border_1()
            .border_color(BORDER)
            .rounded(px(8.))
            .child(pills),
    )
}

fn render_undo_banner(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let Some(banner) = ui.inbox.undo_banner.get() else {
            return view().into_element();
        };
        view()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .bg(SELECTED)
            .border_1()
            .border_color(ACCENT)
            .rounded(px(8.))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(icon_tinted(Icon::Check, ACCENT).w(px(14.)).h(px(14.)))
                    .child(label(banner.label).text_size(px(12.5))),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .child(compact_action("撤销", Variant::Primary, move |_| {
                        undo_last(ui)
                    }))
                    .child(chrome_button(Icon::Close, "关闭提示", move |_| {
                        ui.inbox.undo_banner.set(None);
                    })),
            )
            .into_element()
    })
}

fn render_needs_you_item(ui: Ui, item: NeedsYouItem, now_ms: u64) -> View {
    let key_defer = item.key.clone();
    let key_dismiss = item.key.clone();
    let wm = item.watermark;
    let rel = format_relative_zh(item.timestamp_ms, now_ms);

    match item.kind {
        NeedsYouKind::Card {
            card_id,
            card_number,
            card_title,
            project_name,
            host_label,
            terminal_id,
            headline,
            detail,
            body,
            actor,
            is_review,
            is_blocked,
            unread_seq,
            unread_count,
            can_quick_merge,
            ..
        } => {
            let tone = if is_blocked {
                Tone::Attention
            } else if is_review {
                Tone::Highlight
            } else {
                Tone::Info
            };
            let reply_target = ReplyTarget::Card {
                card_id: card_id.clone(),
                label: format!("#{card_number} {card_title}"),
            };
            let reply_target_btn = reply_target.clone();
            let cid_open = card_id.clone();
            let cid_term = card_id.clone();
            let cid_merge = card_id.clone();
            let cid_dismiss = card_id.clone();
            let term_open = terminal_id.clone();
            let defer_title = format!("#{card_number} {card_title}");
            let dismiss_title = format!("#{card_number} {card_title}");

            let mut top = view()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w(px(0.))
                .child(status_dot(if is_blocked {
                    ATTENTION
                } else if is_review {
                    HIGHLIGHT
                } else {
                    ACCENT
                }))
                .child(badge(project_name))
                .child(
                    muted(format!("#{card_number}"))
                        .font_family(MONO_FONT)
                        .text_size(px(12.)),
                )
                .child(
                    label(card_title.clone())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(13.))
                        .truncate()
                        .flex_1()
                        .min_w(px(0.)),
                );
            if let Some(h) = host_label {
                top = top.child(badge(h));
            }
            if let Some(a) = actor {
                top = top.child(status(a, tone));
            }
            if unread_count > 1 {
                top = top.child(badge(format!("+{unread_count}")));
            }
            if !rel.is_empty() {
                top = top.child(muted(rel).text_size(px(11.5)).flex_shrink_0());
            }

            let excerpt = body
                .filter(|b| !b.trim().is_empty())
                .or(detail.filter(|d| !d.trim().is_empty() && d != &headline));

            let mut actions = view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_end()
                .gap_1p5()
                .pt_1()
                .child(compact_action(
                    "直接回复",
                    Variant::Secondary,
                    move |_| start_inline_reply(ui, reply_target_btn.clone()),
                ));
            if can_quick_merge {
                actions = actions.child(compact_action(
                    "快速合入",
                    Variant::Primary,
                    move |_| {
                        if let Some(card) = ui
                            .snapshot
                            .with_untracked(|s| s.cards.iter().find(|c| c.id == cid_merge).cloned())
                        {
                            crate::card::quick_merge_start(ui, card);
                        }
                    },
                ));
            }
            if let Some(tid) = term_open {
                actions =
                    actions.child(compact_action("看终端 ↗", Variant::Quiet, move |_| {
                        ui.open_card_with_terminal(cid_term.clone(), Some(tid.clone()));
                    }));
            }
            actions = actions
                .child(compact_action(
                    "打开 Card ↗",
                    Variant::Quiet,
                    move |_| {
                        ui.open_card(cid_open.clone());
                    },
                ))
                .child(compact_action("推迟 ▾", Variant::Quiet, move |_| {
                    open_defer_menu(ui, vec![(key_defer.clone(), wm)], defer_title.clone())
                }))
                .child(compact_action("已处理 ✓", Variant::Quiet, move |_| {
                    if unread_seq > 0 {
                        crate::overview::mark_card_read(ui, cid_dismiss.clone(), unread_seq);
                    }
                    dismiss_items(
                        ui,
                        vec![(key_dismiss.clone(), wm)],
                        format!("已处理 {dismiss_title}"),
                    );
                }));

            let mut card = view()
                .p_3()
                .gap_1p5()
                .bg(SURFACE)
                .border_1()
                .border_color(if is_blocked { ATTENTION } else { BORDER })
                .rounded(px(8.))
                .child(top)
                .child(
                    label(headline)
                        .text_size(px(12.5))
                        .text_color(if is_blocked { ATTENTION } else { TEXT }),
                );
            if let Some(ex) = excerpt {
                card = card.child(
                    view()
                        .px_2p5()
                        .py_1p5()
                        .bg(INSET)
                        .border_l_1()
                        .border_color(if is_blocked { ATTENTION } else { ACCENT })
                        .rounded(px(4.))
                        .child(muted(ex).text_size(px(12.)).select_text()),
                );
            }
            card.child(actions)
                .child(inline_reply_box(ui, reply_target))
        }
        NeedsYouKind::ScheduleFailed {
            schedule_id,
            schedule_name,
            project_id,
            project_name,
            host_label,
            state,
            summary,
            error,
            terminal_id,
            ..
        } => {
            let defer_title = format!("Schedule · {schedule_name}");
            let dismiss_title = defer_title.clone();
            let pid_retry = project_id.clone();
            let sid_retry = schedule_id.clone();
            let pid_open = project_id.clone();
            let sid_open = schedule_id.clone();
            let is_failed = state == "failed";

            let mut top = view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(status_dot(if is_failed { DANGER } else { ATTENTION }))
                .child(badge(project_name))
                .child(
                    label(format!("Schedule · {schedule_name}"))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(13.))
                        .flex_1()
                        .truncate(),
                )
                .child(status(
                    if is_failed {
                        "执行失败"
                    } else {
                        "等待确认"
                    },
                    if is_failed {
                        Tone::Danger
                    } else {
                        Tone::Attention
                    },
                ));
            if let Some(h) = host_label {
                top = top.child(badge(h));
            }
            if !rel.is_empty() {
                top = top.child(muted(rel).text_size(px(11.5)));
            }

            let detail_text = error
                .or(summary)
                .unwrap_or_else(|| "定时任务未正常完成，可重试或查看详情。".into());

            let mut actions = view()
                .flex_row()
                .items_center()
                .justify_end()
                .gap_1p5()
                .pt_1()
                .child(compact_action("重试", Variant::Secondary, move |_| {
                    dispatch(
                        ui,
                        "schedule.trigger",
                        json!({ "project_id": pid_retry, "schedule_id": sid_retry }),
                    );
                    ui.notify("已重新触发定时任务");
                }));
            if let Some(tid) = terminal_id {
                actions =
                    actions.child(compact_action("看终端 ↗", Variant::Quiet, move |_| {
                        ui.open_terminal(tid.clone());
                    }));
            }
            actions = actions
                .child(compact_action(
                    "查看详情 ↗",
                    Variant::Quiet,
                    move |_| {
                        crate::schedules::open(ui, Some(pid_open.clone()), Some(sid_open.clone()));
                    },
                ))
                .child(compact_action("推迟 ▾", Variant::Quiet, move |_| {
                    open_defer_menu(ui, vec![(key_defer.clone(), wm)], defer_title.clone())
                }))
                .child(compact_action("忽略 ✓", Variant::Quiet, move |_| {
                    dismiss_items(
                        ui,
                        vec![(key_dismiss.clone(), wm)],
                        format!("已忽略 {dismiss_title}"),
                    )
                }));

            view()
                .p_3()
                .gap_1p5()
                .bg(SURFACE)
                .border_1()
                .border_color(if is_failed { DANGER } else { ATTENTION })
                .rounded(px(8.))
                .child(top)
                .child(
                    view()
                        .px_2p5()
                        .py_1p5()
                        .bg(INSET)
                        .border_l_1()
                        .border_color(if is_failed { DANGER } else { ATTENTION })
                        .rounded(px(4.))
                        .child(
                            muted(detail_text)
                                .font_family(MONO_FONT)
                                .text_size(px(11.5))
                                .select_text(),
                        ),
                )
                .child(actions)
        }
    }
}

fn render_github_stack_item(ui: Ui, item: GitHubStackItem, now_ms: u64) -> View {
    let rel = format_relative_zh(item.timestamp_ms, now_ms);
    let key_defer = item.key.clone();
    let wm = item.watermark;
    let item_for_dismiss = item.clone();
    let item_for_expand = item.clone();
    let item_key_expand = item.key.clone();
    let item_key_body = item.key.clone();
    let url_open = item.url.clone();
    let repo_diff = item.repository.clone();
    let num_diff = item.number;
    let is_pr = item.kind == "pr";
    let url_diff = item.url.clone();
    let title_defer = item.title.clone();
    let card_id_open = item.card_id.clone();
    let card_num_open = item.card_number;

    let gh_ref = GitHubReference {
        repository: item.repository.clone(),
        kind: item.kind.clone(),
        number: item.number,
        sha: None,
        url: item.url.clone(),
    };
    let fallback_title = item.title.clone();

    let reply_target = item.number.map(|num| ReplyTarget::GitHub {
        key: item.key.clone(),
        repository: item.repository.clone(),
        kind: item.kind.clone(),
        number: num,
        url: item.url.clone(),
        label: format!("{} #{num}", item.repository),
    });

    let mut top = view().flex_row().items_center().gap_2().min_w(px(0.));
    if let Some(author) = &item.author {
        top = top.child(
            view()
                .px_1p5()
                .py_0p5()
                .rounded_full()
                .bg(INSET)
                .border_1()
                .border_color(BORDER)
                .child(
                    label(format!("@{author}"))
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ACCENT),
                ),
        );
    }
    top = top
        .child(status(
            item.subtitle.clone(),
            if item.is_ci_failure {
                Tone::Danger
            } else if is_pr {
                Tone::Info
            } else {
                Tone::Neutral
            },
        ))
        .child(
            label(item.title.clone())
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(12.5))
                .flex_1()
                .min_w(px(0.))
                .truncate(),
        );
    if !rel.is_empty() {
        top = top.child(muted(rel).text_size(px(11.)).flex_shrink_0());
    }

    let static_body = item.body.clone();
    let body_block = dynamic(move || {
        let expanded = ui
            .inbox
            .expanded_github_items
            .with(|s| s.contains(&item_key_body));
        let cached_body = ui
            .inbox
            .github_item_bodies
            .with(|m| m.get(&item_key_body).cloned());
        let loading = ui
            .inbox
            .github_item_loading
            .with(|s| s.contains(&item_key_body));
        let text_opt = static_body
            .clone()
            .filter(|b| !b.trim().is_empty())
            .or_else(|| cached_body.filter(|b| !b.trim().is_empty()));
        if loading {
            return muted("正在加载 GitHub 正文…")
                .text_size(px(11.5))
                .into_element();
        }
        let Some(full) = text_opt else {
            return view().into_element();
        };
        let display = if !expanded && full.chars().count() > 220 {
            format!("{}…", full.chars().take(220).collect::<String>())
        } else {
            full
        };
        view()
            .px_2p5()
            .py_1p5()
            .bg(INSET)
            .border_l_1()
            .border_color(BORDER)
            .rounded(px(4.))
            .child(muted(display).text_size(px(12.)).select_text())
            .into_element()
    });

    let mut actions = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_end()
        .gap_1p5()
        .pt_1();

    if let Some(rt) = reply_target.clone() {
        actions = actions.child(compact_action("回复", Variant::Secondary, move |_| {
            start_inline_reply(ui, rt.clone())
        }));
    }
    if item.number.is_some() {
        actions = actions.child(compact_action("正文 ▾", Variant::Quiet, move |_| {
            load_github_item_body(ui, &item_for_expand);
            ui.inbox.expanded_github_items.update(|s| {
                if !s.remove(&item_key_expand) {
                    s.insert(item_key_expand.clone());
                }
            });
        }));
    }
    if is_pr && let Some(num) = num_diff {
        actions = actions.child(compact_action("Diff", Variant::Quiet, move |_| {
            crate::review::open_standalone_pr(ui, repo_diff.clone(), num, url_diff.clone());
        }));
    }
    if let Some(cid) = card_id_open {
        let lbl = card_num_open
            .map(|n| format!("Card #{n} ↗"))
            .unwrap_or_else(|| "打开 Card ↗".into());
        actions = actions.child(compact_action(lbl, Variant::Quiet, move |_| {
            ui.open_card(cid.clone());
        }));
    } else if item.kind != "commit" {
        actions = actions.child(compact_action("转为 Card", Variant::Quiet, move |_| {
            crate::github::add_to_board(ui, gh_ref.clone(), fallback_title.clone());
        }));
    }
    if !url_open.is_empty() {
        actions = actions.child(compact_action("GitHub ↗", Variant::Quiet, move |_| {
            crate::github::open_url(ui, url_open.clone());
        }));
    }
    actions = actions
        .child(compact_action("推迟 ▾", Variant::Quiet, move |_| {
            open_defer_menu(ui, vec![(key_defer.clone(), wm)], title_defer.clone())
        }))
        .child(compact_action("已读 ✓", Variant::Quiet, move |_| {
            dismiss_github_item(ui, item_for_dismiss.clone())
        }));

    let mut row = view()
        .px_3()
        .py_2p5()
        .gap_1p5()
        .border_t_1()
        .border_color(BORDER)
        .child(top);

    if !item.failed_checks.is_empty() {
        let mut checks_row = view().flex_row().flex_wrap().items_center().gap_1();
        for check in &item.failed_checks {
            checks_row = checks_row.child(status(format!("✗ {check}"), Tone::Danger));
        }
        row = row.child(checks_row);
    }

    row = row.child(body_block).child(actions);
    if let Some(rt) = reply_target {
        row = row.child(inline_reply_box(ui, rt));
    }
    row
}

fn render_github_stack(ui: Ui, stack: GitHubProjectStack, now_ms: u64) -> View {
    let stack_id = stack.stack_id.clone();
    let stack_id_toggle = stack_id.clone();
    let stack_id_check = stack_id.clone();
    let expanded = ui
        .inbox
        .expanded_stacks
        .with_untracked(|s| s.contains(&stack_id_check));
    let count = stack.items.len();
    let rel = format_relative_zh(stack.latest_at, now_ms);
    let defer_keys: Vec<(String, Option<u64>)> = stack
        .items
        .iter()
        .map(|i| (i.key.clone(), i.watermark))
        .collect();
    let stack_name_defer = format!("GitHub · {}", stack.display_name);
    let stack_for_clear = stack.clone();

    let mut header_left = view().flex_1().min_w(px(0.)).gap_1().child(
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .child(
                icon_tinted(
                    if expanded {
                        Icon::ChevronDown
                    } else {
                        Icon::Forward
                    },
                    MUTED,
                )
                .w(px(14.))
                .h(px(14.)),
            )
            .child(icon_tinted(Icon::GitHub, TEXT).w(px(15.)).h(px(15.)))
            .child(
                label(format!("GitHub · {}", stack.display_name))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(13.))
                    .truncate(),
            )
            .child(status(
                count.to_string(),
                if stack.has_ci_failure {
                    Tone::Danger
                } else {
                    Tone::Info
                },
            ))
            .children(stack.authors.iter().take(3).map(|a| badge(format!("@{a}")))),
    );
    if !expanded && !stack.preview.is_empty() {
        header_left = header_left.child(
            muted(stack.preview.clone())
                .pl(px(22.))
                .text_size(px(12.))
                .truncate(),
        );
    }

    let header = view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py_2p5()
        .child(
            press_target()
                .flex_1()
                .min_w(px(0.))
                .bg(CLEAR)
                .rounded(px(RADIUS))
                .on_press(move |_: &()| toggle_stack(ui, stack_id_toggle.clone()))
                .child(header_left),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .flex_shrink_0()
                .child(if !rel.is_empty() {
                    muted(rel).text_size(px(11.5)).into_element()
                } else {
                    view().into_element()
                })
                .child(compact_action(
                    "推迟本组 ▾",
                    Variant::Quiet,
                    move |_| open_defer_menu(ui, defer_keys.clone(), stack_name_defer.clone()),
                ))
                .child(compact_action(
                    "全部已读 ✓",
                    Variant::Quiet,
                    move |_| dismiss_github_stack(ui, stack_for_clear.clone()),
                )),
        );

    let mut box_view = view()
        .bg(SURFACE)
        .border_1()
        .border_color(if stack.has_ci_failure { DANGER } else { BORDER })
        .rounded(px(8.))
        .overflow_hidden()
        .child(header);

    if expanded {
        for item in stack.items {
            box_view = box_view.child(render_github_stack_item(ui, item, now_ms));
        }
    }

    let mut wrapper = view().gap_0().child(box_view);
    if !expanded && count > 1 {
        wrapper = wrapper.child(stacked_deck_lip());
    }
    wrapper
}

fn render_schedule_stack(ui: Ui, stack: ScheduleCardStack, now_ms: u64) -> View {
    let stack_id = stack.stack_id.clone();
    let stack_id_toggle = stack_id.clone();
    let expanded = ui
        .inbox
        .expanded_stacks
        .with_untracked(|s| s.contains(&stack_id));
    let count = stack.cards.len();
    let rel = format_relative_zh(stack.latest_at, now_ms);
    let preview = stack
        .cards
        .iter()
        .take(3)
        .map(|c| format!("#{} {}", c.card_number, c.card_title))
        .collect::<Vec<_>>()
        .join(" · ");
    let defer_keys: Vec<(String, Option<u64>)> = stack
        .cards
        .iter()
        .map(|c| (c.key.clone(), c.watermark))
        .collect();
    let dismiss_keys = defer_keys.clone();
    let stack_name = format!("Schedule · {}", stack.schedule_name);
    let stack_name_dismiss = stack_name.clone();

    let mut header_left = view().flex_1().min_w(px(0.)).gap_1().child(
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                icon_tinted(
                    if expanded {
                        Icon::ChevronDown
                    } else {
                        Icon::Forward
                    },
                    MUTED,
                )
                .w(px(14.))
                .h(px(14.)),
            )
            .child(icon_tinted(Icon::Clock, ACCENT).w(px(15.)).h(px(15.)))
            .child(
                label(stack_name.clone())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(13.))
                    .truncate(),
            )
            .child(status(format!("+{count} 张新卡"), Tone::Info))
            .child(badge(stack.project_name.clone())),
    );
    if !expanded && !preview.is_empty() {
        header_left = header_left.child(muted(preview).pl(px(22.)).text_size(px(12.)).truncate());
    }

    let header = view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py_2p5()
        .child(
            press_target()
                .flex_1()
                .min_w(px(0.))
                .bg(CLEAR)
                .rounded(px(RADIUS))
                .on_press(move |_: &()| toggle_stack(ui, stack_id_toggle.clone()))
                .child(header_left),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .flex_shrink_0()
                .child(if !rel.is_empty() {
                    muted(rel).text_size(px(11.5)).into_element()
                } else {
                    view().into_element()
                })
                .child(compact_action(
                    "推迟本组 ▾",
                    Variant::Quiet,
                    move |_| open_defer_menu(ui, defer_keys.clone(), stack_name.clone()),
                ))
                .child(compact_action(
                    "全部忽略 ✓",
                    Variant::Quiet,
                    move |_| {
                        dismiss_items(
                            ui,
                            dismiss_keys.clone(),
                            format!("已忽略 {stack_name_dismiss}"),
                        )
                    },
                )),
        );

    let mut box_view = view()
        .bg(SURFACE)
        .border_1()
        .border_color(BORDER)
        .rounded(px(8.))
        .overflow_hidden()
        .child(header);

    if expanded {
        for c in stack.cards {
            let cid_open = c.card_id.clone();
            let cid_archive = c.card_id.clone();
            let key_dismiss = c.key.clone();
            let wm = c.watermark;
            let title_dismiss = format!("#{} {}", c.card_number, c.card_title);
            let excerpt = {
                let t = c.card_body.trim();
                if t.chars().count() > 260 {
                    format!("{}…", t.chars().take(260).collect::<String>())
                } else {
                    t.to_owned()
                }
            };
            let mut item_view = view()
                .px_3()
                .py_2p5()
                .gap_1p5()
                .border_t_1()
                .border_color(BORDER)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            muted(format!("#{}", c.card_number))
                                .font_family(MONO_FONT)
                                .text_size(px(12.)),
                        )
                        .child(
                            label(c.card_title.clone())
                                .font_weight(FontWeight::MEDIUM)
                                .text_size(px(12.5))
                                .flex_1()
                                .truncate(),
                        )
                        .child(badge(c.stage.clone())),
                );
            if !excerpt.is_empty() {
                item_view = item_view.child(
                    view()
                        .px_2p5()
                        .py_1p5()
                        .bg(INSET)
                        .rounded(px(4.))
                        .child(muted(excerpt).text_size(px(12.)).select_text()),
                );
            }
            item_view = item_view.child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap_1p5()
                    .child(compact_action(
                        "开始做 / 打开 ↗",
                        Variant::Secondary,
                        move |_| {
                            ui.open_card(cid_open.clone());
                        },
                    ))
                    .child(compact_action("归档", Variant::Quiet, move |_| {
                        if let Some(card) = ui.snapshot.with_untracked(|s| {
                            s.cards.iter().find(|x| x.id == cid_archive).cloned()
                        }) {
                            dispatch(
                                ui,
                                "card.archive",
                                json!({ "card_id": card.id, "revision": card.revision }),
                            );
                        }
                    }))
                    .child(compact_action("忽略 ✓", Variant::Quiet, move |_| {
                        dismiss_items(
                            ui,
                            vec![(key_dismiss.clone(), wm)],
                            format!("已忽略 {title_dismiss}"),
                        )
                    })),
            );
            box_view = box_view.child(item_view);
        }
    }

    let mut wrapper = view().gap_0().child(box_view);
    if !expanded && count > 1 {
        wrapper = wrapper.child(stacked_deck_lip());
    }
    wrapper
}

fn render_open_card(ui: Ui, card: InboxCardWork, now_ms: u64) -> View {
    let rel = format_relative_zh(card.last_activity_at, now_ms);
    let cid_open = card.card_id.clone();
    let cid_resolve = card.card_id.clone();
    let key_defer = card.key.clone();
    let key_dismiss = card.key.clone();
    let wm = card.watermark;
    let label_title = format!("#{} {}", card.card_number, card.card_title);
    let label_dismiss = label_title.clone();

    let mut top = view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .child(status_dot(if card.has_live_agent {
            SUCCESS
        } else {
            ACCENT
        }))
        .child(badge(card.project_name.clone()))
        .child(
            muted(format!("#{}", card.card_number))
                .font_family(MONO_FONT)
                .text_size(px(12.)),
        )
        .child(
            label(card.card_title.clone())
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(13.))
                .flex_1()
                .min_w(px(0.))
                .truncate(),
        )
        .child(badge(card.stage.clone()));

    if card.pinned_today {
        top = top.child(status("拉入今天", Tone::Info));
    }
    if let Some(pr) = card.pr_summary {
        let pr_tone = match card.pr_ci_state.as_deref() {
            Some("failure") => Tone::Danger,
            Some("success") => Tone::Success,
            _ => Tone::Neutral,
        };
        top = top.child(status(pr, pr_tone));
    }
    if let Some(h) = card.host_label {
        top = top.child(badge(h));
    }
    if !rel.is_empty() {
        top = top.child(muted(rel).text_size(px(11.5)).flex_shrink_0());
    }

    let actions = view()
        .flex_row()
        .items_center()
        .justify_end()
        .gap_1p5()
        .pt_1()
        .child(compact_action(
            "打开 Card ↗",
            Variant::Secondary,
            move |_| {
                ui.open_card(cid_open.clone());
            },
        ))
        .child(compact_action("Resolve ✓", Variant::Quiet, move |_| {
            if let Some(c) = ui
                .snapshot
                .with_untracked(|s| s.cards.iter().find(|x| x.id == cid_resolve).cloned())
            {
                dispatch(
                    ui,
                    "card.resolve",
                    json!({ "card_id": c.id, "revision": c.revision }),
                );
                ui.notify(format!("已 Resolve #{}", c.number));
            }
        }))
        .child(compact_action("推迟 ▾", Variant::Quiet, move |_| {
            open_defer_menu(ui, vec![(key_defer.clone(), wm)], label_title.clone())
        }))
        .child(compact_action("移出今日", Variant::Quiet, move |_| {
            dismiss_items(
                ui,
                vec![(key_dismiss.clone(), wm)],
                format!("已移出今日 · {label_dismiss}"),
            )
        }));

    view()
        .p_3()
        .gap_1()
        .bg(SURFACE)
        .border_1()
        .border_color(BORDER)
        .rounded(px(8.))
        .child(top)
        .child(actions)
}

fn render_agent_terminal_card(ui: Ui, term: InboxTerminalWork, now_ms: u64) -> View {
    let (status_label, status_tone, is_live) = terminal_status_visual(term.status);
    let rel = format_relative_zh(term.timestamp_ms, now_ms);
    let tid_preview = term.terminal_id.clone();
    let tid_expand = term.terminal_id.clone();
    let tid_refresh = term.terminal_id.clone();
    let tid_open = term.terminal_id.clone();
    let tid_close = term.terminal_id.clone();
    let card_open = term.card_id.clone();
    let title_close = term.title.clone();
    let key_defer = term.key.clone();
    let title_defer = term.title.clone();
    let wait_detail = term.status_detail.clone();
    let fallback_cwd = term.cwd.clone();

    let reply_target = ReplyTarget::Terminal {
        terminal_id: term.terminal_id.clone(),
        label: term
            .agent_name
            .clone()
            .unwrap_or_else(|| term.title.clone()),
    };
    let reply_target_btn = reply_target.clone();

    let display_title = match (&term.agent_name, term.card_number, &term.card_title) {
        (Some(agent), Some(num), Some(ctitle)) => format!("{agent} · #{num} {ctitle}"),
        (Some(agent), _, _) if term.title != *agent => format!("{agent} · {}", term.title),
        _ => term.title.clone(),
    };

    let mut top = view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .child(icon_colored(engine_icon(term.agent_engine.as_deref()), 16.))
        .child(
            label(display_title)
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(13.))
                .flex_1()
                .min_w(px(0.))
                .truncate(),
        )
        .child(if is_live {
            live_status(status_label, status_tone).into_element()
        } else {
            status(status_label, status_tone).into_element()
        });
    if !rel.is_empty() {
        top = top.child(muted(rel).text_size(px(11.5)).flex_shrink_0());
    }

    // Project + Host + exact cwd + branch row.
    let proj_display = match &term.project_name {
        Some(p) => format!("{p} ({})", term.host_label),
        None => format!("无归属项目 ({})", term.host_label),
    };
    let tid_cwd = term.terminal_id.clone();
    let mut loc_row = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_1p5()
        .min_w(px(0.))
        .child(icon_tinted(Icon::Folder, MUTED).w(px(12.5)).h(px(12.5)))
        .child(badge(proj_display))
        .child(
            muted(move || {
                let resolved = ui
                    .inbox
                    .terminal_previews
                    .with(|m| m.get(&tid_cwd).and_then(|p| p.cwd.clone()))
                    .or_else(|| fallback_cwd.clone());
                match resolved {
                    Some(path) if !path.trim().is_empty() => shorten_path(&path),
                    _ => "(目录未知)".into(),
                }
            })
            .font_family(MONO_FONT)
            .text_size(px(11.5))
            .truncate(),
        );
    if let Some(br) = &term.branch
        && !br.is_empty()
    {
        loc_row = loc_row.child(badge(format!("⎇ {br}")));
    }

    // Inline last status / output preview box.
    let output_box = dynamic(move || {
        let expanded = ui
            .inbox
            .expanded_terminals
            .with(|s| s.contains(&tid_preview));
        let preview = ui
            .inbox
            .terminal_previews
            .with(|m| m.get(&tid_preview).cloned());
        let mut lines_to_show: Vec<String> = Vec::new();
        if let Some(ref detail) = wait_detail
            && !detail.trim().is_empty()
        {
            lines_to_show.push(detail.clone());
        }
        if let Some(ref p) = preview {
            if !p.lines.is_empty() {
                let take = if expanded { 16 } else { 2 };
                let start = p.lines.len().saturating_sub(take);
                for l in &p.lines[start..] {
                    if !lines_to_show.contains(l) {
                        lines_to_show.push(l.clone());
                    }
                }
            } else if p.loading && lines_to_show.is_empty() {
                return muted("正在读取终端最新输出…")
                    .text_size(px(11.5))
                    .into_element();
            }
        }
        if lines_to_show.is_empty() {
            return view().into_element();
        }
        view()
            .px_2p5()
            .py_1p5()
            .bg(INSET)
            .border_l_1()
            .border_color(if term.status == HerdrStatus::Blocked {
                ATTENTION
            } else if term.status == HerdrStatus::Done {
                SUCCESS
            } else {
                ACCENT
            })
            .rounded(px(4.))
            .child(
                label(lines_to_show.join("\n"))
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .text_color(TEXT)
                    .select_text(),
            )
            .into_element()
    });

    let actions = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_end()
        .gap_1p5()
        .pt_1()
        .child(compact_action(
            "直接回复",
            Variant::Secondary,
            move |_| start_inline_reply(ui, reply_target_btn.clone()),
        ))
        .child(compact_action("输出 ▾", Variant::Quiet, move |_| {
            fetch_terminal_preview(ui, tid_refresh.clone(), 20);
            ui.inbox.expanded_terminals.update(|s| {
                if !s.remove(&tid_expand) {
                    s.insert(tid_expand.clone());
                }
            });
        }))
        .child(compact_action("看终端 ↗", Variant::Quiet, move |_| {
            if let Some(cid) = &card_open {
                ui.open_card_with_terminal(cid.clone(), Some(tid_open.clone()));
            } else {
                ui.open_terminal(tid_open.clone());
            }
        }))
        .child(compact_action("关闭终端", Variant::Quiet, move |_| {
            close_terminal_from_inbox(ui, tid_close.clone(), title_close.clone())
        }))
        .child(compact_action("推迟 ▾", Variant::Quiet, move |_| {
            open_defer_menu(ui, vec![(key_defer.clone(), None)], title_defer.clone())
        }));

    view()
        .p_3()
        .gap_1p5()
        .bg(SURFACE)
        .border_1()
        .border_color(if term.status == HerdrStatus::Blocked {
            ATTENTION
        } else {
            BORDER
        })
        .rounded(px(8.))
        .child(top)
        .child(loc_row)
        .child(output_box)
        .child(actions)
        .child(inline_reply_box(ui, reply_target))
}

fn render_plain_shells_stack(ui: Ui, shells: Vec<InboxTerminalWork>) -> Option<View> {
    if shells.is_empty() {
        return None;
    }
    let stack_id = "plain-shells".to_owned();
    let expanded = ui
        .inbox
        .expanded_stacks
        .with_untracked(|s| s.contains(&stack_id));
    let count = shells.len();
    let preview = shells
        .iter()
        .take(3)
        .map(|s| {
            let dir = s
                .cwd
                .as_deref()
                .map(shorten_path)
                .unwrap_or_else(|| "~".into());
            match &s.project_name {
                Some(p) => format!("{p} ({dir})"),
                None => format!("无归属项目 ({dir})"),
            }
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let shells_close = shells.clone();
    let defer_keys: Vec<(String, Option<u64>)> =
        shells.iter().map(|s| (s.key.clone(), None)).collect();

    let mut left = view().flex_1().min_w(px(0.)).gap_1().child(
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                icon_tinted(
                    if expanded {
                        Icon::ChevronDown
                    } else {
                        Icon::Forward
                    },
                    MUTED,
                )
                .w(px(14.))
                .h(px(14.)),
            )
            .child(icon_tinted(Icon::Terminal, MUTED).w(px(14.5)).h(px(14.5)))
            .child(
                label(format!("{count} 个普通 Shell 未关闭"))
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(px(13.)),
            ),
    );
    if !expanded {
        left = left.child(
            muted(preview)
                .pl(px(22.))
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .truncate(),
        );
    }

    let header = view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py_2p5()
        .child(
            press_target()
                .flex_1()
                .min_w(px(0.))
                .bg(CLEAR)
                .on_press(move |_: &()| toggle_stack(ui, stack_id.clone()))
                .child(left),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .flex_shrink_0()
                .child(compact_action(
                    "推迟本组 ▾",
                    Variant::Quiet,
                    move |_| {
                        open_defer_menu(ui, defer_keys.clone(), format!("{count} 个普通 Shell"))
                    },
                ))
                .child(compact_action("全部关闭", Variant::Quiet, move |_| {
                    close_all_plain_shells(ui, shells_close.clone())
                })),
        );

    let mut box_view = view()
        .bg(SURFACE)
        .border_1()
        .border_color(BORDER)
        .rounded(px(8.))
        .overflow_hidden()
        .child(header);

    if expanded {
        for sh in shells {
            let tid_open = sh.terminal_id.clone();
            let tid_close = sh.terminal_id.clone();
            let title_close = sh.title.clone();
            let proj = match &sh.project_name {
                Some(p) => format!("{p} ({})", sh.host_label),
                None => format!("无归属项目 ({})", sh.host_label),
            };
            let cwd_str = sh
                .cwd
                .as_deref()
                .map(shorten_path)
                .unwrap_or_else(|| "(目录未知)".into());
            let mut row = view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(BORDER)
                .child(label(sh.title.clone()).text_size(px(12.5)))
                .child(badge(proj))
                .child(
                    muted(cwd_str)
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .flex_1()
                        .truncate(),
                );
            if let Some(cmd) = sh.shell_command {
                let cmd_text = match sh.shell_exit_code {
                    Some(0) => format!("✓ {cmd}"),
                    Some(code) => format!("✗ {cmd} ({code})"),
                    None => format!("● {cmd}"),
                };
                row = row.child(status(
                    cmd_text,
                    match sh.shell_exit_code {
                        Some(0) => Tone::Success,
                        Some(_) => Tone::Danger,
                        None => Tone::Info,
                    },
                ));
            }
            row = row
                .child(compact_action("看终端 ↗", Variant::Quiet, move |_| {
                    ui.open_terminal(tid_open.clone());
                }))
                .child(compact_action("关闭", Variant::Quiet, move |_| {
                    close_terminal_from_inbox(ui, tid_close.clone(), title_close.clone())
                }));
            box_view = box_view.child(row);
        }
    }

    let mut wrapper = view().gap_0().child(box_view);
    if !expanded && count > 1 {
        wrapper = wrapper.child(stacked_deck_lip());
    }
    Some(wrapper)
}

fn render_carryover_stack(ui: Ui, cards: Vec<InboxCardWork>, day_start_ms: u64) -> Option<View> {
    if cards.is_empty() {
        return None;
    }
    let stack_id = "carryover".to_owned();
    let expanded = ui
        .inbox
        .expanded_stacks
        .with_untracked(|s| s.contains(&stack_id));
    let count = cards.len();

    let header = press_target()
        .w_full()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py_2p5()
        .bg(CLEAR)
        .on_press(move |_: &()| toggle_stack(ui, stack_id.clone()))
        .child(
            icon_tinted(
                if expanded {
                    Icon::ChevronDown
                } else {
                    Icon::Forward
                },
                MUTED,
            )
            .w(px(14.))
            .h(px(14.)),
        )
        .child(icon_tinted(Icon::History, MUTED).w(px(14.)).h(px(14.)))
        .child(
            label(format!("往日未完 ({count} 张 Card)"))
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(12.5))
                .text_color(MUTED),
        )
        .child(view().flex_1())
        .child(muted("点击挑选拉入今天").text_size(px(11.5)));

    let mut box_view = view()
        .bg(SURFACE)
        .border_1()
        .border_color(BORDER)
        .rounded(px(8.))
        .overflow_hidden()
        .child(header);

    if expanded {
        for c in cards {
            let key_pin = c.key.clone();
            let wm = c.watermark;
            let cid_open = c.card_id.clone();
            box_view = box_view.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(BORDER)
                    .child(badge(c.project_name.clone()))
                    .child(
                        muted(format!("#{}", c.card_number))
                            .font_family(MONO_FONT)
                            .text_size(px(12.)),
                    )
                    .child(
                        label(c.card_title.clone())
                            .text_size(px(12.5))
                            .flex_1()
                            .truncate(),
                    )
                    .child(badge(c.stage.clone()))
                    .child(compact_action(
                        "拉入今天",
                        Variant::Secondary,
                        move |_| pin_to_today(ui, key_pin.clone(), day_start_ms, wm),
                    ))
                    .child(compact_action("打开 ↗", Variant::Quiet, move |_| {
                        ui.open_card(cid_open.clone());
                    })),
            );
        }
    }

    Some(box_view)
}

fn render_today_zero(ui: Ui, inbox: &DailyInbox) -> View {
    let done = inbox.completed_today;
    let deferred = inbox.deferred_items.len();
    let mut actions = view()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_2()
        .pt_2();
    if deferred > 0 {
        actions = actions.child(action(
            format!("查看已推迟 ({deferred})"),
            Variant::Secondary,
            move |_| ui.inbox.drawer.set("deferred".into()),
        ));
    }
    actions = actions.child(action("前往 Board", Variant::Quiet, move |_| {
        ui.open_board()
    }));

    view()
        .py_8()
        .px_6()
        .items_center()
        .gap_2()
        .bg(SURFACE)
        .border_1()
        .border_color(BORDER)
        .rounded(px(10.))
        .child(tone_icon_badge(Icon::Check, Tone::Success, false))
        .child(
            label("今日已清零")
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            muted(format!(
                "今天已完成 {done} 项 · 已推迟 {deferred} 项 · 下次日切 {}",
                format_cutoff(inbox.cutoff_minutes)
            ))
            .text_size(px(12.5)),
        )
        .child(actions)
}

fn render_deferred_drawer(ui: Ui, items: Vec<DeferredInboxItem>) -> View {
    if items.is_empty() {
        return view()
            .py_8()
            .items_center()
            .gap_2()
            .bg(SURFACE)
            .border_1()
            .border_color(BORDER)
            .rounded(px(8.))
            .child(muted("当前没有已推迟的事项。"))
            .child(compact_action(
                "返回今天",
                Variant::Secondary,
                move |_| ui.inbox.drawer.set("today".into()),
            ));
    }
    let mut col = view().gap_2();
    for item in items {
        let key_restore = item.key.clone();
        let card_open = item.card_id.clone();
        let term_open = item.terminal_id.clone();
        let url_open = item.url.clone();
        let mut row = view()
            .flex_row()
            .items_center()
            .gap_2()
            .p_3()
            .bg(SURFACE)
            .border_1()
            .border_color(BORDER)
            .rounded(px(8.))
            .child(badge(item.kind_label))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.))
                    .gap_0p5()
                    .child(
                        label(item.title)
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(px(13.))
                            .truncate(),
                    )
                    .child(muted(item.context).text_size(px(11.5)).truncate()),
            )
            .child(status(
                format!("唤醒 {}", format_defer_until(item.defer_until)),
                Tone::Neutral,
            ))
            .child(compact_action(
                "恢复到今天",
                Variant::Secondary,
                move |_| restore_item(ui, key_restore.clone()),
            ));
        if let Some(cid) = card_open {
            row = row.child(compact_action("打开 ↗", Variant::Quiet, move |_| {
                ui.open_card(cid.clone())
            }));
        } else if let Some(tid) = term_open {
            row = row.child(compact_action("看终端 ↗", Variant::Quiet, move |_| {
                ui.open_terminal(tid.clone())
            }));
        } else if let Some(url) = url_open {
            row = row.child(compact_action("GitHub ↗", Variant::Quiet, move |_| {
                crate::github::open_url(ui, url.clone())
            }));
        }
        col = col.child(row);
    }
    col
}

fn render_done_drawer(ui: Ui, items: Vec<DoneInboxItem>, now_ms: u64) -> View {
    if items.is_empty() {
        return view()
            .py_8()
            .items_center()
            .gap_2()
            .bg(SURFACE)
            .border_1()
            .border_color(BORDER)
            .rounded(px(8.))
            .child(muted("今天还没有已完成的记录。"))
            .child(compact_action(
                "返回今天",
                Variant::Secondary,
                move |_| ui.inbox.drawer.set("today".into()),
            ));
    }
    let mut col = view().gap_2();
    for item in items {
        let key_restore = item.key.clone();
        let card_open = item.card_id.clone();
        let rel = format_relative_zh(item.completed_at, now_ms);
        let mut row = view()
            .flex_row()
            .items_center()
            .gap_2()
            .p_3()
            .bg(SURFACE)
            .border_1()
            .border_color(BORDER)
            .rounded(px(8.))
            .child(icon_tinted(Icon::Check, SUCCESS).w(px(14.)).h(px(14.)))
            .child(badge(item.kind_label))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.))
                    .gap_0p5()
                    .child(label(item.title).text_size(px(13.)).truncate())
                    .child(muted(item.context).text_size(px(11.5)).truncate()),
            )
            .child(muted(rel).text_size(px(11.5)));
        if item.can_restore {
            row = row.child(compact_action("恢复", Variant::Quiet, move |_| {
                restore_item(ui, key_restore.clone())
            }));
        }
        if let Some(cid) = card_open {
            row = row.child(compact_action("打开 ↗", Variant::Quiet, move |_| {
                ui.open_card(cid.clone())
            }));
        }
        col = col.child(row);
    }
    col
}

pub fn screen(ui: Ui) -> impl IntoElement {
    view()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(
            scroll().flex_1().min_h(px(0.)).p_5().child(
                view()
                    .w_full()
                    .items_center()
                    .child(
                        view()
                            .w_full()
                            .max_w(px(880.))
                            .pb_8()
                            .gap_4()
                            .child(render_undo_banner(ui))
                            .child(dynamic(move || {
                        let inbox = ui.daily_inbox.get();
                        let drawer = ui.inbox.drawer.get();
                        let _ = ui.inbox.expanded_stacks.get();
                        let gh_error = ui.inbox.github_error.get();
                        let gh_connected = ui.github.gh_status.with(|v| {
                            v["state"].as_str() == Some("connected")
                        }) || ui.github_account.with(|a| a.state == "connected");
                        let now_ms = ui.utility_clock.get_untracked().saturating_mul(1000).max(canban_types::now());

                        if drawer == "deferred" {
                            return render_deferred_drawer(ui, inbox.deferred_items.clone())
                                .into_element();
                        }
                        if drawer == "done" {
                            return render_done_drawer(ui, inbox.done_today_items.clone(), now_ms)
                                .into_element();
                        }

                        let mut col = view().gap_4();

                        // 1. Today's Schedule Pulse Strip.
                        if let Some(pulse) = render_schedule_pulse(ui, &inbox.schedule_pulse) {
                            col = col.child(pulse);
                        }

                        // Subtle GitHub connect hint if disconnected and no GitHub items.
                        if !gh_connected && inbox.github_stacks.is_empty() && !gh_error.is_empty() {
                            col = col.child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .px_3()
                                    .py_2()
                                    .bg(SURFACE)
                                    .border_1()
                                    .border_color(BORDER)
                                    .rounded(px(8.))
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .child(icon_tinted(Icon::GitHub, MUTED).w(px(14.)).h(px(14.)))
                                            .child(
                                                muted("连接 GitHub CLI (gh) 后，GitHub 通知将按项目自动叠放在此页")
                                                    .text_size(px(12.)),
                                            ),
                                    )
                                    .child(compact_action(
                                        "GitHub 账号设置 →",
                                        Variant::Secondary,
                                        move |_| ui.navigate("github-account"),
                                    )),
                            );
                        }

                        // Inbox Zero state!
                        if inbox.remaining_today == 0 {
                            col = col.child(render_today_zero(ui, &inbox));
                            if let Some(carry) = render_carryover_stack(
                                ui,
                                inbox.carryover_cards.clone(),
                                inbox.day_start_ms,
                            ) {
                                col = col.child(carry);
                            }
                            return col.into_element();
                        }

                        // 2. Needs You (Direct, unstacked).
                        if !inbox.needs_you.is_empty() {
                            let group_keys: Vec<(String, Option<u64>)> = inbox
                                .needs_you
                                .iter()
                                .map(|i| (i.key.clone(), i.watermark))
                                .collect();
                            let count = inbox.needs_you.len();
                            let mut sec = view().gap_2().child(section_header(
                                "需要你回应",
                                count,
                                Tone::Attention,
                                Some(
                                    compact_action("推迟本组 ▾", Variant::Quiet, move |_| {
                                        open_defer_menu(
                                            ui,
                                            group_keys.clone(),
                                            format!("需要你回应 ({count})"),
                                        )
                                    })
                                    .into_element(),
                                ),
                            ));
                            for item in inbox.needs_you.iter().cloned() {
                                sec = sec.child(render_needs_you_item(ui, item, now_ms));
                            }
                            col = col.child(sec);
                        }

                        // 3. Auto-Stacked Bundles (GitHub by Project + Schedule New Cards).
                        let bundle_count =
                            inbox.github_stacks.len() + inbox.schedule_stacks.len();
                        if bundle_count > 0 {
                            let mut sec = view().gap_2p5().child(section_header(
                                "自动归类",
                                bundle_count,
                                Tone::Info,
                                None,
                            ));
                            for stack in inbox.github_stacks.iter().cloned() {
                                sec = sec.child(render_github_stack(ui, stack, now_ms));
                            }
                            for stack in inbox.schedule_stacks.iter().cloned() {
                                sec = sec.child(render_schedule_stack(ui, stack, now_ms));
                            }
                            col = col.child(sec);
                        }

                        // 4. Today's Open Work (Touched Cards + Unclosed Agent Terminals + Plain Shells Stack).
                        let open_work_count = inbox.open_cards.len()
                            + inbox.agent_terminals.len()
                            + usize::from(!inbox.plain_shells.is_empty());
                        if open_work_count > 0 {
                            let mut group_keys: Vec<(String, Option<u64>)> = inbox
                                .open_cards
                                .iter()
                                .map(|c| (c.key.clone(), c.watermark))
                                .collect();
                            for t in &inbox.agent_terminals {
                                group_keys.push((t.key.clone(), None));
                            }
                            for s in &inbox.plain_shells {
                                group_keys.push((s.key.clone(), None));
                            }
                            let mut sec = view().gap_2().child(section_header(
                                "今天动过，还没收尾",
                                open_work_count,
                                Tone::Neutral,
                                Some(
                                    compact_action("推迟本组 ▾", Variant::Quiet, move |_| {
                                        open_defer_menu(
                                            ui,
                                            group_keys.clone(),
                                            format!("今天未收尾 ({open_work_count})"),
                                        )
                                    })
                                    .into_element(),
                                ),
                            ));
                            for card in inbox.open_cards.iter().cloned() {
                                sec = sec.child(render_open_card(ui, card, now_ms));
                            }
                            for term in inbox.agent_terminals.iter().cloned() {
                                sec = sec.child(render_agent_terminal_card(ui, term, now_ms));
                            }
                            if let Some(shells_stack) =
                                render_plain_shells_stack(ui, inbox.plain_shells.clone())
                            {
                                sec = sec.child(shells_stack);
                            }
                            col = col.child(sec);
                        }

                        // 5. Carry-over Stack (Collapsed at bottom of Page 1).
                        if let Some(carry) = render_carryover_stack(
                            ui,
                            inbox.carryover_cards.clone(),
                            inbox.day_start_ms,
                        ) {
                            col = col.child(carry);
                        }

                        col.into_element()
                    })),
                    ),
            ),
        )
}
