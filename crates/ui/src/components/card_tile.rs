use super::*;
/// Card content/surface. Board owns pointer capture, drag geometry and activation.
///
/// Layout (rows appear only when they have content):
/// ```text
/// #16  Title wraps beside the number            (3) [bot]
/// GitHub Issue #12 · open · by author
/// First body line, truncated
/// Needs review  ⎇ branch            [((●)) ✦ @developer]
/// ```
/// The hover robot sends the Card to the default Agent; a running Agent pill
/// pulses and opens that Agent's live terminal.
pub fn card_tile(ui: Ui, card: Card) -> View {
    let reminder_id = card.id.clone();
    let count = computed(move || {
        ui.reminder_counts
            .with(|all| all.get(&reminder_id).copied().unwrap_or_default())
    });
    let selected_id = card.id.clone();
    let agent_card = card.id.clone();
    let snapshot = ui.snapshot.get_untracked();
    let hovered = signal(false);
    let active = card.active();
    let mut tile = panel()
        .px_3()
        .py_2p5()
        .min_w(px(0.))
        .gap_1p5()
        .border_1()
        .border_color(BORDER)
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
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
                .items_start()
                .gap_2()
                .min_w(px(0.))
                .child(
                    view()
                        .flex_row()
                        .flex_1()
                        .min_w(px(0.))
                        .gap_1p5()
                        .items_start()
                        .pt_0p5()
                        .child(muted(format!("#{}", card.number)).flex_shrink_0())
                        .child(
                            label(card.title.clone())
                                .font_weight(FontWeight::SEMIBOLD)
                                .flex_1()
                                .min_w(px(0.)),
                        ),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_shrink_0()
                        .gap_1()
                        .items_center()
                        .h(px(22.))
                        .child(reminder_count(
                            move || count.get().total,
                            move || count.get().pending > 0,
                        ))
                        // Space stays reserved so hovering never rewraps the title.
                        .child(when(
                            move || active,
                            move || {
                                send_button(ui, agent_card.clone())
                                    .opacity(move || if hovered.get() { 1.0_f32 } else { 0.0_f32 })
                            },
                        )),
                ),
        );
    if let Some(source) = card.github_issue() {
        let cached = snapshot.github_issues.iter().find(|v| {
            v.project_id == card.project_id
                && v.source.repository_id == source.repository_id
                && v.source.issue_id == source.issue_id
        });
        let mut meta = view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .min_w(px(0.))
            .tooltip(nsh_base::tooltip().child(label(format!(
                "Card #{} · {}#{}",
                card.number, source.repository, source.number
            ))))
            .child(icon(Icon::GitHub).w(px(12.)).h(px(12.)))
            .child(muted(format!(
                "{} #{}",
                if card.is_github_pr() { "PR" } else { "Issue" },
                source.number
            )));
        if let Some(cached) = cached {
            meta = meta
                .child(badge(cached.state.clone()))
                .child(muted(format!("by {}", cached.author_login)).truncate());
        }
        tile = tile.child(meta);
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
    let chips = crate::components::activity::card_status(&snapshot, &card);
    let running = crate::components::activity::running_agents(&snapshot, &card);
    let branch = card.branch.clone().filter(|b| !b.is_empty());
    if !chips.is_empty() || !running.is_empty() || branch.is_some() {
        let mut left = view()
            .flex_row()
            .flex_wrap()
            .flex_1()
            .items_center()
            .gap_1p5()
            .min_w(px(0.))
            .children(chips);
        if let Some(branch) = branch {
            left = left.child(
                view()
                    .flex_row()
                    .flex_shrink(1.)
                    .items_center()
                    .gap_1()
                    .min_w(px(0.))
                    .tooltip(nsh_base::tooltip().child(label(branch.clone())))
                    .child(icon(Icon::Branch).w(px(12.)).h(px(12.)))
                    .child(
                        muted(crate::components::git_workspace::compact(&branch, 24))
                            .truncate()
                            .min_w(px(0.)),
                    ),
            );
        }
        tile = tile.child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_1p5()
                .min_w(px(0.))
                .child(left)
                .children(
                    running
                        .into_iter()
                        .map(|request| agent_pill(ui, request).into_element())
                        .collect::<Vec<_>>(),
                ),
        );
    }
    tile
}

/// Hover entry: open the default Agent's conversation with this Card.
fn send_button(ui: Ui, card: String) -> View {
    chrome_button(Icon::Agent, "Send to Agent", move |_| {
        crate::sessions::send_card(ui, &card)
    })
    .w(px(22.))
    .h(px(22.))
}

/// A running Agent: pulsing mark + provider icon + profile. Opens its terminal.
fn agent_pill(ui: Ui, request: Request) -> impl IntoElement {
    let (mark, provider) = crate::profiles::provider(&request.profile);
    let profile = request.profile.id.clone();
    let hint = format!("{provider} · @{profile} is working — open terminal");
    let a11y = hint.clone();
    let id = request.id.clone();
    view()
        .flex_shrink_0()
        .tooltip(nsh_base::tooltip().child(label(hint)))
        .child(
            nsh_base::button()
                .h(px(22.))
                .px_2()
                .flex_row()
                .items_center()
                .gap_1p5()
                .rounded(px(11.))
                .bg(SELECTED)
                .border_1()
                .border_color(ACCENT)
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                .accessibility_label(move || Some(a11y.clone().into()))
                .child(live_dot(ACCENT))
                .child(icon(mark).w(px(12.)).h(px(12.)))
                .child(
                    text(format!("@{profile}"))
                        .text_size(px(CAPTION_SIZE))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ACCENT)
                        .truncate(),
                )
                .on_press(move |_: &()| crate::terminals::open_request(ui, &id)),
        )
}
