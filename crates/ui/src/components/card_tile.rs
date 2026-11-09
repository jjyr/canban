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
    let mut tile = panel()
        .p_3()
        .min_w(px(0.))
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
                .items_center()
                .child(if let Some(source) = card.github_issue() {
                    view()
                        .flex_row()
                        .gap_1()
                        .items_center()
                        .tooltip(nsh_base::tooltip().child(label(format!(
                            "Card #{} · {}#{}",
                            card.number, source.repository, source.number
                        ))))
                        .child(
                            canban_ui_kit::icon(canban_ui_kit::Icon::GitHub)
                                .w(px(14.))
                                .h(px(14.)),
                        )
                        .child(muted(format!(
                            "#{} · Issue #{}",
                            card.number, source.number
                        )))
                        .into_element()
                } else {
                    muted(format!("#{}", card.number)).into_element()
                })
                .child(
                    view()
                        .w(px(24.))
                        .h(px(24.))
                        .flex_shrink_0()
                        .items_center()
                        .rounded(px(4.))
                        .hover(|s| s.bg(INSET))
                        .justify_center()
                        .label("Drag Card to another stage")
                        .pan_values(Some(&x), Some(&y))
                        .on_pan_start(move |_: &PanEvent| dragged.set(true))
                        .on_pan_end(move |e: &PanEvent| {
                            if !e.cancelled {
                                let target = (column as i32
                                    + (e.translation.x.0 / width.get_untracked().max(1.0)).round()
                                        as i32)
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
                        .child(icon(Icon::Grip).w(px(14.)).h(px(14.))),
                ),
        )
        .child(
            label(card.title.clone())
                .font_weight(FontWeight::SEMIBOLD)
                .min_w(px(0.)),
        );
    let excerpt = card
        .body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("");
    if !excerpt.is_empty() {
        tile = tile.child(muted(excerpt.to_owned()).truncate().min_w(px(0.)));
    }
    if let Some(activity) = crate::components::activity::card_activity(&snapshot, &card) {
        tile = tile.child(activity);
    }
    if let Some(branch) = card.branch.as_ref().filter(|b| !b.is_empty()) {
        tile = tile.child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .min_w(px(0.))
                .tooltip(nsh_base::tooltip().child(label(branch.clone())))
                .child(icon(Icon::Branch).w(px(12.)).h(px(12.)))
                .child(
                    muted(crate::components::git_workspace::compact(branch, 28))
                        .truncate()
                        .min_w(px(0.)),
                ),
        );
    }
    if !card.tags.is_empty() {
        tile = tile.child(
            view().flex_row().flex_wrap().gap_1().children(
                card.tags
                    .iter()
                    .map(|tag| badge(format!("@{tag}")).into_element())
                    .collect::<Vec<_>>(),
            ),
        );
    }
    tile
}
