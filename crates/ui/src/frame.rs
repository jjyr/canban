//! Shared production window frame, including split Terminal chrome and overlays.
use crate::{presenter::*, quick_open, screens, sidebar, terminals};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;

pub const CHROME_LEFT_OFFSET: f32 = 10.0;
pub const CHROME_RIGHT_OFFSET: f32 = 10.0;

pub(crate) fn compute_chrome_padding(
    left_controls: f32,
    right_controls: f32,
    macos: bool,
) -> (f32, f32) {
    let left = if macos {
        left_controls.max(78.) + CHROME_LEFT_OFFSET
    } else {
        left_controls + CHROME_LEFT_OFFSET
    };
    let right = if macos {
        CHROME_RIGHT_OFFSET
    } else {
        right_controls + CHROME_RIGHT_OFFSET
    };
    (left, right)
}

pub fn chrome_left_padding() -> f32 {
    let tb = nsh_core::element::desktop::title_bar();
    compute_chrome_padding(tb.left.0, tb.right.0, cfg!(target_os = "macos")).0
}

pub fn chrome_right_padding() -> f32 {
    let tb = nsh_core::element::desktop::title_bar();
    compute_chrome_padding(tb.left.0, tb.right.0, cfg!(target_os = "macos")).1
}

/// Keep production and Gallery custom chrome on the same native window API.
pub fn configure_window_chrome() {
    let mut title_bar = nsh_core::desktop::TitleBarOptions::default();
    title_bar.style = nsh_core::desktop::TitleBarStyle::Hidden;
    title_bar.height = Some(px(42.));
    let mut attributes = nsh_core::desktop::WindowAttributes::default();
    attributes.title_bar = Some(title_bar);
    nsh_core::element::desktop::current_window().set(attributes);
}

fn subpage_chrome(
    ui: Ui,
    back_label: Option<(&'static str, &'static str)>,
    title: impl IntoReactive<SharedString> + 'static,
) -> View {
    let mut left = view()
        .flex_row()
        .items_center()
        .gap_1p5()
        .min_w(px(0.))
        .window_drag(false);
    if let Some((label_text, dest)) = back_label {
        left = left.child(compact_action(label_text, Variant::Quiet, move |_| {
            ui.navigate(dest)
        }));
    }
    left = left.child(
        label(title)
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(13.5))
            .truncate()
            .min_w(px(0.)),
    );
    view()
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .flex_row()
        .items_center()
        .gap_2()
        .pl_2()
        .window_drag(true)
        .child(left)
        .child(view().flex_1().h_full().window_drag(true))
}

#[derive(Clone, PartialEq, Eq)]
enum ChromeMode {
    Board,
    FullCard,
    Subpage(String),
}

fn route_chrome(ui: Ui) -> impl IntoElement {
    let mode = computed(move || {
        let p = ui.page.get();
        let full_card = p == "card"
            && (ui.card_standalone.get() || ui.narrow.get() || ui.card_terminal_open.get());
        if matches!(p.as_str(), "board" | "new-card" | "terminals") || (p == "card" && !full_card) {
            ChromeMode::Board
        } else if full_card {
            ChromeMode::FullCard
        } else {
            ChromeMode::Subpage(p)
        }
    });
    dynamic(move || {
        let current = mode.get();
        untracked(|| match current {
            ChromeMode::Board => crate::board::chrome_bar(ui).into_element(),
            ChromeMode::FullCard => view()
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .flex_row()
                .items_center()
                .gap_1p5()
                .pl_2()
                .window_drag(true)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .min_w(px(0.))
                        .window_drag(false)
                        .child(compact_action("← Board", Variant::Quiet, move |_| {
                            ui.close_card();
                        }))
                        .child(
                            label(move || {
                                let proj =
                                    ui.current_project().map(|pr| pr.name).unwrap_or_default();
                                if let Some(c) = ui.current_card() {
                                    format!("{proj} / Card #{} · {}", c.number, c.title)
                                } else {
                                    proj
                                }
                            })
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(13.5))
                            .truncate()
                            .min_w(px(0.)),
                        ),
                )
                .child(view().flex_1().h_full().window_drag(true))
                .into_element(),
            ChromeMode::Subpage(p) => match p.as_str() {
                "insights" => crate::insights::chrome_bar(ui).into_element(),
                "schedules" => crate::schedules::chrome_bar(ui).into_element(),
                "github" => crate::github::workspace::chrome_bar(ui).into_element(),
                "github-account" => subpage_chrome(
                    ui,
                    Some(("← GitHub", "github")),
                    "GitHub / Account & access",
                )
                .into_element(),
                "github-repository" => {
                    subpage_chrome(ui, Some(("← Board", "board")), move || {
                        ui.current_project()
                            .map(|pr| format!("{} / GitHub repository", pr.name))
                            .unwrap_or_else(|| "GitHub repository".into())
                    })
                    .into_element()
                }
                "workflows" => crate::workflows::chrome_bar(ui).into_element(),
                "review" => view()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .pl_2()
                    .window_drag(true)
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .min_w(px(0.))
                            .window_drag(false)
                            .child(icon(Icon::Compare).w(px(14.)).h(px(14.)))
                            .child(
                                label(move || {
                                    if let Some(c) = ui.current_card() {
                                        format!("Git · #{} {}", c.number, c.title)
                                    } else if let Some(pr) = ui.current_project() {
                                        format!("Git · {}", pr.name)
                                    } else {
                                        "Git".into()
                                    }
                                })
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(13.5))
                                .truncate()
                                .min_w(px(0.)),
                            ),
                    )
                    .child(view().flex_1().h_full().window_drag(true))
                    .into_element(),
                "settings" => subpage_chrome(ui, None, "Settings").into_element(),
                "archive" => subpage_chrome(ui, Some(("← Board", "board")), move || {
                    ui.current_project()
                        .map(|pr| format!("{} / Archive", pr.name))
                        .unwrap_or_else(|| "Archive".into())
                })
                .into_element(),
                "drafts" => subpage_chrome(ui, Some(("← Board", "board")), "Drafts").into_element(),
                "new-project" => {
                    subpage_chrome(ui, Some(("← Board", "board")), "New Project").into_element()
                }
                "project-location" => subpage_chrome(ui, Some(("← Board", "board")), move || {
                    ui.current_project()
                        .map(|pr| format!("{} / Work location", pr.name))
                        .unwrap_or_else(|| "Work location".into())
                })
                .into_element(),
                "project-agents" => subpage_chrome(ui, Some(("← Board", "board")), move || {
                    ui.current_project()
                        .map(|pr| format!("{} / Project Agents", pr.name))
                        .unwrap_or_else(|| "Project Agents".into())
                })
                .into_element(),
                "agent-setup" => {
                    subpage_chrome(ui, Some(("← Settings", "settings")), "Agent Setup")
                        .into_element()
                }
                "overview" | "agents" => {
                    subpage_chrome(ui, None, "Activity & Overview").into_element()
                }
                _ => view().flex_1().h_full().window_drag(true).into_element(),
            },
        })
    })
}

pub fn backend_notice(ui: Ui) -> impl IntoElement {
    when(
        move || !ui.backend_notice.get().is_empty(),
        move || {
            canban_ui_kit::notice(
                move || ui.backend_notice.get(),
                canban_ui_kit::Tone::Attention,
            )
        },
    )
}

fn is_workspace_page(page: &str) -> bool {
    matches!(page, "board" | "card" | "new-card")
        || !matches!(
            page,
            "agent-setup"
                | "project-agents"
                | "workflows"
                | "project-location"
                | "new-project"
                | "terminals"
                | "terminal"
                | "overview"
                | "agents"
                | "review"
                | "drafts"
                | "archive"
                | "settings"
                | "insights"
                | "schedules"
                | "github"
                | "github-account"
                | "github-repository"
        )
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let in_terminals = computed(move || matches!(ui.page.get().as_str(), "terminals" | "terminal"));
    let is_workspace = computed(move || is_workspace_page(&ui.page.get()));
    let shell_mounted = signal(!in_terminals.get_untracked());
    effect(move || {
        if !in_terminals.get() && !shell_mounted.get_untracked() {
            shell_mounted.set(true);
        }
    });
    let workspace_project = signal::<Option<String>>(if is_workspace.get_untracked() {
        Some(ui.project.get_untracked())
    } else {
        None
    });
    effect(move || {
        let project = ui.project.get();
        if is_workspace.get() && workspace_project.get_untracked().as_ref() != Some(&project) {
            workspace_project.set(Some(project));
        }
    });
    let subpage_route = computed(move || {
        let page = ui.page.get();
        if matches!(page.as_str(), "terminals" | "terminal") || is_workspace_page(&page) {
            None
        } else {
            Some((page, ui.card.get(), ui.project.get()))
        }
    });
    view()
        .flex_1()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .on_layout(move |b: &Bounds<Points>| ui.narrow.set(b.size.width.0 < 950.))
        .child(when(
            move || in_terminals.get(),
            move || terminals::screen(ui),
        ))
        .child(when(
            move || shell_mounted.get(),
            move || {
                view()
                    .flex_1()
                    .min_h(px(0.))
                    .style_when(move || in_terminals.get(), |s| s.hidden())
                    .accessibility_elements_hidden(move || in_terminals.get())
                    .pointer_events(move || {
                        if in_terminals.get() {
                            PointerEvents::None
                        } else {
                            PointerEvents::Auto
                        }
                    })
                    .child(
                        view()
                            .h(px(42.))
                            .flex_shrink_0()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .bg(canban_ui_kit::tokens::SURFACE)
                            .border_b_1()
                            .border_color(canban_ui_kit::tokens::BORDER)
                            .window_drag(true)
                            .pl(move || px(chrome_left_padding()))
                            .pr(move || px(chrome_right_padding()))
                            .child(crate::navigation::controls(ui))
                            .child(route_chrome(ui))
                            .child(crate::hosts::trigger_pill(ui))
                            .child(crate::overview::trigger_pill(ui)),
                    )
                    .child(
                        view()
                            .flex_1()
                            .min_h(px(0.))
                            .flex_row()
                            .child(sidebar(ui))
                            .child(
                                view()
                                    .flex_1()
                                    .min_h(px(0.))
                                    .min_w(px(0.))
                                    .child(
                                        view()
                                            .flex_1()
                                            .min_h(px(0.))
                                            .min_w(px(0.))
                                            .child(dynamic(move || {
                                                let Some(_project) = workspace_project.get() else {
                                                    return view().into_element();
                                                };
                                                untracked(|| {
                                                    view()
                                                        .flex_1()
                                                        .min_h(px(0.))
                                                        .min_w(px(0.))
                                                        .style_when(
                                                            move || !is_workspace.get(),
                                                            |s| s.hidden(),
                                                        )
                                                        .accessibility_elements_hidden(move || {
                                                            !is_workspace.get()
                                                        })
                                                        .pointer_events(move || {
                                                            if is_workspace.get() {
                                                                PointerEvents::Auto
                                                            } else {
                                                                PointerEvents::None
                                                            }
                                                        })
                                                        .child(crate::workspace::screen(ui))
                                                        .into_element()
                                                })
                                            }))
                                            .child(dynamic(move || {
                                                let Some((page, _, _)) = subpage_route.get() else {
                                                    return view().into_element();
                                                };
                                                untracked(|| screens::screen(ui, &page))
                                            })),
                                    )
                                    .child(backend_notice(ui))
                                    .child(crate::utilities::bar(ui)),
                            ),
                    )
            },
        ))
        .child(when(
            move || ui.operation_error.get().is_empty() && !ui.toast.get().is_empty(),
            move || {
                view()
                    .absolute()
                    .bottom(px(44.))
                    .right(px(16.))
                    .max_w(px(420.))
                    .px_3()
                    .py_2()
                    .bg(canban_ui_kit::tokens::SURFACE)
                    .border_1()
                    .border_color(canban_ui_kit::tokens::BORDER)
                    .rounded(px(8.))
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .pointer_events(PointerEvents::None)
                    .child(
                        canban_ui_kit::icon_tinted(
                            canban_ui_kit::Icon::Check,
                            canban_ui_kit::tokens::ACCENT,
                        )
                        .w(px(15.))
                        .h(px(15.)),
                    )
                    .child(
                        canban_ui_kit::label(move || ui.toast.get())
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .min_w(px(0.)),
                    )
            },
        ))
        .child(when(
            move || !ui.operation_error.get().is_empty(),
            move || {
                view()
                    .absolute()
                    .bottom(px(44.))
                    .right(px(16.))
                    .max_w(px(520.))
                    .px_3()
                    .py_2()
                    .bg(canban_ui_kit::tokens::ATTENTION_SURFACE)
                    .border_1()
                    .border_color(canban_ui_kit::tokens::ATTENTION)
                    .rounded(px(8.))
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        canban_ui_kit::label(move || ui.operation_error.get())
                            .text_color(canban_ui_kit::tokens::ATTENTION)
                            .flex_1()
                            .min_w(px(0.)),
                    )
                    .child(canban_ui_kit::chrome_button(
                        canban_ui_kit::Icon::Close,
                        "Dismiss message",
                        move |_| ui.operation_error.set(String::new()),
                    ))
            },
        ))
        .child(quick_open::overlay(ui))
        .child(crate::utilities::overlay(ui))
        .child(terminals::tabs::rename_dialog(ui))
        .child(terminals::tabs::close_confirm_dialog(ui))
        .child(crate::terminals::worktrees::modals(ui))
        .child(crate::review::git_ops::render_discard_modal(ui))
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{Card, Project, Snapshot, Workflow};
    use nsh_core::{
        ApproxTextEngine, Core, EventKind, EventPayload, MountBatch, MountItem, NodeId,
        element::JsEvent, geometry::size,
    };
    use serde_json::json;
    use std::collections::{BTreeMap, HashMap, HashSet};

    struct TreeTracker {
        alive: HashSet<NodeId>,
        parents: HashMap<NodeId, NodeId>,
        texts: HashMap<NodeId, String>,
        custom_components: HashMap<NodeId, String>,
        labels: HashMap<String, NodeId>,
    }

    impl TreeTracker {
        fn new() -> Self {
            Self {
                alive: HashSet::new(),
                parents: HashMap::new(),
                texts: HashMap::new(),
                custom_components: HashMap::new(),
                labels: HashMap::new(),
            }
        }

        fn apply(&mut self, batch: &MountBatch) {
            for item in &batch.items {
                match item {
                    MountItem::Create { id, .. } => {
                        self.alive.insert(*id);
                    }
                    MountItem::Component { id, name } => {
                        self.custom_components.insert(*id, name.to_string());
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
                        self.texts.remove(id);
                        self.custom_components.remove(id);
                    }
                    MountItem::Text { id, text, .. } => {
                        self.texts.insert(*id, text.text.to_string());
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

        #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
        fn mounted_custom_count(&self, component_name: &str) -> usize {
            self.custom_components
                .iter()
                .filter(|(id, name)| *name == component_name && self.is_mounted(**id))
                .count()
        }
    }

    fn pump(core: &mut Core<ApproxTextEngine>, tree: &mut TreeTracker) {
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

    #[test]
    fn frame_preserves_board_workspace_and_chrome_across_card_and_terminals_transitions() {
        #[component]
        fn FrameHarness() -> impl IntoElement {
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
                agent_overrides: BTreeMap::new(),
                tags: vec!["perf".into()],
                workflow: Workflow::default(),
                target_branch: Some("main".into()),
                created_at: 1,
            });
            snapshot.projects.push(Project {
                id: "proj-2".into(),
                name: "OtherProject".into(),
                directory: "/tmp/other".into(),
                location: None,
                github: None,
                detected_github: None,
                github_disconnected: false,
                delivery_mode: Default::default(),
                agent_overrides: BTreeMap::new(),
                tags: Vec::new(),
                workflow: Workflow::default(),
                target_branch: Some("main".into()),
                created_at: 2,
            });
            let card: Card = serde_json::from_value(json!({
                "id": "card-88",
                "project_id": "proj-1",
                "number": 88,
                "title": "Smooth Card & Terminals Transitions",
                "body": "Verify retained Board subtree.",
                "stage": "develop",
                "revision": 1,
                "created_at": 1
            }))
            .unwrap();
            snapshot.cards.push(card);
            snapshot.terminals.push(canban_types::TerminalEntry {
                id: "term-1".into(),
                conversation_id: None,
                number: 1,
                name: Some("Shell 1".into()),
                origin_project_id: Some("proj-1".into()),
                origin_card_id: None,
                agent_engine: None,
                runtime: Some(canban_types::TerminalRuntime {
                    machine: "local".into(),
                    owner_core_id: None,
                    core_id: Some("core-1".into()),
                    terminal: Some("pty-1".into()),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                    observed_agent: None,
                    waiting: None,
                }),
                agent: None,
                created_at: 1,
            });
            ui.snapshot.set(snapshot);
            ui.project.set("proj-1".into());
            ui.board_width.set(1050.);
            ui.terminal_selection.set("term-1".into());

            view()
                .w(px(1600.))
                .h(px(800.))
                .child(
                    view()
                        .h(px(1.))
                        .child(
                            canban_ui_kit::press_target()
                                .accessibility_label(|| Some("harness-open-card".into()))
                                .on_press(move |_: &()| ui.open_card("card-88".into())),
                        )
                        .child(
                            canban_ui_kit::press_target()
                                .accessibility_label(|| Some("harness-close-card".into()))
                                .on_press(move |_: &()| ui.close_card()),
                        )
                        .child(
                            canban_ui_kit::press_target()
                                .accessibility_label(|| Some("harness-go-terminals".into()))
                                .on_press(move |_: &()| ui.navigate("terminals")),
                        )
                        .child(
                            canban_ui_kit::press_target()
                                .accessibility_label(|| Some("harness-go-board".into()))
                                .on_press(move |_: &()| ui.navigate("board")),
                        )
                        .child(
                            canban_ui_kit::press_target()
                                .accessibility_label(|| Some("harness-switch-project".into()))
                                .on_press(move |_: &()| ui.select_project("proj-2".into())),
                        ),
                )
                .child(super::screen(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1600.), px(800.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<FrameHarness>());
        let mut tree = TreeTracker::new();
        pump(&mut core, &mut tree);

        let initial_tile_node = tree
            .mounted_node_for_text("Smooth Card & Terminals Transitions")
            .expect("Board Card tile text mounted");
        let initial_chrome_agents = tree
            .mounted_node_for_text("Agents")
            .expect("Board chrome bar `Agents` action mounted");
        #[cfg(target_os = "macos")]
        let mounted_terminals = |tree: &TreeTracker| tree.mounted_custom_count("canban/terminal");
        #[cfg(not(target_os = "macos"))]
        let mounted_terminals = |tree: &TreeTracker| {
            usize::from(
                tree.texts
                    .iter()
                    .any(|(id, t)| t.contains("Canban native Terminal") && tree.is_mounted(*id)),
            )
        };

        assert_eq!(
            mounted_terminals(&tree),
            0,
            "native terminal must not be mounted while on Board"
        );

        // 1. Opening and closing a Card in split view keeps the exact same Board chrome bar and tile nodes.
        let open_card = *tree.labels.get("harness-open-card").unwrap();
        assert!(core.component_event(open_card, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert_eq!(
            tree.mounted_node_for_text("Agents"),
            Some(initial_chrome_agents),
            "route_chrome must not rebuild Board chrome bar when opening split Card"
        );
        assert!(
            tree.is_mounted(initial_tile_node),
            "Board tile node must stay the exact same mounted node when opening split Card"
        );

        let close_card = *tree.labels.get("harness-close-card").unwrap();
        assert!(core.component_event(close_card, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert_eq!(
            tree.mounted_node_for_text("Agents"),
            Some(initial_chrome_agents),
            "route_chrome must not rebuild Board chrome bar when closing split Card"
        );

        // 2. Navigating to Terminals mounts the single native PTY viewer while keeping the Board subtree retained.
        let go_terminals = *tree.labels.get("harness-go-terminals").unwrap();
        assert!(core.component_event(go_terminals, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert_eq!(
            mounted_terminals(&tree),
            1,
            "Terminals route must mount its active native PTY viewer"
        );
        assert!(
            tree.is_mounted(initial_tile_node),
            "Board tile node must stay retained (hidden) while visiting Terminals"
        );

        // 3. Returning to Board unmounts the Terminals PTY viewer (preserving single-PTY attachment)
        //    and reveals the exact same retained Board tile and chrome nodes without remounting.
        let go_board = *tree.labels.get("harness-go-board").unwrap();
        assert!(core.component_event(go_board, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert_eq!(
            mounted_terminals(&tree),
            0,
            "leaving Terminals must unmount its native PTY viewer immediately"
        );
        assert!(
            tree.is_mounted(initial_tile_node),
            "returning from Terminals to Board must reuse the retained Board workspace"
        );
        assert_eq!(
            tree.mounted_node_for_text("Agents"),
            Some(initial_chrome_agents)
        );

        // 4. Switching Projects replaces the workspace subtree per `docs/ui-motion.md`.
        let switch_proj = *tree.labels.get("harness-switch-project").unwrap();
        assert!(core.component_event(switch_proj, EventKind::Press, &EventPayload::None));
        pump(&mut core, &mut tree);
        assert!(
            !tree.is_mounted(initial_tile_node),
            "switching Projects must replace the previous Project's workspace"
        );
    }

    #[test]
    fn chrome_padding_preserves_macos_traffic_lights_and_clears_linux_right_csd_buttons() {
        assert_eq!(compute_chrome_padding(0., 0., true), (88., 10.));
        assert_eq!(compute_chrome_padding(84., 0., true), (94., 10.));
        // GNOME / KDE right-side window buttons: no 78pt phantom left gap, full right inset.
        assert_eq!(compute_chrome_padding(0., 92., false), (10., 102.));
        // Linux left-side window buttons (custom gtk-decoration-layout): left inset grows cleanly.
        assert_eq!(compute_chrome_padding(76., 0., false), (86., 10.));
    }
    #[test]
    fn git_diff_retains_nodes_during_refresh_and_ignores_other_host_recovery() {
        #[component]
        fn GitRefreshHarness() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            ui.project.set("p".into());
            ui.page.set("review".into());
            ui.snapshot.update(|s| {
                for (id, project, path) in [("card-other", "other", "/wt/other"), ("card-worktree", "p", "/wt/p") ] {
                    s.cards.push(serde_json::from_value(json!({"id":id, "project_id":project, "number":1, "title":id, "body":"", "stage":"review", "revision":1, "created_at":1, "worktree":path})).unwrap());
                }
            });
            ui.review_state.base_ref.set("main".into());
            ui.review_state.compare_ref.set("topic".into());
            let value = crate::review::cache::ReadValue::new(json!({
                "files": [{"filename":"keep.rs", "patch":"@@ -1 +1 @@\n-old\n+retained code", "additions":1, "deletions":1}]
            }));
            crate::review::apply_compare(ui, &value);
            crate::review::init(ui);
            let event = move |host: &str| canban_app_server::hosts::HostEvent {
                host_id: host.into(),
                event: canban_types::AppEvent::Changed {
                    topic: "resync",
                    card_id: None,
                },
            };
            view()
                .size_full()
                .child(
                    canban_ui_kit::press_target()
                        .accessibility_label("refresh-begin")
                        .on_press(move |_: &()| ui.review_state.compare_loading.set(true)),
                )
                .child(
                    canban_ui_kit::press_target()
                        .accessibility_label("refresh-end")
                        .on_press(move |_: &()| ui.review_state.compare_loading.set(false)),
                )
                .child(
                    canban_ui_kit::press_target()
                        .accessibility_label("snapshot-change")
                        .on_press(move |_: &()| {
                            ui.snapshot.update(|s| {
                                s.entity_hosts.insert("unrelated".into(), "devpc".into());
                            })
                        }),
                )
                .child(
                    canban_ui_kit::press_target()
                        .accessibility_label("remote-recovery")
                        .on_press(move |_: &()| {
                            crate::review::handle_host_events(ui, &[event("devpc")])
                        }),
                )
                .child(
                    canban_ui_kit::press_target()
                        .accessibility_label("other-project-recovery")
                        .on_press(move |_: &()| {
                            crate::review::handle_host_events(
                                ui,
                                &[canban_app_server::hosts::HostEvent {
                                    host_id: "local".into(),
                                    event: canban_types::AppEvent::Changed {
                                        topic: "review",
                                        card_id: Some("card-other".into()),
                                    },
                                }],
                            )
                        }),
                )
                .child(
                    canban_ui_kit::press_target()
                        .accessibility_label("other-worktree-recovery")
                        .on_press(move |_: &()| {
                            crate::review::handle_host_events(
                                ui,
                                &[canban_app_server::hosts::HostEvent {
                                    host_id: "local".into(),
                                    event: canban_types::AppEvent::Changed {
                                        topic: "review",
                                        card_id: Some("card-worktree".into()),
                                    },
                                }],
                            )
                        }),
                )
                .child(
                    canban_ui_kit::press_target()
                        .accessibility_label("local-recovery-burst")
                        .on_press(move |_: &()| {
                            for _ in 0..8 {
                                crate::review::handle_host_events(ui, &[event("local")]);
                            }
                        }),
                )
                .child(canban_ui_kit::label(move || {
                    format!(
                        "pending:{} ticket:{}",
                        ui.review_state.refresh_pending.get(),
                        ui.review_state.compare_ticket.get()
                    )
                }))
                .child(crate::review::diff::workbench(ui))
        }
        let mut core = Core::new(ApproxTextEngine, size(px(1360.), px(900.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<GitRefreshHarness>());
        let mut tree = TreeTracker::new();
        pump(&mut core, &mut tree);
        let original_code = tree
            .mounted_node_for_text("+ retained code")
            .expect("diff mounted");
        let code_host = tree.labels["Git diff code"];
        for label in [
            "refresh-begin",
            "snapshot-change",
            "remote-recovery",
            "other-project-recovery",
            "other-worktree-recovery",
        ] {
            let id = tree.labels[label];
            assert!(core.component_event(id, EventKind::Press, &EventPayload::None));
            pump(&mut core, &mut tree);
            assert_eq!(
                tree.mounted_node_for_text("+ retained code"),
                Some(original_code)
            );
            assert!(tree.is_mounted(code_host));
            assert!(
                tree.mounted_node_for_text("pending:false ticket:0")
                    .is_some()
            );
        }
        assert!(core.component_event(
            tree.labels["local-recovery-burst"],
            EventKind::Press,
            &EventPayload::None
        ));
        pump(&mut core, &mut tree);
        assert!(
            tree.mounted_node_for_text("pending:true ticket:0")
                .is_some(),
            "burst should queue once without cancelling current reads"
        );
        assert!(core.component_event(
            tree.labels["refresh-end"],
            EventKind::Press,
            &EventPayload::None
        ));
        pump(&mut core, &mut tree);
        assert!(
            tree.mounted_node_for_text("pending:false ticket:1")
                .is_some(),
            "one trailing refresh should drain the burst"
        );
        assert_eq!(
            tree.mounted_node_for_text("+ retained code"),
            Some(original_code)
        );
        assert!(tree.is_mounted(code_host));
    }
    #[test]
    fn git_fold_keeps_same_path_index_and_disk_patches_independent() {
        #[component]
        fn GitFoldHarness() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            ui.review_state.compare_ref.set("UNCOMMITTED".into());
            let value = crate::review::cache::ReadValue::new(json!({
                "status_files":[{"path":"a.rs","staged":true,"unstaged":true}],
                "files":[
                    {"filename":"a.rs","section":"staged","patch":"@@ -1 +1 @@\n-old\n+index version"},
                    {"filename":"a.rs","section":"unstaged","patch":"@@ -1 +1 @@\n-index version\n+disk version"}
                ]
            }));
            crate::review::apply_compare(ui, &value);
            crate::review::diff::workbench(ui)
        }
        let mut core = Core::new(ApproxTextEngine, size(px(1360.), px(900.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<GitFoldHarness>());
        let mut tree = TreeTracker::new();
        pump(&mut core, &mut tree);
        assert!(tree.mounted_node_for_text("+ index version").is_some());
        assert!(tree.mounted_node_for_text("+ disk version").is_some());
        assert!(core.component_event(
            tree.labels["Collapse diff for staged a.rs"],
            EventKind::Press,
            &EventPayload::None
        ));
        pump(&mut core, &mut tree);
        assert!(tree.mounted_node_for_text("+ index version").is_none());
        assert!(tree.mounted_node_for_text("+ disk version").is_some());
        assert!(tree.labels.contains_key("Expand diff for staged a.rs"));
        assert!(core.component_event(
            tree.labels["Expand diff for staged a.rs"],
            EventKind::Press,
            &EventPayload::None
        ));
        pump(&mut core, &mut tree);
        assert!(tree.mounted_node_for_text("+ index version").is_some());
        assert!(tree.mounted_node_for_text("+ disk version").is_some());
    }

    #[test]
    fn pending_inline_comments_never_disappear_across_sections_folding_or_empty_diffs() {
        use crate::review::{PendingInlineComment, model::diff_key};

        #[component]
        fn PendingCommentFallbackHarness() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            ui.review_state.compare_ref.set("UNCOMMITTED".into());
            let value = crate::review::cache::ReadValue::new(json!({
                "status_files":[
                    {"path":"a.rs","staged":true,"unstaged":true},
                    {"path":"b.rs","staged":true},
                    {"path":"c.rs","unstaged":true},
                    {"path":"d.rs","unstaged":true},
                    {"path":"e.rs","unstaged":true}
                ],
                "files":[
                    {"filename":"a.rs","section":"staged","patch":"@@ -1 +1 @@\n-old\n+index version"},
                    {"filename":"b.rs","section":"staged","patch":"@@ -1 +1 @@\n-b0\n+b1"},
                    {"filename":"c.rs","section":"unstaged","patch":"@@ -1 +1 @@\n-c0\n+c1"},
                    {"filename":"d.rs","section":"unstaged","patch":"@@ -1 +1 @@\n-d0\n+d1"},
                    {"filename":"a.rs","section":"unstaged","patch":"@@ -1 +1 @@\n-index version\n+disk version"}
                ]
            }));
            crate::review::apply_compare(ui, &value);
            ui.review_state.pending_comments.set(vec![
                // 1. Cross-section comment on a visible path (`section == ""` while diff shows `"staged"`/`"unstaged"`).
                PendingInlineComment {
                    id: "c-cross".into(),
                    path: "a.rs".into(),
                    section: String::new(),
                    line: 1,
                    side: "RIGHT".into(),
                    snippet: String::new(),
                    body: "Cross-section comment".into(),
                    author: None,
                    edited_by_user: false,
                },
                // 2. Comment on same path in `"unstaged"` beyond `visible_file_limit` (4), while `"staged"` `a.rs` is within limit.
                PendingInlineComment {
                    id: "c-beyond".into(),
                    path: "a.rs".into(),
                    section: "unstaged".into(),
                    line: 1,
                    side: "RIGHT".into(),
                    snippet: String::new(),
                    body: "Beyond limit comment".into(),
                    author: None,
                    edited_by_user: false,
                },
                // 3. Comment on a collapsed file (`staged` `b.rs`).
                PendingInlineComment {
                    id: "c-collapsed".into(),
                    path: "b.rs".into(),
                    section: "staged".into(),
                    line: 1,
                    side: "RIGHT".into(),
                    snippet: String::new(),
                    body: "Collapsed file comment".into(),
                    author: None,
                    edited_by_user: false,
                },
            ]);
            ui.review_state.collapsed_files.update(|files| {
                files.insert(diff_key("staged", "b.rs"));
            });
            view()
                .w(px(1360.))
                .h(px(900.))
                .child(
                    canban_ui_kit::press_target()
                        .h(px(1.))
                        .accessibility_label(|| Some("harness-empty-diff".into()))
                        .on_press(move |_: &()| {
                            ui.review_state.active_line.set(None);
                            ui.review_state.compare_ref.set("main".into());
                            let empty =
                                crate::review::cache::ReadValue::new(json!({ "files": [] }));
                            crate::review::apply_compare(ui, &empty);
                        }),
                )
                .child(crate::review::diff::workbench(ui))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(1360.), px(900.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<PendingCommentFallbackHarness>());
        let mut tree = TreeTracker::new();
        pump(&mut core, &mut tree);

        // All 3 comments must be rendered in the fallback section (and `b.rs` header shows `1 pending`).
        assert!(
            tree.mounted_node_for_text("Other pending comments (3)")
                .is_some()
        );
        assert!(tree.mounted_node_for_text("1 pending").is_some());
        assert!(
            tree.mounted_node_for_text("Cross-section comment")
                .is_some()
        );
        assert!(tree.mounted_node_for_text("Beyond limit comment").is_some());
        assert!(
            tree.mounted_node_for_text("Collapsed file comment")
                .is_some()
        );

        // Expanding `b.rs` moves its comment inline so fallback drops to 2.
        assert!(core.component_event(
            tree.labels["Expand diff for staged b.rs"],
            EventKind::Press,
            &EventPayload::None
        ));
        pump(&mut core, &mut tree);
        assert!(
            tree.mounted_node_for_text("Other pending comments (2)")
                .is_some()
        );
        assert!(
            tree.mounted_node_for_text("Collapsed file comment")
                .is_some()
        );

        // Clicking `Show in diff` on `unstaged a.rs` reveals it beyond `visible_file_limit`,
        // moving `c-beyond` inline and dropping fallback to 1 (`c-cross`).
        assert!(core.component_event(
            tree.labels["Show unstaged a.rs in diff"],
            EventKind::Press,
            &EventPayload::None
        ));
        pump(&mut core, &mut tree);
        assert!(tree.mounted_node_for_text("+ disk version").is_some());
        assert!(
            tree.mounted_node_for_text("Other pending comments (1)")
                .is_some()
        );
        assert!(tree.mounted_node_for_text("Beyond limit comment").is_some());

        // Clicking `Edit` on the fallback comment mounts the inline composer in the fallback card.
        assert!(core.component_event(
            tree.labels["Edit comment on a.rs"],
            EventKind::Press,
            &EventPayload::None
        ));
        pump(&mut core, &mut tree);
        assert!(tree.labels.contains_key("Inline review comment"));

        // Switching to a non-workdir comparison with zero file changes still renders pending comments.
        assert!(core.component_event(
            tree.labels["harness-empty-diff"],
            EventKind::Press,
            &EventPayload::None
        ));
        pump(&mut core, &mut tree);
        assert!(tree.mounted_node_for_text("No file changes").is_some());
        assert!(
            tree.mounted_node_for_text("Other pending comments (2)")
                .is_some()
        );
    }
}
