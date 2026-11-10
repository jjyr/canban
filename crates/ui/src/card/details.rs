use super::*;
pub fn render(ui: Ui, draft: Signal<Draft>) -> impl IntoElement {
    when(
        move || draft.with(|d| d.details),
        move || {
            view()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .justify_between()
                        .child(label("Attachments"))
                        .child(action("Hide", Variant::Quiet, move |_| {
                            draft.update(|d| d.details = false)
                        })),
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
