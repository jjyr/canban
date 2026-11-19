//! Unified Git Workbench:
//! - Works as a standalone Git client & compare workbench from the Left Rail even without any Card or PR.
//! - Opens directly from Local Cards (worktree/branch) and GitHub PR Cards (zero-clone remote PR diff).
//! - Supports inline line-by-line review comments + batch dispatch to @Agent (Terminals or Card) or GitHub PR.
pub mod cache;
pub mod context;
pub mod diff;
pub mod git_ops;
pub mod model;
pub mod sidebar;
pub mod submit;

pub use model::{LineTarget, PendingInlineComment, PullRefreshTarget, State};

use crate::presenter::*;
use canban_types::{Card, ReviewPreview};
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

pub(crate) fn card_pr_info(c: &Card) -> Option<(String, u64, String)> {
    if c.is_github_pr()
        && let Some(src) = c.github_issue()
    {
        return Some((src.repository.clone(), src.number, src.url.clone()));
    }
    c.pull_request
        .as_ref()
        .filter(|pr| pr.number > 0)
        .map(|pr| (pr.repository.clone(), pr.number, pr.url.clone()))
}

pub(crate) fn card_has_pr(c: &Card) -> bool {
    card_pr_info(c).is_some()
}

pub(crate) fn active_review_run(
    s: &canban_types::Snapshot,
    card_id: &str,
) -> Option<(String, Option<u64>)> {
    s.terminals.iter().find_map(|t| {
        let a = t.agent.as_ref()?;
        if a.card_id == card_id
            && a.trigger == canban_types::ExecutionTrigger::Custom("diff_review".into())
            && !a.finished()
            && !a.agent_stopped()
        {
            Some((
                a.profile.id.clone(),
                crate::components::activity::agent_started_at(s, t),
            ))
        } else {
            None
        }
    })
}

pub fn start_agent_review(
    ui: Ui,
    card_id: &str,
    profile_id: Option<String>,
    focus: Option<String>,
) {
    let profile = profile_id
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::sessions::default_review_profile(ui));
    if crate::APP.get().is_none() {
        ui.notify(format!(
            "Gallery preview · started Agent review (@{profile})"
        ));
        return;
    }
    let mut payload = json!({
        "card_id": card_id,
        "profile_id": profile,
        "request_key": canban_types::id(),
    });
    if let Some(f) = focus.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()) {
        payload["focus"] = json!(f);
    }
    let notify_profile = profile;
    dispatch_then(ui, "review.run_agent", payload, move |_| {
        ui.notify(format!("Started Agent PR review (@{notify_profile})"));
    });
}

pub fn discard_review_draft(ui: Ui, card_id: &str) {
    if crate::APP.get().is_none() {
        let cid = card_id.to_owned();
        ui.snapshot.update(|s| {
            s.review_drafts.retain(|d| d.card_id != cid);
        });
        let st = ui.review_state;
        if ui.card.get_untracked() == card_id {
            batch(|| {
                st.pending_comments.set(Vec::new());
                st.submit_summary.set(String::new());
                st.draft_anchor_sha.set(None);
                st.draft_drift_dismissed.set(false);
            });
        }
        ui.notify("Gallery preview · discarded review draft");
        return;
    }
    let cid = card_id.to_owned();
    dispatch_then(
        ui,
        "review.draft.clear",
        json!({"card_id": cid}),
        move |_| {
            let st = ui.review_state;
            if ui.card.get_untracked() == cid {
                batch(|| {
                    st.pending_comments.set(Vec::new());
                    st.submit_summary.set(String::new());
                    st.draft_anchor_sha.set(None);
                    st.draft_drift_dismissed.set(false);
                    st.loaded_draft_key.replace((String::new(), 0));
                });
            }
            ui.notify("Discarded local review draft");
        },
    );
}

pub fn sync_pending_draft_to_server(ui: Ui) {
    if crate::APP.get().is_none() {
        return;
    }
    let cid = ui.card.get_untracked();
    let st = ui.review_state;
    if cid.is_empty() || st.mode.get_untracked() == "git" {
        return;
    }
    let pending = st.pending_comments.get_untracked();
    let summary = st.submit_summary.get_untracked();
    if pending.is_empty() && summary.trim().is_empty() {
        dispatch_complete(ui, "review.draft.clear", json!({"card_id": cid}), |_| {});
        return;
    }
    let verdict = match st.submit_gh_event.get_untracked().as_str() {
        "APPROVE" => "approve",
        "REQUEST_CHANGES" => "request_changes",
        _ => "comment",
    };
    let comments: Vec<Value> = pending
        .iter()
        .map(|c| {
            json!({
                "id": c.id,
                "path": c.path,
                "section": c.section,
                "line": c.line,
                "side": c.side,
                "snippet": c.snippet,
                "body": c.body,
                "author": c.author,
                "edited_by_user": c.edited_by_user,
            })
        })
        .collect();
    let mut payload = json!({
        "card_id": cid,
        "summary": summary,
        "verdict": verdict,
        "comments": comments,
    });
    if let Some(sha) = st.draft_anchor_sha.get_untracked() {
        payload["compare_sha"] = json!(sha);
    }
    dispatch_complete(ui, "review.draft.set", payload, |_| {});
}

pub fn hydrate_card_review_draft(ui: Ui, card_id: &str, force: bool) {
    let st = ui.review_state;
    let draft_opt = ui
        .index
        .with_untracked(|idx| idx.review_draft(card_id).cloned());
    let Some(draft) = draft_opt else {
        if force {
            st.loaded_draft_key.replace((card_id.to_owned(), 0));
            st.draft_anchor_sha.set(None);
            st.draft_drift_dismissed.set(false);
        }
        return;
    };
    let key = (card_id.to_owned(), draft.updated_at);
    if !force && st.loaded_draft_key.with(|prev| *prev == key) {
        return;
    }
    st.loaded_draft_key.replace(key);
    let pending: Vec<PendingInlineComment> = draft
        .comments
        .into_iter()
        .map(|c| PendingInlineComment {
            id: c.id,
            path: c.path,
            section: c.section,
            line: c.line,
            side: c.side,
            snippet: c.snippet,
            body: c.body,
            author: c.author,
            edited_by_user: c.edited_by_user,
        })
        .collect();
    let gh_event = match draft.verdict.as_str() {
        "approve" => "APPROVE",
        "request_changes" => "REQUEST_CHANGES",
        _ => "COMMENT",
    };
    batch(|| {
        st.pending_comments.set(pending);
        st.submit_summary.set(draft.summary);
        st.submit_gh_event.set(gh_event.into());
        st.draft_anchor_sha.set(draft.compare_sha);
        st.draft_drift_dismissed.set(false);
    });
}

/// Re-entering Git retains this Project's selected checkout and comparison.
pub fn open_compare(ui: Ui) {
    if let Some(project) = ui.navigation_project() {
        ui.project.set(project);
    }
    let st = ui.review_state;
    let same_project = st.read_context.with(|key| {
        let value: Value = serde_json::from_str(key).unwrap_or_default();
        value[0] == ui.active_project_host() && value[1] == ui.project.get_untracked()
    });
    let mode = st.mode.get_untracked();
    let retain_pr =
        same_project && mode == "github_pr" && st.pr_target.with_untracked(Option::is_some);
    let retain_card_local = same_project
        && mode == "card_local"
        && ui.snapshot.with_untracked(|s| {
            let cid = st.review_card_id.get_untracked();
            !cid.is_empty() && s.cards.iter().any(|c| c.id == cid)
        });
    if retain_pr || retain_card_local {
        ui.card.set(st.review_card_id.get_untracked());
        if retain_pr {
            ui.review.set(None);
        }
        prepare_context(ui);
        load_refs(ui);
        refresh_active_diff(ui, false);
    } else if !same_project || mode != "git" {
        reset_git_selection(st);
        git_ops::select_checkout(ui, String::new());
    } else {
        ui.card.set(String::new());
        st.review_card_id.set(String::new());
        st.pr_target.set(None);
        ui.review.set(None);
        load_refs_and_compare(ui, false);
    }
    ui.navigate("review");
}

fn reset_git_selection(st: State) {
    reset_pull_refresh(st);
    cancel_reads(st);
    st.review_card_id.set(String::new());
    st.checkout_cwd.set(String::new());
    st.base_ref.set(String::new());
    st.compare_ref.set(String::new());
    st.log_branch.set(String::new());
    st.three_dot.set(true);
    st.mode.set("git".into());
    st.pr_target.set(None);
    st.range_backup.set(None);
    st.selected_file.set(String::new());
    st.active_line.set(None);
    st.error.set(String::new());
    st.refs_data.set(json!({}));
    st.compare_data.set(json!({}));
    st.prepared_lines.set(Default::default());
    st.prepared_hunks.set(Default::default());
    st.collapsed_files.set(Default::default());
    st.auto_collapsed_seen
        .with(std::collections::BTreeSet::clear);
    st.log_commits.set(Vec::new());
    st.log_has_more.set(false);
    clear_context_expansion(st);
    st.anchor_targets.with(std::collections::BTreeMap::clear);
}

/// A history destination keeps its Project; only foreign Git selections expire.
fn clear_foreign_project_selection(ui: Ui) {
    let host = ui.active_project_host();
    let project = ui.project.get_untracked();
    let foreign = ui.review_state.read_context.with(|key| {
        serde_json::from_str::<Value>(key).ok().is_some_and(|key| {
            key[0].as_str() != Some(host.as_str()) || key[1].as_str() != Some(project.as_str())
        })
    });
    if foreign {
        reset_git_selection(ui.review_state);
    }
}

pub(crate) fn card_target_branch(card: &Card, project: Option<&canban_types::Project>) -> String {
    card.workspace
        .as_ref()
        .and_then(|w| w.target_branch.clone())
        .or_else(|| project.and_then(|p| p.target_branch.clone()))
        .unwrap_or_else(|| "main".into())
}

pub(crate) fn card_compare_params(card: &Card, target: &str) -> Value {
    json!({"card_id":card.id, "cwd":card.worktree, "use_card_worktree":true, "target_branch":target, "three_dot":true})
}

pub fn open_card_pr(ui: Ui, card_id: &str) {
    let Some(card) = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card_id).cloned())
    else {
        open_compare(ui);
        return;
    };
    if let Some((repository, number, url)) = card_pr_info(&card) {
        crate::navigation::checkpoint(ui);
        ui.project.set(card.project_id.clone());
        let st = ui.review_state;
        let same_pr =
            st.mode.get_untracked() == "github_pr" && st.review_card_id.get_untracked() == card.id;
        let cwd = card.worktree.clone().unwrap_or_default();
        batch(|| {
            ui.card.set(card.id.clone());
            st.review_card_id.set(card.id.clone());
            ui.review.set(None);
            st.checkout_cwd.set(cwd);
            st.mode.set("github_pr".into());
            st.pr_target.set(Some(
                json!({"repository":repository, "number":number, "url":url}),
            ));
            st.selected_file.set(String::new());
            st.compare_open.set(false);
            st.agent_review_open.set(false);
        });
        hydrate_card_review_draft(ui, &card.id, !same_pr);
        prepare_context(ui);
        load_refs(ui);
        refresh_active_diff(ui, false);
        ui.navigate("review");
        crate::navigation::checkpoint(ui);
    } else {
        open_card(ui, card_id);
    }
}

pub fn open_card(ui: Ui, card_id: &str) {
    let Some(card) = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card_id).cloned())
    else {
        open_compare(ui);
        return;
    };
    if card.is_github_pr() || (card.worktree.is_none() && card_pr_info(&card).is_some()) {
        open_card_pr(ui, card_id);
        return;
    }
    crate::navigation::checkpoint(ui);
    ui.project.set(card.project_id.clone());
    let st = ui.review_state;
    let same_project = st.read_context.with(|key| {
        let value: Value = serde_json::from_str(key).unwrap_or_default();
        value[0] == ui.active_project_host() && value[1] == card.project_id
    });
    if let Some(path) = card.worktree.clone().filter(|p| !p.is_empty()) {
        let same_checkout = same_project
            && st.mode.get_untracked() == "git"
            && st.checkout_cwd.get_untracked() == path
            && !st.base_ref.get_untracked().is_empty()
            && !st.compare_ref.get_untracked().is_empty();
        if same_checkout {
            ui.card.set(String::new());
            st.review_card_id.set(String::new());
            ui.review.set(None);
            load_refs_and_compare(ui, false);
        } else {
            git_ops::select_checkout(ui, path);
        }
        if let Some((repository, number, url)) = card_pr_info(&card) {
            ui.card.set(card.id.clone());
            st.review_card_id.set(card.id.clone());
            st.pr_target.set(Some(
                json!({"repository":repository, "number":number, "url":url}),
            ));
            if !same_checkout {
                hydrate_card_review_draft(ui, &card.id, true);
            }
        }
    } else {
        let same_checkout = same_project
            && st.mode.get_untracked() == "git"
            && st.checkout_cwd.get_untracked().is_empty()
            && !st.base_ref.get_untracked().is_empty()
            && !st.compare_ref.get_untracked().is_empty();
        if same_checkout {
            ui.card.set(String::new());
            st.review_card_id.set(String::new());
            ui.review.set(None);
            load_refs_and_compare(ui, false);
        } else {
            git_ops::select_checkout(ui, String::new());
        }
    }
    ui.navigate("review");
    crate::navigation::checkpoint(ui);
}

/// Card hints come only from the selected checkout's host-resolved path match.
pub fn related_card(ui: Ui) -> Option<Card> {
    let id = ui
        .review_state
        .refs_data
        .with(|v| v["related_card_id"].as_str().map(str::to_owned))?;
    ui.snapshot.with(|s| {
        s.cards
            .iter()
            .find(|c| {
                c.id == id
                    && c.active()
                    && c.project_id == ui.project.get()
                    && s.card_host(&c.id) == ui.active_project_host()
            })
            .cloned()
    })
}
pub fn open_related_card(ui: Ui) {
    if let Some(card) = related_card(ui) {
        ui.open_card(card.id);
    }
}

pub fn toggle_file(ui: Ui, key: String) {
    ui.review_state.collapsed_files.update(|files| {
        if !files.remove(&key) {
            files.insert(key);
        }
    });
}

/// Reveal a file in the continuous Changes view without filtering out its peers.
pub fn focus_file(ui: Ui, key: String) {
    let st = ui.review_state;
    let index = st.compare_data.with_untracked(|v| {
        v["files"]
            .as_array()
            .and_then(|files| files.iter().position(|f| model::file_key(f) == key))
    });
    let Some(index) = index else {
        st.pending_file.replace(key);
        return;
    };
    st.pending_file.replace(String::new());
    // Reveal intervening headers without constructing every unseen code body.
    let mounted = st.visible_file_limit.get_untracked();
    let deferred = st.compare_data.with_untracked(|v| {
        v["files"]
            .as_array()
            .into_iter()
            .flatten()
            .skip(mounted)
            .take(index.saturating_sub(mounted))
            .map(model::file_key)
            .collect::<Vec<_>>()
    });
    batch(|| {
        st.selected_file.set(key.clone());
        st.collapsed_files.update(|files| {
            files.extend(deferred);
            files.remove(&key);
        });
        st.visible_file_limit
            .update(|limit| *limit = (*limit).max(index + 1));
    });
    let Some(viewport) = st.viewport.with(|v| *v) else {
        return;
    };
    ui.spawn(async move {
        next_frame().await;
        next_frame().await;
        if st.selected_file.try_get().as_ref() != Some(&key) {
            return;
        }
        let Some(target) = st.file_targets.with(|targets| targets.get(&key).copied()) else {
            return;
        };
        let target = target.measure().await;
        let content = viewport.content.measure().await;
        if let (Some(target), Some(content)) = (target, content) {
            viewport
                .scroll
                .scroll_to(Point::new(px(0.), target.page.y - content.page.y), false);
        }
    });
}

pub fn show_all_files(ui: Ui) {
    ui.review_state.selected_file.set(String::new());
    if let Some(viewport) = ui.review_state.viewport.with(|v| *v) {
        viewport.scroll.scroll_to(Point::default(), false);
    }
}

/// Invalidate results and tickets together so requests predating a Git change
/// cannot update the screen or refill the cache.
pub fn invalidate_reads(ui: Ui) {
    let st = ui.review_state;
    st.read_cache.with(|c| {
        c.invalidate_checkout(
            &ui.active_project_host(),
            &ui.project.get_untracked(),
            &st.checkout_cwd.get_untracked(),
            Some(&ui.card.get_untracked()),
        )
    });
    st.summary_revision.update(|n| *n += 1);
    clear_context_expansion(st);
    cancel_reads(st);
}

fn clear_context_expansion(st: State) {
    if st.file_lines_cache.with(|c| !c.is_empty()) {
        st.file_lines_cache.with(cache::FileLinesCache::clear);
        st.file_lines_revision.update(|n| *n += 1);
    }
    if st.expanded_gaps.with_untracked(|m| !m.is_empty()) {
        st.expanded_gaps.set(Default::default());
    }
    if let Some(viewport) = st.viewport.with(|v| *v) {
        viewport.bottom_space.try_set(0.);
    }
    if st.file_lines_loading.with_untracked(|s| !s.is_empty()) {
        st.file_lines_loading.set(Default::default());
    }
    if st.file_lines_errors.with_untracked(|m| !m.is_empty()) {
        st.file_lines_errors.set(Default::default());
    }
}

fn cancel_reads(st: State) {
    st.refresh_pending.set(false);
    st.refs_ticket.update(|n| *n += 1);
    st.log_ticket.update(|n| *n += 1);
    st.compare_ticket.update(|n| *n += 1);
    st.refs_request.replace(String::new());
    st.log_request.replace(String::new());
    st.refs_loading.set(false);
    st.log_loading.set(false);
    st.compare_loading.set(false);
    maybe_finish_pull_refresh(st);
}

pub(crate) const MIN_PULL_REFRESH_HOLD: std::time::Duration = std::time::Duration::from_millis(450);

fn reads_busy_untracked(st: State) -> bool {
    st.refs_loading.get_untracked()
        || st.log_loading.get_untracked()
        || st.compare_loading.get_untracked()
}

pub(crate) fn maybe_finish_pull_refresh(st: State) {
    if reads_busy_untracked(st) {
        return;
    }
    for (refreshing, hold_ready) in [
        (st.log_refreshing, st.log_refresh_hold_ready),
        (st.files_refreshing, st.files_refresh_hold_ready),
        (st.diff_refreshing, st.diff_refresh_hold_ready),
    ] {
        if refreshing.get_untracked() && hold_ready.get_untracked() {
            refreshing.set(false);
        }
    }
}

pub(crate) fn reset_pull_refresh(st: State) {
    for target in [
        PullRefreshTarget::Commits,
        PullRefreshTarget::Files,
        PullRefreshTarget::Diff,
    ] {
        let (refreshing, hold_ready, ticket) = st.pull_slot(target);
        ticket.update(|n| *n += 1);
        hold_ready.set(false);
        refreshing.set(false);
    }
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
    st.pending_file.replace(String::new());
    reset_pull_refresh(st);
    cancel_reads(st);
    batch(|| {
        st.refs_data.set(json!({}));
        st.log_commits.set(Vec::new());
        st.log_has_more.set(false);
        st.compare_data.set(json!({}));
        st.prepared_lines.set(Default::default());
        st.prepared_hunks.set(Default::default());
        st.visible_file_limit.set(4);
        st.collapsed_files.set(Default::default());
        st.auto_collapsed_seen
            .with(std::collections::BTreeSet::clear);
        clear_context_expansion(st);
        st.anchor_targets.with(std::collections::BTreeMap::clear);
    });
}

pub(crate) async fn cached_read(
    ui: Ui,
    method: &'static str,
    params: Value,
    project: String,
) -> Result<std::sync::Arc<cache::ReadValue>, String> {
    let cache = ui.review_state.read_cache.with(|v| v.clone());
    let host = ui
        .snapshot
        .with_untracked(|s| s.project_host(&project).to_owned());
    let key = cache::key(&host, &project, method, &params);
    blocking(move || {
        cache.load(key, || {
            crate::routed_call(method, params, Some(&project)).map_err(|e| format!("{e:#}"))
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

pub fn apply_compare(ui: Ui, value: &cache::ReadValue) {
    let st = ui.review_state;
    let mut newly_auto_folded = Vec::new();
    st.auto_collapsed_seen.with(|seen| {
        for file in value.data["files"].as_array().into_iter().flatten() {
            let key = model::file_key(file);
            if seen.insert(key.clone()) {
                let hunks = value.hunks.get(&key).map(|h| h.as_slice());
                if model::auto_fold_reason(file, hunks).is_some() {
                    newly_auto_folded.push(key);
                }
            }
        }
    });
    batch(|| {
        if !newly_auto_folded.is_empty() {
            st.collapsed_files.update(|files| {
                files.extend(newly_auto_folded);
            });
        }
        st.prepared_lines.set(value.lines.clone());
        st.prepared_hunks.set(value.hunks.clone());
        st.compare_data.set(value.data.clone());
    });
    let pending = st.pending_file.replace(String::new());
    if !pending.is_empty() {
        focus_file(ui, pending);
    }
}

pub fn show_more_files(ui: Ui) {
    let st = ui.review_state;
    let total = st
        .compare_data
        .with_untracked(|v| v["files"].as_array().map_or(0, Vec::len));
    if total == 0 {
        st.visible_file_limit.update(|n| *n += 4);
    } else if st.visible_file_limit.get_untracked() < total {
        st.visible_file_limit.update(|n| *n = (*n + 4).min(total));
    }
}

pub fn refresh(ui: Ui) {
    prepare_context(ui);
    let st = ui.review_state;
    if untracked(|| ui.is_active_project_offline()) {
        st.refresh_pending.set(false);
        return;
    }
    if reads_busy(st) {
        st.refresh_pending.set(true);
        return;
    }
    batch(|| {
        invalidate_reads(ui);
        load_active_diff(ui, true);
        load_refs(ui);
        let branch = st.log_branch.get_untracked();
        if !branch.is_empty() {
            git_ops::load_branch_log(ui, branch, false);
        }
    });
}

fn reads_busy(st: State) -> bool {
    st.refs_loading.get() || st.log_loading.get() || st.compare_loading.get()
}

/// One trailing refresh for a burst; reads already running are allowed to finish.
pub(crate) fn init(ui: Ui) {
    effect(move || {
        if ui.page.get() == "review"
            && ui.review_state.refresh_pending.get()
            && !reads_busy(ui.review_state)
        {
            untracked(|| refresh(ui));
        }
    });
}

pub(crate) fn handle_host_events(ui: Ui, events: &[canban_app_server::hosts::HostEvent]) {
    let host = ui.active_project_host();
    let project = ui.project.get_untracked();
    let mut active_changed = false;
    for event in events {
        let canban_types::AppEvent::Changed { topic, card_id } = &event.event;
        if !matches!(*topic, "resync" | "review") {
            continue;
        }
        let card = card_id.as_ref().and_then(|id| {
            ui.snapshot
                .with_untracked(|s| s.cards.iter().find(|c| &c.id == id).cloned())
        });
        let event_project = card.as_ref().map(|c| c.project_id.as_str());
        let event_cwd = card.as_ref().and_then(|c| c.worktree.as_deref());
        ui.review_state.read_cache.with(|cache| {
            if let (Some(project), Some(cwd)) = (event_project, event_cwd) {
                cache.invalidate_checkout(&event.host_id, project, cwd, card_id.as_deref());
            } else {
                cache.invalidate_scope(&event.host_id, event_project);
            }
        });
        active_changed |= event.host_id == host
            && event_project.is_none_or(|p| p == project)
            && event_cwd.is_none_or(|cwd| {
                cwd == ui.review_state.checkout_cwd.get_untracked()
                    || card_id.as_deref() == Some(ui.card.get_untracked().as_str())
            });
    }
    if active_changed {
        ui.review_state.summary_revision.update(|n| *n += 1);
    }
    if active_changed && ui.page.get_untracked() == "review" {
        refresh(ui);
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

pub(crate) fn sync_log_commits_from_refs(st: State, val: &Value, current: &str) {
    let log = st.log_branch.get_untracked();
    if (log.is_empty() || log == current)
        && st.log_query.get_untracked().trim().is_empty()
        && !st.log_loading.get_untracked()
    {
        let fresh = val["commits"].as_array().cloned().unwrap_or_default();
        let existing_head = st.log_commits.with_untracked(|list| {
            list.first()
                .and_then(|c| c["sha"].as_str())
                .map(str::to_owned)
        });
        let fresh_head = fresh
            .first()
            .and_then(|c| c["sha"].as_str())
            .map(str::to_owned);
        let existing_len = st.log_commits.with_untracked(Vec::len);
        st.log_branch.set(current.to_owned());
        if existing_len == 0 || existing_head != fresh_head || existing_len <= fresh.len() {
            let has_more = fresh.len() >= 40;
            st.log_commits.set(fresh);
            st.log_has_more.set(has_more);
        }
    }
}

fn load_refs_impl(ui: Ui, compare: Option<bool>) {
    let st = ui.review_state;
    let Some(project) = ui.current_project() else {
        return;
    };
    prepare_context(ui);
    if untracked(|| ui.is_active_project_offline()) {
        return;
    }
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
        batch(|| {
            st.refs_loading.set(false);
            match result {
                Ok(value) => {
                    let val = &value.data;
                    let current = val["current_branch"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("HEAD")
                        .to_owned();
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
                            st.base_ref.set("HEAD".into());
                            st.compare_ref.set("UNCOMMITTED".into());
                        }
                        st.refs_data.set(val.clone());
                        sync_log_commits_from_refs(st, val, &current);
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
                    maybe_finish_pull_refresh(st);
                }
                Err(e) => {
                    st.error.set(e);
                    maybe_finish_pull_refresh(st);
                }
            }
        });
    });
}

pub fn refresh_active_diff(ui: Ui, force_remote_refresh: bool) {
    if force_remote_refresh {
        refresh(ui);
    } else {
        load_active_diff(ui, false);
    }
}

fn load_active_diff(ui: Ui, force_remote_refresh: bool) {
    let st = ui.review_state;
    if crate::APP.get().is_none() {
        if let Some(p) = ui.review.get_untracked() {
            let value = cache::ReadValue::new(preview_to_compare_json(&p));
            apply_compare(ui, &value);
        }
        return;
    }
    prepare_context(ui);
    if untracked(|| ui.is_active_project_offline()) {
        return;
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
            card_compare_params(&card, &st.base_ref.get_untracked()),
        )
    } else {
        let base = st.base_ref.get_untracked();
        let compare = st.compare_ref.get_untracked();
        let mut params = json!({"project_id":project.id,
            "base":if base.is_empty() { "HEAD" } else { &base },
            "compare":if compare.is_empty() { "UNCOMMITTED" } else { &compare },
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
        st.refresh_pending.set(false);
        st.compare_request.replace(request);
        batch(|| {
            st.compare_data.set(json!({}));
            st.prepared_lines.set(Default::default());
            st.prepared_hunks.set(Default::default());
            st.selected_file.set(String::new());
            st.visible_file_limit.set(4);
            st.collapsed_files.set(Default::default());
            st.auto_collapsed_seen
                .with(std::collections::BTreeSet::clear);
            clear_context_expansion(st);
            st.anchor_targets.with(std::collections::BTreeMap::clear);
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
            maybe_finish_pull_refresh(st);
        });
    });
}

pub(crate) fn select_git_pair(ui: Ui, base: impl Into<String>, compare: impl Into<String>) {
    let st = ui.review_state;
    batch(move || {
        ui.card.set(String::new());
        ui.review.set(None);
        st.review_card_id.set(String::new());
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
        if !has_card {
            clear_foreign_project_selection(ui);
        }
        if has_card
            && !st.compare_loading.get_untracked()
            && st.compare_data.with_untracked(|v| v["files"].is_null())
        {
            if st.mode.get_untracked() == "github_pr"
                && st.pr_target.with_untracked(Option::is_some)
            {
                load_refs(ui);
                refresh_active_diff(ui, false);
            } else {
                let cid = ui.card.get_untracked();
                open_card(ui, &cid);
            }
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

    effect(move || {
        let _ = ui.index.get();
        let page = ui.page.get();
        let cid = ui.card.get();
        if page == "review" && !cid.is_empty() && st.mode.get_untracked() != "git" {
            untracked(|| hydrate_card_review_draft(ui, &cid, false));
        }
    });

    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .bg(BACKGROUND)
        .child(sidebar::render_top_bar(ui))
        .child(crate::hosts::offline_banner(ui))
        .child(
            view()
                .flex_1()
                .min_h(px(0.))
                .min_w(px(0.))
                .flex_row()
                .child(when(
                    move || st.sidebar_open.get() && !ui.narrow.get(),
                    move || sidebar::render_left_sidebar(ui),
                ))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .child(when(
                            move || st.mode.get() != "git",
                            move || dynamic(move || sidebar::render_card_delivery_banner(ui)),
                        ))
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
                        .child(diff::workbench(ui))
                        .child(submit::render_bottom_bar(ui)),
                ),
        )
}

pub fn toggle_comparison(ui: Ui) {
    ui.review_state.compare_open.update(|open| *open = !*open);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_changes_clear_git_choices_while_same_project_and_history_keep_their_target() {
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let (_owner, ()) = nsh_core::element::create_root(|| {
            let ui = Ui::new();
            let st = ui.review_state;
            ui.snapshot.update(|s| {
                for id in ["a", "b"] {
                    s.projects.push(
                        serde_json::from_value(json!({
                            "id":id,"name":id,"directory":format!("/repo/{id}"),
                            "workflow":canban_types::Workflow::default(),"created_at":1
                        }))
                        .unwrap(),
                    );
                }
            });
            ui.select_project("a".into());
            st.read_context
                .replace(cache::key("local", "a", "checkout", &json!("")));
            st.base_ref.set("main-a".into());
            st.compare_ref.set("feature-a".into());
            open_compare(ui);
            assert_eq!(st.compare_ref.get_untracked(), "feature-a");

            st.checkout_cwd.set("/repo/a-worktree".into());
            st.read_context.replace(cache::key(
                "local",
                "a",
                "checkout",
                &json!("/repo/a-worktree"),
            ));
            open_compare(ui);
            assert_eq!(st.checkout_cwd.get_untracked(), "/repo/a-worktree");
            assert_eq!(st.compare_ref.get_untracked(), "feature-a");
            st.log_branch.set("feature-a".into());
            st.pr_target.set(Some(json!({"number":7})));
            st.refs_data.set(json!({"branches":[{"name":"feature-a"}]}));
            st.compare_data.set(json!({"files":[{"filename":"a.txt"}]}));
            let ticket = st.compare_ticket.get_untracked();
            ui.select_project("b".into());
            open_compare(ui);
            assert_eq!(ui.project.get_untracked(), "b");
            assert_eq!(st.base_ref.get_untracked(), "HEAD");
            assert_eq!(st.compare_ref.get_untracked(), "UNCOMMITTED");
            assert!(st.checkout_cwd.get_untracked().is_empty());
            assert_ne!(st.log_branch.get_untracked(), "feature-a");
            assert!(st.pr_target.get_untracked().is_none());
            assert!(st.refs_data.get_untracked()["branches"].is_null());
            assert!(st.compare_data.get_untracked()["files"].is_null());
            assert!(st.compare_ticket.get_untracked() > ticket);

            // A restored Git page drops foreign choices without changing its Project.
            ui.project.set("a".into());
            st.read_context
                .replace(cache::key("local", "b", "checkout", &json!("")));
            st.compare_ref.set("feature-b".into());
            clear_foreign_project_selection(ui);
            assert_eq!(ui.project.get_untracked(), "a");
            assert!(st.compare_ref.get_untracked().is_empty());
        });
    }

    #[test]
    fn state_initialization_does_not_require_a_mounted_ui_executor() {
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let (_owner, ()) = nsh_core::element::create_root(|| {
            let state = State::new();
            assert!(state.viewport.with(|v| v.is_none()));
        });
    }

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

    #[test]
    fn board_round_trip_retains_pr_and_card_worktree_comparison_context() {
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let (_owner, ()) = nsh_core::element::create_root(|| {
            let ui = Ui::new();
            let st = ui.review_state;
            ui.snapshot.update(|s| {
                s.projects.push(
                    serde_json::from_value(json!({
                        "id":"p1","name":"p1","directory":"/repo/p1",
                        "workflow":canban_types::Workflow::default(),"created_at":1
                    }))
                    .unwrap(),
                );
                s.cards.push(
                    serde_json::from_value(json!({
                        "id":"c-pr","project_id":"p1","number":10,"title":"PR card",
                        "body":"","stage":"review","revision":1,"created_at":1,
                        "worktree":"/repo/p1-wt-pr",
                        "pull_request":{
                            "repository":"org/repo","repository_id":1,"number":42,
                            "url":"https://github.com/org/repo/pull/42","state":"open",
                            "head":"abc123","head_branch":"pr-head","base_branch":"main",
                            "merged":false,"checks":"",
                            "workspace":{"location":{"machine":"local","directory":"/repo/p1"},"path":"/repo/p1-wt-pr"}
                        }
                    }))
                    .unwrap(),
                );
                s.cards.push(
                    serde_json::from_value(json!({
                        "id":"c-wt","project_id":"p1","number":11,"title":"Local card",
                        "body":"","stage":"develop","revision":1,"created_at":2,
                        "worktree":"/repo/p1-wt-local"
                    }))
                    .unwrap(),
                );
            });
            ui.select_project("p1".into());

            // 1. Entering PR mode, navigating to Board, opening/closing another card,
            // and returning via Git rail retains the PR context and card association.
            open_card_pr(ui, "c-pr");
            st.read_context.replace(cache::key(
                "local",
                "p1",
                "checkout",
                &json!("/repo/p1-wt-pr"),
            ));
            st.base_ref.set("main".into());
            st.compare_ref.set("pr-head".into());
            st.pending_comments.set(vec![PendingInlineComment {
                id: "pc-pr".into(),
                path: "src/lib.rs".into(),
                section: String::new(),
                line: 12,
                side: "RIGHT".into(),
                snippet: String::new(),
                body: "Check PR edge case".into(),
                author: None,
                edited_by_user: false,
            }]);

            ui.open_board();
            ui.open_card("c-wt".into());
            ui.close_card();
            assert!(ui.card.get_untracked().is_empty());

            open_compare(ui);
            assert_eq!(ui.page.get_untracked(), "review");
            assert_eq!(st.mode.get_untracked(), "github_pr");
            assert_eq!(ui.card.get_untracked(), "c-pr");
            assert_eq!(st.pr_target.get_untracked().unwrap()["number"], 42);
            assert_eq!(st.pending_comments.with_untracked(Vec::len), 1);

            // 2. Inspecting a single commit on a Card worktree, visiting Board, and
            // re-opening the same Card via `open_card` preserves the commit comparison.
            open_card(ui, "c-wt");
            st.read_context.replace(cache::key(
                "local",
                "p1",
                "checkout",
                &json!("/repo/p1-wt-local"),
            ));
            st.base_ref.set("abc1234~1".into());
            st.compare_ref.set("abc1234".into());
            st.pending_comments.set(vec![PendingInlineComment {
                id: "pc-commit".into(),
                path: "src/main.rs".into(),
                section: String::new(),
                line: 5,
                side: "RIGHT".into(),
                snippet: String::new(),
                body: "Commit comment".into(),
                author: None,
                edited_by_user: false,
            }]);

            ui.open_board();
            open_card(ui, "c-wt");
            assert_eq!(ui.page.get_untracked(), "review");
            assert_eq!(st.mode.get_untracked(), "git");
            assert_eq!(st.checkout_cwd.get_untracked(), "/repo/p1-wt-local");
            assert_eq!(st.base_ref.get_untracked(), "abc1234~1");
            assert_eq!(st.compare_ref.get_untracked(), "abc1234");
            assert_eq!(st.pending_comments.with_untracked(Vec::len), 1);
        });
    }

    #[test]
    fn sync_log_commits_from_refs_updates_stale_head_while_preserving_appended_pages_when_unchanged()
     {
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let (_owner, ()) = nsh_core::element::create_root(|| {
            let st = State::new();
            let initial = json!({
                "current_branch": "main",
                "commits": [
                    {"sha": "1111111", "short_sha": "1111111", "subject": "old head"}
                ]
            });
            sync_log_commits_from_refs(st, &initial, "main");
            assert_eq!(st.log_commits.with_untracked(Vec::len), 1);
            assert_eq!(st.log_commits.get_untracked()[0]["sha"], "1111111");

            // Fresh refs with a new commit on main updates non-empty `log_commits`.
            let updated = json!({
                "current_branch": "main",
                "commits": [
                    {"sha": "2222222", "short_sha": "2222222", "subject": "new head"},
                    {"sha": "1111111", "short_sha": "1111111", "subject": "old head"}
                ]
            });
            sync_log_commits_from_refs(st, &updated, "main");
            assert_eq!(st.log_commits.with_untracked(Vec::len), 2);
            assert_eq!(st.log_commits.get_untracked()[0]["sha"], "2222222");

            // If the user has scrolled to load 80 commits and HEAD did not change,
            // a routine `git.refs` check (which only returns 40 commits) keeps the 80 commits.
            let eighty: Vec<Value> = (0..80)
                .map(|i| {
                    if i == 0 {
                        json!({"sha": "2222222", "short_sha": "2222222", "subject": "new head"})
                    } else {
                        json!({"sha": format!("old-{i}"), "short_sha": "old", "subject": "older"})
                    }
                })
                .collect();
            st.log_commits.set(eighty);
            let forty_same_head = json!({
                "current_branch": "main",
                "commits": (0..40).map(|i| {
                    if i == 0 {
                        json!({"sha": "2222222", "short_sha": "2222222", "subject": "new head"})
                    } else {
                        json!({"sha": format!("old-{i}"), "short_sha": "old", "subject": "older"})
                    }
                }).collect::<Vec<_>>()
            });
            sync_log_commits_from_refs(st, &forty_same_head, "main");
            assert_eq!(st.log_commits.with_untracked(Vec::len), 80);
        });
    }
}
