//! Mobile `Diff` segment inside pushed Card screen: file selector pills, unified hunk diff
//! with `Wrap / Scroll` toggle, offline notice, and sticky `Return for rework` / `Deliver…` bar.
use super::{
    MobileSheet, MobileState,
    chrome::{
        MOBILE_CODE, is_mobile_os, mobile_button, mobile_footnote, mobile_subhead, trigger_haptic,
    },
};
use crate::{
    presenter::*,
    review::model::{DiffLine, parse_unified_hunks},
};
use canban_ui_kit::{tokens::*, *};
use nsh_core::{Haptic, prelude::*};
use serde_json::{Value, json};

#[derive(Clone, PartialEq)]
struct DiffFileItem {
    filename: String,
    status: String,
    additions: u64,
    deletions: u64,
    patch: String,
}

#[derive(Clone, PartialEq)]
struct DiffModel {
    offline: bool,
    empty_workspace: bool,
    loading: bool,
    error: String,
    total_add: u64,
    total_del: u64,
    truncated: bool,
    files: Vec<DiffFileItem>,
    card_revision: u64,
    can_deliver: bool,
    can_rework: bool,
    rework_run: Option<(String, u32)>,
}

fn parse_files(data: &Value) -> Vec<DiffFileItem> {
    data["files"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    let filename = f["filename"].as_str()?.to_owned();
                    Some(DiffFileItem {
                        filename,
                        status: f["status"].as_str().unwrap_or("modified").to_owned(),
                        additions: f["additions"].as_u64().unwrap_or(0),
                        deletions: f["deletions"].as_u64().unwrap_or(0),
                        patch: f["patch"].as_str().unwrap_or("").to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn render_diff_line(line: DiffLine, wrap: bool) -> AnyElement {
    let (prefix, color_fg, bg_row) = match line.kind {
        '+' => ("+", SUCCESS, SUCCESS_SURFACE),
        '-' => ("-", DANGER, DANGER_SURFACE),
        '\\' => ("\\", MUTED, CLEAR),
        _ => (" ", TEXT, CLEAR),
    };
    let ln = line
        .new_line
        .or(line.old_line)
        .map(|n| format!("{n:>4}"))
        .unwrap_or_else(|| "    ".to_owned());

    let code = text(format!("{prefix}{}", line.text))
        .text_size(px(MOBILE_CODE))
        .line_height(px(19.))
        .font_family(MONO_FONT)
        .text_color(color_fg);

    view()
        .w_full()
        .flex_row()
        .items_start()
        .gap_2()
        .px_2p5()
        .py_0p5()
        .bg(bg_row)
        .child(
            text(ln)
                .text_size(px(11.5))
                .line_height(px(19.))
                .font_family(MONO_FONT)
                .text_color(MUTED)
                .tabular_nums()
                .flex_shrink_0(),
        )
        .child(if wrap {
            code.flex_1().min_w(px(0.)).into_element()
        } else {
            code.flex_shrink_0().into_element()
        })
        .into_element()
}

pub(crate) fn segment_view(ui: Ui, state: MobileState, card_id: String) -> impl IntoElement {
    let loaded_data = signal(Option::<Value>::None);
    let loading = signal(false);
    let error = signal(String::new());
    let selected_file = signal(String::new());
    let wrap_lines = signal(false);

    let query = computed({
        let cid = card_id.clone();
        move || {
            ui.snapshot.with(|s| {
                let card = s.cards.iter().find(|c| c.id == cid)?.clone();
                let project = s.projects.iter().find(|p| p.id == card.project_id);
                let host = s.card_host(&card.id).to_owned();
                let offline = s.is_host_offline(&host);
                let has_workspace = card.worktree.is_some()
                    || card.branch.is_some()
                    || card.is_github_pr()
                    || card.pull_request.is_some();
                let target = crate::review::card_target_branch(&card, project);
                let params = crate::review::card_compare_params(&card, &target);
                let rework_run = s
                    .workflow_runs
                    .iter()
                    .find(|r| {
                        r.active()
                            && r.card_id.as_deref() == Some(cid.as_str())
                            && matches!(r.state.as_str(), "waiting_approval" | "stale")
                    })
                    .map(|r| (r.id.clone(), r.visit));
                let can_deliver =
                    card.active() && card.worktree.is_some() && card.delivery.is_none();
                Some((
                    card.project_id,
                    card.revision,
                    offline,
                    has_workspace,
                    params,
                    rework_run,
                    can_deliver,
                ))
            })
        }
    });

    effect({
        let cid = card_id.clone();
        move || {
            let Some((pid, _, offline, has_workspace, params, _, _)) = query.get() else {
                return;
            };
            if !crate::backend::is_connected() {
                loaded_data.set(Some(ui.review_state.compare_data.get()));
                return;
            }
            if !has_workspace || offline || loading.get_untracked() {
                return;
            }
            loading.set(true);
            error.set(String::new());
            let expected_cid = cid.clone();
            ui.spawn(async move {
                let res = crate::review::cached_read(ui, "git.compare", params, pid).await;
                let _ = expected_cid;
                loading.try_set(false);
                match res {
                    Ok(cached) => {
                        loaded_data.try_set(Some(cached.data.clone()));
                    }
                    Err(e) => {
                        error.try_set(e);
                    }
                }
            });
        }
    });

    let model = computed(move || {
        let Some((_, rev, offline, has_workspace, _, rework_run, can_deliver)) = query.get() else {
            return DiffModel {
                offline: false,
                empty_workspace: true,
                loading: false,
                error: String::new(),
                total_add: 0,
                total_del: 0,
                truncated: false,
                files: Vec::new(),
                card_revision: 0,
                can_deliver: false,
                can_rework: false,
                rework_run: None,
            };
        };
        let data = loaded_data.get();
        let files = data.as_ref().map(parse_files).unwrap_or_default();
        DiffModel {
            offline,
            empty_workspace: !has_workspace,
            loading: loading.get(),
            error: error.get(),
            total_add: data
                .as_ref()
                .and_then(|v| v["total_additions"].as_u64())
                .unwrap_or(0),
            total_del: data
                .as_ref()
                .and_then(|v| v["total_deletions"].as_u64())
                .unwrap_or(0),
            truncated: data
                .as_ref()
                .and_then(|v| v["truncated"].as_bool())
                .unwrap_or(false),
            files,
            card_revision: rev,
            can_deliver,
            can_rework: rework_run.is_some(),
            rework_run,
        }
    });

    let deliver_cid = card_id.clone();
    let rework_cid = card_id.clone();

    view()
        .flex_1()
        .flex_col()
        .min_h(px(0.))
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .child(
                    view()
                        .w_full()
                        .p_4()
                        .gap_3()
                        .pb(px(32.))
                        .child(dynamic(move || {
                            let m = model.get();
                            if m.empty_workspace {
                                return super::chrome::empty_state(
                                    Icon::Compare,
                                    Tone::Neutral,
                                    "No workspace changes",
                                    "",
                                    None,
                                )
                                .into_element();
                            }
                            if m.offline && m.files.is_empty() {
                                return notice("Diff unavailable while offline", Tone::Attention)
                                    .into_element();
                            }
                            if m.loading && m.files.is_empty() {
                                return loading_panel("Loading diff…", "").into_element();
                            }
                            if !m.error.is_empty() && m.files.is_empty() {
                                return notice(m.error, Tone::Danger).into_element();
                            }
                            if m.files.is_empty() {
                                return super::chrome::empty_state(
                                    Icon::Check,
                                    Tone::Success,
                                    "Working tree clean",
                                    "",
                                    None,
                                )
                                .into_element();
                            }

                            let chosen = selected_file.get();
                            let active_file = m
                                .files
                                .iter()
                                .find(|f| f.filename == chosen)
                                .or_else(|| m.files.first())
                                .cloned();
                            let wrap = wrap_lines.get();

                            let wrap_btn = press_target()
                                .h(px(30.))
                                .px_2p5()
                                .rounded_full()
                                .bg(if wrap { SELECTED } else { SURFACE })
                                .items_center()
                                .justify_center()
                                .active(|s| s.opacity(0.7))
                                .child(
                                    text(if wrap { "Wrap: On" } else { "Wrap: Off" })
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(if wrap { ACCENT } else { MUTED }),
                                )
                                .on_press(move |_: &()| {
                                    trigger_haptic(Haptic::Selection);
                                    wrap_lines.update(|w| *w = !*w);
                                });

                            let mut out = view().w_full().gap_3().child(
                                view()
                                    .w_full()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_2p5()
                                            .child(
                                                text(format!(
                                                    "{} file{}",
                                                    m.files.len(),
                                                    if m.files.len() == 1 { "" } else { "s" }
                                                ))
                                                .text_size(px(15.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(TEXT)
                                                .tabular_nums(),
                                            )
                                            .child(
                                                text(format!("+{}", m.total_add))
                                                    .text_size(px(14.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(SUCCESS)
                                                    .tabular_nums(),
                                            )
                                            .child(
                                                text(format!("−{}", m.total_del))
                                                    .text_size(px(14.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(DANGER)
                                                    .tabular_nums(),
                                            )
                                            .children(
                                                m.truncated.then(|| mobile_footnote("Partial")),
                                            ),
                                    )
                                    .child(wrap_btn),
                            );

                            out = out.child(scroll().horizontal().scrollbar(false).w_full().child(
                                view().flex_row().items_center().gap_2().children(
                                    m.files.iter().map(|f| {
                                        let fname = f.filename.clone();
                                        let fname_press = fname.clone();
                                        let short =
                                            fname.rsplit('/').next().unwrap_or(&fname).to_owned();
                                        let is_sel = active_file
                                            .as_ref()
                                            .is_some_and(|a| a.filename == fname);
                                        press_target()
                                            .h(px(34.))
                                            .px_3()
                                            .rounded(px(9.))
                                            .bg(if is_sel { ACCENT_FILL } else { SURFACE })
                                            .flex_row()
                                            .items_center()
                                            .gap_1p5()
                                            .cursor_pointer()
                                            .active(|s| s.opacity(0.75))
                                            .child(
                                                text(short)
                                                    .text_size(px(13.5))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(if is_sel {
                                                        ON_ACCENT
                                                    } else {
                                                        TEXT
                                                    }),
                                            )
                                            .child(
                                                text(format!("+{} −{}", f.additions, f.deletions))
                                                    .text_size(px(12.))
                                                    .text_color(if is_sel {
                                                        ON_ACCENT
                                                    } else {
                                                        MUTED
                                                    })
                                                    .opacity(if is_sel { 0.85 } else { 1.0 })
                                                    .tabular_nums(),
                                            )
                                            .on_press(move |_: &()| {
                                                trigger_haptic(Haptic::Selection);
                                                selected_file.set(fname_press.clone());
                                            })
                                    }),
                                ),
                            ));

                            if let Some(file) = active_file {
                                let hunks = parse_unified_hunks(&file.patch);
                                let mut diff_card = super::chrome::surface_block().child(
                                    view()
                                        .w_full()
                                        .px_3p5()
                                        .py_2p5()
                                        .border_b_1()
                                        .border_color(BORDER)
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
                                                .gap_1()
                                                .child(
                                                    text(file.filename.clone())
                                                        .font_family(MONO_FONT)
                                                        .text_size(px(12.5))
                                                        .text_color(TEXT)
                                                        .truncate()
                                                        .select_text()
                                                        .min_w(px(0.))
                                                        .flex_shrink(1.),
                                                )
                                                .children((!file.filename.is_empty()).then(|| {
                                                    let copy_path = file.filename.clone();
                                                    copy_icon_button(
                                                        format!("Copy file path {}", file.filename),
                                                        "Copy file path",
                                                        28.,
                                                        14.,
                                                        CARD_HOVER,
                                                        Placement::Bottom,
                                                        move || copy_text(ui, copy_path.clone()),
                                                    )
                                                })),
                                        )
                                        .child(badge(file.status)),
                                );
                                if hunks.is_empty() {
                                    diff_card =
                                        diff_card.child(view().p_4().child(mobile_subhead(
                                            "Binary or summary-only diff (no patch lines).",
                                        )));
                                } else {
                                    let mut hunks_col = view().w_full();
                                    for hunk in hunks {
                                        hunks_col = hunks_col.child(
                                            view().w_full().px_2p5().py_1p5().bg(SELECTED).child(
                                                text(hunk.header)
                                                    .text_size(px(11.5))
                                                    .font_family(MONO_FONT)
                                                    .text_color(ACCENT),
                                            ),
                                        );
                                        for line in hunk.lines {
                                            hunks_col =
                                                hunks_col.child(render_diff_line(line, wrap));
                                        }
                                    }
                                    if wrap {
                                        diff_card = diff_card.child(hunks_col);
                                    } else {
                                        diff_card = diff_card
                                            .child(scroll().horizontal().w_full().child(hunks_col));
                                    }
                                }
                                out = out.child(diff_card);
                            }

                            out.into_element()
                        })),
                ),
        )
        .child(dynamic(move || {
            let m = model.get();
            if !m.can_deliver && !m.can_rework {
                return view().hidden();
            }
            let deliver_card_id = deliver_cid.clone();
            let rework_card_id = rework_cid.clone();
            let run_opt_rework = m.rework_run.clone();
            let run_opt_deliver = m.rework_run;
            let rev = m.card_revision;
            view()
                .w_full()
                .flex_shrink_0()
                .px_4()
                .pt_2p5()
                .pb(move || {
                    if is_mobile_os() {
                        px(safe_area().bottom.0.max(12.))
                    } else {
                        px(12.)
                    }
                })
                .border_t_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .flex_row()
                .items_center()
                .gap_2p5()
                .children(m.can_rework.then(|| {
                    mobile_button("Return for Rework", Variant::Secondary, move |_| {
                        if run_opt_rework.is_some() {
                            state.open_sheet(MobileSheet::Rework(rework_card_id.clone()));
                        }
                    })
                    .flex_1()
                }))
                .children(m.can_deliver.then(|| {
                    mobile_button("Deliver…", Variant::Primary, move |_| {
                        if !crate::backend::is_connected() {
                            ui.notify("Gallery preview · Deliver Card");
                            return;
                        }
                        if run_opt_deliver.is_some() {
                            dispatch(
                                ui,
                                "workflow.deliver",
                                json!({"card_id": deliver_card_id, "revision": rev}),
                            );
                        } else if let Some(card) = ui.snapshot.with_untracked(|s| {
                            s.cards.iter().find(|c| c.id == deliver_card_id).cloned()
                        }) {
                            crate::card::quick_merge_start(ui, card);
                        }
                    })
                    .flex_1()
                }))
        }))
}
