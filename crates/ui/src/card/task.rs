use super::*;
use canban_ui_kit::markdown::MarkdownInput;

pub fn render(ui: Ui, draft: Signal<Draft>, reading: Reading) -> impl IntoElement {
    let body = proxy(draft, false);
    let editing = computed(move || draft.with(|d| d.editing));
    let github = ui
        .current_card()
        .is_some_and(|c| c.github_issue().is_some());
    let id = ui.card.get_untracked();
    view()
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .child(muted("Description"))
                .child(view().flex_1())
                .child(
                    action("Edit", Variant::Quiet, move |_| {
                        if let Some(editor) = reading.editor.get() {
                            editor.blur();
                        }
                        draft.update(|d| d.editing = true);
                        reading.top();
                    })
                    .disabled(move || editing.get()),
                ),
        )
        .child(dynamic(move || {
            if editing.get() {
                let editor = MarkdownInput::new();
                let paste = id.clone();
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
                        );
                    }
                });
                let save_id = id.clone();
                editor.on_submit(
                    move || !ui.quick_open.get() && ui.utility_popup.get().is_empty(),
                    {
                        let save_id = id.clone();
                        move || {
                            if !ui.pending.get_untracked()
                                && !super::attachments::busy(draft, false)
                            {
                                save_card(ui, save_id.clone(), draft);
                            }
                        }
                    },
                );
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
                    .child(editor.render_with_preview(
                        body,
                        editor.source(body, "Task description", "Describe the task in Markdown…"),
                        false,
                        signal(220.),
                        move || super::markdown(ui, move || body.get()).into_element(),
                    ))
                    .child(super::attachments::controls(
                        ui, draft, editor, false, paste,
                    ))
                    .child(
                        view()
                            .flex_row()
                            .gap_2()
                            .child(
                                action("Save", Variant::Primary, move |_| {
                                    save_card(ui, save_id.clone(), draft)
                                })
                                .disabled(move || {
                                    ui.pending.get() || super::attachments::busy(draft, false)
                                }),
                            )
                            .child(
                                action("Cancel", Variant::Quiet, move |_| {
                                    draft.update(|d| {
                                        d.title = d.base_title.clone();
                                        d.body = d.base_body.clone();
                                        d.editing = false;
                                        d.uploads.retain(|a| a.is_comment());
                                    });
                                })
                                .disabled(move || {
                                    ui.pending.get() || super::attachments::busy(draft, false)
                                }),
                            ),
                    )
                    .into_element();
            }
            view()
                .gap_2()
                .child(crate::github::card_source(ui))
                .child(super::markdown(ui, move || draft.get().body))
                .into_element()
        }))
        .child(when(
            move || draft.with(|d| !d.error.is_empty()),
            move || {
                view()
                    .gap_2()
                    .child(notice(move || draft.get().error, Tone::Danger))
                    .child(action("Reload saved task", Variant::Secondary, move |_| {
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
                    }))
            },
        ))
}
