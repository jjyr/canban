//! Human approval and stage progression share the same transactional boundary.
use super::*;

pub(super) fn advance(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    request: &Request,
) -> Result<()> {
    if let Some(next) = &request.on_completed
        && card.stage == request.stage
        && card.active()
    {
        let project: Project = db.get(&card.project_id)?;
        let stage = canban_workflow_engine::stage(&project.workflow, next)?;
        let from = card.stage.clone();
        card.stage = next.clone();
        log(
            db,
            card,
            "card.stage_changed",
            "system",
            &format!("{from} → {next}"),
            Some(request.id.clone()),
            json!({"reason":"workflow_completed"}),
        )?;
        enqueue(db, config, card, &project, stage, "stage_entry", None, None)?;
    }
    Ok(())
}
impl AppServer {
    pub(super) fn continue_card(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Human review must be approved by the user, not an Agent invocation"
        );
        let config = self.config();
        self.mutate("card.continue", &p, |db| {
            let mut card: Card = db.get(strp(&p, "card_id")?)?;
            revise(&card, &p)?;
            ensure!(card.active(), "Restore the Card before continuing");
            let gate = card
                .human_gate
                .clone()
                .context("Card is not waiting for human review")?;
            if let Some(id) = gate.request_id {
                let mut request: Request = db.get(&id)?;
                if request.state == "completed" {
                    card.human_gate = None;
                    card.attention = None;
                    advance(db, &config, &mut card, &request)?;
                } else if matches!(
                    request.state.as_str(),
                    "blocked" | "failed" | "needs_report"
                ) {
                    ensure!(
                        !db.list::<Attempt>()?
                            .iter()
                            .any(|a| a.request_id == request.id
                                && a.terminal.is_some()
                                && a.observed.as_ref().is_none_or(|v| v["state"] != "finished")),
                        "Answer the live Agent first, or end it before retrying"
                    );
                    request.state = "queued".into();
                    db.save(&request)?;
                    card.human_gate = None;
                    card.attention = None;
                } else {
                    ensure!(
                        request.state != "unknown",
                        "Inspect the uncertain launch before explicitly retrying it"
                    );
                    card.human_gate = None;
                    card.attention = None;
                }
            } else {
                card.human_gate = None;
                card.attention = None;
                // Routing may have failed before a request could be persisted.
                // Explicit approval can requeue that stage after its configuration is fixed.
                let already_queued = db.list::<Request>()?.iter().any(|r| {
                    r.card_id == card.id && r.stage == card.stage && r.trigger == "stage_entry"
                });
                if !already_queued {
                    let project: Project = db.get(&card.project_id)?;
                    let stage = canban_workflow_engine::stage(&project.workflow, &card.stage)?;
                    enqueue(
                        db,
                        &config,
                        &mut card,
                        &project,
                        stage,
                        "stage_entry",
                        None,
                        None,
                    )?;
                }
            }
            card.revision += 1;
            log(
                db,
                &card,
                "workflow.approved",
                "user",
                "Human reviewed and continued the workflow",
                None,
                json!({}),
            )?;
            db.save(&card)?;
            Ok(serde_json::to_value(card)?)
        })
    }
}
