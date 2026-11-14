use super::{
    State, git_ops, load_refs, open_card, open_compare, refresh_active_diff, select_git_pair,
};
use crate::presenter::*;
use canban_types::Card;
use canban_ui_kit::tokens::*;
use canban_ui_kit::*;
use nsh_core::element::desktop::{MenuItem, popup_menu, popup_menu_at};
use nsh_core::prelude::*;
use serde_json::{Value, json};

pub fn render_left_sidebar(ui: Ui) -> impl IntoElement {
    let st = ui.review_state;
    view()
        .w(px(244.))
        .flex_shrink_0()
        .min_h(px(0.))
        .border_r_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .child(
            view()
                .h(px(44.))
                .px_3()
                .flex_row()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .child(icon(Icon::Compare).w(px(14.)).h(px(14.)))
                        .child(label("Git").font_weight(FontWeight::SEMIBOLD)),
                )
                .child(chrome_button(
                    Icon::Refresh,
                    "Refresh Git refs, log & diff",
                    move |_| {
                        load_refs(ui);
                        let br = st.log_branch.get_untracked();
                        git_ops::load_branch_log(ui, br, false);
                        refresh_active_diff(ui, true);
                    },
                )),
        )
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .child(dynamic(move || render_sidebar_sections(ui, st))),
        )
}

fn render_sidebar_sections(ui: Ui, st: State) -> AnyElement {
    let refs = st.refs_data.get();
    let mode = st.mode.get();
    let cur_base = st.base_ref.get();
    let cur_cmp = st.compare_ref.get();
    let cur_card = ui.card.get();

    let current_branch = refs["current_branch"].as_str().unwrap_or("").to_owned();
    let default_branch = refs["default_branch"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("main")
        .to_owned();
    let dirty_count = refs["dirty_count"].as_u64().unwrap_or(0);
    let staged_count = refs["staged_count"].as_u64().unwrap_or(0);
    let commits = refs["commits"].as_array().cloned().unwrap_or_default();

    // 1. WORKSPACE (Working tree, Staged changes, Last commit)
    let mut workspace = section_group("Workspace");

    let workdir_selected = mode == "git" && cur_cmp == "WORKDIR";
    workspace = workspace.child(sidebar_row(
        "Working tree",
        if dirty_count > 0 {
            format!("{dirty_count} uncommitted")
        } else {
            "clean".into()
        },
        workdir_selected,
        move || select_git_pair(ui, "HEAD", "WORKDIR"),
    ));

    if staged_count > 0 {
        let staged_selected = mode == "git" && cur_cmp == "STAGED";
        workspace = workspace.child(sidebar_row(
            "Staged changes",
            format!("{staged_count} staged"),
            staged_selected,
            move || select_git_pair(ui, "HEAD", "STAGED"),
        ));
    }

    let last_commit_selected = mode == "git" && cur_base == "HEAD~1" && cur_cmp == "HEAD";
    workspace = workspace.child(sidebar_row(
        "Last commit",
        "HEAD~1 → HEAD".into(),
        last_commit_selected,
        move || select_git_pair(ui, "HEAD~1", "HEAD"),
    ));

    // 2. CARDS & PRs (searchable + selectable)
    let active_pid = ui.project.get();
    let reviewable_cards: Vec<Card> = ui.snapshot.with(|s| {
        s.cards
            .iter()
            .filter(|c| {
                c.active()
                    && (active_pid.is_empty() || c.project_id == active_pid)
                    && (c.worktree.is_some()
                        || c.branch.is_some()
                        || c.is_github_pr()
                        || c.pull_request.is_some()
                        || c.stage == "review")
            })
            .cloned()
            .collect()
    });

    let cards_section = if reviewable_cards.is_empty() {
        None
    } else {
        let total_cards = reviewable_cards.len();
        let cq = st.card_query.get();
        let trimmed_cq = cq.trim();
        let filtered_cards: Vec<Card> = if trimmed_cq.is_empty() {
            reviewable_cards
        } else {
            let mut scored: Vec<(usize, usize, Card)> = reviewable_cards
                .into_iter()
                .enumerate()
                .filter_map(|(idx, c)| {
                    let br = c
                        .branch
                        .clone()
                        .or_else(|| c.pull_request.as_ref().map(|p| p.head_branch.clone()))
                        .unwrap_or_default();
                    let pr_num = c
                        .pull_request
                        .as_ref()
                        .map(|p| p.number)
                        .or_else(|| c.github_issue().map(|g| g.number))
                        .unwrap_or(0);
                    let mut fields = vec![
                        format!("#{} {}", c.number, c.title),
                        c.number.to_string(),
                        c.stage.clone(),
                    ];
                    if !br.is_empty() {
                        fields.push(br);
                    }
                    if pr_num > 0 {
                        fields.push(format!("PR #{pr_num}"));
                        fields.push(pr_num.to_string());
                    }
                    canban_ui_kit::search::score(&fields, trimmed_cq).map(|s| (s, idx, c))
                })
                .collect();
            scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            scored.into_iter().map(|(_, _, c)| c).collect()
        };

        let mut grp = section_group("Cards & PRs").count(total_cards);
        if total_cards > 3 || !trimmed_cq.is_empty() {
            grp = grp.child(
                view().px_1().pb_1().child(
                    field(st.card_query, "Filter #card, PR, or branch…")
                        .min_h(px(26.))
                        .px_2()
                        .py_1()
                        .text_size(px(11.5))
                        .label("Filter Cards & PRs"),
                ),
            );
        }
        if filtered_cards.is_empty() {
            grp = grp.child(
                view()
                    .px_2()
                    .py_1()
                    .child(muted("No matching Cards or PRs").text_size(px(11.))),
            );
        } else {
            for c in filtered_cards {
                let cid = c.id.clone();
                let is_sel = cur_card == c.id && mode != "git";
                let is_pr = c.is_github_pr() || c.pull_request.is_some();
                let prefix = if is_pr {
                    format!("PR · #{}", c.number)
                } else {
                    format!("#{}", c.number)
                };
                let br_hint = c
                    .branch
                    .as_deref()
                    .or_else(|| c.pull_request.as_ref().map(|p| p.head_branch.as_str()))
                    .unwrap_or("");
                let meta = if !br_hint.is_empty() {
                    if br_hint.chars().count() > 14 {
                        let s: String = br_hint.chars().take(12).collect();
                        format!("{s}…")
                    } else {
                        br_hint.to_owned()
                    }
                } else {
                    c.stage.clone()
                };
                grp = grp.child(sidebar_row(
                    format!("{prefix} {}", c.title),
                    meta,
                    is_sel,
                    move || open_card(ui, &cid),
                ));
            }
        }
        Some(grp)
    };

    // 3. BRANCH COMMITS (with searchable + typeable branch picker, filter, and pagination)
    let log_branch = {
        let lb = st.log_branch.get();
        if lb.is_empty() {
            if !current_branch.is_empty() {
                current_branch.clone()
            } else {
                default_branch.clone()
            }
        } else {
            lb
        }
    };
    let loaded_log = st.log_commits.get();
    let display_commits = if !loaded_log.is_empty() || !st.log_query.get().trim().is_empty() {
        loaded_log
    } else {
        commits
    };
    let has_more = st.log_has_more.get();
    let log_loading = st.log_loading.get();

    let log_br_trigger = canban_ui_kit::press_target()
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
            label(if log_branch.chars().count() > 16 {
                let s: String = log_branch.chars().take(14).collect();
                format!("{s}…")
            } else {
                log_branch.clone()
            })
            .font_family("Menlo")
            .text_size(px(10.5)),
        )
        .child(icon(Icon::ChevronDown).w(px(10.)).h(px(10.)));

    let log_br_for_opts = log_branch.clone();
    let log_br_picker = search_popover(
        log_br_trigger,
        move || {
            let r = st.refs_data.get();
            build_log_ref_options(&r, &log_br_for_opts)
        },
        move |chosen| {
            git_ops::load_branch_log(ui, chosen, false);
        },
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

    let mut commits_grp = view().gap_1p5();
    commits_grp = commits_grp.child(
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
            .child(log_br_picker),
    );

    let filter_br = log_branch.clone();
    commits_grp = commits_grp.child(
        view().px_1().child(
            field(st.log_query, "Filter commits or SHA…")
                .min_h(px(28.))
                .px_2()
                .py_1()
                .text_size(px(11.5))
                .on_change({
                    let br = filter_br.clone();
                    move |_: &str| {
                        git_ops::load_branch_log(ui, br.clone(), false);
                    }
                })
                .on_submit(move |_: &str| {
                    git_ops::load_branch_log(ui, filter_br.clone(), false);
                }),
        ),
    );

    if display_commits.is_empty() {
        commits_grp = commits_grp.child(
            view().px_2().py_1p5().child(
                muted(if log_loading {
                    "Loading commits…"
                } else {
                    "No matching commits"
                })
                .text_size(px(11.)),
            ),
        );
    } else {
        let mut list_rows = view().gap_0p5();
        for cm in display_commits {
            let sha = cm["sha"].as_str().unwrap_or("").to_owned();
            let short = cm["short_sha"].as_str().unwrap_or("").to_owned();
            let subj = cm["subject"].as_str().unwrap_or("").to_owned();
            let rel = cm["relative_time"].as_str().unwrap_or("").to_owned();
            if sha.is_empty() {
                continue;
            }
            let is_sel = mode == "git" && cur_cmp == sha;
            let click_sha = sha.clone();
            list_rows = list_rows.child(sidebar_commit_row(short, subj, rel, is_sel, move || {
                git_ops::select_single_commit(ui, click_sha.clone(), 0, false);
            }));
        }
        if has_more {
            let more_br = log_branch;
            list_rows = list_rows.child(
                view().pt_1().px_1().child(
                    compact_action(
                        if log_loading {
                            "Loading…"
                        } else {
                            "Load more commits…"
                        },
                        Variant::Quiet,
                        move |_| git_ops::load_branch_log(ui, more_br.clone(), true),
                    )
                    .disabled(move || st.log_loading.get()),
                ),
            );
        }
        commits_grp = commits_grp.child(list_rows);
    }

    let mut container = view().p_2p5().gap_4().child(workspace);
    if let Some(cs) = cards_section {
        container = container.child(cs);
    }
    container = container.child(commits_grp);
    container.into_element()
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
                        .font_family("Menlo")
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
            .font_family("Menlo")
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
                st.base_ref.set(chosen);
                if st.mode.get_untracked() == "github_pr" {
                    st.mode.set("git".into());
                }
                refresh_active_diff(ui, false);
            } else {
                let cur_b = st.base_ref.get_untracked();
                let b = match chosen.as_str() {
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
                        .flex_wrap()
                        .items_center()
                        .gap_1p5()
                        .child(chrome_button(
                            Icon::Sidebar,
                            "Toggle Git sidebar",
                            move |_| st.sidebar_open.update(|v| *v = !*v),
                        ))
                        .child(when(
                            move || !ui.card.get().is_empty(),
                            move || {
                                chrome_button(Icon::Back, "Back to Card", move |_| {
                                    ui.page.set("card".into());
                                })
                            },
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
                        .child(git_ops::render_checkout_picker(ui))
                        .child(render_ref_picker(ui, true))
                        .child(muted("←").text_size(px(12.)))
                        .child(render_ref_picker(ui, false))
                        .child(when(
                            move || {
                                st.mode.get() == "git"
                                    && !matches!(
                                        st.compare_ref.get().as_str(),
                                        "WORKDIR" | "STAGED" | "UNSTAGED"
                                    )
                            },
                            move || {
                                view()
                                    .tooltip(hint_tooltip("Swap Base and Compare refs"))
                                    .child(compact_action("⇄", Variant::Quiet, move |_| {
                                        let b = st.base_ref.get_untracked();
                                        let c = st.compare_ref.get_untracked();
                                        select_git_pair(ui, c, b);
                                    }))
                            },
                        ))
                        .child(when(
                            move || st.mode.get() != "github_pr",
                            move || {
                                view()
                                    .tooltip(hint_tooltip(
                                        "Toggle merge-base (A...B) vs direct (A..B) diff",
                                    ))
                                    .child(compact_action(
                                        move || {
                                            if st.three_dot.get() { "A...B" } else { "A..B" }
                                        },
                                        Variant::Quiet,
                                        move |_| {
                                            st.three_dot.update(|v| *v = !*v);
                                            refresh_active_diff(ui, false);
                                        },
                                    ))
                            },
                        )),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_1p5()
                        .child(git_ops::render_sync_controls(ui))
                        .child(when(
                            move || !ui.card.get().is_empty() && !ui.narrow.get(),
                            move || {
                                compact_action(
                                    move || {
                                        if st.card_drawer_open.get() {
                                            "Hide Card"
                                        } else {
                                            "Card"
                                        }
                                    },
                                    Variant::Secondary,
                                    move |_| st.card_drawer_open.update(|v| *v = !*v),
                                )
                            },
                        ))
                        .child(chrome_button(Icon::Refresh, "Refresh diff", move |_| {
                            refresh_active_diff(ui, true);
                        })),
                ),
        )
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
                            label(format!("{repo} · PR #{num}"))
                                .font_weight(FontWeight::SEMIBOLD),
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
        let state_label = if pr.merged {
            "merged".to_owned()
        } else {
            pr.state.clone()
        };
        let pr_summary = if pr.number > 0 {
            if pr.checks.is_empty() {
                format!("PR #{} · {state_label}", pr.number)
            } else {
                format!("PR #{} · {state_label} · {}", pr.number, pr.checks)
            }
        } else if pr.state == "not_published" {
            "No PR yet".into()
        } else {
            "PR sync needed".into()
        };
        let pr_tone = if pr.merged {
            Tone::Success
        } else if pr.state == "open" {
            Tone::Info
        } else {
            Tone::Neutral
        };
        left_info = left_info.child(status(pr_summary, pr_tone));
        if !pr.url.is_empty() {
            let url = pr.url.clone();
            left_info = left_info.child(compact_action("GitHub ↗", Variant::Quiet, move |_| {
                crate::github::open_url(ui, url.clone());
            }));
        }
        let refresh_cid = c.id.clone();
        left_info = left_info.child(chrome_button(Icon::Refresh, "Refresh PR status", move |_| {
            dispatch(ui, "github.pr.refresh", json!({ "card_id": refresh_cid }));
        }));
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
        let rev = c.revision;
        right_controls = right_controls.child(compact_action(
            "Return to Develop",
            Variant::Secondary,
            move |_| {
                let cid = cid_outer.clone();
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
        let rev = c.revision;
        let head = pr.head.clone();
        right_controls = right_controls.child(
            compact_action("Merge PR", Variant::Primary, move |_| {
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
                let live_warning = ui.snapshot.with(|s| {
                    canban_types::agent_activity_warning(&p.card_id, &s.terminals)
                });
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
