//! Mobile `Board` tab: pinned stage pills, horizontally swipeable stage pager (`Carousel`),
//! Linear-style hairline flat Card rows with long-press native context menu, Assignee-enabled
//! Card creation sheet, and searchable Project picker.
use super::{
    CardSegment, MobileSheet, MobileState,
    chrome::{
        MOBILE_CAPTION, MOBILE_FOOTNOTE, disclosure_chevron, empty_state, hairline_list_inset,
        is_mobile_os, mobile_button, mobile_field, mobile_footnote, mobile_inline_button,
        mobile_subhead, mobile_title, offline_sync_banner_inset, section_header,
        tab_scroll_bottom_pad, trigger_haptic,
    },
    mobile_hosts,
};
use crate::{components::card_tile::CardTileModel, presenter::*};
use canban_types::{Card, HostSummary, Project, Stage};
use canban_ui_kit::{pull_refresh_scroll, tokens::*, *};
use nsh_base::{MenuButtonItem, carousel, carousel_content, carousel_item, context_menu};
use nsh_core::{Haptic, HapticImpact, prelude::*};
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Clone, PartialEq)]
struct BoardHeaderModel {
    project_id: String,
    project_name: String,
    host_id: String,
    stages: Vec<(String, String, usize)>, // (id, name, count)
}

#[derive(Clone, PartialEq)]
struct StageStructure {
    project_id: String,
    stages: Vec<(String, String)>, // (id, name)
}

fn build_board_header(ui: Ui) -> BoardHeaderModel {
    let pid = ui.project.get();
    ui.snapshot.with(|s| {
        let proj = s
            .projects
            .iter()
            .find(|p| p.id == pid)
            .or_else(|| s.projects.first());
        let Some(project) = proj else {
            return BoardHeaderModel {
                project_id: String::new(),
                project_name: "Board".into(),
                host_id: String::new(),
                stages: Vec::new(),
            };
        };
        let host_id = s.project_host(&project.id).to_owned();
        let stages = project
            .workflow
            .stages
            .iter()
            .map(|st| {
                let count = s
                    .cards
                    .iter()
                    .filter(|c| c.project_id == project.id && c.active() && c.stage == st.id)
                    .count();
                (st.id.clone(), st.name.clone(), count)
            })
            .collect();
        BoardHeaderModel {
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            host_id,
            stages,
        }
    })
}

fn card_excerpt(body: &str) -> &str {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("")
}

fn mobile_card_row(
    ui: Ui,
    state: MobileState,
    model: CardTileModel,
    stages: Vec<(String, String)>,
) -> AnyElement {
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
        sync_mark: _,
    } = model;

    let card_id = card.id.clone();
    let open_id = card_id.clone();
    let resolved = card.resolved();
    let num_str = if card.number == 0 {
        "Local".to_owned()
    } else {
        format!("#{}", card.number)
    };

    let mut top_chips: Vec<AnyElement> = Vec::new();
    for (lbl, tone, live) in status_specs.into_iter().take(2) {
        if live {
            top_chips.push(live_status(lbl, tone).into_element());
        } else {
            top_chips.push(status(lbl, tone).into_element());
        }
    }
    if review_running {
        top_chips.push(live_status("Reviewing", Tone::Info).into_element());
    } else if review_draft.is_some() {
        top_chips.push(status("Review ready", Tone::Info).into_element());
    }

    let actor_chip: Option<AnyElement> = if let Some(badge) = running.first() {
        let tone = if badge.herdr_status == canban_types::HerdrStatus::Blocked {
            Tone::Attention
        } else {
            Tone::Info
        };
        Some(live_status(format!("@{}", badge.profile_id), tone).into_element())
    } else if let Some(badge) = ephemeral_agents.first() {
        let tone = if badge.herdr_status == canban_types::HerdrStatus::Blocked {
            Tone::Attention
        } else {
            Tone::Info
        };
        Some(live_status(badge.provider_name.clone(), tone).into_element())
    } else {
        card.assignee
            .as_deref()
            .map(|aid| status(format!("@{aid}"), Tone::Neutral).into_element())
    };

    let top_row = view()
        .w_full()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_1p5()
                .child(
                    text(num_str)
                        .text_size(px(MOBILE_FOOTNOTE))
                        .font_family(MONO_FONT)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(MUTED)
                        .tabular_nums(),
                )
                .children(top_chips),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .flex_shrink_0()
                .children(actor_chip)
                .child(disclosure_chevron()),
        );

    let title_view = mobile_title(card.title.clone())
        .text_color(if resolved { MUTED } else { TEXT })
        .number_of_lines(2);

    let excerpt = card_excerpt(&card.body).to_owned();
    let github_meta = card.github_issue().map(|source| {
        let mut row = view()
            .flex_row()
            .items_center()
            .gap_1p5()
            .child(icon_tinted(Icon::GitHub, MUTED).w(px(13.)).h(px(13.)))
            .child(
                mobile_footnote(format!(
                    "{} #{}",
                    if card.is_github_pr() { "PR" } else { "Issue" },
                    source.number
                ))
                .tabular_nums(),
            );
        if let Some((gh_state, _)) = &github_cached {
            row = row.child(mobile_footnote(format!("· {gh_state}")));
        }
        row
    });

    let branch = card.branch.clone().filter(|b| !b.is_empty());
    let has_pr = pr_view.is_some();
    let has_footer = !tags.is_empty() || branch.is_some() || replies > 0 || has_terminal || has_pr;

    let mut card_surface = press_target()
        .w_full()
        .px_4()
        .py_3p5()
        .gap_1p5()
        .bg(BACKGROUND)
        .cursor_pointer()
        .active(|s| s.opacity(0.72))
        .child(top_row)
        .child(title_view)
        .children(github_meta)
        .children((!excerpt.is_empty()).then(|| mobile_subhead(excerpt).number_of_lines(2)))
        .on_press(move |_: &()| {
            trigger_haptic(Haptic::Selection);
            state.push_card(open_id.clone(), CardSegment::Conversation);
        });

    if resolved {
        card_surface = card_surface.opacity(0.68);
    }

    if has_footer {
        let mut meta_left = view()
            .flex_1()
            .min_w(px(0.))
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2();

        for tag in tags.iter().take(3) {
            let is_active = active_tags.iter().any(|t| t.eq_ignore_ascii_case(tag));
            meta_left = meta_left.child(
                view()
                    .h(px(22.))
                    .px_1p5()
                    .rounded(px(5.))
                    .bg(if is_active { SELECTED } else { SURFACE })
                    .items_center()
                    .justify_center()
                    .child(
                        text(format!("#{tag}"))
                            .text_size(px(MOBILE_CAPTION))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if is_active { ACCENT } else { MUTED }),
                    ),
            );
        }
        if tags.len() > 3 {
            meta_left = meta_left.child(mobile_footnote(format!("+{}", tags.len() - 3)));
        }

        if let Some(pr) = &pr_view
            && let Some(pr_pill) = crate::components::pr_status::pill(ui, pr, true)
        {
            meta_left = meta_left.child(pr_pill);
        }

        if let Some(br) = branch {
            meta_left = meta_left.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .min_w(px(0.))
                    .child(icon_tinted(Icon::Branch, MUTED).w(px(12.)).h(px(12.)))
                    .child(
                        mobile_footnote(crate::components::git_workspace::compact(&br, 20))
                            .font_family(MONO_FONT)
                            .truncate(),
                    ),
            );
        }

        if replies > 0 {
            meta_left = meta_left.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(
                        icon_tinted(Icon::MessageSquare, MUTED)
                            .w(px(12.))
                            .h(px(12.)),
                    )
                    .child(mobile_footnote(replies.to_string()).tabular_nums()),
            );
        }

        let mut footer = view()
            .w_full()
            .pt_0p5()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .child(meta_left);

        if has_terminal {
            let term_cid = card.id.clone();
            footer = footer.child(
                press_target()
                    .h(px(26.))
                    .px_2()
                    .rounded(px(6.))
                    .bg(SURFACE)
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .active(|s| s.opacity(0.65))
                    .accessibility_label(Some(SharedString::from("Open card terminal")))
                    .child(
                        icon_tinted(Icon::Terminal, terminal_tint)
                            .w(px(13.))
                            .h(px(13.)),
                    )
                    .child(
                        text("Terminal")
                            .text_size(px(MOBILE_CAPTION))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(terminal_tint),
                    )
                    .on_press(move |_: &()| {
                        trigger_haptic(Haptic::Selection);
                        state.push_card(term_cid.clone(), CardSegment::Terminal);
                    }),
            );
        }

        card_surface = card_surface.child(footer);
    }

    if !is_mobile_os() {
        let move_id = card_id;
        return view()
            .w_full()
            .child(card_surface)
            .on_long_press(move || {
                trigger_haptic(Haptic::Impact(HapticImpact::Medium));
                state.open_sheet(MobileSheet::MoveCard(move_id.clone()));
            })
            .into_element();
    }

    let stage_items: Vec<MenuButtonItem> = stages
        .into_iter()
        .map(|(sid, sname)| {
            MenuButtonItem::new(format!("stage:{sid}"), sname).checked(sid == card.stage)
        })
        .collect();

    let mut menu_items = vec![
        MenuButtonItem::new("open", "Open Card").icon("doc.text"),
        MenuButtonItem::new("edit", "Edit Card").icon("pencil"),
        MenuButtonItem::new("terminal", "Open Terminal").icon("terminal"),
        MenuButtonItem::new("diff", "View Diff").icon("arrow.left.arrow.right.square"),
    ];
    if !stage_items.is_empty() {
        menu_items
            .push(MenuButtonItem::submenu("Move to…", stage_items).icon("arrow.right.circle"));
    }
    menu_items.push(MenuButtonItem::section(
        "",
        [MenuButtonItem::new(
            "resolve",
            if resolved {
                "Reopen Card"
            } else {
                "Resolve Card"
            },
        )
        .icon(if resolved {
            "arrow.uturn.left.circle"
        } else {
            "checkmark.circle"
        })],
    ));

    let cid_menu = card.id.clone();
    let card_for_resolve = card;
    context_menu()
        .w_full()
        .items(menu_items)
        .trigger(card_surface)
        .on_select(move |choice: &SharedString| {
            trigger_haptic(Haptic::Selection);
            let raw = choice.as_ref();
            if let Some(target_stage) = raw.strip_prefix("stage:") {
                move_card(ui, &cid_menu, target_stage);
                state.board_stage.set(target_stage.to_owned());
                return;
            }
            match raw {
                "open" => state.push_card(cid_menu.clone(), CardSegment::Conversation),
                "edit" => state.open_sheet(MobileSheet::EditCard(cid_menu.clone())),
                "terminal" => state.push_card(cid_menu.clone(), CardSegment::Terminal),
                "diff" => state.push_card(cid_menu.clone(), CardSegment::Diff),
                "resolve" => {
                    if card_for_resolve.resolved() {
                        crate::card::resolve_reopen(ui, card_for_resolve.clone());
                    } else {
                        crate::card::resolve_start(ui, card_for_resolve.clone());
                    }
                }
                _ => {}
            }
        })
        .into_element()
}

pub(crate) fn screen(ui: Ui, state: MobileState) -> impl IntoElement {
    let header_model = computed(move || build_board_header(ui));
    let stage_structure = computed(move || {
        let hm = header_model.get();
        StageStructure {
            project_id: hm.project_id,
            stages: hm
                .stages
                .into_iter()
                .map(|(id, name, _)| (id, name))
                .collect(),
        }
    });

    let active_stage = computed(move || {
        let hm = header_model.get();
        let chosen = state.board_stage.get();
        if !chosen.is_empty() && hm.stages.iter().any(|(id, _, _)| id == &chosen) {
            return chosen;
        }
        hm.stages
            .iter()
            .find(|(_, _, count)| *count > 0)
            .or_else(|| hm.stages.first())
            .map(|(id, _, _)| id.clone())
            .unwrap_or_else(|| "backlog".to_owned())
    });

    let active_stage_idx = computed(move || {
        let hm = header_model.get();
        let cur = active_stage.get();
        hm.stages
            .iter()
            .position(|(id, _, _)| id == &cur)
            .unwrap_or(0)
    });

    let stage_cards_map = computed(move || {
        let hm = header_model.get();
        let mut by_stage: BTreeMap<String, Vec<CardTileModel>> = BTreeMap::new();
        if hm.project_id.is_empty() {
            return by_stage;
        }
        let cards: Vec<Card> = ui.snapshot.with(|s| {
            let mut list: Vec<Card> = s
                .cards
                .iter()
                .filter(|c| c.project_id == hm.project_id && c.active())
                .cloned()
                .collect();
            list.sort_by(Card::board_cmp);
            list
        });
        for card in cards {
            let model = CardTileModel::build(ui, card);
            by_stage
                .entry(model.card.stage.clone())
                .or_default()
                .push(model);
        }
        by_stage
    });

    let pager_width = signal(390.0_f32);

    let stage_strip = dynamic(move || {
        let hm = header_model.get();
        let cur = active_stage.get();
        if hm.stages.is_empty() {
            return view().hidden();
        }
        view()
            .w_full()
            .flex_shrink_0()
            .border_b_1()
            .border_color(BORDER)
            .bg(BACKGROUND)
            .child(
                scroll().horizontal().scrollbar(false).w_full().child(
                    view()
                        .px_4()
                        .py_2()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .children(hm.stages.into_iter().map(|(id, name, count)| {
                            let selected = id == cur;
                            let next_id = id.clone();
                            press_target()
                                .h(px(32.))
                                .px_3()
                                .rounded_full()
                                .bg(if selected { ACCENT_FILL } else { SURFACE })
                                .flex_row()
                                .items_center()
                                .gap_1p5()
                                .cursor_pointer()
                                .active(|s| s.opacity(0.75))
                                .child(
                                    text(name)
                                        .text_size(px(13.5))
                                        .font_weight(if selected {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::MEDIUM
                                        })
                                        .text_color(if selected { ON_ACCENT } else { TEXT }),
                                )
                                .child(
                                    text(count.to_string())
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(if selected { ON_ACCENT } else { MUTED })
                                        .opacity(if selected { 0.85 } else { 1.0 })
                                        .tabular_nums(),
                                )
                                .on_press(move |_: &()| {
                                    trigger_haptic(Haptic::Selection);
                                    state.board_stage.set(next_id.clone());
                                })
                        })),
                ),
            )
    });

    let scoped_banner = dynamic(move || {
        let hid = header_model.get().host_id;
        let scope = (!hid.is_empty()).then_some(hid);
        offline_sync_banner_inset(ui, state, scope)
    });

    let pager = view()
        .flex_1()
        .w_full()
        .min_h(px(0.))
        .on_layout(move |bounds: &Bounds<Points>| {
            if bounds.size.width.0 > 50.0 {
                pager_width.set(bounds.size.width.0);
            }
        })
        .child(dynamic(move || {
            let structure = stage_structure.get();
            if structure.project_id.is_empty() || structure.stages.is_empty() {
                return view()
                    .size_full()
                    .p_4()
                    .child(empty_state(
                        Icon::Board,
                        Tone::Neutral,
                        "No Project selected",
                        "",
                        Some(
                            mobile_button("Switch Project", Variant::Secondary, move |_| {
                                state.open_sheet(MobileSheet::ProjectPicker);
                            })
                            .into_element(),
                        ),
                    ))
                    .into_element();
            }

            let stages_for_change = structure.stages.clone();
            let slides = structure
                .stages
                .clone()
                .into_iter()
                .map(|(stage_id, stage_name)| {
                    let scroll_ref = element_ref();
                    let pid_create = structure.project_id.clone();
                    let sid_lookup = stage_id.clone();
                    let all_stages = structure.stages.clone();
                    let stage_list = pull_refresh_scroll(
                        scroll().size_full().child(
                            view()
                                .w_full()
                                .pb(px(tab_scroll_bottom_pad()))
                                .child(dynamic(move || {
                                    let models = stage_cards_map
                                        .with(|m| m.get(&sid_lookup).cloned().unwrap_or_default());
                                    if models.is_empty() {
                                        let pid_btn = pid_create.clone();
                                        return empty_state(
                                            Icon::Board,
                                            Tone::Neutral,
                                            format!("No Cards in {stage_name}"),
                                            "",
                                            Some(
                                                mobile_inline_button(
                                                    "New Card",
                                                    Variant::Secondary,
                                                    move |_| {
                                                        state.open_sheet(MobileSheet::CreateCard(
                                                            pid_btn.clone(),
                                                        ));
                                                    },
                                                )
                                                .into_element(),
                                            ),
                                        )
                                        .into_element();
                                    }
                                    let rows: Vec<AnyElement> = models
                                        .into_iter()
                                        .map(|m| mobile_card_row(ui, state, m, all_stages.clone()))
                                        .collect();
                                    hairline_list_inset(rows, 16.0).into_element()
                                })),
                        ),
                        scroll_ref,
                        move || ui.refreshing.get(),
                        move || refresh(ui),
                    );

                    carousel_item()
                        .accessibility_label(Some(SharedString::from(format!("{stage_id} stage"))))
                        .w(move || px(pager_width.get()))
                        .h_full()
                        .flex_shrink_0()
                        .child(stage_list)
                });

            carousel()
                .size_full()
                .index(move || Some(active_stage_idx.get() as u32))
                .on_change(move |idx: &u32| {
                    if let Some((next_id, _)) = stages_for_change.get(*idx as usize)
                        && state.board_stage.get_untracked() != *next_id
                    {
                        trigger_haptic(Haptic::Selection);
                        state.board_stage.set(next_id.clone());
                    }
                })
                .child(carousel_content().size_full().flex_row().children(slides))
                .into_element()
        }));

    view()
        .size_full()
        .flex_col()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(stage_strip)
        .child(scoped_banner)
        .child(pager)
}

pub(crate) fn move_card_sheet_body(
    ui: Ui,
    state: MobileState,
    card_id: String,
) -> impl IntoElement {
    let cid_lookup = card_id.clone();
    let info = computed(move || {
        ui.snapshot.with(|s| {
            let card = s.cards.iter().find(|c| c.id == cid_lookup)?.clone();
            let stages: Vec<Stage> = s
                .projects
                .iter()
                .find(|p| p.id == card.project_id)
                .map(|p| p.workflow.stages.clone())
                .unwrap_or_default();
            Some((card, stages))
        })
    });

    dynamic(move || {
        let Some((card, stages)) = info.get() else {
            return mobile_subhead("Card unavailable").into_element();
        };

        let stage_rows: Vec<AnyElement> = stages
            .into_iter()
            .map(|stage| {
                let is_current = stage.id == card.stage;
                let cid = card.id.clone();
                let stage_id = stage.id.clone();
                press_target()
                    .w_full()
                    .min_h(px(50.))
                    .px_4()
                    .py_2p5()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .cursor_pointer()
                    .active(|s| s.opacity(0.75))
                    .child(
                        text(stage.name)
                            .text_size(px(16.))
                            .font_weight(if is_current {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(if is_current { ACCENT } else { TEXT }),
                    )
                    .children(
                        is_current.then(|| icon_tinted(Icon::Check, ACCENT).w(px(18.)).h(px(18.))),
                    )
                    .on_press(move |_: &()| {
                        trigger_haptic(Haptic::Selection);
                        move_card(ui, &cid, &stage_id);
                        state.board_stage.set(stage_id.clone());
                        state.close_sheet();
                    })
                    .into_element()
            })
            .collect();

        let term_cid = card.id.clone();
        let edit_cid = card.id.clone();
        let resolved = card.resolved();
        let mut col = view()
            .w_full()
            .gap_4()
            .child(mobile_title(format!("#{} {}", card.number, card.title)));
        if !stage_rows.is_empty() {
            col = col.child(
                view()
                    .w_full()
                    .child(section_header("MOVE TO STAGE", None))
                    .child(hairline_list_inset(stage_rows, 16.0)),
            );
        }
        col.child(
            view()
                .w_full()
                .flex_row()
                .gap_2()
                .child(
                    mobile_inline_button("Edit", Variant::Secondary, move |_| {
                        state.open_sheet(MobileSheet::EditCard(edit_cid.clone()));
                    })
                    .flex_1(),
                )
                .child(
                    mobile_inline_button("Terminal", Variant::Secondary, move |_| {
                        state.close_sheet();
                        state.push_card(term_cid.clone(), CardSegment::Terminal);
                    })
                    .flex_1(),
                )
                .child(
                    mobile_inline_button(
                        if resolved { "Reopen" } else { "Resolve" },
                        Variant::Secondary,
                        move |_| {
                            if resolved {
                                crate::card::resolve_reopen(ui, card.clone());
                            } else {
                                crate::card::resolve_start(ui, card.clone());
                            }
                            state.close_sheet();
                        },
                    )
                    .flex_1(),
                ),
        )
        .into_element()
    })
}

pub(crate) fn create_card_sheet_body(
    ui: Ui,
    state: MobileState,
    project_id: String,
) -> impl IntoElement {
    let title = signal(String::new());
    let body = signal(String::new());
    let stages = untracked(|| {
        ui.snapshot.with_untracked(|s| {
            s.projects
                .iter()
                .find(|p| p.id == project_id)
                .map(|p| p.workflow.stages.clone())
                .unwrap_or_default()
        })
    });
    let profiles: Vec<String> = untracked(|| {
        ui.profiles
            .get_untracked()
            .into_iter()
            .filter(|p| p.enabled)
            .map(|p| p.id)
            .collect()
    });
    let initial_stage = stages
        .first()
        .map(|st| st.id.clone())
        .unwrap_or_else(|| "backlog".to_owned());
    let stage = signal(initial_stage);
    let assignee = signal(Option::<String>::None);
    let busy = signal(false);

    view()
        .w_full()
        .gap_4()
        .child(
            view()
                .w_full()
                .gap_1p5()
                .child(section_header("TITLE", None))
                .child(mobile_field(title, "Card title")),
        )
        .child(
            view()
                .w_full()
                .gap_1p5()
                .child(section_header("DESCRIPTION", None))
                .child(
                    nsh_base::input()
                        .value(move || body.get())
                        .on_change(move |v: &str| body.set(v.to_owned()))
                        .multiline(true)
                        .placeholder("Details…")
                        .min_h(px(104.))
                        .px_3p5()
                        .py_2p5()
                        .rounded(px(12.))
                        .bg(INSET)
                        .border_1()
                        .border_color(BORDER)
                        .text_size(px(16.))
                        .text_color(TEXT)
                        .focus(|s| s.border_color(ACCENT)),
                ),
        )
        .children((!stages.is_empty()).then(|| {
            view()
                .w_full()
                .gap_1p5()
                .child(section_header("STAGE", None))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .children(stages.into_iter().map(move |st| {
                            let sid = st.id.clone();
                            let sid_bg = sid.clone();
                            let sid_fg = sid;
                            press_target()
                                .h(px(36.))
                                .px_3p5()
                                .rounded_full()
                                .bg(move || {
                                    if stage.get() == sid_bg {
                                        SELECTED
                                    } else {
                                        SURFACE
                                    }
                                })
                                .items_center()
                                .justify_center()
                                .active(|s| s.opacity(0.75))
                                .child(
                                    text(st.name)
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(move || {
                                            if stage.get() == sid_fg { ACCENT } else { TEXT }
                                        }),
                                )
                                .on_press(move |_: &()| {
                                    trigger_haptic(Haptic::Selection);
                                    stage.set(st.id.clone());
                                })
                        })),
                )
        }))
        .children((!profiles.is_empty()).then(|| {
            view()
                .w_full()
                .gap_1p5()
                .child(section_header("ASSIGNEE", None))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            press_target()
                                .h(px(36.))
                                .px_3()
                                .rounded_full()
                                .bg(move || {
                                    if assignee.with(Option::is_none) {
                                        SELECTED
                                    } else {
                                        SURFACE
                                    }
                                })
                                .items_center()
                                .justify_center()
                                .active(|s| s.opacity(0.75))
                                .child(
                                    text("Unassigned")
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(move || {
                                            if assignee.with(Option::is_none) {
                                                ACCENT
                                            } else {
                                                MUTED
                                            }
                                        }),
                                )
                                .on_press(move |_: &()| {
                                    trigger_haptic(Haptic::Selection);
                                    assignee.set(None);
                                }),
                        )
                        .children(profiles.into_iter().map(move |pid| {
                            let pid_bg = pid.clone();
                            let pid_fg = pid.clone();
                            let pid_press = pid.clone();
                            press_target()
                                .h(px(36.))
                                .px_3()
                                .rounded_full()
                                .bg(move || {
                                    if assignee.with(|a| a.as_deref() == Some(pid_bg.as_str())) {
                                        SELECTED
                                    } else {
                                        SURFACE
                                    }
                                })
                                .items_center()
                                .justify_center()
                                .active(|s| s.opacity(0.75))
                                .child(
                                    text(format!("@{pid}"))
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(move || {
                                            if assignee
                                                .with(|a| a.as_deref() == Some(pid_fg.as_str()))
                                            {
                                                ACCENT
                                            } else {
                                                TEXT
                                            }
                                        }),
                                )
                                .on_press(move |_: &()| {
                                    trigger_haptic(Haptic::Selection);
                                    assignee.set(Some(pid_press.clone()));
                                })
                        })),
                )
        }))
        .child(
            mobile_button(
                move || {
                    if busy.get() {
                        "Creating…"
                    } else {
                        "Create Card"
                    }
                },
                Variant::Primary,
                move |_| {
                    let t = title.get_untracked().trim().to_owned();
                    if t.is_empty() || busy.get_untracked() {
                        return;
                    }
                    let b = body.get_untracked();
                    let st = stage.get_untracked();
                    let asg = assignee.get_untracked();
                    if !crate::backend::is_connected() {
                        let new_id = format!("card-{}", canban_types::id());
                        let new_card: Card = serde_json::from_value(json!({
                            "id": new_id,
                            "project_id": project_id,
                            "number": 99,
                            "title": t,
                            "body": b,
                            "stage": st,
                            "assignee": asg,
                            "revision": 1,
                            "created_at": canban_types::now(),
                        }))
                        .unwrap();
                        ui.snapshot.update(|s| s.cards.push(new_card));
                        state.board_stage.set(st);
                        state.close_sheet();
                        return;
                    }
                    busy.set(true);
                    let pid = project_id.clone();
                    let next_stage = st.clone();
                    let mut params = json!({"project_id": pid, "title": t, "body": b, "stage": st});
                    if let Some(a) = asg {
                        params["assignee"] = json!(a);
                    }
                    dispatch_complete(ui, "card.create", params, move |res| {
                        busy.try_set(false);
                        if let Ok(val) = res {
                            state.board_stage.set(next_stage.clone());
                            state.close_sheet();
                            if let Some(cid) = val["id"].as_str() {
                                state.push_card(cid.to_owned(), CardSegment::Conversation);
                            }
                        }
                    });
                },
            )
            .disabled(move || busy.get() || title.get().trim().is_empty()),
        )
}

pub(crate) fn project_picker_sheet_body(ui: Ui, state: MobileState) -> impl IntoElement {
    let query = signal(String::new());

    let grouped = computed(move || {
        let hosts = mobile_hosts(ui);
        let active_pid = ui.project.get();
        let needle = query.get().trim().to_lowercase();
        ui.snapshot.with(|s| {
            let count_active = |pid: &str| {
                s.cards
                    .iter()
                    .filter(|c| c.project_id == pid && c.active())
                    .count()
            };
            let matches = |host_name: &str, p: &Project| {
                needle.is_empty()
                    || p.name.to_lowercase().contains(&needle)
                    || p.directory.to_lowercase().contains(&needle)
                    || host_name.to_lowercase().contains(&needle)
            };

            type ProjectPickerGroup = (Option<HostSummary>, Vec<(Project, usize)>);
            let mut groups: Vec<ProjectPickerGroup> = Vec::new();
            for host in &hosts {
                let projects: Vec<(Project, usize)> = s
                    .projects
                    .iter()
                    .filter(|p| s.project_host(&p.id) == host.id && matches(&host.name, p))
                    .map(|p| (p.clone(), count_active(&p.id)))
                    .collect();
                if !projects.is_empty() {
                    groups.push((Some(host.clone()), projects));
                }
            }
            let unowned: Vec<(Project, usize)> = s
                .projects
                .iter()
                .filter(|p| {
                    let h = s.project_host(&p.id);
                    !hosts.iter().any(|known| known.id == h) && matches("", p)
                })
                .map(|p| (p.clone(), count_active(&p.id)))
                .collect();
            if !unowned.is_empty() {
                groups.push((None, unowned));
            }
            (active_pid, s.projects.len(), groups)
        })
    });

    view()
        .w_full()
        .gap_4()
        .child(mobile_field(query, "Search projects…"))
        .child(dynamic(move || {
            let (active_pid, total_projects, groups) = grouped.get();
            if total_projects == 0 {
                return empty_state(Icon::Projects, Tone::Neutral, "No Projects", "", None)
                    .into_element();
            }
            if groups.is_empty() {
                return empty_state(Icon::Search, Tone::Neutral, "No matches", "", None)
                    .into_element();
            }
            let mut col = view().w_full().gap_5();
            for (host_opt, projects) in groups {
                let header_el = if let Some(host) = host_opt {
                    let (conn_text, conn_tone) = super::chrome::host_connection_status(&host);
                    let badges = status(conn_text, conn_tone).into_element();
                    section_header(host.name.to_uppercase(), Some(badges))
                } else {
                    section_header("PROJECTS", None)
                };

                let rows: Vec<AnyElement> = projects
                    .into_iter()
                    .map(|(project, card_count)| {
                        let is_sel = project.id == active_pid;
                        let pid = project.id.clone();
                        press_target()
                            .w_full()
                            .min_h(px(56.))
                            .px_4()
                            .py_2p5()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .cursor_pointer()
                            .active(|s| s.opacity(0.75))
                            .child(
                                view()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .gap_0p5()
                                    .child(
                                        text(project.name)
                                            .text_size(px(16.))
                                            .font_weight(if is_sel {
                                                FontWeight::SEMIBOLD
                                            } else {
                                                FontWeight::MEDIUM
                                            })
                                            .text_color(if is_sel { ACCENT } else { TEXT })
                                            .truncate(),
                                    )
                                    .child(
                                        mobile_footnote(project.directory)
                                            .font_family(MONO_FONT)
                                            .truncate(),
                                    ),
                            )
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .flex_shrink_0()
                                    .child(mobile_footnote(format!("{card_count}")).tabular_nums())
                                    .children(is_sel.then(|| {
                                        icon_tinted(Icon::Check, ACCENT).w(px(18.)).h(px(18.))
                                    })),
                            )
                            .on_press(move |_: &()| {
                                super::switch_project(ui, state, pid.clone());
                                state.close_sheet();
                            })
                            .into_element()
                    })
                    .collect();

                col = col.child(
                    view()
                        .w_full()
                        .child(header_el)
                        .child(hairline_list_inset(rows, 16.0)),
                );
            }
            col.into_element()
        }))
}
