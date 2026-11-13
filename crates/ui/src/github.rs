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

pub fn active_login_from(snapshot: &Snapshot, gh: &Value) -> Option<String> {
    snapshot
        .github
        .login
        .as_ref()
        .filter(|_| snapshot.github.state == "connected")
        .cloned()
        .or_else(|| {
            gh["login"]
                .as_str()
                .filter(|_| gh["state"] == "connected")
                .map(str::to_owned)
        })
}

pub fn active_login_untracked(ui: Ui) -> Option<String> {
    let snapshot = ui.snapshot.get_untracked();
    let gh = ui.github.gh_status.get_untracked();
    active_login_from(&snapshot, &gh)
}

pub fn is_connected_untracked(ui: Ui) -> bool {
    active_login_untracked(ui).is_some()
}

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
pub fn format_timestamp_relative(raw: &str) -> String {
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(raw) else {
        return raw.to_owned();
    };
    let ts_ms = parsed.timestamp_millis();
    if ts_ms <= 0 {
        return raw.to_owned();
    }
    let elapsed_secs = now().saturating_sub(ts_ms as u64) / 1000;
    if elapsed_secs < 60 {
        "just now".into()
    } else if elapsed_secs < 3600 {
        format!("{}m ago", elapsed_secs / 60)
    } else if elapsed_secs < 86_400 {
        format!("{}h ago", elapsed_secs / 3600)
    } else if elapsed_secs < 86_400 * 30 {
        format!("{}d ago", elapsed_secs / 86_400)
    } else {
        parsed.format("%b %-d, %Y").to_string()
    }
}

pub fn matching_cards(snapshot: &Snapshot, r: &GitHubReference) -> Vec<Card> {
    snapshot
        .cards
        .iter()
        .filter(|c| {
            c.removed_at.is_none()
                && (c.github_issue().is_some_and(|s| {
                    s.repository.eq_ignore_ascii_case(&r.repository)
                        && Some(s.number) == r.number
                        && c.source_kind() == r.kind
                }) || c.pull_request.as_ref().is_some_and(|p| {
                    r.kind == "pr"
                        && p.repository.eq_ignore_ascii_case(&r.repository)
                        && Some(p.number) == r.number
                }) || c.github_links.iter().any(|l| {
                    l.repository.eq_ignore_ascii_case(&r.repository)
                        && l.kind == r.kind
                        && l.number == r.number
                        && l.sha == r.sha
                }))
        })
        .cloned()
        .collect()
}

fn create_local_linked_card(
    ui: Ui,
    project_id: String,
    r: GitHubReference,
    fallback_title: String,
) {
    let kind_label = match r.kind.as_str() {
        "pr" => "PR",
        "commit" => "Commit",
        _ => "Issue",
    };
    let ref_tag = r
        .number
        .map(|n| format!("{}#{n}", r.repository))
        .or_else(|| {
            r.sha
                .as_deref()
                .map(|s| format!("{}@{}", r.repository, &s[..s.len().min(7)]))
        })
        .unwrap_or_else(|| r.repository.clone());
    let title = if fallback_title.trim().is_empty() {
        format!("{kind_label} {ref_tag}")
    } else {
        fallback_title.trim().to_owned()
    };
    let body = format!("Linked from GitHub {kind_label} [{ref_tag}]({}).", r.url);
    let link_url = r.url.clone();
    dispatch_complete(
        ui,
        "card.create",
        json!({
            "project_id": project_id,
            "title": title,
            "body": body,
        }),
        move |res| {
            if let Ok(card_val) = res
                && let (Some(card_id), Some(revision)) =
                    (card_val["id"].as_str(), card_val["revision"].as_u64())
            {
                let cid = card_id.to_owned();
                dispatch_then(
                    ui,
                    "github.reference.add",
                    json!({
                        "card_id": cid,
                        "revision": revision,
                        "url": link_url,
                    }),
                    |_| {},
                );
            }
        },
    );
}

pub fn add_to_board(ui: Ui, r: GitHubReference, fallback_title: String) {
    use nsh_core::element::desktop::{MenuItem, popup_menu};
    if r.kind == "commit" {
        let projects = ui.snapshot.with_untracked(|s| s.projects.clone());
        if projects.is_empty() {
            ui.navigate("new-project");
            return;
        }
        let current_pid = ui.project.get_untracked();
        if let Some(p) = projects
            .iter()
            .find(|p| p.id == current_pid)
            .or_else(|| (projects.len() == 1).then(|| &projects[0]))
        {
            create_local_linked_card(ui, p.id.clone(), r, fallback_title);
        } else {
            ui.spawn(async move {
                popup_menu(
                    projects
                        .into_iter()
                        .map(|p| {
                            let r = r.clone();
                            let title = fallback_title.clone();
                            let pid = p.id.clone();
                            MenuItem::new(format!("Add linked Card in {}", p.name)).on_select(
                                move || {
                                    create_local_linked_card(
                                        ui,
                                        pid.clone(),
                                        r.clone(),
                                        title.clone(),
                                    )
                                },
                            )
                        })
                        .collect(),
                )
                .await;
            });
        }
        return;
    }

    let (synced_projects, all_projects, current_project_id) = ui.snapshot.with_untracked(|s| {
        let synced: Vec<_> = s
            .projects
            .iter()
            .filter(|p| {
                p.github
                    .as_ref()
                    .is_some_and(|g| g.repository.eq_ignore_ascii_case(&r.repository))
            })
            .cloned()
            .collect();
        (synced, s.projects.clone(), ui.project.get_untracked())
    });

    if synced_projects.len() == 1 {
        dispatch(
            ui,
            "github.item.add",
            json!({
                "reference": r,
                "project_id": synced_projects[0].id,
            }),
        );
    } else if synced_projects.len() > 1 {
        ui.spawn(async move {
            popup_menu(
                synced_projects
                    .into_iter()
                    .map(|p| {
                        let r = r.clone();
                        let pid = p.id.clone();
                        MenuItem::new(format!("Add synced Card to {}", p.name)).on_select(
                            move || {
                                dispatch(
                                    ui,
                                    "github.item.add",
                                    json!({
                                        "reference": r.clone(),
                                        "project_id": pid.clone(),
                                    }),
                                )
                            },
                        )
                    })
                    .collect(),
            )
            .await;
        });
    } else if all_projects.is_empty() {
        ui.navigate("new-project");
    } else if all_projects.len() == 1 {
        let p = all_projects[0].clone();
        let pid = p.id.clone();
        let r_local = r.clone();
        let title_local = fallback_title.clone();
        ui.spawn(async move {
            popup_menu(vec![
                MenuItem::new(format!("Add linked Card to {}", p.name)).on_select(move || {
                    create_local_linked_card(ui, pid.clone(), r_local.clone(), title_local.clone());
                }),
                MenuItem::new(format!("Connect {} to {}…", p.name, r.repository))
                    .on_select(move || ui.navigate("github-repository")),
            ])
            .await;
        });
    } else {
        let preferred = all_projects
            .iter()
            .find(|p| p.id == current_project_id)
            .cloned();
        ui.spawn(async move {
            let mut items = Vec::new();
            if let Some(cur) = &preferred {
                let r_cur = r.clone();
                let title_cur = fallback_title.clone();
                let pid = cur.id.clone();
                items.push(
                    MenuItem::new(format!("Add linked Card to {} (current)", cur.name)).on_select(
                        move || {
                            create_local_linked_card(
                                ui,
                                pid.clone(),
                                r_cur.clone(),
                                title_cur.clone(),
                            )
                        },
                    ),
                );
            }
            for p in all_projects {
                if preferred.as_ref().is_some_and(|cur| cur.id == p.id) {
                    continue;
                }
                let r_item = r.clone();
                let title_item = fallback_title.clone();
                let pid = p.id.clone();
                items.push(
                    MenuItem::new(format!("Add linked Card to {}", p.name)).on_select(move || {
                        create_local_linked_card(
                            ui,
                            pid.clone(),
                            r_item.clone(),
                            title_item.clone(),
                        )
                    }),
                );
            }
            items.push(
                MenuItem::new(format!("Connect current Project to {}…", r.repository))
                    .on_select(move || ui.navigate("github-repository")),
            );
            popup_menu(items).await;
        });
    }
}

pub fn settings(ui: Ui) -> impl IntoElement {
    let account = computed(move || ui.snapshot.with(|s| s.github.clone()));
    let open = signal(account.with_untracked(|a| {
        a.state == "connected" || a.state == "authorizing" || a.error.is_some()
    }));
    effect(move || {
        let a = account.get();
        if a.state == "connected" || a.state == "authorizing" || a.error.is_some() {
            open.set(true);
        }
    });

    dynamic(move || {
        let is_open = open.get();
        let a = account.get();
        let installation_url = a.installation_url.clone();
        let mut area = view().gap_2();
        area = area.child(muted(
            "Only needed if you explicitly want repository-scoped GitHub App tokens instead of reusing `gh` CLI.",
        ));
        if a.state == "authorizing" {
            area = area.child(label("Waiting for browser authorization on GitHub…"));
            if let Some(code) = a.user_code {
                area = area.child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(muted("Device code:"))
                        .child(badge(code)),
                );
            }
            let mut actions = view().flex_row().flex_wrap().items_center().gap_2();
            if let Some(url) = a.verification_uri {
                actions = actions.child(action(
                    "Open GitHub authorization",
                    Variant::Primary,
                    move |_| open_url(ui, url.clone()),
                ));
            }
            actions = actions.child(action("Cancel sign-in", Variant::Quiet, move |_| {
                dispatch(ui, "github.auth.cancel", json!({}))
            }));
            area = area.child(actions);
        } else if a.state == "connected" {
            area = area
                .child(muted(
                    a.login
                        .map(|l| {
                            format!(
                                "Repository reads and sync override `gh` CLI with GitHub App token (@{l}). Notifications still use `gh` CLI."
                            )
                        })
                        .unwrap_or_else(|| "GitHub App token active.".into()),
                ))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(action(
                            "Manage repository installations",
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
                        .child(action("Disconnect App", Variant::Quiet, move |_| {
                            dispatch(ui, "github.auth.disconnect", json!({}))
                        })),
                );
        } else {
            area = area
                .child(muted(
                    "If you are already signed into `gh` CLI, you do not need to sign in here. GitHub App tokens cannot read GitHub Notifications.",
                ))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(action(
                            "Sign in with GitHub App",
                            Variant::Secondary,
                            move |_| {
                                dispatch_complete(
                                    ui,
                                    "github.auth.begin",
                                    json!({}),
                                    move |r| {
                                        if let Ok(v) = r
                                            && let Some(url) = v["verification_uri"].as_str()
                                        {
                                            open_url(ui, url.into());
                                        }
                                    },
                                )
                            },
                        )),
                );
        }
        if let Some(e) = a.error {
            area = area.child(notice(e, Tone::Attention));
        }
        collapsible_section_group(
            "Advanced · GitHub App Token",
            None,
            is_open,
            move |_| open.update(|v| *v = !*v),
            area,
        )
        .into_element()
    })
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
        let mut params = if is_github {
            json!({"project_id":submit_project,"number":d.selected_issue.as_ref().map(|r|&r["issue"]["number"]),"source_generation":d.selected_issue.as_ref().map(|r|&r["source_generation"]),"instructions":d.instructions,"kind":kind.get_untracked()})
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
                        // Remain on New Card with a fresh draft identity so
                        // users can create multiple Cards consecutively while
                        // preserving any post-submit input.
                        let still_here = draft.try_get().is_some()
                            && ui.page.get_untracked() == "new-card"
                            && ui.new_draft.get_untracked() == key;
                        if still_here {
                            let next = format!("new-card:{}", id());
                            drafts::put(ui, next.clone(), latest);
                            drafts::remove(ui, &key);
                            ui.new_draft.set(next);
                            crate::navigation::replace(ui);
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
        computed(move || active.get() && kind.get() != "local"),
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
                .gap_1()
                .p_3()
                .child(heading("New Card").flex_1())
                .child(crate::editor::expand(ui))
                .child(chrome_button(Icon::Close, "Close", move |_| {
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
                            .style_when(move || kind.get() == "local", |s| s.hidden())
                            .accessibility_elements_hidden(move || kind.get() == "local")
                            .child(dynamic(move || {
                                if !source_available.get() {
                                    return view()
                                        .gap_2()
                                        .child(muted(
                                            "Connect this Project to a GitHub repository first.",
                                        ))
                                        .child(action(
                                            "Connect Project repository",
                                            Variant::Secondary,
                                            move |_| ui.navigate("github-repository"),
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
                                                    "Change source",
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
                                        .label("Search GitHub Issues or PRs"),
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
                .child(view().flex_1())
                .child(compact_action("Discard", Variant::Quiet, move |_| {
                    drafts::remove(ui, &key);
                    ui.navigate("board");
                }))
                .child(
                    compact_action("Create", Variant::Primary, move |_| {
                        ui.card_submit.update(|n| *n += 1)
                    })
                    .disabled(move || !can_create.get()),
                ),
        )
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
                    .items_center()
                    .gap_2()
                    .child(chrome_button(Icon::Refresh, "Refresh PR", move |_| {
                        dispatch(ui, "github.pr.refresh", json!({"card_id":refresh}))
                    }))
                    .child(
                        compact_action("Open on GitHub", Variant::Quiet, move |_| {
                            open_url(ui, url.clone())
                        })
                        .disabled(pr.url.is_empty()),
                    ),
            );
        if pr.number > 0 && pr.state == "open" && !pr.merged {
            area = area.child(
                action("Merge PR", Variant::Primary, move |_| {
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
