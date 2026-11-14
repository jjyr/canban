use super::*;
/// Card content/surface. Board owns pointer capture, drag geometry and activation.
///
/// Layout (rows appear only when they have content):
/// ```text
/// #16  Title wraps beside the number            (3) [>_]
/// GitHub Issue #12 · open · by author
/// First body line, truncated
/// [#search] [#ux] [#agent] +1
/// Needs review  ⎇ branch            [((●)) ✦ @developer]
/// ```
/// The `[>_]` button opens this Card's quick terminal split directly on the Card page
/// (pinned with an active dot when a session is running, or revealed on hover); active
/// Agent pills pulse (or flag attention) and open that Agent tab on the Card.
pub fn card_tile(ui: Ui, card: Card) -> View {
    let reminder_id = card.id.clone();
    let count = computed(move || {
        ui.reminder_counts
            .with(|all| all.get(&reminder_id).copied().unwrap_or_default())
    });
    let selected_id = card.id.clone();
    let terminal_card = card.id.clone();
    let card_number = card.number;
    let archive_card = card.clone();
    let snapshot = ui.snapshot.get_untracked();
    let live_entries = crate::terminals::card_live_entries(&snapshot, &card.id);
    let has_terminal = !live_entries.is_empty();
    let dot_tone = if !has_terminal {
        None
    } else if live_entries.iter().any(|e| e.needs_attention()) {
        Some(ATTENTION)
    } else {
        Some(ACCENT)
    };
    let hovered = signal(false);
    let active = card.active();
    let merged = card.merged();
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
            } else if merged {
                INSET.resolve()
            } else {
                SURFACE.resolve()
            }
        })
        .hover(|s| s.bg(CARD_HOVER));
    if merged {
        tile = tile.border_dashed().opacity(0.72);
    }
    tile = tile.child(
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
                        text(card.title.clone())
                            .text_size(px(BODY_SIZE))
                            .text_color(if merged { MUTED } else { TEXT })
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
                    .child(when(
                        move || active && merged,
                        move || archive_button(ui, archive_card.clone()),
                    ))
                    // Space stays reserved so hovering never rewraps the title.
                    .child(when(
                        move || active || has_terminal,
                        move || {
                            terminal_button(ui, terminal_card.clone(), card_number, dot_tone)
                                .opacity(move || {
                                    if has_terminal || hovered.get() {
                                        1.0_f32
                                    } else {
                                        0.0_f32
                                    }
                                })
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
            .tooltip(hint_tooltip(format!(
                "Card #{} · {}#{}",
                card.number, source.repository, source.number
            )))
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
    let local_tags = ui.local_tags.get_untracked();
    let known_tags = crate::tags::known_map(&snapshot, &local_tags, Some(&card.project_id));
    let tags = crate::tags::card_tags(&card, &known_tags);
    if !tags.is_empty() {
        let active_tags = ui.github.filters.with_untracked(|all| {
            all.get(&card.project_id)
                .map(|f| f.tags.clone())
                .unwrap_or_default()
        });
        let mut tag_row = view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .min_w(px(0.));
        for tag in tags.iter().take(crate::tags::MAX_BOARD_TILE_TAGS) {
            let is_active = active_tags
                .iter()
                .any(|t| t.to_lowercase() == tag.to_lowercase());
            tag_row = tag_row.child(tag_pill(ui, &card.project_id, tag, is_active));
        }
        let extra = tags.len().saturating_sub(crate::tags::MAX_BOARD_TILE_TAGS);
        if extra > 0 {
            let all_hint = tags
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" ");
            tag_row = tag_row.child(
                view()
                    .h(px(20.))
                    .px_1p5()
                    .flex_row()
                    .items_center()
                    .rounded(px(5.))
                    .bg(INSET)
                    .tooltip(hint_tooltip(all_hint))
                    .child(muted(format!("+{extra}")).text_size(px(11.))),
            );
        }
        tile = tile.child(tag_row);
    }
    let ui_merging = ui
        .merging_cards
        .with_untracked(|ids| ids.contains(&card.id));
    let chips = crate::components::activity::card_status_with_merging(&snapshot, &card, ui_merging);
    let running = crate::components::activity::running_agents(&snapshot, &card);
    let ephemeral_agents: Vec<_> = live_entries
        .into_iter()
        .filter(|e| e.agent.is_none() && e.saved.agent_engine.is_some())
        .collect();
    let branch = card.branch.clone().filter(|b| !b.is_empty());
    if !chips.is_empty() || !running.is_empty() || !ephemeral_agents.is_empty() || branch.is_some()
    {
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
                    .tooltip(hint_tooltip(branch.clone()))
                    .child(icon(Icon::Branch).w(px(12.)).h(px(12.)))
                    .child(
                        muted(crate::components::git_workspace::compact(&branch, 24))
                            .truncate()
                            .min_w(px(0.)),
                    ),
            );
        }
        let mut pills: Vec<AnyElement> = running
            .into_iter()
            .map(|request| agent_pill(ui, request).into_element())
            .collect();
        pills.extend(
            ephemeral_agents
                .into_iter()
                .map(|entry| ephemeral_agent_pill(ui, &card.id, entry).into_element()),
        );
        tile = tile.child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_1p5()
                .min_w(px(0.))
                .child(left)
                .children(pills),
        );
    }
    tile
}

/// Clickable tag chip on a Board Card: toggles Board filtering by `#tag` without opening the Card.
fn tag_pill(ui: Ui, project_id: &str, tag: &str, is_active: bool) -> impl IntoElement {
    let pid = project_id.to_owned();
    let tag_value = tag.to_owned();
    let label_text = format!("#{tag}");
    let hint = if is_active {
        format!("Clear #{tag} Board filter")
    } else {
        format!("Filter Board by #{tag}")
    };
    let a11y = hint.clone();
    view().flex_shrink_0().tooltip(hint_tooltip(hint)).child(
        press_target()
            .h(px(20.))
            .px_1p5()
            .max_w(px(140.))
            .flex_row()
            .items_center()
            .rounded(px(5.))
            .bg(if is_active { SELECTED } else { INSET })
            .border_1()
            .border_color(if is_active { ACCENT } else { BORDER })
            .hover(|s| s.bg(CARD_HOVER).border_color(ACCENT))
            .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
            .accessibility_label(move || Some(a11y.clone().into()))
            .child(
                text(label_text)
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if is_active { ACCENT } else { MUTED })
                    .truncate(),
            )
            .on_press(move |_: &()| crate::tags::toggle_board_filter(ui, &pid, &tag_value)),
    )
}

/// Quick action on merged Cards: archive immediately without waiting for tomorrow.
fn archive_button(ui: Ui, card: Card) -> View {
    chrome_button_disabled(
        Icon::Archive,
        "Archive Card",
        move || ui.pending.get(),
        move |_| crate::card_actions::apply(ui, card.clone(), crate::card_actions::Action::Archive),
    )
    .w(px(22.))
    .h(px(22.))
}

/// Quick or live Terminal entry: open this Card's side-by-side terminal split directly.
fn terminal_button(ui: Ui, card: String, number: u64, dot_tone: Option<ThemeColor>) -> View {
    let title = format!("Open Terminal for Card #{number}");
    let hint = title.clone();
    let mut btn = press_target()
        .relative()
        .w(px(22.))
        .h(px(22.))
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .bg(CLEAR)
        .hover(|s| s.bg(INSET))
        .focus(|s| s.border_1().border_color(ACCENT))
        .accessibility_label(move || Some(title.clone().into()))
        .child(icon(Icon::Terminal).w(px(15.)).h(px(15.)))
        .on_press(move |_: &()| crate::terminals::open_card_quick(ui, &card));
    if let Some(color) = dot_tone {
        btn = btn.child(
            view()
                .absolute()
                .top(px(2.))
                .right(px(2.))
                .w(px(5.))
                .h(px(5.))
                .rounded_full()
                .bg(color)
                .pointer_events(PointerEvents::None),
        );
    }
    view()
        .flex_shrink_0()
        .w(px(22.))
        .h(px(22.))
        .window_drag(false)
        .tooltip(hint_tooltip(hint).placement(Placement::Bottom))
        .child(btn)
}

/// A running workflow/profile Agent: pulsing mark + provider icon + profile. Opens Card + Agent terminal.
fn agent_pill(ui: Ui, entry: TerminalEntry) -> impl IntoElement {
    let agent = entry
        .agent
        .as_ref()
        .expect("agent_pill requires an Agent terminal");
    let (mark, provider) = crate::profiles::provider(&agent.profile);
    let profile = agent.profile.id.clone();
    let hint = format!("{provider} · @{profile} is working — open in Card");
    let a11y = hint.clone();
    let id = entry.id.clone();
    view().flex_shrink_0().tooltip(hint_tooltip(hint)).child(
        press_target()
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

/// A live interactive CLI Agent spawned on this Card: opens the Card with that Agent tab selected.
fn ephemeral_agent_pill(ui: Ui, card_id: &str, entry: crate::terminals::Entry) -> impl IntoElement {
    let (mark, provider) = entry
        .provider()
        .unwrap_or_else(|| (Icon::Agent, "Agent".into()));
    let title = entry.saved.name.clone().unwrap_or_else(|| provider.clone());
    let attention = entry.attention_reason();
    let hint = match &attention {
        Some(reason) => format!("{provider} · {reason} — open in Card"),
        None => format!("{provider} is active — open in Card"),
    };
    let a11y = hint.clone();
    let tone_color = if attention.is_some() {
        ATTENTION
    } else {
        ACCENT
    };
    let cid = card_id.to_owned();
    let tid = entry.saved.id;
    view().flex_shrink_0().tooltip(hint_tooltip(hint)).child(
        press_target()
            .h(px(22.))
            .px_2()
            .flex_row()
            .items_center()
            .gap_1p5()
            .rounded(px(11.))
            .bg(SELECTED)
            .border_1()
            .border_color(tone_color)
            .hover(|s| s.bg(CARD_HOVER))
            .focus(move |s| s.ring(px(1.)).ring_color(tone_color))
            .accessibility_label(move || Some(a11y.clone().into()))
            .child(if attention.is_some() {
                icon_tinted(Icon::Info, ATTENTION)
                    .w(px(12.))
                    .h(px(12.))
                    .into_element()
            } else {
                live_dot(ACCENT).into_element()
            })
            .child(icon(mark).w(px(12.)).h(px(12.)))
            .child(
                text(title)
                    .text_size(px(CAPTION_SIZE))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(tone_color)
                    .truncate(),
            )
            .on_press(move |_: &()| ui.open_card_with_terminal(cid.clone(), Some(tid.clone()))),
    )
}
