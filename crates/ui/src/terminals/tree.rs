//! Complete Project/Card inventory. Closing a tab never removes this entry.
use super::*;
use crate::drafts::{capture_scroll, scroll_position};
fn toggle(ui: Ui, key: String) {
    ui.terminal_folds.update(|s| {
        if !s.remove(&key) {
            s.insert(key);
        }
    });
}
fn row(ui: Ui, e: Entry) -> impl IntoElement {
    let id = e.saved.id.clone();
    let selected = ui.terminal_selection.get() == id;
    let color = if e.needs_attention() {
        ATTENTION
    } else if e.ended() || e.inactive() {
        MUTED
    } else {
        SUCCESS
    };
    let hint = e.hint();
    let context_id = id.clone();
    view()
        .on_context_menu(move |event: &nsh_core::ContextMenuEvent| {
            ui.spawn(super::tabs::menu(
                ui,
                context_id.clone(),
                Some(event.page_position),
            ));
        })
        .tooltip(
            nsh_base::tooltip()
                .placement(Placement::Right)
                .px_3()
                .py_2()
                .bg(SURFACE)
                .border_1()
                .border_color(BORDER)
                .child(label(hint.clone())),
        )
        .child(
            nsh_base::button()
                .w_full()
                .min_h(px(if e.agent.is_some() { 56. } else { 32. }))
                .px_2()
                .flex_row()
                .items_center()
                .gap_2()
                .rounded(px(5.))
                .bg(if selected { SELECTED } else { CLEAR })
                .hover(|s| s.bg(INSET))
                .focus(|s| s.border_1().border_color(ACCENT))
                .selected(move || selected)
                .accessibility_label(move || Some(hint.clone().into()))
                .py_2()
                .child(if e.agent.is_some() {
                    view()
                        .w_full()
                        .min_w(px(0.))
                        .gap_1()
                        .child(
                            view()
                                .w_full()
                                .min_w(px(0.))
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(label(e.title()).flex_1().min_w(px(0.)).truncate())
                                .child(
                                    muted(e.stage.clone().unwrap_or_default())
                                        .w(px(super::labels::stage_width(&e)))
                                        .flex_shrink_0()
                                        .truncate(),
                                ),
                        )
                        .child(
                            view()
                                .w_full()
                                .min_w(px(0.))
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(super::labels::provider(&e).flex_1())
                                .child(muted(e.display_status()).text_color(color).flex_shrink_0()),
                        )
                } else {
                    view()
                        .w_full()
                        .min_w(px(0.))
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(label(e.title()).flex_1().min_w(px(0.)).truncate())
                        .child(
                            muted(if e.machine() == "local" {
                                String::new()
                            } else {
                                crate::machines::name(ui, e.machine())
                            })
                            .max_w(px(82.))
                            .truncate(),
                        )
                        .child(
                            view()
                                .w(px(5.))
                                .h(px(5.))
                                .rounded(px(3.))
                                .bg(color)
                                .flex_shrink_0(),
                        )
                })
                .on_press(move |_: &()| ui.open_terminal(id.clone())),
        )
}
pub fn sidebar(ui: Ui) -> impl IntoElement {
    // Runtime observation timestamps must not recreate pointer/focus targets.
    let inventory = computed(move || {
        let s = ui.snapshot.get();
        (
            s.projects
                .iter()
                .map(|p| (p.id.clone(), p.name.clone()))
                .collect::<Vec<_>>(),
            s.cards
                .iter()
                .map(|c| (c.id.clone(), c.number, c.title.clone(), c.archived_at))
                .collect::<Vec<_>>(),
            entries(&s)
                .into_iter()
                .map(|e| {
                    (
                        e.saved.id.clone(),
                        e.project_id.clone(),
                        e.hint(),
                        e.ended(),
                        e.inactive(),
                        e.needs_attention(),
                    )
                })
                .collect::<Vec<_>>(),
        )
    });
    view()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .bg(SURFACE)
        .px_2()
        .py_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .px_2()
                .pb_2()
                .child(label("Terminals").font_weight(FontWeight::SEMIBOLD))
                .child(
                    action(
                        move || {
                            if ui.terminal_inactive.get() {
                                "Hide inactive"
                            } else {
                                "Show inactive"
                            }
                        },
                        Variant::Quiet,
                        move |_| ui.terminal_inactive.update(|v| *v = !*v),
                    )
                    .bg(move || {
                        if ui.terminal_inactive.get() {
                            SELECTED
                        } else {
                            CLEAR
                        }
                    }),
                ),
        )
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .content_offset(scroll_position(ui, "terminals:tree"))
                .on_scroll(move |e: &ScrollEvent| capture_scroll(ui, "terminals:tree".into(), e))
                .child(dynamic(move || {
                    let _ = inventory.get();
                    let snapshot = ui.snapshot.get_untracked();
                    let rows: Vec<_> = entries(&snapshot)
                        .into_iter()
                        .filter(Entry::listed)
                        .collect();
                    let folds = ui.terminal_folds.get();
                    let show_inactive = ui.terminal_inactive.get();
                    let mut body = view().gap_2();
                    let mut groups: Vec<_> = snapshot
                        .projects
                        .iter()
                        .map(|p| (Some(p.id.clone()), p.name.clone()))
                        .collect();
                    if rows.iter().any(|e| e.project_id.is_none()) {
                        groups.push((None, "Other".into()));
                    }
                    for (project, name) in groups {
                        let key = format!("project:{}", project.clone().unwrap_or_default());
                        let collapsed = folds.contains(&key);
                        let mut group = view().gap_1().pt_2();
                        let mut heading = view().flex_row().items_center().min_w(px(0.)).child(
                            nsh_base::button()
                                .flex_1()
                                .min_w(px(0.))
                                .min_h(px(32.))
                                .px_2()
                                .items_start()
                                .bg(CLEAR)
                                .hover(|s| s.bg(INSET))
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(muted(if collapsed { "▸" } else { "▾" }))
                                .child(icon(Icon::Folder).w(px(14.)).h(px(14.)))
                                .child(
                                    label(name.clone())
                                        .text_color(MUTED)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .truncate(),
                                )
                                .on_press(move |_: &()| toggle(ui, key.clone())),
                        );
                        if let Some(pid) = project.clone() {
                            heading = heading.child(chrome_button_disabled(
                                Icon::Plus,
                                format!("New terminal in {name}"),
                                move || ui.pending.get(),
                                move |_| create(ui, pid.clone(), None),
                            ));
                        }
                        group = group.child(heading);
                        if !collapsed {
                            for e in rows.iter().filter(|e| {
                                e.project_id == project
                                    && (show_inactive
                                        || !e.inactive()
                                        || e.saved.id == ui.terminal_selection.get_untracked()
                                        || ui
                                            .terminal_tabs
                                            .with_untracked(|tabs| tabs.contains(&e.saved.id)))
                            }) {
                                group = group.child(
                                    view()
                                        .ml_3()
                                        .pl_2()
                                        .border_l_1()
                                        .border_color(BORDER)
                                        .child(row(ui, e.clone())),
                                );
                            }
                        }
                        body = body.child(group);
                    }
                    if rows.is_empty() {
                        body = body.child(
                            action("New terminal", Variant::Quiet, move |_| new_tab(ui))
                                .disabled(move || ui.pending.get()),
                        );
                    }
                    body
                })),
        )
}
