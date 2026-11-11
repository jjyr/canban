//! Review is a separate route so Task/Comment drafts survive refreshes.
//! Contract: an immutable preview is accepted only with its revision and token.
//! Refresh is asynchronous; changing Card/target invalidates a pending preview.
use crate::presenter::*;
use canban_types::ReviewPreview;
use canban_ui_kit::tokens::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;
use serde_json::json;

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
    if p.truncated {
        body = body.child(notice(
            "Diff exceeds 2 MiB and is truncated. Inspect the complete checkout in the terminal before accepting.",
            Tone::Attention,
        ));
    }
    if parsed.len() <= 1 {
        let fence = "`".repeat(
            p.patch
                .split(|c| c != '`')
                .map(str::len)
                .max()
                .unwrap_or(0)
                .max(2)
                + 1,
        );
        let code = format!("{fence}diff\n{}\n{fence}", p.patch);
        return body.child(nsh_base::text_view().markdown(code));
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
                    nsh_base::button()
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
fn refresh_diff(ui: Ui, target: Signal<String>) {
    let generation = ui.review_request.get_untracked() + 1;
    ui.review_request.set(generation);
    ui.review.set(None);
    ui.status.set("Loading Card diff…".into());
    let card = ui.card.get_untracked();
    let branch = target.get_untracked();
    let Some(app) = crate::APP.get().cloned() else {
        ui.status
            .set("Gallery preview: no AppServer connected".into());
        return;
    };
    ui.spawn(async move {
        let expected = card.clone();
        let expected_branch = branch.clone();
        let result = blocking(move || {
            app.call(
                "review.preview",
                json!({"card_id":card,"target_branch":branch}),
            )
        })
        .await;
        if ui.review_request.get_untracked() != generation
            || ui.page.get_untracked() != "review"
            || ui.card.get_untracked() != expected
            || target.try_get().as_deref() != Some(expected_branch.as_str())
        {
            return;
        }
        match result {
            Ok(Ok(value)) => match serde_json::from_value(value) {
                Ok(p) => {
                    ui.review.set(Some(p));
                    ui.status.set("Diff ready · refresh if files change".into());
                }
                Err(e) => ui
                    .operation_error
                    .set(format!("Invalid review preview: {e}")),
            },
            Ok(Err(e)) => ui
                .operation_error
                .set(format!("Could not load diff: {e:#}")),
            Err(e) => ui.operation_error.set(format!("Diff worker: {e}")),
        }
    });
}
pub fn screen(ui: Ui) -> impl IntoElement {
    let card = ui.current_card();
    let target_initial = ui
        .snapshot
        .with_untracked(|s| {
            card.as_ref()
                .and_then(|c| s.projects.iter().find(|p| p.id == c.project_id))
                .and_then(|p| p.target_branch.clone())
        })
        .unwrap_or_else(|| "main".into());
    let message_initial = card
        .as_ref()
        .map(|c| format!("Canban #{}: {}", c.number, c.title))
        .unwrap_or_default();
    let key = format!("review:{}", ui.card.get_untracked());
    let d = crate::drafts::document(
        ui,
        key,
        crate::drafts::Draft {
            title: target_initial,
            body: message_initial,
            ..Default::default()
        },
    );
    let target = signal(d.get_untracked().title);
    let message = signal(d.get_untracked().body);
    effect(move || {
        let title = target.get();
        let body = message.get();
        d.update(|d| {
            d.title = title;
            d.body = body;
        });
    });
    let mode = signal(
        if card
            .as_ref()
            .and_then(|c| c.delivery_mode.as_ref())
            .cloned()
            .unwrap_or_else(|| {
                ui.current_project()
                    .map(|p| p.delivery_mode)
                    .unwrap_or_default()
            })
            == canban_types::DeliveryMode::GitHubPr
        {
            "github_pr"
        } else {
            "local"
        }
        .to_owned(),
    );
    if ui.review.get_untracked().is_none()
        && card.as_ref().is_some_and(|c| c.delivery.is_none())
        && crate::APP.get().is_some()
    {
        refresh_diff(ui, target);
    }
    view().flex_1().min_h(px(0.0)).p_4().gap_3()
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(heading("Review and deliver"))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .child(compact_action("← Develop", Variant::Secondary, move |_| {
                            if let Some(c) = ui.current_card() {
                                let source = ui.location();
                                dispatch_then(
                                    ui,
                                    "card.move",
                                    json!({"card_id":c.id,"revision":c.revision,"stage":"develop"}),
                                    move |_| {
                                        if ui.location() == source {
                                            ui.review.set(None);
                                            ui.page.set("card".into());
                                        }
                                    },
                                );
                            }
                        })),
                ),
        )
        .child(crate::github::pr_panel(ui))
        .child(dynamic(move || {
            if let Some(c) = ui.current_card()
                && let Some(d) = c.delivery.clone()
                && ui.review.get().is_none()
            {
                let archive_card = c.clone();
                let archived = c.archived_at.is_some();
                let short = &d.commit[..d.commit.len().min(10)];
                return panel()
                    .border_1()
                    .border_color(SUCCESS)
                    .bg(SUCCESS_SURFACE)
                    .gap_2()
                    .child(
                        label(format!(
                            "Delivered · merged into {} ({short})",
                            d.target_branch
                        ))
                        .text_color(SUCCESS)
                        .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(muted(format!("Last accepted delivery: {} · {}", d.target_branch, d.commit)))
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
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
                                .disabled(move || ui.pending.get() || archived),
                            )
                            .child(chrome_button(Icon::Refresh, "Refresh diff", move |_| {
                                refresh_diff(ui, target)
                            })),
                    )
                    .into_element();
            }
            view().into_element()
        }))
        .child(
            panel()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(label("Delivery").font_weight(FontWeight::SEMIBOLD))
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .child(crate::github::choice(
                                    ui,
                                    mode,
                                    vec![("local", "Local merge"), ("github_pr", "GitHub PR")],
                                ))
                                .child(chrome_button(Icon::Refresh, "Refresh diff", move |_| {
                                    refresh_diff(ui, target)
                                })),
                        ),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_3()
                        .child(
                            view()
                                .w(px(200.))
                                .min_w(px(140.))
                                .gap_1()
                                .child(muted("Target branch"))
                                .child(field(target, "Existing local branch")),
                        )
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(220.))
                                .gap_1()
                                .child(muted("Commit message"))
                                .child(field(message, "Commit message")),
                        ),
                )
                .child(dynamic(move || {
                    if let Some(p) = ui.review.get()
                        && let Some(reason) = p.reason
                    {
                        let card_id = p.card_id.clone();
                        return view()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(notice(reason, Tone::Attention))
                            .child(compact_action("Terminals ↗", Variant::Quiet, move |_| {
                                crate::terminals::open_card(ui, &card_id);
                            }))
                            .into_element();
                    }
                    view().into_element()
                }))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            action(
                                move || {
                                    if mode.get() == "github_pr" {
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
                                            if mode.get_untracked() == "github_pr" {
                                                "github.pr.publish"
                                            } else {
                                                "review.merge"
                                            },
                                            json!({"card_id":p.card_id,"revision":p.revision,"token":p.token,"target_branch":p.target_branch,"message":message.get_untracked()}),
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
                                ui.pending.get()
                                    || ui.review.get().is_none_or(|p| {
                                        !p.merge_allowed
                                            || p.target_branch != target.get()
                                            || ui.current_card().is_none_or(|c| {
                                                c.revision != p.revision || c.id != p.card_id
                                            })
                                    })
                            }),
                        )
                        .child(dynamic(move || {
                            ui.current_card()
                                .and_then(|c| c.delivery)
                                .map(|d| {
                                    muted(format!(
                                        "Last accepted delivery: {} · {}",
                                        d.target_branch, d.commit
                                    ))
                                    .into_element()
                                })
                                .unwrap_or_else(|| view().into_element())
                        })),
                ),
        )
        .child(scroll().flex_1().child(dynamic(move || match ui.review.get() {
            Some(p) => diff_panel(p).into_element(),
            None => muted("Refresh diff to preview changes.").into_element(),
        })))
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
}
