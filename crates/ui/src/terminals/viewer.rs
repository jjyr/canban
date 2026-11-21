//! Selected conversation controls, retained context and one native attachment.
use super::*;
use serde_json::json;
fn context(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let Some(row) = selected(ui) else {
            return view();
        };
        let mut body = view().gap_3();
        for reason in &row.reasons {
            body = body.child(notice(reason.message.clone(), Tone::Attention));
        }
        if row.card.archived_at.is_some() {
            body = body.child(notice(
                "Archived Card · restore it before starting or resuming an Agent.",
                Tone::Info,
            ));
        }
        let att = row.agent().unwrap();
        let id = row.id.clone();
        let host_id = ui.snapshot.with(|s| s.card_host(&row.card.id).to_owned());
        body = body
            .child(label("Assignment"))
            .child(read_text(att.instruction.clone()))
            .child(label(format!(
                "Run {} · {} · {}",
                att.number,
                att.state,
                att.terminal.as_deref().unwrap_or("No terminal")
            )))
            .child(muted(format!(
                "{} · {}",
                crate::hosts::host_name(ui, &host_id),
                att.cwd
            )))
            .child(action("Read saved output", Variant::Quiet, move |_| {
                load_text(ui, "execution.output", json!({"terminal_id":id}))
            }));
        if let Some((state, message)) = ui.snapshot.with(|snapshot| {
            snapshot
                .reports
                .iter()
                .rev()
                .find(|r| r.terminal_id.as_deref() == row.outcome_terminal_id())
                .map(|report| (report.state.clone(), report.message.clone()))
        }) {
            body = body
                .child(label("Saved outcome"))
                .child(read_text(format!("{state} · {message}")));
        }
        let output = ui.output.get();
        if !output.is_empty() {
            body = body.child(read_text(output));
        }
        body
    })
}
pub fn embedded(ui: Ui, selected_id: Memo<String>) -> impl IntoElement {
    let in_terminals = computed(move || matches!(ui.page.get().as_str(), "terminals" | "terminal"));
    let target = computed(move || {
        // Keep the retained Terminals shell free of an active native PTY viewer when off-page
        // so the Card split can own the sole active viewer without double-attaching.
        if !in_terminals.get() {
            return None;
        }
        let id = selected_id.get();
        if id.is_empty() || ui.closing_terminals.with(|c| c.contains(&id)) {
            return None;
        }
        ui.terminal_entries
            .with(|rows| rows.iter().find(|e| e.saved.id == id).cloned())
            .and_then(|e| {
                let b = e.binding()?;
                Some((
                    b["id"].as_str().unwrap_or("").to_owned(),
                    b["core_id"].as_str().unwrap_or("").to_owned(),
                    b["terminal"].as_str().unwrap_or("").to_owned(),
                    b["read_only"] == true,
                    b["input_status"].as_str().unwrap_or("").to_owned(),
                ))
            })
    });
    let binding_key = computed(move || {
        target.with(|t| {
            t.as_ref().map(|(entry_id, core_id, terminal, _, _)| {
                (entry_id.clone(), core_id.clone(), terminal.clone())
            })
        })
    });
    let has_live_target = computed(move || binding_key.with(Option::is_some));
    dynamic(move || {
        if !in_terminals.get() {
            return view()
                .flex_1()
                .min_h(px(0.))
                .bg(crate::utilities::terminal_bg_color)
                .into_element();
        }
        let placeholder = move || {
            let sel = selected_id.get();
            let has_entry = !sel.is_empty()
                && !ui.closing_terminals.with(|c| c.contains(&sel))
                && ui
                    .terminal_entries
                    .with(|rows| rows.iter().any(|e| e.saved.id == sel));
            view()
                .absolute()
                .inset_0()
                .bg(crate::utilities::terminal_bg_color)
                .p_5()
                .gap_3()
                .child(if has_entry {
                    view().gap_2().child(muted(move || {
                        let sel = selected_id.get();
                        ui.snapshot.with(|s| {
                            s.configuration_sessions
                                .iter()
                                .find(|c| c.id == sel)
                                .map(|c| c.error.clone().unwrap_or_else(|| format!("{}…", c.state)))
                                .or_else(|| {
                                    s.merge_batches.iter().find(|b| b.id == sel).map(|b| {
                                        b.error.clone().unwrap_or_else(|| format!("{}…", b.state))
                                    })
                                })
                                .or_else(|| {
                                    s.scheduled_runs.iter().find(|r| r.id == sel).map(|r| {
                                        r.error.clone().unwrap_or_else(|| format!("{}…", r.state))
                                    })
                                })
                                .unwrap_or_else(|| "Connecting…".into())
                        })
                    }))
                } else {
                    view().items_start().gap_3().child(
                        action("New terminal", Variant::Secondary, move |_| new_tab(ui))
                            .disabled(move || ui.pending.get()),
                    )
                })
        };
        #[cfg(target_os = "macos")]
        {
            let native_view = if crate::APP.get().is_none() {
                native("canban/terminal")
                    .prop("theme", crate::utilities::terminal_theme)
                    .prop("font", move || crate::utilities::terminal_font(ui))
                    .prop(
                        "ui_command_keys",
                        json!(["o", "n", "/", "i", "j", "t", "w", "[", "]", "{", "}"]),
                    )
                    .prop("focus", move || {
                        json!({
                            "ticket": ui.terminal_focus.get(),
                            "enabled": has_live_target.get()
                                && !ui.quick_open.get()
                                && ui.utility_popup.get().is_empty()
                                && ui.terminal_rename.get().is_none()
                                && ui.terminal_close_confirm.get().is_none()
                                && ui.worktree_modal.get().is_none()
                                && ui.worktree_promote.get().is_none()
                                && ui.worktree_remove_confirm.get().is_none()
                                && ui.project_delete_confirm.get().is_none()
                        })
                    })
                    .prop(
                        "content",
                        json!(concat!(
                            "\x1b[1;36mCanban native Terminal\x1b[0m · Gallery preview (no Core connection)\r\n",
                            "Styles: \x1b[1mBold\x1b[0m · \x1b[3mItalic\x1b[0m · \x1b[4:3;58;2;255;105;97mCurly underline\x1b[0m · 世界 · ░▒▓█\r\n",
                            "Links : https://canban.dev · \x1b]8;;https://ghostty.org\x07libghostty-vt\x1b]8;;\x07\r\n",
                        )),
                    )
                    .prop("read_only", json!(true))
                    .w_full()
                    .flex_1()
                    .min_h(px(0.))
                    .style_when(move || !has_live_target.get(), |s| s.hidden())
            } else {
                native("canban/terminal")
                    .prop("theme", crate::utilities::terminal_theme)
                    .prop("font", move || crate::utilities::terminal_font(ui))
                    .prop(
                        "ui_command_keys",
                        json!(["o", "n", "/", "i", "j", "t", "w", "[", "]", "{", "}"]),
                    )
                    .prop("focus", move || {
                        json!({
                            "ticket": ui.terminal_focus.get(),
                            "enabled": has_live_target.get()
                                && !ui.quick_open.get()
                                && ui.utility_popup.get().is_empty()
                                && ui.terminal_rename.get().is_none()
                                && ui.terminal_close_confirm.get().is_none()
                                && ui.worktree_modal.get().is_none()
                                && ui.worktree_promote.get().is_none()
                                && ui.worktree_remove_confirm.get().is_none()
                                && ui.project_delete_confirm.get().is_none()
                        })
                    })
                    .prop("binding", move || {
                        binding_key.with(|t| match t {
                            Some((entry_id, core_id, terminal)) => {
                                let host_id = ui
                                    .snapshot
                                    .with_untracked(|s| s.terminal_host(entry_id).to_owned());
                                json!({
                                    "config": crate::config_for_host(&host_id),
                                    "terminal": terminal,
                                    "core_id": core_id,
                                })
                            }
                            None => serde_json::Value::Null,
                        })
                    })
                    .prop("read_only", move || {
                        target.with(|t| json!(t.as_ref().is_none_or(|(_, _, _, ro, _)| *ro)))
                    })
                    .prop("input_status", move || {
                        target.with(|t| {
                            json!(
                                t.as_ref()
                                    .map(|(_, _, _, _, status)| status.as_str())
                                    .unwrap_or("")
                            )
                        })
                    })
                    .w_full()
                    .flex_1()
                    .min_h(px(0.))
                    .style_when(move || !has_live_target.get(), |s| s.hidden())
            };
            view()
                .relative()
                .flex_1()
                .min_h(px(0.))
                .bg(crate::utilities::terminal_bg_color)
                .child(native_view)
                .child(when(move || !has_live_target.get(), placeholder))
                .into_element()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let focus_req = move || canban_terminal::linux::FocusRequest {
                ticket: ui.terminal_focus.get(),
                enabled: !ui.quick_open.get()
                    && ui.utility_popup.get().is_empty()
                    && !ui.native_modal.get()
                    && ui.terminal_rename.get().is_none()
                    && ui.terminal_close_confirm.get().is_none()
                    && ui.worktree_modal.get().is_none()
                    && ui.worktree_promote.get().is_none()
                    && ui.worktree_remove_confirm.get().is_none()
                    && ui.project_delete_confirm.get().is_none(),
            };
            if crate::APP.get().is_none() {
                return canban_terminal::linux::preview_terminal(
                    concat!(
                        "\x1b[1;36mCanban native Terminal\x1b[0m · Gallery preview (no Core connection)\r\n",
                        "Styles: \x1b[1mBold\x1b[0m · \x1b[3mItalic\x1b[0m · \x1b[4:3;58;2;255;105;97mCurly underline\x1b[0m · 世界 · ░▒▓█\r\n",
                        "Links : https://canban.dev · \x1b]8;;https://ghostty.org\x07libghostty-vt\x1b]8;;\x07\r\n",
                    ),
                    crate::utilities::terminal_theme,
                    move || crate::utilities::terminal_font(ui),
                    focus_req,
                )
                .w_full()
                .flex_1()
                .min_h(px(0.))
                .into_element();
            }
            canban_terminal::linux::live_terminal(
                move || {
                    target.with(|t| {
                        t.as_ref().map(|(entry_id, core_id, terminal, _, _)| {
                            let host_id = ui
                                .snapshot
                                .with_untracked(|s| s.terminal_host(entry_id).to_owned());
                            canban_terminal::linux::LiveBinding {
                                config: crate::config_for_host(&host_id),
                                core_id: core_id.clone(),
                                terminal: terminal.clone(),
                            }
                        })
                    })
                },
                crate::utilities::terminal_theme,
                move || crate::utilities::terminal_font(ui),
                focus_req,
                move || target.with(|t| t.as_ref().is_none_or(|(_, _, _, ro, _)| *ro)),
                move || {
                    target.with(|t| {
                        t.as_ref()
                            .map(|(_, _, _, _, status)| status.clone())
                            .unwrap_or_default()
                    })
                },
            )
            .w_full()
            .flex_1()
            .min_h(px(0.))
            .into_element()
        }
    })
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let selected_id = computed(move || {
        if ui.narrow.get() && ui.terminal_sidebar.get() {
            String::new()
        } else {
            ui.terminal_selection.get()
        }
    });
    let has_target = computed(move || {
        let id = selected_id.get();
        !id.is_empty()
            && !ui.closing_terminals.with(|c| c.contains(&id))
            && ui.terminal_entries.with(|rows| {
                rows.iter()
                    .any(|e| e.saved.id == id && e.binding().is_some())
            })
    });
    view()
        .w_full()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .bg(crate::utilities::terminal_bg_color)
        .gap_0()
        .child(when(
            move || {
                let sel = ui.terminal_selection.get();
                ui.snapshot
                    .with(|s| s.configuration_sessions.iter().any(|c| c.id == sel))
            },
            move || {
                action("← Back to Workflows", Variant::Quiet, move |_| {
                    let sel = ui.terminal_selection.get_untracked();
                    if let Some(project) = ui.snapshot.with_untracked(|s| {
                        s.configuration_sessions
                            .iter()
                            .find(|c| c.id == sel)
                            .map(|c| c.project_id.clone())
                    }) {
                        ui.project.set(project);
                        ui.navigate("workflows");
                    }
                })
            },
        ))
        .child(when(
            move || {
                let sel = ui.terminal_selection.get();
                ui.snapshot
                    .with(|s| s.scheduled_runs.iter().any(|r| r.id == sel))
            },
            move || {
                action("← Back to Schedules", Variant::Quiet, move |_| {
                    let sel = ui.terminal_selection.get_untracked();
                    if let Some((project, sid)) = ui.snapshot.with_untracked(|s| {
                        s.scheduled_runs
                            .iter()
                            .find(|r| r.id == sel)
                            .map(|r| (r.project_id.clone(), r.schedule_id.clone()))
                    }) {
                        crate::schedules::open(ui, Some(project), Some(sid));
                    }
                })
            },
        ))
        .child(embedded(ui, selected_id))
        .child(when(
            move || ui.terminal_details.get() && has_target.get(),
            move || {
                scroll()
                    .h(px(220.))
                    .border_t_1()
                    .border_color(BORDER)
                    .child(
                        view()
                            .p_4()
                            .gap_2()
                            .child(action("Hide details", Variant::Quiet, move |_| {
                                ui.terminal_details.set(false);
                                ui.terminal_focus.update(|v| *v += 1);
                            }))
                            .child(context(ui)),
                    )
            },
        ))
}
