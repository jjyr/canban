use super::*;
pub(super) fn blocker(db: Db<'_>, card: &Card) -> Result<Option<String>> {
    if db.list::<Attempt>()?.iter().any(|a| {
        a.card_id == card.id
            && a.terminal.is_some()
            && a.observed.as_ref().is_none_or(|v| v["state"] != "finished")
    }) {
        return Ok(Some(
            "End active Card Agent terminals before accepting code".into(),
        ));
    }
    if db.list::<Request>()?.iter().any(|r| {
        r.card_id == card.id && matches!(r.state.as_str(), "queued" | "starting" | "unknown")
    }) {
        return Ok(Some(
            "Card has queued or uncertain work; resolve it before accepting code".into(),
        ));
    }
    Ok(None)
}
impl AppServer {
    pub(super) fn review_preview(&self, p: Value) -> Result<Value> {
        let (card, project, reason) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let c: Card = db.get(strp(&p, "card_id")?)?;
            let project: Project = db.get(&c.project_id)?;
            let project = self.bound_project(&project, &c);
            let reason = blocker(db, &c)?;
            (c, project, reason)
        };
        let mut plan = self.ops(&project.work_location().machine).review_preview(
            &project,
            &card,
            p["target_branch"].as_str(),
        )?;
        if reason.is_some() {
            plan.merge_allowed = false;
            plan.reason = reason;
        }
        Ok(serde_json::to_value(plan)?)
    }
    pub(super) fn review_merge(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Code acceptance requires explicit human review"
        );
        let _gate = self.review_gate.try_lock().map_err(|_| {
            anyhow::anyhow!("Another code delivery is in progress; retry after it completes")
        })?;
        let key = strp(&p, "request_key")?.to_owned();
        // A stored intent must win before any Git effects. Duplicate/unknown
        // delivery requests are never blindly replayed after a lost reply.
        let mut execute = false;
        let mut context = None;
        let saved=self.mutate("review.merge",&p,|db|{
            let card:Card=db.get(strp(&p,"card_id")?)?;revise(&card,&p)?;
            ensure!(card.active(),"Restore the Card before accepting");
            ensure!(card.pull_request.as_ref().is_none_or(|pr|pr.merged),"Resolve the existing PR delivery before local merge");
            if let Some(reason)=blocker(db,&card)? {bail!("{reason}");}
            let project:Project=db.get(&card.project_id)?;let project=self.bound_project(&project,&card);
            let plan=self.ops(&project.work_location().machine).review_preview(&project,&card,p["target_branch"].as_str())?;
            ensure!(plan.token==strp(&p,"token")?,"Review is stale; refresh the diff before accepting");
            ensure!(plan.merge_allowed,"{}",plan.reason.as_deref().unwrap_or("Merge unavailable"));
            log(db,&card,"review.merge_started","user","Accepted reviewed changes for commit/merge",None,json!({"request_key":key,"target_branch":plan.target_branch,"target_head":plan.target_head,"card_head":plan.card_head,"token":plan.token}))?;
            execute=true;context=Some((card,project,plan));
            Ok(json!({"state":"unknown","request_key":key,"message":"Merge intent saved; inspect Git heads before repeating an uncertain delivery"}))
        })?;
        if !execute {
            return Ok(saved);
        }
        let (card, project, plan) = context.unwrap();
        let message = p["message"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Canban #{}: {}", card.number, card.title));
        let result = self.ops(&project.work_location().machine).review_merge(
            &project,
            &card,
            Some(&plan.target_branch),
            &plan.token,
            &message,
        );
        let value=self.store.lock().unwrap().write(|db|{
            let mut current:Card=db.get(&card.id)?;
            let value=match result {
                Ok(commit)=>{
                    current.delivery_mode=Some(DeliveryMode::Local);
                    current.delivery=Some(Delivery{target_branch:plan.target_branch.clone(),commit:commit.clone(),request_key:key.clone(),merged_at:now()});
                    current.attention=None;
                    if let Some(gate)=current.human_gate.take() && let Some(request)=gate.request_id {
                        let request:Request=db.get(&request)?;
                        if request.state=="completed" { crate::workflow::advance(db,&self.config(),&mut current,&request)?; }
                    }
                    log(db,&current,"review.merged","user",&format!("Merged into {}",plan.target_branch),None,json!({"commit":commit,"target_branch":plan.target_branch,"request_key":key}))?;
                    json!({"state":"merged","commit":commit,"target_branch":plan.target_branch,"card_id":card.id})
                }
                Err(error)=>{
                    let message=format!("{error:#}");attention(db,&mut current,&message)?;
                    log(db,&current,"review.merge_failed","system",&message,None,json!({"target_branch":plan.target_branch,"request_key":key}))?;
                    json!({"state":"failed","message":message,"target_branch":plan.target_branch,"card_id":card.id})
                }
            };
            current.revision+=1;db.save(&current)?;
            db.conn.execute("UPDATE operations SET result=?2 WHERE id=?1",params![key,serde_json::to_string(&value)?])?;
            Ok(value)
        })?;
        self.git_changed(&project.work_location().machine);
        self.emit("review", Some(card.id));
        Ok(value)
    }
}
