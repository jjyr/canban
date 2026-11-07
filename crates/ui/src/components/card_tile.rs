use super::*;
pub fn card_tile(
    ui: Ui,
    card: Card,
    column: usize,
    width: Signal<f32>,
    stages: Vec<String>,
) -> impl IntoElement {
    let id = card.id.clone();
    let open_id = id.clone();
    let open_id_for_style = id.clone();
    let x = animated_value(0.0);
    let y = animated_value(0.0);
    let dragged = signal(false);
    let snapshot = ui.snapshot.get_untracked();
    let (summary, tone) = crate::board::activity(&snapshot, &card);
    panel()
        .key(id.clone())
        .gap_2()
        .label(format!("Card #{} {}", card.number, card.title))
        .focusable(true)
        .opacity(move || if dragged.get() { 0.65_f32 } else { 1.0_f32 })
        .border_1()
        .border_color(move || {
            if ui.card.get() == open_id_for_style {
                ACCENT
            } else {
                BORDER
            }
        })
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
                .child(muted(if card.stage == "review" { "Review" } else { "" })),
        )
        .child(label(card.title.clone()).font_weight(FontWeight::SEMIBOLD))
        .child(muted(
            card.body
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with('#'))
                .unwrap_or("")
                .chars()
                .take(100)
                .collect::<String>(),
        ))
        .child(status(summary, tone))
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
