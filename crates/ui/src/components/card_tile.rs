use super::*;
/// Card content/surface. Board owns pointer capture, drag geometry and activation.
pub fn card_tile(ui: Ui, card: Card) -> View {
    let reminder_id = card.id.clone();
    let count = computed(move || {
        ui.reminder_counts
            .with(|all| all.get(&reminder_id).copied().unwrap_or_default())
    });
    let selected_id = card.id.clone();
    let snapshot = ui.snapshot.get_untracked();
    let mut tile = panel()
        .p_3()
        .min_w(px(0.))
        .gap_2()
        .border_1()
        .border_color(BORDER)
        .bg(move || {
            if ui.card.get() == selected_id {
                SELECTED.resolve()
            } else {
                SURFACE.resolve()
            }
        })
        .hover(|s| s.bg(CARD_HOVER))
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
                        .child(icon(Icon::GitHub).w(px(14.)).h(px(14.)))
                        .child(muted(format!(
                            "#{} · {} #{}",
                            card.number,
                            if card.is_github_pr() { "PR" } else { "Issue" },
                            source.number
                        )))
                        .into_element()
                } else {
                    muted(format!("#{}", card.number)).into_element()
                })
                .child(
                    view()
                        .flex_row()
                        .gap_2()
                        .items_center()
                        .child(reminder_count(
                            move || count.get().total,
                            move || count.get().pending > 0,
                        ))
                        .child(
                            icon(Icon::Grip)
                                .w(px(14.))
                                .h(px(14.))
                                .pointer_events(PointerEvents::None),
                        ),
                ),
        )
        .child(
            label(card.title.clone())
                .font_weight(FontWeight::SEMIBOLD)
                .min_w(px(0.)),
        );
    if let Some(source) = card.github_issue()
        && let Some(cached) = snapshot.github_issues.iter().find(|v| {
            v.project_id == card.project_id
                && v.source.repository_id == source.repository_id
                && v.source.issue_id == source.issue_id
        })
    {
        tile = tile.child(
            view()
                .flex_row()
                .gap_1()
                .child(badge(cached.state.clone()))
                .child(muted(format!("by {}", cached.author_login))),
        );
    }
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
    tile
}
