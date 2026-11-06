use crate::{components::*, presenter::*};
use canban_types::Event;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

pub fn sidebar(ui: Ui) -> impl IntoElement {
    let mut side = view()
        .w(px(208.0))
        .p_4()
        .gap_3()
        .bg(SURFACE)
        .border_r_1()
        .border_color(BORDER)
        .child(heading("canban"))
        .child(muted("HUMAN CONTEXT"))
        .child(view().h(px(12.0)))
        .child(muted("PROJECTS"))
        .child(dynamic(move || {
            view().gap_2().children(
                ui.snapshot
                    .with(|s| s.projects.clone())
                    .into_iter()
                    .map(|p| {
                        let name = p.name;
                        let id = p.id;
                        action(name, Variant::Secondary, move |_| {
                            ui.project.set(id.clone());
                            ui.page.set("board".into());
                        })
                        .into_element()
                    })
                    .collect::<Vec<_>>(),
            )
        }))
        .child(action("+ New project", Variant::Quiet, move |_| {
            ui.page.set("new-project".into())
        }))
        .child(view().h(px(12.0)));
    for (name, page) in [
        ("Board", "board"),
        ("Notes", "notes"),
        ("Agent activity", "agents"),
        ("Archived cards", "archive"),
    ] {
        side = side.child(action(name, Variant::Quiet, move |_| {
            ui.page.set(page.into())
        }));
    }
    side.child(view().flex_1())
        .child(muted(move || {
            ui.snapshot.with(|s| {
                format!(
                    "{} unread notices",
                    s.notices.iter().filter(|n| !n.read).count()
                )
            })
        }))
        .child(action("Check Herdr", Variant::Secondary, move |_| {
            dispatch(ui, "herdr.status", json!({}))
        }))
        .child(action("Settings", Variant::Quiet, move |_| {
            ui.page.set("settings".into())
        }))
        .child(action("Component gallery", Variant::Quiet, move |_| {
            ui.page.set("gallery".into())
        }))
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
        "card" => card_detail(ui).into_element(),
        "notes" => notes(ui).into_element(),
        "agents" => agents(ui).into_element(),
        "terminal" => crate::sessions::screen(ui).into_element(),
        "archive" => archived(ui).into_element(),
        "settings" => settings(ui).into_element(),
        "gallery" => gallery(ui).into_element(),
        _ => board(ui).into_element(),
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
    let title = signal(String::new());
    let body = signal(String::new());
    let tags = signal(String::new());
    layout("New Card","Describe the work and its context. Move into an executable stage when ready.")
        .child(panel().max_w(px(900.0)).child(field(title,"Card title")).child(field(body,"Description · Markdown supported").multiline(true).h(px(220.0)))
            .child(field(tags,"Routing tags, comma-separated; empty uses the default agent"))
            .child(action("Create in Backlog",Variant::Primary,move|_|dispatch(ui,"card.create",json!({"project_id":ui.project.get_untracked(),"title":title.get_untracked(),"body":body.get_untracked(),"tags":parse_tags(&tags.get_untracked())})))))
}
fn parse_tags(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn board(ui: Ui) -> impl IntoElement {
    layout(
        move || {
            ui.current_project()
                .map(|p| p.name)
                .unwrap_or_else(|| "Welcome to Canban".into())
        },
        "Backlog holds ideas. Moving a Card starts the stage's configured workflow.",
    )
    .child(
        view()
            .flex_row()
            .gap_3()
            .child(action("+ New Card", Variant::Primary, move |_| {
                if ui.project.get_untracked().is_empty() {
                    ui.page.set("new-project".into())
                } else {
                    ui.page.set("new-card".into())
                }
            }))
            .child(badge(move || {
                ui.snapshot.with(|s| {
                    format!(
                        "{} active cards",
                        s.cards
                            .iter()
                            .filter(|c| c.project_id == ui.project.get() && c.archived_at.is_none())
                            .count()
                    )
                })
            })),
    )
    .child(dynamic(move || {
        // A Project switch rebuilds the Board; editors live on separate routes.
        let project_id = ui.project.get();
        let snapshot = ui.snapshot.get();
        let Some(project) = snapshot.projects.iter().find(|p| p.id == project_id) else {
            return panel()
                .child(label(
                    "Create a Project to collect ideas and dispatch work.",
                ))
                .into_element();
        };
        let stages = project
            .workflow
            .stages
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>();
        let width = signal(300.0);
        view()
            .flex_1()
            .flex_row()
            .gap_3()
            .on_layout({
                let count = stages.len();
                move |b: &Bounds<Points>| {
                    width.set(
                        (b.size.width.0 - 12.0 * (count.saturating_sub(1)) as f32)
                            / (count.max(1) as f32),
                    )
                }
            })
            .children(
                project
                    .workflow
                    .stages
                    .iter()
                    .enumerate()
                    .map(|(index, stage)| {
                        let cards = snapshot
                            .cards
                            .iter()
                            .filter(|c| {
                                c.project_id == project_id
                                    && c.stage == stage.id
                                    && c.archived_at.is_none()
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        view()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap_3()
                            .child(
                                view()
                                    .flex_row()
                                    .justify_between()
                                    .child(
                                        label(stage.name.clone()).font_weight(FontWeight::SEMIBOLD),
                                    )
                                    .child(badge(cards.len().to_string())),
                            )
                            .child(muted(
                                stage
                                    .profile
                                    .as_ref()
                                    .map(|id| format!("Agent: @{id}"))
                                    .unwrap_or_else(|| {
                                        if stage.execute {
                                            "Agent: Card tags / default".into()
                                        } else {
                                            "No automatic Agent".into()
                                        }
                                    }),
                            ))
                            .child(
                                scroll().flex_1().child(
                                    view().gap_3().children(
                                        cards
                                            .into_iter()
                                            .map(|c| {
                                                card_tile(ui, c, index, width, stages.clone())
                                                    .into_element()
                                            })
                                            .collect::<Vec<_>>(),
                                    ),
                                ),
                            )
                            .into_element()
                    })
                    .collect::<Vec<_>>(),
            )
            .into_element()
    }))
}
fn card_detail(ui: Ui) -> impl IntoElement {
    let card = ui.current_card();
    let Some(card) = card else {
        return layout("Card", "Loading Card…").into_element();
    };
    let card_id = card.id.clone();
    let title = signal(card.title.clone());
    let body = signal(card.body.clone());
    let tags = signal(card.tags.join(", "));
    let comment = signal(String::new());
    let attachment = signal(String::new());
    let project = ui.current_project();
    let mut stages = view().flex_row().gap_2();
    if let Some(project) = project {
        for s in project.workflow.stages {
            let id = card.id.clone();
            let stage = s.id;
            stages = stages.child(action(s.name, Variant::Secondary, move |_| {
                move_card(ui, &id, &stage)
            }));
        }
    }
    let card_id2 = card_id.clone();
    let card_id3 = card_id.clone();
    let card_id4 = card_id.clone();
    let card_id5 = card_id.clone();
    let paste_body = card_id.clone();
    let paste_comment = card_id.clone();
    layout(move||ui.current_card().map(|c|format!("#{} · {}",c.number,c.title)).unwrap_or_default(),move||ui.current_card().map(|c|format!("{} · revision {}{}",c.stage,c.revision,if c.archived_at.is_some(){" · Archived"}else{""})).unwrap_or_default())
        .child(view().flex_row().gap_2().child(action("← Board",Variant::Quiet,move|_|ui.page.set("board".into()))).child(stages))
        .child(scroll().flex_1().child(view().gap_4()
            .child(dynamic(move||{let c=ui.current_card();let id=card_id2.clone();if let Some(message)=c.and_then(|c|c.attention){panel().child(label(message).text_color(DANGER)).child(action("Acknowledge",Variant::Secondary,move|_|dispatch(ui,"card.attention.clear",json!({"card_id":id})))).into_element()}else{view().into_element()}}))
            .child(panel().child(label("Task and context")).child(field(title,"Title")).child(field(body,"Description").multiline(true).h(px(150.0)).on_paste(move|e:&nsh_core::PasteEvent|paste_resources(ui,paste_body.clone(),e,body))).child(field(tags,"Routing tags"))
                .child(action("Save Card",Variant::Primary,move|_|{if let Some(c)=ui.current_card(){dispatch(ui,"card.update",json!({"card_id":c.id,"revision":c.revision,"title":title.get_untracked(),"body":body.get_untracked(),"tags":parse_tags(&tags.get_untracked())}));}}))
                .child(nsh_base::text_view().markdown(move||Some(body.get().into()))))
            .child(crate::sessions::card_agents(ui))
            .child(context_picker(ui))
            .child(panel().child(label("Resources")).child(field(attachment,"Absolute file path to attach"))
                .child(action("Attach file",Variant::Secondary,move|_|{let path=attachment.get_untracked();let name=std::path::Path::new(&path).file_name().map(|s|s.to_string_lossy().into_owned()).unwrap_or_default();dispatch(ui,"attachment.add",json!({"card_id":card_id3,"source":path,"name":name}));}))
                .child(dynamic(move||view().gap_2().children(ui.snapshot.with(|s|s.artifacts.iter().filter(|a|a.card_id==card_id4).map(|a|{let card=a.card_id.clone();let path=a.path.clone();action(format!("Open {} · {} bytes",a.name,a.bytes),Variant::Quiet,move|_|open_resource(ui,card.clone(),path.clone())).into_element()}).collect::<Vec<_>>())))))
            .child(panel().child(label("Timeline")).child(dynamic(move||view().children(ui.snapshot.with(|s|s.events.iter().filter(|e|e.card_id==card_id5).cloned().map(|e|timeline_item(ui,e).into_element()).collect::<Vec<_>>())))))
            .child(panel().child(label("Comment / ask an Agent")).child(muted("Choose a configured profile below, or type @profile. Mentions do not change the Card stage or its automatic Agent."))
                .child(crate::profiles::mention_picker(ui.profiles, comment))
                .child(field(comment,"Write a comment…").multiline(true).h(px(100.0)).on_paste(move|e:&nsh_core::PasteEvent|paste_resources(ui,paste_comment.clone(),e,comment)))
                .child(action("Post comment",Variant::Primary,move|_|{let body=comment.get_untracked();dispatch_then(ui,"comment.add",json!({"card_id":card_id,"body":body}),move|_|comment.set(String::new()));})))
            .child(dynamic(move||{let id=ui.card.get();view().gap_3().children(ui.snapshot.with(|s|s.requests.iter().filter(|r|r.card_id==id).cloned().map(|r|run_panel(ui,r).into_element()).collect::<Vec<_>>()))}))
            .child(output_panel(ui))
            .child(action(if card.archived_at.is_some(){"Restore Card"}else{"Archive Card and compress history"},Variant::Danger,move|_|{if let Some(c)=ui.current_card(){dispatch(ui,if c.archived_at.is_some(){"card.restore"}else{"card.archive"},json!({"card_id":c.id,"revision":c.revision}));}}))
        )).into_element()
}
fn notes(ui: Ui) -> impl IntoElement {
    let selected = signal(String::new());
    let title = signal(String::new());
    let body = signal(String::new());
    let rev = signal(0_u64);
    layout("Project Notes","Notes are included in the captured Human Context of new executions.")
        .child(dynamic(move||{let project=ui.project.get();view().flex_row().gap_2().children(ui.snapshot.with(|s|s.notes.iter().filter(|n|n.project_id==project).cloned().map(|note|action(note.title.clone(),Variant::Secondary,move|_|{selected.set(note.id.clone());title.set(note.title.clone());body.set(note.body.clone());rev.set(note.revision);}).into_element()).collect::<Vec<_>>()))}))
        .child(scroll().flex_1().child(view().gap_3().child(panel().child(field(title,"Note title")).child(field(body,"Markdown note").multiline(true).h(px(280.0)))
            .child(view().flex_row().gap_2().child(action("Save Note",Variant::Primary,move|_|{
                let id=selected.get_untracked();let current_rev=ui.snapshot.with_untracked(|s|s.notes.iter().find(|n|n.id==id).map(|n|n.revision)).unwrap_or(rev.get_untracked());
                let mut p=json!({"project_id":ui.project.get_untracked(),"title":title.get_untracked(),"body":body.get_untracked()});if !id.is_empty(){p["note_id"]=json!(id);p["revision"]=json!(current_rev);}dispatch_then(ui,"note.save",p,move|value|{if let Some(id)=value["id"].as_str(){selected.set(id.into());}rev.set(value["revision"].as_u64().unwrap_or(0));});
            })).child(action("New Note",Variant::Secondary,move|_|{selected.set(String::new());title.set(String::new());body.set(String::new());rev.set(0);}))))
            .child(panel().child(label("Preview")).child(nsh_base::text_view().markdown(move||Some(body.get().into()))))))
}
fn agents(ui: Ui) -> impl IntoElement {
    layout(
        "Agent activity",
        "One Session model · independent conversations · runtime status from Herdr",
    )
    .child(
        scroll().flex_1().child(
            view()
                .gap_3()
                .child(dynamic(move || {
                    let project = ui.project.get();
                    view().gap_3().children(ui.snapshot.with(|s| {
                        s.requests
                            .iter()
                            .filter(|r| r.project_id == project)
                            .cloned()
                            .map(|r| {
                                let id = r.card_id.clone();
                                panel()
                                    .child(crate::sessions::execution(ui, r))
                                    .child(action("Open Card", Variant::Quiet, move |_| {
                                        ui.open_card(id.clone())
                                    }))
                                    .into_element()
                            })
                            .collect::<Vec<_>>()
                    }))
                }))
                .child(output_panel(ui)),
        ),
    )
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
        .unwrap()
        .home
        .to_string_lossy()
        .into_owned();
    layout("Settings", "JSON configuration; changes apply to newly queued work after reload.")
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
fn gallery(ui: Ui) -> impl IntoElement {
    let input = signal("中文与 English · long text fixture".into());
    let profile_draft = signal(String::new());
    let profile_fixture = signal(canban_types::Config::default().profiles);
    let count = signal(0_u32);
    layout(
        "Component Gallery",
        "The same UI Kit controls and product components used in Canban.",
    )
    .child(
        view()
            .flex_row()
            .gap_2()
            .child(action("Light appearance", Variant::Secondary, |_| {
                set_color_scheme(Appearance::Light)
            }))
            .child(action("Dark appearance", Variant::Secondary, |_| {
                set_color_scheme(Appearance::Dark)
            })),
    )
    .child(
        scroll().flex_1().child(
            view()
                .gap_4()
                .child(
                    panel()
                        .child(heading("Typography and surfaces"))
                        .child(label("Body · a reusable semantic text style"))
                        .child(muted(
                            "Secondary text · long text and narrow windows should remain readable",
                        ))
                        .child(badge("Needs attention"))
                        .child(
                            action("Disabled action", Variant::Secondary, |_| {}).disabled(true),
                        ),
                )
                .child(
                    panel()
                        .child(label("Buttons · hover / press / keyboard focus"))
                        .child(
                            view()
                                .flex_row()
                                .gap_3()
                                .child(action("Primary", Variant::Primary, move |_| {
                                    count.set(count.get_untracked() + 1)
                                }))
                                .child(action("Secondary", Variant::Secondary, move |_| {
                                    count.set(count.get_untracked() + 1)
                                }))
                                .child(action("Danger", Variant::Danger, move |_| {
                                    count.set(count.get_untracked() + 1)
                                })),
                        )
                        .child(muted(move || format!("Press count: {}", count.get()))),
                )
                .child(
                    panel()
                        .child(label("Controlled input / IME / Markdown preview"))
                        .child(field(input, "Type here").multiline(true).h(px(110.0)))
                        .child(nsh_base::text_view().markdown(move || Some(input.get().into()))),
                )
                .child(
                    panel()
                        .child(label("Shared product Timeline item"))
                        .child(timeline_item(ui, canban_timeline_fixture())),
                )
                .child(
                    panel()
                        .child(label("Agent profile mention picker"))
                        .child(crate::profiles::mention_picker(
                            profile_fixture,
                            profile_draft,
                        ))
                        .child(
                            field(profile_draft, "Question with @profile")
                                .multiline(true)
                                .h(px(80.0)),
                        ),
                )
                .child(crate::profiles::registry(profile_fixture))
                .child(
                    panel().child(label("Native libghostty terminal")).child(
                        native("canban/terminal")
                            .prop("fixture", json!(true))
                            .h(px(320.0)),
                    ),
                ),
        ),
    )
}
fn canban_timeline_fixture() -> Event {
    Event{id:"gallery-event".into(),project_id:"gallery-project".into(),card_id:"gallery-card".into(),sequence:1,kind:"comment.created".into(),actor:"user".into(),message:"A **shared component**, rendered from deterministic fixture data.\n\n中文输入、选择、复制与长文本。".into(),request_id:None,data:json!({}),created_at:0}
}
