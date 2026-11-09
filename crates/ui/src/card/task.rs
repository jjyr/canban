use super::*;
pub fn render(ui: Ui, draft: Signal<Draft>, reading: Reading) -> impl IntoElement {
    let body = proxy(draft, false);
    let github = ui
        .current_card()
        .is_some_and(|c| c.github_issue().is_some());
    let id = ui.card.get_untracked();
    let paste = id.clone();
    view()
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(action(
                    move || {
                        if draft.with(|d| d.task_expanded || d.editing) {
                            "▾ Task"
                        } else {
                            "▸ Task"
                        }
                    },
                    Variant::Quiet,
                    move |_| {
                        draft.update(|d| {
                            d.task_expanded = !d.task_expanded;
                        });
                        reading.top();
                    },
                ))
                .child(view().flex_1())
                .child(action("Edit", Variant::Quiet, move |_| {
                    draft.update(|d| {
                        d.editing = !d.editing;
                        d.task_expanded = true;
                    });
                    reading.top();
                })),
        )
        .child(dynamic(move || {
            if draft.with(|d| d.editing) {
                let paste = paste.clone();
                let save_id = id.clone();
                return view()
                    .gap_2()
                    .child(
                        bound_field(
                            move || draft.get().title,
                            move |v| draft.update(|d| d.title = v.into()),
                            "Card title",
                        )
                        .disabled(github),
                    )
                    .child(
                        field(body, "Task description")
                            .multiline(true)
                            .h(px(160.))
                            .on_paste(move |e: &nsh_core::PasteEvent| {
                                paste_resources(ui, paste.clone(), e, body, false)
                            }),
                    )
                    .child(bound_field(
                        move || draft.get().tags,
                        move |v| draft.update(|d| d.tags = v.into()),
                        "Routing tags",
                    ))
                    .child(
                        action("Save", Variant::Primary, move |_| {
                            save_card(ui, save_id.clone(), draft)
                        })
                        .disabled(move || ui.pending.get()),
                    )
                    .into_element();
            }
            if draft.with(|d| d.task_expanded) {
                return view()
                    .gap_2()
                    .child(crate::github::card_source(ui))
                    .child(
                        nsh_base::text_view()
                            .markdown(move || Some(draft.get().body.into()))
                            .on_link_press(move |e: &nsh_base::TextViewLinkPressEvent| {
                                if let Some(id) = e.url.strip_prefix("note:") {
                                    ui.open_note(id.into());
                                }
                            }),
                    )
                    .into_element();
            }
            let text = draft.with(|d| {
                d.body
                    .lines()
                    .filter(|s| !s.trim().is_empty())
                    .take(3)
                    .map(|line| line.trim().trim_start_matches('#').trim())
                    .collect::<Vec<_>>()
                    .join(" ")
            });
            if text.is_empty() {
                return view().into_element();
            }
            muted(crate::components::git_workspace::compact(&text, 150))
                .truncate()
                .into_element()
        }))
        .child(dynamic(move || {
            let error = draft.with(|d| d.error.clone());
            if error.is_empty() {
                return view().into_element();
            }
            view()
                .gap_2()
                .child(notice(error, Tone::Danger))
                .child(action("Reload saved task", Variant::Secondary, move |_| {
                    if let Some(c) = ui.current_card() {
                        let old = draft.get_untracked();
                        let mut next = Draft::card(&c);
                        next.comment = old.comment;
                        next.task_expanded = true;
                        draft.set(next);
                    }
                }))
                .into_element()
        }))
}
