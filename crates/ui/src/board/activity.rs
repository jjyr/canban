//! Board-only window chrome Project tab strip and persistent Search trigger.
//! No background work or durable acknowledgement.
pub(crate) mod model;
use crate::{presenter::*, quick_open};
use canban_ui_kit::{tokens::*, *};
use model::Summary;
use nsh_core::prelude::*;
use std::collections::BTreeMap;

pub fn strip(ui: Ui, trailing: impl IntoElement) -> impl IntoElement {
    let rows = computed(move || {
        let idx = ui.index.get();
        let hosts = ui.snapshot.with(|s| {
            idx.project_order
                .iter()
                .map(|p| {
                    let hid = s.project_host(&p.id);
                    let label = s.project_host_badge(&p.id);
                    let offline = s.is_host_offline(hid);
                    (p.id.clone(), (label, offline))
                })
                .collect::<BTreeMap<_, _>>()
        });
        ui.reminders.with(|reminders| {
            model::projects_from_reminders_with_hosts(&idx.project_order, reminders, &hosts)
        })
    });
    let has_projects = computed(move || !rows.get().is_empty());
    let last_id = computed(move || rows.get().last().map(|r| r.id.clone()));
    let layout_sig = computed(move || {
        rows.get()
            .into_iter()
            .map(|r| {
                let w = model::tab_width(&r).round() as u32;
                (r.id, w)
            })
            .collect::<Vec<_>>()
    });

    let container_w = signal(0_f32);
    let viewport_w = signal(0_f32);
    let scroll_x = signal(0_f32);
    let max_tab_w = computed(move || model::max_tab_width(container_w.get()));
    let strip_ref = element_ref();
    let prev_reveal = stored((String::new(), 0_f32));

    effect(move || {
        let selected = ui.project.get();
        let vw = viewport_w.get();
        let cap = max_tab_w.get();
        let _ = layout_sig.get();
        let (prev_selected, prev_vw) = prev_reveal.get();
        if vw > 0. && (selected != prev_selected || prev_vw <= 0.) {
            prev_reveal.replace((selected.clone(), vw));
            if let Some(target_x) = model::reveal_scroll_x(
                &rows.get_untracked(),
                &selected,
                vw,
                scroll_x.get_untracked(),
                cap,
            ) {
                scroll_x.set(target_x);
                strip_ref.scroll_to(
                    Point {
                        x: px(target_x),
                        y: px(0.),
                    },
                    false,
                );
            }
        }
    });

    view()
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .overflow_hidden()
        .flex_row()
        .items_center()
        .gap_1()
        .window_drag(true)
        .on_layout(move |b: &Bounds<Points>| {
            let w = b.size.width.0;
            if (container_w.get_untracked() - w).abs() > 0.5 {
                container_w.set(w);
            }
        })
        .child(when(
            move || has_projects.get(),
            move || {
                scroll()
                    .horizontal()
                    .scrollbar(false)
                    .element_ref(&strip_ref)
                    .flex_shrink(1.)
                    .min_w(px(0.))
                    .h_full()
                    .border_l_1()
                    .border_r_1()
                    .border_color(BORDER)
                    .on_layout(move |b: &Bounds<Points>| {
                        let w = b.size.width.0;
                        if (viewport_w.get_untracked() - w).abs() > 0.5 {
                            viewport_w.set(w);
                        }
                    })
                    .on_scroll(move |e: &ScrollEvent| scroll_x.set(e.offset.x.0))
                    .child(
                        view()
                            .h_full()
                            .flex_row()
                            .items_center()
                            .gap_0()
                            .child(each(
                                move || rows.get(),
                                |r| r.id.clone(),
                                move |row, _| project_tab(ui, row, max_tab_w, last_id),
                            )),
                    )
            },
        ))
        .child(picker_trigger(ui, rows, has_projects))
        .child(trailing)
        .child(
            view()
                .flex_1()
                .min_w(move || px(if ui.narrow.get() { 0. } else { 64. }))
                .h_full()
                .window_drag(true),
        )
}

fn project_tab(
    ui: Ui,
    row: ReadSignal<Summary>,
    max_tab_w: Memo<f32>,
    last_id: Memo<Option<String>>,
) -> impl IntoElement {
    let id = row.get_untracked().id;
    let id_sel = id.clone();
    let id_last = id.clone();
    let is_selected = computed(move || ui.project.get() == id_sel);
    let is_last = computed(move || last_id.get().as_deref() == Some(id_last.as_str()));
    let tab_w = computed(move || model::tab_width(&row.get()).min(max_tab_w.get()));

    view()
        .relative()
        .h_full()
        .w(move || px(tab_w.get()))
        .flex_shrink_0()
        .window_drag(false)
        .border_r_1()
        .border_color(move || if is_last.get() { CLEAR } else { BORDER })
        .tooltip(
            hint_tooltip(move || SharedString::from(row.get().tooltip()))
                .placement(Placement::Bottom),
        )
        .child(dynamic(move || {
            let selected = is_selected.get();
            let mut cell = view().relative().w_full().h_full().child(
                canban_ui_kit::press_target()
                    .w_full()
                    .h_full()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .px_2p5()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .role(Some(AccessibilityRole::Tab))
                    .selected(selected)
                    .bg(if selected { SELECTED } else { CLEAR })
                    .hover(move |s| s.bg(if selected { SELECTED } else { CARD_HOVER }))
                    .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                    .accessibility_label(move || {
                        let r = row.get();
                        let name = r.display_name();
                        let mut label_text = if r.attention == 0 {
                            format!("Project · {name}")
                        } else {
                            format!("Project · {name} · Cards with updates: {}", r.attention)
                        };
                        if selected {
                            label_text.push_str(" · Current");
                        }
                        Some(label_text.into())
                    })
                    .child(
                        icon_tinted(Icon::Folder, if selected { ACCENT } else { MUTED })
                            .w(px(12.))
                            .h(px(12.))
                            .flex_shrink_0(),
                    )
                    .child(
                        label(move || row.get().display_name())
                            .text_size(px(12.5))
                            .font_weight(if selected {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .text_color(move || {
                                if selected || row.get().attention > 0 {
                                    TEXT.resolve()
                                } else {
                                    MUTED.resolve()
                                }
                            })
                            .min_w(px(0.))
                            .flex_1()
                            .truncate(),
                    )
                    .child(when(
                        move || row.get().attention > 0,
                        move || {
                            view().flex_shrink_0().child(reminder_count(
                                move || row.get().attention,
                                move || row.get().pending,
                            ))
                        },
                    ))
                    .on_press(move |_: &()| {
                        let pid = row.get_untracked().id;
                        if ui.project.get_untracked() != pid
                            || ui.page.get_untracked() != "board"
                            || !ui.card.get_untracked().is_empty()
                        {
                            quick_open::switch_project(ui, &pid);
                        }
                    }),
            );
            if selected {
                cell = cell.child(
                    view()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(2.))
                        .bg(ACCENT)
                        .pointer_events(PointerEvents::None),
                );
            }
            cell
        }))
}

fn picker_trigger(ui: Ui, rows: Memo<Vec<Summary>>, has_projects: Memo<bool>) -> impl IntoElement {
    view()
        .flex_shrink_0()
        .window_drag(false)
        .tooltip(
            hint_tooltip(move || {
                let n = rows.get().len();
                if n == 0 {
                    SharedString::from("Switch or create Project")
                } else {
                    SharedString::from(format!("Switch or search Projects ({n})"))
                }
            })
            .placement(Placement::Bottom),
        )
        .child(dynamic(move || {
            if has_projects.get() {
                canban_ui_kit::press_target()
                    .w(px(30.))
                    .h(px(30.))
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .bg(CLEAR)
                    .hover(|s| s.bg(INSET))
                    .focus(|s| s.border_1().border_color(ACCENT))
                    .accessibility_label(|| Some("Switch Project".into()))
                    .child(icon(Icon::ChevronDown).w(px(14.)).h(px(14.)))
                    .on_press(move |_: &()| ui.show_search(true))
                    .into_element()
            } else {
                canban_ui_kit::press_target()
                    .h(px(28.))
                    .px_2()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(CLEAR)
                    .hover(|s| s.bg(INSET))
                    .focus(|s| s.border_color(ACCENT))
                    .accessibility_label(|| Some("Switch Project · Choose Project".into()))
                    .child(
                        label("Choose Project")
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(13.)),
                    )
                    .child(
                        icon(Icon::ChevronDown)
                            .w(px(13.))
                            .h(px(13.))
                            .flex_shrink_0(),
                    )
                    .on_press(move |_: &()| ui.show_search(true))
                    .into_element()
            }
        }))
}
