//! Board composition with native whole-Card dragging and durable placement.
pub mod activity;
mod columns;
mod drag;
mod model;
mod transitions;
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
    let project_controls = when(
        move || project.get().is_some(),
        move || {
            view()
                .flex_row()
                .items_center()
                .gap_0p5()
                .flex_shrink_0()
                .window_drag(false)
                .child(chrome_indicator_button(
                    Icon::GitHub,
                    move || {
                        if let Some(p) = project.get() {
                            if let Some(g) = &p.github {
                                if g.paused {
                                    format!("GitHub · {} (paused)", g.repository)
                                } else {
                                    format!("GitHub · Connected to {}", g.repository)
                                }
                            } else if let Some(d) = &p.detected_github {
                                format!("GitHub · Detected {} (not linked)", d.repository)
                            } else {
                                "GitHub repository & synchronization".into()
                            }
                        } else {
                            "GitHub repository & synchronization".into()
                        }
                    },
                    move || {
                        project
                            .get()
                            .and_then(|p| p.github)
                            .is_some_and(|g| !g.paused)
                    },
                    move |_| ui.navigate("github-repository"),
                ))
                .child(when(
                    move || project.get().is_some_and(|p| p.github.is_some()),
                    move || {
                        let gh_project = computed(move || {
                            project.get().filter(|p| p.github.is_some()).map(|p| p.id)
                        });
                        dynamic(move || {
                            if let Some(pid) = gh_project.get() {
                                crate::github::sync_button(ui, pid).into_element()
                            } else {
                                view().into_element()
                            }
                        })
                    },
                ))
        },
    );
    view()
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .overflow_hidden()
        .flex_row()
        .items_center()
        .gap_1p5()
        .pl_1()
        .window_drag(true)
        .child(activity::strip(ui, project_controls))
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
                                            MenuItem::new("Workflows…").on_select(move || {
                                                ui.workflows.section.set("workflows".into());
                                                ui.navigate("workflows");
                                            }),
                                            MenuItem::new("Scheduled Tasks…").on_select(
                                                move || {
                                                    crate::workflows::open_schedules(ui, None);
                                                },
                                            ),
                                            MenuItem::new("Drafts…")
                                                .on_select(move || ui.navigate("drafts")),
                                            MenuItem::new("Archived Cards")
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
                                            MenuItem::separator(),
                                            MenuItem::new("Archive Project…").on_select(
                                                move || {
                                                    let pid = ui.project.get_untracked();
                                                    if !pid.is_empty() {
                                                        ui.project_archive_confirm.set(Some(pid));
                                                    }
                                                },
                                            ),
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
    view()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .p_4()
        .gap_3()
        .child(crate::github::filters::toolbar(ui))
        .child(dynamic(move || {
            let Some(project) = project.get() else {
                if !ui.snapshot_ready.get() {
                    return canban_ui_kit::loading_panel(
                        "Loading workspace…",
                        "Connecting to your Projects and Cards.",
                    )
                    .into_element();
                }
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
                let stages = project.workflow.stages.clone();
                let width = signal(0f32);
                let compact = compact_mode(ui, width, stages.len());
                let state = drag::BoardDrag::new(ui, project.clone(), compact);
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
                        move || stage_tabs(ui, state, stages.clone(), focus)
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
                                    .child({
                                        let visible_stages = computed(move || {
                                            let is_compact = compact.get();
                                            if is_compact {
                                                let focused = focus.get();
                                                (
                                                    true,
                                                    stages
                                                        .iter()
                                                        .filter(|s| s.id == focused)
                                                        .cloned()
                                                        .collect::<Vec<_>>(),
                                                )
                                            } else {
                                                (false, stages.clone())
                                            }
                                        });
                                        dynamic(move || {
                                            let (compact, shown) = visible_stages.get();
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
                                        })
                                    }),
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
            .index
            .with_untracked(|idx| idx.cards.get(&id).map(|c| c.stage.clone()));
        if let Some(stage) = stage.filter(|s| ids.contains(s)) {
            focus.set(stage);
        }
    });
    focus
}

fn stage_tabs(
    ui: Ui,
    state: drag::BoardDrag,
    stages: Vec<canban_types::Stage>,
    focus: Signal<String>,
) -> impl IntoElement {
    let tab_counts = computed(move || {
        let cards = state.cards.get();
        let pid = ui.project.get();
        ui.index.with(|idx| {
            stages
                .iter()
                .map(|stage| {
                    let visible = cards.iter().filter(|c| c.stage == stage.id).count();
                    let total = idx
                        .cards
                        .values()
                        .filter(|c| c.project_id == pid && c.active() && c.stage == stage.id)
                        .count();
                    let title = if visible == total {
                        format!("{} {visible}", stage.name)
                    } else {
                        format!("{} {visible} / {total}", stage.name)
                    };
                    (stage.id.clone(), title)
                })
                .collect::<Vec<_>>()
        })
    });
    dynamic(move || {
        let items = tab_counts.get();
        segmented().children(
            items
                .into_iter()
                .map(|(id, title)| {
                    let selected = id.clone();
                    compact_segment(
                        title,
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

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{Project, Snapshot, Workflow};
    use nsh_core::{
        AccessibilityRole, ApproxTextEngine, Core, EventKind, EventPayload, MountBatch, MountItem,
        NodeId, element::JsEvent, geometry::size,
    };
    use std::collections::{BTreeMap, HashMap};

    #[test]
    fn deep_review_column_retains_bottom_card_texts_after_scrolling_past_reclaim_threshold() {
        use nsh_core::{
            EventKind, EventPayload, NodeId, ViewKind, element::JsEvent, geometry::point,
        };
        use std::collections::{HashMap, HashSet};

        #[component]
        fn DeepReviewBoard() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let mut snapshot = Snapshot::default();
            snapshot.projects.push(Project {
                id: "proj-1".into(),
                name: "Canban".into(),
                directory: "/tmp/canban".into(),
                github: None,
                detected_github: None,
                github_disconnected: false,
                delivery_mode: Default::default(),
                agent_overrides: BTreeMap::new(),
                tags: vec!["bug".into(), "ui".into()],
                workflow: Workflow::default(),
                target_branch: Some("main".into()),
                created_at: 1,
            });
            for n in 1..=35u64 {
                let mut card: canban_types::Card = serde_json::from_value(serde_json::json!({
                    "id": format!("rev-{n}"),
                    "project_id": "proj-1",
                    "number": n,
                    "title": format!("Review Card Title #{n}"),
                    "body": if n % 2 == 0 {
                        format!("Summary body line for review card #{n} #bug")
                    } else {
                        String::new()
                    },
                    "stage": "review",
                    "revision": 1,
                    "created_at": n
                }))
                .unwrap();
                card.branch = Some(format!("canban-0afd/{n}"));
                if n > 15 {
                    card.delivery = Some(canban_types::Delivery {
                        target_branch: "main".into(),
                        commit: format!("deadbeef{n:02x}"),
                        request_key: format!("req-{n}"),
                        merged_at: 1_700_000_000_000u64 + n,
                    });
                }
                snapshot.cards.push(card);
            }
            ui.snapshot.set(snapshot);
            ui.project.set("proj-1".into());
            ui.onboarded.set(true);
            view()
                .w(px(1200.))
                .h(px(760.))
                .child(
                    press_target()
                        .accessibility_label(Some("Trigger Unrelated Snapshot Refresh".into()))
                        .on_press(move |_: &()| {
                            ui.snapshot.update(|s| {
                                s.cards.push(
                                    serde_json::from_value(serde_json::json!({
                                        "id": "backlog-extra",
                                        "project_id": "proj-1",
                                        "number": 100,
                                        "title": "Unrelated Backlog Card",
                                        "body": "",
                                        "stage": "backlog",
                                        "revision": 1,
                                        "created_at": 100
                                    }))
                                    .unwrap(),
                                );
                            });
                        }),
                )
                .child(screen(ui))
        }

        struct TreeState {
            kinds: HashMap<NodeId, ViewKind>,
            parents: HashMap<NodeId, NodeId>,
            texts: HashMap<NodeId, String>,
            frames: HashMap<NodeId, Bounds<Points>>,
            labels: HashMap<String, NodeId>,
        }
        impl TreeState {
            fn new() -> Self {
                Self {
                    kinds: HashMap::new(),
                    parents: HashMap::new(),
                    texts: HashMap::new(),
                    frames: HashMap::new(),
                    labels: HashMap::new(),
                }
            }
            fn apply(&mut self, batch: &nsh_core::MountBatch) {
                for item in &batch.items {
                    match item {
                        MountItem::Create { id, kind } => {
                            self.kinds.insert(*id, *kind);
                        }
                        MountItem::Insert { parent, child, .. } => {
                            self.parents.insert(*child, *parent);
                        }
                        MountItem::Remove { id } => {
                            self.parents.remove(id);
                        }
                        MountItem::Delete { id } => {
                            self.kinds.remove(id);
                            self.parents.remove(id);
                            self.texts.remove(id);
                            self.frames.remove(id);
                        }
                        MountItem::Text { id, text, .. } => {
                            self.texts.insert(*id, text.text.to_string());
                        }
                        MountItem::Frame { id, frame } => {
                            self.frames.insert(*id, *frame);
                        }
                        MountItem::ViewProps { id, props } => {
                            if let Some(label) = &props.accessibility.label {
                                self.labels.insert(label.to_string(), *id);
                            }
                        }
                        _ => {}
                    }
                }
            }
            fn is_mounted(&self, mut id: NodeId) -> bool {
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
            fn mounted_texts(&self) -> HashSet<String> {
                self.texts
                    .iter()
                    .filter(|(id, _)| self.kinds.contains_key(id) && self.is_mounted(**id))
                    .map(|(_, t)| t.clone())
                    .collect()
            }
        }

        fn pump(core: &mut Core<ApproxTextEngine>, tree: &mut TreeState) {
            for _ in 0..8 {
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

        let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(760.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<DeepReviewBoard>());
        let mut tree = TreeState::new();
        pump(&mut core, &mut tree);

        // Locate the vertical scroll view of the Review column (rightmost column scroll view).
        let review_scroll = tree
            .kinds
            .iter()
            .filter(|(_, kind)| **kind == ViewKind::Scroll)
            .filter_map(|(id, _)| core.measure(*id).map(|m| (*id, m)))
            // Column scroll views sit inside the 3 stage columns (width <= 420pt).
            .filter(|(_, m)| m.frame.size.width.0 > 100. && m.frame.size.width.0 <= 450.)
            .max_by(|a, b| a.1.page.x.0.total_cmp(&b.1.page.x.0))
            .map(|(id, _)| id)
            .expect("Review column scroll view");

        // Scroll deep to the bottom (past 3x viewport height = ~2200pt).
        // Resolved cards (#16..=#35, merged_at = base + n) sort by resolved_at descending,
        // so the oldest resolved cards (#16..=#22) sit at the bottom of the Review column.
        let scroll_measure = core.measure(review_scroll).unwrap();
        let viewport_h = scroll_measure.frame.size.height.0;
        let (_, content, viewport, _) = core.scroll_listener(review_scroll).unwrap();
        let deep_y = (content.height.0 - viewport.height.0).max(0.);
        assert!(
            deep_y > viewport_h * 3.,
            "deep scroll offset {deep_y} must exceed 3x viewport reclaim threshold"
        );
        core.set_scroll_offset(review_scroll, point(px(0.), px(deep_y)), 0);
        // Native hosts deliver the Scroll event in addition to updating Core's
        // clipping offset. Virtual owners consume that event to mount new rows.
        assert!(core.component_event(
            review_scroll,
            EventKind::Scroll,
            &EventPayload::Scroll(point(px(0.), px(deep_y)))
        ));
        pump(&mut core, &mut tree);

        let mounted = tree.mounted_texts();
        for n in 16..=22 {
            let expected_title = format!("Review Card Title #{n}");
            assert!(
                mounted.contains(&expected_title),
                "Card #{n} title `{expected_title}` must remain mounted and visible at bottom of 35-card Review column; mounted sample={:?}",
                mounted
                    .iter()
                    .filter(|s| s.starts_with("Review Card Title"))
                    .collect::<Vec<_>>()
            );
        }

        // Trigger an unrelated snapshot update while scrolled to the bottom and verify bottom
        // Card text layers remain intact and non-duplicated, and rows have distinct `top` frames.
        let trigger = *tree
            .labels
            .get("Trigger Unrelated Snapshot Refresh")
            .expect("refresh trigger button");
        assert!(core.component_event(trigger, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);

        let mounted_after = tree.mounted_texts();
        for n in 16..=22 {
            let expected_title = format!("Review Card Title #{n}");
            let occurrences = tree
                .texts
                .iter()
                .filter(|(id, t)| {
                    *t == &expected_title && tree.kinds.contains_key(id) && tree.is_mounted(**id)
                })
                .count();
            assert_eq!(
                occurrences, 1,
                "Card #{n} title `{expected_title}` must appear once (no blanking or duplicate mount), got {occurrences}; mounted_after={mounted_after:?}"
            );
        }
    }

    struct ChromeTree {
        kinds: HashMap<NodeId, nsh_core::ViewKind>,
        parents: HashMap<NodeId, NodeId>,
        texts: HashMap<NodeId, (String, FontWeight)>,
        tabs: HashMap<NodeId, (String, bool)>,
        labels: HashMap<String, NodeId>,
    }

    impl ChromeTree {
        fn new() -> Self {
            Self {
                kinds: HashMap::new(),
                parents: HashMap::new(),
                texts: HashMap::new(),
                tabs: HashMap::new(),
                labels: HashMap::new(),
            }
        }

        fn apply(&mut self, batch: &MountBatch) {
            for item in &batch.items {
                match item {
                    MountItem::Create { id, kind } => {
                        self.kinds.insert(*id, *kind);
                    }
                    MountItem::Insert { parent, child, .. } => {
                        self.parents.insert(*child, *parent);
                    }
                    MountItem::Remove { id } => {
                        self.parents.remove(id);
                    }
                    MountItem::Delete { id } => {
                        self.kinds.remove(id);
                        self.parents.remove(id);
                        self.texts.remove(id);
                        self.tabs.remove(id);
                    }
                    MountItem::Text { id, text, .. } => {
                        let weight = text
                            .spans
                            .first()
                            .map(|s| FontWeight(s.style.font_weight))
                            .unwrap_or(FontWeight::NORMAL);
                        self.texts.insert(*id, (text.text.to_string(), weight));
                    }
                    MountItem::ViewProps { id, props } => {
                        if let Some(label) = &props.accessibility.label {
                            self.labels.insert(label.to_string(), *id);
                        }
                        if props.accessibility.role == AccessibilityRole::Tab {
                            let lbl = props
                                .accessibility
                                .label
                                .as_ref()
                                .map(|s| s.to_string())
                                .unwrap_or_default();
                            self.tabs
                                .insert(*id, (lbl, props.accessibility.state.selected));
                        } else {
                            self.tabs.remove(id);
                        }
                    }
                    _ => {}
                }
            }
        }

        fn is_mounted(&self, mut id: NodeId) -> bool {
            if !self.kinds.contains_key(&id) {
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

        fn mounted_text(&self, needle: &str) -> Option<(NodeId, FontWeight)> {
            self.texts
                .iter()
                .find(|(id, (t, _))| t == needle && self.is_mounted(**id))
                .map(|(id, (_, w))| (*id, *w))
        }
    }

    fn pump_chrome(core: &mut Core<ApproxTextEngine>, tree: &mut ChromeTree) {
        for _ in 0..8 {
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

    fn many_projects_snapshot() -> Snapshot {
        let mut snapshot = Snapshot::default();
        for i in 1..=12u64 {
            let mut project = Project {
                id: format!("p{i}"),
                name: format!("Project-{i:02}"),
                directory: format!("/tmp/project-{i:02}"),
                github: None,
                detected_github: None,
                github_disconnected: false,
                delivery_mode: Default::default(),
                agent_overrides: BTreeMap::new(),
                tags: Vec::new(),
                workflow: Workflow::default(),
                target_branch: Some("main".into()),
                created_at: i,
            };
            if i == 1 {
                project.github = Some(canban_types::GitHubSource {
                    repository: "jjyr/canban".into(),
                    repository_id: 1,
                    generation: "g".into(),
                    intake: canban_types::GitHubIntake::Off,
                    pr_intake: canban_types::GitHubPrIntake::Off,
                    authors: Vec::new(),
                    labels: Vec::new(),
                    include_drafts: false,
                    paused: false,
                    synced_at: None,
                    error: None,
                });
            }
            snapshot.projects.push(project);
        }
        snapshot
    }

    #[test]
    fn board_chrome_bar_renders_scrollable_project_tabs_and_persistent_search_trigger() {
        #[component]
        fn WideChromeProbe() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            ui.snapshot.set(many_projects_snapshot());
            ui.project.set("p1".into());
            ui.page.set("board".into());
            ui.narrow.set(false);
            view()
                .w(px(1440.))
                .h(px(42.))
                .child(view().h(px(0.)).child(label(move || {
                    format!(
                        "probe:project={}:qo={}:pp={}",
                        ui.project.get(),
                        ui.quick_open.get(),
                        ui.project_picker.get()
                    )
                })))
                .child(chrome_bar(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1440.), px(42.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<WideChromeProbe>());
        let mut tree = ChromeTree::new();
        pump_chrome(&mut core, &mut tree);

        // One horizontal Scroll hosts all 12 project tabs without +N truncation.
        let scroll_ids: Vec<_> = tree
            .kinds
            .iter()
            .filter(|(id, kind)| **kind == nsh_core::ViewKind::Scroll && tree.is_mounted(**id))
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(scroll_ids.len(), 1, "expected 1 horizontal project scroll");
        let scroll_m = core.measure(scroll_ids[0]).expect("scroll measure");
        assert!(scroll_m.frame.size.width.0 > 100.);

        for i in 1..=12 {
            let name = format!("Project-{i:02}");
            let (_, weight) = tree
                .mounted_text(&name)
                .unwrap_or_else(|| panic!("expected tab text {name}"));
            assert_eq!(
                weight,
                if i == 1 {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                },
                "unexpected font weight for {name}"
            );
        }
        assert!(
            !tree
                .texts
                .iter()
                .filter(|(id, _)| tree.is_mounted(**id))
                .any(|(_, (t, _))| {
                    t.starts_with("Projects ·")
                        || (t.starts_with('+') && t[1..].chars().all(|c| c.is_ascii_digit()))
                }),
            "scrollable strip must not render old +N / Projects · N truncation labels"
        );

        let mounted_tabs = |tree: &ChromeTree, core: &Core<ApproxTextEngine>| {
            let mut items = tree
                .tabs
                .iter()
                .filter(|(id, _)| tree.is_mounted(**id))
                .filter_map(|(id, (lbl, sel))| {
                    let m = core.measure(*id)?;
                    Some((m.page.x.0, *id, lbl.clone(), *sel))
                })
                .collect::<Vec<_>>();
            items.sort_by(|a, b| a.0.total_cmp(&b.0));
            items
        };

        let before = mounted_tabs(&tree, &core);
        assert_eq!(before.len(), 12);
        assert!(before[0].3, "Project-01 tab should start selected");
        assert!(
            before[1..].iter().all(|t| !t.3),
            "only Project-01 should be selected initially"
        );
        let order_before: Vec<_> = (1..=12).map(|i| format!("Project-{i:02}")).collect();
        for (idx, expected) in order_before.iter().enumerate() {
            assert!(
                before[idx].2.contains(expected),
                "tab {idx} was {:?}, expected {expected}",
                before[idx].2
            );
        }

        // Clicking Project-05 switches active selection in place without reordering tabs.
        let p5_tab = before[4].1;
        assert!(core.component_event(p5_tab, EventKind::Press, &EventPayload::None));
        pump_chrome(&mut core, &mut tree);
        assert!(
            tree.mounted_text("probe:project=p5:qo=false:pp=false")
                .is_some()
        );
        let after = mounted_tabs(&tree, &core);
        assert_eq!(after.len(), 12);
        for (idx, expected) in order_before.iter().enumerate() {
            assert!(
                after[idx].2.contains(expected),
                "tab {idx} reordered after click: {:?}",
                after[idx].2
            );
            assert_eq!(after[idx].3, idx == 4, "selection state mismatch at {idx}");
        }
        assert_eq!(
            tree.mounted_text("Project-05").unwrap().1,
            FontWeight::SEMIBOLD
        );
        assert_eq!(
            tree.mounted_text("Project-01").unwrap().1,
            FontWeight::MEDIUM
        );

        // Persistent Switch Project button stays visible within bounds and opens project picker.
        let switch_id = *tree
            .labels
            .get("Switch Project")
            .expect("persistent Switch Project button");
        let switch_m = core.measure(switch_id).expect("switch measure");
        let add_card_m = core
            .measure(tree.mounted_text("+ Add Card").unwrap().0)
            .expect("add card measure");
        assert!(switch_m.frame.size.width.0 > 0.);
        assert!(switch_m.page.x.0 >= scroll_m.page.x.0 + scroll_m.frame.size.width.0 - 0.5);
        assert!(add_card_m.page.x.0 + add_card_m.frame.size.width.0 <= 1440. + 0.5);

        assert!(core.component_event(switch_id, EventKind::Press, &EventPayload::None));
        pump_chrome(&mut core, &mut tree);
        assert!(
            tree.mounted_text("probe:project=p5:qo=true:pp=true")
                .is_some()
        );

        // Narrow 720pt chrome keeps scroll bounded and Switch Project / GitHub / + Add Card in bounds.
        #[component]
        fn NarrowChromeProbe() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            ui.snapshot.set(many_projects_snapshot());
            ui.project.set("p1".into());
            ui.page.set("board".into());
            ui.narrow.set(true);
            view().w(px(720.)).h(px(42.)).child(chrome_bar(ui))
        }

        let mut narrow_core = Core::new(ApproxTextEngine, size(px(720.), px(42.)));
        narrow_core.set_components(nsh_base::registry());
        narrow_core.mount_root(&nsh_core::element::Root::of::<NarrowChromeProbe>());
        let mut narrow_tree = ChromeTree::new();
        pump_chrome(&mut narrow_core, &mut narrow_tree);

        let n_scroll = *narrow_tree
            .kinds
            .iter()
            .find(|(id, kind)| **kind == nsh_core::ViewKind::Scroll && narrow_tree.is_mounted(**id))
            .unwrap()
            .0;
        let n_scroll_m = narrow_core.measure(n_scroll).unwrap();
        let n_switch_m = narrow_core
            .measure(narrow_tree.labels["Switch Project"])
            .unwrap();
        let n_add_m = narrow_core
            .measure(narrow_tree.mounted_text("+ Add Card").unwrap().0)
            .unwrap();
        assert!(n_scroll_m.frame.size.width.0 > 0.);
        assert!(n_switch_m.frame.size.width.0 > 0.);
        assert!(n_switch_m.page.x.0 >= n_scroll_m.page.x.0 + n_scroll_m.frame.size.width.0 - 0.5);
        assert!(n_add_m.page.x.0 + n_add_m.frame.size.width.0 <= 720. + 0.5);
    }

    #[test]
    fn board_chrome_bar_empty_projects_renders_choose_project_trigger() {
        #[component]
        fn EmptyChromeProbe() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            ui.snapshot.set(Snapshot::default());
            ui.project.set(String::new());
            ui.page.set("board".into());
            view()
                .w(px(900.))
                .h(px(42.))
                .child(view().h(px(0.)).child(label(move || {
                    format!(
                        "empty:qo={}:pp={}",
                        ui.quick_open.get(),
                        ui.project_picker.get()
                    )
                })))
                .child(chrome_bar(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(900.), px(42.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<EmptyChromeProbe>());
        let mut tree = ChromeTree::new();
        pump_chrome(&mut core, &mut tree);

        assert!(tree.mounted_text("Choose Project").is_some());
        assert!(
            !tree
                .kinds
                .iter()
                .any(|(id, k)| *k == nsh_core::ViewKind::Scroll && tree.is_mounted(*id)),
            "empty workspace must not mount project tab scroll"
        );
        let choose_id = *tree
            .labels
            .get("Switch Project · Choose Project")
            .expect("Choose Project trigger");
        assert!(core.component_event(choose_id, EventKind::Press, &EventPayload::None));
        pump_chrome(&mut core, &mut tree);
        assert!(tree.mounted_text("empty:qo=true:pp=true").is_some());
    }
}
