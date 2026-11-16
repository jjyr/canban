//! Unified Git Workbench:
//! - Works as a standalone Git client & compare workbench from the Left Rail even without any Card or PR.
//! - Opens directly from Local Cards (worktree/branch) and GitHub PR Cards (zero-clone remote PR diff).
//! - Supports inline line-by-line review comments + batch dispatch to @Agent (Terminals or Card) or GitHub PR.
pub mod cache;
pub mod diff;
pub mod git_ops;
pub mod model;
pub mod sidebar;
pub mod submit;

pub use model::{LineTarget, PendingInlineComment, State};

use crate::presenter::*;
use canban_types::{Card, DeliveryMode, MergeStrategy, ReviewPreview, WorkflowStep};
use canban_ui_kit::tokens::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileDiff {
    path: String,
    additions: usize,
    deletions: usize,
    patch: String,
}

fn parse_patch(patch: &str, files: &[String]) -> Vec<FileDiff> {
    let mut chunks = Vec::new();
    let mut current_path = String::new();
    let mut current_lines = Vec::new();
    let mut additions = 0usize;
    let mut deletions = 0usize;

    let flush = |chunks: &mut Vec<FileDiff>,
                 path: &mut String,
                 lines: &mut Vec<String>,
                 add: &mut usize,
                 del: &mut usize| {
        if !lines.is_empty() {
            let name = if path.is_empty() {
                files
                    .get(chunks.len())
                    .cloned()
                    .unwrap_or_else(|| format!("file-{}", chunks.len() + 1))
            } else {
                path.clone()
            };
            chunks.push(FileDiff {
                path: name,
                additions: *add,
                deletions: *del,
                patch: lines.join("\n"),
            });
            path.clear();
            lines.clear();
            *add = 0;
            *del = 0;
        }
    };

    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            flush(
                &mut chunks,
                &mut current_path,
                &mut current_lines,
                &mut additions,
                &mut deletions,
            );
            current_path = rest
                .split_whitespace()
                .last()
                .and_then(|b| b.strip_prefix("b/").or(Some(b)))
                .unwrap_or(rest)
                .to_owned();
        } else if current_path.is_empty()
            && let Some(p) = line.strip_prefix("+++ b/")
        {
            current_path = p.trim().to_owned();
        }
        if line.starts_with('+') && !line.starts_with("+++") {
            additions += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            deletions += 1;
        }
        current_lines.push(line.to_owned());
    }
    flush(
        &mut chunks,
        &mut current_path,
        &mut current_lines,
        &mut additions,
        &mut deletions,
    );
    chunks
}

/// Retained public component for Gallery `controls` scene compatibility.
pub fn diff_panel(p: ReviewPreview) -> impl IntoElement {
    let parsed = parse_patch(&p.patch, &p.files);
    let mut body = panel().gap_3().child(
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                label(format!(
                    "{} → {} · {} file{}",
                    &p.card_head[..p.card_head.len().min(10)],
                    p.target_branch,
                    p.files.len(),
                    if p.files.len() == 1 { "" } else { "s" }
                ))
                .font_weight(FontWeight::SEMIBOLD),
            )
            .child(muted(p.files.join(" · "))),
    );
    if let Some(reason) = p.reason {
        body = body.child(notice(reason, Tone::Attention));
    }
    if let Some(warning) = p.warning {
        body = body.child(notice(warning, Tone::Info));
    }
    if parsed.is_empty() {
        return body.child(muted("No file changes between these commits."));
    }
    let folded = signal(std::collections::BTreeSet::<String>::new());
    for file in parsed {
        let path = file.path.clone();
        let toggle_path = path.clone();
        let open_path = path.clone();
        let fence = "`".repeat(
            file.patch
                .split(|c| c != '`')
                .map(str::len)
                .max()
                .unwrap_or(0)
                .max(2)
                + 1,
        );
        let code = format!("{fence}diff\n{}\n{fence}", file.patch);
        let stats = format!("+{} −{}", file.additions, file.deletions);
        body = body.child(
            view()
                .rounded(px(RADIUS))
                .border_1()
                .border_color(BORDER)
                .bg(INSET)
                .overflow_hidden()
                .child(
                    canban_ui_kit::press_target()
                        .w_full()
                        .min_h(px(32.))
                        .px_3()
                        .py_1()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .bg(SURFACE)
                        .hover(|s| s.bg(CARD_HOVER))
                        .focus(|s| s.border_1().border_color(ACCENT))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .min_w(px(0.))
                                .flex_1()
                                .child(muted(move || {
                                    if folded.with(|s| s.contains(&open_path)) {
                                        "▸"
                                    } else {
                                        "▾"
                                    }
                                }))
                                .child(
                                    label(path.clone())
                                        .font_weight(FontWeight::MEDIUM)
                                        .truncate()
                                        .min_w(px(0.)),
                                ),
                        )
                        .child(muted(stats))
                        .on_press(move |_: &()| {
                            folded.update(|set| {
                                if !set.remove(&toggle_path) {
                                    set.insert(toggle_path.clone());
                                }
                            });
                        }),
                )
                .child(when(
                    move || !folded.with(|s| s.contains(&path)),
                    move || {
                        view()
                            .p_2()
                            .child(nsh_base::text_view().markdown(code.clone()))
                    },
                )),
        );
    }
    body
}

pub fn preview_to_compare_json(p: &ReviewPreview) -> Value {
    let parsed = parse_patch(&p.patch, &p.files);
    let mut total_add = 0usize;
    let mut total_del = 0usize;
    let files: Vec<Value> = parsed
        .into_iter()
        .map(|f| {
            total_add += f.additions;
            total_del += f.deletions;
            json!({
                "filename": f.path,
                "status": "modified",
                "additions": f.additions,
                "deletions": f.deletions,
                "patch": f.patch,
            })
        })
        .collect();
    json!({
        "base_ref": p.target_branch,
        "compare_ref": "WORKDIR",
        "base_sha": p.target_head,
        "compare_sha": p.card_head,
        "three_dot": true,
        "files": files,
        "commits": [],
        "total_additions": total_add,
        "total_deletions": total_del,
        "truncated": p.truncated,
        "cwd": p.worktree,
    })
}

fn card_pr_info(c: &Card) -> Option<(String, u64, String)> {
    if c.is_github_pr()
        && let Some(src) = c.github_issue()
    {
        return Some((src.repository.clone(), src.number, src.url.clone()));
    }
    c.pull_request
        .as_ref()
        .map(|pr| (pr.repository.clone(), pr.number, pr.url.clone()))
}

pub fn open_compare(ui: Ui) {
    let st = ui.review_state;
    let expected_context = cache::key(
        &ui.active_project_host(),
        &ui.project.get_untracked(),
        "checkout",
        &json!(""),
    );
    let choose_default = !st.read_context.with(|v| *v == expected_context)
        || st.mode.get_untracked() != "git"
        || !st.checkout_cwd.get_untracked().is_empty()
        || st.base_ref.get_untracked().is_empty()
        || st.compare_ref.get_untracked().is_empty();
    batch(move || {
        ui.card.set(String::new());
        ui.review.set(None);
        st.card_drawer_open.set(false);
        st.pr_target.set(None);
        st.checkout_cwd.set(String::new());
        if choose_default {
            st.log_branch.set(String::new());
            st.log_commits.set(Vec::new());
        }
        st.range_backup.set(None);
        st.mode.set("git".into());
        st.selected_file.set(String::new());
        st.visible_file_limit.set(4);
        st.active_line.set(None);
        st.editing_remote_comment.set(None);
        st.remote_edit_draft.set(String::new());
        st.remote_edit_error.set(String::new());
        st.deleting_remote_comment.set(None);
        st.remote_delete_error.set(String::new());
        st.error.set(String::new());
        ui.navigate("review");
    });
    load_refs_and_compare(ui, choose_default);
}

pub fn open_card(ui: Ui, card_id: &str) {
    let Some(card) = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card_id).cloned())
    else {
        open_compare(ui);
        return;
    };
    let st = ui.review_state;
    let project = ui
        .snapshot
        .with_untracked(|s| s.projects.iter().find(|p| p.id == card.project_id).cloned());
    let target_branch = project
        .as_ref()
        .and_then(|p| p.target_branch.clone())
        .unwrap_or_else(|| "main".into());
    let delivery_default = if card.delivery_mode.clone().unwrap_or_else(|| {
        project
            .as_ref()
            .map(|p| p.delivery_mode.clone())
            .unwrap_or_default()
    }) == DeliveryMode::GitHubPr
    {
        "github_pr"
    } else {
        "local"
    };
    let strategy_default = ui.snapshot.with_untracked(|s| {
        for run in &s.workflow_runs {
            if run.card_id == card.id
                && run.active()
                && let Some(step_id) = &run.step
                && let Some(WorkflowStep::Deliver { strategy, .. }) =
                    run.definition.steps.get(step_id)
            {
                return strategy.as_str();
            }
        }
        for src in s
            .workflow_sources
            .iter()
            .filter(|src| src.project_id == card.project_id && src.enabled)
        {
            if let Some(rev) = s
                .workflow_revisions
                .iter()
                .find(|r| r.workflow_id == src.workflow_id && r.revision == src.revision)
                && let Some((_, WorkflowStep::Deliver { strategy, .. })) = rev
                    .definition
                    .steps
                    .iter()
                    .find(|(_, st)| matches!(st, WorkflowStep::Deliver { .. }))
            {
                return strategy.as_str();
            }
        }
        MergeStrategy::Rebase.as_str()
    });

    let pr = card_pr_info(&card);
    let has_local = card.worktree.is_some() || card.branch.is_some();

    batch(move || {
        ui.project.set(card.project_id.clone());
        ui.card.set(card.id.clone());
        ui.review.set(None);
        st.checkout_cwd
            .set(card.worktree.clone().unwrap_or_default());
        st.log_branch.set(card.branch.clone().unwrap_or_default());
        st.range_backup.set(None);
        st.selected_file.set(String::new());
        st.visible_file_limit.set(4);
        st.active_line.set(None);
        st.editing_remote_comment.set(None);
        st.remote_edit_draft.set(String::new());
        st.remote_edit_error.set(String::new());
        st.deleting_remote_comment.set(None);
        st.remote_delete_error.set(String::new());
        st.error.set(String::new());
        st.delivery_mode.set(delivery_default.into());
        st.delivery_strategy.set(strategy_default.into());
        st.base_ref.set(target_branch.clone());
        if let Some((repo, num, url)) = pr {
            st.pr_target.set(Some(json!({
                "repository": repo,
                "number": num,
                "url": url,
            })));
            if has_local {
                st.mode.set("card_local".into());
                st.compare_ref
                    .set(card.branch.clone().unwrap_or_else(|| "WORKDIR".into()));
            } else {
                st.mode.set("github_pr".into());
                st.compare_ref.set(format!("PR #{num}"));
            }
        } else {
            st.pr_target.set(None);
            st.mode.set("card_local".into());
            st.compare_ref
                .set(card.branch.clone().unwrap_or_else(|| "WORKDIR".into()));
        }
        ui.navigate("review");
    });
    prepare_context(ui);
    load_refs(ui);
    refresh_active_diff(ui, false);
}

/// Invalidate results and tickets together so requests predating a Git change
/// cannot update the screen or refill the cache.
pub fn invalidate_reads(ui: Ui) {
    let st = ui.review_state;
    st.read_cache.with(|c| c.invalidate());
    cancel_reads(st);
}

fn cancel_reads(st: State) {
    st.refs_ticket.update(|n| *n += 1);
    st.log_ticket.update(|n| *n += 1);
    st.compare_ticket.update(|n| *n += 1);
    st.refs_request.replace(String::new());
    st.log_request.replace(String::new());
    st.refs_loading.set(false);
    st.log_loading.set(false);
    st.compare_loading.set(false);
}

fn prepare_context(ui: Ui) {
    if crate::APP.get().is_none() {
        return;
    }
    let st = ui.review_state;
    let context = cache::key(
        &ui.active_project_host(),
        &ui.project.get_untracked(),
        "checkout",
        &json!(st.checkout_cwd.get_untracked()),
    );
    if st.read_context.with(|v| *v == context) {
        return;
    }
    st.read_context.replace(context);
    cancel_reads(st);
    batch(|| {
        st.refs_data.set(json!({}));
        st.log_commits.set(Vec::new());
        st.log_has_more.set(false);
        st.compare_data.set(json!({}));
        st.prepared_lines.set(Default::default());
        st.visible_file_limit.set(4);
    });
}

pub(crate) async fn cached_read(
    ui: Ui,
    method: &'static str,
    params: Value,
    project: String,
) -> Result<std::sync::Arc<cache::ReadValue>, String> {
    let cache = ui.review_state.read_cache.with(|v| v.clone());
    let key = cache::key(&ui.active_project_host(), &project, method, &params);
    blocking(move || {
        cache.load(key, || {
            crate::routed_call(method, params, Some(&project)).map_err(|e| format!("{e:#}"))
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) fn apply_compare(ui: Ui, value: &cache::ReadValue) {
    batch(|| {
        ui.review_state.prepared_lines.set(value.lines.clone());
        ui.review_state.compare_data.set(value.data.clone());
    });
}

pub fn show_more_files(ui: Ui) {
    ui.review_state.visible_file_limit.update(|n| *n += 4);
}

pub fn refresh(ui: Ui) {
    refresh_active_diff(ui, true);
    load_refs(ui);
    let branch = ui.review_state.log_branch.get_untracked();
    if !branch.is_empty() {
        git_ops::load_branch_log(ui, branch, false);
    }
}

pub fn load_refs(ui: Ui) {
    load_refs_impl(ui, None);
}

pub fn load_refs_and_compare(ui: Ui, choose_smart_default: bool) {
    if !choose_smart_default
        && !ui.review_state.base_ref.get_untracked().is_empty()
        && !ui.review_state.compare_ref.get_untracked().is_empty()
    {
        prepare_context(ui);
        load_refs(ui);
        refresh_active_diff(ui, false);
    } else {
        load_refs_impl(ui, Some(choose_smart_default));
    }
}

fn load_refs_impl(ui: Ui, compare: Option<bool>) {
    let st = ui.review_state;
    let Some(project) = ui.current_project() else {
        return;
    };
    prepare_context(ui);
    if crate::APP.get().is_none() {
        git_ops::load_branch_log(ui, String::new(), false);
        return;
    }
    let pid = project.id;
    let mut req = json!({"project_id": pid});
    let cwd = st.checkout_cwd.get_untracked();
    if !cwd.is_empty() {
        req["cwd"] = json!(cwd);
    }
    let request_key = req.to_string();
    if st.refs_loading.get_untracked() && st.refs_request.with(|v| *v == request_key) {
        return;
    }
    st.refs_request.replace(request_key);
    let ticket = st.refs_ticket.get_untracked() + 1;
    st.refs_ticket.set(ticket);
    st.refs_loading.set(true);
    let initial_pair = (st.base_ref.get_untracked(), st.compare_ref.get_untracked());
    ui.spawn(async move {
        let result = cached_read(ui, "git.refs", req, pid).await;
        if st.refs_ticket.try_get() != Some(ticket) {
            return;
        }
        st.refs_loading.set(false);
        match result {
            Ok(value) => {
                let val = &value.data;
                let current = val["current_branch"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("HEAD")
                    .to_owned();
                let default_br = val["default_branch"].as_str().unwrap_or("main").to_owned();
                batch(|| {
                    let pair_unchanged = initial_pair
                        == (st.base_ref.get_untracked(), st.compare_ref.get_untracked());
                    if pair_unchanged
                        && compare.is_some_and(|smart| {
                            smart
                                || st.base_ref.get_untracked().is_empty()
                                || st.compare_ref.get_untracked().is_empty()
                        })
                    {
                        if val["dirty"] == true {
                            st.base_ref.set("HEAD".into());
                            st.compare_ref.set("WORKDIR".into());
                        } else if current != default_br {
                            st.base_ref.set(default_br);
                            st.compare_ref.set(current.clone());
                        } else {
                            st.base_ref.set("HEAD~1".into());
                            st.compare_ref.set("HEAD".into());
                        }
                    }
                    st.refs_data.set(val.clone());
                    // The refs response already contains the first commit page.
                    let log = st.log_branch.get_untracked();
                    if (log.is_empty() || log == current)
                        && st.log_query.get_untracked().trim().is_empty()
                        && st.log_commits.with_untracked(Vec::is_empty)
                        && !st.log_loading.get_untracked()
                    {
                        st.log_branch.set(current.clone());
                        st.log_commits
                            .set(val["commits"].as_array().cloned().unwrap_or_default());
                        st.log_has_more
                            .set(st.log_commits.with_untracked(Vec::len) >= 40);
                    }
                });
                if st.log_commits.with_untracked(Vec::is_empty)
                    && !st.log_loading.get_untracked()
                    && (!st.log_query.get_untracked().trim().is_empty()
                        || st.log_branch.get_untracked() != current)
                {
                    git_ops::load_branch_log(ui, st.log_branch.get_untracked(), false);
                }
                if compare.is_some() {
                    refresh_active_diff(ui, false);
                }
            }
            Err(e) => st.error.set(e),
        }
    });
}

pub fn refresh_active_diff(ui: Ui, force_remote_refresh: bool) {
    let st = ui.review_state;
    if crate::APP.get().is_none() {
        if let Some(p) = ui.review.get_untracked() {
            let value = cache::ReadValue::new(preview_to_compare_json(&p));
            apply_compare(ui, &value);
        }
        return;
    }
    prepare_context(ui);
    if force_remote_refresh {
        invalidate_reads(ui);
    }
    let Some(project) = ui.current_project() else {
        return;
    };
    let mode = st.mode.get_untracked();
    let (method, params) = if mode == "github_pr" {
        let Some(pr) = st.pr_target.get_untracked() else {
            return;
        };
        (
            "review.github_pr",
            json!({"repository":pr["repository"], "number":pr["number"], "refresh":force_remote_refresh}),
        )
    } else if mode == "card_local" {
        let Some(card) = ui.current_card() else {
            return;
        };
        (
            "git.compare",
            json!({"card_id":card.id, "use_card_worktree":true, "target_branch":st.base_ref.get_untracked(), "three_dot":st.three_dot.get_untracked()}),
        )
    } else {
        let base = st.base_ref.get_untracked();
        let compare = st.compare_ref.get_untracked();
        let mut params = json!({"project_id":project.id,
            "base":if base.is_empty() { "HEAD" } else { &base },
            "compare":if compare.is_empty() { "WORKDIR" } else { &compare },
            "three_dot":st.three_dot.get_untracked()});
        let cwd = st.checkout_cwd.get_untracked();
        if !cwd.is_empty() {
            params["cwd"] = json!(cwd);
        }
        ("git.compare", params)
    };
    let request = cache::key(&ui.active_project_host(), &project.id, method, &params);
    let same_request = st.compare_request.with(|r| *r == request);
    if same_request && st.compare_loading.get_untracked() {
        return;
    }
    if !same_request {
        st.compare_request.replace(request);
        batch(|| {
            st.compare_data.set(json!({}));
            st.prepared_lines.set(Default::default());
            st.selected_file.set(String::new());
            st.visible_file_limit.set(4);
        });
    }
    let ticket = st.compare_ticket.get_untracked() + 1;
    st.compare_ticket.set(ticket);
    st.compare_loading.set(true);
    st.error.set(String::new());
    ui.spawn(async move {
        let result = cached_read(ui, method, params, project.id).await;
        if st.compare_ticket.try_get() != Some(ticket) {
            return;
        }
        batch(|| {
            st.compare_loading.set(false);
            match result {
                Ok(value) => {
                    let val = &value.data;
                    if mode == "github_pr" {
                        if let Some(base) = val["base_ref"].as_str() {
                            st.base_ref.set(base.into());
                        }
                        if let Some(compare) = val["compare_ref"].as_str() {
                            st.compare_ref.set(compare.into());
                        }
                    }
                    ui.review.set(if mode == "card_local" {
                        serde_json::from_value::<ReviewPreview>(val["review_preview"].clone()).ok()
                    } else {
                        None
                    });
                    apply_compare(ui, &value);
                }
                Err(error) => st.error.set(error),
            }
        });
    });
}

pub(crate) fn select_git_pair(ui: Ui, base: impl Into<String>, compare: impl Into<String>) {
    let st = ui.review_state;
    batch(move || {
        ui.card.set(String::new());
        ui.review.set(None);
        st.card_drawer_open.set(false);
        st.pr_target.set(None);
        st.range_backup.set(None);
        st.mode.set("git".into());
        st.base_ref.set(base.into());
        st.compare_ref.set(compare.into());
        st.selected_file.set(String::new());
        st.visible_file_limit.set(4);
        st.active_line.set(None);
    });
    refresh_active_diff(ui, false);
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let st = ui.review_state;

    if crate::APP.get().is_some() {
        let has_card = !ui.card.get_untracked().is_empty();
        if has_card
            && !st.compare_loading.get_untracked()
            && st.compare_data.with_untracked(|v| v["files"].is_null())
        {
            let cid = ui.card.get_untracked();
            open_card(ui, &cid);
        } else if !has_card
            && !st.refs_loading.get_untracked()
            && st.base_ref.get_untracked().is_empty()
        {
            load_refs_and_compare(ui, true);
        } else if !st.refs_loading.get_untracked() && !st.compare_loading.get_untracked() {
            // A remounted page reuses a fresh cache or asynchronously revalidates
            // after expiry/events, including Back/Forward navigation.
            load_refs(ui);
            refresh_active_diff(ui, false);
        }
    } else if let Some(p) = ui.review.get_untracked()
        && st.compare_data.with_untracked(|v| v["files"].is_null())
    {
        st.mode.set("card_local".into());
        st.base_ref.set(p.target_branch.clone());
        st.compare_ref.set("WORKDIR".into());
        st.compare_data.set(preview_to_compare_json(&p));
    }

    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .flex_row()
        .bg(BACKGROUND)
        .child(when(
            move || st.sidebar_open.get() && !ui.narrow.get(),
            move || sidebar::render_left_sidebar(ui),
        ))
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .child(sidebar::render_top_bar(ui))
                .child(dynamic(move || sidebar::render_card_delivery_banner(ui)))
                .child(dynamic(move || {
                    let err = st.error.get();
                    if err.is_empty() {
                        view().into_element()
                    } else {
                        view()
                            .px_3()
                            .pt_2()
                            .child(notice(err, Tone::Danger))
                            .into_element()
                    }
                }))
                .child(
                    view()
                        .flex_1()
                        .min_h(px(0.))
                        .min_w(px(0.))
                        .flex_row()
                        .child(view().flex_1().min_w(px(0.)).min_h(px(0.)).child(dynamic(
                            move || {
                                let data = if crate::APP.get().is_none()
                                    && let Some(p) = ui.review.get()
                                {
                                    preview_to_compare_json(&p)
                                } else if crate::APP.get().is_none()
                                    && !ui.card.get().is_empty()
                                    && ui.current_card().is_some_and(|c| c.delivery.is_some())
                                {
                                    json!({ "files": [] })
                                } else {
                                    st.compare_data.get()
                                };
                                diff::render_diff_workbench(ui, &data, st.compare_loading.get())
                            },
                        )))
                        .child(when(
                            move || {
                                st.card_drawer_open.get()
                                    && !ui.card.get().is_empty()
                                    && !ui.narrow.get()
                            },
                            move || {
                                view()
                                    .w(px(380.))
                                    .flex_shrink_0()
                                    .min_h(px(0.))
                                    .border_l_1()
                                    .border_color(BORDER)
                                    .bg(SURFACE)
                                    .child(crate::card::screen(ui))
                            },
                        )),
                )
                .child(submit::render_bottom_bar(ui)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_patch_splits_multi_file_diffs_and_counts_lines() {
        let patch = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n-old\n+new1\n+new2\ndiff --git a/src/b.rs b/src/b.rs\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1 @@\n-foo\n+bar";
        let files = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
        let parsed = parse_patch(patch, &files);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].path, "src/a.rs");
        assert_eq!(parsed[0].additions, 2);
        assert_eq!(parsed[0].deletions, 1);
        assert_eq!(parsed[1].path, "src/b.rs");
        assert_eq!(parsed[1].additions, 1);
        assert_eq!(parsed[1].deletions, 1);
    }

    #[test]
    fn unified_hunk_parser_tracks_old_and_new_line_numbers() {
        let patch = "@@ -10,3 +20,4 @@ fn demo()\n context\n-removed\n+added_one\n+added_two";
        let lines = model::parse_unified_hunk_lines(patch);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0].kind, '@');
        assert_eq!(
            (lines[1].kind, lines[1].old_line, lines[1].new_line),
            (' ', Some(10), Some(20))
        );
        assert_eq!(
            (lines[2].kind, lines[2].old_line, lines[2].new_line),
            ('-', Some(11), None)
        );
        assert_eq!(
            (lines[3].kind, lines[3].old_line, lines[3].new_line),
            ('+', None, Some(21))
        );
        assert_eq!(
            (lines[4].kind, lines[4].old_line, lines[4].new_line),
            ('+', None, Some(22))
        );
    }

    #[test]
    fn git_ref_options_include_workspace_shortcuts_branches_tags_and_commits() {
        let refs = json!({
            "dirty_count": 3,
            "staged_count": 1,
            "branches": [
                {"name": "main", "is_default": true, "short_sha": "a1b2c3d"},
                {"name": "feat/git-client", "is_current": true, "ahead": 2, "short_sha": "e4f5a6b"},
                {"name": "origin/main", "remote": true, "short_sha": "a1b2c3d"}
            ],
            "tags": [{"name": "v0.3.0", "short_sha": "1122334"}],
            "commits": [{"sha": "e4f5a6b7890", "short_sha": "e4f5a6b", "subject": "ui: git client"}]
        });

        let compare_opts = sidebar::build_git_ref_options(&refs, false, "WORKDIR");
        assert!(compare_opts.iter().any(|o| o.id == "WORKDIR" && o.selected));
        assert!(compare_opts.iter().any(|o| o.id == "STAGED"));
        assert!(
            compare_opts
                .iter()
                .any(|o| o.id == "feat/git-client" && o.detail == "↑2")
        );
        assert!(
            compare_opts
                .iter()
                .any(|o| o.id == "origin/main" && o.group.as_deref() == Some("Remote Branches"))
        );
        assert!(compare_opts.iter().any(|o| o.id == "v0.3.0"));
        assert!(compare_opts.iter().any(|o| o.id == "e4f5a6b7890"));

        let base_opts = sidebar::build_git_ref_options(&refs, true, "main");
        assert!(!base_opts.iter().any(|o| o.id == "WORKDIR"));
        assert!(base_opts.iter().any(|o| o.id == "HEAD"));
        assert!(base_opts.iter().any(|o| o.id == "main" && o.selected));
    }
}
