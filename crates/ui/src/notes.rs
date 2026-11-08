//! Database document workspace: editor/preview plus links, Cards and provenance.
use crate::{
    components::*,
    drafts::{self, *},
    presenter::*,
};
use canban_types::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;
pub fn screen(ui: Ui) -> impl IntoElement {
    let query = signal(String::new());
    view().flex_1().min_h(px(0.0)).min_w(px(0.0)).child(split(
        view()
            .min_h(px(0.0))
            .flex_1()
            .p_3()
            .gap_3()
            .bg(SURFACE)
            .child(
                muted(move || {
                    let _ = ui.project.get();
                    ui.current_project()
                        .map(|p| p.name)
                        .unwrap_or_else(|| "No Project selected".into())
                })
                .truncate(),
            )
            .child(
                view()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .child(heading("Notes"))
                    .child(action("+ New", Variant::Quiet, move |_| {
                        ui.note.set(format!("new:{}", id()));
                        ui.clear_output();
                    })),
            )
            .child(field(query, "Find a Note…"))
            .child(scroll().flex_1().child(dynamic(move || {
                let project = ui.project.get();
                let q = query.get().to_lowercase();
                let s = ui.snapshot.get();
                let notes = s
                    .notes
                    .into_iter()
                    .filter(|n| n.project_id == project && n.title.to_lowercase().contains(&q))
                    .collect::<Vec<_>>();
                if notes.is_empty() {
                    return empty(
                        "Project Notes",
                        "Create a document for ideas, decisions or shared context.",
                    )
                    .into_element();
                }
                view()
                    .gap_1()
                    .children(
                        notes
                            .into_iter()
                            .map(|n| {
                                let id = n.id;
                                let selected = id.clone();
                                navigation(
                                    n.title,
                                    move || ui.note.get() == selected,
                                    move |_| ui.open_note(id.clone()),
                                )
                                .into_element()
                            })
                            .collect::<Vec<_>>(),
                    )
                    .into_element()
            })))
            .into_element(),
        dynamic(move || {
            let selected = ui.note.get();
            let project = ui.project.get();
            untracked(|| editor(ui, &selected, &project))
        })
        .into_element(),
        signal(220.0),
        160.0,
        360.0,
    ))
}
pub fn editor(ui: Ui, selected: &str, project: &str) -> impl IntoElement + use<> {
    let n = ui.snapshot.with_untracked(|s| {
        s.notes
            .iter()
            .find(|n| n.id == selected && n.project_id == project)
            .cloned()
    });
    let key = if selected.is_empty() {
        format!("note:new:{project}")
    } else {
        format!("note:{selected}")
    };
    let note_id = n.as_ref().map(|n| n.id.clone());
    let note_for_save = note_id.clone();
    let note_for_relations = note_id.clone();
    let project = project.to_owned();
    let draft = document(
        ui,
        key.clone(),
        n.as_ref().map(Draft::note).unwrap_or_else(|| Draft {
            editing: true,
            ..Default::default()
        }),
    );
    if let Some(id) = note_id.clone() {
        effect(move || {
            if let Some(n) = ui
                .snapshot
                .with(|s| s.notes.iter().find(|n| n.id == id).cloned())
            {
                let mut d = draft.get();
                d.reconcile(Draft::note(&n));
                if d != draft.get_untracked() {
                    draft.set(d);
                }
            }
        });
    }
    let preview = signal(false);
    let related = signal(!ui.narrow.get_untracked());
    effect(move || {
        if ui.narrow.get() {
            related.set(false);
        }
    });
    view()
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .child(dynamic(move || {
            let narrow = ui.narrow.get();
            let show = related.get();
            if narrow && show {
                return view()
                    .flex_1()
                    .child(action("← Note", Variant::Quiet, move |_| {
                        related.set(false)
                    }))
                    .child(relations(ui, note_for_relations.clone(), draft))
                    .into_element();
            }
            let body = document_body(
                ui,
                draft,
                key.clone(),
                project.clone(),
                note_for_save.clone(),
                preview,
                related,
            )
            .into_element();
            if narrow || !show {
                return body;
            }
            split(
                body,
                relations(ui, note_for_relations.clone(), draft).into_element(),
                signal(560.0),
                360.0,
                240.0,
            )
            .into_element()
        }))
}
fn document_body(
    ui: Ui,
    draft: Signal<Draft>,
    key: String,
    project: String,
    note_id: Option<String>,
    preview: Signal<bool>,
    related: Signal<bool>,
) -> impl IntoElement {
    let note_for_save = note_id.clone();
    let is_new = note_id.is_none();
    view().flex_1().min_h(px(0.0)).min_w(px(0.0)).p_4().gap_3()
        .child(view().flex_row().flex_wrap().justify_between().items_center().gap_2()
            .child(heading(move||{let title=draft.get().title;if title.is_empty(){"New Note".into()}else{title}}))
            .child(muted(move||if ui.pending.get(){"Saving…"}else if is_new{"Not saved"}else if draft.with(|d|d.dirty()){"Unsaved changes"}else{"Saved"})))
        .child(view().flex_row().flex_wrap().gap_2()
            .child(action("Edit",Variant::Quiet,move|_|preview.set(false)))
            .child(action("Preview",Variant::Quiet,move|_|preview.set(true)))
            .child(action("Save Note",Variant::Primary,move|_|{let value=draft.get_untracked();let mut p=json!({"project_id":project,"title":value.title,"body":value.body});if let Some(id)=&note_for_save{p["note_id"]=json!(id);p["revision"]=json!(value.revision);}drafts::save(ui,key.clone(),draft,"note.save",p);}).disabled(move||ui.pending.get() || draft.with(|d|d.title.trim().is_empty())))
            .child(action("Links & activity",Variant::Quiet,move|_|related.set(!related.get_untracked()))))
        .child(dynamic(move||{let error=draft.with(|d|d.error.clone());let conflict=note_id.as_ref().is_some_and(|id|ui.snapshot.with(|s|s.notes.iter().find(|n|&n.id==id).is_some_and(|n|n.revision!=draft.get_untracked().revision))) && draft.with(|d|d.dirty());
            if error.is_empty() && !conflict{return view().into_element()}
            let selected=note_id.clone();view().gap_2().child(notice(if error.is_empty(){"The saved Note changed. Your draft is retained; compare the saved revision before replacing it.".into()}else{error},Tone::Attention))
                .child(action("View saved revision",Variant::Secondary,{let selected=selected.clone();move|_|{if let Some(id)=&selected && let Some(n)=ui.snapshot.with_untracked(|s|s.notes.iter().find(|n|&n.id==id).cloned()){load_text(ui,"note.revision",json!({"note_id":id,"revision":n.revision}));}}}))
                .child(action("Replace draft with saved version",Variant::Quiet,move|_|{if let Some(id)=&selected && let Some(n)=ui.snapshot.with_untracked(|s|s.notes.iter().find(|n|&n.id==id).cloned()){draft.set(Draft::note(&n));}})).into_element()
        }))
        .child(dynamic(move||if preview.get(){scroll().flex_1().child(nsh_base::text_view().markdown(move||Some(draft.get().body.into())).on_link_press(move|e:&nsh_base::TextViewLinkPressEvent|{if let Some(id)=e.url.strip_prefix("note:"){ui.open_note(id.into());}})).into_element()}else{
            view().flex_1().min_h(px(0.0)).gap_3()
                .child(bound_field(move||draft.get().title,move|s|draft.update(|d|d.title=s.into()),"Note name"))
                .child(bound_field(move||draft.get().body,move|s|draft.update(|d|d.body=s.into()),"Write Markdown…").multiline(true).flex_1().min_h(px(160.0))).into_element()
        }))
        .child(dynamic(move||{if ui.output.get().is_empty(){view().into_element()}else{scroll().max_h(px(180.0)).child(output_panel(ui)).into_element()}}))
}
pub fn relations(ui: Ui, note_id: Option<String>, draft: Signal<Draft>) -> impl IntoElement {
    let insert = signal(false);
    scroll().flex_1().min_h(px(0.0)).bg(INSET).child(
        view()
            .p_4()
            .gap_4()
            .child(
                view()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .child(label("Related").font_weight(FontWeight::SEMIBOLD))
                    .child(action("Insert link", Variant::Quiet, move |_| {
                        insert.set(!insert.get_untracked())
                    })),
            )
            .child(dynamic(move || {
                if !insert.get() {
                    return view().into_element();
                }
                let selected = ui.note.get();
                let project = ui.project.get();
                view()
                    .gap_1()
                    .children(ui.snapshot.with(|s| {
                        s.notes
                            .iter()
                            .filter(|n| n.id != selected && n.project_id == project)
                            .map(|n| {
                                let link =
                                    format!("[{}](note:{})", n.title.replace(['[', ']'], ""), n.id);
                                action(n.title.clone(), Variant::Secondary, move |_| {
                                    draft.update(|d| {
                                        if !d.body.is_empty() {
                                            d.body.push_str("\n\n");
                                        }
                                        d.body.push_str(&link);
                                    });
                                    insert.set(false);
                                })
                                .into_element()
                            })
                            .collect::<Vec<_>>()
                    }))
                    .into_element()
            }))
            .child(dynamic(move || {
                let s = ui.snapshot.get();
                let selected = note_id.as_deref().unwrap_or("");
                let Some(n) = s.notes.iter().find(|n| n.id == selected) else {
                    return muted("Save this Note to retain links, revisions and activity.")
                        .into_element();
                };
                let mut area = view().gap_3();
                for (title, ids) in [
                    ("Links", n.links.clone()),
                    (
                        "Backlinks",
                        s.notes
                            .iter()
                            .filter(|other| other.links.contains(&n.id))
                            .map(|n| n.id.clone())
                            .collect(),
                    ),
                ] {
                    let mut links = view().gap_1().child(muted(title));
                    if ids.is_empty() {
                        links = links.child(muted("None"));
                    }
                    for id in ids {
                        if let Some(other) = s.notes.iter().find(|n| n.id == id) {
                            links = links.child(action(
                                other.title.clone(),
                                Variant::Quiet,
                                move |_| ui.open_note(id.clone()),
                            ));
                        } else {
                            links = links.child(status("Missing Note", Tone::Attention));
                        }
                    }
                    area = area.child(links);
                }
                let id = n.id.clone();
                area = area.child(label("Related Cards").font_weight(FontWeight::SEMIBOLD));
                for c in s.cards.iter().filter(|c| n.card_ids.contains(&c.id)) {
                    let card = c.id.clone();
                    area = area.child(action(
                        format!("#{} {}", c.number, c.title),
                        Variant::Quiet,
                        move |_| ui.open_card(card.clone()),
                    ));
                }
                area = area.child(
                    action("Create Card from Note", Variant::Secondary, move |_| {
                        ui.source_note.set(id.clone());
                        ui.page.set("new-card".into());
                    })
                    .disabled(move || draft.with(|d| d.dirty())),
                );
                area = area.child(label("Timeline").font_weight(FontWeight::SEMIBOLD));
                let events = s
                    .note_events
                    .iter()
                    .rev()
                    .filter(|e| e.note_id == selected)
                    .collect::<Vec<_>>();
                if events.is_empty() {
                    area = area.child(muted("No recorded changes yet"));
                }
                for e in events {
                    let mut row = view()
                        .gap_2()
                        .py_3()
                        .border_b_1()
                        .border_color(BORDER)
                        .child(activity_header(e.actor.clone(), &e.kind, e.created_at))
                        .child(label(e.message.clone()));
                    let id = e.note_id.clone();
                    let revision = e.revision;
                    row = row.child(action(
                        format!("View revision {revision}"),
                        Variant::Quiet,
                        move |_| {
                            load_text(
                                ui,
                                "note.revision",
                                json!({"note_id":id,"revision":revision}),
                            )
                        },
                    ));
                    if let Some(id) = &e.card_id {
                        let id = id.clone();
                        row = row.child(action("Open related Card", Variant::Quiet, move |_| {
                            ui.open_card(id.clone())
                        }));
                    }
                    if let (Some(request), Some(card)) = (&e.request_id, &e.card_id) {
                        let request = request.clone();
                        let card = card.clone();
                        row =
                            row.child(action("View Agent execution", Variant::Quiet, move |_| {
                                ui.open_card(card.clone());
                                ui.request.set(request.clone());
                                ui.page.set("terminal".into());
                            }));
                    }
                    if let Some(source) = e.data["source_note_id"].as_str() {
                        let id = source.to_owned();
                        row = row.child(action("Open source Note", Variant::Quiet, move |_| {
                            ui.open_note(id.clone())
                        }));
                    }
                    area = area.child(row);
                }
                area.into_element()
            })),
    )
}
