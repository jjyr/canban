//! GitHub-specific surfaces, shared with the native Gallery.
mod detail;
pub mod filters;
pub mod references;
mod repository;
mod sync;
pub use sync::{sync_button, sync_hint};
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
pub use workspace::{Browser, init, open, open_section, screen};

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
    ui.snapshot.with_untracked(|snapshot| {
        ui.github
            .gh_status
            .with_untracked(|gh| active_login_from(snapshot, gh))
    })
}

pub fn is_connected_untracked(ui: Ui) -> bool {
    active_login_untracked(ui).is_some()
}

pub fn open_url(ui: Ui, url: String) {
    if !url.starts_with("https://github.com/") {
        ui.operation_error.set("Invalid GitHub URL".into());
        return;
    }
    if !crate::backend::is_connected() {
        ui.notify(format!("Fixture: opens {url}"));
        return;
    }
    ui.spawn(async move {
        let result = blocking(move || crate::platform::open_external(&url)).await;
        if !matches!(result, Ok(Ok(()))) {
            ui.operation_error
                .set("Could not open GitHub in the browser".into());
        }
    });
}

/// Resolves the browser jump URL for a GitHub comment, inline PR thread comment, or review.
pub fn comment_web_url(entry: &Value, item_url: Option<&str>, section: &str) -> Option<String> {
    if let Some(url) = entry["html_url"]
        .as_str()
        .filter(|u| u.starts_with("https://github.com/"))
    {
        return Some(url.to_owned());
    }
    let cid = entry["id"].as_u64().filter(|&n| n > 0)?;
    let base = item_url
        .filter(|u| u.starts_with("https://github.com/"))?
        .split('#')
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    let anchor = match section {
        "threads" => format!("discussion_r{cid}"),
        "reviews" => format!("pullrequestreview-{cid}"),
        _ => format!("issuecomment-{cid}"),
    };
    Some(format!("{base}#{anchor}"))
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
            "mentions": [],
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
            let Some(backend) = crate::backend::get() else {
                loading.try_set(false);
                return;
            };
            ui.spawn(async move {
                let result = blocking(move || {
                    backend.call_local(
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
    let instructions_suppressed = signal(Vec::<String>::new());
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
        let profiles = ui.profiles.get_untracked();
        let mut params = if is_github {
            let mentions = crate::profiles::active_recipients(
                &[&d.instructions],
                &[],
                &instructions_suppressed.get_untracked(),
                &profiles,
            );
            json!({"project_id":submit_project,"number":d.selected_issue.as_ref().map(|r|&r["issue"]["number"]),"source_generation":d.selected_issue.as_ref().map(|r|&r["source_generation"]),"instructions":d.instructions,"kind":kind.get_untracked(),"mentions":mentions})
        } else {
            let mentions = crate::profiles::active_recipients(
                &[&d.title, &d.body],
                &d.recipients,
                &d.suppressed_recipients,
                &profiles,
            );
            json!({"project_id":submit_project,"title":d.title,"body":d.body,"mentions":mentions})
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
                                instructions_suppressed.try_set(Vec::new());
                            }
                            if latest.selected_issue == d.selected_issue {
                                latest.selected_issue = None;
                            }
                        } else {
                            if latest.title == d.title && latest.body == d.body {
                                latest.recipients.clear();
                                latest.suppressed_recipients.clear();
                            }
                            if latest.title == d.title {
                                latest.title.clear();
                            }
                            if latest.body == d.body {
                                latest.body.clear();
                            }
                        }
                        // The presenter opens the acknowledged Card. Retain
                        // post-submit input in Drafts for explicit restoration.
                        if latest.has_content(&key) {
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
    let local_title = crate::card::title_proxy(draft);
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
    effect(move || {
        let profiles = ui.profiles.get();
        let t = local_title.get();
        let b = local.get();
        let cur = draft.with_untracked(|d| d.suppressed_recipients.clone());
        if cur.is_empty() {
            return;
        }
        let present = crate::profiles::active_recipients(&[&t, &b], &[], &[], &profiles);
        let next: Vec<String> = cur.into_iter().filter(|id| present.contains(id)).collect();
        if next != draft.with_untracked(|d| d.suppressed_recipients.clone()) {
            draft.update(|d| d.suppressed_recipients = next);
        }
    });
    effect(move || {
        let profiles = ui.profiles.get();
        let text = instructions.get();
        let cur = instructions_suppressed.get_untracked();
        if cur.is_empty() {
            return;
        }
        let present = crate::profiles::active_recipients(&[&text], &[], &[], &profiles);
        let next: Vec<String> = cur.into_iter().filter(|id| present.contains(id)).collect();
        if next != instructions_suppressed.get_untracked() {
            instructions_suppressed.set(next);
        }
    });
    let height = signal(280.);
    let active = computed(move || ui.page.get() == "new-card");
    effect(move || {
        if !active.get() {
            return;
        }
        on_key_with(
            "escape",
            KeyOptions {
                when: nsh_core::keymap::When::Always,
                ..Default::default()
            },
            move |_| {
                if ui.page.get_untracked() == "new-card" && !ui.has_blocking_overlay() {
                    ui.navigate("board");
                }
            },
        );
    });
    let on_local_mention: std::rc::Rc<dyn Fn(String)> = std::rc::Rc::new(move |id| {
        draft.update(|d| {
            d.suppressed_recipients.retain(|p| p != &id);
        });
    });
    let (title_editor, title_view) = crate::editor::title(
        ui,
        local_title,
        computed(move || target.get()),
        computed(move || active.get() && kind.get() == "local"),
        true,
        || false,
        move || ui.card_submit.update(|n| *n += 1),
        Some(on_local_mention.clone()),
    );
    let (local_editor, local_view) = crate::editor::body_with_mentions(
        ui,
        local,
        computed(move || target.get()),
        computed(move || active.get() && kind.get() == "local"),
        height,
        move || ui.card_submit.update(|n| *n += 1),
        Some(on_local_mention),
    );
    let (instructions_editor, instructions_view) = crate::editor::body_with_mentions(
        ui,
        instructions,
        computed(move || target.get()),
        computed(move || active.get() && kind.get() != "local"),
        height,
        move || ui.card_submit.update(|n| *n += 1),
        Some(std::rc::Rc::new(move |id| {
            instructions_suppressed.update(|list| list.retain(|p| p != &id));
        })),
    );
    effect(move || {
        if !active.get() {
            title_editor.blur();
            local_editor.blur();
            instructions_editor.blur();
        } else if kind.get() == "local" {
            instructions_editor.blur();
        } else {
            title_editor.blur();
            local_editor.blur();
        }
    });
    view()
        .key(key.clone())
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .bg(canban_ui_kit::tokens::SURFACE)
        .on_layout(move |b: &Bounds<Points>| height.set((b.size.height.0 - 350.).max(150.)))
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
                            .child(title_view)
                            .child(local_view)
                            .child(when(
                                move || {
                                    let profiles = ui.profiles.get();
                                    let d = draft.get();
                                    !crate::profiles::active_recipients(
                                        &[&d.title, &d.body],
                                        &d.recipients,
                                        &d.suppressed_recipients,
                                        &profiles,
                                    )
                                    .is_empty()
                                },
                                move || {
                                    crate::editor::recipient_chips(
                                        ui,
                                        vec![local_title, local],
                                        move || draft.with(|d| d.recipients.clone()),
                                        move || draft.with(|d| d.suppressed_recipients.clone()),
                                        move |id, still_in_prose| {
                                            draft.update(|d| {
                                                d.recipients.retain(|p| p != &id);
                                                if still_in_prose
                                                    && !d.suppressed_recipients.contains(&id)
                                                {
                                                    d.suppressed_recipients.push(id);
                                                }
                                            });
                                        },
                                    )
                                },
                            )),
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
                            .child(instructions_view)
                            .child(when(
                                move || {
                                    let profiles = ui.profiles.get();
                                    let text = instructions.get();
                                    !crate::profiles::active_recipients(
                                        &[&text],
                                        &[],
                                        &instructions_suppressed.get(),
                                        &profiles,
                                    )
                                    .is_empty()
                                },
                                move || {
                                    crate::editor::recipient_chips(
                                        ui,
                                        vec![instructions],
                                        Vec::new,
                                        move || instructions_suppressed.get(),
                                        move |id, still_in_prose| {
                                            if still_in_prose {
                                                instructions_suppressed.update(|list| {
                                                    if !list.contains(&id) {
                                                        list.push(id);
                                                    }
                                                });
                                            }
                                        },
                                    )
                                },
                            )),
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
                        compact_action("Open on GitHub ↗", Variant::Quiet, move |_| {
                            open_url(ui, url.clone())
                        })
                        .disabled(pr.url.is_empty()),
                    )
                    .child(when(
                        {
                            let has_checks = pr.number > 0 && !pr.url.is_empty();
                            move || has_checks
                        },
                        {
                            let checks_url = pr
                                .checks_summary
                                .as_ref()
                                .and_then(|s| s.url.clone())
                                .unwrap_or_else(|| {
                                    format!("{}/checks", pr.url.trim_end_matches('/'))
                                });
                            move || {
                                let url = checks_url.clone();
                                compact_action("Open CI Checks ↗", Variant::Quiet, move |_| {
                                    open_url(ui, url.clone())
                                })
                            }
                        },
                    )),
            );
        if pr.number > 0 && pr.state == "open" && !pr.merged {
            area = area.child(
                action("Merge PR", Variant::Primary, move |_| {
                    let cur_rev = ui
                        .snapshot
                        .with_untracked(|s| {
                            s.cards.iter().find(|c| c.id == merge).map(|c| c.revision)
                        })
                        .unwrap_or(rev);
                    dispatch(
                        ui,
                        "github.pr.merge",
                        json!({"card_id":merge,"revision":cur_rev,"head":head}),
                    )
                })
                .disabled(move || ui.pending.get()),
            );
        }
        area.into_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_web_url_uses_html_url_and_builds_fallback_anchors_per_section() {
        let explicit = json!({
            "id": 601,
            "html_url": "https://github.com/acme/app/pull/42#discussion_r601"
        });
        assert_eq!(
            comment_web_url(
                &explicit,
                Some("https://github.com/acme/app/pull/42"),
                "threads"
            )
            .as_deref(),
            Some("https://github.com/acme/app/pull/42#discussion_r601")
        );

        let fallback = json!({"id": 77});
        assert_eq!(
            comment_web_url(
                &fallback,
                Some("https://github.com/acme/app/issues/12"),
                "discussion"
            )
            .as_deref(),
            Some("https://github.com/acme/app/issues/12#issuecomment-77")
        );
        assert_eq!(
            comment_web_url(
                &fallback,
                Some("https://github.com/acme/app/pull/12/"),
                "threads"
            )
            .as_deref(),
            Some("https://github.com/acme/app/pull/12#discussion_r77")
        );
        assert_eq!(
            comment_web_url(
                &fallback,
                Some("https://github.com/acme/app/pull/12#files"),
                "reviews"
            )
            .as_deref(),
            Some("https://github.com/acme/app/pull/12#pullrequestreview-77")
        );

        // Rejects non-GitHub URLs and missing IDs.
        let untrusted =
            json!({"id": 77, "html_url": "https://evil.example/pull/12#issuecomment-77"});
        assert!(
            comment_web_url(
                &untrusted,
                Some("https://evil.example/pull/12"),
                "discussion"
            )
            .is_none()
        );
        assert!(
            comment_web_url(
                &json!({"id": 0}),
                Some("https://github.com/acme/app/issues/12"),
                "discussion"
            )
            .is_none()
        );
    }

    #[test]
    fn opening_new_card_focuses_title_editor() {
        use nsh_core::{ApproxTextEngine, Command, Core, MountItem, NodeId, geometry::size};
        use std::collections::HashMap;

        #[component]
        fn NewCardHost() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            ui.project.set("proj-1".into());
            ui.snapshot.update(|s| {
                s.projects.push(canban_types::Project {
                    id: "proj-1".into(),
                    name: "Canban".into(),
                    directory: "/tmp/canban".into(),
                    github: None,
                    detected_github: None,
                    github_disconnected: false,
                    delivery_mode: Default::default(),
                    agent_overrides: Default::default(),
                    tags: Default::default(),
                    workflow: canban_types::Workflow::default(),
                    target_branch: Some("main".into()),
                    created_at: 0,
                });
            });
            view()
                .w(px(1200.0))
                .h(px(800.0))
                .child(
                    view()
                        .test_id("open-new-card")
                        .h(px(20.0))
                        .on_press(move || ui.new_card()),
                )
                .child(crate::workspace::screen(ui))
        }

        fn drain(
            core: &mut Core<ApproxTextEngine>,
            labels: &mut HashMap<NodeId, Option<String>>,
            open_btn: &mut Option<NodeId>,
            focus_commands: &mut Vec<NodeId>,
        ) {
            for _ in 0..10 {
                let mut events = Vec::new();
                core.take_events(&mut events);
                for ev in events {
                    match ev {
                        nsh_core::element::JsEvent::LayoutListener {
                            host,
                            id,
                            listener,
                            frame,
                        } => core.component_layout(host, id, &listener, frame),
                        nsh_core::element::JsEvent::Event(id, kind, payload) => {
                            core.component_event(id, kind, &payload);
                        }
                        _ => {}
                    }
                }
                let Some(batch) = core.commit() else {
                    if !core.has_events() {
                        break;
                    }
                    continue;
                };
                for item in batch.items {
                    match item {
                        MountItem::ViewProps { id, props } => {
                            if props.test_id.as_deref() == Some("open-new-card") {
                                *open_btn = Some(id);
                            }
                            labels.insert(id, props.accessibility.label.map(|s| s.to_string()));
                        }
                        MountItem::Command {
                            id,
                            command: Command::Focus,
                        } => {
                            focus_commands.push(id);
                        }
                        _ => {}
                    }
                }
            }
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1200.0), px(800.0)));
        core.mount_root(&nsh_core::element::Root::of::<NewCardHost>());
        let mut labels = HashMap::new();
        let mut open_btn = None;
        let mut focus_commands = Vec::new();
        drain(&mut core, &mut labels, &mut open_btn, &mut focus_commands);
        assert!(focus_commands.is_empty());

        core.component_event(
            open_btn.expect("open-new-card button"),
            nsh_core::EventKind::Press,
            &nsh_core::EventPayload::None,
        );
        drain(&mut core, &mut labels, &mut open_btn, &mut focus_commands);

        let focused_labels: Vec<_> = focus_commands
            .iter()
            .filter_map(|id| labels.get(id).and_then(|l| l.as_deref()))
            .collect();
        assert_eq!(focused_labels, vec!["Card title"]);
    }
}
