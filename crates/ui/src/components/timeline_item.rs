use super::*;
pub fn timeline_item(ui: Ui, event: Event) -> impl IntoElement {
    let is_comment = event.kind == "comment.created";
    let req = event.request_id.clone();
    let mut row = view()
        .gap_1()
        .py_3()
        .border_b_1()
        .border_color(BORDER)
        .child(activity_header(
            event.actor.clone(),
            &event.kind,
            event.created_at,
        ))
        .child(if is_comment {
            nsh_base::text_view()
                .markdown(event.message.clone())
                .into_element()
        } else {
            label(event.message).into_element()
        });
    if let Some(id) = event.data["note_id"].as_str() {
        let id = id.to_owned();
        row = row.child(action("Open Note", Variant::Quiet, move |_| {
            ui.open_note(id.clone())
        }));
    }
    if let Some(path) = event.data["path"].as_str() {
        let path = path.to_owned();
        let card = event.card_id.clone();
        row = row.child(action("Read report", Variant::Quiet, move |_| {
            load_text(ui, "resource.read", json!({"card_id":card,"path":path}))
        }));
    }
    if let Some(request) = req {
        let attempts = ui.snapshot.with_untracked(|s| {
            s.attempts
                .iter()
                .filter(|a| a.request_id == request)
                .cloned()
                .collect::<Vec<_>>()
        });
        if let Some(attempt) = attempts.last() {
            let id = attempt.id.clone();
            row = row.child(action("View output", Variant::Quiet, move |_| {
                load_text(ui, "execution.output", json!({"attempt_id":id}))
            }));
        }
    }
    row
}
