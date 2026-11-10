//! GitHub-specific surfaces, shared with the native Gallery.
mod detail;
pub mod filters;
pub mod references;
mod repository;
pub mod workspace;
use crate::{
    drafts::{self, Draft},
    presenter::*,
};
use canban_types::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;
pub use repository::screen as repository_screen;
use serde_json::{Value, json};
pub use workspace::account;
pub use workspace::{Browser, init, open, screen};
pub fn open_url(ui: Ui, url: String) {
    if !url.starts_with("https://github.com/") {
        ui.operation_error.set("Invalid GitHub URL".into());
        return;
    }
    ui.spawn(async move {
        let result = blocking(move || {
            std::process::Command::new("/usr/bin/open")
                .arg(url)
                .status()
        })
        .await;
        if !matches!(result,Ok(Ok(s)) if s.success()) {
            ui.operation_error
                .set("Could not open GitHub in the browser".into());
        }
    });
}
pub fn choice(
    ui: Ui,
    value: Signal<String>,
    options: Vec<(&'static str, &'static str)>,
) -> impl IntoElement {
    use nsh_core::element::desktop::{MenuItem, popup_menu};
    dynamic(move || {
        let choices = options.clone();
        let text = choices
            .iter()
            .find(|(v, _)| *v == value.get())
            .map(|(_, t)| *t)
            .unwrap_or("");
        action(format!("{text} ▾"), Variant::Secondary, move |_| {
            let choices = choices.clone();
            ui.spawn(async move {
                popup_menu(
                    choices
                        .into_iter()
                        .map(|(id, text)| {
                            MenuItem::new(text).on_select(move || {
                                value.try_set(id.into());
                            })
                        })
                        .collect(),
                )
                .await;
            });
        })
    })
}
pub fn settings(ui: Ui) -> impl IntoElement {
    let account = computed(move || ui.snapshot.with(|s| s.github.clone()));
    settings_section(
        "GitHub",
        "Connect your account, then choose a repository for the selected Project.",
    )
    .child(dynamic(move || {
        let a = account.get();
        let installation_url = a.installation_url.clone();
        let mut area = view().gap_2().child(label(
            a.login
                .map(|l| format!("Connected as {l}"))
                .unwrap_or_else(|| {
                    if a.state == "authorizing" {
                        "Waiting for GitHub authorization".into()
                    } else {
                        "Not connected".into()
                    }
                }),
        ));
        if a.state == "authorizing" {
            if let Some(code) = a.user_code {
                area = area.child(label(format!("Verification code: {code}")));
            }
            if let Some(url) = a.verification_uri {
                area = area.child(action(
                    "Open GitHub authorization",
                    Variant::Primary,
                    move |_| open_url(ui, url.clone()),
                ));
            }
            area = area.child(action("Cancel sign-in", Variant::Quiet, move |_| {
                dispatch(ui, "github.auth.cancel", json!({}))
            }));
        } else if a.state == "connected" {
            area = area.child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .child(action(
                        "Manage repository access",
                        Variant::Secondary,
                        move |_| {
                            if let Some(url) = &installation_url {
                                open_url(ui, url.clone());
                            }
                        },
                    ))
                    .child(action("Reconnect", Variant::Quiet, move |_| {
                        dispatch(ui, "github.auth.begin", json!({}))
                    }))
                    .child(action("Disconnect account", Variant::Quiet, move |_| {
                        dispatch(ui, "github.auth.disconnect", json!({}))
                    })),
            );
        } else {
            area = area.child(action("Connect GitHub", Variant::Primary, move |_| {
                dispatch_complete(ui, "github.auth.begin", json!({}), move |r| {
                    if let Ok(v) = r
                        && let Some(url) = v["verification_uri"].as_str()
                    {
                        open_url(ui, url.into());
                    }
                })
            }));
        }
        if let Some(e) = a.error {
            area = area.child(notice(e, Tone::Attention));
        }
        area.into_element()
    }))
    .child(action(
        "Configure current Project repository",
        Variant::Secondary,
        move |_| ui.navigate("github-repository"),
    ))
}
pub fn new_card(ui: Ui) -> impl IntoElement {
    let project = ui.project.get_untracked();
    let target = ui.card_target;
    if target.get_untracked().is_empty() {
        target.set(project.clone());
    }
    let key = format!("new-card:{project}");
    let draft = drafts::document(ui, key.clone(), Draft::default());
    let kind = signal(
        match draft.get_untracked().card_kind.as_str() {
            "github" => "github",
            "pr" => "pr",
            _ => "local",
        }
        .to_owned(),
    );
    effect(move || {
        let k = kind.get();
        draft.update(|d| d.card_kind = k);
    });
    let rows = signal(Vec::<Value>::new());
    let loading = signal(false);
    let search_error = signal(String::new());
    let epoch = signal(0u64);
    let previous_target = signal(target.get_untracked());
    effect(move || {
        let next = target.get();
        if next != previous_target.get_untracked() {
            draft.update(|d| d.selected_issue = None);
            previous_target.set(next);
        }
    });
    let query_text = computed(move || draft.with(|d| d.issue_query.clone()));
    let selected_issue = computed(move || draft.with(|d| d.selected_issue.clone()));
    let current_source = computed(move || {
        ui.snapshot.with(|s| {
            s.projects
                .iter()
                .find(|p| p.id == target.get())
                .and_then(|p| p.github.clone())
        })
    });
    let source_available = computed(move || current_source.get().is_some());
    effect(move || {
        let query = query_text.get();
        let project = target.get();
        let _source_generation = current_source.get().map(|s| s.generation);
        let enabled = kind.get() != "local";
        let search_kind = kind.get();
        epoch.update(|n| *n += 1);
        let generation = epoch.get_untracked();
        rows.set(Vec::new());
        search_error.set(String::new());
        if !enabled || query.trim().is_empty() {
            loading.set(false);
            return;
        }
        loading.set(true);

        set_timeout(std::time::Duration::from_millis(250), move || {
            if epoch.try_get() != Some(generation) {
                return;
            }
            let Some(app) = crate::APP.get().cloned() else {
                loading.try_set(false);
                return;
            };
            ui.spawn(async move {
                let result = blocking(move || {
                    app.call(
                        "github.issue.search",
                        json!({"project_id":project,"query":query,"kind":search_kind}),
                    )
                })
                .await;
                if epoch.try_get() != Some(generation) {
                    return;
                }
                loading.try_set(false);
                match result {
                    Ok(Ok(v)) => {
                        rows.try_set(v.as_array().cloned().unwrap_or_default());
                    }
                    Ok(Err(e)) => {
                        search_error.try_set(format!("{e:#}"));
                    }
                    Err(e) => {
                        search_error.try_set(e.to_string());
                    }
                }
            });
        });
    });
    let submit_key = key.clone();
    let can_create = computed(move || {
        !ui.pending.get()
            && !target.get().is_empty()
            && if kind.get() != "local" {
                draft.get().selected_issue.is_some()
            } else {
                !draft.get().title.trim().is_empty()
            }
    });
    // Button, command help and keyboard all dispatch the same scoped form intent.
    let seen_submit = stored(ui.card_submit.get_untracked());
    effect(move || {
        let ticket = ui.card_submit.get();
        if seen_submit.replace(ticket) == ticket
            || ui.page.get_untracked() != "new-card"
            || !ui.utility_popup.get_untracked().is_empty()
            || ui.quick_open.get_untracked()
            || !can_create.get_untracked()
        {
            return;
        }
        let d = draft.get_untracked();
        let is_github = kind.get_untracked() != "local";
        let submit_project = target.get_untracked();
        let params = if is_github {
            json!({"project_id":submit_project,"number":d.selected_issue.as_ref().map(|r|&r["issue"]["number"]),"source_generation":d.selected_issue.as_ref().map(|r|&r["source_generation"]),"instructions":d.instructions,"kind":kind.get_untracked()})
        } else {
            json!({"project_id":submit_project,"title":d.title,"body":d.body})
        };
        let key = submit_key.clone();
        dispatch_complete(
            ui,
            if is_github {
                "github.issue.add"
            } else {
                "card.create"
            },
            params,
            move |result| {
                if result.is_ok() {
                    ui.drafts.with(|d| {
                        d.remove(&key);
                    });
                }
            },
        );
    });
    let form = view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_3()
        .child(heading("New Card"))
        .child(
            action(
                move || {
                    format!(
                        "Project: {} ▾",
                        ui.snapshot.with(|s| s
                            .projects
                            .iter()
                            .find(|p| p.id == target.get())
                            .map(|p| p.name.clone())
                            .unwrap_or_else(|| "Choose Project".into()))
                    )
                },
                Variant::Secondary,
                move |_| crate::utilities::open(ui, "card-project"),
            )
            .self_start()
            .max_w(px(800.))
            .accessibility_label(move || {
                Some(
                    format!(
                        "Choose target Project · {}",
                        ui.snapshot.with(|s| s
                            .projects
                            .iter()
                            .find(|p| p.id == target.get())
                            .map(|p| p.name.clone())
                            .unwrap_or_else(|| "None selected".into()))
                    )
                    .into(),
                )
            }),
        )
        .child(
            segmented()
                .child(segment(
                    "Local",
                    None,
                    move || kind.get() == "local",
                    move |_| kind.set("local".into()),
                ))
                .child(segment(
                    "GitHub Issue",
                    Some(Icon::GitHub),
                    move || kind.get() == "github",
                    move |_| {
                        kind.set("github".into());
                        draft.update(|d| d.selected_issue = None);
                    },
                ))
                .child(segment(
                    "GitHub PR",
                    Some(Icon::GitHub),
                    move || kind.get() == "pr",
                    move |_| {
                        kind.set("pr".into());
                        draft.update(|d| d.selected_issue = None);
                    },
                )),
        );
    form.child(
        scroll()
            .flex_1()
            .min_h(px(0.))
            .child(panel().max_w(px(800.)).child(dynamic(move || {
                if kind.get() == "local" {
                    return view()
                        .key("local-card-editor")
                        .gap_3()
                        .child(
                            bound_field(
                                move || draft.get().title,
                                move |v| draft.update(|d| d.title = v.into()),
                                "Card title",
                            )
                            .key("local-card-title"),
                        )
                        .child(
                            bound_field(
                                move || draft.get().body,
                                move |v| draft.update(|d| d.body = v.into()),
                                "Task description",
                            )
                            .key("local-card-body")
                            .multiline(true)
                            .h(px(220.)),
                        )
                        .into_element();
                }
                if !source_available.get() {
                    return view()
                        .gap_2()
                        .child(muted("Connect this Project to a GitHub repository first."))
                        .child(action(
                            "Open GitHub settings",
                            Variant::Secondary,
                            move |_| ui.navigate("github-account"),
                        ))
                        .into_element();
                }
                let selected = selected_issue.get();
                let mut area = view().key("github-card-editor").gap_3().child(muted(
                    current_source
                        .get_untracked()
                        .map(|s| s.repository)
                        .unwrap_or_default(),
                ));
                if let Some(row) = selected {
                    let i = row["issue"].clone();
                    area = area
                        .child(
                            view()
                                .key("github-source-preview")
                                .flex_row()
                                .gap_2()
                                .child(icon(Icon::GitHub))
                                .child(label(format!(
                                    "#{} · {}",
                                    i["number"],
                                    i["title"].as_str().unwrap_or("")
                                )))
                                .child(action("Change Issue", Variant::Quiet, move |_| {
                                    draft.update(|d| d.selected_issue = None)
                                })),
                        )
                        .child(
                            nsh_base::text_view()
                                .markdown(i["body"].as_str().unwrap_or("").to_owned())
                                .max_h(px(220.)),
                        );
                } else {
                    area = area.child(
                        bound_field(
                            move || draft.get().issue_query,
                            move |v| draft.update(|d| d.issue_query = v.into()),
                            "Search by title or #number",
                        )
                        .key("github-issue-search")
                        .label("Search GitHub Issues or PRs"),
                    );
                    area = area.child(dynamic(move || {
                        let mut results = view().gap_2();
                        if loading.get() {
                            results = results.child(muted("Searching…"));
                        }
                        if !search_error.get().is_empty() {
                            results = results.child(notice(search_error.get(), Tone::Attention));
                        }
                        for row in rows.get() {
                            let issue = &row["issue"];
                            let existing = row["card_id"]
                                .as_str()
                                .map(str::to_owned)
                                .filter(|_| row["removed"] != true);
                            let text = format!(
                                "#{}  {} · {}",
                                issue["number"],
                                issue["title"].as_str().unwrap_or(""),
                                if existing.is_some() {
                                    "Open existing"
                                } else {
                                    issue["state"].as_str().unwrap_or("")
                                }
                            );
                            results = results.child(action(text, Variant::Secondary, move |_| {
                                if let Some(id) = &existing {
                                    ui.open_card(id.clone());
                                } else {
                                    draft.update(|d| d.selected_issue = Some(row.clone()));
                                }
                            }));
                        }
                        if !loading.get()
                            && search_error.get().is_empty()
                            && !query_text.get().trim().is_empty()
                            && rows.get().is_empty()
                        {
                            results = results.child(muted("No matching Issues"));
                        }
                        results
                    }));
                }
                area.child(
                    bound_field(
                        move || draft.get().instructions,
                        move |v| draft.update(|d| d.instructions = v.into()),
                        "Additional instructions · local to Canban",
                    )
                    .key("github-issue-instructions")
                    .multiline(true)
                    .h(px(160.)),
                )
                .into_element()
            }))),
    )
    .child(
        view()
            .flex_row()
            .gap_2()
            .child(action("Cancel", Variant::Quiet, move |_| {
                ui.navigate("board")
            }))
            .child(
                view()
                    .tooltip(
                        nsh_base::tooltip()
                            .placement(Placement::Top)
                            .child(label("Create · ⌘Enter")),
                    )
                    .child(
                        action("Create", Variant::Primary, move |_| {
                            ui.card_submit.update(|n| *n += 1)
                        })
                        .accessibility_label(|| Some("Create · ⌘Enter".into()))
                        .disabled(move || !can_create.get()),
                    ),
            ),
    )
}
pub fn card_source(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let Some(c) = ui.current_card() else {
            return view().into_element();
        };
        let Some(source) = c.github_issue().cloned() else {
            return view().into_element();
        };
        let cached = ui.snapshot.with(|s| {
            s.github_issues
                .iter()
                .find(|i| {
                    i.project_id == c.project_id
                        && i.source.issue_id == source.issue_id
                        && i.source.repository_id == source.repository_id
                })
                .cloned()
        });
        let source_kind = c.source_kind();
        let mut area = panel().child(
            view()
                .flex_row()
                .gap_2()
                .items_center()
                .child(icon(Icon::GitHub))
                .child(action(
                    format!("{}#{}", source.repository, source.number),
                    Variant::Quiet,
                    move |_| {
                        open(
                            ui,
                            GitHubReference {
                                repository: source.repository.clone(),
                                kind: source_kind.into(),
                                number: Some(source.number),
                                sha: None,
                                url: source.url.clone(),
                            },
                        )
                    },
                )),
        );
        if let Some(i) = cached {
            area = area
                .child(view().flex_row().child(badge(i.state)))
                .child(nsh_base::text_view().markdown(i.body));
        }
        area.child(action(
            "Remove from Board · keep GitHub source",
            Variant::Quiet,
            move |_| {
                dispatch(
                    ui,
                    "card.remove",
                    json!({"card_id":c.id,"revision":c.revision}),
                )
            },
        ))
        .into_element()
    })
}
pub fn pr_panel(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let Some(c) = ui.current_card() else {
            return view().into_element();
        };
        let Some(pr) = c.pull_request else {
            return view().into_element();
        };
        let refresh = c.id.clone();
        let merge = c.id.clone();
        let head = pr.head.clone();
        let rev = c.revision;
        let url = pr.url.clone();
        let mut area = panel()
            .child(
                view()
                    .flex_row()
                    .gap_2()
                    .items_center()
                    .child(icon(Icon::GitHub))
                    .child(label(if pr.number > 0 {
                        format!(
                            "Delivery PR #{} · {}",
                            pr.number,
                            if pr.merged { "merged" } else { &pr.state }
                        )
                    } else {
                        if pr.state == "not_published" {
                            "No delivery PR · refresh the diff to publish".into()
                        } else {
                            "PR publication needs reconciliation".into()
                        }
                    })),
            )
            .child(muted(format!(
                "{} → {} · Checks: {} · {} · {}",
                pr.head_branch,
                pr.base_branch,
                pr.checks,
                pr.review_summary,
                pr.head.chars().take(10).collect::<String>()
            )))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .child(action("Refresh PR", Variant::Secondary, move |_| {
                        dispatch(ui, "github.pr.refresh", json!({"card_id":refresh}))
                    }))
                    .child(
                        action("Open on GitHub", Variant::Quiet, move |_| {
                            open_url(ui, url.clone())
                        })
                        .disabled(pr.url.is_empty()),
                    ),
            );
        if pr.number > 0 && pr.state == "open" && !pr.merged {
            area = area.child(
                action("Accept · merge on GitHub", Variant::Primary, move |_| {
                    dispatch(
                        ui,
                        "github.pr.merge",
                        json!({"card_id":merge,"revision":rev,"head":head}),
                    )
                })
                .disabled(move || ui.pending.get()),
            );
        }
        area.into_element()
    })
}
