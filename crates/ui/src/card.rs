//! Reading-first Card detail, shared by Board and focused work.
use crate::{components::*, drafts::*, presenter::*};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;
pub fn screen(ui: Ui) -> impl IntoElement {
    let Some(card) = ui.current_card() else {
        return view().child(muted("Select a Card")).into_element();
    };
    let id = card.id.clone();
    let key = format!("card:{id}");
    let draft = document(ui, key.clone(), Draft::card(&card));
    effect(move || {
        if let Some(card) = ui.current_card() {
            let mut value = draft.get();
            value.reconcile(Draft::card(&card));
            if value != draft.get_untracked() {
                draft.set(value);
            }
        }
    });
    let editing = computed(move || draft.with(|d| d.editing));
    let details = computed(move || draft.with(|d| d.details));
    let scroll_key = format!("card:{id}:content");
    let position = scroll_position(ui, &scroll_key);
    let body_proxy = signal_proxy_body(draft);
    let comment_proxy = signal_proxy_comment(draft);
    let paste_body = id.clone();
    let paste_comment = id.clone();
    let card_for_save = id.clone();
    let card_for_comment = id.clone();
    let card_context = id.clone();
    let card_archive = id.clone();
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .bg(SURFACE)
        .p_4()
        .gap_3()
        .child(
            view()
                .flex_row()
                .justify_between()
                .items_center()
                .gap_2()
                .child(dynamic(move || {
                    if (ui.narrow.get() || ui.return_overview.get() || ui.terminal_card_return.get()) && ui.page.get() == "card" {
                        action(if ui.terminal_card_return.get_untracked() {"← Terminals"} else if ui.return_overview.get_untracked() {"← Overview"} else {"← Board"}, Variant::Quiet, move |_| {
                            if ui.terminal_card_return.get_untracked() {ui.return_terminal();} else if ui.return_overview.get_untracked() { ui.back_board(); } else { ui.page.set("board".into()); }
                        })
                        .into_element()
                    } else {
                        muted("Card details").into_element()
                    }
                }))
                .child(action("Close Card", Variant::Quiet, move |_| {
                    if ui.terminal_card_return.get_untracked() {ui.return_terminal();} else if ui.return_overview.get_untracked() { ui.back_board(); } else { ui.card.set(String::new()); ui.page.set("board".into()); }
                })),
        )
        .child(heading(move || {
            ui.current_card()
                .map(|c| format!("#{} {}", c.number, c.title))
                .unwrap_or_default()
        }))
        .child(dynamic(move || {
            let Some(c) = ui.current_card() else {
                return view().into_element();
            };
            let s = ui.snapshot.get();
            let (state, tone) = crate::board::activity(&s, &c);
            view()
                .flex_row()
                .flex_wrap()
                .gap_2()
                .child(badge(c.stage))
                .child(status(state, tone))
                .into_element()
        }))
        .child(crate::components::git_workspace::panel(ui))
        .child(dynamic(move || {
            ui.current_card()
                .and_then(|c| c.attention)
                .map(|message| notice(message, Tone::Attention).into_element())
                .unwrap_or_else(|| view().into_element())
        }))
        .child(dynamic(move || {
            let Some(c) = ui.current_card() else { return view().into_element(); };
            if c.human_gate.is_none() { return view().into_element(); }
            view().gap_2()
                .child(muted("Automatic work is paused. Review the report, answer the Agent, or choose a stage."))
                .child(action("Reviewed · continue workflow", Variant::Primary, move |_| {
                    if let Some(c) = ui.current_card() { dispatch(ui, "card.continue", json!({"card_id":c.id,"revision":c.revision})); }
                }).disabled(move || ui.pending.get()))
                .into_element()
        }))
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.0))
                .content_offset(position)
                .on_scroll(move |e: &ScrollEvent| capture_scroll(ui, scroll_key.clone(), e))
                .child(
                    view()
                        .gap_4()
                        .child(
                            view()
                                .flex_row()
                                .justify_between()
                                .items_center()
                                .child(label("Task").font_weight(FontWeight::SEMIBOLD))
                                .child(action(move_label(&card), Variant::Quiet, move |_| {
                                    draft.update(|d| d.editing = !d.editing)
                                })),
                        )
                        .child(dynamic(move || {
                            if editing.get() {
                                let save_id = card_for_save.clone();
                                view()
                                    .gap_3()
                                    .child(bound_field(
                                        move || draft.get().title,
                                        move |s| draft.update(|d| d.title = s.into()),
                                        "Card title",
                                    ))
                                    .child(
                                        field(body_proxy, "Task description")
                                            .multiline(true)
                                            .h(px(180.0))
                                            .on_paste({
                                                let id = paste_body.clone();
                                                move |e: &nsh_core::PasteEvent| {
                                                    paste_resources(
                                                        ui,
                                                        id.clone(),
                                                        e,
                                                        body_proxy,
                                                        false,
                                                    )
                                                }
                                            }),
                                    )
                                    .child(bound_field(
                                        move || draft.get().tags,
                                        move |s| draft.update(|d| d.tags = s.into()),
                                        "Routing tags",
                                    ))
                                    .child(
                                        view()
                                            .flex_row()
                                            .gap_2()
                                            .child(
                                                action("Save Card", Variant::Primary, move |_| {
                                                    save_card(ui, save_id.clone(), draft)
                                                })
                                                .disabled(move || ui.pending.get()),
                                            )
                                            .child(muted(move || {
                                                if draft.with(|d| d.dirty()) {
                                                    "Unsaved changes"
                                                } else {
                                                    "Saved"
                                                }
                                            })),
                                    )
                                    .into_element()
                            } else {
                                nsh_base::text_view()
                                    .markdown(move || Some(draft.get().body.into()))
                                    .on_link_press(move |e: &nsh_base::TextViewLinkPressEvent| {
                                        if let Some(id) = e.url.strip_prefix("note:") {
                                            ui.open_note(id.into());
                                        }
                                    })
                                    .into_element()
                            }
                        }))
                        .child(dynamic(move || {
                            let error = draft.with(|d| d.error.clone());
                            if error.is_empty() {
                                return view().into_element();
                            }
                            let c = ui.current_card();
                            view()
                                .gap_2()
                                .child(notice(error, Tone::Danger))
                                .child(action("Reload saved task", Variant::Secondary, move |_| {
                                    if let Some(c) = &c {
                                        let mut saved = Draft::card(c);
                                        saved.comment = draft.get_untracked().comment;
                                        draft.set(saved);
                                    }
                                }))
                                .into_element()
                        }))
                        .child(crate::sessions::card_agents(ui))
                        .child(dynamic(move || {
                            let c = ui.current_card();
                            let Some(c) = c else {
                                return view().into_element();
                            };
                            let mut area = view().gap_2();
                            if let Some(report) = ui.snapshot.with(|s| {
                                s.reports.iter().rev().find(|r| r.card_id == c.id).cloned()
                            }) {
                                area = area
                                    .child(label("Latest result").font_weight(FontWeight::SEMIBOLD))
                                    .child(label(report.message));
                                if let Some(path) = report.path {
                                    let card = report.card_id;
                                    area = area.child(action(
                                        "Read report",
                                        Variant::Quiet,
                                        move |_| {
                                            load_text(
                                                ui,
                                                "resource.read",
                                                json!({"card_id":card,"path":path}),
                                            )
                                        },
                                    ));
                                }
                            }
                            if ui.current_project().is_some_and(|p| p.workflow.stages.iter().any(|s| s.id == c.stage && s.require_human_review)) || c.stage == "review" {
                                area = area.child(action(
                                    "Review changes",
                                    Variant::Primary,
                                    move |_| {
                                        ui.review.set(None);
                                        ui.review_request
                                            .set(ui.review_request.get_untracked() + 1);
                                        ui.page.set("review".into());
                                        ui.expanded.set(false);
                                        ui.focused_card.set(false);
                                    },
                                ));
                            }
                            area.into_element()
                        }))
                        .child(action(
                            "Context, resources and stage actions",
                            Variant::Quiet,
                            move |_| draft.update(|d| d.details = !d.details),
                        ))
                        .child(dynamic(move || {
                            if !details.get() {
                                return view().into_element();
                            }
                            let mut area = view().gap_3().child(context_picker(ui));
                            if let Some(p) = ui.current_project() {
                                area = area.child(label("Move Card")).child(
                                    view().flex_row().flex_wrap().gap_2().children(
                                        p.workflow
                                            .stages
                                            .into_iter()
                                            .map(|stage| {
                                                let id = card_context.clone();
                                                let title = format!("Move to {}", stage.name);
                                                action(title, Variant::Secondary, move |_| {
                                                    move_card(ui, &id, &stage.id)
                                                })
                                                .into_element()
                                            })
                                            .collect::<Vec<_>>(),
                                    ),
                                );
                            }
                            let path = signal(String::new());
                            let id = card_context.clone();
                            area = area.child(field(path, "File path to attach")).child(action(
                                "Attach file",
                                Variant::Secondary,
                                move |_| {
                                    let path = path.get_untracked();
                                    let name = std::path::Path::new(&path)
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_default();
                                    dispatch(
                                        ui,
                                        "attachment.add",
                                        json!({"card_id":id,"source":path,"name":name}),
                                    );
                                },
                            ));
                            area = area.child(dynamic(move || {
                                let id = ui.card.get();
                                view().gap_1().children(ui.snapshot.with(|s| {
                                    s.artifacts
                                        .iter()
                                        .filter(|a| a.card_id == id)
                                        .map(|a| {
                                            let card = a.card_id.clone();
                                            let path = a.path.clone();
                                            action(a.name.clone(), Variant::Quiet, move |_| {
                                                open_resource(ui, card.clone(), path.clone())
                                            })
                                            .into_element()
                                        })
                                        .collect::<Vec<_>>()
                                }))
                            }));
                            let archive = card_archive.clone();
                            area.child(action(
                                if card.archived_at.is_some() {
                                    "Restore Card"
                                } else {
                                    "Archive Card"
                                },
                                Variant::Danger,
                                move |_| {
                                    if let Some(c) = ui.current_card() {
                                        dispatch(
                                            ui,
                                            if c.archived_at.is_some() {
                                                "card.restore"
                                            } else {
                                                "card.archive"
                                            },
                                            json!({"card_id":archive,"revision":c.revision}),
                                        );
                                    }
                                },
                            ))
                            .into_element()
                        }))
                        .child(dynamic(move || {
                            if ui.output.get().is_empty() {
                                view().into_element()
                            } else {
                                output_panel(ui).into_element()
                            }
                        }))
                        .child(
                            view()
                                .gap_1()
                                .child(label("Timeline").font_weight(FontWeight::SEMIBOLD))
                                .child(dynamic(move || {
                                    let card = ui.card.get();
                                    view().children(ui.snapshot.with(|s| {
                                        s.events
                                            .iter()
                                            .filter(|e| e.card_id == card)
                                            .cloned()
                                            .map(|e| timeline_item(ui, e).into_element())
                                            .collect::<Vec<_>>()
                                    }))
                                })),
                        ),
                ),
        )
        .child(
            view()
                .border_t_1()
                .border_color(BORDER)
                .pt_3()
                .gap_2()
                .child(crate::profiles::mention_picker(ui.profiles, comment_proxy))
                .child(
                    field(comment_proxy, "Comment or @profile…")
                        .multiline(true)
                        .h(px(72.0))
                        .on_paste(move |e: &nsh_core::PasteEvent| {
                            paste_resources(ui, paste_comment.clone(), e, comment_proxy, true)
                        }),
                )
                .child(
                    view()
                        .flex_row()
                        .justify_between()
                        .items_center()
                        .child(muted("@mentions append to the selected Agent"))
                        .child(
                            action("Send", Variant::Primary, move |_| {
                                let body = draft.get_untracked().comment;
                                let id = card_for_comment.clone();
                                let key = key.clone();
                                dispatch_complete(
                                    ui,
                                    "comment.add",
                                    json!({"card_id":id,"body":body}),
                                    move |result| {
                                        let mut value = draft.try_get().unwrap_or_else(|| {
                                            ui.drafts
                                                .with(|m| m.get(&key).cloned().unwrap_or_default())
                                        });
                                        match result {
                                            Ok(_) => {
                                                if value.comment == body {
                                                    value.comment.clear();
                                                }
                                                value.error.clear();
                                            }
                                            Err(e) => value.error = e,
                                        }
                                        ui.drafts.with(|m| {
                                            m.insert(key, value.clone());
                                        });
                                        draft.try_set(value);
                                    },
                                );
                            })
                            .disabled(move || {
                                ui.pending.get() || draft.with(|d| d.comment.trim().is_empty())
                            }),
                        ),
                ),
        )
        .into_element()
}
fn move_label(_card: &canban_types::Card) -> &'static str {
    "Edit / Read"
}
fn signal_proxy_comment(draft: Signal<Draft>) -> Signal<String> {
    let text = signal(draft.get_untracked().comment);
    effect(move || {
        let value = text.get();
        if value != draft.get_untracked().comment {
            draft.update(|d| d.comment = value);
        }
    });
    effect(move || {
        let value = draft.get().comment;
        if text.get_untracked() != value {
            text.set(value);
        }
    });
    text
}

fn signal_proxy_body(draft: Signal<Draft>) -> Signal<String> {
    let text = signal(draft.get_untracked().body);
    effect(move || {
        let value = text.get();
        if value != draft.get_untracked().body {
            draft.update(|d| d.body = value);
        }
    });
    effect(move || {
        let value = draft.get().body;
        if text.get_untracked() != value {
            text.set(value);
        }
    });
    text
}
