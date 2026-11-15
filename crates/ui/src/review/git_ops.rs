//! Git client actions and UI widgets for the Git workbench:
//! - Active checkout / worktree / PR & Card branch switcher + remote sync controls (`Fetch`, `Pull`, `Push`)
//! - Per-branch paginated commit history & search (`git.log`)
//! - Single-commit inspection banner with merge-commit parent selector & prev/next navigation
//! - File-level staging, unstaging, discard confirmation modal, and inline commit / amend composer.
use super::{load_refs, load_refs_and_compare, open_card, refresh_active_diff, select_git_pair};
use crate::presenter::*;
use canban_ui_kit::tokens::*;
use canban_ui_kit::*;
use nsh_core::element::desktop::{MenuItem, popup_menu_at};
use nsh_core::prelude::*;
use serde_json::{Value, json};

fn active_checkout_param(ui: Ui) -> Option<String> {
    let cwd = ui.review_state.checkout_cwd.get_untracked();
    if !cwd.trim().is_empty() {
        return Some(cwd);
    }
    if ui.review_state.mode.get_untracked() == "card_local"
        && let Some(wt) = ui
            .current_card()
            .and_then(|c| c.worktree)
            .filter(|s| !s.is_empty())
    {
        return Some(wt);
    }
    None
}

fn git_params(ui: Ui, mut extra: Value) -> Option<Value> {
    let proj = ui.current_project()?;
    extra["project_id"] = json!(proj.id);
    if let Some(cwd) = active_checkout_param(ui) {
        extra["cwd"] = json!(cwd);
    }
    if !ui.card.get_untracked().is_empty() {
        extra["card_id"] = json!(ui.card.get_untracked());
    }
    Some(extra)
}

/// Switches the active checkout (Project main directory, Card worktree, or ManagedWorktree).
pub fn select_checkout(ui: Ui, cwd: String) {
    let st = ui.review_state;
    batch(move || {
        ui.card.set(String::new());
        ui.review.set(None);
        st.card_drawer_open.set(false);
        st.pr_target.set(None);
        st.mode.set("git".into());
        st.checkout_cwd.set(cwd);
        st.log_branch.set(String::new());
        st.log_commits.set(Vec::new());
        st.range_backup.set(None);
        st.selected_file.set(String::new());
        st.active_line.set(None);
        st.error.set(String::new());
    });
    load_refs_and_compare(ui, true);
}

/// Loads paginated commit history for `branch` (or `st.log_branch` / `current_branch`).
pub fn load_branch_log(ui: Ui, branch: String, append: bool) {
    let st = ui.review_state;
    let rev = if branch.trim().is_empty() {
        let cur = st.log_branch.get_untracked();
        if !cur.is_empty() {
            cur
        } else {
            st.refs_data.with_untracked(|v| {
                v["current_branch"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .or_else(|| v["default_branch"].as_str())
                    .unwrap_or("HEAD")
                    .to_owned()
            })
        }
    } else {
        branch
    };
    st.log_branch.set(rev.clone());

    let query = st.log_query.get_untracked();
    let skip = if append {
        st.log_commits.with_untracked(Vec::len)
    } else {
        0
    };

    if crate::APP.get().is_none() {
        // Gallery fallback: filter static fixture commits in memory.
        let q = query.trim().to_lowercase();
        let commits: Vec<Value> = st.refs_data.with_untracked(|v| {
            v["commits"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|c| {
                    q.is_empty()
                        || c["subject"]
                            .as_str()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&q)
                        || c["short_sha"]
                            .as_str()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&q)
                        || c["author"]
                            .as_str()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&q)
                })
                .collect()
        });
        st.log_commits.set(commits);
        st.log_has_more.set(false);
        return;
    }

    let Some(params) = git_params(
        ui,
        json!({
            "rev": rev,
            "skip": skip,
            "limit": 40,
            "query": query,
        }),
    ) else {
        return;
    };

    let ticket = st.log_ticket.get_untracked() + 1;
    st.log_ticket.set(ticket);
    st.log_loading.set(true);
    let active_proj = ui.project.get_untracked();

    ui.spawn(async move {
        let res =
            blocking(move || crate::routed_call("git.log", params, Some(active_proj.as_str())))
                .await;
        if st.log_ticket.try_get() != Some(ticket) {
            return;
        }
        st.log_loading.set(false);
        if let Ok(Ok(val)) = res {
            let page = val["commits"].as_array().cloned().unwrap_or_default();
            let has_more = val["has_more"].as_bool().unwrap_or(false);
            if append {
                st.log_commits.update(|list| list.extend(page));
            } else {
                st.log_commits.set(page);
            }
            st.log_has_more.set(has_more);
        }
    });
}

/// Inspects a single commit's diff against `parent_idx` (`0` = first parent, `1` = second merge parent).
pub fn select_single_commit(ui: Ui, sha: String, parent_idx: usize, save_range: bool) {
    let st = ui.review_state;
    let base_spec = if parent_idx == 0 {
        format!("{sha}~1")
    } else {
        format!("{sha}^{}", parent_idx + 1)
    };
    batch(move || {
        if save_range && st.range_backup.get_untracked().is_none() {
            st.range_backup.set(Some((
                st.mode.get_untracked(),
                st.base_ref.get_untracked(),
                st.compare_ref.get_untracked(),
            )));
        } else if !save_range {
            st.range_backup.set(None);
        }
        ui.review.set(None);
        st.mode.set("git".into());
        st.commit_parent_idx.set(parent_idx);
        st.base_ref.set(base_spec);
        st.compare_ref.set(sha);
        st.three_dot.set(false);
        st.selected_file.set(String::new());
        st.active_line.set(None);
    });
    refresh_active_diff(ui, false);
}

pub fn restore_range_backup(ui: Ui) {
    let st = ui.review_state;
    let Some((mode, base, cmp)) = st.range_backup.get_untracked() else {
        return;
    };
    batch(move || {
        st.range_backup.set(None);
        st.mode.set(mode);
        st.base_ref.set(base);
        st.compare_ref.set(cmp);
        st.three_dot.set(true);
        st.selected_file.set(String::new());
        st.active_line.set(None);
    });
    refresh_active_diff(ui, false);
}

fn reload_after_git_mutation(ui: Ui) {
    load_refs(ui);
    let br = ui.review_state.log_branch.get_untracked();
    load_branch_log(ui, br, false);
    refresh_active_diff(ui, false);
}

pub fn run_fetch(ui: Ui) {
    if crate::APP.get().is_none() {
        ui.notify("Fetched from origin (Gallery preview)");
        return;
    }
    let Some(params) = git_params(ui, json!({ "prune": true })) else {
        return;
    };
    dispatch_then(ui, "git.fetch", params, move |_| {
        reload_after_git_mutation(ui);
    });
}

pub fn run_pull(ui: Ui) {
    if crate::APP.get().is_none() {
        ui.review_state.refs_data.update(|v| {
            v["behind"] = json!(0);
        });
        ui.notify("Fast-forwarded current branch (Gallery preview)");
        return;
    }
    let Some(params) = git_params(ui, json!({})) else {
        return;
    };
    dispatch_then(ui, "git.pull", params, move |_| {
        reload_after_git_mutation(ui);
    });
}

pub fn run_push(ui: Ui) {
    if crate::APP.get().is_none() {
        ui.review_state.refs_data.update(|v| {
            v["ahead"] = json!(0);
        });
        ui.notify("Pushed current branch (Gallery preview)");
        return;
    }
    let Some(params) = git_params(ui, json!({})) else {
        return;
    };
    dispatch_then(ui, "git.push", params, move |_| {
        reload_after_git_mutation(ui);
    });
}

pub fn run_stage(ui: Ui, paths: Vec<String>, all: bool) {
    let st = ui.review_state;
    if crate::APP.get().is_none() {
        let update_fn = |v: &mut Value| {
            let counts = v["status_files"].as_array_mut().map(|arr| {
                for item in arr.iter_mut() {
                    let p = item["path"].as_str().unwrap_or("");
                    if all || paths.iter().any(|t| t == p) {
                        item["staged"] = json!(true);
                        item["unstaged"] = json!(false);
                        item["untracked"] = json!(false);
                    }
                }
                let sc = arr.iter().filter(|i| i["staged"] == true).count();
                let uc = arr
                    .iter()
                    .filter(|i| i["unstaged"] == true || i["untracked"] == true)
                    .count();
                (sc, uc, arr.len())
            });
            if let Some((sc, uc, total)) = counts {
                v["staged_count"] = json!(sc);
                v["unstaged_count"] = json!(uc);
                v["dirty_count"] = json!(total);
                v["dirty"] = json!(total > 0);
            }
        };
        st.refs_data.update(update_fn);
        st.compare_data.update(update_fn);
        ui.notify("Staged changes");
        return;
    }
    let Some(params) = git_params(ui, json!({ "paths": paths, "all": all })) else {
        return;
    };
    dispatch_then(ui, "git.stage", params, move |_| {
        load_refs(ui);
        refresh_active_diff(ui, false);
    });
}

pub fn run_unstage(ui: Ui, paths: Vec<String>, all: bool) {
    let st = ui.review_state;
    if crate::APP.get().is_none() {
        let update_fn = |v: &mut Value| {
            let counts = v["status_files"].as_array_mut().map(|arr| {
                for item in arr.iter_mut() {
                    let p = item["path"].as_str().unwrap_or("");
                    if all || paths.iter().any(|t| t == p) {
                        item["staged"] = json!(false);
                        item["unstaged"] = json!(true);
                    }
                }
                let sc = arr.iter().filter(|i| i["staged"] == true).count();
                let uc = arr
                    .iter()
                    .filter(|i| i["unstaged"] == true || i["untracked"] == true)
                    .count();
                (sc, uc)
            });
            if let Some((sc, uc)) = counts {
                v["staged_count"] = json!(sc);
                v["unstaged_count"] = json!(uc);
            }
        };
        st.refs_data.update(update_fn);
        st.compare_data.update(update_fn);
        ui.notify("Unstaged changes");
        return;
    }
    let Some(params) = git_params(ui, json!({ "paths": paths, "all": all })) else {
        return;
    };
    dispatch_then(ui, "git.unstage", params, move |_| {
        load_refs(ui);
        refresh_active_diff(ui, false);
    });
}

pub fn request_discard(ui: Ui, paths: Vec<String>) {
    if !paths.is_empty() {
        ui.review_state.discard_confirm.set(Some(paths));
    }
}

pub fn confirm_discard(ui: Ui) {
    let st = ui.review_state;
    let Some(paths) = st.discard_confirm.get_untracked() else {
        return;
    };
    if crate::APP.get().is_none() {
        st.discard_confirm.set(None);
        st.refs_data.update(|v| {
            if let Some(arr) = v["status_files"].as_array_mut() {
                arr.retain(|i| !paths.iter().any(|p| Some(p.as_str()) == i["path"].as_str()));
            }
        });
        st.compare_data.update(|v| {
            if let Some(arr) = v["status_files"].as_array_mut() {
                arr.retain(|i| !paths.iter().any(|p| Some(p.as_str()) == i["path"].as_str()));
            }
            if let Some(arr) = v["files"].as_array_mut() {
                arr.retain(|i| {
                    !paths
                        .iter()
                        .any(|p| Some(p.as_str()) == i["filename"].as_str())
                });
            }
        });
        ui.notify("Discarded local changes");
        return;
    }
    let Some(params) = git_params(ui, json!({ "paths": paths })) else {
        st.discard_confirm.set(None);
        return;
    };
    dispatch_then(ui, "git.discard", params, move |_| {
        st.discard_confirm.set(None);
        reload_after_git_mutation(ui);
    });
}

pub fn run_commit(ui: Ui, and_push: bool) {
    let st = ui.review_state;
    let summary = st.commit_summary.get_untracked();
    if summary.trim().is_empty() {
        ui.notify("Enter a commit summary first");
        return;
    }
    let desc = st.commit_description.get_untracked();
    let full_msg = if desc.trim().is_empty() {
        summary.trim().to_owned()
    } else {
        format!("{}\n\n{}", summary.trim(), desc.trim())
    };
    let amend = st.commit_amend.get_untracked();
    let staged_count = st
        .refs_data
        .with_untracked(|v| v["staged_count"].as_u64().unwrap_or(0));
    let dirty_count = st
        .refs_data
        .with_untracked(|v| v["dirty_count"].as_u64().unwrap_or(0));
    let stage_all = !amend && staged_count == 0 && dirty_count > 0;

    if crate::APP.get().is_none() {
        st.commit_summary.set(String::new());
        st.commit_description.set(String::new());
        st.commit_amend.set(false);
        st.refs_data.update(|v| {
            v["dirty"] = json!(false);
            v["dirty_count"] = json!(0);
            v["staged_count"] = json!(0);
            v["unstaged_count"] = json!(0);
            v["status_files"] = json!([]);
        });
        st.compare_data.update(|v| {
            v["files"] = json!([]);
            v["status_files"] = json!([]);
        });
        ui.notify(if and_push {
            "Committed & pushed (Gallery preview)"
        } else {
            "Committed changes (Gallery preview)"
        });
        return;
    }

    let Some(params) = git_params(
        ui,
        json!({
            "message": full_msg,
            "amend": amend,
            "stage_all": stage_all,
            "push": and_push,
        }),
    ) else {
        return;
    };

    dispatch_then(ui, "git.commit", params, move |val| {
        st.commit_summary.set(String::new());
        st.commit_description.set(String::new());
        st.commit_amend.set(false);
        if let Some(push_err) = val["push_error"].as_str().filter(|s| !s.is_empty()) {
            ui.operation_error
                .set(format!("Commit succeeded, but push failed: {push_err}"));
        }
        reload_after_git_mutation(ui);
    });
}

/// Top bar checkout, worktree, and PR / Card branch selector with fuzzy search.
pub fn render_checkout_picker(ui: Ui) -> impl IntoElement {
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
        .accessibility_label("Switch active Git checkout, worktree, or PR / Card branch")
        .child(icon(Icon::Folder).w(px(12.)).h(px(12.)))
        .child(
            label(move || {
                let mode = st.mode.get();
                if mode != "git"
                    && let Some(c) = ui.current_card()
                {
                    let title_short = if c.title.chars().count() > 18 {
                        let s: String = c.title.chars().take(16).collect();
                        format!("{s}…")
                    } else {
                        c.title.clone()
                    };
                    if mode == "github_pr"
                        && let Some(pr) = st.pr_target.get()
                    {
                        let n = pr["number"].as_u64().unwrap_or(c.number);
                        return format!("PR #{n} · {title_short}");
                    }
                    return format!("#{} {title_short}", c.number);
                }
                let refs = st.refs_data.get();
                let cur_br = refs["current_branch"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("detached");
                let cwd = st.checkout_cwd.get();
                let checkouts = refs["checkouts"].as_array().cloned().unwrap_or_default();
                let matched_kind = checkouts
                    .iter()
                    .find(|c| {
                        let p = c["path"].as_str().unwrap_or("");
                        if cwd.is_empty() {
                            c["kind"].as_str() == Some("project")
                        } else {
                            p == cwd
                        }
                    })
                    .and_then(|c| c["kind"].as_str())
                    .unwrap_or("project");
                let tag = match matched_kind {
                    "card" => " · card",
                    "worktree" => " · wt",
                    _ => "",
                };
                let shown = if cur_br.chars().count() > 20 {
                    let s: String = cur_br.chars().take(18).collect();
                    format!("{s}…")
                } else {
                    cur_br.to_owned()
                };
                format!("{shown}{tag}")
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
            let mode = st.mode.get();
            let cur_cwd = st.checkout_cwd.get();
            let cur_card = ui.card.get();
            let active_pid = ui.project.get();

            let mut checkouts = refs["checkouts"].as_array().cloned().unwrap_or_default();
            if checkouts.is_empty()
                && let Some(proj) = ui.current_project()
            {
                checkouts.push(json!({
                    "kind": "project",
                    "label": format!("{} (main)", proj.name),
                    "path": proj.directory,
                    "branch": proj.target_branch,
                }));
            }

            let mut options = Vec::<PickerOption>::new();

            // 1. Checkouts & Worktrees (main repo + standalone ManagedWorktrees)
            for c in &checkouts {
                let kind = c["kind"].as_str().unwrap_or("project");
                if kind == "card" {
                    continue;
                }
                let path = c["path"].as_str().unwrap_or("").to_owned();
                let lbl = c["label"].as_str().unwrap_or("Checkout").to_owned();
                let br = c["branch"].as_str().unwrap_or("").to_owned();
                let is_cur = mode == "git"
                    && if cur_cwd.is_empty() {
                        kind == "project"
                    } else {
                        path == cur_cwd
                    };
                let target_id = if kind == "project" {
                    "cwd:".to_owned()
                } else {
                    format!("cwd:{path}")
                };
                let ic = if kind == "worktree" {
                    Icon::Branch
                } else {
                    Icon::Projects
                };
                options.push(
                    PickerOption::new(target_id, lbl)
                        .group("Checkouts & Worktrees")
                        .detail(br.clone())
                        .icon(ic)
                        .selected(is_cur)
                        .keywords([br, path, kind.to_owned()]),
                );
            }

            // 2. PRs & Card Branches
            let cards = ui.snapshot.with(|s| {
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
                    .collect::<Vec<_>>()
            });

            for c in cards {
                let is_sel = mode != "git" && cur_card == c.id;
                let is_pr = c.is_github_pr() || c.pull_request.is_some();
                let pr_num = c
                    .pull_request
                    .as_ref()
                    .map(|p| p.number)
                    .or_else(|| c.github_issue().map(|g| g.number))
                    .unwrap_or(0);
                let title = if is_pr && pr_num > 0 && pr_num != c.number {
                    format!("#{} · PR #{} {}", c.number, pr_num, c.title)
                } else if is_pr {
                    format!("PR #{} {}", c.number, c.title)
                } else {
                    format!("#{} {}", c.number, c.title)
                };
                let br = c
                    .branch
                    .clone()
                    .or_else(|| c.pull_request.as_ref().map(|p| p.head_branch.clone()))
                    .unwrap_or_default();
                let detail = if !br.is_empty() {
                    format!("{br} · {}", c.stage)
                } else {
                    c.stage.clone()
                };
                let mut kw = vec![
                    format!("#{}", c.number),
                    c.number.to_string(),
                    c.stage.clone(),
                ];
                if !br.is_empty() {
                    kw.push(br);
                }
                if pr_num > 0 {
                    kw.push(format!("PR #{pr_num}"));
                    kw.push(pr_num.to_string());
                }
                if let Some(wt) = &c.worktree {
                    kw.push(wt.clone());
                }
                options.push(
                    PickerOption::new(format!("card:{}", c.id), title)
                        .group("Cards & PRs")
                        .detail(detail)
                        .icon(if is_pr { Icon::GitHub } else { Icon::Board })
                        .selected(is_sel)
                        .keywords(kw),
                );
            }

            options
        },
        move |selected_id| {
            if let Some(cid) = selected_id.strip_prefix("card:") {
                open_card(ui, cid);
            } else if let Some(cwd) = selected_id.strip_prefix("cwd:") {
                select_checkout(ui, cwd.to_owned());
            }
        },
    )
    .placeholder("Search checkouts, PRs, #card, or branch…")
    .empty_text("No matching checkouts or PR / Card branches")
    .width(360.)
    .list_height(300.)
}

/// Top bar remote Git sync actions: `Fetch`, `Pull ↓M` (`--ff-only`), and `Push ↑N`.
pub fn render_sync_controls(ui: Ui) -> impl IntoElement {
    let st = ui.review_state;
    dynamic(move || {
        if st.mode.get() == "github_pr" {
            return view().into_element();
        }
        let refs = st.refs_data.get();
        let has_remote = refs["has_remote"].as_bool().unwrap_or(false);
        if !has_remote {
            return view().into_element();
        }
        let ahead = refs["ahead"].as_u64().unwrap_or(0);
        let behind = refs["behind"].as_u64().unwrap_or(0);
        let has_upstream = refs["upstream"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty());
        let on_branch = refs["current_branch"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty());

        let pull_label = if behind > 0 {
            format!("Pull ↓{behind}")
        } else {
            "Pull".to_owned()
        };
        let push_label = if !has_upstream && on_branch {
            "Push ↑ (publish)".to_owned()
        } else if ahead > 0 {
            format!("Push ↑{ahead}")
        } else {
            "Push".to_owned()
        };

        view()
            .flex_row()
            .items_center()
            .gap_1()
            .child(
                compact_action("Fetch", Variant::Quiet, move |_| run_fetch(ui))
                    .disabled(move || ui.is_method_pending("git.fetch")),
            )
            .child(
                compact_action(
                    pull_label,
                    if behind > 0 {
                        Variant::Secondary
                    } else {
                        Variant::Quiet
                    },
                    move |_| run_pull(ui),
                )
                .disabled(move || ui.is_method_pending("git.pull") || !on_branch),
            )
            .child(
                compact_action(
                    push_label,
                    if ahead > 0 || (!has_upstream && on_branch) {
                        Variant::Secondary
                    } else {
                        Variant::Quiet
                    },
                    move |_| run_push(ui),
                )
                .disabled(move || ui.is_method_pending("git.push") || !on_branch),
            )
            .into_element()
    })
}

/// Renders the full commit metadata header card when inspecting a single commit's diff.
pub fn render_commit_detail_card(ui: Ui, detail: &Value) -> AnyElement {
    let st = ui.review_state;
    let sha = detail["sha"].as_str().unwrap_or("").to_owned();
    if sha.is_empty() {
        return view().into_element();
    }
    let short = detail["short_sha"]
        .as_str()
        .unwrap_or(&sha[..sha.len().min(7)])
        .to_owned();
    let author = detail["author"].as_str().unwrap_or("Author").to_owned();
    let email = detail["email"].as_str().unwrap_or("").to_owned();
    let rel_time = detail["relative_time"].as_str().unwrap_or("").to_owned();
    let date_iso = detail["date_iso"].as_str().unwrap_or("").to_owned();
    let subject = detail["subject"].as_str().unwrap_or("").to_owned();
    let body = detail["body"].as_str().unwrap_or("").trim().to_owned();
    let active_parent = detail["parent_index"].as_u64().unwrap_or(0) as usize;
    let parents = detail["parents"].as_array().cloned().unwrap_or_default();

    // Find prev/next commits in `log_commits` (or fallback to `refs_data["commits"]`).
    let nav_list = {
        let lc = st.log_commits.get();
        if lc.is_empty() {
            st.refs_data
                .with(|v| v["commits"].as_array().cloned().unwrap_or_default())
        } else {
            lc
        }
    };
    let pos = nav_list
        .iter()
        .position(|c| c["sha"].as_str() == Some(sha.as_str()));
    let newer_sha = pos
        .and_then(|idx| idx.checked_sub(1))
        .and_then(|i| nav_list.get(i))
        .and_then(|c| c["sha"].as_str())
        .map(str::to_owned);
    let older_sha = pos
        .and_then(|idx| nav_list.get(idx + 1))
        .and_then(|c| c["sha"].as_str())
        .map(str::to_owned);
    let has_backup = st.range_backup.get().is_some();

    let copy_sha = sha.clone();
    let mut top_actions = view().flex_row().flex_wrap().items_center().gap_1p5();
    if has_backup {
        top_actions = top_actions.child(compact_action(
            "← Back to range",
            Variant::Secondary,
            move |_| restore_range_backup(ui),
        ));
    }
    top_actions = top_actions.child(compact_action("Copy SHA", Variant::Quiet, move |_| {
        copy_text(ui, copy_sha.clone());
    }));
    if let Some(n_sha) = newer_sha {
        let save = has_backup;
        top_actions = top_actions.child(compact_action("← Newer", Variant::Quiet, move |_| {
            select_single_commit(ui, n_sha.clone(), 0, save);
        }));
    }
    if let Some(o_sha) = older_sha {
        let save = has_backup;
        top_actions = top_actions.child(compact_action("Older →", Variant::Quiet, move |_| {
            select_single_commit(ui, o_sha.clone(), 0, save);
        }));
    }

    let author_line = if email.is_empty() {
        format!("{author} · {rel_time}")
    } else {
        format!("{author} <{email}> · {rel_time}")
    };

    let mut card = view()
        .p_3()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(BORDER)
        .bg(SURFACE)
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
                        .items_center()
                        .gap_2()
                        .child(icon(Icon::History).w(px(13.)).h(px(13.)))
                        .child(
                            label(format!("Commit {short}"))
                                .font_family("Menlo")
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ACCENT),
                        )
                        .child(if parents.len() > 1 {
                            badge("Merge commit").into_element()
                        } else if parents.is_empty() {
                            badge("Root commit").into_element()
                        } else {
                            view().into_element()
                        }),
                )
                .child(top_actions),
        )
        .child(
            label(subject)
                .text_size(px(13.5))
                .font_weight(FontWeight::SEMIBOLD),
        );

    if !body.is_empty() {
        card = card.child(
            view()
                .p_2()
                .rounded(px(5.))
                .bg(INSET)
                .child(label(body).font_family("Menlo").text_size(px(11.5))),
        );
    }

    let mut meta_row = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_2()
        .child(muted(author_line).text_size(px(11.5)));

    if !date_iso.is_empty() {
        meta_row = meta_row.child(muted(date_iso).font_family("Menlo").text_size(px(10.5)));
    }
    card = card.child(meta_row);

    if parents.len() > 1 {
        let mut parent_bar = view().flex_row().flex_wrap().items_center().gap_1p5();
        parent_bar = parent_bar.child(muted("Diff against parent:").text_size(px(11.)));
        for p in parents {
            let idx = p["index"].as_u64().unwrap_or(0) as usize;
            let p_short = p["short_sha"].as_str().unwrap_or("").to_owned();
            let p_subj: String = p["subject"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(28)
                .collect();
            let c_sha = sha.clone();
            let save = has_backup;
            let btn_lbl = if p_subj.is_empty() {
                format!("Parent {} ({p_short})", idx + 1)
            } else {
                format!("Parent {} ({p_short} · {p_subj})", idx + 1)
            };
            parent_bar = parent_bar.child(compact_action(
                btn_lbl,
                if idx == active_parent {
                    Variant::Primary
                } else {
                    Variant::Quiet
                },
                move |_| select_single_commit(ui, c_sha.clone(), idx, save),
            ));
        }
        card = card.child(parent_bar);
    }

    card.into_element()
}

/// Renders the Working Tree left pane: Staged Changes, Unstaged/Untracked Changes,
/// per-file `+` / `−` / context menu actions, and the inline Commit / Amend composer.
pub fn render_working_tree_pane(ui: Ui, data: &Value) -> AnyElement {
    let st = ui.review_state;
    let selected = st.selected_file.get();
    let pending_all = st.pending_comments.get();
    let files = data["files"].as_array().cloned().unwrap_or_default();
    let total_add = data["total_additions"].as_u64().unwrap_or(0);
    let total_del = data["total_deletions"].as_u64().unwrap_or(0);

    let status_files: Vec<Value> = data["status_files"]
        .as_array()
        .cloned()
        .or_else(|| st.refs_data.with(|v| v["status_files"].as_array().cloned()))
        .unwrap_or_default();

    let mut staged_items = Vec::<Value>::new();
    let mut unstaged_items = Vec::<Value>::new();
    for sf in &status_files {
        if sf["staged"].as_bool().unwrap_or(false) {
            staged_items.push(sf.clone());
        }
        if sf["unstaged"].as_bool().unwrap_or(false) || sf["untracked"].as_bool().unwrap_or(false) {
            unstaged_items.push(sf.clone());
        }
    }

    let mut list = view().gap_2().p_2();

    // All files filter pill
    let all_selected = selected.is_empty();
    list = list.child(
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
            .on_press(move |_: &()| {
                if matches!(
                    st.compare_ref.get_untracked().as_str(),
                    "STAGED" | "UNSTAGED"
                ) {
                    select_git_pair(ui, "HEAD", "WORKDIR");
                } else {
                    st.selected_file.set(String::new());
                }
            })
            .child(
                label(format!("All changes ({})", files.len()))
                    .text_size(px(12.))
                    .font_weight(if all_selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    }),
            )
            .child(
                muted(format!("+{total_add} −{total_del}"))
                    .font_family("Menlo")
                    .text_size(px(10.5)),
            ),
    );

    // Staged section
    let staged_len = staged_items.len();
    let mut staged_grp = view().gap_0p5();
    staged_grp = staged_grp.child(
        view()
            .px_1p5()
            .py_0p5()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                muted(format!("Staged ({staged_len})"))
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(if staged_len > 0 {
                compact_action("− Unstage All", Variant::Quiet, move |_| {
                    run_unstage(ui, Vec::new(), true);
                })
                .into_element()
            } else {
                view().into_element()
            }),
    );
    if staged_items.is_empty() {
        staged_grp = staged_grp.child(
            view()
                .px_2()
                .py_1()
                .child(muted("No staged files").text_size(px(11.))),
        );
    } else {
        for sf in staged_items {
            let fname = sf["path"].as_str().unwrap_or("").to_owned();
            let code = sf["index_code"].as_str().unwrap_or("M").to_owned();
            let pending_cnt = pending_all.iter().filter(|c| c.path == fname).count();
            staged_grp = staged_grp.child(working_file_row(
                ui,
                fname,
                code,
                true,
                selected.clone(),
                pending_cnt,
            ));
        }
    }
    list = list.child(staged_grp);

    // Unstaged & Untracked section
    let unstaged_len = unstaged_items.len();
    let mut unstaged_grp = view().gap_0p5();
    unstaged_grp = unstaged_grp.child(
        view()
            .px_1p5()
            .py_0p5()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                muted(format!("Changes ({unstaged_len})"))
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(if unstaged_len > 0 {
                compact_action("+ Stage All", Variant::Quiet, move |_| {
                    run_stage(ui, Vec::new(), true);
                })
                .into_element()
            } else {
                view().into_element()
            }),
    );
    if unstaged_items.is_empty() {
        unstaged_grp = unstaged_grp.child(
            view()
                .px_2()
                .py_1()
                .child(muted("Working tree clean").text_size(px(11.))),
        );
    } else {
        for sf in unstaged_items {
            let fname = sf["path"].as_str().unwrap_or("").to_owned();
            let code = if sf["untracked"].as_bool().unwrap_or(false) {
                "U".to_owned()
            } else {
                sf["worktree_code"].as_str().unwrap_or("M").to_owned()
            };
            let pending_cnt = pending_all.iter().filter(|c| c.path == fname).count();
            unstaged_grp = unstaged_grp.child(working_file_row(
                ui,
                fname,
                code,
                false,
                selected.clone(),
                pending_cnt,
            ));
        }
    }
    list = list.child(unstaged_grp);

    view()
        .w(px(268.))
        .flex_shrink_0()
        .min_h(px(0.))
        .border_r_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .child(scroll().flex_1().min_h(px(0.)).child(list))
        .child(render_commit_composer(ui, staged_len, unstaged_len))
        .into_element()
}

fn working_file_row(
    ui: Ui,
    fname: String,
    code: String,
    is_staged_row: bool,
    selected: String,
    pending_cnt: usize,
) -> impl IntoElement {
    let st = ui.review_state;
    let is_sel = selected == fname;
    let short_name = fname.rsplit('/').next().unwrap_or(&fname).to_owned();
    let badge_col = match code.as_str() {
        "A" | "U" => SUCCESS,
        "D" => DANGER,
        "R" | "C" => ACCENT,
        _ => MUTED,
    };

    let click_path = fname.clone();
    let action_path = fname.clone();
    let menu_path = fname.clone();

    view()
        .tooltip(hint_tooltip(fname.clone()))
        .flex_row()
        .items_center()
        .gap_0p5()
        .px_1()
        .rounded(px(5.))
        .bg(if is_sel { SELECTED } else { CLEAR })
        .hover(|s| s.bg(if is_sel { SELECTED } else { CARD_HOVER }))
        .on_context_menu(move |ev: &nsh_core::ContextMenuEvent| {
            let p = menu_path.clone();
            let pos = ev.page_position;
            ui.spawn(async move {
                let p_stage = p.clone();
                let p_discard = p.clone();
                let p_copy = p.clone();
                let items = vec![
                    if is_staged_row {
                        MenuItem::new("Unstage file").on_select(move || {
                            run_unstage(ui, vec![p_stage.clone()], false);
                        })
                    } else {
                        MenuItem::new("Stage file").on_select(move || {
                            run_stage(ui, vec![p_stage.clone()], false);
                        })
                    },
                    MenuItem::new("Discard changes…").on_select(move || {
                        request_discard(ui, vec![p_discard.clone()]);
                    }),
                    MenuItem::separator(),
                    MenuItem::new("Copy relative path").on_select(move || {
                        copy_text(ui, p_copy.clone());
                    }),
                ];
                popup_menu_at(items, pos).await;
            });
        })
        .child(
            canban_ui_kit::press_target()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(26.))
                .px_1p5()
                .py_0p5()
                .flex_row()
                .items_center()
                .gap_1p5()
                .on_press(move |_: &()| st.selected_file.set(click_path.clone()))
                .child(
                    label(code)
                        .font_family("Menlo")
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(badge_col),
                )
                .child(
                    label(short_name)
                        .text_size(px(12.))
                        .truncate()
                        .flex_1()
                        .min_w(px(0.)),
                )
                .child(if pending_cnt > 0 {
                    status(pending_cnt.to_string(), Tone::Info).into_element()
                } else {
                    view().into_element()
                }),
        )
        .child(compact_action(
            if is_staged_row { "−" } else { "+" },
            Variant::Quiet,
            move |_| {
                if is_staged_row {
                    run_unstage(ui, vec![action_path.clone()], false);
                } else {
                    run_stage(ui, vec![action_path.clone()], false);
                }
            },
        ))
}

fn render_commit_composer(ui: Ui, staged_len: usize, unstaged_len: usize) -> impl IntoElement {
    let st = ui.review_state;
    let desc_focused = signal(false);

    let has_remote = st
        .refs_data
        .with(|v| v["has_remote"].as_bool().unwrap_or(false));
    let on_branch = st.refs_data.with(|v| {
        v["current_branch"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
    });

    let primary_label = move || {
        let amend = st.commit_amend.get();
        if amend {
            "Amend Commit".to_owned()
        } else if staged_len > 0 {
            format!("Commit ({staged_len})")
        } else if unstaged_len > 0 {
            format!("Stage All & Commit ({unstaged_len})")
        } else {
            "Commit".to_owned()
        }
    };

    let can_commit = move || {
        let has_msg = st.commit_summary.with(|s| !s.trim().is_empty());
        let amend = st.commit_amend.get();
        has_msg
            && (amend || staged_len > 0 || unstaged_len > 0)
            && !ui.is_method_pending("git.commit")
    };

    view()
        .p_2p5()
        .border_t_1()
        .border_color(BORDER)
        .bg(INSET)
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    label("Commit")
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    canban_ui_kit::press_target()
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(4.))
                        .hover(|s| s.bg(CARD_HOVER))
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .on_press(move |_: &()| {
                            let next = !st.commit_amend.get_untracked();
                            st.commit_amend.set(next);
                            if next && st.commit_summary.get_untracked().trim().is_empty() {
                                let (subj, body) = st.refs_data.with_untracked(|v| {
                                    (
                                        v["last_commit_subject"].as_str().unwrap_or("").to_owned(),
                                        v["last_commit_body"].as_str().unwrap_or("").to_owned(),
                                    )
                                });
                                st.commit_summary.set(subj);
                                if st.commit_description.get_untracked().trim().is_empty() {
                                    st.commit_description.set(body);
                                }
                            }
                        })
                        .child(
                            label(move || {
                                if st.commit_amend.get() {
                                    "☑ Amend"
                                } else {
                                    "☐ Amend"
                                }
                            })
                            .text_size(px(11.))
                            .text_color(move || if st.commit_amend.get() { ACCENT } else { MUTED }),
                        ),
                ),
        )
        .child(
            field(st.commit_summary, "Commit summary (required)").on_submit(move |_: &str| {
                if can_commit() {
                    run_commit(ui, false);
                }
            }),
        )
        .child(
            view()
                .h(px(56.))
                .min_w(px(0.))
                .rounded(px(RADIUS))
                .border_1()
                .border_color(move || if desc_focused.get() { ACCENT } else { BORDER })
                .bg(BACKGROUND)
                .overflow_hidden()
                .child(
                    nsh_base::editor()
                        .value(move || Some(st.commit_description.get().into()))
                        .on_change(move |v: &SharedString| st.commit_description.set(v.to_string()))
                        .on_focus(move |_: &()| desc_focused.set(true))
                        .on_blur(move |_: &()| desc_focused.set(false))
                        .accessibility_label("Commit description")
                        .placeholder("Extended description (optional)…")
                        .line_numbers(false)
                        .folding(false)
                        .soft_wrap(true)
                        .active_line(false)
                        .indent_guides(false)
                        .auto_close(false)
                        .auto_indent(false)
                        .font_size(px(12.))
                        .line_height(px(17.))
                        .text_color(TEXT)
                        .px_2()
                        .py_1()
                        .size_full(),
                ),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_1p5()
                .child(
                    compact_action(primary_label, Variant::Primary, move |_| {
                        run_commit(ui, false);
                    })
                    .disabled(move || !can_commit()),
                )
                .child(if has_remote && on_branch {
                    compact_action("Commit & Push", Variant::Secondary, move |_| {
                        run_commit(ui, true);
                    })
                    .disabled(move || !can_commit() || ui.is_method_pending("git.push"))
                    .into_element()
                } else {
                    view().into_element()
                }),
        )
}

/// Modal dialog confirming destructive working-tree file discard.
pub fn render_discard_modal(ui: Ui) -> impl IntoElement {
    let st = ui.review_state;
    when(
        move || st.discard_confirm.get().is_some(),
        move || {
            let paths = st.discard_confirm.get_untracked().unwrap_or_default();
            let summary = if paths.len() == 1 {
                format!(
                    "Permanently discard all local edits to `{}`? Uncommitted changes cannot be recovered.",
                    paths[0]
                )
            } else {
                format!(
                    "Permanently discard all local edits to {} files? Uncommitted changes cannot be recovered.",
                    paths.len()
                )
            };
            let close_modal = move || st.discard_confirm.set(None);

            nsh_base::dialog()
                .animation(canban_ui_kit::motion::modal)
                .close_on_escape(true)
                .close_on_backdrop_press(true)
                .on_cancel(close_modal)
                .child(
                    view()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x00000055))
                        .child(
                            panel()
                                .w(px(410.))
                                .max_w(Percentage(0.95))
                                .border_1()
                                .border_color(BORDER)
                                .gap_3()
                                .child(
                                    label("Discard Local Changes?")
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(notice(summary, Tone::Danger))
                                .child(
                                    view()
                                        .flex_row()
                                        .justify_end()
                                        .gap_2()
                                        .child(action("Cancel", Variant::Quiet, move |_| {
                                            close_modal()
                                        }))
                                        .child(
                                            action(
                                                "Discard Changes",
                                                Variant::Primary,
                                                move |_| confirm_discard(ui),
                                            )
                                            .disabled(move || ui.is_method_pending("git.discard")),
                                        ),
                                ),
                        ),
                )
        },
    )
}
