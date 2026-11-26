//! Board composition with native whole-Card dragging and durable placement.
pub mod activity;
mod columns;
mod drag;
mod model;
mod transitions;
use crate::presenter::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;
pub type DropHandler = std::rc::Rc<dyn Fn(Drop)>;
/// One placement intent, shared by production and fixture hosts.
#[derive(Clone, Debug)]
pub struct Drop {
    pub card_id: String,
    pub revision: u64,
    pub stage: String,
    pub before_card_id: Option<String>,
}
pub fn chrome_bar(ui: Ui) -> impl IntoElement {
    let project = computed(move || {
        let _ = ui.project.get();
        ui.current_project()
    });
    let project_controls = when(
        move || project.get().is_some(),
        move || {
            view()
                .flex_row()
                .items_center()
                .gap_0p5()
                .flex_shrink_0()
                .window_drag(false)
                .child(chrome_indicator_button(
                    Icon::GitHub,
                    move || {
                        if let Some(p) = project.get() {
                            if let Some(g) = &p.github {
                                if g.paused {
                                    format!("GitHub · {} (paused)", g.repository)
                                } else {
                                    format!("GitHub · Connected to {}", g.repository)
                                }
                            } else if let Some(d) = &p.detected_github {
                                format!("GitHub · Detected {} (not linked)", d.repository)
                            } else {
                                "GitHub repository & synchronization".into()
                            }
                        } else {
                            "GitHub repository & synchronization".into()
                        }
                    },
                    move || {
                        project
                            .get()
                            .and_then(|p| p.github)
                            .is_some_and(|g| !g.paused)
                    },
                    move |_| ui.navigate("github-repository"),
                ))
                .child(when(
                    move || project.get().is_some_and(|p| p.github.is_some()),
                    move || {
                        let gh_project = computed(move || {
                            project.get().filter(|p| p.github.is_some()).map(|p| p.id)
                        });
                        dynamic(move || {
                            if let Some(pid) = gh_project.get() {
                                crate::github::sync_button(ui, pid).into_element()
                            } else {
                                view().into_element()
                            }
                        })
                    },
                ))
        },
    );
    view()
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .overflow_hidden()
        .flex_row()
        .items_center()
        .gap_1p5()
        .pl_1()
        .window_drag(true)
        .child(activity::strip(ui, project_controls))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .flex_shrink_0()
                .window_drag(false)
                .child(when(
                    move || project.get().is_some() && !ui.narrow.get(),
                    move || {
                        compact_action("Agents", Variant::Quiet, move |_| {
                            ui.navigate("project-agents")
                        })
                    },
                ))
                .child(when(
                    move || project.get().is_some() && !ui.narrow.get(),
                    move || {
                        compact_action("Workflows", Variant::Quiet, move |_| {
                            ui.navigate("workflows")
                        })
                    },
                ))
                .child(when(
                    move || !ui.narrow.get(),
                    move || {
                        compact_action(
                            move || {
                                let _ = ui.draft_epoch.get();
                                let count = ui.drafts.with(|all| {
                                    all.iter()
                                        .filter(|(key, draft)| draft.has_content(key))
                                        .count()
                                });
                                if count == 0 {
                                    "Drafts".into()
                                } else {
                                    format!("Drafts ({count})")
                                }
                            },
                            Variant::Secondary,
                            move |_| ui.navigate("drafts"),
                        )
                    },
                ))
                .child(when(
                    move || project.get().is_some(),
                    move || compact_action("+ Add Card", Variant::Primary, move |_| ui.new_card()),
                ))
                .child(when(
                    move || project.get().is_some(),
                    move || {
                        let anchor = element_ref();
                        chrome_button(Icon::More, "Project actions", move |_| {
                            use nsh_core::element::desktop::{MenuItem, popup_menu_at};
                            ui.spawn(async move {
                                if let Some(bounds) = anchor.measure().await {
                                    popup_menu_at(
                                        vec![
                                            MenuItem::new("Workflows…").on_select(move || {
                                                ui.workflows.section.set("workflows".into());
                                                ui.navigate("workflows");
                                            }),
                                            MenuItem::new("Scheduled Tasks…").on_select(
                                                move || {
                                                    crate::workflows::open_schedules(ui, None);
                                                },
                                            ),
                                            MenuItem::new("Drafts…")
                                                .on_select(move || ui.navigate("drafts")),
                                            MenuItem::new("Archived Cards")
                                                .on_select(move || ui.navigate("archive")),
                                            MenuItem::new("Project Agents…")
                                                .on_select(move || ui.navigate("project-agents")),
                                            MenuItem::new("Work location…")
                                                .on_select(move || ui.navigate("project-location")),
                                            MenuItem::new("GitHub repository…").on_select(
                                                move || ui.navigate("github-repository"),
                                            ),
                                            MenuItem::separator(),
                                            MenuItem::new("New Managed Branch Worktree…")
                                                .on_select(move || {
                                                    ui.worktree_modal
                                                        .set(Some(ui.project.get_untracked()));
                                                }),
                                            MenuItem::separator(),
                                            MenuItem::new("Switch Project…")
                                                .on_select(move || ui.show_search(true)),
                                            MenuItem::new("New Project…")
                                                .on_select(move || ui.navigate("new-project")),
                                            MenuItem::separator(),
                                            MenuItem::new("Archive Project…").on_select(
                                                move || {
                                                    let pid = ui.project.get_untracked();
                                                    if !pid.is_empty() {
                                                        ui.project_archive_confirm.set(Some(pid));
                                                    }
                                                },
                                            ),
                                        ],
                                        point(
                                            bounds.page.x,
                                            bounds.page.y + bounds.frame.size.height,
                                        ),
                                    )
                                    .await;
                                }
                            });
                        })
                        .element_ref(&anchor)
                    },
                )),
        )
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let project = computed(move || {
        let _ = ui.project.get();
        ui.current_project()
    });
    view()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .p_4()
        .gap_3()
        .child(crate::github::filters::toolbar(ui))
        .child(dynamic(move || {
            let Some(project) = project.get() else {
                if !ui.snapshot_ready.get() {
                    return canban_ui_kit::loading_panel(
                        "Loading workspace…",
                        "Connecting to your Projects and Cards.",
                    )
                    .into_element();
                }
                let create = entry_card(
                    Icon::Plus,
                    "Create a Project",
                    "Organize Cards and Agent work around a source directory.",
                    move |_| ui.navigate("new-project"),
                );
                if !ui.onboarded.get() {
                    return view()
                        .gap_3()
                        .child(crate::agents_setup::onboarding_banner(ui))
                        .child(create)
                        .into_element();
                }
                return create.into_element();
            };
            untracked(|| {
                let stages = project.workflow.stages.clone();
                let width = signal(0f32);
                let compact = compact_mode(ui, width, stages.len());
                let state = drag::BoardDrag::new(ui, project.clone(), compact);
                let focus = focus_stage(ui, &stages);
                view()
                    .flex_1()
                    .min_h(px(0.))
                    .min_w(px(0.))
                    .gap_3()
                    .relative()
                    .element_ref(&state.root)
                    .on_layout(move |b: &Bounds<Points>| {
                        width.set(b.size.width.0);
                        state.dirty()
                    })
                    .child(when(move || compact.get(), {
                        let stages = stages.clone();
                        move || stage_tabs(ui, state, stages.clone(), focus)
                    }))
                    .child(
                        scroll()
                            .horizontal()
                            .element_ref(&state.horizontal)
                            .flex_1()
                            .min_h(px(0.))
                            .on_scroll(move |e: &ScrollEvent| state.horizontal_scroll(e.offset.x.0))
                            .child(
                                view()
                                    .flex_row()
                                    .gap_3()
                                    .pr_2()
                                    .min_h(px(0.))
                                    .flex_1()
                                    .on_layout(move |b: &Bounds<Points>| {
                                        state.content_width(b.size.width.0)
                                    })
                                    .child({
                                        let visible_stages = computed(move || {
                                            let is_compact = compact.get();
                                            if is_compact {
                                                let focused = focus.get();
                                                (
                                                    true,
                                                    stages
                                                        .iter()
                                                        .filter(|s| s.id == focused)
                                                        .cloned()
                                                        .collect::<Vec<_>>(),
                                                )
                                            } else {
                                                (false, stages.clone())
                                            }
                                        });
                                        dynamic(move || {
                                            let (compact, shown) = visible_stages.get();
                                            untracked(|| {
                                                state.retain(
                                                    &shown
                                                        .iter()
                                                        .map(|s| s.id.clone())
                                                        .collect::<Vec<_>>(),
                                                );
                                                view()
                                                    .flex_row()
                                                    .gap_3()
                                                    .flex_1()
                                                    .min_h(px(0.))
                                                    .children(
                                                        shown
                                                            .into_iter()
                                                            .map(|stage| {
                                                                columns::column(
                                                                    ui, state, stage, compact,
                                                                )
                                                                .into_element()
                                                            })
                                                            .collect::<Vec<_>>(),
                                                    )
                                                    .into_element()
                                            })
                                        })
                                    }),
                            ),
                    )
                    .child(state.overlay())
                    .into_element()
            })
        }))
}

/// With the Card pane open beside the Board and too little room for every
/// column, show one stage at a time (stage tabs) instead of clipped columns.
fn compact_mode(ui: Ui, width: Signal<f32>, stages: usize) -> Memo<bool> {
    computed(move || {
        let page = ui.page.get();
        let pane = (page == "new-card" || page == "card" && !ui.card.get().is_empty())
            && !ui.narrow.get()
            && !ui.card_standalone.get();
        let needed =
            stages as f32 * columns::COLUMN_MIN + stages.saturating_sub(1) as f32 * 12. + 8.;
        let width = width.get();
        pane && stages > 1 && width > 0. && width < needed
    })
}

/// The compact stage follows the opened Card; tabs can switch it explicitly.
fn focus_stage(ui: Ui, stages: &[canban_types::Stage]) -> Signal<String> {
    let first = stages.first().map(|s| s.id.clone()).unwrap_or_default();
    let ids: Vec<_> = stages.iter().map(|s| s.id.clone()).collect();
    let focus = signal(first);
    effect(move || {
        let id = ui.card.get();
        let stage = ui
            .index
            .with_untracked(|idx| idx.cards.get(&id).map(|c| c.stage.clone()));
        if let Some(stage) = stage.filter(|s| ids.contains(s)) {
            focus.set(stage);
        }
    });
    focus
}

fn stage_tabs(
    ui: Ui,
    state: drag::BoardDrag,
    stages: Vec<canban_types::Stage>,
    focus: Signal<String>,
) -> impl IntoElement {
    let tab_counts = computed(move || {
        let cards = state.cards.get();
        let pid = ui.project.get();
        ui.index.with(|idx| {
            stages
                .iter()
                .map(|stage| {
                    let visible = cards.iter().filter(|c| c.stage == stage.id).count();
                    let total = idx
                        .cards
                        .values()
                        .filter(|c| c.project_id == pid && c.active() && c.stage == stage.id)
                        .count();
                    let title = if visible == total {
                        format!("{} {visible}", stage.name)
                    } else {
                        format!("{} {visible} / {total}", stage.name)
                    };
                    (stage.id.clone(), title)
                })
                .collect::<Vec<_>>()
        })
    });
    dynamic(move || {
        let items = tab_counts.get();
        segmented().children(
            items
                .into_iter()
                .map(|(id, title)| {
                    let selected = id.clone();
                    compact_segment(
                        title,
                        None,
                        move || focus.get() == selected,
                        move |_| focus.set(id.clone()),
                    )
                    .into_element()
                })
                .collect::<Vec<_>>(),
        )
    })
}
