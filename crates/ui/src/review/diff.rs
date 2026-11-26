use super::context::{
    CONTEXT_STEP, ExpandKind, FileRenderContext, anchor_ref_for, expanded_context_line,
    render_expander_bar, render_static_hunk_bar,
};
use super::git_ops;
use super::model::{
    AnchorEdge, AutoFoldReason, DiffLine, LineTarget, PendingInlineComment, auto_fold_reason,
    parse_unified_hunks,
};
use crate::presenter::*;
use canban_ui_kit::tokens::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

fn multiline_comment_input(
    value: Signal<String>,
    label_text: &'static str,
    placeholder: &'static str,
    height: f32,
) -> View {
    let focused = signal(false);
    view()
        .h(px(height))
        .min_w(px(0.))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(move || if focused.get() { ACCENT } else { BORDER })
        .bg(BACKGROUND)
        .overflow_hidden()
        .child(
            nsh_base::editor()
                .value(move || Some(value.get().into()))
                .on_change(move |v: &SharedString| value.set(v.to_string()))
                .on_focus(move |_: &()| focused.set(true))
                .on_blur(move |_: &()| focused.set(false))
                .accessibility_label(label_text)
                .placeholder(placeholder)
                .line_numbers(false)
                .folding(false)
                .soft_wrap(true)
                .active_line(false)
                .indent_guides(false)
                .auto_close(false)
                .auto_indent(false)
                .font_size(px(12.5))
                .line_height(px(18.))
                .text_color(TEXT)
                .px_2()
                .py_1()
                .size_full(),
        )
}

fn working_tree_mode(ui: Ui) -> bool {
    ui.review_state.mode.get() == "git"
        && matches!(
            ui.review_state.compare_ref.get().as_str(),
            "" | "UNCOMMITTED" | "WORKDIR" | "STAGED" | "UNSTAGED"
        )
}

const FILES_AUTO_EXPAND_THRESHOLD: f32 = 320.;

fn maybe_auto_expand_files(ui: Ui, total_files: usize, e: &ScrollEvent) {
    let st = ui.review_state;
    if total_files == 0 || st.visible_file_limit.get_untracked() >= total_files {
        return;
    }
    let viewport_h = e.size.height.0;
    let content_h = e.content_size.height.0;
    if viewport_h <= 0. || e.offset.y.0 <= 0. || content_h <= viewport_h {
        return;
    }
    let remaining = content_h - e.offset.y.0 - viewport_h;
    if remaining <= FILES_AUTO_EXPAND_THRESHOLD {
        super::show_more_files(ui);
    }
}

/// Scroll hosts stay mounted across loading, Snapshot and equal-result updates.
/// Only the initial unresolved query uses the full-size loading placeholder.
pub fn workbench(ui: Ui) -> impl IntoElement {
    let st = ui.review_state;
    let viewport = super::model::DiffViewport {
        scroll: element_ref(),
        content: element_ref(),
        bottom_space: signal(0.),
    };
    st.viewport.replace(Some(viewport));
    let data: Memo<Arc<Value>> = computed(move || {
        let mut value = if !crate::backend::is_connected()
            && let Some(p) = ui.review.get()
        {
            super::preview_to_compare_json(&p)
        } else if !crate::backend::is_connected()
            && !ui.card.get().is_empty()
            && ui.current_card().is_some_and(|c| c.delivery.is_some())
        {
            json!({"files": []})
        } else {
            st.compare_data.get()
        };
        if value["status_files"].is_null() && working_tree_mode(ui) {
            let status = st.refs_data.with(|v| v["status_files"].clone());
            if !status.is_null() {
                value["status_files"] = status;
            }
        }
        Arc::new(value)
    });
    let last_diff_scroll = stored(Option::<ScrollEvent>::None);
    watch_scroll(
        move || viewport.scroll.node(),
        move |e: &ScrollEvent| {
            last_diff_scroll.replace(Some(*e));
            let total = data.with_untracked(|v| v["files"].as_array().map_or(0, Vec::len));
            maybe_auto_expand_files(ui, total, e);
        },
    );
    effect(move || {
        let total = data.with(|v| v["files"].as_array().map_or(0, Vec::len));
        let limit = st.visible_file_limit.get();
        if limit < total
            && let Some(m) = last_diff_scroll.get()
        {
            untracked(|| maybe_auto_expand_files(ui, total, &m));
        }
    });
    let login = computed(move || {
        ui.snapshot.with(|s| {
            ui.github
                .gh_status
                .with(|gh| crate::github::active_login_from(s, gh))
        })
    });
    let initial_loading = computed(move || {
        data.with(|v| v["files"].is_null()) && (st.refs_loading.get() || st.compare_loading.get())
    });
    view().flex_1().min_w(px(0.)).min_h(px(0.)).child(
        when(
            move || initial_loading.get(),
            move || {
                view()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .p_6()
                    .child(muted("Loading diff…"))
            },
        )
        .otherwise(move || {
            view()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .flex_row()
                .child(files_pane(ui, data))
                .child(
                    view()
                        .flex_1()
                        .min_h(px(0.))
                        .min_w(px(0.))
                        .child(
                            view()
                                .h(px(36.))
                                .px_3()
                                .justify_center()
                                .border_b_1()
                                .border_color(BORDER)
                                .child(
                                    label("Changes")
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD),
                                ),
                        )
                        .child(
                            pull_refresh_scroll(
                                scroll().min_w(px(0.)).child(
                                    view()
                                        .element_ref(&viewport.content)
                                        .child(dynamic(move || {
                                            code_content(ui, &data.get(), login)
                                        }))
                                        .child(view().h(move || px(viewport.bottom_space.get()))),
                                ),
                                viewport.scroll,
                                move || st.diff_refreshing.get(),
                                move || git_ops::pull_refresh(ui, super::PullRefreshTarget::Diff),
                            )
                            .label("Git diff code"),
                        ),
                )
        }),
    )
}

pub(super) const FILES_PANE_WIDTH: f32 = 268.;

pub(super) fn split_file_display(path: &str) -> (&str, Option<String>) {
    let Some((parent, name)) = path.rsplit_once('/') else {
        return (path, None);
    };
    if parent.is_empty() {
        return (name, None);
    }
    if parent.chars().count() <= 20 {
        return (name, Some(parent.to_owned()));
    }
    let mut parts = parent.rsplit('/');
    let (Some(last), Some(prev)) = (parts.next(), parts.next()) else {
        return (name, Some(parent.to_owned()));
    };
    if parts.next().is_some() {
        (name, Some(format!("…/{prev}/{last}")))
    } else {
        (name, Some(parent.to_owned()))
    }
}

pub(crate) fn file_path_tooltip(path: &str, previous: Option<&str>) -> String {
    if let Some(prev) = previous.filter(|p| !p.is_empty() && *p != path) {
        format!("{prev} → {path}")
    } else {
        path.to_owned()
    }
}

pub(crate) fn diff_header_path(
    path: String,
    previous: Option<&str>,
    font_weight: FontWeight,
    mono: bool,
    copy_ui: Option<Ui>,
) -> View {
    let full_tip = file_path_tooltip(&path, previous);
    let copy_path = path.clone();
    let mut name = label(path)
        .min_w(px(0.))
        .truncate()
        .font_weight(font_weight);
    if copy_ui.is_some() {
        name = name.select_text();
    }
    if mono {
        name = name.font_family(MONO_FONT).text_size(px(12.));
    }
    let mut row = view()
        .flex_1()
        .min_w(px(0.))
        .flex_row()
        .items_center()
        .gap_1()
        .child(
            view()
                .min_w(px(0.))
                .flex_shrink(1.)
                .tooltip(hint_tooltip(full_tip).placement(Placement::Bottom))
                .child(name),
        );
    if let Some(ui) = copy_ui
        && !copy_path.is_empty()
    {
        let path_for_copy = copy_path.clone();
        row = row.child(copy_icon_button(
            format!("Copy file path {copy_path}"),
            "Copy file path",
            22.,
            13.,
            CARD_HOVER,
            Placement::Bottom,
            move || copy_text(ui, path_for_copy.clone()),
        ));
    }
    row
}

pub(super) fn diff_stat_counts(stats: Option<(u64, u64)>) -> Option<View> {
    let (add, del) = stats?;
    if add == 0 && del == 0 {
        return None;
    }
    let mut row = view().flex_row().items_center().gap_1().flex_shrink_0();
    if add > 0 {
        row = row.child(
            label(format!("+{add}"))
                .font_family(MONO_FONT)
                .text_size(px(10.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(SUCCESS),
        );
    }
    if del > 0 {
        row = row.child(
            label(format!("−{del}"))
                .font_family(MONO_FONT)
                .text_size(px(10.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(DANGER),
        );
    }
    Some(row)
}

pub(super) fn file_status_badge(code: &str, conflicted: bool) -> View {
    let (bg_col, fg_col, glyph) = if conflicted {
        (ATTENTION_SURFACE, ATTENTION, "!")
    } else {
        match code {
            "A" | "added" => (SUCCESS_SURFACE, SUCCESS, "A"),
            "U" | "?" | "untracked" => (SUCCESS_SURFACE, SUCCESS, "U"),
            "D" | "removed" | "deleted" => (DANGER_SURFACE, DANGER, "D"),
            "R" | "renamed" => (HIGHLIGHT_SURFACE, HIGHLIGHT, "R"),
            "C" | "copied" => (HIGHLIGHT_SURFACE, HIGHLIGHT, "C"),
            _ => (INSET, MUTED, "M"),
        }
    };
    view()
        .w(px(16.))
        .h(px(16.))
        .flex_shrink_0()
        .rounded(px(4.))
        .bg(bg_col)
        .items_center()
        .justify_center()
        .child(
            text(glyph)
                .font_family(MONO_FONT)
                .text_size(px(10.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_col),
        )
}

pub(super) fn inline_fold_button(ui: Ui, key: String, title: String) -> impl IntoElement {
    let st = ui.review_state;
    let check_key = key.clone();
    let folded = computed(move || st.collapsed_files.with(|files| files.contains(&check_key)));
    dynamic(move || {
        let is_folded = folded.get();
        let a11y_label = format!(
            "{} diff for {title}",
            if is_folded { "Expand" } else { "Collapse" }
        );
        let tip = a11y_label.clone();
        let toggle_key = key.clone();
        view()
            .flex_shrink_0()
            .tooltip(hint_tooltip(tip).placement(Placement::Bottom))
            .child(
                canban_ui_kit::press_target()
                    .w(px(18.))
                    .h(px(18.))
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .bg(CLEAR)
                    .hover(|s| s.bg(INSET))
                    .focus(|s| s.border_1().border_color(ACCENT))
                    .accessibility_label(move || Some(a11y_label.clone().into()))
                    .on_press(move |_: &()| super::toggle_file(ui, toggle_key.clone()))
                    .child(
                        icon_tinted(
                            if is_folded {
                                Icon::Forward
                            } else {
                                Icon::ChevronDown
                            },
                            MUTED,
                        )
                        .w(px(11.))
                        .h(px(11.)),
                    ),
            )
    })
}

pub(super) struct FileRowSpec {
    pub path: String,
    pub previous_path: Option<String>,
    pub status_code: String,
    pub conflicted: bool,
    pub fold_key: String,
    pub fold_title: String,
    pub is_selected: Memo<bool>,
    pub pending_count: Memo<usize>,
    pub stats: Option<(u64, u64)>,
}

pub(super) fn render_file_row(
    ui: Ui,
    spec: FileRowSpec,
    on_select: impl Fn() + 'static,
    trailing: Option<AnyElement>,
) -> View {
    let (short_name, dir_hint) = split_file_display(&spec.path);
    let short_name = short_name.to_owned();
    let row_tooltip = file_path_tooltip(&spec.path, spec.previous_path.as_deref());
    let is_sel = spec.is_selected;
    let pending_cnt = spec.pending_count;

    let file_label = label(short_name)
        .text_size(px(12.))
        .style_with(move |s| {
            s.font_weight(if is_sel.get() {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
        })
        .truncate()
        .min_w(px(0.));

    let mut name_group = view()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_1()
        .min_w(px(0.))
        .overflow_hidden();
    if let Some(dir) = dir_hint {
        name_group = name_group.child(file_label.flex_shrink(1.)).child(
            muted(dir)
                .text_size(px(10.5))
                .truncate()
                .flex_1()
                .min_w(px(0.)),
        );
    } else {
        name_group = name_group.child(file_label.flex_1());
    }

    let right_meta = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .child(when(
            move || pending_cnt.get() > 0,
            move || {
                view().pr_1().child(
                    view()
                        .min_w(px(16.))
                        .h(px(16.))
                        .px_1()
                        .rounded(px(4.))
                        .bg(INSET)
                        .items_center()
                        .justify_center()
                        .child(
                            text(move || pending_cnt.get().to_string())
                                .text_size(px(10.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ACCENT),
                        ),
                )
            },
        ))
        .children(diff_stat_counts(spec.stats));

    let row_btn = canban_ui_kit::press_target()
        .w_full()
        .min_h(px(24.))
        .px_1()
        .rounded(px(4.))
        .flex_row()
        .items_center()
        .justify_between()
        .gap_1p5()
        .focus(|s| s.border_1().border_color(ACCENT))
        .on_press(move |_: &()| on_select())
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .flex_1()
                .min_w(px(0.))
                .child(file_status_badge(&spec.status_code, spec.conflicted))
                .child(name_group),
        )
        .child(right_meta);

    let mut row = view()
        .relative()
        .min_h(px(28.))
        .pl_1p5()
        .pr_1()
        .py_0p5()
        .rounded(px(6.))
        .flex_row()
        .items_center()
        .gap_1()
        .bg(move || if is_sel.get() { SELECTED } else { CLEAR })
        .hover(move |s| s.bg(move || if is_sel.get() { SELECTED } else { CARD_HOVER }))
        .child(when(
            move || is_sel.get(),
            move || {
                view()
                    .absolute()
                    .left(px(0.))
                    .top(px(5.))
                    .bottom(px(5.))
                    .w(px(2.5))
                    .rounded_full()
                    .bg(ACCENT)
                    .pointer_events(PointerEvents::None)
            },
        ))
        .child(inline_fold_button(ui, spec.fold_key, spec.fold_title))
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .tooltip(hint_tooltip(row_tooltip).placement(Placement::Right))
                .child(row_btn),
        );
    if let Some(trail) = trailing {
        row = row.child(trail);
    }
    row
}

pub(super) fn render_all_files_row(
    title: String,
    total_add: u64,
    total_del: u64,
    all_selected: Memo<bool>,
    on_press: impl Fn() + 'static,
) -> View {
    view()
        .relative()
        .child(
            canban_ui_kit::press_target()
                .w_full()
                .min_h(px(28.))
                .pl_2p5()
                .pr_2()
                .py_1()
                .rounded(px(6.))
                .flex_row()
                .items_center()
                .justify_between()
                .gap_1p5()
                .bg(move || if all_selected.get() { SELECTED } else { CLEAR })
                .hover(move |s| {
                    s.bg(move || {
                        if all_selected.get() {
                            SELECTED
                        } else {
                            CARD_HOVER
                        }
                    })
                })
                .focus(|s| s.border_1().border_color(ACCENT))
                .on_press(move |_: &()| on_press())
                .child(
                    label(title)
                        .text_size(px(12.))
                        .style_with(move |s| {
                            s.font_weight(if all_selected.get() {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                        })
                        .truncate()
                        .min_w(px(0.)),
                )
                .children(diff_stat_counts(Some((total_add, total_del)))),
        )
        .child(when(
            move || all_selected.get(),
            move || {
                view()
                    .absolute()
                    .left(px(0.))
                    .top(px(5.))
                    .bottom(px(5.))
                    .w(px(2.5))
                    .rounded_full()
                    .bg(ACCENT)
                    .pointer_events(PointerEvents::None)
            },
        ))
}

fn files_pane(ui: Ui, data: Memo<Arc<Value>>) -> impl IntoElement {
    let st = ui.review_state;
    let files_scroll = element_ref();
    view()
        .w(px(FILES_PANE_WIDTH))
        .flex_shrink_0()
        .min_h(px(0.))
        .border_r_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .child(
            view()
                .h(px(36.))
                .px_3()
                .justify_center()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    label("Files")
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD),
                ),
        )
        .child(pull_refresh_scroll(
            scroll().label("Git files list").child(dynamic(move || {
                let d = data.get();
                if working_tree_mode(ui) {
                    git_ops::render_working_tree_list(ui, &d)
                } else {
                    standard_file_list(ui, &d)
                }
            })),
            files_scroll,
            move || st.files_refreshing.get(),
            move || git_ops::pull_refresh(ui, super::PullRefreshTarget::Files),
        ))
        .child(when(
            move || working_tree_mode(ui),
            move || git_ops::render_commit_composer(ui, data),
        ))
}

fn standard_file_list(ui: Ui, data: &Value) -> AnyElement {
    let st = ui.review_state;
    let files = data["files"].as_array().cloned().unwrap_or_default();
    let total_add = data["total_additions"].as_u64().unwrap_or(0);
    let total_del = data["total_deletions"].as_u64().unwrap_or(0);
    let file_count = files.len();
    let mut file_list = view().gap_0p5().p_2();
    let all_selected = computed(move || st.selected_file.with(String::is_empty));
    file_list = file_list.child(render_all_files_row(
        format!("All files ({file_count})"),
        total_add,
        total_del,
        all_selected,
        move || {
            super::show_all_files(ui);
        },
    ));

    for f in &files {
        let fname = f["filename"].as_str().unwrap_or("").to_owned();
        let previous_path = f["previous_filename"].as_str().map(str::to_owned);
        let f_add = f["additions"].as_u64().unwrap_or(0);
        let f_del = f["deletions"].as_u64().unwrap_or(0);
        let f_status = f["status"].as_str().unwrap_or("modified").to_owned();
        let key = super::model::file_key(f);
        let sel_key = key.clone();
        let is_sel = computed(move || st.selected_file.with(|s| s == &sel_key));
        let click_name = key.clone();
        let pending_path = fname.clone();
        let pending_section = f["section"].as_str().unwrap_or("").to_owned();
        let pending_for_file = computed(move || {
            st.pending_comments.with(|list| {
                list.iter()
                    .filter(|c| {
                        c.path == pending_path
                            && (pending_section.is_empty()
                                || c.section.is_empty()
                                || c.section == pending_section)
                    })
                    .count()
            })
        });

        file_list = file_list.child(render_file_row(
            ui,
            FileRowSpec {
                path: fname.clone(),
                previous_path,
                status_code: f_status,
                conflicted: false,
                fold_key: key,
                fold_title: fname,
                is_selected: is_sel,
                pending_count: pending_for_file,
                stats: Some((f_add, f_del)),
            },
            move || super::focus_file(ui, click_name.clone()),
            None,
        ));
    }

    file_list.into_element()
}

#[derive(Clone)]
struct FileCardItem {
    ctx: Arc<FileRenderContext>,
    additions: u64,
    deletions: u64,
    is_workdir_mode: bool,
    is_file_staged: bool,
    is_file_unstaged: bool,
    fold_reason: Option<AutoFoldReason>,
    initially_auto_folded: bool,
}

impl PartialEq for FileCardItem {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.ctx, &other.ctx)
            && self.additions == other.additions
            && self.deletions == other.deletions
            && self.is_workdir_mode == other.is_workdir_mode
            && self.is_file_staged == other.is_file_staged
            && self.is_file_unstaged == other.is_file_unstaged
            && self.fold_reason == other.fold_reason
            && self.initially_auto_folded == other.initially_auto_folded
    }
}

fn code_content(ui: Ui, data: &Value, login: Memo<Option<String>>) -> AnyElement {
    let st = ui.review_state;
    let files = data["files"].as_array().cloned().unwrap_or_default();
    let commits = data["commits"].as_array().cloned().unwrap_or_default();
    let remote_threads_all = data["threads"].as_array().cloned().unwrap_or_default();
    let truncated = data["truncated"].as_bool().unwrap_or(false);
    let commit_detail = data["commit_detail"]
        .as_object()
        .map(|_| &data["commit_detail"]);
    let is_workdir_mode = st.mode.get() == "git"
        && matches!(
            st.compare_ref.get().as_str(),
            "" | "UNCOMMITTED" | "WORKDIR" | "STAGED" | "UNSTAGED"
        );

    if files.is_empty()
        && commit_detail.is_none()
        && !is_workdir_mode
        && st.pending_comments.with(Vec::is_empty)
        && st.active_line.with(Option::is_none)
    {
        return view()
            .flex_1()
            .items_center()
            .justify_center()
            .p_6()
            .gap_2()
            .child(label("No file changes").font_weight(FontWeight::SEMIBOLD))
            .child(muted(
                "The selected base and compare references have identical file contents.",
            ))
            .into_element();
    }

    let pr_target = st.pr_target.get();
    let pr_base_url = pr_target.as_ref().and_then(|pr| {
        pr["url"]
            .as_str()
            .filter(|u| !u.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                let repo = pr["repository"].as_str().unwrap_or("");
                let num = pr["number"].as_u64().unwrap_or(0);
                (!repo.is_empty() && num > 0)
                    .then(|| format!("https://github.com/{repo}/pull/{num}"))
            })
    });
    let mut threads_by_path: HashMap<String, Vec<Value>> = HashMap::new();
    for t in remote_threads_all {
        if let Some(p) = t["path"].as_str() {
            threads_by_path.entry(p.to_owned()).or_default().push(t);
        }
    }
    let empty_threads: Arc<Vec<Value>> = Arc::new(Vec::new());
    let mode = st.mode.get();
    let compare_ref = st.compare_ref.get();
    let live_sha = data["compare_sha"].as_str().unwrap_or("").to_owned();
    let compare_cwd = data["cwd"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let c = st.checkout_cwd.get_untracked();
            (!c.is_empty()).then_some(c)
        });

    let status_by_path: HashMap<String, (bool, bool)> = if is_workdir_mode {
        data["status_files"]
            .as_array()
            .cloned()
            .or_else(|| st.refs_data.with(|v| v["status_files"].as_array().cloned()))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| {
                let p = s["path"].as_str()?.to_owned();
                let staged = s["staged"].as_bool().unwrap_or(false);
                let unstaged = s["unstaged"].as_bool().unwrap_or(false)
                    || s["untracked"].as_bool().unwrap_or(false);
                Some((p, (staged, unstaged)))
            })
            .collect()
    } else {
        HashMap::new()
    };

    // Right main diff viewer
    let mut diff_column = view().gap_3().p_3();
    st.file_targets.replace(Default::default());

    if let Some(detail) = commit_detail {
        let detail = detail.clone();
        diff_column = diff_column.child(dynamic(move || {
            git_ops::render_commit_detail_card(ui, &detail)
        }));
    }

    if truncated {
        diff_column = diff_column.child(notice(
            "Diff output is capped for UI responsiveness. Inspect very large changes in Terminals if needed.",
            Tone::Attention,
        ));
    }

    let drift_sha = live_sha.clone();
    diff_column = diff_column.child(dynamic(move || {
        let has_pending = st.pending_comments.with(|list| !list.is_empty());
        if let Some(anchor_sha) = st.draft_anchor_sha.get().filter(|s| !s.is_empty())
            && !drift_sha.is_empty()
            && anchor_sha != drift_sha
            && !st.draft_drift_dismissed.get()
            && has_pending
        {
            let short_anchor = &anchor_sha[..anchor_sha.len().min(7)];
            let short_live = &drift_sha[..drift_sha.len().min(7)];
            let cid_for_rerun = ui.card.get();
            view()
                .p_2p5()
                .rounded(px(RADIUS))
                .border_1()
                .border_color(ATTENTION)
                .bg(ATTENTION_SURFACE)
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
                        .child(icon_tinted(Icon::TriangleAlert, ATTENTION).w(px(14.)).h(px(14.)))
                        .child(
                            label(format!(
                                "Drafts anchored to earlier commit {short_anchor} (head is now {short_live})"
                            ))
                            .text_size(px(12.))
                            .text_color(ATTENTION),
                        ),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .children((!cid_for_rerun.is_empty()).then(move || {
                            let cid = cid_for_rerun.clone();
                            compact_action("Re-run Agent Review", Variant::Secondary, move |_| {
                                super::start_agent_review(ui, &cid, None, None);
                            })
                        }))
                        .child(compact_action("Dismiss", Variant::Quiet, move |_| {
                            st.draft_drift_dismissed.set(true);
                        })),
                )
                .into_element()
        } else {
            view().into_element()
        }
    }));

    // Collapsible commit summary bar when comparing branches/ranges (isolated from file list!)
    if !commits.is_empty() && commit_detail.is_none() {
        let commits = Arc::new(commits);
        diff_column = diff_column.child(dynamic(move || {
            let commit_len = commits.len();
            let is_open = st.commits_open.get();
            let mut commit_rows = view().gap_1().pt_1();
            if is_open {
                for c in commits.iter().take(30) {
                    let sha = c["sha"].as_str().unwrap_or("").to_owned();
                    let short = c["short_sha"].as_str().unwrap_or("").to_owned();
                    let subj = c["subject"].as_str().unwrap_or("").to_owned();
                    let author = c["author"].as_str().unwrap_or("").to_owned();
                    let rel = c["relative_time"].as_str().unwrap_or("").to_owned();
                    let click_sha = sha.clone();
                    commit_rows = commit_rows.child(
                        canban_ui_kit::press_target()
                            .w_full()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded(px(4.))
                            .bg(INSET)
                            .hover(|s| s.bg(CARD_HOVER))
                            .on_press(move |_: &()| {
                                if !click_sha.is_empty() {
                                    git_ops::select_single_commit(ui, click_sha.clone(), 0, true);
                                }
                            })
                            .child(
                                label(short)
                                    .font_family(MONO_FONT)
                                    .text_size(px(11.))
                                    .text_color(ACCENT),
                            )
                            .child(
                                label(subj)
                                    .text_size(px(12.))
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate(),
                            )
                            .child(muted(format!("{author} · {rel}")).text_size(px(11.)))
                            .child(muted("Inspect →").text_size(px(10.5))),
                    );
                }
            }
            collapsible_section_group(
                "Commits in range",
                Some(commit_len),
                is_open,
                move |_| st.commits_open.update(|v| *v = !*v),
                commit_rows,
            )
        }));
    }

    if files.is_empty() {
        diff_column = diff_column.child(
            view()
                .p_6()
                .rounded(px(RADIUS))
                .border_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .items_center()
                .justify_center()
                .gap_1p5()
                .child(
                    label(if is_workdir_mode {
                        "Working tree is clean"
                    } else if commit_detail.is_some() {
                        "No file changes in this commit"
                    } else {
                        "No file changes"
                    })
                    .font_weight(FontWeight::SEMIBOLD),
                )
                .child(muted(if is_workdir_mode {
                    "No modified or untracked files in this checkout. You can still pull, push, or amend the last commit."
                } else if commit_detail.is_some() {
                    "This commit did not introduce net file content changes relative to the selected parent."
                } else {
                    "The selected base and compare references have identical file contents."
                })),
        );
    }

    let total_files = files.len();
    let prepared_hunks = st.prepared_hunks.get();

    // Seed GitHub-style default-fold protection for generated/lockfiles and extreme patches
    // even when fixture data was set directly without `apply_compare`.
    let mut deferred_auto_fold = BTreeSet::new();
    st.auto_collapsed_seen.with(|seen| {
        for file in &files {
            let key = super::model::file_key(file);
            if seen.insert(key.clone()) {
                let hunks = prepared_hunks.get(&key).map(|h| h.as_slice());
                if auto_fold_reason(file, hunks).is_some() {
                    deferred_auto_fold.insert(key);
                }
            }
        }
    });
    let deferred_applied = signal(deferred_auto_fold.is_empty());
    if !deferred_auto_fold.is_empty() {
        let to_fold: Vec<String> = deferred_auto_fold.iter().cloned().collect();
        ui.spawn(async move {
            batch(|| {
                st.collapsed_files.update(|set| {
                    set.extend(to_fold);
                });
                deferred_applied.try_set(true);
            });
        });
    }

    let mut file_index_by_key: HashMap<String, usize> = HashMap::with_capacity(files.len());
    let mut all_file_cards: Vec<FileCardItem> = Vec::with_capacity(files.len());
    for (idx, file) in files.into_iter().enumerate() {
        let path = file["filename"].as_str().unwrap_or("").to_owned();
        let section = file["section"].as_str().unwrap_or("").to_owned();
        let key = super::model::file_key(&file);
        file_index_by_key.insert(key.clone(), idx);
        let f_status = file["status"].as_str().unwrap_or("modified").to_owned();
        let additions = file["additions"].as_u64().unwrap_or(0);
        let deletions = file["deletions"].as_u64().unwrap_or(0);
        let patch = file["patch"].as_str().unwrap_or("").to_owned();
        let (is_file_staged, is_file_unstaged) =
            status_by_path.get(&path).copied().unwrap_or((false, false));
        let file_hunks = prepared_hunks
            .get(&key)
            .cloned()
            .unwrap_or_else(|| Arc::new(parse_unified_hunks(&patch)));
        let fold_reason = auto_fold_reason(&file, Some(file_hunks.as_slice()));
        let initially_auto_folded = deferred_auto_fold.contains(&key);
        let remote_threads = threads_by_path
            .remove(&path)
            .map(Arc::new)
            .unwrap_or_else(|| empty_threads.clone());
        let ctx = Arc::new(FileRenderContext {
            file,
            key,
            path,
            section,
            status: f_status,
            patch,
            hunks: file_hunks,
            mode: mode.clone(),
            compare_ref: compare_ref.clone(),
            compare_sha: live_sha.clone(),
            cwd: compare_cwd.clone(),
            pr_target: pr_target.clone(),
            pr_base_url: pr_base_url.clone(),
            remote_threads,
            login,
        });
        all_file_cards.push(FileCardItem {
            ctx,
            additions,
            deletions,
            is_workdir_mode,
            is_file_staged,
            is_file_unstaged,
            fold_reason,
            initially_auto_folded,
        });
    }
    let all_file_cards = Arc::new(all_file_cards);
    let file_index_by_key = Arc::new(file_index_by_key);
    let deferred_auto_fold = Arc::new(deferred_auto_fold);

    if !all_file_cards.is_empty() {
        let cards_for_each = all_file_cards.clone();
        diff_column = diff_column.child(view().gap_3().child(each(
            move || {
                let limit = st.visible_file_limit.get();
                cards_for_each
                    .iter()
                    .take(limit)
                    .cloned()
                    .collect::<Vec<_>>()
            },
            |item| item.ctx.key.clone(),
            move |item_sig, _| render_file_card(ui, item_sig.get_untracked(), deferred_applied),
        )));
    }

    // Expanded file bodies render every comment in their section, including comments
    // outside the currently visible lines. Keep the remaining comments in the diff.
    let index_for_fallback = file_index_by_key.clone();
    let deferred_for_fallback = deferred_auto_fold.clone();
    diff_column = diff_column.child(dynamic(move || {
        let pending = st.pending_comments.get();
        let active = st.active_line.get();
        if pending.is_empty() && active.is_none() {
            return view();
        }
        let limit = st.visible_file_limit.get();
        let collapsed = st.collapsed_files.get();
        let applied = deferred_applied.get();
        let is_expanded = |key: &str| {
            index_for_fallback.get(key).is_some_and(|&idx| idx < limit)
                && !collapsed.contains(key)
                && (applied || !deferred_for_fallback.contains(key))
        };
        let unrendered_comments: Vec<_> = pending
            .iter()
            .filter(|item| !is_expanded(&super::model::diff_key(&item.section, &item.path)))
            .collect();
        let unrendered_composer = active
            .filter(|target| !is_expanded(&super::model::diff_key(&target.section, &target.path)));
        if unrendered_comments.is_empty() && unrendered_composer.is_none() {
            return view();
        }
        let count = unrendered_comments.len();
        let mut fallback_section = view()
            .rounded(px(RADIUS))
            .border_1()
            .border_color(ACCENT)
            .bg(SURFACE)
            .overflow_hidden()
            .child(
                view()
                    .px_3()
                    .py_2()
                    .bg(INSET)
                    .border_b_1()
                    .border_color(BORDER)
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        label(if count > 0 {
                            format!("Other pending comments ({count})")
                        } else {
                            "Pending inline comment".into()
                        })
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        muted("From collapsed/hidden files or another comparison")
                            .text_size(px(11.)),
                    ),
            );
        for item in unrendered_comments {
            let key = super::model::diff_key(&item.section, &item.path);
            let jump_key = index_for_fallback.contains_key(&key).then_some(key);
            fallback_section =
                fallback_section.child(render_pending_comment_card_with_jump(ui, item, jump_key));
        }
        if let Some(target) = unrendered_composer {
            fallback_section = fallback_section.child(inline_composer_box(ui, target));
        }
        fallback_section
    }));

    diff_column = diff_column.child(dynamic(move || {
        let limit = st.visible_file_limit.get();
        if total_files > limit {
            compact_action(
                format!("Show more files ({} remaining)", total_files - limit),
                Variant::Secondary,
                move |_| super::show_more_files(ui),
            )
            .into_element()
        } else {
            view().into_element()
        }
    }));

    diff_column.into_element()
}

fn render_file_card(
    ui: Ui,
    item: FileCardItem,
    deferred_applied: Signal<bool>,
) -> impl IntoElement {
    let st = ui.review_state;
    let ctx = item.ctx;
    let key = ctx.key.clone();
    let path = ctx.path.clone();
    let section = ctx.section.clone();
    let f_status = ctx.status.clone();
    let target = element_ref();
    st.file_targets.with(|targets| {
        targets.insert(key.clone(), target);
    });

    let pending_path = path.clone();
    let pending_section = section.clone();
    let file_pending_count = computed(move || {
        st.pending_comments.with(|comments| {
            comments
                .iter()
                .filter(|c| c.path == pending_path && c.section == pending_section)
                .count()
        })
    });

    let file_btn_path = path.clone();
    let mut header = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .px_3()
        .py_1p5()
        .bg(INSET)
        .border_b_1()
        .border_color(BORDER)
        .child(fold_button(
            ui,
            key.clone(),
            if section.is_empty() {
                path.clone()
            } else {
                format!("{section} {path}")
            },
        ))
        .child(if section.is_empty() {
            view().into_element()
        } else {
            badge(if section == "staged" {
                "Staged"
            } else {
                "Unstaged"
            })
            .into_element()
        })
        .child(diff_header_path(
            path.clone(),
            ctx.file["previous_filename"].as_str(),
            FontWeight::SEMIBOLD,
            true,
            Some(ui),
        ))
        .child(badge(f_status))
        .child(dynamic(move || {
            let count = file_pending_count.get();
            if count == 0 {
                view().into_element()
            } else {
                status(format!("{count} pending"), Tone::Info).into_element()
            }
        }))
        .child(
            label(format!("+{}", item.additions))
                .text_size(px(11.5))
                .font_family(MONO_FONT)
                .text_color(SUCCESS),
        )
        .child(
            label(format!("−{}", item.deletions))
                .text_size(px(11.5))
                .font_family(MONO_FONT)
                .text_color(DANGER),
        );

    if item.is_workdir_mode {
        if section != "staged" && (item.is_file_unstaged || !item.is_file_staged) {
            let p_stage = path.clone();
            header = header.child(compact_action("+ Stage", Variant::Quiet, move |_| {
                git_ops::run_stage(ui, vec![p_stage.clone()], false);
            }));
        }
        if section != "unstaged" && item.is_file_staged {
            let p_unstage = path.clone();
            header = header.child(compact_action("− Unstage", Variant::Quiet, move |_| {
                git_ops::run_unstage(ui, vec![p_unstage.clone()], false);
            }));
        }
        let p_discard = path.clone();
        header = header.child(compact_action("Discard…", Variant::Quiet, move |_| {
            git_ops::request_discard(ui, vec![p_discard.clone()]);
        }));
    }

    let file_section = section.clone();
    header = header.child(compact_action(
        "+ File comment",
        Variant::Quiet,
        move |_| {
            st.active_line.set(Some(LineTarget {
                path: file_btn_path.clone(),
                section: file_section.clone(),
                line: 1,
                side: "RIGHT".into(),
                snippet: String::new(),
                editing_author: None,
            }));
            st.line_draft.set(String::new());
        },
    ));

    let collapse_key = key.clone();
    let initially_folded = item.initially_auto_folded;
    let is_collapsed = computed(move || {
        st.collapsed_files.with(|set| set.contains(&collapse_key))
            || (initially_folded && !deferred_applied.get())
    });

    // Per-file scoped memos so interacting with one file never re-renders any other file.
    let active_path = path.clone();
    let active_section = section.clone();
    let file_active_line = computed(move || {
        st.active_line
            .get()
            .filter(|t| t.path == active_path && t.section == active_section)
    });
    let pend_path = path.clone();
    let pend_section = section.clone();
    let file_pending = computed(move || {
        st.pending_comments.with(|all| {
            all.iter()
                .filter(|c| c.path == pend_path && c.section == pend_section)
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let thread_ids: Arc<BTreeSet<u64>> = Arc::new(
        ctx.remote_threads
            .iter()
            .filter_map(|t| t["id"].as_u64())
            .filter(|&id| id > 0)
            .collect(),
    );
    let edit_ids = thread_ids.clone();
    let file_editing_remote = computed(move || {
        st.editing_remote_comment
            .get()
            .filter(|id| edit_ids.contains(id))
    });
    let del_ids = thread_ids;
    let file_deleting_remote = computed(move || {
        st.deleting_remote_comment
            .get()
            .filter(|id| del_ids.contains(id))
    });
    let gaps_key = key.clone();
    let file_expanded_gaps = computed(move || {
        st.expanded_gaps.with(|map| {
            map.iter()
                .filter(|((k, _), _)| k == &gaps_key)
                .map(|((_, idx), exp)| (*idx, *exp))
                .collect::<BTreeMap<usize, super::model::GapExpansion>>()
        })
    });
    let load_key = key.clone();
    let file_loading = computed(move || st.file_lines_loading.with(|s| s.contains(&load_key)));
    let err_key = key.clone();
    let file_error = computed(move || st.file_lines_errors.with(|m| m.get(&err_key).cloned()));
    let cache_key = ctx.cache_key();
    let file_cache_ready = computed(move || {
        let _rev = st.file_lines_revision.get();
        st.file_lines_cache
            .with(|c| c.get(&cache_key).map(|entry| entry.total_lines))
    });
    let has_remote_threads = !ctx.remote_threads.is_empty();
    let login_sig = ctx.login;
    let file_login = computed(move || {
        if has_remote_threads {
            login_sig.get()
        } else {
            None
        }
    });

    let fold_reason = item.fold_reason;
    let expand_key = key;
    view()
        .element_ref(&target)
        .rounded(px(RADIUS))
        .border_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .overflow_hidden()
        .child(header)
        .child(dynamic(move || {
            if is_collapsed.get() {
                if let Some(reason) = fold_reason {
                    let k = expand_key.clone();
                    let prompt = match reason {
                        AutoFoldReason::Generated => {
                            "Generated or lockfile diff is collapsed by default."
                        }
                        AutoFoldReason::LargeDiff(_) => {
                            "Large file diff is collapsed by default for responsiveness."
                        }
                    };
                    view()
                        .px_3()
                        .py_2()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(muted(prompt).text_size(px(11.5)))
                        .child(compact_action("Load diff", Variant::Secondary, move |_| {
                            super::toggle_file(ui, k.clone());
                        }))
                        .into_element()
                } else {
                    view().into_element()
                }
            } else {
                render_file_body(
                    ui,
                    &ctx,
                    file_active_line.get(),
                    &file_pending.get(),
                    file_editing_remote.get(),
                    file_deleting_remote.get(),
                    file_login.get(),
                    &file_expanded_gaps.get(),
                    file_loading.get(),
                    file_error.get(),
                    file_cache_ready.get().is_some(),
                )
            }
        }))
}

#[allow(clippy::too_many_arguments)]
fn render_file_body(
    ui: Ui,
    ctx: &FileRenderContext,
    mut active_line: Option<LineTarget>,
    pending_all: &[PendingInlineComment],
    editing_remote: Option<u64>,
    deleting_remote: Option<u64>,
    my_login: Option<String>,
    expanded_gaps: &BTreeMap<usize, super::model::GapExpansion>,
    is_loading: bool,
    load_err: Option<String>,
    has_cached_file: bool,
) -> AnyElement {
    let st = ui.review_state;
    let cache_key = ctx.cache_key();
    let cached_file = if has_cached_file {
        st.file_lines_cache.with(|c| c.get(&cache_key))
    } else {
        None
    };

    let mut threads_by_line: HashMap<(u64, &str), Vec<&Value>> = HashMap::new();
    for rt in ctx.remote_threads.iter() {
        if let Some(ln) = rt["line"].as_u64().or_else(|| rt["original_line"].as_u64()) {
            let side = rt["side"].as_str().unwrap_or("RIGHT");
            threads_by_line.entry((ln, side)).or_default().push(rt);
        }
    }
    let mut pending_by_line: HashMap<(u64, &str), Vec<&PendingInlineComment>> = HashMap::new();
    for item in pending_all {
        pending_by_line
            .entry((item.line, item.side.as_str()))
            .or_default()
            .push(item);
    }

    let mut rendered_comment_ids = BTreeSet::new();

    let body = if ctx.patch.trim().is_empty() {
        view()
            .p_3()
            .child(muted(
                "Binary file, rename without content changes, or empty patch.",
            ))
            .into_element()
    } else {
        let hunks = &ctx.hunks;
        if hunks.is_empty() {
            view()
                .p_3()
                .child(muted(
                    "Binary file, rename without content changes, or empty patch.",
                ))
                .into_element()
        } else {
            let mut rows = view();
            let hunk_count = hunks.len();
            let is_added_or_removed = ctx.status == "added" || ctx.status == "removed";

            // Gap 0: before Hunk 0
            let first_entry_new = hunks[0].entry_new();
            let top_gap_len = first_entry_new.saturating_sub(1);
            let top_exp = expanded_gaps.get(&0).copied().unwrap_or_default();
            let top_shown = if cached_file.is_some() {
                top_exp.from_bottom.min(top_gap_len)
            } else {
                0
            };
            let top_remaining = top_gap_len.saturating_sub(top_shown);

            if !is_added_or_removed && top_remaining > 0 {
                let header_txt = hunks[0].formatted_header(top_shown);
                let label_txt =
                    format_gap_label(&header_txt, top_remaining, is_loading, load_err.as_deref());
                rows = rows.child(render_expander_bar(
                    ui,
                    ctx,
                    0,
                    None,
                    Some(0),
                    Some(top_remaining),
                    ExpandKind::TopOnly,
                    label_txt,
                    is_loading,
                    load_err.is_some(),
                ));
            } else if top_gap_len == 0 {
                rows = rows.child(render_static_hunk_bar(&hunks[0].header));
            }

            if let Some(ref cached) = cached_file
                && top_shown > 0
            {
                let start_ln = first_entry_new - top_shown;
                for ln in start_ln..first_entry_new {
                    let dl = expanded_context_line(cached, ln, ln);
                    rows = render_diff_line_row(
                        ui,
                        ctx,
                        &dl,
                        None,
                        &mut active_line,
                        &threads_by_line,
                        &pending_by_line,
                        editing_remote,
                        deleting_remote,
                        my_login.as_deref(),
                        &mut rendered_comment_ids,
                        rows,
                    );
                }
            }

            for (i, hunk) in hunks.iter().enumerate() {
                let start_ref = anchor_ref_for(st, &ctx.key, i, AnchorEdge::HunkStart);
                let end_ref = anchor_ref_for(st, &ctx.key, i, AnchorEdge::HunkEnd);
                for (li, dl) in hunk.lines.iter().enumerate() {
                    if dl.kind == '\\' {
                        rows = rows.child(render_static_hunk_bar(&dl.text));
                        continue;
                    }
                    let line_ref = if li == 0 {
                        Some(start_ref)
                    } else if li + 1 == hunk.lines.len() {
                        Some(end_ref)
                    } else {
                        None
                    };
                    if li == 0 && hunk.lines.len() == 1 {
                        st.anchor_targets.with(|m| {
                            m.insert((ctx.key.clone(), i, AnchorEdge::HunkEnd), start_ref);
                        });
                    }
                    rows = render_diff_line_row(
                        ui,
                        ctx,
                        dl,
                        line_ref,
                        &mut active_line,
                        &threads_by_line,
                        &pending_by_line,
                        editing_remote,
                        deleting_remote,
                        my_login.as_deref(),
                        &mut rendered_comment_ids,
                        rows,
                    );
                }

                // Gap i + 1: between Hunk i and Hunk i + 1
                if i + 1 < hunk_count {
                    let next_hunk = &hunks[i + 1];
                    let gap_start_old = hunk.exit_old();
                    let gap_start_new = hunk.exit_new();
                    let gap_end_new = next_hunk.entry_new();
                    let gap_len = gap_end_new.saturating_sub(gap_start_new);
                    let gap_exp = expanded_gaps.get(&(i + 1)).copied().unwrap_or_default();
                    let shown_top = if cached_file.is_some() {
                        gap_exp.from_top.min(gap_len)
                    } else {
                        0
                    };
                    let shown_bottom = if cached_file.is_some() {
                        gap_exp.from_bottom.min(gap_len.saturating_sub(shown_top))
                    } else {
                        0
                    };
                    let remaining = gap_len.saturating_sub(shown_top + shown_bottom);

                    if let Some(ref cached) = cached_file {
                        for off in 0..shown_top {
                            let dl = expanded_context_line(
                                cached,
                                gap_start_old + off,
                                gap_start_new + off,
                            );
                            rows = render_diff_line_row(
                                ui,
                                ctx,
                                &dl,
                                None,
                                &mut active_line,
                                &threads_by_line,
                                &pending_by_line,
                                editing_remote,
                                deleting_remote,
                                my_login.as_deref(),
                                &mut rendered_comment_ids,
                                rows,
                            );
                        }
                    }

                    if remaining > 0 {
                        let header_txt = next_hunk.formatted_header(shown_bottom);
                        let label_txt = format_gap_label(
                            &header_txt,
                            remaining,
                            is_loading,
                            load_err.as_deref(),
                        );
                        let kind = if remaining <= CONTEXT_STEP {
                            ExpandKind::MergeAll
                        } else {
                            ExpandKind::SplitUpDown
                        };
                        rows = rows.child(render_expander_bar(
                            ui,
                            ctx,
                            i + 1,
                            Some(i),
                            Some(i + 1),
                            Some(remaining),
                            kind,
                            label_txt,
                            is_loading,
                            load_err.is_some(),
                        ));
                    }

                    if let Some(ref cached) = cached_file
                        && shown_bottom > 0
                    {
                        let gap_end_old = next_hunk.entry_old();
                        let start_new = gap_end_new - shown_bottom;
                        let start_old = gap_end_old.saturating_sub(shown_bottom);
                        for off in 0..shown_bottom {
                            let dl =
                                expanded_context_line(cached, start_old + off, start_new + off);
                            rows = render_diff_line_row(
                                ui,
                                ctx,
                                &dl,
                                None,
                                &mut active_line,
                                &threads_by_line,
                                &pending_by_line,
                                editing_remote,
                                deleting_remote,
                                my_login.as_deref(),
                                &mut rendered_comment_ids,
                                rows,
                            );
                        }
                    }
                }
            }

            if !is_added_or_removed
                && let Some(last_hunk) = hunks.last()
                && !last_hunk.ends_with_no_newline()
            {
                let gap_idx = hunk_count;
                let gap_start_old = last_hunk.exit_old();
                let gap_start_new = last_hunk.exit_new();
                let bot_exp = expanded_gaps.get(&gap_idx).copied().unwrap_or_default();
                let (shown_top, remaining_opt) = if let Some(ref cached) = cached_file {
                    let gap_end_new = (cached.total_lines as u64 + 1).max(gap_start_new);
                    let gap_len = gap_end_new.saturating_sub(gap_start_new);
                    let shown = bot_exp.from_top.min(gap_len);
                    (shown, Some(gap_len.saturating_sub(shown)))
                } else {
                    (0, None)
                };

                if let Some(ref cached) = cached_file {
                    for off in 0..shown_top {
                        let dl =
                            expanded_context_line(cached, gap_start_old + off, gap_start_new + off);
                        rows = render_diff_line_row(
                            ui,
                            ctx,
                            &dl,
                            None,
                            &mut active_line,
                            &threads_by_line,
                            &pending_by_line,
                            editing_remote,
                            deleting_remote,
                            my_login.as_deref(),
                            &mut rendered_comment_ids,
                            rows,
                        );
                    }
                }

                if remaining_opt.is_none_or(|r| r > 0) {
                    let label_txt = if is_loading {
                        "Loading context…".into()
                    } else if let Some(ref err) = load_err {
                        err.clone()
                    } else if let Some(rem) = remaining_opt {
                        if rem <= CONTEXT_STEP {
                            format!(
                                "··· Expand remaining {rem} line{} below ···",
                                if rem == 1 { "" } else { "s" }
                            )
                        } else {
                            format!("··· Expand {CONTEXT_STEP} lines below ({rem} remaining) ···")
                        }
                    } else {
                        format!("··· Expand {CONTEXT_STEP} lines below ···")
                    };
                    rows = rows.child(render_expander_bar(
                        ui,
                        ctx,
                        gap_idx,
                        Some(hunk_count - 1),
                        None,
                        remaining_opt,
                        ExpandKind::BottomOnly,
                        label_txt,
                        is_loading,
                        load_err.is_some(),
                    ));
                }
            }

            rows.into_element()
        }
    };

    let extra_composer = active_line
        .map(|target| inline_composer_box(ui, target).into_element())
        .unwrap_or_else(|| view().into_element());

    let mut unanchored_for_file = view();
    for item in pending_all
        .iter()
        .filter(|c| !rendered_comment_ids.contains(&c.id))
    {
        unanchored_for_file = unanchored_for_file.child(render_pending_comment_card(ui, item));
    }

    view()
        .child(extra_composer)
        .child(unanchored_for_file)
        .child(body)
        .into_element()
}

fn format_gap_label(
    header_txt: &str,
    remaining: u64,
    is_loading: bool,
    load_err: Option<&str>,
) -> String {
    if is_loading {
        format!("{header_txt} · Loading context…")
    } else if let Some(err) = load_err {
        format!("{header_txt} · {err}")
    } else {
        format!(
            "{header_txt} · {remaining} hidden line{}",
            if remaining == 1 { "" } else { "s" }
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn render_diff_line_row(
    ui: Ui,
    ctx: &FileRenderContext,
    dl: &DiffLine,
    row_ref: Option<ElementRef>,
    active_line: &mut Option<LineTarget>,
    threads_by_line: &HashMap<(u64, &str), Vec<&Value>>,
    pending_by_line: &HashMap<(u64, &str), Vec<&PendingInlineComment>>,
    editing_remote: Option<u64>,
    deleting_remote: Option<u64>,
    my_login: Option<&str>,
    rendered_comment_ids: &mut BTreeSet<String>,
    mut rows: View,
) -> View {
    let st = ui.review_state;
    let (comment_line, comment_side) = if dl.kind == '-' {
        (dl.old_line.unwrap_or(1), "LEFT")
    } else {
        (dl.new_line.or(dl.old_line).unwrap_or(1), "RIGHT")
    };
    let old_str = dl
        .old_line
        .map(|n| format!("{n:>4}"))
        .unwrap_or_else(|| "    ".into());
    let new_str = dl
        .new_line
        .map(|n| format!("{n:>4}"))
        .unwrap_or_else(|| "    ".into());
    let prefix = dl.kind;
    let snippet_for_target = dl.text.clone();
    let line_path = ctx.path.clone();
    let side_owned = comment_side.to_owned();

    let text_col = match dl.kind {
        '+' => SUCCESS,
        '-' => DANGER,
        _ => TEXT,
    };
    let mut line_btn = canban_ui_kit::press_target()
        .w_full()
        .px_2()
        .py_0p5()
        .flex_row()
        .items_start()
        .gap_2();
    if let Some(ref r) = row_ref {
        line_btn = line_btn.element_ref(r);
    }
    line_btn = match dl.kind {
        '+' => line_btn.bg(SUCCESS_SURFACE),
        '-' => line_btn.bg(DANGER_SURFACE),
        _ => line_btn.bg(CLEAR),
    };

    let line_section = ctx.section.clone();
    rows = rows.child(
        line_btn
            .hover(|s| s.bg(SELECTED))
            .accessibility_label(format!(
                "Add review comment on {} line {comment_line}",
                ctx.path
            ))
            .on_press(move |_: &()| {
                st.active_line.set(Some(LineTarget {
                    path: line_path.clone(),
                    section: line_section.clone(),
                    line: comment_line,
                    side: side_owned.clone(),
                    snippet: snippet_for_target.clone(),
                    editing_author: None,
                }));
                st.line_draft.set(String::new());
            })
            .child(
                label(format!("{old_str} {new_str}"))
                    .w(px(74.))
                    .flex_shrink_0()
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .text_color(MUTED),
            )
            .child(
                label(format!("{prefix} {}", dl.text))
                    .flex_1()
                    .min_w(px(0.))
                    .font_family(MONO_FONT)
                    .text_size(px(12.))
                    .text_color(text_col),
            ),
    );

    // Render existing remote GitHub PR review comments on this exact line
    if let Some(line_threads) = threads_by_line.get(&(comment_line, comment_side)) {
        for &rt in line_threads {
            let author = rt["user"]["login"]
                .as_str()
                .unwrap_or("reviewer")
                .to_owned();
            let cid = rt["id"].as_u64().unwrap_or(0);
            let web_url = crate::github::comment_web_url(rt, ctx.pr_base_url.as_deref(), "threads");
            let is_own = cid > 0
                && ctx.pr_target.is_some()
                && my_login.is_some_and(|me| author.eq_ignore_ascii_case(me));
            let is_editing = cid > 0 && editing_remote == Some(cid);
            let is_deleting = cid > 0 && deleting_remote == Some(cid);
            let was_edited = match (rt["created_at"].as_str(), rt["updated_at"].as_str()) {
                (Some(c), Some(u)) if !c.is_empty() && !u.is_empty() => c != u,
                _ => false,
            };
            let body_txt = rt["body"].as_str().unwrap_or("").to_owned();
            let mut header_row = view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(icon(Icon::GitHub).w(px(12.)).h(px(12.)))
                .child(
                    label(format!("@{author}"))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(badge("GitHub PR comment"));
            if was_edited {
                header_row = header_row.child(muted("· edited").text_size(px(11.)));
            }
            let mut trailing = view().flex_row().items_center().gap_1();
            let mut has_trailing = false;
            if let Some(jump_url) = web_url {
                has_trailing = true;
                trailing = trailing.child(
                    view()
                        .tooltip(hint_tooltip("Open comment in GitHub"))
                        .child(
                            compact_action("GitHub ↗", Variant::Quiet, move |_| {
                                crate::github::open_url(ui, jump_url.clone())
                            })
                            .accessibility_label(|| Some("Open comment in GitHub".into())),
                        ),
                );
            }
            if is_own && !is_editing && !is_deleting {
                has_trailing = true;
                let initial_body = body_txt.clone();
                let anchor = element_ref();
                trailing = trailing.child(
                    canban_ui_kit::press_target()
                        .element_ref(&anchor)
                        .w(px(26.))
                        .h(px(26.))
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .bg(CLEAR)
                        .hover(|s| s.bg(INSET))
                        .focus(|s| s.border_1().border_color(ACCENT))
                        .accessibility_label(|| Some("PR comment actions".into()))
                        .child(icon_tinted(Icon::More, MUTED).w(px(15.)).h(px(15.)))
                        .on_press(move |_: &()| {
                            use nsh_core::element::desktop::{MenuItem, popup_menu_at};
                            let body_val = initial_body.clone();
                            ui.spawn(async move {
                                let Some(bounds) = anchor.measure().await else {
                                    return;
                                };
                                popup_menu_at(
                                    vec![
                                        MenuItem::new("Edit comment").on_select(move || {
                                            let b = body_val.clone();
                                            batch(move || {
                                                st.remote_edit_error.set(String::new());
                                                st.remote_delete_error.set(String::new());
                                                st.deleting_remote_comment.set(None);
                                                st.remote_edit_draft.set(b);
                                                st.editing_remote_comment.set(Some(cid));
                                            });
                                        }),
                                        MenuItem::new("Delete comment…").on_select(move || {
                                            batch(move || {
                                                st.remote_delete_error.set(String::new());
                                                st.remote_edit_error.set(String::new());
                                                st.editing_remote_comment.set(None);
                                                st.deleting_remote_comment.set(Some(cid));
                                            });
                                        }),
                                    ],
                                    point(bounds.page.x, bounds.page.y + bounds.frame.size.height),
                                )
                                .await;
                            });
                        }),
                );
            }
            if has_trailing {
                header_row = header_row.child(view().flex_1()).child(trailing);
            }
            let mut comment_card = view()
                .mx_4()
                .my_1()
                .p_2p5()
                .rounded(px(RADIUS))
                .border_1()
                .border_color(if is_deleting {
                    DANGER
                } else if is_editing {
                    ACCENT
                } else {
                    BORDER
                })
                .bg(SURFACE)
                .gap_1p5()
                .child(header_row);
            if is_deleting {
                comment_card = comment_card.child(remote_pr_comment_delete_confirm(
                    ui,
                    cid,
                    ctx.pr_target.clone(),
                ));
            }
            let content = if is_editing {
                remote_pr_comment_editor(ui, cid, ctx.pr_target.clone()).into_element()
            } else {
                canban_ui_kit::markdown::reader(move || body_txt.clone()).into_element()
            };
            rows = rows.child(comment_card.child(content));
        }
    }

    // Render any pending inline comments saved for this file + line + side
    if let Some(line_pending) = pending_by_line.get(&(comment_line, comment_side)) {
        for &item in line_pending {
            if rendered_comment_ids.insert(item.id.clone()) {
                rows = rows.child(render_pending_comment_card(ui, item));
            }
        }
    }

    // Render active inline composer right beneath the clicked line
    if active_line
        .as_ref()
        .is_some_and(|target| target.line == comment_line && target.side == comment_side)
        && let Some(target) = active_line.take()
    {
        rows = rows.child(inline_composer_box(ui, target));
    }

    rows
}

fn render_pending_comment_card(ui: Ui, item: &PendingInlineComment) -> View {
    render_pending_comment_card_with_jump(ui, item, None)
}

fn render_pending_comment_card_with_jump(
    ui: Ui,
    item: &PendingInlineComment,
    jump_key: Option<String>,
) -> View {
    let st = ui.review_state;
    let item_id = item.id.clone();
    let edit_item = item.clone();
    let body_txt = item.body.clone();
    let badge_label = match (&item.author, item.edited_by_user) {
        (Some(author), true) => format!("{author} · Edited draft"),
        (Some(author), false) => format!("{author} · Draft"),
        (None, _) => "Pending comment".into(),
    };
    let tone = if item.author.is_some() && !item.edited_by_user {
        Tone::Highlight
    } else {
        Tone::Info
    };
    let side_note = if item.side == "LEFT" { " (LEFT)" } else { "" };
    let section_note = if item.section.is_empty() {
        String::new()
    } else {
        format!(" [{}]", item.section)
    };
    let mut actions = view().flex_row().items_center().gap_1();
    if let Some(key) = jump_key {
        let jump_label = if item.section.is_empty() {
            format!("Show {} in diff", item.path)
        } else {
            format!("Show {} {} in diff", item.section, item.path)
        };
        actions = actions.child(
            compact_action("Show in diff", Variant::Quiet, move |_| {
                super::focus_file(ui, key.clone());
            })
            .accessibility_label(jump_label),
        );
    }
    let edit_label = if item.section.is_empty() {
        format!("Edit comment on {}", item.path)
    } else {
        format!("Edit comment on {} ({})", item.path, item.section)
    };
    actions = actions
        .child(
            compact_action("Edit", Variant::Quiet, move |_| {
                let cur = edit_item.clone();
                st.pending_comments.update(|list| {
                    list.retain(|x| x.id != cur.id);
                });
                st.active_line.set(Some(LineTarget {
                    path: cur.path,
                    section: cur.section,
                    line: cur.line,
                    side: cur.side,
                    snippet: cur.snippet,
                    editing_author: cur.author,
                }));
                st.line_draft.set(cur.body);
            })
            .accessibility_label(edit_label),
        )
        .child(compact_action("Discard ×", Variant::Quiet, move |_| {
            let rm_id = item_id.clone();
            st.pending_comments.update(|list| {
                list.retain(|x| x.id != rm_id);
            });
            super::sync_pending_draft_to_server(ui);
        }));
    view()
        .mx_4()
        .my_1p5()
        .p_2p5()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(ACCENT)
        .bg(SURFACE)
        .gap_1p5()
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
                        .gap_1p5()
                        .child(status(badge_label, tone))
                        .child(
                            muted(format!(
                                "{}:{}{side_note}{section_note}",
                                item.path, item.line
                            ))
                            .font_family(MONO_FONT)
                            .text_size(px(11.)),
                        ),
                )
                .child(actions),
        )
        .child(canban_ui_kit::markdown::reader(move || body_txt.clone()))
}

fn inline_composer_box(ui: Ui, target: LineTarget) -> View {
    let st = ui.review_state;
    let loc_label = format!(
        "Comment on {}:{} ({})",
        target.path,
        target.line,
        if target.side == "LEFT" {
            "old side"
        } else {
            "new side"
        }
    );
    view()
        .mx_4()
        .my_1p5()
        .p_3()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(ACCENT)
        .bg(INSET)
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    label(loc_label)
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(chrome_button(
                    Icon::Close,
                    "Cancel inline comment",
                    move |_| {
                        st.active_line.set(None);
                        st.line_draft.set(String::new());
                    },
                )),
        )
        .child(multiline_comment_input(
            st.line_draft,
            "Inline review comment",
            "Describe the issue, question, or change you want @Agent or the author to make…",
            72.,
        ))
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_end()
                .gap_2()
                .child(compact_action("Cancel", Variant::Quiet, move |_| {
                    st.active_line.set(None);
                    st.line_draft.set(String::new());
                }))
                .child(
                    compact_action("Add to Review", Variant::Primary, move |_| {
                        let body = st.line_draft.get_untracked();
                        if body.trim().is_empty() {
                            return;
                        }
                        super::ensure_active_draft_scope(ui);
                        st.pending_comments.update(|list| {
                            list.push(PendingInlineComment {
                                id: canban_types::id(),
                                path: target.path.clone(),
                                section: target.section.clone(),
                                line: target.line,
                                side: target.side.clone(),
                                snippet: target.snippet.clone(),
                                body: body.trim().to_owned(),
                                author: target.editing_author.clone(),
                                edited_by_user: target.editing_author.is_some(),
                            });
                        });
                        st.active_line.set(None);
                        st.line_draft.set(String::new());
                        super::sync_pending_draft_to_server(ui);
                    })
                    .disabled(move || st.line_draft.with(|s| s.trim().is_empty())),
                ),
        )
}

fn remote_pr_comment_editor(ui: Ui, comment_id: u64, pr_target: Option<Value>) -> View {
    let st = ui.review_state;
    view()
        .gap_2()
        .child(multiline_comment_input(
            st.remote_edit_draft,
            "Edit GitHub PR inline comment",
            "Update your GitHub PR comment…",
            72.,
        ))
        .child(when(
            move || !st.remote_edit_error.get().is_empty(),
            move || notice(move || st.remote_edit_error.get(), Tone::Danger),
        ))
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_end()
                .gap_2()
                .child(compact_action("Cancel", Variant::Quiet, move |_| {
                    batch(move || {
                        st.editing_remote_comment.set(None);
                        st.remote_edit_draft.set(String::new());
                        st.remote_edit_error.set(String::new());
                    });
                }))
                .child(
                    compact_action("Save", Variant::Primary, move |_| {
                        let next_body = st.remote_edit_draft.get_untracked();
                        if next_body.trim().is_empty() {
                            st.remote_edit_error.set("Comment cannot be empty".into());
                            return;
                        }
                        let Some(ref pr) = pr_target else {
                            return;
                        };
                        let repo = pr["repository"].as_str().unwrap_or("").to_owned();
                        let number = pr["number"].as_u64().unwrap_or(0);
                        let url = pr["url"]
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("https://github.com/{repo}/pull/{number}"));
                        if repo.is_empty() || number == 0 {
                            st.remote_edit_error
                                .set("Missing GitHub pull request reference".into());
                            return;
                        }
                        st.remote_edit_error.set(String::new());
                        if !crate::backend::is_connected() {
                            st.compare_data.update(|cd| {
                                if let Some(arr) = cd["threads"].as_array_mut()
                                    && let Some(item) =
                                        arr.iter_mut().find(|x| x["id"].as_u64() == Some(comment_id))
                                {
                                    item["body"] = json!(next_body.trim());
                                    item["updated_at"] = json!("2026-04-19T12:00:00Z");
                                }
                            });
                            st.editing_remote_comment.set(None);
                            st.remote_edit_draft.set(String::new());
                            ui.notify("Saved");
                            return;
                        }
                        let reference = canban_types::GitHubReference {
                            repository: repo,
                            kind: "pr".into(),
                            number: Some(number),
                            sha: None,
                            url,
                        };
                        dispatch_complete(
                            ui,
                            "github.remote.comment.update",
                            json!({
                                "reference": reference,
                                "section": "threads",
                                "comment_id": comment_id,
                                "body": next_body,
                                "request_key": canban_types::id(),
                            }),
                            move |res| match res {
                                Ok(val) if val["state"] == "published" => {
                                    st.editing_remote_comment.set(None);
                                    st.remote_edit_draft.set(String::new());
                                    st.remote_edit_error.set(String::new());
                                    crate::review::refresh_active_diff(ui, true);
                                    ui.notify("Saved");
                                }
                                Ok(val) if val["state"] == "unknown" => {
                                    st.remote_edit_error.set(
                                        "Publication outcome is unknown; check GitHub before retrying."
                                            .into(),
                                    );
                                }
                                Ok(val) => {
                                    let msg = val["message"]
                                        .as_str()
                                        .unwrap_or("Could not update GitHub PR comment");
                                    st.remote_edit_error.set(msg.to_owned());
                                }
                                Err(err) => {
                                    st.remote_edit_error.set(err);
                                }
                            },
                        );
                    })
                    .disabled(move || {
                        ui.is_method_pending("github.remote.comment.update")
                            || st.remote_edit_draft.with(|s| s.trim().is_empty())
                    }),
                ),
        )
}

fn remote_pr_comment_delete_confirm(ui: Ui, comment_id: u64, pr_target: Option<Value>) -> View {
    let st = ui.review_state;
    view()
        .gap_2()
        .px_2p5()
        .py_1p5()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(DANGER)
        .bg(DANGER_SURFACE)
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    label("Delete this inline comment on GitHub?")
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(DANGER),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .child(
                            compact_action("Cancel", Variant::Quiet, move |_| {
                                batch(move || {
                                    st.deleting_remote_comment.set(None);
                                    st.remote_delete_error.set(String::new());
                                });
                            })
                            .disabled(move || ui.is_method_pending("github.remote.comment.delete")),
                        )
                        .child(
                            compact_action("Delete", Variant::Danger, move |_| {
                                let Some(ref pr) = pr_target else {
                                    return;
                                };
                                let repo = pr["repository"].as_str().unwrap_or("").to_owned();
                                let number = pr["number"].as_u64().unwrap_or(0);
                                let url = pr["url"]
                                    .as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| {
                                        format!("https://github.com/{repo}/pull/{number}")
                                    });
                                if repo.is_empty() || number == 0 {
                                    st.remote_delete_error
                                        .set("Missing GitHub pull request reference".into());
                                    return;
                                }
                                st.remote_delete_error.set(String::new());
                                if !crate::backend::is_connected() {
                                    st.compare_data.update(|cd| {
                                        if let Some(arr) = cd["threads"].as_array_mut() {
                                            arr.retain(|x| x["id"].as_u64() != Some(comment_id));
                                        }
                                    });
                                    st.deleting_remote_comment.set(None);
                                    ui.notify("Comment deleted");
                                    return;
                                }
                                let reference = canban_types::GitHubReference {
                                    repository: repo,
                                    kind: "pr".into(),
                                    number: Some(number),
                                    sha: None,
                                    url,
                                };
                                dispatch_complete(
                                    ui,
                                    "github.remote.comment.delete",
                                    json!({
                                        "reference": reference,
                                        "section": "threads",
                                        "comment_id": comment_id,
                                        "request_key": canban_types::id(),
                                    }),
                                    move |res| match res {
                                        Ok(val) if val["state"] == "published" => {
                                            st.deleting_remote_comment.set(None);
                                            st.remote_delete_error.set(String::new());
                                            crate::review::refresh_active_diff(ui, true);
                                            ui.notify("Comment deleted");
                                        }
                                        Ok(val) if val["state"] == "unknown" => {
                                            st.remote_delete_error.set(
                                                val["message"]
                                                    .as_str()
                                                    .unwrap_or(
                                                        "Deletion outcome is unknown; check GitHub before retrying.",
                                                    )
                                                    .to_owned(),
                                            );
                                        }
                                        Ok(val) => {
                                            st.remote_delete_error.set(
                                                val["message"]
                                                    .as_str()
                                                    .unwrap_or(
                                                        "Could not delete GitHub PR comment",
                                                    )
                                                    .to_owned(),
                                            );
                                        }
                                        Err(err) => {
                                            st.remote_delete_error.set(err);
                                        }
                                    },
                                );
                            })
                            .disabled(move || ui.is_method_pending("github.remote.comment.delete")),
                        ),
                ),
        )
        .child(when(
            move || !st.remote_delete_error.get().is_empty(),
            move || notice(move || st.remote_delete_error.get(), Tone::Danger),
        ))
}

/// Both file navigation and Changes dispatch this exact per-patch fold action.
pub fn fold_button(ui: Ui, key: String, title: String) -> impl IntoElement {
    let st = ui.review_state;
    let check_key = key.clone();
    let folded = computed(move || st.collapsed_files.with(|files| files.contains(&check_key)));
    dynamic(move || {
        let is_folded = folded.get();
        let label = format!(
            "{} diff for {title}",
            if is_folded { "Expand" } else { "Collapse" }
        );
        let toggle_key = key.clone();
        chrome_button(
            if is_folded {
                Icon::Forward
            } else {
                Icon::ChevronDown
            },
            label,
            move |_| super::toggle_file(ui, toggle_key.clone()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_file_display_and_working_tree_stats_lookup() {
        assert_eq!(split_file_display("Cargo.toml"), ("Cargo.toml", None));
        assert_eq!(split_file_display("/Cargo.toml"), ("Cargo.toml", None));
        assert_eq!(
            split_file_display("crates/ui/src/review/diff.rs"),
            ("diff.rs", Some("crates/ui/src/review".into()))
        );
        assert_eq!(
            split_file_display("crates/app-server/src/github/delivery.rs"),
            ("delivery.rs", Some("…/src/github".into()))
        );

        assert_eq!(file_path_tooltip("a.rs", None), "a.rs");
        assert_eq!(file_path_tooltip("a.rs", Some("")), "a.rs");
        assert_eq!(file_path_tooltip("a.rs", Some("a.rs")), "a.rs");
        assert_eq!(
            file_path_tooltip("new.rs", Some("old.rs")),
            "old.rs → new.rs"
        );

        let data = json!({
            "files": [
                {"filename": "src/a.rs", "section": "staged", "additions": 10, "deletions": 2},
                {"filename": "src/a.rs", "section": "unstaged", "additions": 3, "deletions": 1},
                {"filename": "src/b.rs", "section": "", "additions": 7, "deletions": 0},
            ]
        });
        let stats = super::git_ops::build_working_tree_stats(&data);
        assert_eq!(
            super::git_ops::lookup_working_tree_stats(&stats, "UNCOMMITTED", true, "src/a.rs"),
            Some((10, 2))
        );
        assert_eq!(
            super::git_ops::lookup_working_tree_stats(&stats, "UNCOMMITTED", false, "src/a.rs"),
            Some((3, 1))
        );
        assert_eq!(
            super::git_ops::lookup_working_tree_stats(&stats, "STAGED", true, "src/b.rs"),
            Some((7, 0))
        );
        assert_eq!(
            super::git_ops::lookup_working_tree_stats(&stats, "STAGED", false, "src/b.rs"),
            None
        );
        assert_eq!(
            super::git_ops::lookup_working_tree_stats(&stats, "UNSTAGED", true, "src/b.rs"),
            None
        );
        assert_eq!(
            super::git_ops::lookup_working_tree_stats(&stats, "UNSTAGED", false, "src/b.rs"),
            Some((7, 0))
        );
        assert_eq!(
            super::git_ops::lookup_working_tree_stats(&stats, "WORKDIR", true, "src/b.rs"),
            Some((7, 0))
        );
        assert_eq!(
            super::git_ops::lookup_working_tree_stats(&stats, "UNCOMMITTED", true, "missing.rs"),
            None
        );
    }
}
