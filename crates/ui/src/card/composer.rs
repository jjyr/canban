use super::*;
use canban_ui_kit::markdown::MarkdownInput;
use nsh_base::editor::EditorCompletionAcceptEvent;
use serde_json::json;

pub fn render(ui: Ui, draft: Signal<Draft>, reading: Reading) -> impl IntoElement {
    let comment = proxy(draft, true);
    let editor = MarkdownInput::new();
    editor.restore_after(
        computed(move || ui.quick_open.get() || !ui.utility_popup.get().is_empty()),
        computed(move || {
            (ui.page.get() == "review"
                || ui.page.get() == "card" && ui.card_tab.get().is_empty())
                && ui.card_id() == ui.card.get()
        }),
    );
    reading.editor.replace(Some(editor));
    let card = untracked(|| ui.card_id());
    let key = format!("card:{card}");
    editor.on_submit(
        move || {
            (ui.page.get() == "review"
                || ui.page.get() == "card" && ui.card_tab.get().is_empty())
                && ui.card_id() == ui.card.get()
                && !ui.quick_open.get()
                && ui.utility_popup.get().is_empty()
        },
        move || ui.comment_submit.update(|n| *n += 1),
    );
    let enabled = computed(move || {
        !ui.is_method_pending("comment.add")
            && !comment.get().trim().is_empty()
            && !super::attachments::busy(draft, true)
            && ui.current_card().is_some_and(|c| c.archived_at.is_none())
    });
    let seen = stored(ui.comment_submit.get_untracked());
    effect(move || {
        let ticket = ui.comment_submit.get();
        if seen.replace(ticket) == ticket
            || !enabled.get_untracked()
            || !ui.utility_popup.get_untracked().is_empty()
            || ui.quick_open.get_untracked()
        {
            return;
        }
        let body = comment.get_untracked();
        let current_draft = draft.get_untracked();
        let mut mentions = crate::profiles::addressed(&body, &ui.profiles.get_untracked());
        for recipient in current_draft.recipients {
            if !mentions.contains(&recipient) {
                mentions.push(recipient);
            }
        }
        mentions.retain(|id| !current_draft.suppressed_recipients.contains(id));
        let submitted_mentions = mentions.clone();
        let card = card.clone();
        let key = key.clone();
        dispatch_complete(
            ui,
            "comment.add",
            json!({"card_id":card,"body":body,"mentions":mentions}),
            move |result| {
                let mut value = draft.try_get().unwrap_or_else(|| {
                    ui.drafts.with(|m| m.get(&key).cloned().unwrap_or_default())
                });
                match result {
                    Ok(_) => {
                        if value.comment == body
                            && value
                                .recipients
                                .iter()
                                .all(|id| submitted_mentions.contains(id))
                        {
                            value.comment.clear();
                            value.recipients.clear();
                            value.suppressed_recipients.clear();
                            value.uploads.retain(|a| !a.is_comment());
                        }
                        value.comment_error.clear();
                        reading.latest();
                    }
                    Err(error) => value.comment_error = error,
                }
                crate::drafts::put(ui, key, value.clone());
                draft.try_set(value);
            },
        );
    });
    editor.on_paste_files({
        let card = untracked(|| ui.card_id());
        move |files, uris| {
            super::attachments::add(
                ui,
                draft,
                card.clone(),
                files,
                super::attachments::paths(&uris),
                true,
            );
        }
    });
    let (source, reference_error) = crate::references::source(
        ui,
        editor,
        comment,
        computed(move || ui.current_card().map(|c| c.project_id).unwrap_or_default()),
        "Card comment",
        "Write a comment… @Agent or #Card / Issue / PR",
    );
    let source = source.on_completion_accept(move |event: &EditorCompletionAcceptEvent| {
        let id = event.item.label.to_string();
        if event
            .item
            .insert_text
            .as_ref()
            .is_some_and(|text| text.as_ref() == format!("@{id} "))
            && ui.profiles.with_untracked(|p| p.iter().any(|p| p.id == id))
        {
            draft.update(|d| {
                d.suppressed_recipients.retain(|p| p != &id);
                if !d.recipients.contains(&id) {
                    d.recipients.push(id);
                }
            });
        }
    });
    view()
        .element_ref(&reading.composer)
        .on_touch(|_: &TouchEvent| {})
        .on_file_drop({
            let card = untracked(|| ui.card_id());
            move |e: &nsh_core::element::FileDropEvent| {
                super::attachments::add(
                    ui,
                    draft,
                    card.clone(),
                    Vec::new(),
                    super::attachments::paths(&e.uris),
                    true,
                );
            }
        })
        .border_t_1()
        .border_color(BORDER)
        .p_2()
        .gap_1()
        .flex_shrink_0()
        .child(when(
            move || {
                ui.current_card()
                    .is_some_and(|c| c.github_issue().is_some() || c.pull_request.is_some())
            },
            || muted("Local discussion · not posted to GitHub"),
        ))
        .child(editor.render_with_preview(
            comment,
            source,
            true,
            reading.composer_height,
            move || super::markdown(ui, move || comment.get()).into_element(),
        ))
        .child(when(
            move || !reference_error.get().is_empty(),
            move || notice(move || reference_error.get(), Tone::Attention),
        ))
        .child(dynamic(move || {
            let d = draft.get();
            let mut recipients = crate::profiles::addressed(&comment.get(), &ui.profiles.get());
            for id in d.recipients {
                if !recipients.contains(&id) {
                    recipients.push(id);
                }
            }
            recipients.retain(|id| !d.suppressed_recipients.contains(id));
            view().flex_row().flex_wrap().gap_1().children(
                recipients
                    .into_iter()
                    .map(|id| {
                        action(format!("@{id} ×"), Variant::Quiet, move |_| {
                            let updated = crate::profiles::remove_address(
                                &comment.get_untracked(),
                                &id,
                                &ui.profiles.get_untracked(),
                            );
                            let still_in_prose =
                                crate::profiles::addressed(&updated, &ui.profiles.get_untracked())
                                    .contains(&id);
                            draft.update(|d| {
                                d.recipients.retain(|p| p != &id);
                                if still_in_prose && !d.suppressed_recipients.contains(&id) {
                                    d.suppressed_recipients.push(id.clone());
                                }
                            });
                            if updated != comment.get_untracked() {
                                comment.set(updated);
                            }
                            editor.focus();
                        })
                        .into_element()
                    })
                    .collect::<Vec<_>>(),
            )
        }))
        .child(when(
            move || draft.with(|d| !d.comment_error.is_empty()),
            move || notice(move || draft.get().comment_error, Tone::Danger),
        ))
        .child(
            view()
                .flex_row()
                .items_end()
                .gap_2()
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .child(super::attachments::controls(
                            ui,
                            draft,
                            editor,
                            true,
                            untracked(|| ui.card_id()),
                        )),
                )
                .child(
                    action("Send", Variant::Primary, move |_| {
                        ui.comment_submit.update(|n| *n += 1)
                    })
                    .accessibility_label(|| Some("Send comment · ⌘Enter".into()))
                    .disabled(move || !enabled.get()),
                ),
        )
}
