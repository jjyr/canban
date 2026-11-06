use super::*;

impl AppServer {
    pub(super) fn report(&self, p: Value) -> Result<Value> {
        let request_id = p["execution"]["request_id"]
            .as_str()
            .context("execution.request_id is required")?;
        let number = u32::try_from(
            p["execution"]["attempt_number"]
                .as_u64()
                .context("execution.attempt_number is required")?,
        )?;
        ensure!(number > 0, "attempt_number must be positive");
        let state = strp(&p, "state")?;
        ensure!(
            ["working", "completed", "blocked", "failed"].contains(&state),
            "invalid_report: unsupported state"
        );
        let key = strp(&p, "request_key")?;
        let fingerprint = canban_context::digest(&serde_json::to_vec(&p)?);
        // Validate association and duplicate key before publishing any report file.
        let (req, attempt, card) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let req: Request = db.get(request_id)?;
            let attempt: Attempt = db.get(&Attempt::key(request_id, number))?;
            let card: Card = db.get(&req.card_id)?;
            (req, attempt, card)
        };
        let report_id = id();
        let path = if let Some(body) = p["body"].as_str() {
            let path = canban_context::run_dir(&self.home, &req, number)
                .join("reports")
                .join(format!("{report_id}.md"));
            canban_context::atomic(&path, body.as_bytes())?;
            Some(canban_context::relative(&self.home, &path)?)
        } else {
            None
        };
        let config = self.config();
        self.mutate("execution.report",&p,|db|{
            if let Some(old)=db.list::<Report>()?.into_iter().find(|r|r.request_key==key){ensure!(old.fingerprint==fingerprint,"request_key_conflict");return Ok(json!({"accepted":true,"duplicate":true}));}
            let mut req:Request=db.get(&req.id)?;let mut run:Attempt=db.get(&attempt.id)?;let mut card:Card=db.get(&card.id)?;
            let report=Report{id:report_id,request_id:req.id.clone(),project_id:req.project_id.clone(),card_id:req.card_id.clone(),attempt:number,state:state.into(),message:p["message"].as_str().unwrap_or("").into(),path,request_key:key.into(),fingerprint,created_at:now()};db.save(&report)?;
            log(db,&card,"execution.reported",&format!("agent:{}",req.profile.id),&format!("{state}: {}",report.message),Some(req.id.clone()),json!({"report_id":report.id,"path":report.path,"attempt":number,"state":state}))?;
            let current=req.attempt==number;
            if run.state!="completed" {run.state=state.into();db.save(&run)?;}
            if current && req.state!="completed" {
                req.state=state.into();db.save(&req)?;
                if state=="blocked"||state=="failed"{attention(db,&mut card,if report.message.is_empty(){state}else{&report.message})?;}
                if state=="completed" && card.archived_at.is_none() {
                    card.attention=None;
                    if let Some(next)=&req.on_completed&& card.stage==req.stage {let project:Project=db.get(&card.project_id)?;let stage=canban_workflow_engine::stage(&project.workflow,next)?;let from=card.stage.clone();card.stage=next.clone();log(db,&card,"card.stage_changed","system",&format!("{from} → {next}"),Some(req.id.clone()),json!({"reason":"agent_completed"}))?;enqueue(db,&config,&mut card,&project,stage,"stage_entry",None,None)?;}
                }
                card.revision+=1;db.save(&card)?;
            }
            Ok(json!({"accepted":true,"duplicate":false,"recorded_state":state,"execution":{"request_id":request_id,"attempt_number":number}}))
        })
    }
}
