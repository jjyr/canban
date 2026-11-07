use super::*;
/// Select execution context and navigate to the same database document.
pub fn context_picker(ui: Ui) -> impl IntoElement {
    view().gap_2().child(label("Human Context · Notes"))
        .child(muted("Selection applies to future executions; saved captures retain their versions."))
        .child(dynamic(move||{
            let Some(card)=ui.current_card()else{return view().into_element()};
            let notes=ui.snapshot.with(|s|s.notes.iter().filter(|n|n.project_id==card.project_id).cloned().collect::<Vec<_>>());
            if notes.is_empty(){return muted("Add a Project Note to make context available.").into_element()}
            view().gap_1().children(notes.into_iter().map(|n|{
                let id=n.id.clone();let open=id.clone();let checked=signal(card.context_notes.as_ref().is_none_or(|ids|ids.contains(&id)));
                view().flex_row().justify_between().items_center().gap_2()
                    .child(checkbox(n.title,checked,move|value:&bool|{
                        if let Some(card)=ui.current_card(){
                            let mut selected=card.context_notes.clone().unwrap_or_else(||ui.snapshot.with_untracked(|s|s.notes.iter().filter(|n|n.project_id==card.project_id).map(|n|n.id.clone()).collect()));
                            selected.retain(|v|v!=&id);if *value{selected.push(id.clone());}
                            let value=*value;dispatch_complete(ui,"card.context.update",json!({"card_id":card.id,"revision":card.revision,"note_ids":selected}),move|r|{if r.is_err(){checked.try_set(!value);}});
                        }
                    }).disabled(move||ui.pending.get()))
                    .child(action("Open Note",Variant::Quiet,move|_|ui.open_note(open.clone()))).into_element()
            }).collect::<Vec<_>>()).into_element()
        }))
}
