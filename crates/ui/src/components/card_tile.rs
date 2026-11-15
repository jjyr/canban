use super::*;
/// Card content/surface. Board owns pointer capture, drag geometry and activation.
///
/// Layout (rows appear only when they have content):
/// ```text
/// #16  Title wraps beside the number            (3) [>_]
/// GitHub Issue #12 · open · by author
/// First body line, truncated
/// [#search] [#ux] [#agent] +1
/// Needs review  ⎇ branch  💬 2      [((●)) ✦ @developer]
/// ```
/// The `[>_]` button opens this Card's quick terminal split directly on the Card page
/// (pinned with a highlighted icon when a session is running, or revealed on hover); active
/// Agent pills pulse (or flag attention) and open that Agent tab on the Card.
#[derive(Clone, PartialEq)]
pub(crate) struct CardTileModel {
    pub card: Card,
    pub live_entries: Vec<crate::terminals::Entry>,
    pub has_terminal: bool,
    pub terminal_tint: ThemeColor,
    pub github_cached: Option<(String, String)>,
    pub tags: Vec<String>,
    pub active_tags: Vec<String>,
    pub status_specs: Vec<(String, Tone, bool)>,
    pub running: Vec<(TerminalEntry, Option<u64>)>,
    pub ephemeral_agents: Vec<(crate::terminals::Entry, Option<u64>)>,
    pub replies: usize,
}

impl CardTileModel {
    pub fn build(ui: Ui, card: Card) -> Self {
        let snapshot = ui.snapshot.get();
        let local_tags = ui.local_tags.get();
        let active_tags = ui.github.filters.with(|all| {
            all.get(&card.project_id)
                .map(|f| f.tags.clone())
                .unwrap_or_default()
        });
        let ui_merging = ui.merging_cards.with(|ids| ids.contains(&card.id));
        let live_entries = ui.terminal_entries.with(|rows| {
            rows.iter()
                .filter(|e| crate::terminals::is_card_live_entry(e, &card.id))
                .cloned()
                .collect::<Vec<_>>()
        });
        Self::from_parts(
            &snapshot,
            &local_tags,
            active_tags,
            ui_merging,
            live_entries,
            card,
        )
    }

    pub fn from_snapshot(ui: Ui, card: Card) -> Self {
        let snapshot = ui.snapshot.get_untracked();
        let local_tags = ui.local_tags.get_untracked();
        let active_tags = ui.github.filters.with_untracked(|all| {
            all.get(&card.project_id)
                .map(|f| f.tags.clone())
                .unwrap_or_default()
        });
        let ui_merging = ui
            .merging_cards
            .with_untracked(|ids| ids.contains(&card.id));
        let live_entries = crate::terminals::card_live_entries(&snapshot, &card.id);
        Self::from_parts(
            &snapshot,
            &local_tags,
            active_tags,
            ui_merging,
            live_entries,
            card,
        )
    }

    fn from_parts(
        snapshot: &Snapshot,
        local_tags: &std::collections::BTreeSet<String>,
        active_tags: Vec<String>,
        ui_merging: bool,
        live_entries: Vec<crate::terminals::Entry>,
        card: Card,
    ) -> Self {
        let has_terminal = !live_entries.is_empty();
        let terminal_tint = card_terminal_icon_tint(&live_entries, false);
        let github_cached = card.github_issue().and_then(|source| {
            snapshot
                .github_issues
                .iter()
                .find(|v| {
                    v.project_id == card.project_id
                        && v.source.repository_id == source.repository_id
                        && v.source.issue_id == source.issue_id
                })
                .map(|v| (v.state.clone(), v.author_login.clone()))
        });
        let known_tags = crate::tags::known_map(snapshot, local_tags, Some(&card.project_id));
        let tags = crate::tags::card_tags(&card, &known_tags);
        let status_specs =
            crate::components::activity::card_status_specs(snapshot, &card, ui_merging);
        let running = crate::components::activity::running_agents(snapshot, &card)
            .into_iter()
            .map(|req| {
                let started = crate::components::activity::agent_started_at(snapshot, &req);
                (req, started)
            })
            .collect();
        let ephemeral_agents = live_entries
            .iter()
            .filter(|e| e.agent.is_none() && e.saved.agent_engine.is_some())
            .cloned()
            .map(|entry| {
                let started_at =
                    (entry.saved.created_at > 1_000_000_000_000).then_some(entry.saved.created_at);
                (entry, started_at)
            })
            .collect();
        let replies = crate::card::timeline::reply_count(&snapshot.events, &card.id);
        Self {
            card,
            live_entries,
            has_terminal,
            terminal_tint,
            github_cached,
            tags,
            active_tags,
            status_specs,
            running,
            ephemeral_agents,
            replies,
        }
    }
}

pub fn card_tile(ui: Ui, card: Card) -> View {
    card_tile_from_model(ui, CardTileModel::from_snapshot(ui, card))
}

pub(crate) fn card_tile_from_model(ui: Ui, model: CardTileModel) -> View {
    let CardTileModel {
        card,
        has_terminal,
        terminal_tint,
        github_cached,
        tags,
        active_tags,
        status_specs,
        running,
        ephemeral_agents,
        replies,
        ..
    } = model;
    let reminder_id = card.id.clone();
    let count = computed(move || {
        ui.reminder_counts
            .with(|all| all.get(&reminder_id).copied().unwrap_or_default())
    });
    let selected_id = card.id.clone();
    let terminal_card = card.id.clone();
    let card_number = card.number;
    let archive_card = card.clone();
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
            .key("header")
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
        let mut meta = view()
            .key("github-meta")
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
        if let Some((state, author_login)) = github_cached {
            meta = meta
                .child(badge(state))
                .child(muted(format!("by {author_login}")).truncate());
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
        tile = tile.child(
            muted(excerpt.to_owned())
                .key("excerpt")
                .truncate()
                .min_w(px(0.)),
        );
    }
    if !tags.is_empty() {
        let mut tag_row = view()
            .key("tags")
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
                    .key("tags-extra")
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
    let chips: Vec<AnyElement> = status_specs
        .into_iter()
        .map(|(text, tone, live)| {
            let key = format!("status:{live}:{text}");
            if live {
                live_status(text, tone).key(key).into_element()
            } else {
                status(text, tone).key(key).into_element()
            }
        })
        .collect();
    let branch = card.branch.clone().filter(|b| !b.is_empty());
    if !chips.is_empty()
        || !running.is_empty()
        || !ephemeral_agents.is_empty()
        || branch.is_some()
        || replies > 0
    {
        let mut left = view()
            .key("footer-left")
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
                    .key("footer-branch")
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
        if replies > 0 {
            let tip = if replies == 1 {
                "1 reply".to_owned()
            } else {
                format!("{replies} replies")
            };
            left = left.child(
                view()
                    .key("footer-replies")
                    .flex_row()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .tooltip(hint_tooltip(tip))
                    .child(icon(Icon::MessageSquare).w(px(12.)).h(px(12.)))
                    .child(muted(replies.to_string())),
            );
        }
        let mut pills: Vec<AnyElement> = running
            .into_iter()
            .map(|(request, started_at)| {
                let pill_key = format!("agent:{}", request.id);
                agent_pill(ui, request, started_at)
                    .key(pill_key)
                    .into_element()
            })
            .collect();
        pills.extend(ephemeral_agents.into_iter().map(|(entry, started_at)| {
            let pill_key = format!("ephemeral:{}", entry.saved.id);
            ephemeral_agent_pill(ui, &card.id, entry, started_at)
                .key(pill_key)
                .into_element()
        }));
        tile = tile.child(
            view()
                .key("footer")
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
    let pill_key = format!("tag:{tag}");
    let label_text = format!("#{tag}");
    let hint = if is_active {
        format!("Clear #{tag} Board filter")
    } else {
        format!("Filter Board by #{tag}")
    };
    let a11y = hint.clone();
    view()
        .key(pill_key)
        .flex_shrink_0()
        .tooltip(hint_tooltip(hint))
        .child(
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
fn agent_pill(ui: Ui, entry: TerminalEntry, started_at: Option<u64>) -> View {
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
) -> View {
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
            name: Some("Terminal 1".into()),
            origin_project_id: None,
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
                observed_agent: None,
            }),
            agent: None,
            created_at: 1,
        });
        s.terminals.push(TerminalEntry {
            id: "cli-attention".into(),
            conversation_id: None,
            number: 2,
            name: Some("Antigravity 2".into()),
            origin_project_id: None,
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
                observed_agent: None,
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

    #[test]
    fn card_tile_renders_reply_count_with_icon_when_positive_and_hides_zero() {
        fn collect_texts_and_svgs(s: Snapshot, card_id: &str) -> (Vec<String>, usize) {
            thread_local! {
                static SNAPSHOT: std::cell::RefCell<Snapshot> = std::cell::RefCell::new(Snapshot::default());
                static CARD_ID: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
            }
            SNAPSHOT.with(|slot| *slot.borrow_mut() = s);
            CARD_ID.with(|slot| *slot.borrow_mut() = card_id.to_owned());

            #[component]
            fn TileProbe() -> impl IntoElement {
                canban_ui_kit::tokens::install_theme();
                let ui = Ui::new();
                let snap = SNAPSHOT.with(|slot| slot.borrow().clone());
                let cid = CARD_ID.with(|slot| slot.borrow().clone());
                let card = snap.cards.iter().find(|c| c.id == cid).unwrap().clone();
                ui.snapshot.set(snap);
                ui.project.set("p1".into());
                view().child(card_tile(ui, card))
            }

            let mut core = Core::new(ApproxTextEngine, size(px(400.0), px(300.0)));
            core.mount_root(&nsh_core::element::Root::of::<TileProbe>());
            let batch = core.commit().expect("initial mount batch");
            let mut texts = Vec::new();
            let mut svg_count = 0usize;
            for item in batch.items {
                match item {
                    MountItem::Text { text, .. } => texts.push(text.text.to_string()),
                    MountItem::Canvas { .. } => svg_count += 1,
                    _ => {}
                }
            }
            (texts, svg_count)
        }

        // 0 replies on a quiet Card (no branch, no status): only the terminal button SVG exists,
        // and "0" is never rendered.
        let quiet = sample_snapshot();
        let (quiet_texts, quiet_svgs) = collect_texts_and_svgs(quiet, "c-idle");
        assert!(
            !quiet_texts.iter().any(|t| t == "0"),
            "0 replies must not render '0': {quiet_texts:?}"
        );
        assert_eq!(quiet_svgs, 1, "quiet 0-reply Card renders only [>_] SVG");

        // 0 replies on a Card that already has a footer (branch): still must not render "0"
        // or add a MessageSquare SVG.
        let mut with_branch = sample_snapshot();
        with_branch.cards[0].branch = Some("canban/1".into());
        let (branch_texts, branch_svgs) = collect_texts_and_svgs(with_branch, "c-idle");
        assert!(
            !branch_texts.iter().any(|t| t == "0"),
            "0 replies with branch footer must not render '0': {branch_texts:?}"
        );
        assert_eq!(branch_svgs, 2, "branch + [>_] SVGs only when replies == 0");

        // Positive replies (3 narrative messages; echoed report + deleted comment ignored):
        // renders MessageSquare SVG + exact count "3".
        let mut with_replies = sample_snapshot();
        let push_ev = |s: &mut Snapshot, seq: u64, kind: &str, data: serde_json::Value| {
            s.events.push(Event {
                id: format!("ev-{seq}"),
                sequence: seq,
                project_id: "p1".into(),
                card_id: "c-idle".into(),
                kind: kind.into(),
                actor: "user".into(),
                message: "hello".into(),
                data,
                created_at: seq,
            });
        };
        push_ev(&mut with_replies, 1, "comment.created", json!({}));
        push_ev(&mut with_replies, 2, "comment.deleted", json!({}));
        push_ev(
            &mut with_replies,
            3,
            "execution.reported",
            json!({"report_id": "r1", "terminal_id": "t1"}),
        );
        push_ev(
            &mut with_replies,
            4,
            "comment.replied",
            json!({"report_id": "r1", "terminal_id": "t1"}),
        );
        push_ev(&mut with_replies, 5, "github.comment", json!({}));

        let (reply_texts, reply_svgs) = collect_texts_and_svgs(with_replies, "c-idle");
        assert!(
            reply_texts.iter().any(|t| t == "3"),
            "expected reply count '3' in {reply_texts:?}"
        );
        assert_eq!(
            reply_svgs, 2,
            "quiet Card with replies renders [>_] + MessageSquare SVGs"
        );
    }
}
