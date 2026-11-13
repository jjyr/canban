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
        .gap_4()
        .child(
            view()
                .flex_row()
                .justify_between()
                .items_center()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .flex_1()
                        .min_w(px(100.))
                        .child(project_picker(ui))
                        .child(when(
                            move || project.get().is_some(),
                            move || {
                                view().flex_shrink_0().child(icon_action(
                                    Icon::GitHub,
                                    "GitHub repository & synchronization".into(),
                                    move || project.get().is_some_and(|p| p.github.is_some()),
                                    || 0,
                                    move |_| ui.navigate("github-repository"),
                                ))
                            },
                        )),
                )
                .child(when(
                    move || project.get().is_some(),
                    move || {
                        action(
                            move || {
                                let n = project.get().map(|p| p.agent_overrides.len()).unwrap_or(0);
                                if n == 0 {
                                    "Agents".into()
                                } else {
                                    format!("Agents ({n})")
                                }
                            },
                            Variant::Quiet,
                            move |_| ui.navigate("project-agents"),
                        )
                        .flex_shrink_0()
                    },
                ))
                .child(when(
                    move || project.get().is_some(),
                    move || {
                        action("Workflows", Variant::Quiet, move |_| {
                            ui.navigate("workflows")
                        })
                        .flex_shrink_0()
                    },
                ))
                .child(
                    action(
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
                    .flex_shrink_0(),
                )
                .child(when(
                    move || project.get().is_some(),
                    move || {
                        action("+ Add Card", Variant::Primary, move |_| ui.new_card())
                            .flex_shrink_0()
                    },
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
        .child(when(
            move || has_github.get(),
            move || crate::github::filters::toolbar(ui),
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
                view()
                    .flex_1()
                    .min_h(px(0.))
                    .min_w(px(0.))
                    .relative()
                    .element_ref(&state.root)
                    .on_layout(move |_: &Bounds<Points>| state.dirty())
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
                                    .min_h(px(0.))
                                    .flex_1()
                                    .on_layout(move |b: &Bounds<Points>| {
                                        state.content_width(b.size.width.0)
                                    })
                                    .children(
                                        project
                                            .workflow
                                            .stages
                                            .into_iter()
                                            .map(|stage| {
                                                columns::column(ui, state, stage).into_element()
                                            })
                                            .collect::<Vec<_>>(),
                                    ),
                            ),
                    )
                    .child(state.overlay())
                    .into_element()
            })
        }))
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
    view().min_w(px(80.)).max_w(px(560.)).child(
        canban_ui_kit::press_target()
            .min_h(px(40.))
            .min_w(px(80.))
            .max_w(px(560.))
            .flex_row()
            .items_center()
            .gap_2()
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
                heading(move || {
                    let _ = ui.project.get();
                    ui.current_project()
                        .map(|p| p.name)
                        .unwrap_or_else(|| "Choose Project".into())
                })
                .truncate()
                .min_w(px(0.)),
            )
            .child(canban_ui_kit::reminder_count(
                move || updates.get().0,
                move || updates.get().1,
            ))
            .child(icon(Icon::ChevronDown).flex_shrink_0())
            .on_press(move |_: &()| ui.show_search(true)),
    )
}
