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
    let target = ui.card_target;
    let key = ui.new_draft.get_untracked();
    let draft = drafts::document(
        ui,
        key.clone(),
        Draft {
            project_id: target.get_untracked(),
            ..Default::default()
        },
    );
    if !draft.get_untracked().project_id.is_empty() {
        target.set(draft.get_untracked().project_id);
    }
    effect(move || {
        let id = target.get();
        if draft.get_untracked().project_id != id {
            draft.update(|d| d.project_id = id);
        }
    });
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
    let submit_key = key.clone();
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
            json!({"project_id":submit_project,"title":d.title,"body":d.body})
        };
        params["request_key"] = json!(format!(
            "{}:{}",
            submit_key,
            canban_context::digest(&serde_json::to_vec(&params).expect("JSON value"))
        ));
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
                match result {
                    Ok(_) => {
                        // Preserve anything entered after the submitted snapshot.
                        let mut latest = draft.try_get().unwrap_or_else(|| {
                            ui.drafts
                                .with(|m| m.get(&key).cloned().unwrap_or(d.clone()))
                        });
                        if is_github {
                            if latest.instructions == d.instructions {
                                latest.instructions.clear();
                            }
                            if latest.selected_issue == d.selected_issue {
                                latest.selected_issue = None;
                            }
                        } else {
                            if latest.title == d.title {
                                latest.title.clear();
                            }
                            if latest.body == d.body {
                                latest.body.clear();
                            }
                        }
                        // If navigation did not leave this form (for example an
                        // expansion during submission), continue with a fresh
                        // draft identity, keeping all post-submit input.
                        let still_here = draft.try_get().is_some()
                            && ui.page.get_untracked() == "new-card"
                            && ui.new_draft.get_untracked() == key;
                        if still_here {
                            let next = format!("new-card:{}", id());
                            drafts::put(ui, next.clone(), latest);
                            drafts::remove(ui, &key);
                            ui.new_draft.set(next);
                        } else if latest.has_content(&key) {
                            drafts::put(ui, key, latest);
                        } else {
                            drafts::remove(ui, &key);
                        }
                    }
                    Err(error) => {
                        let mut latest = draft.try_get().unwrap_or_else(|| {
                            ui.drafts.with(|m| m.get(&key).cloned().unwrap_or(d))
                        });
                        latest.error = error;
                        drafts::put(ui, key, latest.clone());
                        draft.try_set(latest);
                    }
                }
            },
        );
    });
    let local = crate::card::proxy(draft, false);
    let instructions = signal(draft.get_untracked().instructions);
    effect(move || {
        let value = instructions.get();
        if draft.get_untracked().instructions != value {
            draft.update(|d| d.instructions = value);
        }
    });
    effect(move || {
        let value = draft.get().instructions;
        if instructions.get_untracked() != value {
            instructions.set(value);
        }
    });
    let height = signal(280.);
    let active = computed(move || ui.page.get() == "new-card");
    let (_local_editor, local_view) = crate::editor::body(
        ui,
        local,
        computed(move || target.get()),
        computed(move || active.get() && kind.get() == "local"),
        height,
        move || ui.card_submit.update(|n| *n += 1),
    );
    let (_, instructions_view) = crate::editor::body(
        ui,
        instructions,
        computed(move || target.get()),
        computed(move || active.get() && kind.get() == "github"),
        height,
        move || ui.card_submit.update(|n| *n += 1),
    );
    view()
        .key(key.clone())
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .bg(canban_ui_kit::tokens::SURFACE)
        .on_layout(move |b: &Bounds<Points>| height.set((b.size.height.0 - 300.).max(180.)))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .p_3()
                .child(heading("New Card").flex_1())
                .child(crate::editor::expand(ui))
                .child(action("Close", Variant::Quiet, move |_| {
                    ui.navigate("board")
                })),
        )
        .child(
            scroll().flex_1().min_h(px(0.)).child(
                view()
                    .p_3()
                    .gap_3()
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
                        .accessibility_label(|| Some("Choose target Project".into())),
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
                    )
                    .child(
                        view()
                            .gap_3()
                            .style_when(move || kind.get() != "local", |s| s.hidden())
                            .accessibility_elements_hidden(move || kind.get() != "local")
                            .child(
                                bound_field(
                                    move || draft.get().title,
                                    move |v| draft.update(|d| d.title = v.into()),
                                    "Card title",
                                )
                                .label("Card title")
                                .auto_focus(true),
                            )
                            .child(local_view),
                    )
                    .child(
                        view()
                            .gap_3()
                            .style_when(move || kind.get() != "github", |s| s.hidden())
                            .accessibility_elements_hidden(move || kind.get() != "github")
                            .child(dynamic(move || {
                                if !source_available.get() {
                                    return view()
                                        .gap_2()
                                        .child(muted(
                                            "Connect this Project to a GitHub repository first.",
                                        ))
                                        .child(action(
                                            "Open GitHub settings",
                                            Variant::Secondary,
                                            move |_| ui.navigate("settings"),
                                        ))
                                        .into_element();
                                }
                                if let Some(row) = selected_issue.get() {
                                    let issue = row["issue"].clone();
                                    return view()
                                        .gap_2()
                                        .child(
                                            view()
                                                .flex_row()
                                                .gap_2()
                                                .child(icon(Icon::GitHub))
                                                .child(label(format!(
                                                    "#{} · {}",
                                                    issue["number"],
                                                    issue["title"].as_str().unwrap_or_default()
                                                )))
                                                .child(action(
                                                    "Change Issue",
                                                    Variant::Quiet,
                                                    move |_| {
                                                        draft.update(|d| d.selected_issue = None)
                                                    },
                                                )),
                                        )
                                        .child(
                                            crate::editor::reader(ui, move || {
                                                issue["body"]
                                                    .as_str()
                                                    .unwrap_or_default()
                                                    .to_owned()
                                            })
                                            .max_h(px(220.)),
                                        )
                                        .into_element();
                                }
                                view()
                                    .gap_2()
                                    .child(
                                        bound_field(
                                            move || draft.get().issue_query,
                                            move |v| draft.update(|d| d.issue_query = v.into()),
                                            "Search by title or #number",
                                        )
                                        .label("Search GitHub Issues"),
                                    )
                                    .child(dynamic(move || {
                                        let mut results = view().gap_2();
                                        if loading.get() {
                                            results = results.child(muted("Searching…"));
                                        }
                                        if !search_error.get().is_empty() {
                                            results = results.child(notice(
                                                move || search_error.get(),
                                                Tone::Attention,
                                            ));
                                        }
                                        for row in rows.get() {
                                            let issue = &row["issue"];
                                            let existing = row["card_id"]
                                                .as_str()
                                                .map(str::to_owned)
                                                .filter(|_| row["removed"] != true);
                                            let title = format!(
                                                "#{} {} · {}",
                                                issue["number"],
                                                issue["title"].as_str().unwrap_or_default(),
                                                if existing.is_some() {
                                                    "Open existing"
                                                } else {
                                                    issue["state"].as_str().unwrap_or_default()
                                                }
                                            );
                                            results = results.child(action(
                                                title,
                                                Variant::Secondary,
                                                move |_| {
                                                    if let Some(id) = &existing {
                                                        ui.open_card(id.clone());
                                                    } else {
                                                        draft.update(|d| {
                                                            d.selected_issue = Some(row.clone())
                                                        });
                                                    }
                                                },
                                            ));
                                        }
                                        results
                                    }))
                                    .into_element()
                            }))
                            .child(muted("Additional instructions · local to Canban"))
                            .child(instructions_view),
                    )
                    .child(crate::editor::draft_status(ui, key.clone(), draft))
                    .child(when(
                        move || !draft.get().error.is_empty(),
                        move || notice(move || draft.get().error, Tone::Danger),
                    )),
            ),
        )
        .child(
            view()
                .p_3()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(action("Drafts", Variant::Quiet, move |_| {
                    ui.navigate("drafts")
                }))
                .child(view().flex_1())
                .child(action("Discard draft", Variant::Quiet, move |_| {
                    drafts::remove(ui, &key);
                    ui.navigate("board");
                }))
                .child(
                    action("Create · ⌘Enter", Variant::Primary, move |_| {
                        ui.card_submit.update(|n| *n += 1)
                    })
                    .disabled(move || !can_create.get()),
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
