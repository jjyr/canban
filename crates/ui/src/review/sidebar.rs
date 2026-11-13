use super::{State, load_refs, open_card, open_compare, refresh_active_diff, select_git_pair};
use crate::presenter::*;
use canban_types::Card;
use canban_ui_kit::tokens::*;
use canban_ui_kit::*;
use nsh_core::element::desktop::{MenuItem, popup_menu, popup_menu_at};
use nsh_core::prelude::*;
use serde_json::json;

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
                        .child(label("Diff & Review").font_weight(FontWeight::SEMIBOLD)),
                )
                .child(chrome_button(
                    Icon::Refresh,
                    "Refresh Git refs & diff",
                    move |_| {
                        load_refs(ui);
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
    let branches = refs["branches"].as_array().cloned().unwrap_or_default();
    let commits = refs["commits"].as_array().cloned().unwrap_or_default();

    // 1. QUICK COMPARE (Working tree, Staged, Last commit, AND Branches!)
    let mut quick = section_group("Quick Compare");

    let workdir_selected = mode == "git" && cur_cmp == "WORKDIR";
    quick = quick.child(sidebar_row(
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
        quick = quick.child(sidebar_row(
            "Staged changes",
            format!("{staged_count} staged"),
            staged_selected,
            move || select_git_pair(ui, "HEAD", "STAGED"),
        ));
    }

    let last_commit_selected = mode == "git" && cur_base == "HEAD~1" && cur_cmp == "HEAD";
    quick = quick.child(sidebar_row(
        "Last commit",
        "HEAD~1 → HEAD".into(),
        last_commit_selected,
        move || select_git_pair(ui, "HEAD~1", "HEAD"),
    ));

    if !branches.is_empty() {
        quick = quick.child(
            view()
                .pt_2()
                .pb_0p5()
                .px_2()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    muted("Branches")
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(muted(format!("base: {default_branch}")).text_size(px(10.5))),
        );
        for br in branches.iter().take(16) {
            let name = br["name"].as_str().unwrap_or("").to_owned();
            if name.is_empty() {
                continue;
            }
            let is_cur = br["is_current"].as_bool().unwrap_or(false) || name == current_branch;
            let is_def = br["is_default"].as_bool().unwrap_or(false) || name == default_branch;
            let short = br["short_sha"].as_str().unwrap_or("").to_owned();
            let selected_row = mode == "git" && cur_cmp == name && cur_cmp != "WORKDIR";
            let target_base = if is_def {
                format!("{name}~1")
            } else {
                default_branch.clone()
            };
            let click_name = name.clone();
            let meta = if is_cur {
                "current".to_owned()
            } else if is_def {
                "default".to_owned()
            } else {
                short
            };
            quick = quick.child(sidebar_branch_row(
                name,
                meta,
                is_cur,
                selected_row,
                move || select_git_pair(ui, target_base.clone(), click_name.clone()),
            ));
        }
    }

    // 2. CARDS & PRs
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
        let mut grp = section_group("Cards & PRs").count(reviewable_cards.len());
        for c in reviewable_cards.into_iter().take(15) {
            let cid = c.id.clone();
            let is_sel = cur_card == c.id && mode != "git";
            let prefix = if c.is_github_pr() || c.pull_request.is_some() {
                format!("PR · #{}", c.number)
            } else {
                format!("#{}", c.number)
            };
            grp = grp.child(sidebar_row(
                format!("{prefix} {}", c.title),
                c.stage.clone(),
                is_sel,
                move || open_card(ui, &cid),
            ));
        }
        Some(grp)
    };

    // 3. RECENT COMMITS
    let commits_section = if commits.is_empty() {
        None
    } else {
        let mut grp = section_group("Recent Commits").count(commits.len().min(15));
        for cm in commits.into_iter().take(15) {
            let sha = cm["sha"].as_str().unwrap_or("").to_owned();
            let short = cm["short_sha"].as_str().unwrap_or("").to_owned();
            let subj = cm["subject"].as_str().unwrap_or("").to_owned();
            let rel = cm["relative_time"].as_str().unwrap_or("").to_owned();
            if sha.is_empty() {
                continue;
            }
            let is_sel = mode == "git" && cur_cmp == sha;
            let parent = format!("{sha}~1");
            let click_sha = sha.clone();
            grp = grp.child(sidebar_commit_row(short, subj, rel, is_sel, move || {
                select_git_pair(ui, parent.clone(), click_sha.clone())
            }));
        }
        Some(grp)
    };

    let mut container = view().p_2p5().gap_4().child(quick);
    if let Some(cs) = cards_section {
        container = container.child(cs);
    }
    if let Some(cm) = commits_section {
        container = container.child(cm);
    }
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

fn sidebar_branch_row(
    name: String,
    meta: String,
    is_current: bool,
    selected: bool,
    on_click: impl Fn() + 'static,
) -> impl IntoElement {
    view().tooltip(hint_tooltip(name.clone())).child(
        canban_ui_kit::press_target()
            .w_full()
            .min_h(px(26.))
            .px_2()
            .py_0p5()
            .rounded(px(5.))
            .flex_row()
            .items_center()
            .justify_between()
            .gap_1p5()
            .bg(if selected { SELECTED } else { CLEAR })
            .hover(|s| s.bg(if selected { SELECTED } else { CARD_HOVER }))
            .on_press(move |_: &()| on_click())
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .flex_1()
                    .min_w(px(0.))
                    .child(icon(Icon::Branch).w(px(11.5)).h(px(11.5)).flex_shrink_0())
                    .child(
                        label(name)
                            .font_family("Menlo")
                            .text_size(px(11.5))
                            .font_weight(if is_current || selected {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .truncate()
                            .flex_1()
                            .min_w(px(0.)),
                    ),
            )
            .child(
                muted(meta)
                    .font_family("Menlo")
                    .text_size(px(10.))
                    .flex_shrink_0(),
            ),
    )
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

pub fn render_top_bar(ui: Ui) -> impl IntoElement {
    let st = ui.review_state;
    let project_anchor = element_ref();
    let base_anchor = element_ref();
    let cmp_anchor = element_ref();

    view()
        .border_b_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .child(
            view()
                .min_h(px(44.))
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
                            "Toggle Quick Compare sidebar",
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
                                            .map(|p| format!("{} ▾", p.name))
                                            .unwrap_or_else(|| "Select Project ▾".into())
                                    })
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM),
                                )
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
                        .child(
                            canban_ui_kit::press_target()
                                .element_ref(&base_anchor)
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
                                .accessibility_label("Choose Base (target) branch or commit")
                                .child(muted("base:").text_size(px(11.)))
                                .child(
                                    label(move || {
                                        let b = st.base_ref.get();
                                        let shown = if b.len() > 18 && !b.contains('/') {
                                            format!("{}…", &b[..10])
                                        } else if b.is_empty() {
                                            "main".into()
                                        } else {
                                            b
                                        };
                                        format!("{shown} ▾")
                                    })
                                    .font_family("Menlo")
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::MEDIUM),
                                )
                                .on_press(move |_: &()| show_ref_picker(ui, base_anchor, true)),
                        )
                        .child(muted("←").text_size(px(12.)))
                        .child(
                            canban_ui_kit::press_target()
                                .element_ref(&cmp_anchor)
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
                                .accessibility_label(
                                    "Choose Compare (source) branch, commit, or Working tree",
                                )
                                .child(muted("compare:").text_size(px(11.)))
                                .child(
                                    label(move || {
                                        let c = st.compare_ref.get();
                                        let shown = match c.as_str() {
                                            "" | "WORKDIR" => "Working tree".into(),
                                            "STAGED" => "Staged only".into(),
                                            other if other.len() > 18 && !other.contains('/') => {
                                                format!("{}…", &other[..10])
                                            }
                                            other => other.to_owned(),
                                        };
                                        format!("{shown} ▾")
                                    })
                                    .font_family("Menlo")
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::MEDIUM),
                                )
                                .on_press(move |_: &()| show_ref_picker(ui, cmp_anchor, false)),
                        )
                        .child(when(
                            move || {
                                st.mode.get() == "git"
                                    && !matches!(
                                        st.compare_ref.get().as_str(),
                                        "WORKDIR" | "STAGED"
                                    )
                            },
                            move || {
                                compact_action("⇄ Swap", Variant::Quiet, move |_| {
                                    let b = st.base_ref.get_untracked();
                                    let c = st.compare_ref.get_untracked();
                                    select_git_pair(ui, c, b);
                                })
                            },
                        ))
                        .child(when(
                            move || st.mode.get() != "github_pr",
                            move || {
                                compact_action(
                                    move || {
                                        if st.three_dot.get() { "A...B" } else { "A..B" }
                                    },
                                    Variant::Quiet,
                                    move |_| {
                                        st.three_dot.update(|v| *v = !*v);
                                        refresh_active_diff(ui, false);
                                    },
                                )
                            },
                        ))
                        .child(compact_action(
                            move || {
                                if st.custom_ref_open.get() {
                                    "Refs ▴"
                                } else {
                                    "Refs ▾"
                                }
                            },
                            Variant::Quiet,
                            move |_| st.custom_ref_open.update(|v| *v = !*v),
                        )),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .child(dynamic(move || {
                            let Some(card) = ui.current_card() else {
                                return view().into_element();
                            };
                            let Some(pr) = st.pr_target.get() else {
                                return view().into_element();
                            };
                            if card.worktree.is_none() && card.branch.is_none() {
                                return status(
                                    format!("Remote PR #{}", pr["number"].as_u64().unwrap_or(0)),
                                    Tone::Info,
                                )
                                .into_element();
                            }
                            let is_remote = st.mode.get() == "github_pr";
                            let pr_num = pr["number"].as_u64().unwrap_or(0);
                            view()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .p_0p5()
                                .rounded(px(RADIUS))
                                .bg(INSET)
                                .child(compact_action(
                                    "Local branch",
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
                                    format!("Remote PR #{pr_num}"),
                                    if is_remote {
                                        Variant::Primary
                                    } else {
                                        Variant::Quiet
                                    },
                                    move |_| {
                                        st.mode.set("github_pr".into());
                                        refresh_active_diff(ui, false);
                                    },
                                ))
                                .into_element()
                        }))
                        .child(when(
                            move || !ui.card.get().is_empty() && !ui.narrow.get(),
                            move || {
                                compact_action(
                                    move || {
                                        if st.card_drawer_open.get() {
                                            "Hide Card"
                                        } else {
                                            "Show Card"
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
        .child(when(
            move || st.custom_ref_open.get(),
            move || {
                view()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(BORDER)
                    .bg(INSET)
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(muted("Base ref:"))
                    .child(
                        field(st.base_ref, "e.g. main, v0.2.0, HEAD~3, or commit SHA").w(px(210.)),
                    )
                    .child(muted("← Compare ref:"))
                    .child(
                        field(st.compare_ref, "e.g. WORKDIR, STAGED, branch, or SHA").w(px(210.)),
                    )
                    .child(compact_action("Compare", Variant::Primary, move |_| {
                        st.mode.set("git".into());
                        ui.card.set(String::new());
                        refresh_active_diff(ui, false);
                    }))
            },
        ))
}

fn show_ref_picker(ui: Ui, anchor: ElementRef, is_base: bool) {
    let st = ui.review_state;
    let refs = st.refs_data.get_untracked();
    let branches = refs["branches"].as_array().cloned().unwrap_or_default();
    let tags = refs["tags"].as_array().cloned().unwrap_or_default();
    let commits = refs["commits"].as_array().cloned().unwrap_or_default();

    ui.spawn(async move {
        let mut items: Vec<MenuItem> = Vec::new();
        if !is_base {
            items.push(
                MenuItem::new("Working tree (uncommitted)").on_select(move || {
                    let cur_b = st.base_ref.get_untracked();
                    let b = if cur_b.is_empty() {
                        "HEAD".into()
                    } else {
                        cur_b
                    };
                    select_git_pair(ui, b, "WORKDIR");
                }),
            );
            items.push(MenuItem::new("Staged changes only").on_select(move || {
                let cur_b = st.base_ref.get_untracked();
                let b = if cur_b.is_empty() {
                    "HEAD".into()
                } else {
                    cur_b
                };
                select_git_pair(ui, b, "STAGED");
            }));
            items.push(MenuItem::separator());
        } else {
            items.push(MenuItem::new("HEAD").on_select(move || {
                st.base_ref.set("HEAD".into());
                if st.mode.get_untracked() != "card_local" {
                    st.mode.set("git".into());
                }
                refresh_active_diff(ui, false);
            }));
            items.push(
                MenuItem::new("HEAD~1 (previous commit)").on_select(move || {
                    st.base_ref.set("HEAD~1".into());
                    st.mode.set("git".into());
                    refresh_active_diff(ui, false);
                }),
            );
            items.push(MenuItem::separator());
        }

        for br in branches.into_iter().take(25) {
            let name = br["name"].as_str().unwrap_or("").to_owned();
            if name.is_empty() {
                continue;
            }
            let suffix = if br["is_current"].as_bool().unwrap_or(false) {
                " (current)"
            } else if br["is_default"].as_bool().unwrap_or(false) {
                " (default)"
            } else {
                ""
            };
            let chosen = name.clone();
            items.push(
                MenuItem::new(format!("⎇ {name}{suffix}")).on_select(move || {
                    if is_base {
                        st.base_ref.set(chosen.clone());
                        if st.mode.get_untracked() == "github_pr" {
                            st.mode.set("git".into());
                        }
                        refresh_active_diff(ui, false);
                    } else {
                        let b = st.base_ref.get_untracked();
                        select_git_pair(
                            ui,
                            if b.is_empty() { "main".into() } else { b },
                            chosen.clone(),
                        );
                    }
                }),
            );
        }

        if !tags.is_empty() {
            items.push(MenuItem::separator());
            for tg in tags.into_iter().take(10) {
                let name = tg["name"].as_str().unwrap_or("").to_owned();
                if name.is_empty() {
                    continue;
                }
                let chosen = name.clone();
                items.push(MenuItem::new(format!("🏷 {name}")).on_select(move || {
                    if is_base {
                        st.base_ref.set(chosen.clone());
                        st.mode.set("git".into());
                        refresh_active_diff(ui, false);
                    } else {
                        let b = st.base_ref.get_untracked();
                        select_git_pair(
                            ui,
                            if b.is_empty() { "main".into() } else { b },
                            chosen.clone(),
                        );
                    }
                }));
            }
        }

        if !commits.is_empty() {
            items.push(MenuItem::separator());
            for cm in commits.into_iter().take(12) {
                let sha = cm["sha"].as_str().unwrap_or("").to_owned();
                let short = cm["short_sha"].as_str().unwrap_or("").to_owned();
                let subj: String = cm["subject"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(42)
                    .collect();
                if sha.is_empty() {
                    continue;
                }
                let chosen = sha.clone();
                items.push(
                    MenuItem::new(format!("{short} · {subj}")).on_select(move || {
                        if is_base {
                            st.base_ref.set(chosen.clone());
                            st.mode.set("git".into());
                            refresh_active_diff(ui, false);
                        } else {
                            let b = st.base_ref.get_untracked();
                            select_git_pair(
                                ui,
                                if b.is_empty() {
                                    format!("{chosen}~1")
                                } else {
                                    b
                                },
                                chosen.clone(),
                            );
                        }
                    }),
                );
            }
        }

        items.push(MenuItem::separator());
        items.push(
            MenuItem::new("Type custom branch / commit SHA…").on_select(move || {
                st.custom_ref_open.set(true);
            }),
        );

        if let Some(bounds) = anchor.measure().await {
            popup_menu_at(
                items,
                point(bounds.page.x, bounds.page.y + bounds.frame.size.height),
            )
            .await;
        } else {
            popup_menu(items).await;
        }
    });
}

pub fn render_card_delivery_banner(ui: Ui) -> AnyElement {
    let st = ui.review_state;
    let Some(c) = ui.current_card() else {
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
            .py_2p5()
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
                    .gap_0p5()
                    .child(
                        label(format!(
                            "Delivered · merged into {} ({short})",
                            d.target_branch
                        ))
                        .text_color(SUCCESS)
                        .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(muted(format!(
                        "Card #{}: {} · {}",
                        c.number, c.title, d.commit
                    ))),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        action("Archive Card", Variant::Primary, move |_| {
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

    let message_key = format!("review-msg:{}", c.id);
    let msg_draft = crate::drafts::document(
        ui,
        message_key,
        crate::drafts::Draft {
            body: format!("Canban #{}: {}", c.number, c.title),
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

    view()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(BORDER)
        .bg(INSET)
        .gap_2()
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
                        .child(badge(format!("Card #{}", c.number)))
                        .child(
                            label(c.title.clone())
                                .font_weight(FontWeight::SEMIBOLD)
                                .max_w(px(340.))
                                .truncate(),
                        )
                        .child(status(c.stage.clone(), Tone::Neutral)),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_1p5()
                        .child(when(
                            move || card_for_return.stage != "develop",
                            {
                                let c_ret = c.clone();
                                move || {
                                    let cid_outer = c_ret.id.clone();
                                    let rev = c_ret.revision;
                                    compact_action(
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
                                    )
                                }
                            },
                        ))
                        .child(if has_worktree && st.mode.get() == "card_local" {
                            view()
                                .flex_row()
                                .items_center()
                                .gap_1p5()
                                .child(crate::github::choice(
                                    ui,
                                    st.delivery_mode,
                                    vec![("local", "Local merge"), ("github_pr", "GitHub PR")],
                                ))
                                .child(compact_action(
                                    move || {
                                        if st.delivery_customize.get() {
                                            "Commit msg ▴"
                                        } else {
                                            "Commit msg ▾"
                                        }
                                    },
                                    Variant::Quiet,
                                    move |_| st.delivery_customize.update(|v| *v = !*v),
                                ))
                                .child(
                                    action(
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
                                                dispatch_then(
                                                    ui,
                                                    if st.delivery_mode.get_untracked()
                                                        == "github_pr"
                                                    {
                                                        "github.pr.publish"
                                                    } else {
                                                        "review.merge"
                                                    },
                                                    json!({
                                                        "card_id": p.card_id,
                                                        "revision": p.revision,
                                                        "token": p.token,
                                                        "target_branch": p.target_branch,
                                                        "message": commit_msg.get_untracked(),
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
                                                        cur.revision != p.revision
                                                            || cur.id != p.card_id
                                                    })
                                            })
                                    }),
                                )
                                .into_element()
                        } else {
                            view().into_element()
                        }),
                ),
        )
        .child(crate::github::pr_panel(ui))
        .child(when(
            move || st.delivery_customize.get(),
            move || {
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(muted("Merge commit message:"))
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
                // Re-evaluate against the live snapshot: the preview's warning was
                // captured when the Diff opened and goes stale once the Agent idles.
                let live_warning = ui.snapshot.with(|s| {
                    canban_types::agent_activity_warning(&p.card_id, &s.attempts, &s.requests)
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
