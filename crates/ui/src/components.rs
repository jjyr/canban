use crate::presenter::*;
use canban_types::Event;
use canban_types::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;
pub fn card_tile(
    ui: Ui,
    card: Card,
    column: usize,
    width: Signal<f32>,
    stages: Vec<String>,
) -> impl IntoElement {
    let id = card.id.clone();
    let open_id = id.clone();
    let x = animated_value(0.0);
    let y = animated_value(0.0);
    let dragged = signal(false);
    let summary = ui.snapshot.with_untracked(|s| {
        s.requests
            .iter()
            .rev()
            .find(|r| r.card_id == id)
            .map(|r| format!("@{} · {}", r.profile.id, r.state))
            .unwrap_or_else(|| "Not scheduled".into())
    });
    panel()
        .key(id.clone())
        .gap_2()
        .label(format!("Card #{} {}", card.number, card.title))
        .focusable(true)
        .opacity(move || if dragged.get() { 0.65_f32 } else { 1.0_f32 })
        .hover(|s| s.border_color(ACCENT))
        .focus(|s| s.border_color(ACCENT))
        .on_press(move || {
            if !dragged.get_untracked() {
                ui.open_card(open_id.clone());
            }
        })
        .child(
            view()
                .flex_row()
                .justify_between()
                .child(muted(format!("#{}", card.number)))
                .child(badge(if card.attention.is_some() {
                    "Needs attention"
                } else {
                    "Local"
                })),
        )
        .child(label(card.title.clone()).font_weight(FontWeight::SEMIBOLD))
        .child(muted(card.body.chars().take(100).collect::<String>()))
        .child(muted(summary))
        .child(
            view().flex_row().gap_1().children(
                card.tags
                    .iter()
                    .map(|tag| badge(format!("@{tag}")).into_element())
                    .collect::<Vec<_>>(),
            ),
        )
        .child(
            view()
                .h(px(18.0))
                .justify_center()
                .label("Drag Card to another stage")
                .pan_values(Some(&x), Some(&y))
                .on_pan_start(move |_: &PanEvent| dragged.set(true))
                .on_pan_end(move |e: &PanEvent| {
                    if !e.cancelled {
                        let target = (column as i32
                            + (e.translation.x.0 / width.get_untracked().max(1.0)).round() as i32)
                            .clamp(0, stages.len() as i32 - 1)
                            as usize;
                        if target != column {
                            move_card(ui, &id, &stages[target]);
                        }
                    }
                    x.set(0.0);
                    y.set(0.0);
                    set_timeout(std::time::Duration::from_millis(100), move || {
                        dragged.set(false)
                    });
                })
                .child(muted("⠿  drag to move")),
        )
}
pub fn timeline_item(ui: Ui, event: Event) -> impl IntoElement {
    let is_comment = event.kind == "comment.created";
    let req = event.request_id.clone();
    let mut row = view()
        .gap_1()
        .py_3()
        .border_b_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .justify_between()
                .child(muted(format!("{} · {}", event.sequence, event.actor)))
                .child(muted(event.kind.clone())),
        )
        .child(if is_comment {
            nsh_base::text_view()
                .markdown(event.message.clone())
                .into_element()
        } else {
            label(event.message).into_element()
        });
    if let Some(path) = event.data["path"].as_str() {
        let path = path.to_owned();
        let card = event.card_id.clone();
        row = row.child(action("Read report", Variant::Quiet, move |_| {
            load_text(ui, "resource.read", json!({"card_id":card,"path":path}))
        }));
    }
    if let Some(request) = req {
        let attempts = ui.snapshot.with_untracked(|s| {
            s.attempts
                .iter()
                .filter(|a| a.request_id == request)
                .cloned()
                .collect::<Vec<_>>()
        });
        if let Some(attempt) = attempts.last() {
            let id = attempt.id.clone();
            row = row.child(action("View output", Variant::Quiet, move |_| {
                load_text(ui, "execution.output", json!({"attempt_id":id}))
            }));
        }
    }
    row
}
pub fn run_panel(ui: Ui, req: Request) -> impl IntoElement {
    let run = ui.snapshot.with_untracked(|s| {
        s.attempts
            .iter()
            .rev()
            .find(|a| a.request_id == req.id)
            .cloned()
    });
    let mut panel = panel()
        .child(label(format!("@{} · {}", req.profile.id, req.state)))
        .child(muted(format!("{} · {}", req.stage, req.trigger)));
    let open_id = req.id.clone();
    panel = panel.child(action("Open terminal", Variant::Primary, move |_| {
        ui.request.set(open_id.clone());
        ui.page.set("terminal".into());
    }));
    if let Some(run) = run {
        let id = run.id.clone();
        panel = panel
            .child(muted(format!(
                "Attempt {} · {} · {}",
                run.number,
                run.terminal.as_deref().unwrap_or("no terminal"),
                run.cwd
            )))
            .child(action(
                "Read terminal output",
                Variant::Secondary,
                move |_| load_text(ui, "execution.output", json!({"attempt_id":id})),
            ));
        if let Some(error) = run.error {
            panel = panel.child(label(error).text_color(DANGER));
        }
    }
    if matches!(req.state.as_str(), "failed" | "blocked" | "needs_report") {
        let id = req.id.clone();
        panel = panel.child(action(
            "Retry with a fresh Session",
            Variant::Secondary,
            move |_| dispatch(ui, "execution.retry", json!({"request_id":id})),
        ));
    }
    panel
}
pub fn output_panel(ui: Ui) -> impl IntoElement {
    panel()
        .child(
            view()
                .flex_row()
                .justify_between()
                .child(label("Output / report"))
                .child(action("Clear", Variant::Quiet, move |_| {
                    ui.output_request.set(ui.output_request.get_untracked() + 1);
                    ui.output.set(String::new());
                })),
        )
        .child(nsh_base::text_view().markdown(move || Some(ui.output.get().into())))
}

pub fn context_picker(ui: Ui) -> impl IntoElement {
    panel().child(label("Human Context · Notes")).child(muted("Choose the Notes included in the next execution. Earlier Sessions keep their captured versions."))
        .child(dynamic(move||{let Some(card)=ui.current_card()else{return view().into_element()};let notes=ui.snapshot.with(|s|s.notes.iter().filter(|n|n.project_id==card.project_id).cloned().collect::<Vec<_>>());
            if notes.is_empty(){return muted("Add Project Notes to make them available here.").into_element()}
            view().gap_1().children(notes.into_iter().map(|note|{let note_id=note.id.clone();let checked=signal(card.context_notes.as_ref().is_none_or(|ids|ids.contains(&note.id)));
                checkbox(note.title,checked,move|value:&bool|{if let Some(card)=ui.current_card(){let mut selected=card.context_notes.unwrap_or_else(||ui.snapshot.with_untracked(|s|s.notes.iter().filter(|n|n.project_id==card.project_id).map(|n|n.id.clone()).collect()));selected.retain(|id|id!=&note_id);if *value{selected.push(note_id.clone());}dispatch(ui,"card.context.update",json!({"card_id":card.id,"revision":card.revision,"note_ids":selected}));}}).into_element()
            }).collect::<Vec<_>>()).into_element()
        }))
}
