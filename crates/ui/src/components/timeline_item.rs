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
        if let Some(attempt_id) = event.data["attempt_id"].as_str().map(str::to_owned)
            && !matches!(
                event.kind.as_str(),
                "execution.queued"
                    | "execution.started"
                    | "execution.attached"
                    | "execution.reported"
            )
        {
            row = row.child(compact_action("Terminal ↗", Variant::Quiet, move |_| {
                crate::terminals::open_request(ui, &attempt_id)
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
        .unwrap_or_else(|| {
            if event.actor == "user" {
                github_login(ui).unwrap_or_else(|| "Me".into())
            } else {
                event.actor.clone()
            }
        });
    let face = avatar(ui, &event);
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
    if let Some(attempt_id) = event.data["attempt_id"].as_str().map(str::to_owned) {
        let details = attempt_id.clone();
        heading = heading
            .child(compact_action("Details", Variant::Quiet, move |_| {
                ui.request.set(details.clone())
            }))
            .child(compact_action("Terminal ↗", Variant::Quiet, move |_| {
                crate::terminals::open_request(ui, &attempt_id)
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
        .child(face)
        .child(row)
        .into_element()
}
const AVATAR: f32 = 28.;
/// The signed-in GitHub account, when connected.
fn github_login(ui: Ui) -> Option<String> {
    ui.snapshot.with_untracked(|s| {
        s.github
            .login
            .clone()
            .filter(|l| s.github.state == "connected" && !l.is_empty())
    })
}
/// Agents use their provider icon; the local user uses the GitHub avatar when
/// signed in ("ME" while signed out or before the image loads); other actors
/// keep two-letter initials.
fn avatar(ui: Ui, event: &Event) -> AnyElement {
    let frame = || {
        view()
            .relative()
            .flex_shrink_0()
            .w(px(AVATAR))
            .h(px(AVATAR))
            .rounded(px(AVATAR / 2.))
            .overflow_hidden()
            .bg(INSET)
            .items_center()
            .justify_center()
    };
    let initials = |value: &str| {
        text(value.to_owned())
            .text_size(px(10.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(MUTED)
    };
    if let Some(profile) = event.actor.strip_prefix("agent:") {
        let mark = ui
            .snapshot
            .with_untracked(|s| {
                event.data["attempt_id"]
                    .as_str()
                    .and_then(|id| s.attempts.iter().find(|a| a.id == id))
                    .map(|a| a.profile.clone())
            })
            .or_else(|| {
                ui.profiles
                    .with_untracked(|all| all.iter().find(|p| p.id == profile).cloned())
            })
            .map(|p| crate::profiles::provider(&p).0)
            .unwrap_or(Icon::Agent);
        return frame()
            .tooltip(hint_tooltip(format!("@{profile}")))
            .child(icon(mark).w(px(16.)).h(px(16.)))
            .into_element();
    }
    if event.actor == "user" {
        let mut face = frame().child(initials("ME"));
        if let Some(login) = github_login(ui) {
            face = face.child(
                image(format!("https://github.com/{login}.png?size=64"))
                    .absolute()
                    .top(px(0.))
                    .left(px(0.))
                    .w(px(AVATAR))
                    .h(px(AVATAR)),
            );
        }
        return face.into_element();
    }
    let short = event
        .actor
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    frame().child(initials(&short)).into_element()
}
