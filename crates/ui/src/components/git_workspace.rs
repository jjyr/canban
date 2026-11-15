//! Card workspace projection. Reads Git on a worker; old route results are discarded.
use crate::presenter::*;
use canban_types::WorkspaceInfo;
use canban_ui_kit::*;
use nsh_core::prelude::*;
use serde_json::json;
/// Keep full values for copying, but avoid pushing actions outside a narrow pane.
pub fn compact(value: &str, limit: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= limit {
        return value.into();
    }
    let half = (limit.saturating_sub(1)) / 2;
    format!(
        "{}…{}",
        chars[..half].iter().collect::<String>(),
        chars[chars.len() - half..].iter().collect::<String>()
    )
}
fn load(
    ui: Ui,
    info: Signal<Option<WorkspaceInfo>>,
    error: Signal<String>,
    generation: Signal<u64>,
) {
    let Some(card) = ui.current_card() else {
        return;
    };
    if crate::APP.get().is_none() {
        return;
    }
    let token = generation.get_untracked() + 1;
    generation.set(token);
    spawn(async move {
        let result = blocking(move || {
            crate::routed_call("card.workspace", json!({"card_id":card.id}), None)
                .and_then(|v| Ok(serde_json::from_value::<WorkspaceInfo>(v)?))
        })
        .await;
        if generation.try_get() != Some(token) {
            return;
        }
        match result {
            Ok(Ok(value)) => {
                info.try_set(Some(value));
                error.try_set(String::new());
            }
            Ok(Err(e)) => {
                error.try_set(e.to_string());
            }
            Err(e) => {
                error.try_set(e.to_string());
            }
        }
    });
}
pub fn panel(ui: Ui) -> impl IntoElement {
    let info = signal(None::<WorkspaceInfo>);
    let error = signal(String::new());
    let generation = signal(0_u64);
    let binding = computed(move || {
        ui.current_card()
            .map(|c| (c.id, c.worktree, c.branch, c.base_commit, c.delivery))
    });
    effect(move || {
        if let Some((_, path, branch, base_commit, _)) = binding.get() {
            info.set(Some(WorkspaceInfo {
                state: if path.is_some() {
                    "available"
                } else {
                    "not_created"
                }
                .into(),
                path,
                branch,
                base_commit,
                target_branch: untracked(|| ui.current_project().and_then(|p| p.target_branch)),
                dirty: false,
            }));
            untracked(|| load(ui, info, error, generation));
        }
    });
    view()
        .gap_1()
        .child(dynamic(move || {
            let _ = ui.snapshot.get();
            let location = ui
                .current_card()
                .and_then(|c| c.workspace.map(|w| w.location))
                .or_else(|| ui.current_project().map(|p| p.work_location()));
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(
                    muted(
                        location
                            .map(|l| {
                                format!(
                                    "{} · {}",
                                    crate::machines::name(ui, &l.machine),
                                    compact(&l.directory, 48)
                                )
                            })
                            .unwrap_or_default(),
                    )
                    .flex_1()
                    .min_w(px(0.))
                    .truncate(),
                )
                .child(chrome_button(
                    Icon::Refresh,
                    "Refresh workspace status",
                    move |_| load(ui, info, error, generation),
                ))
        }))
        .child(dynamic(move || {
            let Some(info) = info.get() else {
                return view().into_element();
            };
            let mut branch_row = view().flex_row().items_center().gap_1().child(
                muted(format!(
                    "Branch {} → {} · {}{}",
                    compact(info.branch.as_deref().unwrap_or("not created"), 30),
                    compact(info.target_branch.as_deref().unwrap_or("not set"), 20),
                    match info.state.as_str() {
                        "not_created" => "not created",
                        "directory" => "project directory",
                        "available" => "ready",
                        "missing" => "missing",
                        "conflict" => "merge conflict",
                        _ => &info.state,
                    },
                    if info.dirty {
                        " · uncommitted changes"
                    } else {
                        ""
                    }
                ))
                .flex_1()
                .min_w(px(0.))
                .truncate(),
            );
            if let Some(branch) = info.branch.clone() {
                branch_row =
                    branch_row.child(chrome_button(Icon::Copy, "Copy branch", move |_| {
                        copy_text(ui, branch.clone())
                    }));
            }
            let mut area = view().gap_1().child(branch_row);
            if let Some(path) = info.path {
                let copy = path.clone();
                area = area.child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(muted(compact(&path, 42)).flex_1().min_w(px(0.)).truncate())
                        .child(chrome_button(
                            Icon::Copy,
                            "Copy workspace path",
                            move |_| copy_text(ui, copy.clone()),
                        )),
                );
            }
            if let Some(commit) = info.base_commit {
                area = area.child(muted(format!(
                    "Base commit {}",
                    commit.chars().take(12).collect::<String>()
                )));
            }
            if let Some(delivery) = ui.current_card().and_then(|c| c.delivery) {
                area = area.child(muted(format!(
                    "Delivered {} → {}",
                    delivery.commit.chars().take(12).collect::<String>(),
                    delivery.target_branch
                )));
            }
            area.into_element()
        }))
        .child(dynamic(move || {
            if error.get().is_empty() {
                view().into_element()
            } else {
                notice(error.get(), Tone::Attention).into_element()
            }
        }))
}
