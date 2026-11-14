//! Board composition with native whole-Card dragging and durable placement.
pub mod activity;
mod columns;
mod drag;
mod model;
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
    view()
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .flex_row()
        .items_center()
        .gap_1p5()
        .pl_2()
        .window_drag(true)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_0p5()
                .min_w(px(80.))
                .max_w(px(280.))
                .window_drag(false)
                .child(project_picker(ui))
                .child(when(
                    move || project.get().is_some(),
                    move || {
                        chrome_button(
                            Icon::GitHub,
                            "GitHub repository & synchronization",
                            move |_| ui.navigate("github-repository"),
                        )
                    },
                )),
        )
        .child(activity::strip(ui))
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
                                            MenuItem::new("Workflows…")
                                                .on_select(move || ui.navigate("workflows")),
                                            MenuItem::new("Drafts…")
                                                .on_select(move || ui.navigate("drafts")),
                                            MenuItem::new("Archive")
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
    let has_github = computed(move || {
        let current = ui.project.get();
        project.get().is_some_and(|p| p.github.is_some())
            || ui.snapshot.with(|s| {
                s.cards
                    .iter()
                    .any(|c| c.project_id == current && c.github_issue().is_some())
            })
    });
    view()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .p_4()
        .gap_3()
        .child(when(
            move || has_github.get(),
            move || crate::github::filters::toolbar(ui),
        ))
        .child(when(
            move || {
                let _ = ui.github.filters.get();
                !has_github.get()
                    && !crate::tags::active_board_tags(ui, &ui.project.get()).is_empty()
            },
            move || crate::github::filters::tag_bar(ui),
        ))
        .child(dynamic(move || {
            let Some(project) = project.get() else {
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
                let state = drag::BoardDrag::new(ui, project.clone());
                let stages = project.workflow.stages.clone();
                let width = signal(0f32);
                let compact = compact_mode(ui, width, stages.len());
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
                        move || stage_tabs(state, stages.clone(), focus)
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
                                    .child(dynamic(move || {
                                        let compact = compact.get();
                                        let focus = focus.get();
                                        let shown: Vec<_> = stages
                                            .iter()
                                            .filter(|s| !compact || s.id == focus)
                                            .cloned()
                                            .collect();
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
                                    })),
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
            .snapshot
            .with_untracked(|s| s.cards.iter().find(|c| c.id == id).map(|c| c.stage.clone()));
        if let Some(stage) = stage.filter(|s| ids.contains(s)) {
            focus.set(stage);
        }
    });
    focus
}

fn stage_tabs(
    state: drag::BoardDrag,
    stages: Vec<canban_types::Stage>,
    focus: Signal<String>,
) -> impl IntoElement {
    dynamic(move || {
        let cards = state.cards.get();
        segmented().children(
            stages
                .iter()
                .map(|stage| {
                    let id = stage.id.clone();
                    let selected = id.clone();
                    let count = id.clone();
                    compact_segment(
                        format!(
                            "{} {}",
                            stage.name,
                            cards.iter().filter(|c| c.stage == count).count()
                        ),
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

/// Board owns Project selection/creation; the rail only navigates surfaces.
fn project_picker(ui: Ui) -> impl IntoElement {
    use canban_ui_kit::tokens::*;
    let updates = computed(move || {
        let project = ui.project.get();
        let items = ui.reminders.get();
        let cards = items
            .iter()
            .filter(|i| i.project_id == project)
            .map(|i| &i.card_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        (
            cards,
            items.iter().any(|i| i.project_id == project && i.pending),
        )
    });
    view().min_w(px(72.)).max_w(px(240.)).child(
        canban_ui_kit::press_target()
            .h(px(28.))
            .min_w(px(72.))
            .max_w(px(240.))
            .flex_row()
            .items_center()
            .gap_1p5()
            .px_2()
            .rounded(px(RADIUS))
            .border_1()
            .border_color(CLEAR)
            .hover(|s| s.bg(INSET))
            .focus(|s| s.border_color(ACCENT))
            .accessibility_label(move || {
                let _ = ui.project.get();
                Some(
                    format!(
                        "Switch Project · {}",
                        ui.current_project()
                            .map(|p| p.name)
                            .unwrap_or_else(|| "Choose Project".into())
                    )
                    .into(),
                )
            })
            .child(
                label(move || {
                    let _ = ui.project.get();
                    ui.current_project()
                        .map(|p| p.name)
                        .unwrap_or_else(|| "Choose Project".into())
                })
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(13.5))
                .truncate()
                .min_w(px(0.)),
            )
            .child(canban_ui_kit::reminder_count(
                move || updates.get().0,
                move || updates.get().1,
            ))
            .child(
                icon(Icon::ChevronDown)
                    .w(px(13.))
                    .h(px(13.))
                    .flex_shrink_0(),
            )
            .on_press(move |_: &()| ui.show_search(true)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{Project, ProjectAgentOverride, Snapshot, Workflow};
    use nsh_core::{ApproxTextEngine, Core, MountItem, geometry::size};
    use std::collections::BTreeMap;

    #[test]
    fn board_chrome_bar_renders_plain_agents_label_without_override_count() {
        #[component]
        fn ProbeRoot() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let mut snapshot = Snapshot::default();
            snapshot.projects.push(Project {
                id: "proj-1".into(),
                name: "Canban".into(),
                directory: "/tmp/canban".into(),
                location: None,
                github: None,
                delivery_mode: Default::default(),
                agent_overrides: BTreeMap::from([
                    (
                        "developer".into(),
                        ProjectAgentOverride {
                            agent: Some("agy".into()),
                            model: Some("gemini-4-carbon-high".into()),
                            effort: Some("high".into()),
                        },
                    ),
                    (
                        "reviewer".into(),
                        ProjectAgentOverride {
                            agent: None,
                            model: Some("gpt-5.4".into()),
                            effort: Some("medium".into()),
                        },
                    ),
                ]),
                tags: Vec::new(),
                workflow: Workflow::default(),
                target_branch: Some("main".into()),
                created_at: 1,
            });
            ui.snapshot.set(snapshot);
            ui.project.set("proj-1".into());
            ui.narrow.set(false);
            view().w(px(1200.)).h(px(42.)).child(chrome_bar(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(42.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<ProbeRoot>());
        let batch = core.commit().expect("initial mount batch");
        let texts: Vec<String> = batch
            .items
            .into_iter()
            .filter_map(|item| match item {
                MountItem::Text { text, .. } => Some(text.text.to_string()),
                _ => None,
            })
            .collect();

        assert!(
            texts.iter().any(|t| t == "Agents"),
            "expected Board chrome bar to display plain `Agents`, got {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.starts_with("Agents (")),
            "Board chrome bar must not append override count in parentheses, got {texts:?}"
        );
    }
}
