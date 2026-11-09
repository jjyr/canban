//! Selected conversation controls, retained context and one native attachment.
use super::*;
use serde_json::json;
fn resume(ui: Ui, row: canban_notifications::OverviewRow, fresh: bool) {
    let req = row.request.as_ref().unwrap();
    let mut params = json!({"card_id":row.card.id,"profile_id":req.profile.id});
    if fresh {
        params["fresh"] = json!(true);
    } else if let Some(a) = row.attempt {
        params["expected_attempt_id"] = json!(a.id);
    }
    // The selected conversation is stable. Snapshot refresh updates its attempt;
    // a late response must not navigate away from another selected conversation.
    dispatch(ui, "agent.open", params);
}
fn ended_actions(ui: Ui, row: canban_notifications::OverviewRow) -> View {
    let mut actions = view().flex_row().flex_wrap().gap_2();
    if row.card.archived_at.is_none() && model::closed(&row) {
        let can_resume = model::can_resume(&row, &ui.profiles.get());
        let saved = row.clone();
        actions = actions.child(
            action("Resume", Variant::Primary, move |_| {
                resume(ui, saved.clone(), false)
            })
            .disabled(move || !can_resume || ui.pending.get()),
        );
        if ui.profiles.with(|ps| {
            ps.iter()
                .any(|p| p.id == row.request.as_ref().unwrap().profile.id)
        }) {
            actions = actions.child(
                action("Start fresh", Variant::Secondary, move |_| {
                    resume(ui, row.clone(), true)
                })
                .disabled(move || ui.pending.get()),
            );
        }
    }
    actions
}
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
        let req = row.request.as_ref().unwrap();
        body = body
            .child(label("Assignment"))
            .child(read_text(req.instruction.clone()));
        let snapshot = ui.snapshot.get();
        for a in snapshot
            .attempts
            .iter()
            .filter(|a| row.history.contains(&a.id))
        {
            let id = a.id.clone();
            body = body
                .child(label(format!(
                    "Attempt {} · {} · {}",
                    a.number,
                    a.state,
                    a.terminal.as_deref().unwrap_or("No terminal")
                )))
                .child(muted(format!(
                    "{} · {}",
                    crate::machines::name(
                        ui,
                        a.location
                            .as_ref()
                            .map(|l| l.machine.as_str())
                            .unwrap_or("local")
                    ),
                    a.cwd
                )))
                .child(action("Read saved output", Variant::Quiet, move |_| {
                    load_text(ui, "execution.output", json!({"attempt_id":id}))
                }));
        }
        if let Some(report) = snapshot
            .reports
            .iter()
            .rev()
            .find(|r| Some(r.request_id.as_str()) == row.outcome_request_id())
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
pub fn screen(ui: Ui) -> impl IntoElement {
    let binding = computed(move || {
        if ui.narrow.get() && ui.terminal_sidebar.get() {
            return None;
        }
        current(ui).and_then(|e| e.binding())
    });
    view().flex_1().min_h(px(0.)).min_w(px(0.)).gap_0()
        .child(dynamic(move|| {
            let Some(run)=binding.get() else {
                let entry=current(ui);
                return scroll().flex_1().min_h(px(0.)).child(view().flex_1().p_5().gap_3().child(if let Some(e)=entry {
                    let mut content=view().gap_3();
                    if let Some(row)=e.agent {
                        let msg=if row.card.archived_at.is_some(){"Archived Card"}else if model::closed(&row){"Agent session ended"}else if row.attempt.is_none(){"Waiting for the Agent…"}else{"Original terminal unavailable"};
                        content=content.child(label(msg)).child(ended_actions(ui,row)).child(context(ui));
                    } else {
                        let ended=e.ended();
                        content=content.child(label(if ended{"Terminal ended"}else{"Terminal unavailable"}));
                        if let Some(canban_types::TerminalRuntime {error:Some(message),..})=&e.saved.runtime {content=content.child(notice(message.clone(),Tone::Attention));}
                        if ended {
                            content=content.child(action("Reopen terminal",Variant::Primary,move |_|reopen(ui,e.saved.id.clone())));
                        }
                    }
                    content
                } else {
                    view().gap_3().child(action("New terminal",Variant::Primary,move |_|new_tab(ui)).disabled(move||ui.pending.get())).child(muted("⌘T"))
                })).into_element();
            };
            let Some(app)=crate::APP.get() else {
                return native("canban/terminal").prop("ui_command_keys",json!(["o","t","w","[","]","{","}"])).prop("focus",move||json!({"ticket":ui.terminal_focus.get(),"enabled":!ui.quick_open.get() && ui.terminal_rename.get().is_none() && !ui.pending.get()})).key(run["id"].as_str().unwrap_or("").to_owned()).prop("content",json!("Canban native Terminal\r\nGallery preview · no Core connection\r\n")).prop("read_only",json!(true)).flex_1().min_h(px(0.)).into_element();
            };
            native("canban/terminal").prop("ui_command_keys",json!(["o","t","w","[","]","{","}"])).prop("focus",move||json!({"ticket":ui.terminal_focus.get(),"enabled":!ui.quick_open.get() && ui.terminal_rename.get().is_none() && !ui.pending.get()})).key(run["id"].as_str().unwrap_or("").to_owned()).prop("binding",json!({"config":app.config(),"terminal":run["terminal"],"core_id":run["core_id"]})).prop("read_only",run["read_only"].clone()).prop("input_status",run["input_status"].clone()).flex_1().min_h(px(0.)).into_element()
        }))
        .child(when(move||ui.terminal_details.get() && binding.get().is_some(),move||scroll().h(px(220.)).border_t_1().border_color(BORDER).child(view().p_4().gap_2().child(action("Hide details",Variant::Quiet,move |_|ui.terminal_details.set(false))).child(context(ui)))))
}
