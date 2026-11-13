//! Board-only window chrome. No background work or durable acknowledgement.
pub(crate) mod model;
use crate::{presenter::*, quick_open};
use canban_ui_kit::{tokens::*, *};
use model::Summary;
use nsh_core::prelude::*;

pub fn strip(ui: Ui) -> impl IntoElement {
    let width = signal(0_f32);
    let rows = computed(move || {
        let current = ui.project.get();
        model::projects(&ui.snapshot.get(), &ui.recent_destinations.get())
            .into_iter()
            .filter(|r| r.id != current)
            .collect::<Vec<_>>()
    });
    let order = signal(Vec::<String>::new());
    effect(move || {
        let next = model::stable_ids(&order.get_untracked(), &rows.get());
        if next != order.get_untracked() {
            order.set(next);
        }
    });
    let ordered = computed(move || {
        let rows = rows.get();
        order
            .get()
            .iter()
            .filter_map(|id| rows.iter().find(|r| &r.id == id).cloned())
            .collect::<Vec<_>>()
    });
    let visible = computed(move || model::visible_count(&ordered.get(), width.get()));
    view()
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .flex_row()
        .items_center()
        .gap_1()
        .pl_3()
        .on_layout(move |b: &Bounds<Points>| width.set(b.size.width.0))
        .child(each(
            move || {
                ordered
                    .get()
                    .into_iter()
                    .take(visible.get())
                    .collect::<Vec<_>>()
            },
            |r| r.id.clone(),
            move |row, _| chip(ui, row),
        ))
        .child(when(
            move || rows.get().len() > visible.get(),
            move || {
                view().window_drag(false).child(
                    canban_ui_kit::press_target()
                        .h(px(28.))
                        .w(move || {
                            px(if visible.get() == 0 {
                                model::COMPACT_OVERFLOW
                            } else {
                                model::OVERFLOW
                            })
                        })
                        .px_2()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .bg(CLEAR)
                        .hover(|s| s.bg(INSET))
                        .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                        .accessibility_label(move || {
                            Some(
                                format!("More projects · {}", rows.get().len() - visible.get())
                                    .into(),
                            )
                        })
                        .flex_row()
                        .gap_1()
                        .child(
                            muted(move || {
                                let hidden = rows.get().len() - visible.get();
                                if visible.get() == 0 {
                                    format!("Projects · {hidden}")
                                } else {
                                    format!("+{hidden}")
                                }
                            })
                            .text_size(px(12.)),
                        )
                        .child(icon(Icon::ChevronDown).w(px(12.)).h(px(12.)))
                        .on_press(move |_: &()| ui.show_search(true)),
                )
            },
        ))
        .child(view().flex_1().min_w(px(64.)).h_full().window_drag(true))
}
fn chip(ui: Ui, row: ReadSignal<Summary>) -> impl IntoElement {
    view().window_drag(false).flex_shrink_0().child(
        canban_ui_kit::press_target()
            .h(px(26.))
            .w(move || px(model::chip_width(&row.get())))
            .min_w(px(0.))
            .px_2()
            .flex_row()
            .items_center()
            .gap_1()
            .rounded(px(5.))
            .bg(move || {
                if row.get().attention > 0 {
                    INSET.resolve()
                } else {
                    CLEAR.resolve()
                }
            })
            .border_1()
            .border_color(move || {
                if row.get().attention > 0 {
                    BORDER.resolve()
                } else {
                    CLEAR.resolve()
                }
            })
            .hover(|s| s.bg(INSET))
            .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
            .accessibility_label(move || {
                let row = row.get();
                Some(
                    if row.attention == 0 {
                        format!("Switch Project · {}", row.name)
                    } else {
                        format!(
                            "Switch Project · {} · Cards with updates: {}",
                            row.name, row.attention
                        )
                    }
                    .into(),
                )
            })
            .child(icon(Icon::Folder).w(px(12.)).h(px(12.)).flex_shrink_0())
            .child(
                label(move || row.get().name)
                    .text_size(px(12.))
                    .text_color(move || {
                        if row.get().attention > 0 {
                            TEXT.resolve()
                        } else {
                            MUTED.resolve()
                        }
                    })
                    .font_weight(FontWeight::MEDIUM)
                    .min_w(px(0.))
                    .flex_1()
                    .truncate(),
            )
            .child(
                view()
                    .w(px(22.))
                    .flex_shrink_0()
                    .items_center()
                    .child(reminder_count(
                        move || row.get().attention,
                        move || row.get().pending,
                    )),
            )
            .on_press(move |_: &()| quick_open::switch_project(ui, &row.get_untracked().id)),
    )
}
