//! Board composition; stage scroll positions survive focused work and refreshes.
use crate::{components::*, drafts::*, presenter::*};
use canban_types::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;

pub fn matches_filter(s: &Snapshot, c: &Card, filter: &str) -> bool {
    match filter {
        "attention" => c.attention.is_some(),
        "running" => s.requests.iter().any(|r| {
            r.card_id == c.id
                && matches!(
                    r.state.as_str(),
                    "queued" | "starting" | "running" | "working"
                )
        }),
        "review" => c.stage == "review",
        _ => true,
    }
}
pub fn screen(ui: Ui) -> impl IntoElement {
    let has_project = computed(move || {
        let _ = ui.project.get();
        ui.current_project().is_some()
    });
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
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
                    move || has_project.get(),
                    move || {
                        action("Work location…", Variant::Quiet, move |_| {
                            ui.navigate("project-location")
                        })
                    },
                ))
                .child(when(
                    move || has_project.get(),
                    move || {
                        action("+ Add Card", Variant::Primary, move |_| {
                            ui.source_note.set(String::new());
                            ui.navigate("new-card");
                        })
                        .flex_shrink_0()
                    },
                )),
        )
        .child(when(
            move || has_project.get(),
            move || crate::components::scheduling::summary(ui),
        ))
        .child(when(
            move || has_project.get(),
            move || {
                view().flex_row().flex_wrap().gap_2().children(
                    [
                        ("All", "all"),
                        ("Needs attention", "attention"),
                        ("Running", "running"),
                        ("Review", "review"),
                    ]
                    .into_iter()
                    .map(|(name, key)| {
                        navigation(
                            name.into(),
                            move || ui.board_filter.get() == key,
                            move |_| ui.board_filter.set(key.into()),
                        )
                        .into_element()
                    })
                    .collect::<Vec<_>>(),
                )
            },
        ))
        .child(dynamic(move || {
            let s = ui.snapshot.get();
            let filter = ui.board_filter.get();
            let project = ui.project.get();
            let Some(p) = s.projects.iter().find(|p| p.id == project) else {
                return entry_card(
                    Icon::Plus,
                    "Create a Project",
                    "Organize Cards, Notes and Agent work around a source directory.",
                    move |_| ui.navigate("new-project"),
                )
                .into_element();
            };
            let stages = p
                .workflow
                .stages
                .iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>();
            let width = signal(240.0);
            let count = stages.len();
            scroll()
                .horizontal()
                .flex_1()
                .min_h(px(0.0))
                .child(
                    view()
                        .flex_row()
                        .gap_3()
                        .min_h(px(0.0))
                        .flex_1()
                        .on_layout(move |b: &Bounds<Points>| {
                            width.set(
                                (b.size.width.0 - 12.0 * (count.saturating_sub(1)) as f32)
                                    / (count.max(1) as f32),
                            )
                        })
                        .children(
                            p.workflow
                                .stages
                                .iter()
                                .enumerate()
                                .map(|(i, stage)| {
                                    let cards = s
                                        .cards
                                        .iter()
                                        .filter(|c| {
                                            c.project_id == project
                                                && c.stage == stage.id
                                                && c.archived_at.is_none()
                                                && matches_filter(&s, c, &filter)
                                        })
                                        .cloned()
                                        .collect::<Vec<_>>();
                                    let key = format!("board:{project}:{}", stage.id);
                                    let position = scroll_position(ui, &key);
                                    let explanation = if stage.execute {
                                        stage
                                            .profile
                                            .as_ref()
                                            .map(|p| format!("Workflow Agent: @{p}"))
                                            .unwrap_or_else(|| {
                                                "Workflow Agent selected by tags or project default"
                                                    .into()
                                            })
                                    } else {
                                        "No automatic Agent execution in this stage".into()
                                    };
                                    let mut column = view()
                                        .w(px(240.))
                                        .flex_1()
                                        .min_w(px(200.))
                                        .min_h(px(0.))
                                        .gap_3()
                                        .child(
                                            view()
                                                .flex_row()
                                                .justify_between()
                                                .items_center()
                                                .tooltip(
                                                    nsh_base::tooltip().child(label(explanation)),
                                                )
                                                .child(
                                                    label(stage.name.clone())
                                                        .font_weight(FontWeight::SEMIBOLD),
                                                )
                                                .child(badge(cards.len().to_string())),
                                        );
                                    let mut items = view().gap_3();
                                    if cards.is_empty() {
                                        items = items.child(muted("No cards"));
                                    }
                                    items = items.children(
                                        cards
                                            .into_iter()
                                            .map(|c| {
                                                card_tile(ui, c, i, width, stages.clone())
                                                    .into_element()
                                            })
                                            .collect::<Vec<_>>(),
                                    );
                                    column = column.child(
                                        scroll()
                                            .key(key.clone())
                                            .content_offset(position)
                                            .on_scroll(move |e: &ScrollEvent| {
                                                capture_scroll(ui, key.clone(), e)
                                            })
                                            .flex_1()
                                            .child(items),
                                    );
                                    column.into_element()
                                })
                                .collect::<Vec<_>>(),
                        ),
                )
                .into_element()
        }))
}

/// Board owns Project selection/creation; the rail only navigates surfaces.
fn project_picker(ui: Ui) -> impl IntoElement {
    use canban_ui_kit::tokens::*;
    view()
        .flex_1()
        .min_w(px(0.))
        .gap_1()
        .child(muted("Board"))
        .child(
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
