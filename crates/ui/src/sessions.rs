//! One reusable Card/profile entry and native terminal screen.
use crate::{components::run_panel, presenter::*};
use canban_types::Request;
use canban_ui_kit::*;
use nsh_core::prelude::*;
use serde_json::json;

pub fn card_agents(ui: Ui) -> impl IntoElement {
    panel()
        .child(label("Card Agents"))
        .child(muted(
            "Open a live terminal, or start/resume this Card's profile conversation.",
        ))
        .child(dynamic(move || {
            let card = ui.card.get();
            let snapshot = ui.snapshot.get();
            view().flex_row().flex_wrap().gap_2().children(
                ui.profiles
                    .get()
                    .into_iter()
                    .map(|profile| {
                        let profile_id = profile.id.clone();
                        let latest = snapshot
                            .requests
                            .iter()
                            .rev()
                            .find(|r| r.card_id == card && r.profile.id == profile_id);
                        let state = latest.map(|r| r.state.as_str()).unwrap_or("not started");
                        let card = card.clone();
                        action(
                            format!("@{profile_id} · {state}"),
                            Variant::Secondary,
                            move |_| {
                                dispatch_then(
                                    ui,
                                    "agent.open",
                                    json!({"card_id":card,"profile_id":profile.id}),
                                    move |value| {
                                        if let Some(id) = value["request_id"].as_str() {
                                            ui.request.set(id.into());
                                            ui.page.set("terminal".into());
                                        }
                                    },
                                );
                            },
                        )
                        .into_element()
                    })
                    .collect::<Vec<_>>(),
            )
        }))
}
pub fn execution(ui: Ui, request: Request) -> impl IntoElement {
    run_panel(ui, request)
}
pub fn screen(ui: Ui) -> impl IntoElement {
    view().flex_1().min_h(px(0.0)).p_4().gap_3()
        .child(dynamic(move||{
            let request_id=ui.request.get();let snapshot=ui.snapshot.get();
            let Some(request)=snapshot.requests.iter().find(|r|r.id==request_id) else{return label("Select a Card Agent to open its terminal").into_element()};
            let card_id=request.card_id.clone();let profile_id=request.profile.id.clone();
            let card=snapshot.cards.iter().find(|c|c.id==card_id);
            let run=snapshot.attempts.iter().rev().find(|a|a.request_id==request.id);
            let mut toolbar=view().gap_2().child(heading(format!("{} · @{}",card.map(|c|format!("#{} {}",c.number,c.title)).unwrap_or_default(),profile_id)))
                .child(muted(format!("{} · {} · {}",request.stage,request.state,run.map(|a|a.close_state.as_str()).unwrap_or(""))))
                .child(view().flex_row().gap_2()
                    .child(action("Open Card",Variant::Quiet,{let card_id=card_id.clone();move|_|ui.open_card(card_id.clone())}))
                    .child(action("Open / resume conversation",Variant::Primary,{let card_id=card_id.clone();let profile_id=profile_id.clone();move|_|dispatch_then(ui,"agent.open",json!({"card_id":card_id,"profile_id":profile_id}),move|v|{if let Some(id)=v["request_id"].as_str(){ui.request.set(id.into());}})})));
            if let Some(run)=run {
                let interrupt=run.id.clone();let end=run.id.clone();
                toolbar=toolbar.child(view().flex_row().gap_2()
                    .child(action("Interrupt",Variant::Secondary,move|_|dispatch(ui,"agent.interrupt",json!({"attempt_id":interrupt}))))
                    .child(action("End session",Variant::Danger,move|_|dispatch(ui,"agent.end",json!({"attempt_id":end})))));
                if run.observed.as_ref().is_some_and(|v|v["state"]=="finished") {
                    let card_id=card_id.clone();let profile_id=profile_id.clone();
                    toolbar=toolbar.child(action("Start with retained Card context",Variant::Secondary,move|_|dispatch_then(ui,"agent.open",json!({"card_id":card_id,"profile_id":profile_id,"fresh":true}),move|v|{if let Some(id)=v["request_id"].as_str(){ui.request.set(id.into());}})));
                }
                if let Some(error)=&run.error{toolbar=toolbar.child(label(error.clone()));}
            }
            toolbar.into_element()
        }))
        .child(dynamic(move||{
            let request_id=ui.request.get();let snapshot=ui.snapshot.get();
            let Some(run)=snapshot.attempts.iter().rev().find(|a|a.request_id==request_id && a.terminal.is_some()) else{return panel().child(label("Waiting for the Agent terminal…")).into_element()};
            let binding=json!({"config":crate::APP.get().unwrap().config(),"terminal":run.terminal,"core_id":run.core_id});
            native("canban/terminal").key(run.id.clone()).prop("binding",binding).prop("read_only",json!(matches!(run.close_state.as_str(),"pending"|"closing"|"closed"))).flex_1().min_h(px(200.0)).into_element()
        }))
}
