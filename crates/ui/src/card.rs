//! Compact Card reader: two-row header, scrolling context/conversation, fixed composer.
pub(crate) mod attachments;
pub(crate) mod changes;
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
pub(crate) use header::{advance_stage, next_stage_for_ui};
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
    pub(super) fn pause_follow(self) {
        if self.follow.try_get().is_some() {
            self.follow.set(false);
        }
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
        let active_destination = matches!(page.as_str(), "card" | "review")
            && untracked(|| ui.card_id()) == ui.card.get_untracked();
        if !matches!(page.as_str(), "card" | "review") {
            reading.editor.with(|ed| {
                if let Some(editor) = ed {
                    editor.blur();
                }
            });
        }
        if active_destination {
            reading.reader.check(ui);
        }
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
    let last_event = stored(
        ui.index
            .with_untracked(|idx| idx.last_event_by_card.get(&id).copied().unwrap_or(0)),
    );
    let event_card = card.id.clone();
    effect(move || {
        let last = ui.index.with(|idx| {
            idx.last_event_by_card
                .get(&event_card)
                .copied()
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
                                            && ui.editing_comment.get_untracked().is_none()
                                            && ui.deleting_comment.get_untracked().is_none()
                                        {
                                            reading.scroll.scroll_to_end(false);
                                        }
                                    })
                                    .child(review_draft_banner(ui))
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

/// Local unpublished PR review draft banner on the Card detail view.
fn review_draft_banner(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let Some(card) = ui.current_card() else {
            return view().into_element();
        };
        let Some(draft) = ui.index.with(|idx| idx.review_draft(&card.id).cloned()) else {
            return view().into_element();
        };
        let author = draft.author.as_deref().unwrap_or("Agent");
        let head_note = draft
            .compare_sha
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!(" · head {}", &s[..s.len().min(7)]))
            .unwrap_or_default();
        let count = draft.comments.len();
        let mut files = Vec::<String>::new();
        for c in &draft.comments {
            let short = c.path.rsplit('/').next().unwrap_or(&c.path).to_owned();
            if !files.contains(&short) {
                files.push(short);
            }
        }
        let files_summary = if count == 0 {
            "Summary only (0 inline comments)".to_owned()
        } else {
            let shown: Vec<String> = files.iter().take(3).cloned().collect();
            let more = if files.len() > 3 {
                format!(" +{} more", files.len() - 3)
            } else {
                String::new()
            };
            format!(
                "{count} inline comment{} across {}{more}",
                if count == 1 { "" } else { "s" },
                shown.join(", ")
            )
        };
        let summary_preview = draft
            .summary
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .to_owned();
        let cid_discard = card.id.clone();
        let cid_open = card.id;
        let mut box_view = view()
            .p_3()
            .rounded(px(RADIUS))
            .border_1()
            .border_color(ACCENT)
            .bg(SELECTED)
            .gap_1p5()
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .child(icon_tinted(Icon::Eye, ACCENT).w(px(14.)).h(px(14.)))
                            .child(
                                label(format!(
                                    "{author} prepared a local PR review draft (unpublished{head_note})"
                                ))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(12.5)),
                            ),
                    )
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .child(
                                compact_action("Discard", Variant::Quiet, move |_| {
                                    crate::review::discard_review_draft(ui, &cid_discard);
                                })
                                .disabled(move || ui.is_method_pending("review.draft.clear")),
                            )
                            .child(compact_action(
                                "Open in PR Diff & Publish →",
                                Variant::Primary,
                                move |_| crate::review::open_card_pr(ui, &cid_open),
                            )),
                    ),
            );
        if !summary_preview.is_empty() {
            box_view = box_view.child(muted(summary_preview).truncate());
        }
        box_view
            .child(muted(files_summary).text_size(px(11.5)))
            .into_element()
    })
}

/// Task previews, comments and references share one product reader.
pub(super) fn markdown(ui: Ui, body: impl Fn() -> String + 'static) -> nsh_base::TextViewElement {
    crate::editor::reader(ui, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{Card, Event, Project, Snapshot, Workflow};
    use nsh_core::{
        ApproxTextEngine, Command, Core, EventKind, EventMask, EventPayload, MountBatch, MountItem,
        NodeId, ViewKind,
        element::JsEvent,
        geometry::{point, size},
    };
    use std::collections::{HashMap, HashSet};

    #[derive(Default)]
    struct TreeState {
        alive: HashSet<NodeId>,
        parents: HashMap<NodeId, NodeId>,
        kinds: HashMap<NodeId, ViewKind>,
        texts: HashMap<NodeId, String>,
        events: HashMap<NodeId, EventMask>,
        scroll_commands: Vec<(NodeId, f32)>,
    }

    impl TreeState {
        fn apply(&mut self, batch: &MountBatch) {
            for item in &batch.items {
                match item {
                    MountItem::Create { id, kind } => {
                        self.alive.insert(*id);
                        self.kinds.insert(*id, *kind);
                    }
                    MountItem::Insert { parent, child, .. } => {
                        self.parents.insert(*child, *parent);
                    }
                    MountItem::Remove { id } => {
                        self.parents.remove(id);
                    }
                    MountItem::Delete { id } => {
                        self.alive.remove(id);
                        self.parents.remove(id);
                        self.kinds.remove(id);
                        self.texts.remove(id);
                        self.events.remove(id);
                    }
                    MountItem::Text { id, text, .. } => {
                        self.texts.insert(*id, text.text.to_string());
                    }
                    MountItem::Events { id, events } => {
                        self.events.insert(*id, *events);
                    }
                    MountItem::Command { id, command } => match command {
                        Command::ScrollTo { y, .. } => self.scroll_commands.push((*id, y.0)),
                        Command::ScrollToEnd { .. } => self.scroll_commands.push((*id, f32::MAX)),
                        _ => {}
                    },
                    _ => {}
                }
            }
        }

        fn is_mounted(&self, mut id: NodeId) -> bool {
            if !self.alive.contains(&id) {
                return false;
            }
            let mut guard = 0;
            while id != nsh_core::ROOT_ID {
                let Some(&parent) = self.parents.get(&id) else {
                    return false;
                };
                id = parent;
                guard += 1;
                if guard > 256 {
                    return false;
                }
            }
            true
        }

        fn mounted_node_for_text(&self, needle: &str) -> Option<NodeId> {
            self.texts
                .iter()
                .find(|(id, t)| *t == needle && self.is_mounted(**id))
                .map(|(id, _)| *id)
        }
    }

    fn pump(core: &mut Core<ApproxTextEngine>, tree: &mut TreeState) {
        for _ in 0..16 {
            let had_batch = if let Some(batch) = core.commit() {
                tree.apply(&batch);
                true
            } else {
                false
            };
            let mut events = Vec::new();
            core.take_events(&mut events);
            let mut dispatched = false;
            for ev in events {
                match ev {
                    JsEvent::Event(id, kind, payload) => {
                        core.component_event(id, kind, &payload);
                        dispatched = true;
                    }
                    JsEvent::LayoutListener {
                        host,
                        id,
                        listener,
                        frame,
                    } => {
                        core.component_layout(host, id, &listener, frame);
                        dispatched = true;
                    }
                    _ => {}
                }
            }
            if !had_batch && !dispatched {
                break;
            }
        }
    }

    #[test]
    fn read_full_report_does_not_jump_to_tail_when_following_bottom() {
        #[component]
        fn CardHarness() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let mut snapshot = Snapshot::default();
            snapshot.projects.push(Project {
                id: "proj-1".into(),
                name: "Canban".into(),
                directory: "/tmp/canban".into(),
                location: None,
                github: None,
                detected_github: None,
                github_disconnected: false,
                delivery_mode: Default::default(),
                agent_overrides: Default::default(),
                tags: Vec::new(),
                workflow: Workflow::default(),
                target_branch: Some("main".into()),
                created_at: 1,
            });
            let card: Card = serde_json::from_value(serde_json::json!({
                "id": "card-1",
                "project_id": "proj-1",
                "number": 128,
                "title": "Test card",
                "body": "Initial card description.\n\nLine two.\n\nLine three.",
                "stage": "develop",
                "revision": 1,
                "created_at": 1
            }))
            .unwrap();
            snapshot.cards.push(card);

            let long_report = (1..=60)
                .map(|i| format!("Report paragraph line {i}: detailed verification output."))
                .collect::<Vec<_>>()
                .join("\n\n");
            snapshot.events.push(Event {
                id: "ev-1".into(),
                sequence: 1,
                project_id: "proj-1".into(),
                card_id: "card-1".into(),
                kind: "comment.replied".into(),
                actor: "agent:developer".into(),
                message: long_report.clone(),
                data: serde_json::json!({ "body": long_report }),
                created_at: 2,
            });

            ui.snapshot.set(snapshot);
            ui.project.set("proj-1".into());
            ui.card.set("card-1".into());
            ui.page.set("card".into());
            view().w(px(800.)).h(px(600.)).child(screen(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(800.), px(600.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<CardHarness>());
        let mut tree = TreeState::default();
        pump(&mut core, &mut tree);

        assert!(
            tree.mounted_node_for_text("Read full report").is_some(),
            "expected long report comment to show Read full report button"
        );

        let card_scroll = tree
            .kinds
            .iter()
            .filter(|(id, kind)| **kind == ViewKind::Scroll && tree.is_mounted(**id))
            .filter_map(|(id, _)| core.scroll_listener(*id).map(|info| (*id, info)))
            .max_by(|a, b| a.1.1.height.0.total_cmp(&b.1.1.height.0))
            .map(|(id, _)| id)
            .expect("Card reader scroll view");

        let (_, content_before, viewport_before, _) = core.scroll_listener(card_scroll).unwrap();
        let bottom_y = (content_before.height.0 - viewport_before.height.0).max(0.);
        core.set_scroll_offset(card_scroll, point(px(0.), px(bottom_y)), 0);
        assert!(core.component_event(
            card_scroll,
            EventKind::Scroll,
            &EventPayload::Scroll(point(px(0.), px(bottom_y))),
        ));
        pump(&mut core, &mut tree);

        tree.scroll_commands.clear();

        let mut press_target = tree
            .mounted_node_for_text("Read full report")
            .expect("Read full report text node");
        while press_target != nsh_core::ROOT_ID
            && !tree
                .events
                .get(&press_target)
                .is_some_and(|m| m.contains(EventKind::Press))
        {
            let Some(&parent) = tree.parents.get(&press_target) else {
                break;
            };
            press_target = parent;
        }
        assert!(
            tree.events
                .get(&press_target)
                .is_some_and(|m| m.contains(EventKind::Press)),
            "expected pressable ancestor for Read full report"
        );
        assert!(core.component_event(press_target, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);

        let (_, content_after, viewport_after, _) = core.scroll_listener(card_scroll).unwrap();
        assert!(
            content_after.height.0 > content_before.height.0 + 1000.,
            "expected expanded comment to grow scroll content (before={}, after={})",
            content_before.height.0,
            content_after.height.0
        );
        assert!(
            !tree
                .scroll_commands
                .iter()
                .any(|(id, _)| *id == card_scroll),
            "clicking Read full report must not issue a scroll-to-end jump, got {:?}",
            tree.scroll_commands
        );
        assert!(
            tree.mounted_node_for_text("Report paragraph line 1: detailed verification output.")
                .is_some(),
            "top of the expanded report should remain visible in the viewport"
        );

        // Scrolling down to the end of the expanded report reveals the collapsed-toggle button.
        let expanded_bottom_y = (content_after.height.0 - viewport_after.height.0).max(0.);
        core.set_scroll_offset(card_scroll, point(px(0.), px(expanded_bottom_y)), 0);
        assert!(core.component_event(
            card_scroll,
            EventKind::Scroll,
            &EventPayload::Scroll(point(px(0.), px(expanded_bottom_y))),
        ));
        pump(&mut core, &mut tree);
        assert!(
            tree.mounted_node_for_text("Show less").is_some(),
            "expected expanded comment to show Show less at the end of the report"
        );
    }
}
