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
/// (pinned with a highlighted icon when a session is running, or revealed on hover); active
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
    let terminal_tint = card_terminal_icon_tint(&live_entries, false);
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
                            terminal_button(ui, terminal_card.clone(), card_number, terminal_tint)
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
            .map(|request| {
                let started_at = crate::components::activity::agent_started_at(&snapshot, &request);
                agent_pill(ui, request, started_at).into_element()
            })
            .collect();
        pills.extend(ephemeral_agents.into_iter().map(|entry| {
            let started_at =
                (entry.saved.created_at > 1_000_000_000_000).then_some(entry.saved.created_at);
            ephemeral_agent_pill(ui, &card.id, entry, started_at).into_element()
        }));
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

/// Icon tint shared by Board Card tiles and Card header Terminal toggles:
/// highlights the `[>_]` icon directly when live sessions exist instead of adding a badge dot.
pub(crate) fn card_terminal_icon_tint(
    live_entries: &[crate::terminals::Entry],
    selected: bool,
) -> ThemeColor {
    if live_entries.iter().any(|e| e.needs_attention()) {
        ATTENTION
    } else if !live_entries.is_empty() || selected {
        ACCENT
    } else {
        MUTED
    }
}

/// Quick or live Terminal entry: open this Card's side-by-side terminal split directly.
fn terminal_button(ui: Ui, card: String, number: u64, tint: ThemeColor) -> View {
    let title = format!("Open Terminal for Card #{number}");
    let hint = title.clone();
    let btn = press_target()
        .w(px(22.))
        .h(px(22.))
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .bg(CLEAR)
        .hover(|s| s.bg(INSET))
        .focus(|s| s.border_1().border_color(ACCENT))
        .accessibility_label(move || Some(title.clone().into()))
        .child(icon_tinted(Icon::Terminal, tint).w(px(15.)).h(px(15.)))
        .on_press(move |_: &()| crate::terminals::open_card_quick(ui, &card));
    view()
        .flex_shrink_0()
        .w(px(22.))
        .h(px(22.))
        .window_drag(false)
        .tooltip(hint_tooltip(hint).placement(Placement::Bottom))
        .child(btn)
}

/// A running workflow/profile Agent: pulsing mark + provider icon + profile + compact elapsed runtime. Opens Card + Agent terminal.
fn agent_pill(ui: Ui, entry: TerminalEntry, started_at: Option<u64>) -> impl IntoElement {
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
                text(move || {
                    if let Some(ts) = started_at {
                        let elapsed = crate::components::activity::compact_duration(
                            ui.runtime_clock.get().saturating_sub(ts),
                        );
                        format!("@{profile} · {elapsed}")
                    } else {
                        format!("@{profile}")
                    }
                })
                .text_size(px(CAPTION_SIZE))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ACCENT)
                .truncate(),
            )
            .on_press(move |_: &()| crate::terminals::open_request(ui, &id)),
    )
}

/// A live interactive CLI Agent spawned on this Card: opens the Card with that Agent tab selected.
fn ephemeral_agent_pill(
    ui: Ui,
    card_id: &str,
    entry: crate::terminals::Entry,
    started_at: Option<u64>,
) -> impl IntoElement {
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
                text(move || {
                    if let Some(ts) = started_at {
                        let elapsed = crate::components::activity::compact_duration(
                            ui.runtime_clock.get().saturating_sub(ts),
                        );
                        format!("{title} · {elapsed}")
                    } else {
                        title.clone()
                    }
                })
                .text_size(px(CAPTION_SIZE))
                .font_weight(FontWeight::MEDIUM)
                .text_color(tone_color)
                .truncate(),
            )
            .on_press(move |_: &()| ui.open_card_with_terminal(cid.clone(), Some(tid.clone()))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_core::{ApproxTextEngine, Core, MountItem, NodeId, geometry::size};
    use serde_json::json;
    use std::collections::HashMap;

    fn sample_snapshot() -> Snapshot {
        let mut s = Snapshot::default();
        s.projects.push(
            serde_json::from_value(json!({
                "id": "p1",
                "name": "Canban",
                "directory": "/tmp/project",
                "workflow": Workflow::default(),
                "created_at": 1
            }))
            .unwrap(),
        );
        for (i, id) in [(1u64, "c-idle"), (2, "c-live"), (3, "c-attention")] {
            s.cards.push(
                serde_json::from_value(json!({
                    "id": id,
                    "project_id": "p1",
                    "number": i,
                    "title": format!("Card {i}"),
                    "body": "",
                    "stage": "develop",
                    "revision": 1,
                    "created_at": 1
                }))
                .unwrap(),
            );
        }
        s.terminals.push(TerminalEntry {
            id: "shell-live".into(),
            conversation_id: None,
            number: 1,
            name: None,
            manual_number: Some(1),
            origin_card_id: Some("c-live".into()),
            agent_engine: None,
            runtime: Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some("pty-live".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
            }),
            agent: None,
            created_at: 1,
        });
        s.terminals.push(TerminalEntry {
            id: "cli-attention".into(),
            conversation_id: None,
            number: 2,
            name: None,
            manual_number: Some(2),
            origin_card_id: Some("c-attention".into()),
            agent_engine: Some("agy".into()),
            runtime: Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: Some("core".into()),
                terminal: Some("pty-attention".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: Some("blocked".into()),
                agent_turn: Some(1),
            }),
            agent: None,
            created_at: 1,
        });
        s
    }

    #[test]
    fn card_terminal_icon_tint_highlights_active_and_attention_without_dot_badge() {
        let s = sample_snapshot();
        let idle = crate::terminals::card_live_entries(&s, "c-idle");
        let live = crate::terminals::card_live_entries(&s, "c-live");
        let attention = crate::terminals::card_live_entries(&s, "c-attention");

        assert_eq!(card_terminal_icon_tint(&idle, false), MUTED);
        assert_eq!(card_terminal_icon_tint(&idle, true), ACCENT);
        assert_eq!(card_terminal_icon_tint(&live, false), ACCENT);
        assert_eq!(card_terminal_icon_tint(&live, true), ACCENT);
        assert_eq!(card_terminal_icon_tint(&attention, false), ATTENTION);
        assert_eq!(card_terminal_icon_tint(&attention, true), ATTENTION);

        #[component]
        fn ProbeRoot() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let s = sample_snapshot();
            let c_idle = s.cards[0].clone();
            let c_live = s.cards[1].clone();
            let c_attn = s.cards[2].clone();
            ui.snapshot.set(s);
            ui.project.set("p1".into());
            ui.card.set("c-live".into());
            ui.page.set("card".into());
            view()
                .child(card_tile(ui, c_idle))
                .child(card_tile(ui, c_live))
                .child(card_tile(ui, c_attn))
                .child(crate::card::screen(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(900.0), px(600.0)));
        core.mount_root(&nsh_core::element::Root::of::<ProbeRoot>());
        let batch = core.commit().expect("initial mount batch");

        let mut children_of: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut terminal_buttons: Vec<(String, NodeId)> = Vec::new();
        for item in &batch.items {
            match item {
                MountItem::Insert { parent, child, .. } => {
                    children_of.entry(*parent).or_default().push(*child);
                }
                MountItem::ViewProps { id, props } => {
                    if let Some(label) = &props.accessibility.label
                        && (label.starts_with("Open Terminal for Card #")
                            || label.as_ref() == "Toggle Terminal · ⌘J")
                    {
                        terminal_buttons.push((label.to_string(), *id));
                    }
                }
                _ => {}
            }
        }
        assert_eq!(terminal_buttons.len(), 4);
        for (label, button_id) in terminal_buttons {
            let direct = children_of.get(&button_id).cloned().unwrap_or_default();
            assert_eq!(
                direct.len(),
                1,
                "terminal button `{label}` must render only its SVG icon without an extra dot badge child"
            );
        }
    }
}
