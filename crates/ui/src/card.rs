//! Compact Card reader: two-row header, scrolling context/conversation, fixed composer.
pub(crate) mod attachments;
mod composer;
mod details;
mod header;
mod quick_merge;
mod task;
pub mod terminal;
pub(crate) mod timeline;
mod workflow;

use crate::{drafts::*, presenter::*};
use canban_ui_kit::{tokens::*, *};
use nsh_core::{TouchEvent, TouchPhase, prelude::*};
pub(crate) use quick_merge::{
    busy as quick_merge_busy, eligible as quick_merge_eligible, start as quick_merge_start,
};

#[derive(Clone, Copy)]
pub(super) struct Reading {
    scroll: ElementRef,
    content: ElementRef,
    follow: Signal<bool>,
    unread: Signal<usize>,
    composer: ElementRef,
    composer_height: Signal<f32>,
    editor: Stored<Option<canban_ui_kit::markdown::MarkdownInput>>,
    reader: crate::reminders::Reader,
    revealed_request: Stored<String>,
    jump_ready: Signal<bool>,
}
impl Reading {
    /// Layout callbacks are parent-relative. Compare measured root positions
    /// to the scroll content root, so nesting never changes history coordinates.
    async fn bounds(self, target: ElementRef) -> Option<Bounds<Points>> {
        let target = target.measure();
        let content = self.content.measure();
        let target = target.await?;
        let content = content.await?;
        Some(Bounds {
            origin: Point::new(
                target.page.x - content.page.x,
                target.page.y - content.page.y,
            ),
            size: target.frame.size,
        })
    }

    fn latest(self) {
        if self.follow.try_get().is_none() {
            return;
        }
        self.follow.set(true);
        self.unread.set(0);
        self.scroll.scroll_to_end(false);
    }
    fn top(self) {
        self.follow.set(false);
        self.scroll.scroll_to(Point::default(), false);
    }
}
pub fn at(ui: Ui, id: String) -> impl IntoElement {
    if std::env::var_os("CANBAN_MOTION_TRACE").is_some() {
        eprintln!("motion card mounted id={id}");
    }
    screen(Ui {
        card_view: Some(signal(id)),
        ..ui
    })
}
pub fn screen(ui: Ui) -> impl IntoElement {
    let Some(card) = ui.current_card() else {
        return view().child(muted("Select a Card")).into_element();
    };
    let id = card.id.clone();
    let key = format!("card:{id}");
    let scroll_key = format!("card:{id}:content");
    let saved = scroll_position(ui, &scroll_key);
    let reading = Reading {
        scroll: element_ref(),
        content: element_ref(),
        follow: signal(false),
        unread: signal(0),
        composer: element_ref(),
        composer_height: signal(180.),
        editor: stored(None),
        reader: crate::reminders::Reader::new(saved.y.0),
        revealed_request: stored(String::new()),
        jump_ready: signal(false),
    };
    let draft = document(ui, key, Draft::card(&card));
    effect(move || {
        let target = ui.card_event.get();
        let source_card = ui.card_id();
        if target.is_none() {
            return;
        }
        reading.jump_ready.set(false);
        ui.spawn(async move {
            // Markdown measures asynchronously; reveal the Timeline after initial layout,
            // then its actual row bounds refine the requested message position.
            next_frame().await;
            next_frame().await;
            if reading.jump_ready.try_get().is_some()
                && ui.card_event.get_untracked() == target
                && ui.page.get_untracked() == "card"
                && ui.card.get_untracked() == source_card
            {
                reading.jump_ready.set(true);
                reading.scroll.scroll_to_end(false);
            }
        });
    });
    effect(move || {
        let _ = ui.reminders.get();
        let _ = crate::reminders::foreground();
        let page = ui.page.get();
        let _ = ui.card_terminal_open.get();
        let _ = ui.narrow.get();
        let _ = ui.quick_open.get();
        let _ = ui.utility_popup.get();
        let _ = ui.native_modal.get();
        if !matches!(page.as_str(), "card" | "review") {
            reading.editor.with(|ed| {
                if let Some(editor) = ed {
                    editor.blur();
                }
            });
        }
        reading.reader.check(ui);
    });
    let viewport_height = stored(0f32);
    let reminder_card = card.id.clone();
    let unread_updates = computed(move || {
        ui.reminder_counts
            .with(|all| all.get(&reminder_card).map(|n| n.unread).unwrap_or(0))
    });
    effect(move || {
        if let Some(card) = ui.current_card() {
            let mut value = draft.get_untracked();
            value.reconcile(Draft::card(&card));
            if value != draft.get_untracked() {
                draft.set(value);
            }
        }
    });
    let last_event = stored(ui.snapshot.with_untracked(|s| {
        s.events
            .iter()
            .filter(|e| e.card_id == id)
            .map(|e| e.sequence)
            .max()
            .unwrap_or(0)
    }));
    let event_card = card.id.clone();
    effect(move || {
        let last = ui.snapshot.with(|s| {
            s.events
                .iter()
                .filter(|e| e.card_id == event_card)
                .map(|e| e.sequence)
                .max()
                .unwrap_or(0)
        });
        let before = last_event.replace(last);
        if last > before && !reading.follow.get_untracked() {
            reading.unread.update(|n| *n += 1);
        }
    });
    let live_watch_card = card.id.clone();
    effect(move || {
        let live = crate::terminals::card_live_entries_for_ui(ui, &live_watch_card);
        if !live.is_empty()
            && ui.page.get_untracked() == "card"
            && ui.card.get_untracked() == live_watch_card
            && ui.card_tab.get_untracked().is_empty()
        {
            let recent = ui
                .terminal_recent
                .with(|m| m.get(&live_watch_card).cloned());
            if let Some(first) = untracked(|| {
                crate::terminals::default_card_terminal_for_ui(
                    ui,
                    &live_watch_card,
                    recent.as_deref(),
                )
            }) {
                crate::terminals::select_card_tab(ui, &live_watch_card, first);
            }
        }
    });
    let show_terminal = computed(move || {
        ui.page.get() == "card" && ui.card_terminal_open.get() && ui.card_id() == ui.card.get()
    });
    let scroll_key_for_column = scroll_key;
    let reader_column = move || {
        let scroll_key = scroll_key_for_column.clone();
        view()
            .on_layout(move |b: &Bounds<Points>| {
                reading
                    .composer_height
                    .set((b.size.height.0 / 3. - 72.).clamp(72., 200.))
            })
            .on_touch(move |event: &TouchEvent| {
                if event.phase == TouchPhase::Start {
                    if ui.card_terminal_focused.get_untracked() {
                        ui.card_terminal_focused.set(false);
                    }
                    if let Some(root) = reading.composer.node()
                        && event
                            .points
                            .iter()
                            .any(|p| p.changed && !p.path.contains(&root))
                        && let Some(editor) = reading.editor.get()
                        && !editor.held.get_untracked()
                        && (editor.active.get_untracked() || editor.focused.get_untracked())
                    {
                        editor.blur();
                    }
                }
            })
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .bg(SURFACE)
            .child(header::render(ui, draft, reading))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.))
                    .min_h(px(0.))
                    .child(
                        scroll()
                            .flex_1()
                            .min_h(px(0.))
                            .element_ref(&reading.scroll)
                            .content_offset(saved)
                            .on_layout(move |b: &Bounds<Points>| {
                                reading.reader.height(ui, b.size.height.0)
                            })
                            .on_scroll(move |e: &ScrollEvent| {
                                reading.reader.viewport(ui, e.offset.y.0, e.size.height.0);
                                capture_scroll(ui, scroll_key.clone(), e);
                                let before = viewport_height.replace(e.size.height.0);
                                if before > 0.
                                    && (before - e.size.height.0).abs() > 0.5
                                    && reading.follow.get_untracked()
                                {
                                    reading.latest();
                                    return;
                                }
                                let bottom =
                                    e.content_size.height.0 - e.offset.y.0 - e.size.height.0 <= 48.;
                                reading.follow.set(bottom);
                                if bottom {
                                    reading.unread.set(0);
                                }
                            })
                            .child(
                                view()
                                    .element_ref(&reading.content)
                                    .w_full()
                                    .max_w(px(920.))
                                    .self_center()
                                    .p_3()
                                    .gap_3()
                                    .on_layout(move |_: &Bounds<Points>| {
                                        if reading.follow.get_untracked()
                                            && !draft.with_untracked(|d| d.editing)
                                        {
                                            reading.scroll.scroll_to_end(false);
                                        }
                                    })
                                    .child(task::render(ui, draft, reading))
                                    .child(details::render(ui, draft))
                                    .child(when(
                                        move || draft.with(|d| d.workspace_details),
                                        move || {
                                            view()
                                                .gap_2()
                                                .child(
                                                    view()
                                                        .flex_row()
                                                        .items_center()
                                                        .justify_between()
                                                        .child(label("Workspace"))
                                                        .child(chrome_button(
                                                            Icon::Close,
                                                            "Hide workspace details",
                                                            move |_| {
                                                                draft.update(|d| {
                                                                    d.workspace_details = false
                                                                })
                                                            },
                                                        )),
                                                )
                                                .child(crate::components::git_workspace::panel(ui))
                                        },
                                    ))
                                    .child(timeline::render(ui, reading)),
                            ),
                    )
                    .child(when(
                        move || reading.unread.get() > 0 || unread_updates.get() > 0,
                        move || {
                            compact_action(
                                move || {
                                    if unread_updates.get() > 0 {
                                        format!("{} unread ↓", unread_updates.get())
                                    } else {
                                        format!("{} new ↓", reading.unread.get())
                                    }
                                },
                                Variant::Quiet,
                                move |_| reading.latest(),
                            )
                            .self_center()
                        },
                    ))
                    .child(workflow::render(ui))
                    .child(composer::render(ui, draft, reading)),
            )
            .into_element()
    };

    dynamic(move || {
        let term_open = show_terminal.get();
        let is_narrow = ui.narrow.get();
        untracked(|| {
            if !term_open {
                reader_column()
            } else if is_narrow {
                terminal::pane(ui).into_element()
            } else {
                canban_ui_kit::split(
                    reader_column(),
                    terminal::pane(ui).into_element(),
                    ui.card_terminal_width,
                    400.,
                    360.,
                )
                .into_element()
            }
        })
    })
    .into_element()
}

pub(crate) fn proxy(draft: Signal<Draft>, comment: bool) -> Signal<String> {
    let text = signal(if comment {
        draft.get_untracked().comment
    } else {
        draft.get_untracked().body
    });
    effect(move || {
        let value = text.get();
        if comment {
            if value != draft.get_untracked().comment {
                draft.update(|d| d.comment = value);
            }
        } else if value != draft.get_untracked().body {
            draft.update(|d| d.body = value);
        }
    });
    effect(move || {
        let value = if comment {
            draft.get().comment
        } else {
            draft.get().body
        };
        if text.get_untracked() != value {
            text.set(value);
        }
    });
    text
}

/// Task previews, comments and references share one product reader.
pub(super) fn markdown(ui: Ui, body: impl Fn() -> String + 'static) -> nsh_base::TextViewElement {
    crate::editor::reader(ui, body)
}
