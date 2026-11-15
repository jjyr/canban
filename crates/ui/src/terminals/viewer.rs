//! Selected conversation controls, retained context and one native attachment.
use super::*;
use serde_json::{Value, json};
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
                crate::machines::name(
                    ui,
                    att.location
                        .as_ref()
                        .map(|l| l.machine.as_str())
                        .unwrap_or("local")
                ),
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
    let target = computed(move || {
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
    let has_live_target = computed(move || target.with(|t| t.is_some()));
    dynamic(move || {
        if !has_live_target.get() {
            let sel = selected_id.get();
            let has_entry = !sel.is_empty()
                && !ui.closing_terminals.with(|c| c.contains(&sel))
                && ui
                    .terminal_entries
                    .with(|rows| rows.iter().any(|e| e.saved.id == sel));
            return view()
                .flex_1()
                .min_h(px(0.))
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
                                .unwrap_or_else(|| "Connecting…".into())
                        })
                    }))
                } else {
                    view().items_start().gap_3().child(
                        action("New terminal", Variant::Secondary, move |_| new_tab(ui))
                            .disabled(move || ui.pending.get()),
                    )
                })
                .into_element();
        }
        if crate::APP.get().is_none() {
            return native("canban/terminal")
                .prop("theme", crate::utilities::terminal_theme)
                .prop("font", move || crate::utilities::terminal_font(ui))
                .prop(
                    "ui_command_keys",
                    json!(["o", "n", "/", "i", "j", "t", "w", "[", "]", "{", "}"]),
                )
                .prop("focus", move || {
                    json!({
                        "ticket": ui.terminal_focus.get(),
                        "enabled": !ui.quick_open.get()
                            && ui.utility_popup.get().is_empty()
                            && ui.terminal_rename.get().is_none()
                            && ui.terminal_close_confirm.get().is_none()
                            && ui.worktree_modal.get().is_none()
                            && ui.worktree_promote.get().is_none()
                            && ui.worktree_remove_confirm.get().is_none()
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
                .into_element();
        }
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
                    "enabled": !ui.quick_open.get()
                        && ui.utility_popup.get().is_empty()
                        && ui.terminal_rename.get().is_none()
                        && ui.terminal_close_confirm.get().is_none()
                        && ui.worktree_modal.get().is_none()
                        && ui.worktree_promote.get().is_none()
                        && ui.worktree_remove_confirm.get().is_none()
                })
            })
            .prop("binding", move || {
                target.with(|t| match t {
                    Some((entry_id, core_id, terminal, _, _)) => {
                        let host_id = ui
                            .snapshot
                            .with_untracked(|s| s.terminal_host(entry_id).to_owned());
                        json!({
                            "config": crate::config_for_host(&host_id),
                            "terminal": terminal,
                            "core_id": core_id,
                        })
                    }
                    None => Value::Null,
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
            .into_element()
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
