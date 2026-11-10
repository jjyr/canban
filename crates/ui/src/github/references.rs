use super::*;
/// References never change a Local Card's source kind or publish local text.
pub fn panel(ui: Ui) -> impl IntoElement {
    let card = ui.card_id();
    let draft = drafts::document(ui, format!("github-links:{card}"), Draft::default());
    view().gap_2().child(label("GitHub references"))
        .child(dynamic(move||{
            let Some(c)=ui.current_card()else{return view().into_element()};
            let mut rows=view().gap_1();
            for r in c.github_links {
                let open_ref=r.clone();let remove=r.clone();let id=c.id.clone();
                rows=rows.child(view().flex_row().gap_1().child(action(format!("{} · {} {}",r.repository,r.kind,r.number.map(|n|format!("#{n}")).or(r.sha).unwrap_or_default()),Variant::Quiet,move |_|super::open(ui,open_ref.clone())))
                    .child(action("Remove reference",Variant::Quiet,move |_|dispatch(ui,"github.reference.remove",json!({"card_id":id,"revision":c.revision,"reference":remove})))));
            }
            if let Some(pr)=c.pull_request{let r=GitHubReference{repository:pr.repository,kind:"pr".into(),number:Some(pr.number),sha:None,url:pr.url};rows=rows.child(action("Open delivery PR · discussion & diff",Variant::Secondary,move |_|super::open(ui,r.clone())));}
            rows.into_element()
        }))
        .child(view().flex_row().gap_2().child(bound_field(move||draft.get().title,move|v|draft.update(|d|d.title=v.into()),"Paste an Issue, PR or commit URL").key(format!("github-link:{card}")))
            .child(action("Add reference",Variant::Secondary,move |_|{if let Some(c)=ui.current_card(){dispatch_complete(ui,"github.reference.add",json!({"card_id":c.id,"revision":c.revision,"url":draft.get_untracked().title}),move|result|{if result.is_ok(){draft.update(|d|d.title.clear());}})}}).disabled(move||ui.pending.get()||draft.get().title.trim().is_empty())))
}
