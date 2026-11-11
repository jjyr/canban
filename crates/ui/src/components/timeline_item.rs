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
        if let Some(request) = event.request_id
            && !matches!(
                event.kind.as_str(),
                "execution.queued"
                    | "execution.started"
                    | "execution.attached"
                    | "execution.reported"
            )
        {
            row = row.child(compact_action("Terminal ↗", Variant::Quiet, move |_| {
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
    let initials = actor
        .trim_start_matches('@')
        .chars()
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let mut heading = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .px_3()
        .py_1()
        .bg(INSET)
        .child(label(actor).font_weight(FontWeight::MEDIUM))
        .child(muted(crate::components::activity::age(event.created_at)))
        .child(view().flex_1());
    if let Some(request) = event.request_id.clone() {
        let details = request.clone();
        heading = heading
            .child(compact_action("Details", Variant::Quiet, move |_| {
                ui.request.set(details.clone())
            }))
            .child(compact_action("Terminal ↗", Variant::Quiet, move |_| {
                crate::terminals::open_request(ui, &request)
            }));
    }
    let mut row = view()
        .flex_1()
        .min_w(px(0.))
        .border_1()
        .border_color(BORDER)
        .rounded(px(RADIUS))
        .overflow_hidden()
        .child(heading);
    let measured = signal(0f32);
    if narrative {
        row = row
            .child(
                view()
                    .p_3()
                    .max_h(move || px(if expanded.get() { f32::MAX } else { 280. }))
                    .overflow_hidden()
                    .child(
                        view()
                            .flex_shrink_0()
                            .on_layout(move |b: &Bounds<Points>| measured.set(b.size.height.0))
                            .child(crate::links::reader(ui, move || body.get())),
                    ),
            )
            .child(when(
                move || measured.get() > 256.,
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
    view()
        .flex_row()
        .items_start()
        .gap_2()
        .py_1()
        .child(
            view()
                .w(px(28.))
                .h(px(28.))
                .rounded(px(14.))
                .bg(INSET)
                .items_center()
                .justify_center()
                .child(muted(initials)),
        )
        .child(row)
        .into_element()
}
