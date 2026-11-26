use super::*;
use canban_ui_kit::markdown::MarkdownInput;
use serde_json::json;

pub fn render(ui: Ui, draft: Signal<Draft>, reading: Reading) -> impl IntoElement {
    let comment = proxy(draft, true);
    let editor = MarkdownInput::new();
    editor.restore_after(
        computed(move || ui.quick_open.get() || !ui.utility_popup.get().is_empty()),
        computed(move || {
            (ui.page.get() == "review"
                || ui.page.get() == "card" && !(ui.narrow.get() && ui.card_terminal_open.get()))
                && ui.card_id() == ui.card.get()
                && ui.editing_comment.get().is_none()
                && !ui.card_terminal_focused.get()
        }),
    );
    reading.editor.replace(Some(editor));
    let card = untracked(|| ui.card_id());
    let key = format!("card:{card}");
    editor.on_submit(
        move || {
            (ui.page.get() == "review"
                || ui.page.get() == "card" && !(ui.narrow.get() && ui.card_terminal_open.get()))
                && ui.card_id() == ui.card.get()
                && ui.editing_comment.get().is_none()
                && !ui.card_terminal_focused.get()
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
        let assignee = ui.current_card().and_then(|c| c.assignee);
        let mentions = crate::profiles::active_recipients_with_assignee(
            &[&body],
            &current_draft.recipients,
            assignee.as_deref(),
            &current_draft.suppressed_recipients,
            &ui.profiles.get_untracked(),
        );
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
    let (source, reference_error) = crate::references::attach(
        ui,
        editor,
        editor.source(
            comment,
            "Card comment",
            "Write a comment… @Agent, #Card or #tag",
        ),
        comment,
        computed(move || ui.current_card().map(|c| c.project_id).unwrap_or_default()),
        crate::references::ReferenceFormat::Markdown,
        Some(std::rc::Rc::new(move |id| {
            draft.update(|d| {
                d.suppressed_recipients.retain(|p| p != &id);
                if !d.recipients.contains(&id) {
                    d.recipients.push(id);
                }
            });
        })),
    );
    view()
        .element_ref(&reading.composer)
        .on_touch(move |e: &TouchEvent| {
            if e.phase == TouchPhase::Start && ui.card_terminal_focused.get_untracked() {
                ui.card_terminal_focused.set(false);
            }
        })
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
        .child(crate::editor::recipient_chips_with_assignee(
            ui,
            vec![comment],
            move || draft.with(|d| d.recipients.clone()),
            move || ui.current_card().and_then(|c| c.assignee),
            move || draft.with(|d| d.suppressed_recipients.clone()),
            move |id, still_in_prose| {
                let is_assignee = ui
                    .current_card()
                    .and_then(|c| c.assignee)
                    .is_some_and(|a| a == id);
                draft.update(|d| {
                    d.recipients.retain(|p| p != &id);
                    if (still_in_prose || is_assignee) && !d.suppressed_recipients.contains(&id) {
                        d.suppressed_recipients.push(id);
                    }
                });
                editor.focus();
            },
        ))
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
                    .accessibility_label(|| {
                        Some(
                            format!(
                                "Send comment · {}",
                                crate::commands::shortcuts::SUBMIT_DISPLAY
                            )
                            .into(),
                        )
                    })
                    .disabled(move || !enabled.get()),
                ),
        )
}
