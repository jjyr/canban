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
        let snapshot = ui.snapshot.get();
        if let Some(report) = snapshot
            .reports
            .iter()
            .rev()
            .find(|r| r.terminal_id.as_deref() == row.outcome_terminal_id())
        {
            body = body
                .child(label("Saved outcome"))
                .child(read_text(format!("{} · {}", report.state, report.message)));
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
        if id.is_empty() {
            return None;
        }
        entries(&ui.snapshot.get())
            .into_iter()
            .find(|e| e.saved.id == id)
            .and_then(|e| {
                let b = e.binding()?;
                Some((
                    b["id"].as_str().unwrap_or("").to_owned(),
                    b["core_id"].as_str().unwrap_or("").to_owned(),
                    b["terminal"].as_str().unwrap_or("").to_owned(),
                ))
            })
    });
    let read_only = computed(move || {
        let id = selected_id.get();
        entries(&ui.snapshot.get())
            .into_iter()
            .find(|e| e.saved.id == id)
            .and_then(|e| e.binding())
            .is_none_or(|b| b["read_only"] == true)
    });
    let input_status = computed(move || {
        let id = selected_id.get();
        entries(&ui.snapshot.get())
            .into_iter()
            .find(|e| e.saved.id == id)
            .and_then(|e| e.binding())
            .and_then(|b| b["input_status"].as_str().map(str::to_owned))
            .unwrap_or_default()
    });
    dynamic(move || {
        let Some((id, core_id, terminal)) = target.get() else {
            let sel = selected_id.get();
            let entry = entries(&ui.snapshot.get())
                .into_iter()
                .find(|e| e.saved.id == sel);
            return view()
                .flex_1()
                .min_h(px(0.))
                .p_5()
                .gap_3()
                .child(if entry.is_some() {
                    view().gap_2().child(muted(move || {
                        let sel = selected_id.get();
                        ui.snapshot.with(|s| {
                            s.configuration_sessions
                                .iter()
                                .find(|c| c.id == sel)
                                .map(|c| c.error.clone().unwrap_or_else(|| format!("{}…", c.state)))
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
        };
        let Some(app) = crate::APP.get() else {
            return native("canban/terminal")
                .prop("theme", crate::utilities::terminal_theme)
                .prop("font", move || crate::utilities::terminal_font(ui))
                .prop("ui_command_keys", json!(["o","n","/","i","j","t","w","[","]","{","}"]))
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
                .key(id)
                .prop("content", json!(concat!(
                    "\x1b[1;36mCanban native Terminal\x1b[0m · Gallery preview (no Core connection)\r\n",
                    "Styles: \x1b[1mBold\x1b[0m · \x1b[3mItalic\x1b[0m · \x1b[4:3;58;2;255;105;97mCurly underline\x1b[0m · 世界 · ░▒▓█\r\n",
                    "Links : https://canban.dev · \x1b]8;;https://ghostty.org\x07libghostty-vt\x1b]8;;\x07\r\n",
                )))
                .prop("read_only", json!(true))
                .flex_1()
                .min_h(px(0.))
                .into_element();
        };
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
            .key(id)
            .prop(
                "binding",
                json!({"config":app.config(),"terminal":terminal,"core_id":core_id}),
            )
            .prop("read_only", move || json!(read_only.get()))
            .prop("input_status", move || json!(input_status.get()))
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
            && entries(&ui.snapshot.get())
                .into_iter()
                .any(|e| e.saved.id == id && e.binding().is_some())
    });
    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .gap_0()
        .child(when(
            move || {
                ui.snapshot.with(|s| {
                    s.configuration_sessions
                        .iter()
                        .any(|c| c.id == ui.terminal_selection.get())
                })
            },
            move || {
                action("← Back to Workflows", Variant::Quiet, move |_| {
                    if let Some(project) = ui.snapshot.with_untracked(|s| {
                        s.configuration_sessions
                            .iter()
                            .find(|c| c.id == ui.terminal_selection.get_untracked())
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
