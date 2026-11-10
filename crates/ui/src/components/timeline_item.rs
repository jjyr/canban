use super::*;
pub fn timeline_item(ui: Ui, event: Event) -> impl IntoElement {
    let narrative = matches!(
        event.kind.as_str(),
        "comment.created" | "comment.replied" | "execution.reported"
    );
    render(ui, event, narrative)
}
pub fn render(ui: Ui, event: Event, narrative: bool) -> impl IntoElement {
    if !narrative {
        let mut row = view()
            .flex_row()
            .items_center()
            .gap_2()
            .py_1()
            .child(muted("·"))
            .child(muted(event.message).flex_1().min_w(px(0.)))
            .child(muted(crate::components::activity::age(event.created_at)));
        if let Some(request) = event.request_id {
            row = row.child(action("Agent ↗", Variant::Quiet, move |_| {
                crate::terminals::open_request(ui, &request)
            }));
        }
        return row.into_element();
    }
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
    let actor = if let Some(request) = event.request_id.clone() {
        action(format!("{actor} ↗"), Variant::Quiet, move |_| {
            crate::terminals::open_request(ui, &request)
        })
        .into_element()
    } else {
        label(actor).font_weight(FontWeight::MEDIUM).into_element()
    };
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
                .child(actor)
                .child(muted(crate::components::activity::age(event.created_at))),
        );
    if narrative {
        row = row
            .child(crate::links::reader(ui, move || {
                let value = body.get();
                if expanded.get() {
                    value
                } else {
                    value.chars().take(12000).collect::<String>()
                }
            }))
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
    row.into_element()
}
