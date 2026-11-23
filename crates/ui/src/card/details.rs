use super::*;
pub fn render(ui: Ui, draft: Signal<Draft>) -> impl IntoElement {
    let attachments = computed(move || {
        let card = ui.card_id();
        ui.index.with(|idx| {
            idx.artifacts
                .iter()
                .filter(|a| a.card_id == card)
                .map(|a| (a.card_id.clone(), a.name.clone(), a.path.clone()))
                .collect::<Vec<_>>()
        })
    });
    when(
        move || draft.with(|d| d.details),
        move || {
            view()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(label("Attachments"))
                        .child(chrome_button(Icon::Close, "Hide attachments", move |_| {
                            draft.update(|d| d.details = false)
                        })),
                )
                .child(dynamic(move || {
                    view().gap_1().children(
                        attachments
                            .get()
                            .into_iter()
                            .map(|(id, name, path)| {
                                action(name, Variant::Quiet, move |_| {
                                    open_resource(ui, id.clone(), path.clone())
                                })
                                .into_element()
                            })
                            .collect::<Vec<_>>(),
                    )
                }))
        },
    )
}
