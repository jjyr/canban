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
                return view().flex_1().min_h(px(0.)).p_5().gap_3().child(if entry.is_some() {
                    view().gap_2().child(muted("Connecting…"))
                } else {
                    view().items_start().gap_3().child(when(move || card_context(ui).is_some(), move || {
                        let c=card_context(ui).unwrap();
                        view().gap_2().child(label(format!("#{} {}",c.number,c.title)))
                            .child(action("Choose Agent",Variant::Primary,move |_|choose_card_agent(ui)).disabled(move ||ui.pending.get()))
                    })).child(action("New terminal",Variant::Secondary,move |_|new_tab(ui)).disabled(move||ui.pending.get()))
                }).into_element();
            };
            let Some(app)=crate::APP.get() else {
                return native("canban/terminal").prop("theme",crate::utilities::terminal_theme).prop("ui_command_keys",json!(["o","n","/","t","w","[","]","{","}"])).prop("focus",move||json!({"ticket":ui.terminal_focus.get(),"enabled":!ui.quick_open.get() && ui.utility_popup.get().is_empty() && ui.terminal_rename.get().is_none()})).key(run["id"].as_str().unwrap_or("").to_owned()).prop("content",json!("Canban native Terminal\r\nGallery preview · no Core connection\r\n")).prop("read_only",json!(true)).flex_1().min_h(px(0.)).into_element();
            };
            native("canban/terminal").prop("theme",crate::utilities::terminal_theme).prop("ui_command_keys",json!(["o","n","/","t","w","[","]","{","}"])).prop("focus",move||json!({"ticket":ui.terminal_focus.get(),"enabled":!ui.quick_open.get() && ui.utility_popup.get().is_empty() && ui.terminal_rename.get().is_none()})).key(run["id"].as_str().unwrap_or("").to_owned()).prop("binding",json!({"config":app.config(),"terminal":run["terminal"],"core_id":run["core_id"]})).prop("read_only",run["read_only"].clone()).prop("input_status",run["input_status"].clone()).flex_1().min_h(px(0.)).into_element()
        }))
        .child(when(move||ui.terminal_details.get() && binding.get().is_some(),move||scroll().h(px(220.)).border_t_1().border_color(BORDER).child(view().p_4().gap_2().child(action("Hide details",Variant::Quiet,move |_|ui.terminal_details.set(false))).child(context(ui)))))
}
