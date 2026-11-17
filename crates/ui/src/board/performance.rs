//! Opt-in production Core benchmark. Input is a read-only exported Snapshot;
//! results cover UI model/layout commits, not GPU presentation or input latency.
use super::*;
use canban_types::Snapshot;
use nsh_core::{ApproxTextEngine, Core, MountItem, NodeId, element::JsEvent, geometry::size};
use std::{cell::RefCell, collections::BTreeMap, time::Instant};
thread_local! {
    static INPUT: RefCell<Snapshot> = RefCell::new(Snapshot::default());
}
#[component]
fn Probe() -> impl IntoElement {
    canban_ui_kit::tokens::install_theme();
    let ui = Ui::new();
    let mut s = INPUT.with(|v| v.borrow().clone());
    let mut empty = s.projects[0].clone();
    empty.id = "empty-benchmark".into();
    empty.name = "Empty".into();
    empty.github = None;
    s.projects.push(empty);
    ui.snapshot.set(s);
    ui.project.set("empty-benchmark".into());
    ui.page.set("board".into());
    let pid = INPUT.with(|v| v.borrow().projects[0].id.clone());
    view()
        .w(px(1200.))
        .h(px(760.))
        .child(
            press_target()
                .accessibility_label(Some("Benchmark Select Project".into()))
                .on_press(move |_: &()| ui.project.set(pid.clone())),
        )
        .child(
            press_target()
                .accessibility_label(Some("Benchmark Select Empty".into()))
                .on_press(move |_: &()| ui.project.set("empty-benchmark".into())),
        )
        .child(screen(ui))
}
fn pump(core: &mut Core<ApproxTextEngine>, labels: &mut BTreeMap<NodeId, String>) {
    for _ in 0..12 {
        core.run_frame(nsh_core::time::now());
        let mut changed = false;
        if let Some(batch) = core.commit() {
            changed = true;
            for item in batch.items {
                match item {
                    MountItem::ViewProps { id, props } => {
                        if let Some(label) = &props.accessibility.label {
                            labels.insert(id, label.to_string());
                        }
                    }
                    MountItem::Delete { id } => {
                        labels.remove(&id);
                    }
                    _ => {}
                }
            }
        }
        let mut events = Vec::new();
        core.take_events(&mut events);
        for event in events {
            match event {
                JsEvent::Event(id, kind, payload) => {
                    core.component_event(id, kind, &payload);
                    changed = true;
                }
                JsEvent::LayoutListener {
                    host,
                    id,
                    listener,
                    frame,
                } => {
                    core.component_layout(host, id, &listener, frame);
                    changed = true;
                }
                _ => {}
            }
        }
        if !changed && !core.wants_frame() {
            break;
        }
    }
}
#[test]
#[ignore = "requires CANBAN_BOARD_BENCH_SNAPSHOT; use --run-ignored only"]
fn exported_project_switch_benchmark() {
    let path = std::env::var("CANBAN_BOARD_BENCH_SNAPSHOT").expect("exported Snapshot path");
    let s: Snapshot = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let pid = s.projects[0].id.clone();
    let count = s
        .cards
        .iter()
        .filter(|c| c.project_id == pid && c.active())
        .count();
    INPUT.with(|v| *v.borrow_mut() = s);
    let mut core = Core::new(ApproxTextEngine, size(px(1200.), px(760.)));
    core.set_components(nsh_base::registry());
    let initial = Instant::now();
    core.mount_root(&nsh_core::element::Root::of::<Probe>());
    let mut labels = BTreeMap::new();
    pump(&mut core, &mut labels);
    let initial_micros = initial.elapsed().as_micros();
    let select = *labels
        .iter()
        .find(|(_, label)| label.as_str() == "Benchmark Select Project")
        .unwrap()
        .0;
    let empty = *labels
        .iter()
        .find(|(_, label)| label.as_str() == "Benchmark Select Empty")
        .unwrap()
        .0;
    let mut samples = Vec::new();
    let mut max_owners = 0;
    for _ in 0..12 {
        let started = Instant::now();
        assert!(core.component_event(
            select,
            nsh_core::EventKind::Press,
            &nsh_core::EventPayload::None
        ));
        pump(&mut core, &mut labels);
        samples.push(started.elapsed().as_micros());
        let owners = labels
            .values()
            .filter(|label| label.starts_with("Card #"))
            .count();
        max_owners = max_owners.max(owners);
        assert!(
            owners > 0 && owners < 100,
            "{} Card owners for {count} Cards",
            owners
        );
        assert!(core.component_event(
            empty,
            nsh_core::EventKind::Press,
            &nsh_core::EventPayload::None
        ));
        pump(&mut core, &mut labels);
    }
    samples.sort();
    eprintln!(
        "BOARD_BENCH cards={count} initial_core_micros={initial_micros} core_p50_micros={} core_p95_micros={} max_native_card_owners={max_owners}",
        samples[6], samples[11]
    );
}
