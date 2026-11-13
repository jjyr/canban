use super::model::format_pending_markdown;
use crate::presenter::*;
use canban_types::GitHubReference;
use canban_ui_kit::tokens::*;
use canban_ui_kit::*;
use nsh_core::element::desktop::{MenuItem, popup_menu};
use nsh_core::prelude::*;
use serde_json::{Value, json};

fn summary_editor(value: Signal<String>) -> View {
    let focused = signal(false);
    view()
        .h(px(76.))
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
                .accessibility_label("Overall review summary")
                .placeholder("Optional overall summary or instructions for @Agent / GitHub review…")
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

fn default_profile_id(ui: Ui) -> String {
    let def = ui.default_agent.get_untracked();
    ui.profiles
        .with_untracked(|profiles| {
            profiles
                .iter()
                .find(|p| p.id == def)
                .or_else(|| profiles.iter().find(|p| p.id == "developer"))
                .or_else(|| profiles.first())
                .map(|p| p.id.clone())
        })
        .unwrap_or_else(|| {
            if def.is_empty() {
                "developer".into()
            } else {
                def
            }
        })
}

pub fn context_label_untracked(ui: Ui) -> String {
    let st = ui.review_state;
    let mode = st.mode.get_untracked();
    if mode == "github_pr"
        && let Some(pr) = st.pr_target.get_untracked()
    {
        return format!(
            "PR {}#{}",
            pr["repository"].as_str().unwrap_or(""),
            pr["number"].as_u64().unwrap_or(0)
        );
    }
    if let Some(card) = ui.current_card()
        && mode != "git"
    {
        let base = st.base_ref.get_untracked();
        let cmp = card
            .branch
            .clone()
            .unwrap_or_else(|| st.compare_ref.get_untracked());
        return format!("Card #{}: {} ({base} ← {cmp})", card.number, card.title);
    }
    let base = st.base_ref.get_untracked();
    let cmp = st.compare_ref.get_untracked();
    let cmp_display = match cmp.as_str() {
        "WORKDIR" => "Working tree",
        "STAGED" => "Staged changes",
        other => other,
    };
    format!("{base} ← {cmp_display}")
}

pub fn open_submit_panel(ui: Ui) {
    let st = ui.review_state;
    if st.submit_profile.get_untracked().is_empty() {
        st.submit_profile.set(default_profile_id(ui));
    }
    let has_card = ui.current_card().is_some() && st.mode.get_untracked() != "git";
    let is_pr = st.mode.get_untracked() == "github_pr" || st.pr_target.get_untracked().is_some();
    let cur_action = st.submit_action.get_untracked();
    if has_card {
        if !matches!(
            cur_action.as_str(),
            "card_agent" | "card_comment" | "github_review" | "terminal"
        ) {
            st.submit_action.set("card_agent".into());
        }
    } else if is_pr {
        if !matches!(
            cur_action.as_str(),
            "terminal" | "new_card" | "github_review"
        ) {
            st.submit_action.set("terminal".into());
        }
    } else if !matches!(cur_action.as_str(), "terminal" | "new_card") {
        st.submit_action.set("terminal".into());
    }
    st.submit_open.set(true);
}

pub fn render_bottom_bar(ui: Ui) -> impl IntoElement {
    let st = ui.review_state;
    view()
        .flex_shrink_0()
        .border_t_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .child(when(
            move || st.submit_open.get(),
            move || render_submit_drawer(ui),
        ))
        .child(
            view()
                .px_3()
                .py_2()
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
                        .child(dynamic(move || {
                            let n = st.pending_comments.with(Vec::len);
                            if n == 0 {
                                muted("No pending inline comments · click any diff line or send a summary review").into_element()
                            } else {
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(status(
                                        format!(
                                            "{n} pending inline comment{}",
                                            if n == 1 { "" } else { "s" }
                                        ),
                                        Tone::Info,
                                    ))
                                    .child(compact_action("Clear", Variant::Quiet, move |_| {
                                        st.pending_comments.set(Vec::new());
                                    }))
                                    .into_element()
                            }
                        })),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(compact_action("Copy Markdown", Variant::Quiet, move |_| {
                            let md = format_pending_markdown(
                                &context_label_untracked(ui),
                                &st.submit_summary.get_untracked(),
                                &st.pending_comments.get_untracked(),
                            );
                            copy_text(ui, md);
                        }))
                        .child(compact_action(
                            move || {
                                let n = st.pending_comments.with(Vec::len);
                                if st.submit_open.get() {
                                    "Hide Review Panel ▾".into()
                                } else if n > 0 {
                                    format!("Finish Review ({n}) ▴")
                                } else {
                                    "Review & Send to @Agent…".into()
                                }
                            },
                            Variant::Primary,
                            move |_| {
                                if st.submit_open.get_untracked() {
                                    st.submit_open.set(false);
                                } else {
                                    open_submit_panel(ui);
                                }
                            },
                        )),
                ),
        )
}

fn render_submit_drawer(ui: Ui) -> impl IntoElement {
    let st = ui.review_state;
    let has_card = ui.current_card().is_some() && st.mode.get_untracked() != "git";
    let pr_info = st.pr_target.get_untracked().or_else(|| {
        ui.current_card().and_then(|c| {
            if c.is_github_pr() {
                c.github_issue().map(|s| {
                    json!({
                        "repository": s.repository,
                        "number": s.number,
                        "url": s.url,
                    })
                })
            } else {
                c.pull_request.as_ref().map(|pr| {
                    json!({
                        "repository": pr.repository,
                        "number": pr.number,
                        "url": pr.url,
                    })
                })
            }
        })
    });
    let has_pr = pr_info.is_some();

    let mut choices: Vec<(&'static str, &'static str)> = Vec::new();
    if has_card {
        choices.push(("card_agent", "Send to Card @Agent (local)"));
        choices.push(("card_comment", "Save to Card Timeline only"));
    } else {
        choices.push(("terminal", "Run @Agent in Terminal (no Card needed)"));
        choices.push(("new_card", "Create Card & assign @Agent"));
    }
    if has_pr {
        choices.push(("github_review", "Sync Review to GitHub PR"));
    }
    if has_card {
        choices.push(("terminal", "Run standalone @Agent in Terminal"));
    }

    view()
        .p_3()
        .gap_2p5()
        .border_b_1()
        .border_color(BORDER)
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
                        .gap_2()
                        .child(label("Dispatch Review").font_weight(FontWeight::SEMIBOLD))
                        .child(muted(move || context_label_untracked(ui))),
                )
                .child(chrome_button(Icon::Close, "Close dispatch panel", move |_| {
                    st.submit_open.set(false);
                })),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .children(
                    choices
                        .into_iter()
                        .map(|(id, text)| {
                            let target_id = id.to_owned();
                            let check_id = id.to_owned();
                            canban_ui_kit::press_target()
                                .h(px(28.))
                                .px_2p5()
                                .rounded(px(RADIUS))
                                .border_1()
                                .border_color(move || {
                                    if st.submit_action.get() == check_id {
                                        ACCENT
                                    } else {
                                        BORDER
                                    }
                                })
                                .bg(move || {
                                    if st.submit_action.get() == target_id {
                                        SELECTED
                                    } else {
                                        SURFACE
                                    }
                                })
                                .hover(|s| s.bg(CARD_HOVER))
                                .child(label(text).text_size(px(12.)).font_weight(FontWeight::MEDIUM))
                                .on_press({
                                    let set_id = id.to_owned();
                                    move |_: &()| st.submit_action.set(set_id.clone())
                                })
                                .into_element()
                        })
                        .collect::<Vec<_>>(),
                ),
        )
        .child(summary_editor(st.submit_summary))
        .child(dynamic(move || {
            let act = st.submit_action.get();
            match act.as_str() {
                "github_review" => view()
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
                            .child(muted("GitHub PR verdict:"))
                            .child(crate::github::choice(
                                ui,
                                st.submit_gh_event,
                                vec![
                                    ("COMMENT", "Comment"),
                                    ("APPROVE", "Approve"),
                                    ("REQUEST_CHANGES", "Request changes"),
                                ],
                            ))
                            .child(
                                muted("Publishes summary + pending inline line comments to GitHub.")
                                    .text_size(px(11.5)),
                            ),
                    )
                    .child(submit_primary_button(ui, pr_info.clone()))
                    .into_element(),
                "card_comment" => view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(muted(
                        "Posts the formatted review + line comments to the Card Timeline without waking an Agent.",
                    ))
                    .child(submit_primary_button(ui, pr_info.clone()))
                    .into_element(),
                "new_card" => view()
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
                            .child(profile_picker(ui, st.submit_profile))
                            .child(
                                field(st.submit_new_card_title, "Card title (optional)")
                                    .w(px(280.)),
                            ),
                    )
                    .child(submit_primary_button(ui, pr_info.clone()))
                    .into_element(),
                "terminal" => {
                    let cmp_ref = st.compare_ref.get();
                    let can_isolate = !matches!(cmp_ref.as_str(), "" | "WORKDIR" | "STAGED");
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
                                .gap_3()
                                .child(profile_picker(ui, st.submit_profile))
                                .child(if can_isolate {
                                    canban_ui_kit::press_target()
                                        .h(px(26.))
                                        .px_2()
                                        .rounded(px(5.))
                                        .flex_row()
                                        .items_center()
                                        .gap_1p5()
                                        .bg(SURFACE)
                                        .border_1()
                                        .border_color(BORDER)
                                        .on_press(move |_: &()| {
                                            st.submit_create_worktree.update(|v| *v = !*v)
                                        })
                                        .child(label(move || {
                                            if st.submit_create_worktree.get() {
                                                "☑ Run in isolated worktree"
                                            } else {
                                                "☐ Run in isolated worktree"
                                            }
                                        }).text_size(px(12.)))
                                        .into_element()
                                } else {
                                    muted("Runs directly in the Project checkout via Herdr Terminals.")
                                        .text_size(px(11.5))
                                        .into_element()
                                }),
                        )
                        .child(submit_primary_button(ui, pr_info.clone()))
                        .into_element()
                }
                _ => {
                    // "card_agent"
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
                                .gap_3()
                                .child(profile_picker(ui, st.submit_profile))
                                .child(
                                    canban_ui_kit::press_target()
                                        .h(px(26.))
                                        .px_2()
                                        .rounded(px(5.))
                                        .flex_row()
                                        .items_center()
                                        .gap_1p5()
                                        .bg(SURFACE)
                                        .border_1()
                                        .border_color(BORDER)
                                        .on_press(move |_: &()| {
                                            st.submit_move_develop.update(|v| *v = !*v)
                                        })
                                        .child(
                                            label(move || {
                                                if st.submit_move_develop.get() {
                                                    "☑ Move Card to Develop"
                                                } else {
                                                    "☐ Keep current stage"
                                                }
                                            })
                                            .text_size(px(12.)),
                                        ),
                                )
                                .child(
                                    muted("Local only — never posts to GitHub.")
                                        .text_size(px(11.5)),
                                ),
                        )
                        .child(submit_primary_button(ui, pr_info.clone()))
                        .into_element()
                }
            }
        }))
}

fn profile_picker(ui: Ui, selected: Signal<String>) -> impl IntoElement {
    dynamic(move || {
        let profiles = ui.profiles.get();
        let current = selected.get();
        let display = if current.is_empty() {
            default_profile_id(ui)
        } else {
            current.clone()
        };
        let proj_for_icon = ui.current_project();
        let mark = profiles
            .iter()
            .find(|p| p.id == display)
            .map(|p| crate::profiles::effective_provider(p, proj_for_icon.as_ref()).0)
            .unwrap_or(Icon::Agent);
        action_with_icon(
            mark,
            format!("@{display} ▾"),
            Variant::Secondary,
            move |_| {
                let list = profiles.clone();
                ui.spawn(async move {
                    let items: Vec<MenuItem> = if list.is_empty() {
                        vec![MenuItem::new("@developer").on_select(move || {
                            selected.try_set("developer".into());
                        })]
                    } else {
                        let proj = ui.current_project();
                        list.into_iter()
                            .map(|p| {
                                let pid = p.id.clone();
                                let (prov, overridden) =
                                    crate::profiles::effective_provider_name(&p, proj.as_ref());
                                let suffix = if overridden {
                                    format!("{prov} · Project override")
                                } else {
                                    prov
                                };
                                MenuItem::new(format!("@{} ({suffix})", p.id)).on_select(
                                    move || {
                                        selected.try_set(pid.clone());
                                    },
                                )
                            })
                            .collect()
                    };
                    popup_menu(items).await;
                });
            },
        )
    })
}

fn submit_primary_button(ui: Ui, pr_info: Option<Value>) -> impl IntoElement {
    let st = ui.review_state;
    let act = st.submit_action.get();
    let btn_label = match act.as_str() {
        "github_review" => "Submit to GitHub PR",
        "card_comment" => "Post to Card Timeline",
        "new_card" => "Create Card & Dispatch",
        "terminal" => "Launch @Agent Terminal",
        _ => "Send to Card @Agent",
    };

    action(btn_label, Variant::Primary, move |_| {
        let action_kind = st.submit_action.get_untracked();
        let summary = st.submit_summary.get_untracked();
        let pending = st.pending_comments.get_untracked();
        let inline_json: Vec<serde_json::Value> = pending
            .iter()
            .map(|c| {
                json!({
                    "path": c.path,
                    "line": c.line,
                    "side": c.side,
                    "snippet": c.snippet,
                    "body": c.body,
                })
            })
            .collect();

        if crate::APP.get().is_none() {
            // Gallery mode: simulate clean completion with toast feedback.
            st.pending_comments.set(Vec::new());
            st.submit_summary.set(String::new());
            st.submit_open.set(false);
            ui.notify(format!("Gallery preview · dispatched ({action_kind})"));
            return;
        }

        if action_kind == "github_review" {
            let Some(ref pr) = pr_info else {
                ui.operation_error
                    .set("No GitHub PR associated with this diff".into());
                return;
            };
            let repo = pr["repository"].as_str().unwrap_or("").to_owned();
            let num = pr["number"].as_u64().unwrap_or(0);
            let url = pr["url"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("https://github.com/{repo}/pull/{num}"));
            let head = st.compare_data.get_untracked()["compare_sha"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            let gh_comments: Vec<serde_json::Value> = pending
                .iter()
                .map(|c| {
                    json!({
                        "path": c.path,
                        "line": c.line,
                        "side": c.side,
                        "body": c.body,
                    })
                })
                .collect();
            let r = GitHubReference {
                repository: repo,
                kind: "pr".into(),
                number: Some(num),
                sha: None,
                url,
            };
            dispatch_then(
                ui,
                "github.remote.review",
                json!({
                    "reference": r,
                    "head": head,
                    "body": summary,
                    "comments": gh_comments,
                    "event": st.submit_gh_event.get_untracked(),
                }),
                move |_| {
                    st.pending_comments.set(Vec::new());
                    st.submit_summary.set(String::new());
                    st.submit_open.set(false);
                    ui.notify("Review published to GitHub PR");
                },
            );
            return;
        }

        let profile_id = {
            let p = st.submit_profile.get_untracked();
            if p.is_empty() {
                default_profile_id(ui)
            } else {
                p
            }
        };
        let project_id = ui
            .current_card()
            .map(|c| c.project_id)
            .or_else(|| ui.current_project().map(|p| p.id))
            .unwrap_or_else(|| ui.project.get_untracked());
        let ctx_label = context_label_untracked(ui);

        let payload = match action_kind.as_str() {
            "card_comment" => json!({
                "mode": "card",
                "card_id": ui.card.get_untracked(),
                "summary": summary,
                "comments": inline_json,
                "context_label": ctx_label,
                "move_to_develop": false,
            }),
            "card_agent" => json!({
                "mode": "card",
                "card_id": ui.card.get_untracked(),
                "profile_id": profile_id,
                "summary": summary,
                "comments": inline_json,
                "context_label": ctx_label,
                "move_to_develop": st.submit_move_develop.get_untracked(),
            }),
            "new_card" => json!({
                "mode": "new_card",
                "project_id": project_id,
                "profile_id": profile_id,
                "title": st.submit_new_card_title.get_untracked(),
                "summary": summary,
                "comments": inline_json,
                "context_label": ctx_label,
            }),
            _ => json!({
                "mode": "terminal",
                "project_id": project_id,
                "profile_id": profile_id,
                "compare_ref": st.compare_ref.get_untracked(),
                "create_worktree": st.submit_create_worktree.get_untracked(),
                "cwd": st.compare_data.get_untracked()["cwd"].as_str(),
                "summary": summary,
                "comments": inline_json,
                "context_label": ctx_label,
            }),
        };

        dispatch_then(ui, "review.send_agent", payload, move |res| {
            st.pending_comments.set(Vec::new());
            st.submit_summary.set(String::new());
            st.submit_new_card_title.set(String::new());
            st.submit_open.set(false);
            match res["mode"].as_str().unwrap_or("") {
                "terminal" => {
                    if let Some(tid) = res["terminal_id"].as_str() {
                        let tid_owned = tid.to_owned();
                        ui.terminal_tabs.update(|tabs| {
                            if !tabs.contains(&tid_owned) {
                                tabs.push(tid_owned.clone());
                            }
                        });
                        ui.terminal_selection.set(tid_owned);
                        ui.navigate("terminals");
                    }
                }
                "new_card" => {
                    if let Some(cid) = res["card_id"].as_str() {
                        ui.card.set(cid.to_owned());
                        ui.page.set("card".into());
                    }
                }
                _ => {
                    ui.notify("Review sent to Card");
                }
            }
        });
    })
    .disabled(move || {
        ui.is_method_pending("review.send_agent")
            || ui.is_method_pending("github.remote.review")
            || (st.submit_action.get() != "github_review"
                && st.submit_summary.with(|s| s.trim().is_empty())
                && st.pending_comments.with(Vec::is_empty))
            || (st.submit_action.get() == "github_review"
                && st.submit_gh_event.get() == "COMMENT"
                && st.submit_summary.with(|s| s.trim().is_empty())
                && st.pending_comments.with(Vec::is_empty))
            || (st.submit_action.get() == "github_review"
                && st.submit_gh_event.get() == "REQUEST_CHANGES"
                && st.submit_summary.with(|s| s.trim().is_empty()))
    })
}
