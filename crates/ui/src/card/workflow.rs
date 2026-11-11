use super::*;
use canban_types::WorkflowStep;
use serde_json::json;
pub fn render(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let card_id = ui.card_id();
        let runs = ui.snapshot.with(|s| {
            s.workflow_runs
                .iter()
                .filter(|r| {
                    r.card_id == card_id
                        && r.active()
                        && matches!(r.state.as_str(), "waiting_approval" | "stale")
                })
                .cloned()
                .collect::<Vec<_>>()
        });
        view().gap_2().children(runs.into_iter().rev().map(|run| {
            let mut body=view().p_2().border_t_1().border_color(BORDER).gap_2().child(muted(format!("{} · {}",run.definition.name, if run.state == "stale" { "Changes required" } else { "Review required" })));
            if run.active() && (run.state=="waiting_approval"||run.state=="stale") {
                    if run.state=="waiting_approval" {
                        let id=run.id.clone();let visit=run.visit;
                        body=body.child(action("Approve and continue",Variant::Primary,move |_|{if let Some(card)=ui.current_card(){dispatch(ui,"workflow.decide",json!({"run_id":id,"visit":visit,"revision":card.revision,"decision":"approve"}));}}));
                    }
                    let can_rework=run.state=="stale" || run.step.as_ref().is_some_and(|k|matches!(&run.definition.steps[k],WorkflowStep::Approval{rework:Some(_),..}));
                    if can_rework {let id=run.id.clone();let visit=run.visit;body=body.child(action("Request changes",Variant::Secondary,move |_|{if let Some(card)=ui.current_card(){dispatch(ui,"workflow.decide",json!({"run_id":id,"visit":visit,"revision":card.revision,"decision":"rework"}));}}));}
            }
            for output in run.outputs.values(){if let Some(path)=output["path"].as_str(){let path=path.to_owned();let card=card_id.clone();body=body.child(action("Read step report",Variant::Quiet,move |_|open_resource(ui,card.clone(),path.clone())));}}
            body.into_element()
        }).collect::<Vec<_>>())
    })
}
