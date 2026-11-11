use super::*;

pub fn render(ui: Ui, draft: Signal<Draft>, reading: Reading) -> impl IntoElement {
    let body = proxy(draft, false);
    let editing = computed(move || draft.with(|d| d.editing));
    let github = ui
        .current_card()
        .is_some_and(|c| c.github_issue().is_some());
    let id = untracked(|| ui.card_id());
    let seen = stored(ui.task_submit.get_untracked());
    effect({
        let id = id.clone();
        move || {
            let ticket = ui.task_submit.get();
            if seen.replace(ticket) != ticket
                && editing.get_untracked()
                && matches!(ui.page.get_untracked().as_str(), "card" | "review")
                && ui.card_id() == ui.card.get_untracked()
                && ui.utility_popup.get_untracked().is_empty()
                && !ui.quick_open.get_untracked()
                && !ui.native_modal.get_untracked()
            {
                save_card(ui, id.clone(), draft);
            }
        }
    });
    effect(move || {
        if editing.get()
            && matches!(ui.page.get().as_str(), "card" | "review")
            && ui.card_id() == ui.card.get()
            && ui.utility_popup.get().is_empty()
            && !ui.quick_open.get()
            && !ui.native_modal.get()
        {
            on_key_with(
                "mod+enter",
                KeyOptions {
                    when: nsh_core::keymap::When::Always,
                    ..Default::default()
                },
                move |_| ui.task_submit.update(|n| *n += 1),
            );
        }
    });
    view()
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .child(muted("Description"))
                .child(view().flex_1())
                .child(chrome_button_disabled(
                    Icon::Edit,
                    "Edit description",
                    move || editing.get(),
                    move |_| {
                        if let Some(editor) = reading.editor.get() {
                            editor.blur();
                        }
                        draft.update(|d| d.editing = true);
                        reading.top();
                    },
                )),
        )
        .child(dynamic(move || {
            if editing.get() {
                let paste = id.clone();
                let height = signal(280.);
                effect(move || height.set(if ui.card_standalone.get() { 520. } else { 280. }));
                let current = computed(move || {
                    matches!(ui.page.get().as_str(), "card" | "review")
                        && ui.card_id() == ui.card.get()
                });
                let (editor, body_view) = crate::editor::body(
                    ui,
                    body,
                    computed(move || draft.with(|d| d.project_id.clone())),
                    current,
                    height,
                    move || ui.task_submit.update(|n| *n += 1),
                );
                editor.on_paste_files({
                    let card = id.clone();
                    move |files, uris| {
                        super::attachments::add(
                            ui,
                            draft,
                            card.clone(),
                            files,
                            super::attachments::paths(&uris),
                            false,
                        )
                    }
                });
                let focused_once = stored(false);
                return view()
                    .on_layout(move |_: &Bounds<Points>| {
                        // The native sink must exist before taking focus.
                        if !focused_once.replace(true) {
                            editor.focus();
                        }
                    })
                    .gap_2()
                    .child(
                        bound_field(
                            move || draft.get().title,
                            move |v| draft.update(|d| d.title = v.into()),
                            "Card title",
                        )
                        .disabled(github),
                    )
                    .child(body_view)
                    .child(super::attachments::controls(
                        ui, draft, editor, false, paste,
                    ))
                    .child(crate::editor::draft_status(
                        ui,
                        format!("card:{}", id),
                        draft,
                    ))
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                compact_action("Save", Variant::Primary, move |_| {
                                    ui.task_submit.update(|n| *n += 1)
                                })
                                .disabled(move || {
                                    ui.pending.get()
                                        || draft.with(|d| d.title.trim().is_empty())
                                        || super::attachments::busy(draft, false)
                                }),
                            )
                            .child(
                                compact_action("Cancel", Variant::Quiet, move |_| {
                                    draft.update(|d| d.editing = false)
                                })
                                .disabled(move || {
                                    ui.pending.get()
                                        || draft.with(|d| d.title.trim().is_empty())
                                        || super::attachments::busy(draft, false)
                                }),
                            ),
                    )
                    .into_element();
            }
            view()
                .gap_2()
                .child(crate::github::card_source(ui))
                .child(crate::github::references::panel(ui))
                .child(super::markdown(ui, move || {
                    ui.current_card().map(|c| c.body).unwrap_or_default()
                }))
                .into_element()
        }))
        .child(when(
            move || draft.with(|d| !d.error.is_empty()),
            move || {
                view()
                    .gap_2()
                    .child(notice(move || draft.get().error, Tone::Danger))
                    .child(muted(
                        "Latest saved description · your draft is retained above",
                    ))
                    .child(
                        crate::editor::reader(ui, move || {
                            ui.current_card().map(|c| c.body).unwrap_or_default()
                        })
                        .max_h(px(220.)),
                    )
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .gap_2()
                            .child(compact_action(
                                "Keep draft",
                                Variant::Secondary,
                                move |_| {
                                    if let Some(c) = ui.current_card() {
                                        draft.update(|d| {
                                            if github {
                                                d.title = c.title.clone();
                                            }
                                            d.base_title = c.title;
                                            d.base_body = c.body;
                                            d.revision = c.revision;
                                            d.error.clear();
                                        });
                                    }
                                },
                            ))
                            .child(compact_action("Discard", Variant::Secondary, move |_| {
                                if let Some(c) = ui.current_card() {
                                    draft.update(|d| {
                                        let saved = Draft::card(&c);
                                        d.title = saved.title;
                                        d.body = saved.body;
                                        d.base_title = saved.base_title;
                                        d.base_body = saved.base_body;
                                        d.revision = saved.revision;
                                        d.error.clear();
                                    });
                                }
                            })),
                    )
            },
        ))
}
