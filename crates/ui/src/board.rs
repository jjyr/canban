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
                .child(project_picker(ui))
                .child(when(
                    move || project.get().is_some(),
                    move || {
                        action("Work location…", Variant::Quiet, move |_| {
                            ui.navigate("project-location")
                        })
                    },
                ))
                .child(when(
                    move || project.get().is_some(),
                    move || {
                        action("+ Add Card", Variant::Primary, move |_| ui.new_card())
                            .flex_shrink_0()
                    },
                )),
        )
        .child(dynamic(move || {
            let Some(project) = project.get() else {
                return entry_card(
                    Icon::Plus,
                    "Create a Project",
                    "Organize Cards and Agent work around a source directory.",
                    move |_| ui.navigate("new-project"),
                )
                .into_element();
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
    view().flex_1().min_w(px(0.)).child(
        nsh_base::button()
            .min_h(px(40.))
            .w_full()
            .min_w(px(0.))
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
                .flex_1()
                .truncate()
                .min_w(px(0.)),
            )
            .child(icon(Icon::ChevronDown))
            .on_press(move |_: &()| ui.show_search(true)),
    )
}
