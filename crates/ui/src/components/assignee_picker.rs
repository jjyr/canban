//! Searchable Assignee combobox with compact recent-frequency recommendations.
//!
//! Used by both New Card (`draft_assignee_picker`) and Card Detail Header
//! (`card_header_assignee_picker`). Derives frequent recommendations from
//! the current Project's (or global fallback's) recent Card assignments in the
//! in-memory `SnapshotIndex` without extra backend RPCs or storage fields.
use crate::presenter::Ui;
use canban_types::{Card, Profile, Project};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

pub const UNASSIGNED_OPTION_ID: &str = "";
pub const RECENT_CARD_WINDOW: usize = 30;
pub const MAX_INLINE_FREQUENT: usize = 2;
pub const MAX_DROPDOWN_FREQUENT: usize = 3;
pub const GROUPING_MIN_PROFILES: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssigneePickerMode {
    Draft,
    Card,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrequentAssignee {
    pub profile_id: String,
    pub count: usize,
    pub latest_created_at: u64,
    pub from_project: bool,
}

fn normalize_assignee(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim)
        .map(|s| s.trim_start_matches('@'))
        .filter(|s| !s.is_empty())
}

fn rank_window(
    mut scoped: Vec<(&Card, &str)>,
    from_project: bool,
    enabled_order: &BTreeMap<&str, usize>,
) -> Vec<FrequentAssignee> {
    if scoped.is_empty() {
        return Vec::new();
    }
    scoped.sort_by(|(a, _), (b, _)| {
        Reverse(a.created_at)
            .cmp(&Reverse(b.created_at))
            .then_with(|| Reverse(a.number).cmp(&Reverse(b.number)))
            .then_with(|| a.id.cmp(&b.id))
    });
    scoped.truncate(RECENT_CARD_WINDOW);

    let mut stats: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
    for (card, aid) in scoped {
        let entry = stats.entry(aid).or_insert((0, 0));
        entry.0 += 1;
        if card.created_at > entry.1 {
            entry.1 = card.created_at;
        }
    }

    let mut ranked: Vec<FrequentAssignee> = stats
        .into_iter()
        .map(|(aid, (count, latest_created_at))| FrequentAssignee {
            profile_id: aid.to_owned(),
            count,
            latest_created_at,
            from_project,
        })
        .collect();
    ranked.sort_by(|a, b| {
        Reverse(a.count)
            .cmp(&Reverse(b.count))
            .then_with(|| Reverse(a.latest_created_at).cmp(&Reverse(b.latest_created_at)))
            .then_with(|| {
                enabled_order
                    .get(a.profile_id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX)
                    .cmp(
                        &enabled_order
                            .get(b.profile_id.as_str())
                            .copied()
                            .unwrap_or(usize::MAX),
                    )
            })
            .then_with(|| a.profile_id.cmp(&b.profile_id))
    });
    ranked
}

/// Derives up to `limit` frequently assigned enabled profiles from recent non-removed Cards.
///
/// Prioritizes the current Project's recent `RECENT_CARD_WINDOW` assigned Cards, backfills from
/// global recent Cards when the Project has fewer than `limit` distinct assignees, and falls
/// back to a single enabled `default_agent` when no Card assignment history exists anywhere.
pub fn frequent_assignees<'a>(
    cards: impl IntoIterator<Item = &'a Card>,
    project_id: Option<&str>,
    profiles: &[Profile],
    default_agent: Option<&str>,
    limit: usize,
) -> Vec<FrequentAssignee> {
    if limit == 0 {
        return Vec::new();
    }
    let enabled_order: BTreeMap<&str, usize> = profiles
        .iter()
        .filter(|p| p.enabled)
        .enumerate()
        .map(|(idx, p)| (p.id.as_str(), idx))
        .collect();
    if enabled_order.is_empty() {
        return Vec::new();
    }

    let eligible: Vec<(&Card, &str)> = cards
        .into_iter()
        .filter(|c| c.removed_at.is_none())
        .filter_map(|c| {
            let aid = normalize_assignee(c.assignee.as_deref())?;
            enabled_order.contains_key(aid).then_some((c, aid))
        })
        .collect();

    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    if let Some(pid) = project_id.map(str::trim).filter(|s| !s.is_empty()) {
        let project_cards: Vec<(&Card, &str)> = eligible
            .iter()
            .copied()
            .filter(|(c, _)| c.project_id == pid)
            .collect();
        for item in rank_window(project_cards, true, &enabled_order) {
            if seen.insert(item.profile_id.clone()) {
                out.push(item);
                if out.len() >= limit {
                    return out;
                }
            }
        }
    }

    if out.len() < limit && !eligible.is_empty() {
        for item in rank_window(eligible, false, &enabled_order) {
            if seen.insert(item.profile_id.clone()) {
                out.push(item);
                if out.len() >= limit {
                    return out;
                }
            }
        }
    }

    if out.is_empty()
        && let Some(def) = normalize_assignee(default_agent)
        && enabled_order.contains_key(def)
    {
        out.push(FrequentAssignee {
            profile_id: def.to_owned(),
            count: 0,
            latest_created_at: 0,
            from_project: false,
        });
    }

    out.truncate(limit);
    out
}

/// Filters `frequent` to at most `MAX_INLINE_FREQUENT` pills, excluding the currently selected assignee.
pub fn inline_frequent_assignees(
    frequent: &[FrequentAssignee],
    current_assignee: Option<&str>,
) -> Vec<FrequentAssignee> {
    let current = normalize_assignee(current_assignee);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for item in frequent {
        if Some(item.profile_id.as_str()) == current {
            continue;
        }
        if seen.insert(item.profile_id.as_str()) {
            out.push(item.clone());
            if out.len() >= MAX_INLINE_FREQUENT {
                break;
            }
        }
    }
    out
}

fn profile_option(
    prof: &Profile,
    project: Option<&Project>,
    count: usize,
    current: Option<&str>,
    group: Option<&str>,
) -> PickerOption {
    let (prov_icon, prov_name, overridden) = crate::profiles::effective_provider(prof, project);
    let base_detail = if prof.is_identity() {
        format!("Identity · {prov_name}")
    } else {
        prov_name.clone()
    };
    let detail = match (overridden, count > 0) {
        (true, true) => format!("{base_detail} · Override · {count}x"),
        (true, false) => format!("{base_detail} · Override"),
        (false, true) => format!("{base_detail} · {count}x"),
        (false, false) => base_detail,
    };

    let mut keywords = vec![prof.id.clone(), prov_name];
    if let Some(name) = prof
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case(&prof.id))
    {
        keywords.push(name.to_owned());
    }
    if prof.is_identity() {
        keywords.push("identity".into());
    }
    if overridden {
        keywords.push("override".into());
    }
    let role = prof.role_instruction.trim();
    if !role.is_empty() {
        keywords.push(role.chars().take(80).collect());
    }

    let mut opt = PickerOption::new(prof.id.clone(), format!("@{}", prof.id))
        .detail(detail)
        .icon(prov_icon)
        .colored_icon(true)
        .selected(current == Some(prof.id.as_str()))
        .keywords(keywords);
    if let Some(g) = group {
        opt = opt.group(g);
    }
    opt
}

/// Builds deduplicated `PickerOption` items for `SearchPopover`, grouping into `Frequent`
/// and `All Agents` when at least `GROUPING_MIN_PROFILES` enabled profiles exist.
pub fn build_assignee_options(
    profiles: &[Profile],
    project: Option<&Project>,
    frequent: &[FrequentAssignee],
    current_assignee: Option<&str>,
    mode: AssigneePickerMode,
) -> Vec<PickerOption> {
    let enabled: Vec<&Profile> = profiles.iter().filter(|p| p.enabled).collect();
    if enabled.is_empty() {
        return Vec::new();
    }
    let current = normalize_assignee(current_assignee);

    let mut options = Vec::with_capacity(enabled.len() + 1);
    match mode {
        AssigneePickerMode::Draft => {
            options.push(
                PickerOption::new(UNASSIGNED_OPTION_ID, "Unassigned")
                    .icon(Icon::Agent)
                    .selected(current.is_none())
                    .keywords(["none", "clear", "unassign", "unassigned"]),
            );
        }
        AssigneePickerMode::Card => {
            if current.is_some() {
                options.push(
                    PickerOption::new(UNASSIGNED_OPTION_ID, "Unassign")
                        .detail("Clear assignee")
                        .icon(Icon::Close)
                        .keywords(["none", "clear", "unassign", "unassigned", "remove"]),
                );
            }
        }
    }

    let mut seen_ids: BTreeSet<&str> = BTreeSet::new();
    let mut frequent_entries: Vec<(&Profile, &FrequentAssignee)> = Vec::new();
    for item in frequent {
        if frequent_entries.len() >= MAX_DROPDOWN_FREQUENT {
            break;
        }
        if let Some(prof) = enabled.iter().copied().find(|p| p.id == item.profile_id)
            && seen_ids.insert(prof.id.as_str())
        {
            frequent_entries.push((prof, item));
        }
    }

    let use_groups = enabled.len() >= GROUPING_MIN_PROFILES && !frequent_entries.is_empty();
    let freq_group = use_groups.then_some("Frequent");
    let all_group = use_groups.then_some("All Agents");

    for (prof, item) in frequent_entries {
        options.push(profile_option(
            prof, project, item.count, current, freq_group,
        ));
    }
    for prof in enabled {
        if seen_ids.insert(prof.id.as_str()) {
            options.push(profile_option(prof, project, 0, current, all_group));
        }
    }

    options
}

/// Applies shared fuzzy filtering/ranking to Assignee options.
pub fn filter_assignee_options(options: &[PickerOption], query: &str) -> Vec<PickerOption> {
    canban_ui_kit::filter_picker_options(options, query, None)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DraftTriggerModel {
    assignee: Option<String>,
    icon: Icon,
    detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QuickPickChip {
    profile_id: String,
    icon: Icon,
    detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CardAssigneeTrigger {
    card_id: String,
    project_id: String,
    active: bool,
    assignee: Option<String>,
    assignee_icon: Icon,
}

/// Searchable Assignee dropdown + Top-2 frequent quick-pick pills for New Card.
pub fn draft_assignee_picker(
    ui: Ui,
    project_id: Memo<String>,
    current: Memo<Option<String>>,
    on_change: impl Fn(Option<String>) + 'static,
) -> impl IntoElement {
    let on_change: Rc<dyn Fn(Option<String>)> = Rc::new(on_change);
    let has_enabled = computed(move || ui.profiles.with(|ps| ps.iter().any(|p| p.enabled)));

    when(move || has_enabled.get(), {
        let on_change = on_change.clone();
        move || {
            let frequent_memo = computed(move || {
                let pid = project_id.get();
                let profs = ui.profiles.get();
                let def = ui.default_agent.get();
                ui.index.with(|idx| {
                    frequent_assignees(
                        idx.cards.values(),
                        Some(&pid),
                        &profs,
                        Some(&def),
                        MAX_DROPDOWN_FREQUENT,
                    )
                })
            });

            let trigger_model = computed(move || {
                let cur = current
                    .get()
                    .and_then(|s| normalize_assignee(Some(&s)).map(str::to_owned));
                let pid = project_id.get();
                let proj = ui.index.with(|idx| idx.projects.get(&pid).cloned());
                let (icon, detail) = cur
                    .as_deref()
                    .and_then(|aid| {
                        ui.profiles.with(|ps| {
                            ps.iter().find(|p| p.id == aid).map(|p| {
                                let (ic, name, _) =
                                    crate::profiles::effective_provider(p, proj.as_ref());
                                (ic, name)
                            })
                        })
                    })
                    .unwrap_or_else(|| (Icon::Agent, "Unassigned".into()));
                DraftTriggerModel {
                    assignee: cur,
                    icon,
                    detail,
                }
            });

            let quick_picks_memo = computed(move || {
                let cur = trigger_model.with(|m| m.assignee.clone());
                let freq = frequent_memo.get();
                let inline = inline_frequent_assignees(&freq, cur.as_deref());
                let pid = project_id.get();
                let proj = ui.index.with(|idx| idx.projects.get(&pid).cloned());
                ui.profiles.with(|ps| {
                    inline
                        .into_iter()
                        .filter_map(|item| {
                            let prof = ps.iter().find(|p| p.enabled && p.id == item.profile_id)?;
                            let (ic, prov_name, _) =
                                crate::profiles::effective_provider(prof, proj.as_ref());
                            let detail = if item.count > 0 {
                                format!("{prov_name} · {}x", item.count)
                            } else {
                                prov_name
                            };
                            Some(QuickPickChip {
                                profile_id: item.profile_id,
                                icon: ic,
                                detail,
                            })
                        })
                        .collect::<Vec<_>>()
                })
            });

            let trigger = press_target()
                .h(px(28.))
                .px_2p5()
                .rounded(px(6.))
                .border_1()
                .border_color(BORDER)
                .bg(move || {
                    if trigger_model.with(|m| m.assignee.is_some()) {
                        SELECTED
                    } else {
                        INSET
                    }
                })
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.border_color(ACCENT))
                .flex_row()
                .items_center()
                .gap_1p5()
                .accessibility_label(move || {
                    Some(
                        trigger_model
                            .with(|m| match &m.assignee {
                                Some(aid) => format!("Assignee: @{aid}"),
                                None => "Assignee: Unassigned".to_owned(),
                            })
                            .into(),
                    )
                })
                .child(dynamic(move || {
                    let m = trigger_model.get();
                    if m.assignee.is_some() {
                        icon_colored(m.icon, 13.).into_element()
                    } else {
                        icon_tinted(Icon::Agent, MUTED)
                            .w(px(13.))
                            .h(px(13.))
                            .into_element()
                    }
                }))
                .child(dynamic(move || {
                    let m = trigger_model.get();
                    match m.assignee {
                        Some(aid) => text(format!("@{aid}"))
                            .text_size(px(CAPTION_SIZE))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ACCENT)
                            .max_w(px(150.))
                            .truncate()
                            .into_element(),
                        None => muted("Unassigned").into_element(),
                    }
                }))
                .child(icon_tinted(Icon::ChevronDown, MUTED).w(px(11.)).h(px(11.)));

            let on_select = on_change.clone();
            let popover = search_popover(
                trigger,
                move || {
                    let profs = ui.profiles.get();
                    let pid = project_id.get();
                    let proj = ui.index.with(|idx| idx.projects.get(&pid).cloned());
                    let freq = frequent_memo.get();
                    let cur = trigger_model.with(|m| m.assignee.clone());
                    build_assignee_options(
                        &profs,
                        proj.as_ref(),
                        &freq,
                        cur.as_deref(),
                        AssigneePickerMode::Draft,
                    )
                },
                move |selected| {
                    let next = normalize_assignee(Some(&selected)).map(str::to_owned);
                    on_select(next);
                },
            )
            .placeholder("Search @agent or provider…")
            .empty_text("No matching agents")
            .width(340.)
            .list_height(280.)
            .align(Align::Start)
            .on_open_change(move |open| ui.set_popover_open(open));

            let on_clear = on_change.clone();
            let on_picks = on_change.clone();

            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_1p5()
                .child(muted("Assignee").text_size(px(CAPTION_SIZE)))
                .child(popover)
                .child(when(
                    move || trigger_model.with(|m| m.assignee.is_some()),
                    move || {
                        let on_clear = on_clear.clone();
                        view().tooltip(hint_tooltip("Clear assignee")).child(
                            press_target()
                                .h(px(28.))
                                .w(px(26.))
                                .items_center()
                                .justify_center()
                                .rounded(px(6.))
                                .bg(SELECTED)
                                .border_1()
                                .border_color(BORDER)
                                .hover(|s| s.bg(CARD_HOVER))
                                .focus(|s| s.border_color(ACCENT))
                                .accessibility_label(|| Some("Clear assignee".into()))
                                .child(icon_tinted(Icon::Close, MUTED).w(px(11.)).h(px(11.)))
                                .on_press(move |_: &()| on_clear(None)),
                        )
                    },
                ))
                .child(dynamic(move || {
                    let picks = quick_picks_memo.get();
                    if picks.is_empty() {
                        return view().into_element();
                    }
                    let mut row = view().flex_row().flex_wrap().items_center().gap_1().child(
                        view()
                            .tooltip(hint_tooltip("Frequently assigned"))
                            .child(icon_tinted(Icon::Zap, MUTED).w(px(11.)).h(px(11.))),
                    );
                    for pick in picks {
                        let pid = pick.profile_id.clone();
                        let pid_press = pick.profile_id.clone();
                        let tip = format!("Assign @{pid} · {}", pick.detail);
                        let a11y = format!("Assign @{pid}");
                        let on_pick = on_picks.clone();
                        row = row.child(
                            view().tooltip(hint_tooltip(tip)).child(
                                press_target()
                                    .h(px(26.))
                                    .px_2()
                                    .rounded(px(6.))
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .bg(INSET)
                                    .border_1()
                                    .border_color(BORDER)
                                    .hover(|s| s.bg(CARD_HOVER))
                                    .focus(|s| s.border_color(ACCENT))
                                    .accessibility_label(move || Some(a11y.clone().into()))
                                    .child(icon_colored(pick.icon, 12.))
                                    .child(
                                        text(format!("@{pid}"))
                                            .text_size(px(CAPTION_SIZE))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(TEXT),
                                    )
                                    .on_press(move |_: &()| on_pick(Some(pid_press.clone()))),
                            ),
                        );
                    }
                    row.into_element()
                }))
                .into_element()
        }
    })
}

/// Searchable Assignee combobox for the Card Detail metadata row, with an
/// inline `[ Open ↗ ]` session button in the popover header when assigned.
pub fn card_header_assignee_picker(ui: Ui) -> impl IntoElement {
    let trigger_model = computed(move || {
        let c = ui.current_card()?;
        let proj = ui
            .index
            .with(|idx| idx.projects.get(&c.project_id).cloned());
        let assignee = c
            .assignee
            .as_deref()
            .and_then(|s| normalize_assignee(Some(s)).map(str::to_owned));
        let assignee_icon = assignee
            .as_deref()
            .and_then(|aid| {
                ui.profiles.with(|ps| {
                    ps.iter()
                        .find(|p| p.id == aid)
                        .map(|p| crate::profiles::effective_provider(p, proj.as_ref()).0)
                })
            })
            .unwrap_or(Icon::Agent);
        let active = c.active();
        Some(CardAssigneeTrigger {
            card_id: c.id,
            project_id: c.project_id,
            active,
            assignee,
            assignee_icon,
        })
    });

    when(
        move || trigger_model.with(Option::is_some),
        move || {
            let frequent_memo = computed(move || {
                let Some(pid) = trigger_model.with(|m| m.as_ref().map(|x| x.project_id.clone()))
                else {
                    return Vec::new();
                };
                let profs = ui.profiles.get();
                let def = ui.default_agent.get();
                ui.index.with(|idx| {
                    frequent_assignees(
                        idx.cards.values(),
                        Some(&pid),
                        &profs,
                        Some(&def),
                        MAX_DROPDOWN_FREQUENT,
                    )
                })
            });

            let trigger = press_target()
                .h(px(24.))
                .px_2()
                .rounded(px(5.))
                .flex_row()
                .items_center()
                .gap_1()
                .bg(move || {
                    if trigger_model.with(|m| m.as_ref().is_some_and(|x| x.assignee.is_some())) {
                        SELECTED
                    } else {
                        INSET
                    }
                })
                .hover(|st| st.bg(CARD_HOVER))
                .focus(|st| st.border_1().border_color(ACCENT))
                .disabled(move || !trigger_model.with(|m| m.as_ref().is_some_and(|x| x.active)))
                .accessibility_label(move || {
                    Some(
                        trigger_model
                            .with(|m| match m.as_ref().and_then(|x| x.assignee.as_deref()) {
                                Some(pid) => {
                                    format!("Assigned to @{pid} — replies sync automatically")
                                }
                                None => "Assign an Agent so replies sync automatically".to_owned(),
                            })
                            .into(),
                    )
                })
                .child(dynamic(move || {
                    let Some(m) = trigger_model.get() else {
                        return view().into_element();
                    };
                    if m.assignee.is_some() {
                        icon_colored(m.assignee_icon, 12.).into_element()
                    } else {
                        icon_tinted(Icon::Agent, MUTED)
                            .w(px(12.))
                            .h(px(12.))
                            .into_element()
                    }
                }))
                .child(dynamic(move || {
                    let Some(m) = trigger_model.get() else {
                        return view().into_element();
                    };
                    if let Some(pid) = m.assignee {
                        text(format!("@{pid} ▾"))
                            .text_size(px(CAPTION_SIZE))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ACCENT)
                            .max_w(px(140.))
                            .truncate()
                            .into_element()
                    } else {
                        muted("Assign ▾").into_element()
                    }
                }));

            let popover = search_popover(
                trigger,
                move || {
                    let Some(m) = trigger_model.get() else {
                        return Vec::new();
                    };
                    let profs = ui.profiles.get();
                    let proj = ui
                        .index
                        .with(|idx| idx.projects.get(&m.project_id).cloned());
                    let freq = frequent_memo.get();
                    build_assignee_options(
                        &profs,
                        proj.as_ref(),
                        &freq,
                        m.assignee.as_deref(),
                        AssigneePickerMode::Card,
                    )
                },
                move |selected| {
                    let Some(cid) =
                        trigger_model.with_untracked(|m| m.as_ref().map(|x| x.card_id.clone()))
                    else {
                        return;
                    };
                    let next = normalize_assignee(Some(&selected)).map(str::to_owned);
                    crate::presenter::assign_card(ui, &cid, next.as_deref());
                },
            )
            .placeholder("Filter @agent…")
            .empty_text("No matching agents")
            .width(340.)
            .list_height(280.)
            .align(Align::Start)
            .on_open_change(move |open| ui.set_popover_open(open))
            .header_action(move |close| {
                let m = trigger_model.get()?;
                let aid = m.assignee?;
                let cid = m.card_id;
                let aid_tip = aid.clone();
                let aid_a11y = aid.clone();
                Some(
                    view()
                        .flex_shrink_0()
                        .tooltip(hint_tooltip(format!("Open @{aid_tip} session")))
                        .child(
                            press_target()
                                .h(px(28.))
                                .px_2()
                                .rounded(px(6.))
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .bg(SELECTED)
                                .border_1()
                                .border_color(BORDER)
                                .hover(|s| s.bg(CARD_HOVER))
                                .focus(|s| s.border_color(ACCENT))
                                .accessibility_label(move || {
                                    Some(format!("Open @{aid_a11y} session").into())
                                })
                                .child(icon_tinted(Icon::Terminal, ACCENT).w(px(12.)).h(px(12.)))
                                .child(
                                    text("Open ↗")
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(ACCENT),
                                )
                                .on_press(move |_: &()| {
                                    close();
                                    crate::terminals::open_card_agent(ui, &cid, Some(&aid));
                                }),
                        )
                        .into_element(),
                )
            });

            view()
                .tooltip(hint_tooltip(move || {
                    trigger_model.with(|m| match m.as_ref().and_then(|x| x.assignee.as_deref()) {
                        Some(pid) => format!("Assigned to @{pid} — replies sync automatically"),
                        None => "Assign an Agent so replies sync automatically".to_owned(),
                    })
                }))
                .child(popover)
                .into_element()
        },
    )
}
