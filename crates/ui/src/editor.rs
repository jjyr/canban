//! Shared Card body: source, preview, references and retained draft feedback.
use crate::{drafts::*, presenter::*};
use canban_ui_kit::{markdown::MarkdownInput, *};
use nsh_core::prelude::*;

pub fn body(
    ui: Ui,
    text: Signal<String>,
    project: Memo<String>,
    active: Memo<bool>,
    height: Signal<f32>,
    submit: impl Fn() + 'static,
) -> (MarkdownInput, impl IntoElement) {
    let editor = MarkdownInput::new();
    editor.restore_after(
        computed(move || ui.quick_open.get() || !ui.utility_popup.get().is_empty()),
        active,
    );
    editor.on_submit(
        move || {
            active.get()
                && !ui.quick_open.get()
                && ui.utility_popup.get().is_empty()
                && !ui.native_modal.get()
        },
        submit,
    );
    let focus_ticket = stored(ui.editor_focus.get_untracked());
    effect(move || {
        let ticket = ui.editor_focus.get();
        if focus_ticket.replace(ticket) != ticket && active.get_untracked() {
            spawn(async move {
                next_frame().await;
                if active.try_get() == Some(true) {
                    editor.restore_focus();
                }
            });
        }
    });
    let (source, reference_error) = crate::references::source(
        ui,
        editor,
        text,
        project,
        "Task description",
        "Describe the task in Markdown…",
    );
    let body = editor.render_with_preview(text, source, false, height, move || {
        reader(ui, move || text.get()).into_element()
    });
    (
        editor,
        view().gap_2().child(body).child(when(
            move || !reference_error.get().is_empty(),
            move || notice(move || reference_error.get(), Tone::Attention),
        )),
    )
}
pub fn reader(ui: Ui, body: impl Fn() -> String + 'static) -> nsh_base::TextViewElement {
    crate::links::reader(ui, body)
}

pub fn draft_status(ui: Ui, key: String, _draft: Signal<Draft>) -> impl IntoElement {
    let status_key = key.clone();
    let error_key = key.clone();
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(muted(move || {
            let _ = ui.draft_status.get();
            persistence::status(&status_key)
        }))
        .child(when(
            move || {
                let _ = ui.draft_status.get();
                persistence::status(&error_key).starts_with("Draft not saved:")
            },
            move || {
                let key = key.clone();
                action("Retry draft save", Variant::Quiet, move |_| {
                    persistence::retry(&key)
                })
            },
        ))
}
/// The workspace changes width without navigating or replacing the source.
pub fn expand(ui: Ui) -> impl IntoElement {
    compact_action(
        move || {
            if ui.card_standalone.get() {
                "Collapse editor"
            } else {
                "Expand editor"
            }
        },
        Variant::Quiet,
        move |_| {
            ui.card_standalone.update(|v| *v = !*v);
            ui.editor_focus.update(|n| *n += 1);
        },
    )
    .disabled(move || ui.narrow.get())
}
