//! Compact Card reader: two-row header, scrolling context/conversation, fixed composer.
pub(crate) mod attachments;
mod composer;
mod details;
mod header;
mod task;
mod timeline;
mod workflow;
use crate::{drafts::*, presenter::*};
use canban_ui_kit::{tokens::*, *};
use nsh_core::{TouchEvent, TouchPhase, prelude::*};

#[derive(Clone, Copy)]
pub(super) struct Reading {
    scroll: ElementRef,
    follow: Signal<bool>,
    unread: Signal<usize>,
    composer: ElementRef,
    composer_height: Signal<f32>,
    editor: Stored<Option<canban_ui_kit::markdown::MarkdownInput>>,
    reader: crate::reminders::Reader,
}
impl Reading {
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
        follow: signal(false),
        unread: signal(0),
        composer: element_ref(),
        composer_height: signal(180.),
        editor: stored(None),
        reader: crate::reminders::Reader::new(saved.y.0),
    };
    let draft = document(ui, key, Draft::card(&card));
    effect(move || {
        let _ = ui.reminders.get();
        let _ = crate::reminders::foreground();
        let _ = ui.page.get();
        let _ = ui.quick_open.get();
        let _ = ui.utility_popup.get();
        let _ = ui.native_modal.get();
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
    view()
        .on_layout(move |b: &Bounds<Points>| {
            reading
                .composer_height
                .set((b.size.height.0 / 3. - 72.).clamp(72., 200.))
        })
        .on_touch(move |event: &TouchEvent| {
            if event.phase == TouchPhase::Start
                && let Some(root) = reading.composer.node()
                && event
                    .points
                    .iter()
                    .any(|p| p.changed && !p.path.contains(&root))
                && let Some(editor) = reading.editor.get()
                && !editor.held.get_untracked()
            {
                editor.blur();
            }
        })
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .bg(SURFACE)
        .child(header::render(ui, draft, reading))
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .element_ref(&reading.scroll)
                .content_offset(saved)
                .on_layout(move |b: &Bounds<Points>| reading.reader.height(ui, b.size.height.0))
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
                    let bottom = e.content_size.height.0 - e.offset.y.0 - e.size.height.0 <= 48.;
                    reading.follow.set(bottom);
                    if bottom {
                        reading.unread.set(0);
                    }
                })
                .child(
                    view()
                        .p_3()
                        .gap_3()
                        .on_layout(move |_: &Bounds<Points>| {
                            if reading.follow.get_untracked() {
                                reading.scroll.scroll_to_end(false);
                            }
                        })
                        .child(task::render(ui, draft, reading))
                        .child(details::render(ui, draft))
                        .child(workflow::render(ui))
                        .child(when(
                            move || draft.with(|d| d.workspace_details),
                            move || {
                                view()
                                    .gap_2()
                                    .child(action(
                                        "Hide workspace details",
                                        Variant::Quiet,
                                        move |_| draft.update(|d| d.workspace_details = false),
                                    ))
                                    .child(crate::components::git_workspace::panel(ui))
                            },
                        ))
                        .child(timeline::render(ui, reading)),
                ),
        )
        .child(when(
            move || reading.unread.get() > 0 || unread_updates.get() > 0,
            move || {
                action(
                    move || {
                        if unread_updates.get() > 0 {
                            format!("Unread updates ({}) ↓", unread_updates.get())
                        } else {
                            format!("New activity ({}) ↓", reading.unread.get())
                        }
                    },
                    Variant::Quiet,
                    move |_| reading.latest(),
                )
                .self_center()
            },
        ))
        .child(composer::render(ui, draft, reading))
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
