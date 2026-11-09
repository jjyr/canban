use super::*;
use nsh_core::keymap::When;
use serde_json::json;
pub fn render(ui: Ui, draft: Signal<Draft>, reading: Reading) -> impl IntoElement {
    let comment = proxy(draft, true);
    let focused = signal(false);
    let card = ui.card.get_untracked();
    let key = format!("card:{card}");
    effect(move || {
        if focused.get() && !ui.quick_open.get() && ui.utility_popup.get().is_empty() {
            on_key_with(
                crate::commands::SEND_COMMENT.key,
                KeyOptions {
                    when: When::Always,
                    ..Default::default()
                },
                move |_| ui.comment_submit.update(|n| *n += 1),
            );
        }
    });
    let enabled = computed(move || {
        !ui.pending.get()
            && !comment.get().trim().is_empty()
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
        let mentions = crate::profiles::addressed(&body, &ui.profiles.get_untracked());
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
                        if value.comment == body {
                            value.comment.clear();
                        }
                        value.error.clear();
                        reading.latest();
                    }
                    Err(error) => value.error = error,
                }
                ui.drafts.with(|m| {
                    m.insert(key, value.clone());
                });
                draft.try_set(value);
            },
        );
    });
    let paste = ui.card.get_untracked();
    view()
        .border_t_1()
        .border_color(BORDER)
        .p_2()
        .gap_1()
        .flex_shrink_0()
        .child(
            field(comment, "Comment or @Agent…")
                .label("Card comment")
                .multiline(true)
                .h(move || {
                    px(
                        (48. + comment.get().lines().count().saturating_sub(2) as f32 * 18.)
                            .min(120.),
                    )
                })
                .on_focus(move |_: &()| focused.set(true))
                .on_blur(move |_: &()| focused.set(false))
                .on_paste(move |e: &nsh_core::PasteEvent| {
                    paste_resources(ui, paste.clone(), e, comment, true)
                }),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(crate::profiles::mention_picker(ui.profiles, comment))
                .child(chrome_button(Icon::Plus, "Attach file", move |_| {
                    draft.update(|d| d.details = true);
                    reading.top();
                }))
                .child(view().flex_1())
                .child(
                    view()
                        .tooltip(nsh_base::tooltip().child(label("Send · ⌘Enter")))
                        .child(
                            action("Send", Variant::Primary, move |_| {
                                ui.comment_submit.update(|n| *n += 1)
                            })
                            .accessibility_label(|| Some("Send comment · ⌘Enter".into()))
                            .disabled(move || !enabled.get()),
                        ),
                ),
        )
}
