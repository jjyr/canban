//! Standalone Canban-managed Git branch worktree creation, tree actions,
//! Card promotion, and guarded deletion.
use super::*;
use canban_types::ManagedWorktree;
use nsh_core::element::desktop::{MenuItem, popup_menu, popup_menu_at};

pub fn entry_in_worktree(e: &Entry, wt: &ManagedWorktree) -> bool {
    if e.card.is_some() {
        return false;
    }
    let Some(cwd) = e.cwd() else {
        return false;
    };
    if e.machine() != wt.location.machine {
        return false;
    }
    cwd == wt.path || cwd.starts_with(&format!("{}/", wt.path.trim_end_matches('/')))
}

pub fn open_shell(ui: Ui, wt: &ManagedWorktree) {
    if crate::APP.get().is_none() {
        let id = format!("gallery-wt-shell-{}", canban_types::id());
        let saved = canban_types::TerminalEntry {
            id: id.clone(),
            conversation_id: None,
            number: ui
                .snapshot
                .with_untracked(|s| s.terminals.iter().map(|t| t.number).max().unwrap_or(8))
                + 1,
            name: Some(wt.branch.clone()),
            manual_number: Some(
                ui.snapshot.with_untracked(|s| {
                    s.terminals
                        .iter()
                        .filter_map(|t| t.manual_number)
                        .max()
                        .unwrap_or(0)
                }) + 1,
            ),
            origin_card_id: None,
            agent_engine: None,
            created_at: canban_types::now(),
            runtime: Some(canban_types::TerminalRuntime {
                machine: wt.location.machine.clone(),
                owner_core_id: None,
                core_id: Some("gallery".into()),
                terminal: Some(id.clone()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
            }),
        };
        let ctx = canban_types::TerminalContext {
            terminal_id: id.clone(),
            machine: wt.location.machine.clone(),
            core_id: "gallery".into(),
            terminal: id.clone(),
            cwd: Some(wt.path.clone()),
            branch: Some(wt.branch.clone()),
            project_id: Some(wt.project_id.clone()),
            card_id: None,
        };
        ui.snapshot.update(|s| {
            s.terminals.push(saved);
            s.terminal_contexts.push(ctx);
        });
        ui.open_terminal(id);
        return;
    }
    launch_custom(
        ui,
        json!({
            "project_id": wt.project_id,
            "machine": wt.location.machine,
            "cwd": wt.path,
            "name": wt.branch,
        }),
    );
}

pub fn open_agent(ui: Ui, wt: &ManagedWorktree, profile_id: &str) {
    if crate::APP.get().is_none() {
        open_shell(ui, wt);
        return;
    }
    launch_custom(
        ui,
        json!({
            "project_id": wt.project_id,
            "machine": wt.location.machine,
            "cwd": wt.path,
            "profile_id": profile_id,
            "name": format!("{} · @{profile_id}", wt.branch),
        }),
    );
}

pub fn open_diff(ui: Ui, wt: &ManagedWorktree) {
    ui.project.set(wt.project_id.clone());
    crate::review::select_git_pair(ui, wt.base_branch.clone(), wt.branch.clone());
    ui.navigate("review");
}

pub fn request_remove(ui: Ui, wt_id: String, force: bool) {
    if crate::APP.get().is_none() {
        ui.snapshot
            .update(|s| s.worktrees.retain(|w| w.id != wt_id));
        ui.worktree_remove_confirm.set(None);
        ui.notify("Worktree deleted");
        return;
    }
    let wid = wt_id.clone();
    dispatch_complete(
        ui,
        "worktree.remove",
        json!({"worktree_id": wt_id, "force": force}),
        move |res| match res {
            Ok(_) => {
                ui.worktree_remove_confirm.set(None);
            }
            Err(err) => {
                if !force && err.to_lowercase().contains("uncommitted") {
                    ui.operation_error.set(String::new());
                    ui.worktree_remove_confirm.set(Some(wid));
                }
            }
        },
    );
}

pub async fn menu(ui: Ui, wt: ManagedWorktree, at: Option<Point<Points>>) {
    let wt_shell = wt.clone();
    let wt_diff = wt.clone();
    let wt_promote = wt.id.clone();
    let wt_copy = wt.path.clone();
    let wt_del = wt.id.clone();

    let mut items = vec![
        MenuItem::new(format!("⎇ {} (base: {})", wt.branch, wt.base_branch)).disabled(true),
        MenuItem::new(wt.path.clone()).disabled(true),
        MenuItem::separator(),
        MenuItem::new("Open Shell in Worktree").on_select(move || open_shell(ui, &wt_shell)),
    ];

    for p in ui.profiles.get_untracked() {
        let wt_agent = wt.clone();
        let pid = p.id.clone();
        items.push(
            MenuItem::new(format!("Launch @{} CLI in Worktree", p.id))
                .on_select(move || open_agent(ui, &wt_agent, &pid)),
        );
    }

    items.push(MenuItem::separator());
    items.push(MenuItem::new("Open in Diff & Review").on_select(move || open_diff(ui, &wt_diff)));
    items.push(
        MenuItem::new("Promote to Board Card…")
            .on_select(move || ui.worktree_promote.set(Some(wt_promote.clone()))),
    );
    items.push(
        MenuItem::new("Copy Worktree Path").on_select(move || copy_text(ui, wt_copy.clone())),
    );
    items.push(MenuItem::separator());
    items.push(
        MenuItem::new("Delete Worktree…")
            .on_select(move || request_remove(ui, wt_del.clone(), false)),
    );

    if let Some(pos) = at {
        popup_menu_at(items, pos).await;
    } else {
        popup_menu(items).await;
    }
    ui.terminal_focus.update(|v| *v += 1);
}

pub fn modals(ui: Ui) -> impl IntoElement {
    view()
        .child(create_modal(ui))
        .child(promote_modal(ui))
        .child(remove_confirm_modal(ui))
}

fn create_modal(ui: Ui) -> impl IntoElement {
    when(
        move || ui.worktree_modal.get().is_some(),
        move || {
            let initial_project = ui
                .worktree_modal
                .get_untracked()
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    let cur = ui.project.get_untracked();
                    if !cur.is_empty() {
                        Some(cur)
                    } else {
                        ui.snapshot
                            .with_untracked(|s| s.projects.first().map(|p| p.id.clone()))
                    }
                })
                .unwrap_or_default();
            let project_id = signal(initial_project);
            let branch = signal(String::new());
            let base_branch = signal(String::new());
            let sync_base = signal(false);
            let launch_mode = signal("shell".to_owned());
            let profile_id = signal(crate::sessions::default_profile(ui));
            let error = signal(String::new());

            let close_modal = move || {
                ui.worktree_modal.set(None);
                ui.terminal_focus.update(|n| *n += 1);
            };

            let submit = move || {
                let pid = project_id.get_untracked();
                let br = branch.get_untracked().trim().to_owned();
                if pid.is_empty() {
                    error.set("Choose a Project first".into());
                    return;
                }
                if br.is_empty() {
                    error.set("Enter a Git branch name (for example feat/my-branch)".into());
                    return;
                }
                let base = base_branch.get_untracked().trim().to_owned();
                let sync = sync_base.get_untracked();
                let launch = launch_mode.get_untracked();
                let prof = profile_id.get_untracked();

                if crate::APP.get().is_none() {
                    let wt = ManagedWorktree {
                        id: format!("wt-{}", canban_types::id()),
                        project_id: pid.clone(),
                        branch: br.clone(),
                        base_branch: if base.is_empty() { "main".into() } else { base },
                        base_commit: Some("0123456789abcdef".into()),
                        path: format!("/preview/worktrees/{br}"),
                        location: canban_types::WorkLocation {
                            machine: "local".into(),
                            directory: "/preview/source".into(),
                        },
                        created_at: canban_types::now(),
                    };
                    let wt_open = wt.clone();
                    ui.snapshot.update(|s| s.worktrees.push(wt));
                    ui.worktree_modal.set(None);
                    ui.navigate("terminals");
                    if launch != "none" {
                        open_shell(ui, &wt_open);
                    }
                    return;
                }

                dispatch_complete(
                    ui,
                    "worktree.create",
                    json!({
                        "project_id": pid,
                        "branch": br,
                        "base_branch": (!base.is_empty()).then_some(base),
                        "sync_base": sync,
                        "launch": launch,
                        "profile_id": (launch == "agent").then_some(prof),
                    }),
                    move |res| match res {
                        Ok(val) => {
                            ui.worktree_modal.set(None);
                            if let Some(tid) = val["terminal"]["id"].as_str() {
                                ui.open_terminal(tid.to_owned());
                            } else {
                                ui.navigate("terminals");
                            }
                        }
                        Err(e) => {
                            let _ = error.try_set(e);
                        }
                    },
                );
            };
            let submit_enter = submit;
            let proj_anchor = element_ref();

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
                                .w(px(440.))
                                .max_w(Percentage(0.95))
                                .border_1()
                                .border_color(BORDER)
                                .gap_3()
                                .child(
                                    view()
                                        .flex_row()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            label("New Managed Branch Worktree")
                                                .font_weight(FontWeight::SEMIBOLD),
                                        )
                                        .child(chrome_button(
                                            Icon::Close,
                                            "Close worktree dialog",
                                            move |_| close_modal(),
                                        )),
                                )
                                .child(muted(
                                    "Create an isolated Git branch worktree managed by Canban without cluttering the Board.",
                                ))
                                .child(
                                    view()
                                        .gap_1p5()
                                        .child(label("Project"))
                                        .child(
                                            canban_ui_kit::press_target()
                                                .element_ref(&proj_anchor)
                                                .h(px(32.))
                                                .px_2p5()
                                                .rounded(px(RADIUS))
                                                .border_1()
                                                .border_color(BORDER)
                                                .bg(INSET)
                                                .hover(|s| s.bg(CARD_HOVER))
                                                .flex_row()
                                                .items_center()
                                                .justify_between()
                                                .child(label(move || {
                                                    let pid = project_id.get();
                                                    ui.snapshot
                                                        .with(|s| {
                                                            s.projects
                                                                .iter()
                                                                .find(|p| p.id == pid)
                                                                .map(|p| p.name.clone())
                                                        })
                                                        .unwrap_or_else(|| "Select Project".into())
                                                }))
                                                .child(icon(Icon::ChevronDown).w(px(13.)).h(px(13.)))
                                                .on_press(move |_: &()| {
                                                    let projects = ui
                                                        .snapshot
                                                        .with_untracked(|s| s.projects.clone());
                                                    if projects.is_empty() {
                                                        return;
                                                    }
                                                    ui.spawn(async move {
                                                        let items: Vec<_> = projects
                                                            .into_iter()
                                                            .map(|p| {
                                                                let id = p.id.clone();
                                                                MenuItem::new(p.name).on_select(
                                                                    move || {
                                                                        let _ = project_id
                                                                            .try_set(id.clone());
                                                                    },
                                                                )
                                                            })
                                                            .collect();
                                                        if let Some(b) = proj_anchor.measure().await
                                                        {
                                                            popup_menu_at(
                                                                items,
                                                                point(
                                                                    b.page.x,
                                                                    b.page.y + b.frame.size.height,
                                                                ),
                                                            )
                                                            .await;
                                                        } else {
                                                            popup_menu(items).await;
                                                        }
                                                    });
                                                }),
                                        ),
                                )
                                .child(
                                    view()
                                        .gap_1p5()
                                        .child(label("Branch name"))
                                        .child(
                                            field(branch, "feat/my-spike")
                                                .label("Branch name")
                                                .auto_focus(true)
                                                .on_submit(move |_: &str| submit_enter()),
                                        ),
                                )
                                .child(
                                    view()
                                        .gap_1p5()
                                        .child(label("Base branch (optional)"))
                                        .child(
                                            field(base_branch, "Defaults to repository target branch")
                                                .label("Base branch"),
                                        ),
                                )
                                .child(checkbox(
                                    "Sync base branch from upstream before branching".into(),
                                    sync_base,
                                    move |v| sync_base.set(*v),
                                ))
                                .child(
                                    view()
                                        .gap_1p5()
                                        .child(label("Open with"))
                                        .child(
                                            segmented()
                                                .child(compact_segment(
                                                    "Shell",
                                                    Some(Icon::Terminal),
                                                    move || launch_mode.get() == "shell",
                                                    move |_| launch_mode.set("shell".into()),
                                                ))
                                                .child(compact_segment(
                                                    "Agent CLI",
                                                    Some(Icon::Agent),
                                                    move || launch_mode.get() == "agent",
                                                    move |_| launch_mode.set("agent".into()),
                                                ))
                                                .child(compact_segment(
                                                    "Create only",
                                                    None,
                                                    move || launch_mode.get() == "none",
                                                    move |_| launch_mode.set("none".into()),
                                                )),
                                        )
                                        .child(when(
                                            move || launch_mode.get() == "agent",
                                            move || {
                                                segmented().children(
                                                    ui.profiles
                                                        .get()
                                                        .into_iter()
                                                        .map(|p| {
                                                            let id = p.id.clone();
                                                            let check = id.clone();
                                                            compact_segment(
                                                                format!("@{}", p.id),
                                                                None,
                                                                move || profile_id.get() == check,
                                                                move |_| profile_id.set(id.clone()),
                                                            )
                                                            .into_element()
                                                        })
                                                        .collect::<Vec<_>>(),
                                                )
                                            },
                                        )),
                                )
                                .child(when(
                                    move || !error.get().is_empty(),
                                    move || notice(move || error.get(), Tone::Attention),
                                ))
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
                                                "Create Worktree",
                                                Variant::Primary,
                                                move |_| submit(),
                                            )
                                            .disabled(move || ui.is_method_pending("worktree.create")),
                                        ),
                                ),
                        ),
                )
        },
    )
}

fn promote_modal(ui: Ui) -> impl IntoElement {
    when(
        move || ui.worktree_promote.get().is_some(),
        move || {
            let wt_id = ui.worktree_promote.get_untracked().unwrap_or_default();
            let wt = ui
                .snapshot
                .with_untracked(|s| s.worktrees.iter().find(|w| w.id == wt_id).cloned());
            let initial_title = wt
                .as_ref()
                .map(|w| w.branch.replace(['/', '-', '_'], " "))
                .unwrap_or_default();
            let branch_label = wt.as_ref().map(|w| w.branch.clone()).unwrap_or_default();
            let title = signal(initial_title);
            let body = signal(String::new());
            let error = signal(String::new());

            let close_modal = move || {
                ui.worktree_promote.set(None);
                ui.terminal_focus.update(|n| *n += 1);
            };

            let submit = move || {
                let t = title.get_untracked().trim().to_owned();
                if t.is_empty() {
                    error.set("Card title is required".into());
                    return;
                }
                let b = body.get_untracked();
                if crate::APP.get().is_none() {
                    ui.worktree_promote.set(None);
                    ui.notify("Promoted worktree to Card");
                    return;
                }
                let wid = wt_id.clone();
                dispatch_complete(
                    ui,
                    "worktree.promote",
                    json!({"worktree_id": wid, "title": t, "body": b}),
                    move |res| match res {
                        Ok(val) => {
                            ui.worktree_promote.set(None);
                            if let Some(cid) = val["id"].as_str() {
                                ui.open_card(cid.to_owned());
                            }
                        }
                        Err(e) => {
                            let _ = error.try_set(e);
                        }
                    },
                );
            };
            let submit_enter = submit.clone();

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
                                .w(px(440.))
                                .max_w(Percentage(0.95))
                                .border_1()
                                .border_color(BORDER)
                                .gap_3()
                                .child(
                                    view()
                                        .flex_row()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            label("Promote Worktree to Board Card")
                                                .font_weight(FontWeight::SEMIBOLD),
                                        )
                                        .child(chrome_button(
                                            Icon::Close,
                                            "Close promote dialog",
                                            move |_| close_modal(),
                                        )),
                                )
                                .child(muted(format!(
                                    "Transfer branch `{branch_label}` and all uncommitted files directly to a new Board Card."
                                )))
                                .child(
                                    field(title, "Card title")
                                        .label("Card title")
                                        .auto_focus(true)
                                        .on_submit(move |_: &str| submit_enter()),
                                )
                                .child(
                                    field(body, "Optional task context…").label("Description"),
                                )
                                .child(when(
                                    move || !error.get().is_empty(),
                                    move || notice(move || error.get(), Tone::Attention),
                                ))
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
                                                "Create Card",
                                                Variant::Primary,
                                                move |_| submit(),
                                            )
                                            .disabled(move || {
                                                ui.is_method_pending("worktree.promote")
                                            }),
                                        ),
                                ),
                        ),
                )
        },
    )
}

fn remove_confirm_modal(ui: Ui) -> impl IntoElement {
    when(
        move || ui.worktree_remove_confirm.get().is_some(),
        move || {
            let wt_id = ui
                .worktree_remove_confirm
                .get_untracked()
                .unwrap_or_default();
            let branch = ui
                .snapshot
                .with_untracked(|s| {
                    s.worktrees
                        .iter()
                        .find(|w| w.id == wt_id)
                        .map(|w| w.branch.clone())
                })
                .unwrap_or_else(|| "worktree".into());
            let close_modal = move || {
                ui.worktree_remove_confirm.set(None);
                ui.terminal_focus.update(|n| *n += 1);
            };
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
                                .w(px(400.))
                                .max_w(Percentage(0.95))
                                .border_1()
                                .border_color(BORDER)
                                .gap_3()
                                .child(
                                    label("Delete Dirty Worktree?")
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(notice(
                                    format!(
                                        "Worktree `{branch}` has uncommitted changes. Deleting it permanently discards uncommitted files on disk."
                                    ),
                                    Tone::Danger,
                                ))
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
                                                "Force Delete",
                                                Variant::Primary,
                                                move |_| request_remove(ui, wt_id.clone(), true),
                                            )
                                            .disabled(move || {
                                                ui.is_method_pending("worktree.remove")
                                            }),
                                        ),
                                ),
                        ),
                )
        },
    )
}
