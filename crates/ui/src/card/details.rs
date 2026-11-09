use super::*;
use crate::components::context_picker;
use serde_json::json;
pub fn render(ui: Ui, draft: Signal<Draft>) -> impl IntoElement {
    when(
        move || draft.with(|d| d.details),
        move || {
            let path = signal(String::new());
            view()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .justify_between()
                        .child(label("Context and attachments"))
                        .child(action("Hide", Variant::Quiet, move |_| {
                            draft.update(|d| d.details = false)
                        })),
                )
                .child(context_picker(ui))
                .child(
                    view()
                        .flex_row()
                        .gap_2()
                        .child(field(path, "File path to attach").flex_1().min_w(px(0.)))
                        .child(
                            action("Attach", Variant::Secondary, move |_| {
                                if let Some(c) = ui.current_card() {
                                    let source = path.get_untracked();
                                    let name = std::path::Path::new(&source)
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_default();
                                    dispatch(
                                        ui,
                                        "attachment.add",
                                        json!({"card_id":c.id,"source":source,"name":name}),
                                    );
                                }
                            })
                            .disabled(move || ui.pending.get() || path.get().trim().is_empty()),
                        ),
                )
                .child(dynamic(move || {
                    let card = ui.card.get();
                    view().gap_1().children(ui.snapshot.with(|s| {
                        s.artifacts
                            .iter()
                            .filter(|a| a.card_id == card)
                            .map(|a| {
                                let (id, path) = (a.card_id.clone(), a.path.clone());
                                action(a.name.clone(), Variant::Quiet, move |_| {
                                    open_resource(ui, id.clone(), path.clone())
                                })
                                .into_element()
                            })
                            .collect::<Vec<_>>()
                    }))
                }))
        },
    )
}
