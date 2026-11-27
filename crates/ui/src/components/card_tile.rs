use super::*;
/// Card content/surface. Board owns pointer capture, drag geometry and activation.
///
/// Layout (rows appear only when they have content):
/// ```text
/// #16  [Needs attention]                        (3) [>_]
/// Full-width Card title wraps cleanly across the card
/// GitHub Issue #12 · open · by author
/// First body line, truncated
/// [#search] [#ux] [#agent] +1  [PR #147] [✕ CI failed]
/// ⎇ branch · 💬 2                 [(⚙)(✺) 1 blocked · 2 ▾]
/// ```
/// Top meta puts `#number`, card-level status chips, and compact actions on one row so the
/// title gets 100% width; signals (`tags`, PR/CI, review drafts) share a dedicated row; and
/// the single-line grounded footer pairs quiet metadata (`branch`, `replies`) on the left
/// with a single bounded actor chip (collapsed Agent Inspector Popover or static assignee)
/// on the right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EphemeralAgentBadge {
    pub terminal_id: String,
    pub title: String,
    pub provider_icon: Icon,
    pub provider_name: String,
    pub display_status: String,
    pub attention_reason: Option<String>,
    pub herdr_status: canban_types::HerdrStatus,
    pub started_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CardTileModel {
    pub card: Card,
    pub has_terminal: bool,
    pub terminal_tint: ThemeColor,
    pub github_cached: Option<(String, String)>,
    pub tags: Vec<String>,
    pub active_tags: Vec<String>,
    pub status_specs: Vec<(String, Tone, bool)>,
    pub running: Vec<crate::index::RunningAgentBadge>,
    pub ephemeral_agents: Vec<EphemeralAgentBadge>,
    pub replies: usize,
    pub review_draft: Option<canban_types::ReviewDraft>,
    pub review_running: bool,
    pub pr_view: Option<crate::components::pr_status::CardPrView>,
    pub sync_mark: Option<canban_types::SyncMark>,
}

impl CardTileModel {
    pub fn build(ui: Ui, card: Card) -> Self {
        let active_tags = ui.github.filters.with(|all| {
            all.get(&card.project_id)
                .map(|f| f.tags.clone())
                .unwrap_or_default()
        });
        let ui_merging = ui.merging_cards.with(|ids| ids.contains(&card.id));
        let (has_terminal, terminal_tint, ephemeral_agents) = ui.terminal_entries.with(|rows| {
            let live_entries: Vec<crate::terminals::Entry> = rows
                .iter()
                .filter(|e| crate::terminals::is_card_live_entry(e, &card.id))
                .cloned()
                .collect();
            (
                !live_entries.is_empty(),
                card_terminal_icon_tint(&live_entries, false),
                extract_ephemeral_badges(&live_entries),
            )
        });
        let idx = ui.index.get();
        let card_status = ui.card_status.get();
        let known_index = ui.known_tags.get();
        ui.snapshot.with(|snapshot| {
            Self::from_indexed(
                snapshot,
                &idx,
                &card_status,
                &known_index,
                active_tags,
                ui_merging,
                has_terminal,
                terminal_tint,
                ephemeral_agents,
                card,
            )
        })
    }

    pub fn from_snapshot(ui: Ui, card: Card) -> Self {
        let active_tags = ui.github.filters.with_untracked(|all| {
            all.get(&card.project_id)
                .map(|f| f.tags.clone())
                .unwrap_or_default()
        });
        let ui_merging = ui
            .merging_cards
            .with_untracked(|ids| ids.contains(&card.id));
        let (has_terminal, terminal_tint, ephemeral_agents) =
            ui.terminal_entries.with_untracked(|rows| {
                let live_entries: Vec<crate::terminals::Entry> = rows
                    .iter()
                    .filter(|e| crate::terminals::is_card_live_entry(e, &card.id))
                    .cloned()
                    .collect();
                (
                    !live_entries.is_empty(),
                    card_terminal_icon_tint(&live_entries, false),
                    extract_ephemeral_badges(&live_entries),
                )
            });
        let idx = ui.index.get_untracked();
        let card_status = ui.card_status.get_untracked();
        let known_index = ui.known_tags.get_untracked();
        ui.snapshot.with_untracked(|snapshot| {
            Self::from_indexed(
                snapshot,
                &idx,
                &card_status,
                &known_index,
                active_tags,
                ui_merging,
                has_terminal,
                terminal_tint,
                ephemeral_agents,
                card,
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn from_indexed(
        snapshot: &Snapshot,
        idx: &crate::index::SnapshotIndex,
        card_status: &std::collections::BTreeMap<String, crate::index::CardStatusDigest>,
        known_index: &crate::index::KnownTagsIndex,
        active_tags: Vec<String>,
        ui_merging: bool,
        has_terminal: bool,
        terminal_tint: ThemeColor,
        ephemeral_agents: Vec<EphemeralAgentBadge>,
        card: Card,
    ) -> Self {
        let github_cached = card.github_issue().and_then(|source| {
            idx.github_issue(&card.project_id, source.repository_id, source.issue_id)
                .map(|v| (v.state.clone(), v.author_login.clone()))
        });
        let matches_indexed = idx.cards.get(&card.id) == Some(&card);
        let tags = if matches_indexed && card.removed_at.is_none() {
            known_index.tags_for_card(&card.id)
        } else {
            let known_tags = known_index.for_project(Some(&card.project_id));
            crate::tags::card_tags(&card, &known_tags)
        };
        let (mut status_specs, running) = if matches_indexed
            && let Some(digest) = card_status.get(&card.id)
        {
            let specs = if ui_merging {
                digest.specs_ui_merging.clone()
            } else {
                digest.specs_normal.clone()
            };
            (specs, digest.running.clone())
        } else {
            let specs = crate::components::activity::card_status_specs(snapshot, &card, ui_merging);
            let running = crate::components::activity::running_agents(snapshot, &card)
                .into_iter()
                .filter_map(|req| {
                    let agent = req.agent.as_ref()?;
                    let (provider_icon, provider_name) = crate::profiles::provider(&agent.profile);
                    let wait_detail = req
                        .runtime_wait()
                        .and_then(crate::terminals::entries::format_runtime_wait);
                    let started_at = crate::components::activity::agent_started_at(snapshot, &req);
                    Some(crate::index::RunningAgentBadge {
                        terminal_id: req.id.clone(),
                        card_id: agent.card_id.clone(),
                        profile_id: agent.profile.id.clone(),
                        provider_icon,
                        provider_name,
                        herdr_status: req.herdr_status(),
                        wait_detail,
                        started_at,
                    })
                })
                .collect();
            (specs, running)
        };
        let sync_mark = snapshot.pending_sync.get(&card.id).cloned();
        if let Some(mark) = sync_mark {
            let (label, tone) = crate::hosts::sync_chip_spec(mark);
            status_specs.insert(0, (label.to_owned(), tone, false));
        }
        let replies = idx.reply_count(&card.id);
        let review_draft = idx.review_draft(&card.id).cloned();
        let review_running = crate::review::active_review_run(snapshot, &card.id).is_some();
        let pr_view = crate::components::pr_status::from_card(snapshot, idx, &card);
        Self {
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
            review_draft,
            review_running,
            pr_view,
            sync_mark,
        }
    }
}

fn extract_ephemeral_badges(live_entries: &[crate::terminals::Entry]) -> Vec<EphemeralAgentBadge> {
    live_entries
        .iter()
        .filter(|e| e.agent.is_none() && e.saved.has_active_agent())
        .map(|entry| {
            let (provider_icon, provider_name) = entry
                .provider()
                .unwrap_or_else(|| (Icon::Agent, "Agent".into()));
            let title = entry
                .saved
                .name
                .clone()
                .unwrap_or_else(|| provider_name.clone());
            let started_at =
                (entry.saved.created_at > 1_000_000_000_000).then_some(entry.saved.created_at);
            EphemeralAgentBadge {
                terminal_id: entry.saved.id.clone(),
                title,
                provider_icon,
                provider_name,
                display_status: entry.display_status(),
                attention_reason: entry.attention_reason(),
                herdr_status: entry.herdr_status(),
                started_at,
            }
        })
        .collect()
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
        review_draft,
        review_running,
        pr_view,
        sync_mark,
    } = model;
    let _ = sync_mark;
    let reminder_id = card.id.clone();
    let count = computed(move || {
        ui.reminder_counts
            .with(|all| all.get(&reminder_id).copied().unwrap_or_default())
    });
    let selected_id = card.id.clone();
    let is_selected = computed(move || ui.card.with(|cid| cid == &selected_id));
    let terminal_card = card.id.clone();
    let card_number = card.number;
    let archive_card = card.clone();
    let hovered = signal(false);
    let active = card.active();
    let resolved = card.resolved();
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
            if is_selected.get() {
                SELECTED.resolve()
            } else if resolved {
                INSET.resolve()
            } else {
                SURFACE.resolve()
            }
        })
        .hover(|s| s.bg(CARD_HOVER));
    if resolved {
        tile = tile.border_dashed().opacity(0.72);
    }
    let resolved_tip = crate::card::resolve::tooltip(&card);
    let chips: Vec<AnyElement> = status_specs
        .into_iter()
        .map(|(text, tone, live)| {
            let key = format!("status:{live}:{text}");
            if live {
                live_status(text, tone).key(key).into_element()
            } else if text == "Resolved"
                && let Some(tip) = resolved_tip.clone()
            {
                view()
                    .key(key)
                    .flex_shrink_0()
                    .tooltip(hint_tooltip(tip))
                    .child(status(text, tone))
                    .into_element()
            } else {
                status(text, tone).key(key).into_element()
            }
        })
        .collect();
    // 1. Top Meta Row: #number + primary card-level status chips on the left, actions on the right.
    tile = tile.child(
        view()
            .key("header")
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .min_w(px(0.))
            .min_h(px(22.))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .flex_1()
                    .min_w(px(0.))
                    .gap_1p5()
                    .items_center()
                    .child(
                        muted(if card.number == 0 {
                            "Local".to_owned()
                        } else {
                            format!("#{}", card.number)
                        })
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .tabular_nums()
                        .flex_shrink_0(),
                    )
                    .children(chips),
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
                        move || active && resolved,
                        move || archive_button(ui, archive_card.clone()),
                    ))
                    // Default-visible Review icon on PR Cards so users discover Agent PR Review without hovering.
                    .child(when(
                        {
                            let is_pr = active && !resolved && crate::review::card_has_pr(&card);
                            move || is_pr
                        },
                        {
                            let review_card_id = card.id.clone();
                            let has_draft = review_draft.is_some();
                            let draft_count =
                                review_draft.as_ref().map(|d| d.comments.len()).unwrap_or(0);
                            move || {
                                review_button(
                                    ui,
                                    review_card_id.clone(),
                                    card_number,
                                    has_draft,
                                    draft_count,
                                    review_running,
                                )
                            }
                        },
                    ))
                    // Space stays reserved so hovering never shifts top meta chips.
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
    // 2. Full-width Card title.
    tile = tile.child(
        text(card.title.clone())
            .key("title")
            .text_size(px(BODY_SIZE))
            .text_color(if resolved { MUTED } else { TEXT })
            .font_weight(FontWeight::SEMIBOLD)
            .w_full()
            .min_w(px(0.)),
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
    if let Some(sched) = &card.schedule_source {
        let meta = view()
            .key("schedule-meta")
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .min_w(px(0.))
            .tooltip(hint_tooltip(format!(
                "Scheduled by {} ({}) · Run #{}",
                sched.workflow_name, sched.cron, sched.run_number
            )))
            .child(icon_tinted(Icon::Clock, ACCENT).w(px(12.)).h(px(12.)))
            .child(
                muted(format!(
                    "Scheduled · {} #{}",
                    sched.workflow_name, sched.run_number
                ))
                .truncate(),
            )
            .child(badge(sched.cron.clone()));
        tile = tile.child(meta);
    }
    // 3. Single-line body excerpt.
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
    // 4. Signals & taxonomy row: tags + PR/CI status + local review draft.
    let pr_pill = pr_view
        .as_ref()
        .and_then(|info| crate::components::pr_status::pill(ui, info, true));
    if !tags.is_empty() || pr_pill.is_some() || review_draft.is_some() {
        let mut signals = view()
            .key("signals")
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .min_w(px(0.));
        for tag in tags.iter().take(crate::tags::MAX_BOARD_TILE_TAGS) {
            let is_active = active_tags
                .iter()
                .any(|t| t.to_lowercase() == tag.to_lowercase());
            signals = signals.child(tag_pill(ui, &card.project_id, tag, is_active));
        }
        let extra = tags.len().saturating_sub(crate::tags::MAX_BOARD_TILE_TAGS);
        if extra > 0 {
            let all_hint = tags
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" ");
            signals = signals.child(
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
        if let Some(pr_pill) = pr_pill {
            signals = signals.child(pr_pill.key("footer-pr-ci"));
        }
        if let Some(draft) = &review_draft {
            signals = signals.child(review_draft_pill(ui, &card.id, draft));
        }
        tile = tile.child(signals);
    }
    // 5. Grounded single-line footer: quiet metadata (branch, replies) on the left,
    // single actor slot (active Agent popover chip, or static assignee when idle) on the right.
    let branch = card.branch.clone().filter(|b| !b.is_empty());
    let has_live_agents = !running.is_empty() || !ephemeral_agents.is_empty();
    let assignee_badge = (!has_live_agents).then_some(()).and_then(|()| {
        card.assignee.as_deref().map(|aid| {
            let (provider_icon, provider_name) = ui
                .profiles
                .with(|ps| {
                    ps.iter()
                        .find(|p| p.id == aid)
                        .map(crate::profiles::provider)
                })
                .unwrap_or((Icon::Agent, "Agent".into()));
            (aid.to_owned(), provider_icon, provider_name)
        })
    });
    if branch.is_some() || replies > 0 || has_live_agents || assignee_badge.is_some() {
        let mut left = view()
            .key("footer-left")
            .flex_row()
            .flex_1()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .overflow_hidden();
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
                    .child(icon(Icon::Branch).w(px(12.)).h(px(12.)).flex_shrink_0())
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
                    .child(
                        icon(Icon::MessageSquare)
                            .w(px(12.))
                            .h(px(12.))
                            .flex_shrink_0(),
                    )
                    .child(muted(replies.to_string())),
            );
        }
        let right_pill: Option<AnyElement> = if has_live_agents {
            Some(
                crate::components::agent_popover::agent_summary_popover(
                    ui,
                    &card.id,
                    card.number,
                    &running,
                    &ephemeral_agents,
                )
                .into_element(),
            )
        } else {
            assignee_badge.map(|(profile_id, provider_icon, provider_name)| {
                let tip = format!(
                    "Assigned to @{profile_id} ({provider_name}) — replies sync automatically"
                );
                view()
                    .key(format!("assignee:{profile_id}"))
                    .h(px(22.))
                    .px_1p5()
                    .max_w(px(124.))
                    .overflow_hidden()
                    .flex_shrink_0()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .rounded(px(6.))
                    .bg(INSET)
                    .border_1()
                    .border_color(BORDER)
                    .tooltip(hint_tooltip(tip))
                    .child(
                        icon_tinted(provider_icon, MUTED)
                            .w(px(11.))
                            .h(px(11.))
                            .flex_shrink_0(),
                    )
                    .child(
                        muted(format!("@{profile_id}"))
                            .text_size(px(11.))
                            .min_w(px(0.))
                            .truncate(),
                    )
                    .into_element()
            })
        };
        tile = tile.child(
            view()
                .key("footer")
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .min_w(px(0.))
                .pt_0p5()
                .child(left)
                .children(right_pill),
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

/// Always-visible PR review entry on Board Card tiles: opens draft diff if ready,
/// opens terminal if reviewing, or launches Agent PR Review immediately.
fn review_button(
    ui: Ui,
    card_id: String,
    number: u64,
    has_draft: bool,
    draft_count: usize,
    review_running: bool,
) -> View {
    let default_prof = crate::sessions::default_review_profile(ui);
    let hint = if review_running {
        format!("Agent review running on Card #{number} — open Terminal")
    } else if has_draft {
        if draft_count > 0 {
            format!("Open {draft_count} PR review draft(s) in Diff")
        } else {
            "Open PR review draft in Diff".to_owned()
        }
    } else {
        format!("Run Agent PR Review (@{default_prof})")
    };
    let a11y = hint.clone();
    let tint = if review_running || has_draft {
        ACCENT
    } else {
        MUTED
    };
    let btn = press_target()
        .w(px(22.))
        .h(px(22.))
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .bg(if has_draft { SELECTED } else { CLEAR })
        .hover(|s| s.bg(INSET))
        .focus(|s| s.border_1().border_color(ACCENT))
        .accessibility_label(move || Some(a11y.clone().into()))
        .child(icon_tinted(Icon::Eye, tint).w(px(14.)).h(px(14.)))
        .on_press(move |_: &()| {
            if review_running {
                crate::terminals::open_card_quick(ui, &card_id);
            } else if has_draft {
                crate::review::open_card_pr(ui, &card_id);
            } else {
                crate::review::start_agent_review(ui, &card_id, None, None);
            }
        });
    view()
        .flex_shrink_0()
        .w(px(22.))
        .h(px(22.))
        .window_drag(false)
        .tooltip(hint_tooltip(hint).placement(Placement::Bottom))
        .child(btn)
}

/// Clickable footer capsule when a local Agent PR review draft is ready for human review.
fn review_draft_pill(ui: Ui, card_id: &str, draft: &canban_types::ReviewDraft) -> View {
    let cid = card_id.to_owned();
    let count = draft.comments.len();
    let label_text = if count == 0 {
        "Review ready ↗".to_owned()
    } else if count == 1 {
        "1 review draft ↗".to_owned()
    } else {
        format!("{count} review drafts ↗")
    };
    let author_hint = draft.author.as_deref().unwrap_or("Agent");
    let hint = format!(
        "Local PR review draft by {author_hint} ({count} inline comment{}) — click to review & publish",
        if count == 1 { "" } else { "s" }
    );
    let a11y = hint.clone();
    view()
        .key("footer-review-draft")
        .flex_shrink_0()
        .window_drag(false)
        .tooltip(hint_tooltip(hint))
        .child(
            press_target()
                .h(px(20.))
                .px_1p5()
                .flex_row()
                .items_center()
                .gap_1()
                .rounded(px(5.))
                .bg(SELECTED)
                .border_1()
                .border_color(ACCENT)
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                .accessibility_label(move || Some(a11y.clone().into()))
                .child(icon_tinted(Icon::Eye, ACCENT).w(px(11.)).h(px(11.)))
                .child(
                    text(label_text)
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ACCENT),
                )
                .on_press(move |_: &()| crate::review::open_card_pr(ui, &cid)),
        )
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
