use super::{State, git_ops, open_compare, refresh_active_diff, select_git_pair};
use crate::presenter::*;
use canban_ui_kit::tokens::*;
use canban_ui_kit::*;
use nsh_core::element::desktop::{MenuItem, popup_menu, popup_menu_at};
use nsh_core::prelude::*;
use serde_json::{Value, json};

pub fn render_refresh(ui: Ui) -> impl IntoElement {
    let busy = computed(move || {
        let st = ui.review_state;
        st.refs_loading.get()
            || st.log_loading.get()
            || st.compare_loading.get()
            || git_ops::sync_pending(ui)
    });
    dynamic(move || {
        let busy = busy.get();
        canban_ui_kit::chrome::sync_action(
            Icon::Refresh,
            if busy {
                "Synchronizing Git…"
            } else {
                "Refresh Git refs, log & diff"
            }
            .into(),
            busy,
            busy,
            move |_| super::refresh(ui),
        )
    })
}

const COMMITS_AUTO_LOAD_THRESHOLD: f32 = 160.;

pub fn render_left_sidebar(ui: Ui) -> impl IntoElement {
    view()
        .w(px(244.))
        .flex_shrink_0()
        .min_h(px(0.))
        .border_r_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .child(render_commits(ui))
}

fn log_branch(st: State) -> String {
    let branch = st.log_branch.get();
    if !branch.is_empty() {
        return branch;
    }
    st.refs_data.with(|r| {
        r["current_branch"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| r["default_branch"].as_str())
            .unwrap_or("HEAD")
            .to_owned()
    })
}

fn log_branch_untracked(st: State) -> String {
    let branch = st.log_branch.get_untracked();
    if !branch.is_empty() {
        return branch;
    }
    st.refs_data.with_untracked(|r| {
        r["current_branch"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| r["default_branch"].as_str())
            .unwrap_or("HEAD")
            .to_owned()
    })
}

fn maybe_auto_load_more_commits(ui: Ui, e: &ScrollEvent) {
    let st = ui.review_state;
    if !st.log_has_more.get_untracked() || st.log_loading.get_untracked() {
        return;
    }
    let viewport_h = e.size.height.0;
    let content_h = e.content_size.height.0;
    if viewport_h <= 0. || e.offset.y.0 <= 0. || content_h <= viewport_h {
        return;
    }
    let remaining = content_h - e.offset.y.0 - viewport_h;
    if remaining <= COMMITS_AUTO_LOAD_THRESHOLD {
        git_ops::load_branch_log(ui, log_branch_untracked(st), true);
    }
}

/// Mounted outside the changing sidebar rows: asynchronous reads never replace
/// the branch picker or either search field.
fn render_commits(ui: Ui) -> impl IntoElement {
    let st = ui.review_state;
    let commits_scroll = element_ref();
    let last_commits_metrics = stored(Option::<ScrollEvent>::None);
    watch_scroll(
        move || commits_scroll.node(),
        move |e: &ScrollEvent| {
            last_commits_metrics.replace(Some(*e));
            maybe_auto_load_more_commits(ui, e);
        },
    );
    effect(move || {
        let has_more = st.log_has_more.get();
        let loading = st.log_loading.get();
        let _len = st.log_commits.with(Vec::len);
        if has_more
            && !loading
            && let Some(m) = last_commits_metrics.get()
        {
            untracked(|| maybe_auto_load_more_commits(ui, &m));
        }
    });
    let trigger = canban_ui_kit::press_target()
        .px_1p5()
        .py_0p5()
        .rounded(px(4.))
        .border_1()
        .border_color(BORDER)
        .bg(INSET)
        .hover(|s| s.bg(CARD_HOVER))
        .flex_row()
        .items_center()
        .gap_1()
        .accessibility_label("Choose or type branch/ref for commit history")
        .child(icon(Icon::Branch).w(px(10.5)).h(px(10.5)))
        .child(
            label(move || {
                let branch = log_branch(st);
                if branch.chars().count() > 16 {
                    format!("{}…", branch.chars().take(14).collect::<String>())
                } else {
                    branch
                }
            })
            .font_family(MONO_FONT)
            .text_size(px(10.5)),
        )
        .child(icon(Icon::ChevronDown).w(px(10.)).h(px(10.)));
    let picker = search_popover(
        trigger,
        move || {
            st.refs_data
                .with(|r| build_log_ref_options(r, &log_branch(st)))
        },
        move |chosen| git_ops::load_branch_log(ui, chosen, false),
    )
    .placeholder("Search or type branch / ref…")
    .empty_text("No matching branches or tags")
    .custom_option(|q| {
        Some(
            PickerOption::new(q.to_owned(), format!("Use ref \"{q}\""))
                .detail("custom")
                .icon(Icon::Branch)
                .mono(true),
        )
    })
    .width(268.)
    .list_height(260.)
    .align(Align::End);
    let query = input::debounced(st.log_query, std::time::Duration::from_millis(150));
    let previous = stored(st.log_query.get_untracked());
    effect(move || {
        let q = query.get();
        if previous.with(|p| *p != q) {
            previous.replace(q);
            git_ops::load_branch_log(ui, log_branch(st), false);
        }
    });
    view()
        .flex_1()
        .min_h(px(0.))
        .child(
            view()
                .pt_2p5()
                .px_2p5()
                .pb_1p5()
                .gap_1p5()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    view()
                        .px_1p5()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_1()
                        .child(
                            muted("Commits")
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD),
                        )
                        .child(picker),
                )
                .child(
                    view().px_1().child(
                        field(st.log_query, "Filter commits or SHA…")
                            .min_h(px(28.))
                            .px_2()
                            .py_1()
                            .text_size(px(11.5))
                            .on_submit(move |_: &str| {
                                git_ops::load_branch_log(ui, log_branch_untracked(st), false)
                            }),
                    ),
                )
                .child(dynamic(move || {
                    let count = st
                        .refs_data
                        .with(|v| v["dirty_count"].as_u64().unwrap_or(0));
                    sidebar_row(
                        "Uncommitted",
                        count.to_string(),
                        st.mode.get() == "git"
                            && matches!(
                                st.compare_ref.get().as_str(),
                                "UNCOMMITTED" | "WORKDIR" | "STAGED" | "UNSTAGED"
                            ),
                        move || {
                            st.compare_open.set(false);
                            select_git_pair(ui, "HEAD", "UNCOMMITTED");
                        },
                    )
                })),
        )
        .child(pull_refresh_scroll(
            scroll()
                .label("Git commits list")
                .on_scroll(move |e: &ScrollEvent| {
                    last_commits_metrics.replace(Some(*e));
                    maybe_auto_load_more_commits(ui, e);
                })
                .child(
                    view()
                        .px_2p5()
                        .py_1p5()
                        .child(dynamic(move || render_commit_rows(ui))),
                ),
            commits_scroll,
            move || st.log_refreshing.get(),
            move || git_ops::pull_refresh(ui, super::PullRefreshTarget::Commits),
        ))
}

fn render_commit_rows(ui: Ui) -> AnyElement {
    let st = ui.review_state;
    let display_commits = st.log_commits.get();
    let loading = st.log_loading.get();
    if display_commits.is_empty() {
        return view()
            .px_2()
            .py_1p5()
            .child(
                muted(if loading {
                    "Loading commits…"
                } else {
                    "No matching commits"
                })
                .text_size(px(11.)),
            )
            .into_element();
    }
    let mode = st.mode.get();
    let selected = st.compare_ref.get();
    let mut rows = view().gap_0p5();
    for cm in display_commits {
        let sha = cm["sha"].as_str().unwrap_or("").to_owned();
        if sha.is_empty() {
            continue;
        }
        let short = cm["short_sha"].as_str().unwrap_or("").to_owned();
        let subject = cm["subject"].as_str().unwrap_or("").to_owned();
        let time = cm["relative_time"].as_str().unwrap_or("").to_owned();
        rows = rows.child(sidebar_commit_row(
            short,
            subject,
            time,
            mode == "git" && selected == sha,
            move || {
                git_ops::select_single_commit(ui, sha.clone(), 0, false);
            },
        ));
    }
    if st.log_has_more.get() {
        rows = rows.child(
            view().pt_1().px_1().child(
                compact_action(
                    if loading {
                        "Loading…"
                    } else {
                        "Load more commits…"
                    },
                    Variant::Quiet,
                    move |_| git_ops::load_branch_log(ui, log_branch(st), true),
                )
                .disabled(move || st.log_loading.get()),
            ),
        );
    }
    rows.into_element()
}

fn sidebar_row(
    title: impl Into<SharedString>,
    meta: String,
    selected: bool,
    on_click: impl Fn() + 'static,
) -> impl IntoElement {
    let title_str = title.into();
    canban_ui_kit::press_target()
        .w_full()
        .min_h(px(28.))
        .px_2()
        .py_1()
        .rounded(px(5.))
        .flex_row()
        .items_center()
        .justify_between()
        .gap_1p5()
        .bg(if selected { SELECTED } else { CLEAR })
        .hover(|s| s.bg(if selected { SELECTED } else { CARD_HOVER }))
        .on_press(move |_: &()| on_click())
        .child(
            label(title_str)
                .text_size(px(12.))
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .truncate()
                .flex_1()
                .min_w(px(0.)),
        )
        .child(muted(meta).text_size(px(10.5)).flex_shrink_0())
}

fn sidebar_commit_row(
    short_sha: String,
    subject: String,
    rel_time: String,
    selected: bool,
    on_click: impl Fn() + 'static,
) -> impl IntoElement {
    view()
        .tooltip(hint_tooltip(format!(
            "{short_sha} · {subject} ({rel_time})"
        )))
        .child(
            canban_ui_kit::press_target()
                .w_full()
                .min_h(px(27.))
                .px_2()
                .py_0p5()
                .rounded(px(5.))
                .flex_row()
                .items_center()
                .gap_1p5()
                .bg(if selected { SELECTED } else { CLEAR })
                .hover(|s| s.bg(if selected { SELECTED } else { CARD_HOVER }))
                .on_press(move |_: &()| on_click())
                .child(
                    label(short_sha)
                        .font_family(MONO_FONT)
                        .text_size(px(11.))
                        .text_color(ACCENT)
                        .flex_shrink_0(),
                )
                .child(
                    label(subject)
                        .text_size(px(11.5))
                        .truncate()
                        .flex_1()
                        .min_w(px(0.)),
                ),
        )
}

fn build_log_ref_options(refs: &Value, current_selected: &str) -> Vec<PickerOption> {
    let branches = refs["branches"].as_array().cloned().unwrap_or_default();
    let tags = refs["tags"].as_array().cloned().unwrap_or_default();
    let mut options = Vec::new();

    for br in branches {
        let name = br["name"].as_str().unwrap_or("").to_owned();
        if name.is_empty() {
            continue;
        }
        let is_remote = br["remote"].as_bool().unwrap_or(false) || name.starts_with("origin/");
        let is_cur = br["is_current"].as_bool().unwrap_or(false);
        let is_def = br["is_default"].as_bool().unwrap_or(false);
        let short = br["short_sha"].as_str().unwrap_or("").to_owned();
        let detail = if is_cur {
            "current".to_owned()
        } else if is_def {
            "default".to_owned()
        } else {
            short.clone()
        };
        let grp = if is_remote {
            "Remote Branches"
        } else {
            "Branches"
        };
        options.push(
            PickerOption::new(name.clone(), name)
                .group(grp)
                .detail(detail)
                .icon(Icon::Branch)
                .mono(true)
                .selected(current_selected == br["name"].as_str().unwrap_or(""))
                .keywords([short]),
        );
    }

    for tg in tags {
        let name = tg["name"].as_str().unwrap_or("").to_owned();
        if name.is_empty() {
            continue;
        }
        let short = tg["short_sha"].as_str().unwrap_or("").to_owned();
        options.push(
            PickerOption::new(name.clone(), name.clone())
                .group("Tags")
                .detail(short.clone())
                .mono(true)
                .selected(current_selected == name)
                .keywords([short, "tag".into()]),
        );
    }

    options
}

pub(crate) fn build_git_ref_options(
    refs: &Value,
    is_base: bool,
    current_val: &str,
) -> Vec<PickerOption> {
    let dirty_count = refs["dirty_count"].as_u64().unwrap_or(0);
    let staged_count = refs["staged_count"].as_u64().unwrap_or(0);
    let branches = refs["branches"].as_array().cloned().unwrap_or_default();
    let tags = refs["tags"].as_array().cloned().unwrap_or_default();
    let commits = refs["commits"].as_array().cloned().unwrap_or_default();

    let mut options = Vec::new();

    if !is_base {
        options.push(
            PickerOption::new("UNCOMMITTED", "Uncommitted")
                .group("Workspace")
                .detail("staged + unstaged")
                .icon(Icon::Edit)
                .selected(current_val == "UNCOMMITTED"),
        );
        options.push(
            PickerOption::new("WORKDIR", "Working tree")
                .group("Workspace")
                .detail(if dirty_count > 0 {
                    format!("{dirty_count} uncommitted")
                } else {
                    "clean".into()
                })
                .icon(Icon::Edit)
                .selected(current_val.is_empty() || current_val == "WORKDIR")
                .keywords(["workdir", "uncommitted", "dirty", "working"]),
        );
        options.push(
            PickerOption::new("STAGED", "Staged changes")
                .group("Workspace")
                .detail(if staged_count > 0 {
                    format!("{staged_count} staged")
                } else {
                    "index".into()
                })
                .icon(Icon::Check)
                .selected(current_val == "STAGED")
                .keywords(["staged", "index", "cached"]),
        );
        options.push(
            PickerOption::new("UNSTAGED", "Unstaged changes")
                .group("Workspace")
                .detail("working tree")
                .icon(Icon::Compare)
                .selected(current_val == "UNSTAGED")
                .keywords(["unstaged", "modified"]),
        );
    } else {
        options.push(
            PickerOption::new("HEAD", "HEAD")
                .group("Shortcuts")
                .detail("current commit")
                .icon(Icon::History)
                .mono(true)
                .selected(current_val == "HEAD")
                .keywords(["head", "latest"]),
        );
        options.push(
            PickerOption::new("HEAD~1", "HEAD~1")
                .group("Shortcuts")
                .detail("previous commit")
                .icon(Icon::History)
                .mono(true)
                .selected(current_val == "HEAD~1")
                .keywords(["parent", "last", "previous"]),
        );
    }

    for br in branches {
        let name = br["name"].as_str().unwrap_or("").to_owned();
        if name.is_empty() {
            continue;
        }
        let is_remote = br["remote"].as_bool().unwrap_or(false) || name.starts_with("origin/");
        let is_cur = br["is_current"].as_bool().unwrap_or(false);
        let is_def = br["is_default"].as_bool().unwrap_or(false);
        let short = br["short_sha"].as_str().unwrap_or("").to_owned();
        let ahead = br["ahead"].as_u64().unwrap_or(0);
        let behind = br["behind"].as_u64().unwrap_or(0);
        let sync_tag = match (ahead > 0, behind > 0) {
            (true, true) => format!("↑{ahead} ↓{behind}"),
            (true, false) => format!("↑{ahead}"),
            (false, true) => format!("↓{behind}"),
            (false, false) => String::new(),
        };
        let detail = if !sync_tag.is_empty() {
            sync_tag
        } else if is_cur {
            "current".to_owned()
        } else if is_def {
            "default".to_owned()
        } else {
            short.clone()
        };
        let grp = if is_remote {
            "Remote Branches"
        } else {
            "Branches"
        };
        options.push(
            PickerOption::new(name.clone(), name.clone())
                .group(grp)
                .detail(detail)
                .icon(Icon::Branch)
                .mono(true)
                .selected(current_val == name)
                .keywords([short]),
        );
    }

    for tg in tags {
        let name = tg["name"].as_str().unwrap_or("").to_owned();
        if name.is_empty() {
            continue;
        }
        let short = tg["short_sha"].as_str().unwrap_or("").to_owned();
        options.push(
            PickerOption::new(name.clone(), name.clone())
                .group("Tags")
                .detail(short.clone())
                .mono(true)
                .selected(current_val == name)
                .keywords([short, "tag".into()]),
        );
    }

    for cm in commits {
        let sha = cm["sha"].as_str().unwrap_or("").to_owned();
        if sha.is_empty() {
            continue;
        }
        let short = cm["short_sha"].as_str().unwrap_or("").to_owned();
        let subj: String = cm["subject"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(40)
            .collect();
        options.push(
            PickerOption::new(sha.clone(), format!("{short} · {subj}"))
                .group("Recent Commits")
                .icon(Icon::History)
                .selected(current_val == sha || (!short.is_empty() && current_val == short))
                .keywords([sha, short]),
        );
    }

    options
}

fn render_ref_picker(ui: Ui, is_base: bool) -> impl IntoElement {
    let st = ui.review_state;

    let trigger = canban_ui_kit::press_target()
        .h(px(28.))
        .px_2p5()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(BORDER)
        .bg(INSET)
        .hover(|s| s.bg(CARD_HOVER))
        .flex_row()
        .items_center()
        .gap_1p5()
        .accessibility_label(if is_base {
            "Choose or type Base (target) branch or ref"
        } else {
            "Choose or type Compare (source) branch, ref, or Working tree"
        })
        .child(icon(Icon::Branch).w(px(11.5)).h(px(11.5)))
        .child(
            label(move || {
                if is_base {
                    let b = st.base_ref.get();
                    if b.chars().count() > 18 && !b.contains('/') {
                        let s: String = b.chars().take(10).collect();
                        format!("{s}…")
                    } else if b.is_empty() {
                        "main".into()
                    } else {
                        b
                    }
                } else {
                    let c = st.compare_ref.get();
                    match c.as_str() {
                        "UNCOMMITTED" => "Uncommitted".into(),
                        "" | "WORKDIR" => "Working tree".into(),
                        "STAGED" => "Staged".into(),
                        "UNSTAGED" => "Unstaged".into(),
                        other if other.chars().count() > 18 && !other.contains('/') => {
                            let s: String = other.chars().take(10).collect();
                            format!("{s}…")
                        }
                        other => other.to_owned(),
                    }
                }
            })
            .font_family(MONO_FONT)
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM),
        )
        .child(icon(Icon::ChevronDown).w(px(11.)).h(px(11.)));

    search_popover(
        trigger,
        move || {
            let refs = st.refs_data.get();
            let cur = if is_base {
                st.base_ref.get()
            } else {
                st.compare_ref.get()
            };
            build_git_ref_options(&refs, is_base, &cur)
        },
        move |chosen| {
            if is_base {
                let compare = st.compare_ref.get_untracked();
                select_git_pair(
                    ui,
                    chosen,
                    if compare == "UNCOMMITTED" {
                        "WORKDIR".into()
                    } else {
                        compare
                    },
                );
            } else {
                let cur_b = st.base_ref.get_untracked();
                let b = match chosen.as_str() {
                    "UNCOMMITTED" => "HEAD".into(),
                    "WORKDIR" | "STAGED" => {
                        if cur_b.is_empty() {
                            "HEAD".into()
                        } else {
                            cur_b
                        }
                    }
                    "UNSTAGED" => "HEAD".into(),
                    other => {
                        if cur_b.is_empty() {
                            if other.len() >= 7 && other.chars().all(|ch| ch.is_ascii_hexdigit()) {
                                format!("{other}~1")
                            } else {
                                st.refs_data.with_untracked(|v| {
                                    v["default_branch"]
                                        .as_str()
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or("main")
                                        .to_owned()
                                })
                            }
                        } else {
                            cur_b
                        }
                    }
                };
                select_git_pair(ui, b, chosen);
            }
        },
    )
    .placeholder(if is_base {
        "Search or type base branch, tag, SHA, HEAD~2…"
    } else {
        "Search or type compare branch, SHA, WORKDIR…"
    })
    .empty_text("No matching Git refs")
    .custom_option(|q| {
        Some(
            PickerOption::new(q.to_owned(), format!("Use ref \"{q}\""))
                .detail("custom")
                .icon(Icon::Branch)
                .mono(true),
        )
    })
    .width(320.)
    .list_height(300.)
}

pub fn render_top_bar(ui: Ui) -> impl IntoElement {
    let st = ui.review_state;
    let project_anchor = element_ref();
    view()
        .border_b_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .child(
            view()
                .min_h(px(42.))
                .px_3()
                .py_1p5()
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
                        .min_w(px(0.))
                        .child(chrome_button(
                            Icon::Sidebar,
                            "Toggle Git sidebar",
                            move |_| st.sidebar_open.update(|v| *v = !*v),
                        ))
                        .child(
                            canban_ui_kit::press_target()
                                .element_ref(&project_anchor)
                                .h(px(28.))
                                .px_2p5()
                                .rounded(px(RADIUS))
                                .border_1()
                                .border_color(BORDER)
                                .bg(INSET)
                                .hover(|s| s.bg(CARD_HOVER))
                                .flex_row()
                                .items_center()
                                .gap_1p5()
                                .child(icon(Icon::Projects).w(px(12.)).h(px(12.)))
                                .child(
                                    label(move || {
                                        ui.current_project()
                                            .map(|p| p.name)
                                            .unwrap_or_else(|| "Select Project".into())
                                    })
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM),
                                )
                                .child(icon(Icon::ChevronDown).w(px(11.)).h(px(11.)))
                                .on_press(move |_: &()| {
                                    let projects =
                                        ui.snapshot.with_untracked(|s| s.projects.clone());
                                    if projects.is_empty() {
                                        return;
                                    }
                                    ui.spawn(async move {
                                        let items: Vec<MenuItem> = projects
                                            .into_iter()
                                            .map(|p| {
                                                let pid = p.id.clone();
                                                let is_cur = ui.project.get_untracked() == pid;
                                                MenuItem::new(format!(
                                                    "{}{}",
                                                    if is_cur { "✓ " } else { "" },
                                                    p.name
                                                ))
                                                .on_select(move || {
                                                    ui.select_project(pid.clone());
                                                    open_compare(ui);
                                                })
                                            })
                                            .collect();
                                        if let Some(bounds) = project_anchor.measure().await {
                                            popup_menu_at(
                                                items,
                                                point(
                                                    bounds.page.x,
                                                    bounds.page.y + bounds.frame.size.height,
                                                ),
                                            )
                                            .await;
                                        } else {
                                            popup_menu(items).await;
                                        }
                                    });
                                }),
                        )
                        .child(when(
                            move || st.mode.get() != "github_pr",
                            move || git_ops::render_checkout_picker(ui),
                        ))
                        .child(when(
                            move || st.mode.get() != "github_pr",
                            move || {
                                compact_action("Compare…", Variant::Quiet, move |_| {
                                    super::toggle_comparison(ui)
                                })
                            },
                        )),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .child(when(
                            move || {
                                st.mode.get() != "github_pr" || !st.checkout_cwd.get().is_empty()
                            },
                            move || git_ops::render_sync_controls(ui),
                        ))
                        .child(render_refresh(ui))
                        .child(dynamic(move || {
                            if let Some(card) =
                                super::related_card(ui).filter(|_| st.mode.get() != "github_pr")
                            {
                                compact_action(
                                    format!("#{} ↗", card.number),
                                    Variant::Quiet,
                                    move |_| super::open_related_card(ui),
                                )
                                .into_element()
                            } else {
                                view().into_element()
                            }
                        })),
                ),
        )
        .child(when(
            move || st.compare_open.get() && st.mode.get() != "github_pr",
            move || {
                view()
                    .px_3()
                    .py_2()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(muted("Base"))
                    .child(render_ref_picker(ui, true))
                    .child(muted("→ Compare"))
                    .child(render_ref_picker(ui, false))
                    .child(
                        compact_action("⇄", Variant::Quiet, move |_| {
                            let base = st.base_ref.get_untracked();
                            let compare = st.compare_ref.get_untracked();
                            if !matches!(
                                compare.as_str(),
                                "UNCOMMITTED" | "WORKDIR" | "STAGED" | "UNSTAGED"
                            ) {
                                select_git_pair(ui, compare, base);
                            }
                        })
                        .disabled(move || {
                            matches!(
                                st.compare_ref.get().as_str(),
                                "UNCOMMITTED" | "WORKDIR" | "STAGED" | "UNSTAGED"
                            )
                        }),
                    )
                    .child(
                        compact_action(
                            move || if st.three_dot.get() { "A...B" } else { "A..B" },
                            Variant::Quiet,
                            move |_| {
                                st.three_dot.update(|v| *v = !*v);
                                refresh_active_diff(ui, false);
                            },
                        )
                        .disabled(move || {
                            matches!(
                                st.compare_ref.get().as_str(),
                                "UNCOMMITTED" | "STAGED" | "UNSTAGED"
                            )
                        }),
                    )
                    .child(chrome_button(
                        Icon::Close,
                        "Close Git comparison controls",
                        move |_| st.compare_open.set(false),
                    ))
            },
        ))
}

pub fn render_card_delivery_banner(ui: Ui) -> AnyElement {
    let st = ui.review_state;
    let Some(c) = ui.current_card() else {
        // Standalone remote GitHub PR inspection (opened from GitHub browser without a Card)
        if st.mode.get() == "github_pr"
            && let Some(pr) = st.pr_target.get()
        {
            let repo = pr["repository"].as_str().unwrap_or("").to_owned();
            let num = pr["number"].as_u64().unwrap_or(0);
            let url = pr["url"].as_str().unwrap_or("").to_owned();
            return view()
                .px_3()
                .py_1p5()
                .border_b_1()
                .border_color(BORDER)
                .bg(INSET)
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(icon(Icon::GitHub).w(px(13.)).h(px(13.)))
                        .child(
                            label(format!("{repo} · PR #{num}")).font_weight(FontWeight::SEMIBOLD),
                        ),
                )
                .child(
                    compact_action("GitHub ↗", Variant::Quiet, move |_| {
                        crate::github::open_url(ui, url.clone());
                    })
                    .disabled(pr["url"].as_str().unwrap_or("").is_empty()),
                )
                .into_element();
        }
        return view().into_element();
    };
    if st.mode.get() == "git" {
        return view().into_element();
    }

    if let Some(d) = c.delivery.clone()
        && ui.review.get().is_none()
    {
        let archive_card = c.clone();
        let archived = c.archived_at.is_some();
        let short = &d.commit[..d.commit.len().min(10)];
        return view()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(SUCCESS)
            .bg(SUCCESS_SURFACE)
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
                    .child(
                        label(format!(
                            "Delivered · merged into {} ({short})",
                            d.target_branch
                        ))
                        .text_color(SUCCESS)
                        .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(muted(format!("#{} {}", c.number, c.title))),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        compact_action("Archive Card", Variant::Primary, move |_| {
                            crate::card_actions::apply(
                                ui,
                                archive_card.clone(),
                                crate::card_actions::Action::Archive,
                            );
                        })
                        .disabled(move || ui.is_method_pending("card.archive") || archived),
                    )
                    .child(chrome_button(Icon::Refresh, "Refresh diff", move |_| {
                        refresh_active_diff(ui, true)
                    })),
            )
            .into_element();
    }

    let default_commit_msg = format!("Canban #{}: {}", c.number, c.title);
    let message_key = format!("review-msg:{}", c.id);
    let msg_draft = crate::drafts::document(
        ui,
        message_key,
        crate::drafts::Draft {
            body: default_commit_msg.clone(),
            ..Default::default()
        },
    );
    let commit_msg = signal(msg_draft.get_untracked().body);
    effect(move || {
        let m = commit_msg.get();
        msg_draft.update(|d| d.body = m);
    });

    let card_for_return = c.clone();
    let has_worktree = c.worktree.is_some();
    let has_local = c.worktree.is_some() || c.branch.is_some();
    let pr_target = st.pr_target.get();
    let is_remote = st.mode.get() == "github_pr";

    // Inline PR summary & actions (replaces the bulky nested `pr_panel` box)
    let mut left_info = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .child(badge(format!("#{}", c.number)))
        .child(
            label(c.title.clone())
                .font_weight(FontWeight::SEMIBOLD)
                .max_w(px(280.))
                .truncate(),
        )
        .child(status(c.stage.clone(), Tone::Neutral));

    if let Some(pr) = &c.pull_request {
        if let Some(pr_view) = ui.snapshot.with(|s| {
            ui.index
                .with(|idx| crate::components::pr_status::from_card(s, idx, &c))
        }) && let Some(pr_pill) = crate::components::pr_status::pill(ui, &pr_view, false)
        {
            left_info = left_info.child(pr_pill);
        } else {
            let pr_summary: String = if pr.state == "not_published" {
                "No PR yet".into()
            } else {
                "PR sync needed".into()
            };
            left_info = left_info.child(status(pr_summary, Tone::Neutral));
            if !pr.url.is_empty() {
                let url = pr.url.clone();
                left_info =
                    left_info.child(compact_action("GitHub ↗", Variant::Quiet, move |_| {
                        crate::github::open_url(ui, url.clone());
                    }));
            }
        }
        let refresh_cid = c.id.clone();
        left_info = left_info.child(chrome_button(
            Icon::Refresh,
            "Refresh PR status",
            move |_| {
                dispatch(ui, "github.pr.refresh", json!({ "card_id": refresh_cid }));
            },
        ));
    } else if let Some(prt) = &pr_target {
        let num = prt["number"].as_u64().unwrap_or(0);
        let url = prt["url"].as_str().unwrap_or("").to_owned();
        if num > 0 {
            left_info = left_info.child(status(format!("PR #{num}"), Tone::Info));
        }
        if !url.is_empty() {
            left_info = left_info.child(compact_action("GitHub ↗", Variant::Quiet, move |_| {
                crate::github::open_url(ui, url.clone());
            }));
        }
    }

    let mut right_controls = view().flex_row().flex_wrap().items_center().gap_1p5();

    if c.active() && !c.resolved() {
        let active_review = ui.snapshot.with(|s| super::active_review_run(s, &c.id));
        if let Some((prof, started_at)) = active_review {
            let cid = c.id.clone();
            let now_ms = ui.runtime_clock.get();
            let elapsed = started_at
                .map(|t| {
                    format!(
                        " · {}",
                        crate::components::activity::compact_duration(now_ms.saturating_sub(t))
                    )
                })
                .unwrap_or_default();
            right_controls = right_controls.child(
                canban_ui_kit::press_target()
                    .h(px(26.))
                    .px_2()
                    .rounded(px(13.))
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .bg(SELECTED)
                    .hover(|s| s.bg(CARD_HOVER))
                    .on_press(move |_: &()| crate::terminals::open_card_quick(ui, &cid))
                    .child(live_dot(ACCENT))
                    .child(
                        label(format!("@{prof} reviewing{elapsed}"))
                            .text_size(px(11.5))
                            .text_color(ACCENT),
                    ),
            );
        } else {
            let cid_quick = c.id.clone();
            let default_prof = crate::sessions::default_review_profile(ui);
            right_controls = right_controls.child(
                compact_action(
                    move || {
                        if ui.is_method_pending("review.run_agent") {
                            format!("Starting @{default_prof}…")
                        } else {
                            format!("Ask @{default_prof}")
                        }
                    },
                    Variant::Secondary,
                    move |_| super::start_agent_review(ui, &cid_quick, None, None),
                )
                .disabled(move || ui.is_method_pending("review.run_agent")),
            );
        }
    }

    // Unified Local / PR #N switcher when both local branch/worktree and PR exist
    if let Some(prt) = &pr_target
        && has_local
    {
        let pr_num = prt["number"].as_u64().unwrap_or(0);
        right_controls = right_controls.child(
            view()
                .flex_row()
                .items_center()
                .gap_0p5()
                .p_0p5()
                .rounded(px(6.))
                .bg(SURFACE)
                .border_1()
                .border_color(BORDER)
                .child(compact_action(
                    "Local",
                    if !is_remote {
                        Variant::Primary
                    } else {
                        Variant::Quiet
                    },
                    move |_| {
                        st.mode.set("card_local".into());
                        refresh_active_diff(ui, false);
                    },
                ))
                .child(compact_action(
                    format!("PR #{pr_num}"),
                    if is_remote {
                        Variant::Primary
                    } else {
                        Variant::Quiet
                    },
                    move |_| {
                        st.mode.set("github_pr".into());
                        refresh_active_diff(ui, false);
                    },
                )),
        );
    }

    if card_for_return.stage != "develop" {
        let cid_outer = c.id.clone();
        let fallback_rev = c.revision;
        right_controls = right_controls.child(compact_action(
            "Return to Develop",
            Variant::Secondary,
            move |_| {
                let cid = cid_outer.clone();
                let rev = ui
                    .snapshot
                    .with_untracked(|s| s.cards.iter().find(|c| c.id == cid).map(|c| c.revision))
                    .unwrap_or(fallback_rev);
                dispatch_then(
                    ui,
                    "card.move",
                    json!({"card_id": cid, "revision": rev, "stage": "develop"}),
                    move |_| {
                        ui.review.set(None);
                        ui.page.set("card".into());
                    },
                );
            },
        ));
    }

    // Remote PR merge button when inspecting an open published delivery PR
    if let Some(pr) = &c.pull_request
        && pr.number > 0
        && pr.state == "open"
        && !pr.merged
        && (is_remote || !has_worktree)
    {
        let merge_cid = c.id.clone();
        let fallback_rev = c.revision;
        let head = pr.head.clone();
        right_controls = right_controls.child(
            compact_action("Merge PR", Variant::Primary, move |_| {
                let rev = ui
                    .snapshot
                    .with_untracked(|s| {
                        s.cards
                            .iter()
                            .find(|c| c.id == merge_cid)
                            .map(|c| c.revision)
                    })
                    .unwrap_or(fallback_rev);
                dispatch(
                    ui,
                    "github.pr.merge",
                    json!({"card_id": merge_cid, "revision": rev, "head": head}),
                );
            })
            .disabled(move || ui.pending.get()),
        );
    }

    if has_worktree && !is_remote {
        let default_msg_check = default_commit_msg.clone();
        right_controls = right_controls
            .child(crate::github::choice(
                ui,
                st.delivery_mode,
                vec![("local", "Local merge"), ("github_pr", "GitHub PR")],
            ))
            .child(when(
                move || st.delivery_mode.get() == "local",
                move || {
                    crate::github::choice(
                        ui,
                        st.delivery_strategy,
                        vec![
                            ("rebase", "Rebase"),
                            ("squash", "Squash"),
                            ("merge", "Merge commit"),
                        ],
                    )
                },
            ))
            .child(
                view()
                    .tooltip(hint_tooltip("Customize delivery commit message"))
                    .child(chrome_button(
                        Icon::Edit,
                        "Customize delivery commit message",
                        move |_| st.delivery_customize.update(|v| *v = !*v),
                    )),
            )
            .child(
                compact_action(
                    move || {
                        if st.delivery_mode.get() == "github_pr" {
                            "Publish PR"
                        } else {
                            "Merge"
                        }
                    },
                    Variant::Primary,
                    move |_| {
                        if let Some(p) = ui.review.get_untracked() {
                            let source = ui.location();
                            let msg = commit_msg.get_untracked();
                            let customized = st.delivery_customize.get_untracked()
                                && msg.trim() != default_msg_check;
                            dispatch_then(
                                ui,
                                if st.delivery_mode.get_untracked() == "github_pr" {
                                    "github.pr.publish"
                                } else {
                                    "review.merge"
                                },
                                json!({
                                    "card_id": p.card_id,
                                    "revision": p.revision,
                                    "token": p.token,
                                    "target_branch": p.target_branch,
                                    "strategy": st.delivery_strategy.get_untracked(),
                                    "message": msg,
                                    "message_customized": customized,
                                }),
                                move |_| {
                                    if ui.location() == source {
                                        ui.review.set(None);
                                    }
                                },
                            );
                        }
                    },
                )
                .disabled(move || {
                    ui.is_method_pending("review.merge")
                        || ui.is_method_pending("github.pr.publish")
                        || ui.review.get().is_none_or(|p| {
                            !p.merge_allowed
                                || ui.current_card().is_none_or(|cur| {
                                    cur.revision != p.revision || cur.id != p.card_id
                                })
                        })
                }),
            );
    }

    view()
        .px_3()
        .py_1p5()
        .border_b_1()
        .border_color(BORDER)
        .bg(INSET)
        .gap_1p5()
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(left_info)
                .child(right_controls),
        )
        .child(when(
            move || st.delivery_customize.get(),
            move || {
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(muted(move || match st.delivery_strategy.get().as_str() {
                        "squash" => "Squash message:",
                        "merge" => "Merge message:",
                        _ => "Commit message:",
                    }))
                    .child(field(commit_msg, "Commit message").flex_1())
            },
        ))
        .child(dynamic(move || {
            if let Some(p) = ui.review.get() {
                let mut col = view().flex_col().gap_1p5();
                let mut any = false;
                if let Some(reason) = p.reason {
                    any = true;
                    col = col.child(notice(reason, Tone::Attention));
                }
                let live_warning = ui
                    .snapshot
                    .with(|s| canban_types::agent_activity_warning(&p.card_id, &s.terminals));
                if let Some(warning) = live_warning {
                    any = true;
                    let cid = p.card_id.clone();
                    col = col.child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(notice(warning, Tone::Info))
                            .child(compact_action("Terminals ↗", Variant::Quiet, move |_| {
                                crate::terminals::open_card(ui, &cid);
                            })),
                    );
                }
                if any {
                    return col.into_element();
                }
            }
            view().into_element()
        }))
        .into_element()
}
