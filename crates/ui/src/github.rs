//! GitHub-specific form sections; shared native controls and one Card composer.
use crate::{
    drafts::{self, Draft},
    presenter::*,
};
use canban_types::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;
use serde_json::{Value, json};
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
    let selected_project = computed(move || ui.current_project().map(|p| (p.id, p.name)));
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
    .child(dynamic(move || {
        let Some((id, name)) = selected_project.get() else {
            return muted("Select a Project to connect its repository.").into_element();
        };
        project_settings(ui, id, name).into_element()
    }))
}
fn project_settings(ui: Ui, id: String, name: String) -> impl IntoElement {
    // Sync timestamps/errors must not rebuild editable native controls.
    let config_id = id.clone();
    let saved = computed(move || {
        ui.snapshot.with(|s| {
            s.projects
                .iter()
                .find(|p| p.id == config_id)
                .map(|p| {
                    (
                        p.github
                            .as_ref()
                            .map(|g| g.repository.clone())
                            .unwrap_or_default(),
                        if p.github
                            .as_ref()
                            .is_some_and(|g| g.intake == GitHubIntake::Collaborators)
                        {
                            "collaborators"
                        } else {
                            "all"
                        }
                        .to_owned(),
                        if p.delivery_mode == DeliveryMode::GitHubPr {
                            "github_pr"
                        } else {
                            "local"
                        }
                        .to_owned(),
                    )
                })
                .unwrap_or_default()
        })
    });
    let initial = saved.get_untracked();
    let d = drafts::document(
        ui,
        format!("github-settings:{id}"),
        Draft {
            title: initial.0.clone(),
            base_title: initial.0,
            ..Default::default()
        },
    );
    let intake = signal(initial.1.clone());
    let mode = signal(initial.2.clone());
    let baseline = signal((initial.1, initial.2));
    effect(move || {
        let (repository, saved_intake, saved_mode) = saved.get();
        let mut next = d.get_untracked();
        if next.title == repository {
            next.base_title = repository.clone();
        }
        next.reconcile(Draft {
            title: repository,
            ..Default::default()
        });
        if next != d.get_untracked() {
            d.set(next);
        }
        let previous = baseline.get_untracked();
        if intake.get_untracked() == previous.0 {
            intake.set(saved_intake.clone());
        }
        if mode.get_untracked() == previous.1 {
            mode.set(saved_mode.clone());
        }
        baseline.set((saved_intake, saved_mode));
    });
    let save = id.clone();
    view().gap_3().key(format!("github-project:{id}"))
        .child(label(format!("Project: {name}")))
        .child(bound_field(move||d.get().title,move|v|d.update(|d|d.title=v.into()),"owner/repository").key("github-repository"))
        .child(label("Automatically add Open Issues"))
        .child(choice(ui,intake,vec![("all","All authors"),("collaborators","Repository collaborators only")]))
        .child(label("Default code delivery"))
        .child(choice(ui,mode,vec![("local","Local merge"),("github_pr","GitHub PR")]))
        .child(action("Save repository connection",Variant::Primary,move |_|dispatch(ui,"project.github",json!({"project_id":save,"repository":d.get_untracked().title.trim(),"intake":intake.get_untracked(),"delivery_mode":mode.get_untracked()}))).disabled(move||ui.pending.get() || d.get().title.trim().is_empty()))
        .child(dynamic(move || {
            let source = ui.snapshot.with(|s| s.projects.iter().find(|p|p.id == id).and_then(|p|p.github.clone()));
            let Some(source) = source else { return view().into_element(); };
            let sync = id.clone(); let disconnect = id.clone();
            let mut area = view().gap_2()
                .child(muted(format!("{} · {}",source.repository,source.synced_at.map(|_|"Synced").unwrap_or("Not synced yet"))))
                .child(view().flex_row().flex_wrap().gap_2().child(action("Sync now",Variant::Secondary,move |_|dispatch(ui,"github.sync",json!({"project_id":sync})))).child(action("Disconnect repository",Variant::Quiet,move |_|dispatch(ui,"github.source.disconnect",json!({"project_id":disconnect})))));
            if let Some(e)=source.error { area=area.child(notice(e,Tone::Attention)); }
            area.into_element()
        }))
}
pub fn new_card(ui: Ui) -> impl IntoElement {
    let project = ui.project.get_untracked();
    let target = ui.card_target;
    if target.get_untracked().is_empty() {
        target.set(project.clone());
    }
    let note_id = ui.source_note.get_untracked();
    let note = ui
        .snapshot
        .with_untracked(|s| s.notes.iter().find(|n| n.id == note_id).cloned());
    let key = format!("new-card:{project}:{note_id}");
    let draft = drafts::document(
        ui,
        key.clone(),
        note.as_ref()
            .map(|n| Draft {
                title: n.title.clone(),
                body: n.body.clone(),
                instructions: n.body.clone(),
                ..Default::default()
            })
            .unwrap_or_default(),
    );
    let kind = signal(
        if draft.get_untracked().card_kind == "github" {
            "github"
        } else {
            "local"
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
        let enabled = kind.get() == "github";
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
                        json!({"project_id":project,"query":query}),
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
    let source_project = note.as_ref().map(|n| n.project_id.clone());
    let submit_key = key.clone();
    let submit_note = note_id.clone();
    let can_create = computed(move || {
        !ui.pending.get()
            && !target.get().is_empty()
            && if kind.get() == "github" {
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
        let is_github = kind.get_untracked() == "github";
        let submit_project = target.get_untracked();
        let mut params = if is_github {
            json!({"project_id":submit_project,"number":d.selected_issue.as_ref().map(|r|&r["issue"]["number"]),"source_generation":d.selected_issue.as_ref().map(|r|&r["source_generation"]),"instructions":d.instructions})
        } else {
            json!({"project_id":submit_project,"title":d.title,"body":d.body,"tags":d.tags.split(',').map(str::trim).filter(|s|!s.is_empty()).collect::<Vec<_>>()})
        };
        if !submit_note.is_empty() && source_project.as_deref() == Some(submit_project.as_str()) {
            params["source_note_id"] = json!(submit_note);
        }
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
    let mut form = view()
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
                    move |_| kind.set("github".into()),
                )),
        );
    if let Some(n) = note {
        form = form.child(dynamic(move || {
            let text = if target.get() == n.project_id {
                format!("Source Note: {} · included in this Card's context", n.title)
            } else {
                format!(
                    "Copied from {} · Note link stays in the original Project",
                    n.title
                )
            };
            notice(text, Tone::Info)
        }));
    }
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
                        .child(
                            bound_field(
                                move || draft.get().tags,
                                move |v| draft.update(|d| d.tags = v.into()),
                                "Routing tags",
                            )
                            .key("local-card-tags"),
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
                            move |_| ui.navigate("settings"),
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
                        .label("Search GitHub Issues"),
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
        let mut area = panel().child(
            view()
                .flex_row()
                .gap_2()
                .items_center()
                .child(icon(Icon::GitHub))
                .child(action(
                    format!("{}#{}", source.repository, source.number),
                    Variant::Quiet,
                    move |_| open_url(ui, source.url.clone()),
                )),
        );
        if let Some(i) = cached {
            area = area
                .child(view().flex_row().child(badge(i.state)))
                .child(nsh_base::text_view().markdown(i.body));
        }
        area.child(action(
            "Remove from Board · keep GitHub Issue",
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
