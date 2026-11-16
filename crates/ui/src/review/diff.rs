use super::git_ops;
use super::model::{LineTarget, PendingInlineComment, parse_unified_hunk_lines};
use crate::presenter::*;
use canban_ui_kit::tokens::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;
use serde_json::{Value, json};

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

pub fn render_diff_workbench(ui: Ui, data: &Value, loading: bool) -> AnyElement {
    let st = ui.review_state;
    let files = data["files"].as_array().cloned().unwrap_or_default();
    let commits = data["commits"].as_array().cloned().unwrap_or_default();
    let remote_threads = data["threads"].as_array().cloned().unwrap_or_default();
    let total_add = data["total_additions"].as_u64().unwrap_or(0);
    let total_del = data["total_deletions"].as_u64().unwrap_or(0);
    let truncated = data["truncated"].as_bool().unwrap_or(false);
    let commit_detail = data["commit_detail"]
        .as_object()
        .map(|_| &data["commit_detail"]);
    let is_workdir_mode = st.mode.get() == "git"
        && matches!(
            st.compare_ref.get().as_str(),
            "" | "WORKDIR" | "STAGED" | "UNSTAGED"
        );

    if loading && files.is_empty() && commit_detail.is_none() && !is_workdir_mode {
        return view()
            .flex_1()
            .items_center()
            .justify_center()
            .p_6()
            .child(muted("Loading diff…"))
            .into_element();
    }

    if files.is_empty() && commit_detail.is_none() && !is_workdir_mode {
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

    let selected = st.selected_file.get();
    let active_line = st.active_line.get();
    let pending_all = st.pending_comments.get();
    let editing_remote = st.editing_remote_comment.get();
    let deleting_remote = st.deleting_remote_comment.get();
    let my_login = ui.snapshot.with(|s| {
        ui.github
            .gh_status
            .with(|gh| crate::github::active_login_from(s, gh))
    });
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

    let status_files_for_actions: Vec<Value> = if is_workdir_mode {
        data["status_files"]
            .as_array()
            .cloned()
            .or_else(|| st.refs_data.with(|v| v["status_files"].as_array().cloned()))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let left_pane = if is_workdir_mode {
        git_ops::render_working_tree_pane(ui, data)
    } else {
        // Standard file list pane (216px)
        let file_count = files.len();
        let mut file_list = view().gap_0p5().p_1p5();
        let all_selected = selected.is_empty();
        file_list = file_list.child(
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
                .bg(if all_selected { SELECTED } else { CLEAR })
                .hover(|s| s.bg(if all_selected { SELECTED } else { CARD_HOVER }))
                .on_press(move |_: &()| st.selected_file.set(String::new()))
                .child(
                    label(format!("All files ({file_count})"))
                        .text_size(px(12.))
                        .font_weight(if all_selected {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        }),
                )
                .child(
                    muted(format!("+{total_add} −{total_del}"))
                        .font_family(MONO_FONT)
                        .text_size(px(10.5)),
                ),
        );

        for f in &files {
            let fname = f["filename"].as_str().unwrap_or("").to_owned();
            let f_add = f["additions"].as_u64().unwrap_or(0);
            let f_del = f["deletions"].as_u64().unwrap_or(0);
            let f_status = f["status"].as_str().unwrap_or("modified");
            let is_sel = selected == fname;
            let click_name = fname.clone();
            let pending_for_file = pending_all.iter().filter(|c| c.path == fname).count();
            let short_name = fname.rsplit('/').next().unwrap_or(&fname).to_owned();
            let status_char = match f_status {
                "added" => ("A", SUCCESS),
                "removed" => ("D", DANGER),
                "renamed" => ("R", ACCENT),
                _ => ("M", MUTED),
            };

            let mut right_meta = view().flex_row().items_center().gap_1().flex_shrink_0();
            if pending_for_file > 0 {
                right_meta = right_meta.child(status(pending_for_file.to_string(), Tone::Info));
            }
            right_meta = right_meta
                .child(
                    label(format!("+{f_add}"))
                        .font_family(MONO_FONT)
                        .text_size(px(10.5))
                        .text_color(SUCCESS),
                )
                .child(
                    label(format!("−{f_del}"))
                        .font_family(MONO_FONT)
                        .text_size(px(10.5))
                        .text_color(DANGER),
                );

            file_list = file_list.child(
                view().tooltip(hint_tooltip(fname.clone())).child(
                    canban_ui_kit::press_target()
                        .w_full()
                        .min_h(px(27.))
                        .px_2()
                        .py_0p5()
                        .rounded(px(5.))
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_1p5()
                        .bg(if is_sel { SELECTED } else { CLEAR })
                        .hover(|s| s.bg(if is_sel { SELECTED } else { CARD_HOVER }))
                        .on_press(move |_: &()| st.selected_file.set(click_name.clone()))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_1p5()
                                .flex_1()
                                .min_w(px(0.))
                                .child(
                                    label(status_char.0)
                                        .font_family(MONO_FONT)
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(status_char.1),
                                )
                                .child(
                                    label(short_name)
                                        .text_size(px(12.))
                                        .truncate()
                                        .min_w(px(0.)),
                                ),
                        )
                        .child(right_meta),
                ),
            );
        }

        view()
            .w(px(216.))
            .flex_shrink_0()
            .min_h(px(0.))
            .border_r_1()
            .border_color(BORDER)
            .bg(SURFACE)
            .child(scroll().flex_1().min_h(px(0.)).child(file_list))
            .into_element()
    };

    // Right main diff viewer
    let mut diff_column = view().gap_3().p_3();

    if let Some(detail) = commit_detail {
        diff_column = diff_column.child(git_ops::render_commit_detail_card(ui, detail));
    }

    if truncated {
        diff_column = diff_column.child(notice(
            "Diff output is capped for UI responsiveness. Inspect very large changes in Terminals if needed.",
            Tone::Attention,
        ));
    }

    // Collapsible commit summary bar when comparing branches/ranges (clickable to inspect single commit!)
    if !commits.is_empty() && commit_detail.is_none() {
        let commit_len = commits.len();
        let is_open = st.commits_open.get();
        let mut commit_rows = view().gap_1().pt_1();
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
        diff_column = diff_column.child(collapsible_section_group(
            "Commits in range",
            Some(commit_len),
            is_open,
            move |_| st.commits_open.update(|v| *v = !*v),
            commit_rows,
        ));
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
                    } else {
                        "No file changes in this commit"
                    })
                    .font_weight(FontWeight::SEMIBOLD),
                )
                .child(muted(if is_workdir_mode {
                    "No modified or untracked files in this checkout. You can still pull, push, or amend the last commit."
                } else {
                    "This commit did not introduce net file content changes relative to the selected parent."
                })),
        );
    }

    let visible_files: Vec<Value> = if selected.is_empty() {
        files
    } else {
        files
            .into_iter()
            .filter(|f| f["filename"].as_str() == Some(selected.as_str()))
            .collect()
    };

    for file in visible_files {
        let path = file["filename"].as_str().unwrap_or("").to_owned();
        let f_status = file["status"].as_str().unwrap_or("modified").to_owned();
        let additions = file["additions"].as_u64().unwrap_or(0);
        let deletions = file["deletions"].as_u64().unwrap_or(0);
        let patch = file["patch"].as_str().unwrap_or("").to_owned();
        let file_btn_path = path.clone();
        let sf_entry = status_files_for_actions
            .iter()
            .find(|s| s["path"].as_str() == Some(path.as_str()));
        let is_file_staged = sf_entry
            .and_then(|s| s["staged"].as_bool())
            .unwrap_or(false);
        let is_file_unstaged = sf_entry
            .map(|s| {
                s["unstaged"].as_bool().unwrap_or(false)
                    || s["untracked"].as_bool().unwrap_or(false)
            })
            .unwrap_or(false);

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
            .child(
                label(path.clone())
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .font_family(MONO_FONT)
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(badge(f_status))
            .child(
                label(format!("+{additions}"))
                    .text_size(px(11.5))
                    .font_family(MONO_FONT)
                    .text_color(SUCCESS),
            )
            .child(
                label(format!("−{deletions}"))
                    .text_size(px(11.5))
                    .font_family(MONO_FONT)
                    .text_color(DANGER),
            );

        if is_workdir_mode {
            if is_file_unstaged || !is_file_staged {
                let p_stage = path.clone();
                header = header.child(compact_action("+ Stage", Variant::Quiet, move |_| {
                    git_ops::run_stage(ui, vec![p_stage.clone()], false);
                }));
            }
            if is_file_staged {
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

        header = header.child(compact_action(
            "+ File comment",
            Variant::Quiet,
            move |_| {
                st.active_line.set(Some(LineTarget {
                    path: file_btn_path.clone(),
                    line: 1,
                    side: "RIGHT".into(),
                    snippet: String::new(),
                }));
                st.line_draft.set(String::new());
            },
        ));

        let body = if patch.trim().is_empty() {
            view()
                .p_3()
                .child(muted(
                    "Binary file, rename without content changes, or empty patch.",
                ))
                .into_element()
        } else {
            let parsed_lines = parse_unified_hunk_lines(&patch);
            let mut rows = view();

            // Show file-level comment composer if line == 1 and no specific hunk line matched or at top
            for dl in parsed_lines.into_iter().take(500) {
                if dl.kind == '@' || dl.kind == '\\' {
                    rows = rows.child(
                        view().px_3().py_1().bg(INSET).child(
                            label(dl.text)
                                .font_family(MONO_FONT)
                                .text_size(px(11.5))
                                .text_color(MUTED),
                        ),
                    );
                    continue;
                }

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
                let line_path = path.clone();
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
                line_btn = match dl.kind {
                    '+' => line_btn.bg(SUCCESS_SURFACE),
                    '-' => line_btn.bg(DANGER_SURFACE),
                    _ => line_btn.bg(CLEAR),
                };

                rows = rows.child(
                    line_btn
                        .hover(|s| s.bg(SELECTED))
                        .accessibility_label(format!(
                            "Add review comment on {path} line {comment_line}"
                        ))
                        .on_press(move |_: &()| {
                            st.active_line.set(Some(LineTarget {
                                path: line_path.clone(),
                                line: comment_line,
                                side: side_owned.clone(),
                                snippet: snippet_for_target.clone(),
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
                for rt in remote_threads.iter().filter(|t| {
                    t["path"].as_str() == Some(path.as_str())
                        && t["line"].as_u64().or_else(|| t["original_line"].as_u64())
                            == Some(comment_line)
                        && t["side"].as_str().unwrap_or("RIGHT") == comment_side
                }) {
                    let author = rt["user"]["login"]
                        .as_str()
                        .unwrap_or("reviewer")
                        .to_owned();
                    let cid = rt["id"].as_u64().unwrap_or(0);
                    let web_url =
                        crate::github::comment_web_url(rt, pr_base_url.as_deref(), "threads");
                    let is_own = cid > 0
                        && pr_target.is_some()
                        && my_login
                            .as_deref()
                            .is_some_and(|me| author.eq_ignore_ascii_case(me));
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
                                                MenuItem::new("Edit comment").on_select(
                                                    move || {
                                                        let b = body_val.clone();
                                                        batch(move || {
                                                            st.remote_edit_error.set(String::new());
                                                            st.remote_delete_error
                                                                .set(String::new());
                                                            st.deleting_remote_comment.set(None);
                                                            st.remote_edit_draft.set(b);
                                                            st.editing_remote_comment
                                                                .set(Some(cid));
                                                        });
                                                    },
                                                ),
                                                MenuItem::new("Delete comment…").on_select(
                                                    move || {
                                                        batch(move || {
                                                            st.remote_delete_error
                                                                .set(String::new());
                                                            st.remote_edit_error.set(String::new());
                                                            st.editing_remote_comment.set(None);
                                                            st.deleting_remote_comment
                                                                .set(Some(cid));
                                                        });
                                                    },
                                                ),
                                            ],
                                            point(
                                                bounds.page.x,
                                                bounds.page.y + bounds.frame.size.height,
                                            ),
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
                            pr_target.clone(),
                        ));
                    }
                    let content = if is_editing {
                        remote_pr_comment_editor(ui, cid, pr_target.clone()).into_element()
                    } else {
                        canban_ui_kit::markdown::reader(move || body_txt.clone()).into_element()
                    };
                    rows = rows.child(comment_card.child(content));
                }

                // Render any pending inline comments saved for this file + line + side
                for item in pending_all
                    .iter()
                    .filter(|c| c.path == path && c.line == comment_line && c.side == comment_side)
                {
                    let item_id = item.id.clone();
                    let edit_item = item.clone();
                    let body_txt = item.body.clone();
                    rows = rows.child(
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
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_1p5()
                                            .child(status("Pending comment", Tone::Info))
                                            .child(
                                                muted(format!("{}:{}", item.path, item.line))
                                                    .font_family(MONO_FONT)
                                                    .text_size(px(11.)),
                                            ),
                                    )
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_1()
                                            .child(compact_action(
                                                "Edit",
                                                Variant::Quiet,
                                                move |_| {
                                                    let cur = edit_item.clone();
                                                    st.pending_comments.update(|list| {
                                                        list.retain(|x| x.id != cur.id);
                                                    });
                                                    st.active_line.set(Some(LineTarget {
                                                        path: cur.path,
                                                        line: cur.line,
                                                        side: cur.side,
                                                        snippet: cur.snippet,
                                                    }));
                                                    st.line_draft.set(cur.body);
                                                },
                                            ))
                                            .child(compact_action(
                                                "Remove",
                                                Variant::Quiet,
                                                move |_| {
                                                    let rm_id = item_id.clone();
                                                    st.pending_comments.update(|list| {
                                                        list.retain(|x| x.id != rm_id);
                                                    });
                                                },
                                            )),
                                    ),
                            )
                            .child(label(body_txt).text_size(px(12.5))),
                    );
                }

                // Render active inline composer right beneath the clicked line
                if let Some(ref target) = active_line
                    && target.path == path
                    && target.line == comment_line
                    && target.side == comment_side
                {
                    let save_target = target.clone();
                    rows = rows.child(inline_composer_box(ui, save_target));
                }
            }
            rows.into_element()
        };

        // Fallback composer when "+ File comment" was clicked on an empty/binary patch
        let extra_composer = if let Some(ref target) = active_line
            && target.path == path
            && patch.trim().is_empty()
        {
            inline_composer_box(ui, target.clone()).into_element()
        } else {
            view().into_element()
        };

        diff_column = diff_column.child(
            view()
                .rounded(px(RADIUS))
                .border_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .overflow_hidden()
                .child(header)
                .child(extra_composer)
                .child(body),
        );
    }

    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .flex_row()
        .child(left_pane)
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .min_w(px(0.))
                .child(diff_column),
        )
        .into_element()
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
                        st.pending_comments.update(|list| {
                            list.push(PendingInlineComment {
                                id: canban_types::id(),
                                path: target.path.clone(),
                                line: target.line,
                                side: target.side.clone(),
                                snippet: target.snippet.clone(),
                                body: body.trim().to_owned(),
                            });
                        });
                        st.active_line.set(None);
                        st.line_draft.set(String::new());
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
                        if crate::APP.get().is_none() {
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
                                if crate::APP.get().is_none() {
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
