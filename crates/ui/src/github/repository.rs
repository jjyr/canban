use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Rules {
    #[serde(default = "all_scope")]
    issue_scope: String,
    #[serde(default = "all_scope")]
    pr_scope: String,
    intake: String,
    pr_intake: String,
    authors: String,
    labels: String,
    drafts: bool,
    paused: bool,
    delivery: String,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            issue_scope: "all".into(),
            pr_scope: "all".into(),
            intake: "off".into(),
            pr_intake: "off".into(),
            authors: String::new(),
            labels: String::new(),
            drafts: false,
            paused: false,
            delivery: "local".into(),
        }
    }
}

fn rules(d: Signal<Draft>) -> Rules {
    serde_json::from_str(&d.get().body).unwrap_or_default()
}

fn rules_untracked(d: Signal<Draft>) -> Rules {
    serde_json::from_str(&d.get_untracked().body).unwrap_or_default()
}

fn edit(d: Signal<Draft>, f: impl FnOnce(&mut Rules)) {
    let mut r = rules_untracked(d);
    f(&mut r);
    if serde_json::to_string(&r).ok().as_deref() != Some(d.get_untracked().body.as_str()) {
        d.update(|d| d.body = serde_json::to_string(&r).unwrap());
    }
}

fn enum_name<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .unwrap()
        .as_str()
        .unwrap_or("")
        .into()
}

fn split_csv(s: String) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn all_scope() -> String {
    "all".into()
}
fn controlled_choice(
    _ui: Ui,
    value: impl Fn() -> String + Copy + 'static,
    changed: impl Fn(String) + Copy + 'static,
    options: Vec<(&'static str, &'static str)>,
) -> impl IntoElement {
    use canban_ui_kit::picker::{PickerOption, search_popover};
    let labels = options.clone();
    search_popover(
        action(
            move || {
                let current = value();
                format!(
                    "{} ▾",
                    labels
                        .iter()
                        .find(|(id, _)| *id == current)
                        .map(|(_, label)| *label)
                        .unwrap_or("")
                )
            },
            Variant::Secondary,
            |_| {},
        ),
        move || {
            let selected = value();
            options
                .iter()
                .map(|(id, label)| PickerOption::new(*id, *label).selected(*id == selected))
                .collect()
        },
        changed,
    )
    .width(280.)
    .list_height(200.)
}
fn rule_choice(
    ui: Ui,
    d: Signal<Draft>,
    pr: bool,
    options: Vec<(&'static str, &'static str)>,
) -> impl IntoElement {
    controlled_choice(
        ui,
        move || {
            if pr {
                rules(d).pr_intake
            } else {
                rules(d).intake
            }
        },
        move |v| {
            edit(d, |r| {
                if pr {
                    r.pr_intake = v.clone();
                    r.pr_scope = v;
                } else {
                    r.intake = v.clone();
                    r.issue_scope = v;
                }
            })
        },
        options,
    )
}

pub fn settings(ui: Ui, id: String, name: String) -> impl IntoElement {
    let source = id.clone();
    let saved = computed(move || {
        ui.snapshot.with(|s| {
            s.projects
                .iter()
                .find(|p| p.id == source)
                .map(|p| {
                    let mut r = Rules {
                        delivery: enum_name(&p.delivery_mode),
                        ..Default::default()
                    };
                    let repo = if let Some(g) = &p.github {
                        r.intake = enum_name(&g.intake);
                        r.pr_intake = enum_name(&g.pr_intake);
                        if r.intake != "off" {
                            r.issue_scope = r.intake.clone();
                        }
                        if r.pr_intake != "off" {
                            r.pr_scope = r.pr_intake.clone();
                        }
                        r.authors = g.authors.join(", ");
                        r.labels = g.labels.join(", ");
                        r.drafts = g.include_drafts;
                        r.paused = g.paused;
                        g.repository.clone()
                    } else if let Some(det) = &p.detected_github {
                        det.repository.clone()
                    } else {
                        String::new()
                    };
                    (repo, r)
                })
                .unwrap_or_default()
        })
    });
    let initial = saved.get_untracked();
    let body = serde_json::to_string(&initial.1).unwrap();
    let d = drafts::document(
        ui,
        format!("github-repository:{id}"),
        Draft {
            title: initial.0.clone(),
            base_title: initial.0.clone(),
            body: body.clone(),
            base_body: body,
            ..Default::default()
        },
    );
    let candidates = signal(Value::Null);
    let candidate_error = signal(String::new());
    let repository_info = signal(Value::Null);
    if crate::backend::is_connected() {
        let pid = id.clone();
        ui.spawn(async move {
            let result = blocking(move || {
                crate::routed_call("github.repositories", json!({"project_id":pid}), Some(&pid))
            })
            .await;
            match result {
                Ok(Ok(v)) => {
                    candidates.try_set(v);
                }
                other => {
                    candidate_error.try_set(format!("Repository discovery failed: {other:?}"));
                }
            }
        });
    }
    let saved_repo = computed(move || saved.get().0);
    let info_ticket = signal(0u64);
    let info_pid = id.clone();
    effect(move || {
        let repo = saved_repo.get();
        let ticket = info_ticket.get_untracked() + 1;
        info_ticket.set(ticket);
        if repo.is_empty() {
            if !repository_info.get_untracked().is_null() {
                repository_info.set(Value::Null);
            }
            return;
        }
        if !crate::backend::is_connected() {
            return;
        }
        let pid = info_pid.clone();
        ui.spawn(async move {
            let result = blocking(move || {
                crate::routed_call(
                    "github.repository.info",
                    json!({"project_id":pid,"repository":repo}),
                    Some(&pid),
                )
            })
            .await;
            if info_ticket.try_get() != Some(ticket) {
                return;
            }
            match result {
                Ok(Ok(info)) => {
                    let _ = repository_info.try_set(info);
                }
                _ => {
                    if repository_info.try_get().is_some_and(|v| !v.is_null()) {
                        let _ = repository_info.try_set(Value::Null);
                    }
                }
            }
        });
    });
    let baseline = signal(initial.1);
    effect(move || {
        let (repository, remote) = saved.get();
        let old = baseline.get_untracked();
        let mut current =
            serde_json::from_str::<Rules>(&d.get_untracked().body).unwrap_or_default();
        macro_rules! merge {
            ($($field:ident),*) => {
                $(if current.$field == old.$field {
                    current.$field = remote.$field.clone();
                })*
            };
        }
        merge!(intake, pr_intake, authors, labels, drafts, paused, delivery);
        let mut next = d.get_untracked();
        {
            let d = &mut next;
            let next_title = if d.title == d.base_title {
                repository.clone()
            } else {
                d.title.clone()
            };
            let next_body = serde_json::to_string(&current).unwrap();
            if d.title != next_title || d.base_title != repository || d.body != next_body {
                d.title = next_title;
                d.base_title = repository;
                d.body = next_body;
            }
            d.base_body = serde_json::to_string(&remote).unwrap();
        }
        if next != d.get_untracked() {
            d.set(next);
        }
        baseline.set(remote);
    });
    let save = id.clone();
    let sync = id.clone();
    let disconnect = id.clone();
    let status_id = id.clone();
    let detected_id = id.clone();
    let banner_id = id.clone();
    let sync_hint_id = id.clone();
    let badge_state = computed(move || {
        ui.snapshot.with(|s| {
            s.projects
                .iter()
                .find(|p| p.id == status_id)
                .and_then(|p| p.github.as_ref().map(|g| (g.repository.clone(), g.paused)))
        })
    });
    let detected_state = computed(move || {
        ui.snapshot.with(|s| {
            s.projects
                .iter()
                .find(|p| p.id == detected_id)
                .map(|p| (p.github.is_some(), p.detected_github.clone()))
                .unwrap_or((false, None))
        })
    });
    let sync_hint_text = computed(move || {
        ui.snapshot.with(|s| {
            s.projects
                .iter()
                .find(|p| p.id == sync_hint_id)
                .map(|project| super::sync_hint(project, s.github_sync.get(&sync_hint_id), now()))
                .unwrap_or_default()
        })
    });

    view()
        .max_w(px(780.))
        .gap_6()
        .pb_6()
        .key(format!("github-repository:{id}"))
        .child(
            section_group(format!("Repository Binding · {name}"))
                .description(
                    "Connect this Project to a GitHub repository so synced Cards and PR deliveries map directly.",
                )
                .trailing(dynamic(move || {
                    if let Some((repository, paused)) = badge_state.get() {
                        status(
                            if paused {
                                format!("{repository} · Paused")
                            } else {
                                format!("Connected · {repository}")
                            },
                            if paused { Tone::Neutral } else { Tone::Success },
                        )
                        .into_element()
                    } else {
                        status("Not linked", Tone::Neutral).into_element()
                    }
                }))
                .child(
                    view()
                        .gap_2()
                        .child(dynamic(move || {
                            let (connected, detected) = detected_state.get();
                            let Some(det) = detected.filter(|_| !connected) else {
                                return view().into_element();
                            };
                            let link_pid = banner_id.clone();
                            let link_repo = det.repository.clone();
                            view()
                                .p_3()
                                .rounded(px(canban_ui_kit::tokens::RADIUS))
                                .bg(canban_ui_kit::tokens::INSET)
                                .border_1()
                                .border_color(canban_ui_kit::tokens::BORDER)
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(muted(format!(
                                    "Detected `{}` from git remote `{}`.",
                                    det.repository, det.remote
                                )))
                                .child(
                                    compact_action(
                                        format!("Link {}", det.remote),
                                        Variant::Secondary,
                                        move |_| {
                                            let r = rules_untracked(d);
                                            d.update(|draft| draft.title = link_repo.clone());
                                            dispatch(
                                                ui,
                                                "project.github",
                                                json!({
                                                    "project_id": link_pid,
                                                    "repository": link_repo,
                                                    "intake": r.intake,
                                                    "pr_intake": r.pr_intake,
                                                    "authors": split_csv(r.authors),
                                                    "labels": split_csv(r.labels),
                                                    "include_drafts": r.drafts,
                                                    "paused": r.paused,
                                                    "delivery_mode": r.delivery,
                                                }),
                                            );
                                        },
                                    )
                                    .disabled(move || ui.is_method_pending("project.github")),
                                )
                                .into_element()
                        }))
                        .child(
                            bound_field(
                                move || d.get().title,
                                move |v| d.update(|d| d.title = v.into()),
                                "owner/repository",
                            )
                            .key("github-repository"),
                        )
                        .child(dynamic(move || {
                            let (connected, detected) = detected_state.get();
                            let Some(det) = detected.filter(|_| connected) else {
                                return view().into_element();
                            };
                            let differs = !d
                                .get()
                                .title
                                .trim()
                                .eq_ignore_ascii_case(&det.repository);
                            let mut row = view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .child(muted(format!("Detected from git remote `{}`", det.remote)));
                            if differs {
                                let restore_repo = det.repository.clone();
                                row = row
                                    .child(muted("·"))
                                    .child(compact_action(
                                        format!("Use {}", det.remote),
                                        Variant::Quiet,
                                        move |_| {
                                            d.update(|draft| draft.title = restore_repo.clone());
                                        },
                                    ));
                            }
                            row.into_element()
                        }))
                        .child(dynamic(move || {
                            let data = candidates.get();
                            let mut area = view().gap_2();
                            for candidate in data["candidates"].as_array().into_iter().flatten() {
                                let repo = candidate["repository"].as_str().unwrap_or("").to_owned();
                                let label = format!("Use {} · {}", candidate["remote"].as_str().unwrap_or("remote"), repo);
                                area = area.child(compact_action(label, Variant::Quiet, move |_| d.update(|d| d.title = repo.clone())));
                            }
                            let info = repository_info.get();
                            if let Some(parent) = info["parent"]["full_name"].as_str() {
                                let parent = parent.to_owned();
                                area = area.child(muted(format!("{} is a fork of {parent}", info["full_name"].as_str().unwrap_or("Repository"))))
                                    .child(compact_action("Use upstream repository", Variant::Secondary, move |_| d.update(|d| d.title = parent.clone())));
                            }
                            if !candidate_error.get().is_empty() { area = area.child(notice(candidate_error.get(), Tone::Attention)); }
                            area
                        }))
                        .child(toggle("Pause automatic synchronization", move || rules(d).paused,
                            move |enabled| edit(d, |r| r.paused = enabled))),
                ),
        )
        .child(
            section_group("Automatic Intake")
                .description(
                    "New matches enter Backlog without starting an Agent. Updating rules preserves existing Cards.",
                )
                .child(
                    view()
                        .gap_3()
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap_4()
                                .child(
                                    view()
                                        .gap_1()
                                        .child(toggle("Automatically sync Issues", move || rules(d).intake != "off", move |enabled| edit(d, |r| {
                                            r.intake = if enabled { r.issue_scope.clone() } else { "off".into() };
                                        })))
                                        .child(when(move || rules(d).intake != "off", move || rule_choice(
                                            ui, d, false,
                                            vec![
                                                ("all", "All authors"),
                                                ("collaborators", "Repository collaborators"),
                                            ],
                                        ))),
                                )
                                .child(
                                    view()
                                        .gap_1()
                                        .child(toggle("Automatically sync pull requests", move || rules(d).pr_intake != "off", move |enabled| edit(d, |r| {
                                            r.pr_intake = if enabled { r.pr_scope.clone() } else { "off".into() };
                                        })))
                                        .child(when(move || rules(d).pr_intake != "off", move || rule_choice(
                                            ui, d, true,
                                            vec![
                                                ("all", "All authors"),
                                                ("collaborators", "Repository collaborators"),
                                                ("review_requested", "Review requested from me"),
                                                ("assigned", "Assigned to me"),
                                                ("authored", "Created by me"),
                                            ],
                                        ))),
                                ),
                        )
                        .child(
                            bound_field(
                                move || rules(d).authors,
                                move |v| edit(d, |r| r.authors = v.into()),
                                "Authors filter: comma-separated logins, or empty for everyone",
                            )
                            .key("github-rule-authors"),
                        )
                        .child(
                            bound_field(
                                move || rules(d).labels,
                                move |v| edit(d, |r| r.labels = v.into()),
                                "Labels filter: match any of these, or empty for all",
                            )
                            .key("github-rule-labels"),
                        )
                        .child(toggle("Include Draft PRs", move || rules(d).drafts,
                            move |enabled| edit(d, |r| r.drafts = enabled))),
                ),
        )
        .child(
            section_group("Code Delivery")
                .description("Choose how completed Card branches are delivered by default.")
                .child(controlled_choice(
                    ui, move || rules(d).delivery, move |v| edit(d, |r| r.delivery = v),
                    vec![("local", "Local merge"), ("github_pr", "GitHub PR")],
                )),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    action(move || if ui.is_method_pending("project.github") { "Saving…" } else { "Save repository settings" }, Variant::Primary, move |_| {
                        let r = rules_untracked(d);
                        dispatch(
                            ui,
                            "project.github",
                            json!({
                                "project_id": save,
                                "repository": d.get_untracked().title.trim(),
                                "intake": r.intake,
                                "pr_intake": r.pr_intake,
                                "authors": split_csv(r.authors),
                                "labels": split_csv(r.labels),
                                "include_drafts": r.drafts,
                                "paused": r.paused,
                                "delivery_mode": r.delivery,
                            }),
                        );
                    })
                    .disabled(move || {
                        ui.is_method_pending("project.github") || d.get().title.trim().is_empty()
                    }),
                )
                .child(super::sync_button(ui, sync))
                .child(compact_action("Disconnect", Variant::Quiet, move |_| {
                    dispatch(
                        ui,
                        "github.source.disconnect",
                        json!({"project_id": disconnect}),
                    )
                })),
        )
        .child(view().child(muted(move || sync_hint_text.get())))
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let active_project = computed(move || {
        let pid = ui.project.get();
        ui.index.with(|idx| {
            idx.projects
                .get(&pid)
                .map(|p| (p.id.clone(), p.name.clone()))
        })
    });
    view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_4()
        .child(scroll().flex_1().min_h(px(0.)).child(dynamic(move || {
            let project = active_project.get();
            untracked(|| {
                if let Some((id, name)) = project {
                    settings(ui, id, name).into_element()
                } else {
                    muted("Select or create a Project first.").into_element()
                }
            })
        })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{
        DetectedGitHub, GitHubIntake, GitHubPrIntake, GitHubSource, Project, Snapshot, Workflow,
    };
    use nsh_core::{ApproxTextEngine, Core, MountItem, geometry::size};

    fn collect_texts(root: nsh_core::element::Root) -> Vec<String> {
        let mut core = Core::new(ApproxTextEngine, size(px(900.), px(700.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&root);
        let batch = core.commit().expect("initial mount batch");
        batch
            .items
            .into_iter()
            .filter_map(|item| match item {
                MountItem::Text { text, .. } => Some(text.text.to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn repository_binding_defaults_to_intake_off_and_renders_detected_git_remote_states() {
        assert_eq!(Rules::default().intake, "off");
        assert_eq!(Rules::default().pr_intake, "off");

        #[component]
        fn DisconnectedDetectedProbe() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let mut snapshot = Snapshot::default();
            snapshot.projects.push(Project {
                id: "proj-1".into(),
                name: "Canban".into(),
                directory: "/tmp/canban".into(),
                github: None,
                detected_github: Some(DetectedGitHub {
                    repository: "jjyr/canban".into(),
                    remote: "origin".into(),
                }),
                github_disconnected: true,
                delivery_mode: Default::default(),
                agent_overrides: Default::default(),
                tags: Vec::new(),
                workflow: Workflow::default(),
                target_branch: Some("main".into()),
                created_at: 1,
            });
            ui.snapshot.set(snapshot);
            ui.project.set("proj-1".into());
            screen(ui)
        }

        let disconnected_texts =
            collect_texts(nsh_core::element::Root::of::<DisconnectedDetectedProbe>());
        assert!(
            disconnected_texts
                .iter()
                .any(|t| t == "Detected `jjyr/canban` from git remote `origin`."),
            "expected detected remote banner when unlinked, got {disconnected_texts:?}"
        );
        assert!(
            disconnected_texts.iter().any(|t| t == "Link origin"),
            "expected `Link origin` button when unlinked, got {disconnected_texts:?}"
        );
        assert!(
            disconnected_texts.iter().any(|t| t == "Not linked"),
            "expected `Not linked` status badge, got {disconnected_texts:?}"
        );

        #[component]
        fn ConnectedDetectedProbe() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let mut snapshot = Snapshot::default();
            snapshot.projects.push(Project {
                id: "proj-1".into(),
                name: "Canban".into(),
                directory: "/tmp/canban".into(),
                github: Some(GitHubSource {
                    pr_intake: GitHubPrIntake::Off,
                    authors: Vec::new(),
                    labels: Vec::new(),
                    include_drafts: false,
                    paused: false,
                    repository: "jjyr/canban".into(),
                    repository_id: 1,
                    generation: "gen-1".into(),
                    intake: GitHubIntake::Off,
                    synced_at: None,
                    error: None,
                }),
                detected_github: Some(DetectedGitHub {
                    repository: "jjyr/canban".into(),
                    remote: "origin".into(),
                }),
                github_disconnected: false,
                delivery_mode: Default::default(),
                agent_overrides: Default::default(),
                tags: Vec::new(),
                workflow: Workflow::default(),
                target_branch: Some("main".into()),
                created_at: 1,
            });
            ui.snapshot.set(snapshot);
            ui.project.set("proj-1".into());
            screen(ui)
        }

        let connected_texts =
            collect_texts(nsh_core::element::Root::of::<ConnectedDetectedProbe>());
        assert!(
            connected_texts
                .iter()
                .any(|t| t == "Connected · jjyr/canban"),
            "expected connected badge, got {connected_texts:?}"
        );
        assert!(
            connected_texts
                .iter()
                .any(|t| t == "Detected from git remote `origin`"),
            "expected detected remote caption when connected, got {connected_texts:?}"
        );
    }

    #[test]
    fn repository_screen_stays_mounted_across_unrelated_snapshot_updates() {
        use nsh_core::{EventKind, EventPayload, ROOT_ID};

        fn sample_project(id: &str, name: &str, repo: &str) -> Project {
            Project {
                id: id.into(),
                name: name.into(),
                directory: format!("/tmp/{id}"),
                github: Some(GitHubSource {
                    pr_intake: GitHubPrIntake::Off,
                    authors: Vec::new(),
                    labels: Vec::new(),
                    include_drafts: false,
                    paused: false,
                    repository: repo.into(),
                    repository_id: 1,
                    generation: format!("gen-{id}"),
                    intake: GitHubIntake::Off,
                    synced_at: None,
                    error: None,
                }),
                detected_github: Some(DetectedGitHub {
                    repository: repo.into(),
                    remote: "origin".into(),
                }),
                github_disconnected: false,
                delivery_mode: Default::default(),
                agent_overrides: Default::default(),
                tags: Vec::new(),
                workflow: Workflow::default(),
                target_branch: Some("main".into()),
                created_at: 1,
            }
        }

        #[component]
        fn StabilityProbe() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let mut snapshot = Snapshot::default();
            snapshot
                .projects
                .push(sample_project("proj-1", "Canban", "jjyr/canban"));
            snapshot
                .projects
                .push(sample_project("proj-2", "Other", "jjyr/other"));
            ui.snapshot.set(snapshot);
            ui.project.set("proj-1".into());
            view()
                .w(px(900.))
                .h(px(700.))
                .on_press(move |_: &()| {
                    ui.snapshot.update(|s| {
                        s.github_sync.insert(
                            "proj-2".into(),
                            GitHubSyncStatus {
                                generation: "gen-proj-2".into(),
                                phase: "issues".into(),
                                ..Default::default()
                            },
                        );
                        s.projects[1].target_branch = Some("develop".into());
                    });
                })
                .child(
                    compact_action("Switch project", Variant::Quiet, move |_| {
                        ui.project.set("proj-2".into());
                    })
                    .accessibility_label("probe-switch-project"),
                )
                .child(screen(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(900.), px(700.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<StabilityProbe>());
        let initial = core.commit().expect("initial mount");
        let host = initial
            .items
            .iter()
            .find_map(|item| match item {
                MountItem::Insert { parent, child, .. } if *parent == ROOT_ID => Some(*child),
                _ => None,
            })
            .expect("root host node");
        let find_text = |items: &[MountItem], expected: &str| {
            items.iter().find_map(|item| match item {
                MountItem::Text { id, text, .. } if text.text == expected => Some(*id),
                _ => None,
            })
        };
        let heading_id =
            find_text(&initial.items, "Repository Binding · Canban").expect("heading text");
        let badge_id = find_text(&initial.items, "Connected · jjyr/canban").expect("badge text");
        let detected_id =
            find_text(&initial.items, "Detected from git remote `origin`").expect("detected text");
        let save_id =
            find_text(&initial.items, "Save repository settings").expect("save button text");
        let switch_btn = initial
            .items
            .iter()
            .find_map(|item| match item {
                MountItem::ViewProps { id, props }
                    if props.accessibility.label.as_deref() == Some("probe-switch-project") =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .expect("switch project button");

        assert!(core.component_event(host, EventKind::Press, &EventPayload::None));
        if let Some(update) = core.commit() {
            let watched = [heading_id, badge_id, detected_id, save_id];
            assert!(
                !update.items.iter().any(|item| matches!(
                    item,
                    MountItem::Delete { id } if watched.contains(id)
                )),
                "github-repository nodes must not be deleted/remounted on unrelated snapshot updates: {:?}",
                update.items
            );
            assert!(
                !update.items.iter().any(|item| matches!(
                    item,
                    MountItem::Text { text, .. }
                        if matches!(
                            text.text.as_ref(),
                            "Repository Binding · Canban"
                                | "Connected · jjyr/canban"
                                | "Detected from git remote `origin`"
                                | "Save repository settings"
                        )
                )),
                "github-repository texts must not be re-emitted on unrelated snapshot updates: {:?}",
                update.items
            );
        }

        // Switching the active project must remount settings for the new project.
        assert!(core.component_event(switch_btn, EventKind::Press, &EventPayload::None));
        let switched = core.commit().expect("project switch commit");
        assert!(
            find_text(&switched.items, "Repository Binding · Other").is_some(),
            "expected switched project heading, got {:?}",
            switched.items
        );
    }
}
