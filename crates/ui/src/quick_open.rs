//! One Quick Open and Project picker, built from local metadata and NSH Command.
mod model;
use crate::presenter::*;
use canban_ui_kit::*;
pub use model::{Candidate, Target, candidates, ranked};
use nsh_core::{keymap::When, prelude::*};

pub fn bindings(ui: Ui) {
    on_key_with(
        "mod+o",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| ui.show_search(false),
    );
}
pub fn overlay(ui: Ui) -> impl IntoElement {
    when(
        move || ui.quick_open.get(),
        move || {
            let all = computed(move || candidates(&ui.snapshot.get()));
            let rows = computed(move || {
                ranked(
                    &all.get(),
                    &ui.search_query.get(),
                    &ui.recent_destinations.with(|v| v.clone()),
                    ui.project_picker.get(),
                )
            });
            let choices = computed(move || {
                rows.get()
                    .into_iter()
                    .map(|r| SearchChoice {
                        selected: ui.project_picker.get()
                            && matches!(&r.target, Target::Project(id) if *id == ui.project.get()),
                        id: r.id,
                        title: r.title,
                        detail: r.detail,
                        icon: match r.kind {
                            "Project" => Icon::Projects,
                            "Card" => Icon::Board,
                            "Note" => Icon::Note,
                            _ => Icon::Agent,
                        },
                    })
                    .collect()
            });
            search_dialog(
                move || {
                    if ui.project_picker.get() {
                        "Switch Project".into()
                    } else {
                        "Quick Open".into()
                    }
                },
                ui.search_query,
                choices,
                move |id| {
                    // Resolve against fresh local state, rather than an obsolete retained attempt.
                    if let Some(row) = candidates(&ui.snapshot.get_untracked())
                        .into_iter()
                        .find(|r| r.id == id)
                    {
                        activate(ui, row);
                    } else {
                        ui.status
                            .set("This destination is no longer available".into());
                        ui.quick_open.set(false);
                    }
                },
                move || ui.quick_open.set(false),
                move || {
                    if ui.project_picker.get() {
                        "Search projects…".into()
                    } else {
                        "Search projects, cards, notes, agents…".into()
                    }
                },
                view()
                    .gap_2()
                    .child(muted(move || {
                        format!(
                            "{} results · ↑ ↓ choose · Enter open · Esc close",
                            rows.get().len()
                        )
                    }))
                    .child(when(
                        move || ui.project_picker.get(),
                        move || {
                            action("+ New Project", Variant::Quiet, move |_| {
                                ui.quick_open.set(false);
                                ui.navigate("new-project");
                            })
                        },
                    )),
            )
        },
    )
}
pub fn activate(ui: Ui, row: Candidate) {
    ui.quick_open.set(false);
    ui.recent_destinations.with(|ids| {
        ids.retain(|id| id != &row.id);
        ids.insert(0, row.id.clone());
        ids.truncate(32);
    });
    match row.target {
        Target::Project(id) => ui.select_project(id),
        Target::Card(id) => {
            if ui.page.get_untracked() == "overview" {
                ui.return_overview.set(true);
            }
            ui.open_card(id);
        }
        Target::Note(id) => {
            ui.return_overview.set(false);
            ui.open_note(id);
        }
        Target::Agent(id) => {
            let rows = canban_notifications::overview(&ui.snapshot.get_untracked());
            let Some(row) = rows.into_iter().find(|r| r.id == id) else {
                return;
            };
            if ui.page.get_untracked() == "overview" {
                ui.return_overview.set(true);
            }
            if row
                .attempt
                .as_ref()
                .is_some_and(|a| !a.finished() && a.terminal.is_some())
                && row.card.archived_at.is_none()
            {
                ui.open_card(row.card.id.clone());
                if let Some(req) = row.request {
                    ui.request.set(req.id);
                    ui.page.set("terminal".into());
                }
            } else {
                // Closed/queued/uncertain Agents open their exact inspector. Resume is
                // an explicit existing action; merely navigating never queues work.
                ui.navigate("overview");
                ui.overview_selection.set(row.id.clone());
                ui.overview_selected.set(Some(row));
            }
        }
    }
}
