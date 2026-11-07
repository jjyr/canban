use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

pub fn sidebar(ui: Ui) -> impl IntoElement {
    let mut side = view()
        .w(px(188.0))
        .p_3()
        .gap_2()
        .bg(SURFACE)
        .border_r_1()
        .border_color(BORDER)
        .child(
            view()
                .py_3()
                .child(heading("canban"))
                .child(muted("HUMAN CONTEXT")),
        )
        .child(dynamic(move || {
            let count = canban_notifications::overview(&ui.snapshot.get())
                .iter()
                .filter(|r| r.actionable())
                .count();
            let title = if count > 0 {
                format!("Overview   ! {count}")
            } else {
                "Overview".into()
            };
            view()
                .gap_1()
                .child(navigation(
                    title,
                    move || ui.page.get() == "overview" || ui.return_overview.get(),
                    move |_| ui.navigate("overview"),
                ))
                .child(if count > 0 {
                    status(format!("{count} need you"), Tone::Attention).into_element()
                } else {
                    view().into_element()
                })
        }))
        .child(view().h(px(8.0)))
        .child(muted("PROJECTS"))
        .child(dynamic(move || {
            view().gap_1().children(ui.snapshot.with(|s| {
                s.projects
                    .iter()
                    .map(|p| {
                        let id = p.id.clone();
                        let selected = id.clone();
                        navigation(
                            p.name.clone(),
                            move || ui.project.get() == selected,
                            move |_| {
                                ui.project.set(id.clone());
                                ui.card.set(String::new());
                                ui.note.set(String::new());
                                ui.navigate("board");
                            },
                        )
                        .into_element()
                    })
                    .collect::<Vec<_>>()
            }))
        }))
        .child(action("+ New project", Variant::Quiet, move |_| {
            ui.navigate("new-project")
        }))
        .child(view().h(px(12.0)));
    for (name, page) in [
        ("Board", "board"),
        ("Notes", "notes"),
        ("Archived Cards", "archive"),
    ] {
        side = side.child(navigation(
            name.into(),
            move || {
                let selected = ui.page.get();
                if page == "board" {
                    !ui.return_overview.get()
                        && matches!(selected.as_str(), "board" | "card" | "terminal" | "review")
                } else {
                    selected == page
                }
            },
            move |_| {
                ui.navigate(page);
            },
        ));
    }
    side.child(view().flex_1())
        .child(action("Check Herdr", Variant::Quiet, move |_| {
            dispatch(ui, "herdr.status", json!({}))
        }))
        .child(navigation(
            "Settings".into(),
            move || ui.page.get() == "settings",
            move |_| ui.navigate("settings"),
        ))
}
fn layout(
    title: impl IntoReactive<SharedString>,
    subtitle: impl IntoReactive<SharedString>,
) -> View {
    view()
        .flex_1()
        .p_5()
        .gap_4()
        .min_h(px(0.0))
        .child(view().gap_1().child(heading(title)).child(muted(subtitle)))
}
pub fn screen(ui: Ui, page: &str) -> AnyElement {
    match page {
        "new-project" => new_project(ui).into_element(),
        "new-card" => new_card(ui).into_element(),
        "notes" => crate::notes::screen(ui).into_element(),
        "overview" | "agents" => crate::overview::screen(ui).into_element(),
        "board" | "card" | "terminal" | "review" => crate::workspace::screen(ui).into_element(),
        "archive" => archived(ui).into_element(),
        "settings" => settings(ui).into_element(),
        _ => crate::workspace::screen(ui).into_element(),
    }
}
fn new_project(ui: Ui) -> impl IntoElement {
    let name = signal(String::new());
    let directory = signal(String::new());
    layout("New project","Each Project has a Board and a source directory. Git is optional.")
        .child(panel().max_w(px(720.0)).child(label("Project name")).child(field(name,"Name"))
            .child(label("Source directory")).child(field(directory,"/absolute/path/to/project"))
            .child(muted("The directory will be created if it does not exist. Uses config/workflows/default.json."))
            .child(action("Create project",Variant::Primary,move|_|dispatch(ui,"project.create",json!({"name":name.get_untracked(),"directory":directory.get_untracked()})))))
}
fn new_card(ui: Ui) -> impl IntoElement {
    let project = ui.project.get_untracked();
    let source = ui.source_note.get_untracked();
    let note = ui
        .snapshot
        .with_untracked(|s| s.notes.iter().find(|n| n.id == source).cloned());
    let key = format!("new-card:{project}:{source}");
    let initial = note
        .as_ref()
        .map(|n| crate::drafts::Draft {
            title: n.title.clone(),
            body: n.body.clone(),
            ..Default::default()
        })
        .unwrap_or_default();
    let draft = crate::drafts::document(ui, key.clone(), initial);
    let mut form = layout("New Card", "Define the scope, then create it in Backlog.").child(
        action("← Back", Variant::Quiet, move |_| {
            if source.is_empty() {
                ui.page.set("board".into());
            } else {
                ui.open_note(source.clone());
            }
        }),
    );
    if let Some(n) = note {
        form = form.child(notice(
            format!("Source Note: {} · included in this Card's context", n.title),
            Tone::Info,
        ));
    }
    form.child(panel().max_w(px(800.0)).child(bound_field(move||draft.get().title,move|s|draft.update(|d|d.title=s.into()),"Card title"))
        .child(bound_field(move||draft.get().body,move|s|draft.update(|d|d.body=s.into()),"Task description").multiline(true).h(px(220.0)))
        .child(bound_field(move||draft.get().tags,move|s|draft.update(|d|d.tags=s.into()),"Routing tags"))
        .child(muted(move||draft.get().error))
        .child(action("Create in Backlog",Variant::Primary,move|_|{
            let value=draft.get_untracked();let mut p=json!({"project_id":project,"title":value.title,"body":value.body,"tags":value.tags.split(',').map(str::trim).filter(|s|!s.is_empty()).collect::<Vec<_>>()});
            let source=ui.source_note.get_untracked();if !source.is_empty(){p["source_note_id"]=json!(source);}
            let key=key.clone();
            dispatch_complete(ui,"card.create",p,move|result|match result{Ok(_)=>{ui.drafts.with(|m|{m.remove(&key);});},Err(e)=>{if let Some(mut value)=draft.try_get(){value.error=e;draft.try_set(value);}}});
        }).disabled(move||ui.pending.get() || draft.with(|d|d.title.trim().is_empty()))))
}
fn archived(ui: Ui) -> impl IntoElement {
    layout(
        "Archived Cards",
        "History and execution results are retained in zstd-compressed Card packages.",
    )
    .child(scroll().flex_1().child(dynamic(move || {
        let project = ui.project.get();
        view().gap_3().children(ui.snapshot.with(|s| {
            s.cards
                .iter()
                .filter(|c| c.project_id == project && c.archived_at.is_some())
                .map(|c| {
                    let id = c.id.clone();
                    panel()
                        .child(label(format!("#{} · {}", c.number, c.title)))
                        .child(muted(c.archive.clone().unwrap_or_else(|| {
                            c.archive_error
                                .clone()
                                .unwrap_or_else(|| "Compression pending".into())
                        })))
                        .child(action(
                            "Open archived Card",
                            Variant::Secondary,
                            move |_| ui.open_card(id.clone()),
                        ))
                        .into_element()
                })
                .collect::<Vec<_>>()
        }))
    })))
}
fn settings(ui: Ui) -> impl IntoElement {
    let home = crate::APP
        .get()
        .map(|app| app.home.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Gallery preview".into());
    layout("Settings", "JSON configuration; changes apply to newly queued work after reload.")
        .child(crate::components::scheduling::settings(ui))
        .child(scroll().flex_1().child(view().gap_4()
            .child(panel()
                .child(label("Application home"))
                .child(muted(home.clone()))
                .child(label("Agent profiles / routing / Herdr endpoint"))
                .child(muted(format!("{home}/config/agents.json")))
                .child(label("Default workflow for new Projects"))
                .child(muted(format!("{home}/config/workflows/default.json")))
                .child(action("Reload agent configuration", Variant::Primary, move |_| dispatch(ui, "config.reload", json!({}))))
                .child(action("Check Herdr connection", Variant::Secondary, move |_| dispatch(ui, "herdr.status", json!({})))))
            .child(crate::profiles::registry(ui.profiles))
            .child(muted("Closing the application stops scheduling. Herdr-owned programs can continue; reopen Canban to receive reports and reconcile state."))))
}
