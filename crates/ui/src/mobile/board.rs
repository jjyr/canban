//! Mobile `Board` tab: project picker pill, stage strip, vertical card list,
//! long-press `Move to…` sheet, and offline-capable Card creation.
use super::{
    CardSegment, MobileSheet, MobileState,
    chrome::{offline_sync_banner, security_badge, trigger_haptic},
    host_security, mobile_hosts,
};
use crate::{
    components::card_tile::{CardTileModel, card_tile_from_model},
    presenter::*,
};
use canban_types::{Card, Project, Stage};
use canban_ui_kit::{pull_refresh_scroll, tokens::*, *};
use nsh_core::{Haptic, HapticImpact, prelude::*};
use serde_json::json;

#[derive(Clone, PartialEq)]
struct BoardHeaderModel {
    project_id: String,
    project_name: String,
    host_id: String,
    stages: Vec<(String, String, usize)>, // (id, name, count)
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
                project_name: "Select Project".into(),
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

pub(crate) fn screen(ui: Ui, state: MobileState) -> impl IntoElement {
    let scroll_ref = element_ref();
    let header_model = computed(move || build_board_header(ui));

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

    let card_models = computed(move || {
        let hm = header_model.get();
        let stage = active_stage.get();
        if hm.project_id.is_empty() {
            return Vec::new();
        }
        let cards: Vec<Card> = ui.snapshot.with(|s| {
            s.cards
                .iter()
                .filter(|c| c.project_id == hm.project_id && c.active() && c.stage == stage)
                .cloned()
                .collect()
        });
        cards
            .into_iter()
            .map(|c| CardTileModel::build(ui, c))
            .collect::<Vec<_>>()
    });

    let top_bar = dynamic(move || {
        let hm = header_model.get();
        let pid_create = hm.project_id.clone();
        let has_proj = !hm.project_id.is_empty();
        view()
            .w_full()
            .px_3()
            .py_2p5()
            .border_b_1()
            .border_color(BORDER)
            .bg(SURFACE)
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                press_target()
                    .flex_1()
                    .min_w(px(0.))
                    .h(px(34.))
                    .px_3()
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(BORDER)
                    .bg(INSET)
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .cursor_pointer()
                    .accessibility_label(Some(SharedString::from("Switch project")))
                    .child(
                        label(hm.project_name)
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .min_w(px(0.)),
                    )
                    .child(icon(Icon::ChevronDown).w(px(14.)).h(px(14.)))
                    .on_press(move |_: &()| {
                        trigger_haptic(Haptic::Selection);
                        state.open_sheet(MobileSheet::ProjectPicker);
                    }),
            )
            .child(
                compact_action("+ Card", Variant::Primary, move |_| {
                    if has_proj {
                        trigger_haptic(Haptic::Selection);
                        state.open_sheet(MobileSheet::CreateCard(pid_create.clone()));
                    }
                })
                .disabled(move || !has_proj),
            )
    });

    let stage_strip = dynamic(move || {
        let hm = header_model.get();
        let cur = active_stage.get();
        if hm.stages.is_empty() {
            return view();
        }
        view()
            .w_full()
            .h(px(46.))
            .flex_shrink_0()
            .border_b_1()
            .border_color(BORDER)
            .bg(SURFACE)
            .child(
                scroll().horizontal().scrollbar(false).size_full().child(
                    view()
                        .h_full()
                        .px_3()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .children(hm.stages.into_iter().map(|(id, name, count)| {
                            let selected = id == cur;
                            let next_id = id.clone();
                            press_target()
                                .h(px(30.))
                                .px_3()
                                .rounded_full()
                                .border_1()
                                .border_color(if selected { ACCENT } else { BORDER })
                                .bg(if selected { SELECTED } else { INSET })
                                .flex_row()
                                .items_center()
                                .gap_1p5()
                                .cursor_pointer()
                                .child(
                                    text(format!("{name} {count}"))
                                        .text_size(px(12.5))
                                        .font_weight(if selected {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::MEDIUM
                                        })
                                        .text_color(if selected { ACCENT } else { TEXT }),
                                )
                                .on_press(move |_: &()| {
                                    trigger_haptic(Haptic::Selection);
                                    state.board_stage.set(next_id.clone());
                                })
                        })),
                ),
            )
    });

    let list_body = pull_refresh_scroll(
        scroll().child(view().w_full().p_3().gap_2p5().child(dynamic(move || {
            let models = card_models.get();
            if models.is_empty() {
                return panel()
                    .p_5()
                    .gap_2()
                    .items_center()
                    .child(heading("No Cards in this stage"))
                    .child(muted("Tap + Card to create one, or switch stages above."))
                    .into_element();
            }
            view()
                .w_full()
                .gap_2p5()
                .children(models.into_iter().map(|model| {
                    let card_id = model.card.id.clone();
                    let open_id = card_id.clone();
                    let move_id = card_id.clone();
                    view()
                        .w_full()
                        .rounded(px(RADIUS))
                        .border_1()
                        .border_color(BORDER)
                        .bg(SURFACE)
                        .cursor_pointer()
                        .on_press(move |_: &PressEvent| {
                            trigger_haptic(Haptic::Selection);
                            state.push_card(open_id.clone(), CardSegment::Conversation);
                        })
                        .on_long_press(move |_: &PressEvent| {
                            trigger_haptic(Haptic::Impact(HapticImpact::Medium));
                            state.open_sheet(MobileSheet::MoveCard(move_id.clone()));
                        })
                        .child(card_tile_from_model(ui, model))
                }))
                .into_element()
        }))),
        scroll_ref,
        move || ui.refreshing.get(),
        move || refresh(ui),
    );

    let scoped_banner = dynamic(move || {
        let hid = header_model.get().host_id;
        let scope = (!hid.is_empty()).then_some(hid);
        offline_sync_banner(ui, state, scope)
    });

    view()
        .size_full()
        .flex_col()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(
            view()
                .w_full()
                .px_4()
                .pt_3()
                .pb_1()
                .bg(SURFACE)
                .child(heading("Board")),
        )
        .child(top_bar)
        .child(stage_strip)
        .child(scoped_banner)
        .child(list_body)
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
            return muted("Card no longer available").into_element();
        };
        let mut col = view()
            .w_full()
            .gap_3()
            .child(
                label(format!("#{} {}", card.number, card.title)).font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                label("MOVE TO STAGE")
                    .text_size(px(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(MUTED),
            );

        for stage in stages {
            let is_current = stage.id == card.stage;
            let cid = card.id.clone();
            let stage_id = stage.id.clone();
            col = col.child(
                press_target()
                    .w_full()
                    .px_3()
                    .py_2p5()
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(if is_current { ACCENT } else { BORDER })
                    .bg(if is_current { SELECTED } else { INSET })
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .child(label(stage.name))
                    .children(is_current.then(|| status("Current", Tone::Info)))
                    .on_press(move |_: &()| {
                        trigger_haptic(Haptic::Selection);
                        move_card(ui, &cid, &stage_id);
                        state.board_stage.set(stage_id.clone());
                        state.close_sheet();
                    }),
            );
        }

        let term_cid = card.id.clone();
        col = col.child(
            view()
                .pt_2()
                .flex_row()
                .flex_wrap()
                .gap_2()
                .child(action("Open Terminal", Variant::Secondary, move |_| {
                    state.close_sheet();
                    state.push_card(term_cid.clone(), CardSegment::Terminal);
                }))
                .child(action(
                    if card.resolved() { "Reopen" } else { "Resolve" },
                    Variant::Quiet,
                    move |_| {
                        if card.resolved() {
                            crate::card::resolve_reopen(ui, card.clone());
                        } else {
                            crate::card::resolve_start(ui, card.clone());
                        }
                        state.close_sheet();
                    },
                )),
        );

        col.into_element()
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
    let initial_stage = stages
        .first()
        .map(|st| st.id.clone())
        .unwrap_or_else(|| "backlog".to_owned());
    let stage = signal(initial_stage);
    let busy = signal(false);

    view()
        .w_full()
        .gap_3()
        .child(label("Title"))
        .child(field(title, "What needs doing?"))
        .child(label("Description"))
        .child(
            nsh_base::input()
                .value(move || body.get())
                .on_change(move |v: &str| body.set(v.to_owned()))
                .multiline(true)
                .placeholder("Context, acceptance criteria, or links…")
                .min_h(px(96.))
                .px_3()
                .py_2()
                .rounded(px(RADIUS))
                .bg(INSET)
                .border_1()
                .border_color(BORDER)
                .text_size(px(13.))
                .text_color(TEXT),
        )
        .children((!stages.is_empty()).then(|| {
            view().w_full().gap_1p5().child(label("Stage")).child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap_1p5()
                    .children(stages.into_iter().map(move |st| {
                        let sid = st.id.clone();
                        let sid_press = sid.clone();
                        let active = move || stage.get() == sid;
                        compact_segment(st.name, None, active, move |_| {
                            stage.set(sid_press.clone())
                        })
                    })),
            )
        }))
        .child(
            action(
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
                    trigger_haptic(Haptic::Selection);
                    if !crate::backend::is_connected() {
                        let new_id = format!("card-{}", canban_types::id());
                        let new_card: Card = serde_json::from_value(json!({
                            "id": new_id,
                            "project_id": project_id,
                            "number": 99,
                            "title": t,
                            "body": b,
                            "stage": st,
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
                    dispatch_complete(
                        ui,
                        "card.create",
                        json!({"project_id": pid, "title": t, "body": b, "stage": st}),
                        move |res| {
                            busy.try_set(false);
                            if let Ok(val) = res {
                                state.board_stage.set(next_stage.clone());
                                state.close_sheet();
                                if let Some(cid) = val["id"].as_str() {
                                    state.push_card(cid.to_owned(), CardSegment::Conversation);
                                }
                            }
                        },
                    );
                },
            )
            .disabled(move || busy.get() || title.get().trim().is_empty()),
        )
}

pub(crate) fn project_picker_sheet_body(ui: Ui, state: MobileState) -> impl IntoElement {
    let grouped = computed(move || {
        let hosts = mobile_hosts(ui);
        let active_pid = ui.project.get();
        ui.snapshot.with(|s| {
            let mut groups: Vec<(Option<canban_types::HostSummary>, Vec<Project>)> = Vec::new();
            for host in &hosts {
                let projects: Vec<Project> = s
                    .projects
                    .iter()
                    .filter(|p| s.project_host(&p.id) == host.id)
                    .cloned()
                    .collect();
                if !projects.is_empty() {
                    groups.push((Some(host.clone()), projects));
                }
            }
            let unowned: Vec<Project> = s
                .projects
                .iter()
                .filter(|p| {
                    let h = s.project_host(&p.id);
                    !hosts.iter().any(|known| known.id == h)
                })
                .cloned()
                .collect();
            if !unowned.is_empty() {
                groups.push((None, unowned));
            }
            (active_pid, groups)
        })
    });

    dynamic(move || {
        let (active_pid, groups) = grouped.get();
        if groups.is_empty() {
            return panel()
                .p_4()
                .gap_2()
                .child(label("No Projects yet"))
                .child(muted("Connect a machine or create a Project on desktop."))
                .into_element();
        }
        let mut col = view().w_full().gap_4();
        for (host_opt, projects) in groups {
            let mut sec = view().w_full().gap_2();
            if let Some(host) = host_opt {
                let sec_type = host_security(ui, &host);
                let (conn_text, conn_tone) = super::chrome::host_connection_status(&host);
                sec = sec.child(
                    view()
                        .w_full()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(label(host.name).font_weight(FontWeight::SEMIBOLD))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_1p5()
                                .child(security_badge(sec_type))
                                .child(status(conn_text, conn_tone)),
                        ),
                );
            }
            for project in projects {
                let is_sel = project.id == active_pid;
                let pid = project.id.clone();
                sec = sec.child(
                    press_target()
                        .w_full()
                        .px_3()
                        .py_2p5()
                        .rounded(px(RADIUS))
                        .border_1()
                        .border_color(if is_sel { ACCENT } else { BORDER })
                        .bg(if is_sel { SELECTED } else { INSET })
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .cursor_pointer()
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.))
                                .child(label(project.name).font_weight(FontWeight::MEDIUM))
                                .child(muted(project.directory).truncate()),
                        )
                        .children(
                            is_sel.then(|| icon_tinted(Icon::Check, ACCENT).w(px(16.)).h(px(16.))),
                        )
                        .on_press(move |_: &()| {
                            trigger_haptic(Haptic::Selection);
                            ui.project.set(pid.clone());
                            state.board_stage.set(String::new());
                            state.close_sheet();
                        }),
                );
            }
            col = col.child(sec);
        }
        col.into_element()
    })
}
