use super::*;
pub fn timeline_item(ui: Ui, event: Event) -> impl IntoElement {
    let narrative = matches!(
        event.kind.as_str(),
        "comment.created" | "comment.replied" | "execution.reported"
    );
    let body = signal(event.message.clone());
    let error = signal(String::new());
    let expanded = signal(false);
    let card = event.card_id.clone();
    if narrative
        && let Some(path) = event.data["path"].as_str().map(str::to_owned)
        && let Some(app) = crate::APP.get().cloned()
    {
        let card = card.clone();
        ui.spawn(async move {
            match blocking(move || app.call("resource.read", json!({"card_id":card,"path":path})))
                .await
            {
                Ok(Ok(value)) => {
                    if let Some(text) = value["body"].as_str().or(value["text"].as_str()) {
                        body.try_set(text.into());
                    }
                }
                Ok(Err(e)) => {
                    error.try_set(format!("Report unavailable: {e:#}"));
                }
                Err(e) => {
                    error.try_set(e.to_string());
                }
            }
        });
    }
    let actor = event
        .actor
        .strip_prefix("agent:")
        .map(|a| format!("@{a}"))
        .unwrap_or(event.actor.clone());
    let mut row = view()
        .gap_2()
        .py_2()
        .border_b_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .justify_between()
                .gap_2()
                .child(label(actor).font_weight(FontWeight::MEDIUM))
                .child(muted(crate::components::activity::age(event.created_at))),
        );
    if narrative {
        row = row
            .child(
                nsh_base::text_view()
                    .markdown(move || {
                        let value = body.get();
                        Some(if expanded.get() {
                            value.into()
                        } else {
                            value.chars().take(12000).collect::<String>().into()
                        })
                    })
                    .on_link_press(move |e: &nsh_base::TextViewLinkPressEvent| {
                        if let Some(id) = e.url.strip_prefix("note:") {
                            ui.open_note(id.into());
                        } else if let Some(id) = e.url.strip_prefix("attachment:")
                            && let Some(a) = ui.snapshot.with_untracked(|s| {
                                s.artifacts.iter().find(|a| a.id == id).cloned()
                            })
                        {
                            open_resource(ui, a.card_id, a.path);
                        }
                    }),
            )
            .child(when(
                move || body.with(|s| s.chars().count() > 12000),
                move || {
                    action(
                        move || {
                            if expanded.get() {
                                "Show less"
                            } else {
                                "Read full report"
                            }
                        },
                        Variant::Quiet,
                        move |_| expanded.update(|v| *v = !*v),
                    )
                },
            ));
    } else {
        row = row.child(read_text(event.message));
    }
    row = row.child(when(
        move || !error.get().is_empty(),
        move || notice(error.get(), Tone::Attention),
    ));
    if let Some(id) = event.data["note_id"].as_str() {
        let id = id.to_owned();
        row = row.child(action("Open Note", Variant::Quiet, move |_| {
            ui.open_note(id.clone())
        }));
    }
    if let Some(request) = event.request_id {
        row = row.child(action("Open terminal", Variant::Quiet, move |_| {
            crate::terminals::open_request(ui, &request)
        }));
    }
    row
}
