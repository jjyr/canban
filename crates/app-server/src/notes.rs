//! Database-backed document operations shared by native editors and Agents.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Origin {
    actor: String,
    card: Option<String>,
    terminal_id: Option<String>,
}
fn origin(db: Db<'_>, p: &Value, project: &str) -> Result<Origin> {
    if let Some(tid) = p["terminal_id"]
        .as_str()
        .or_else(|| p["execution"]["terminal_id"].as_str())
        .filter(|s| !s.is_empty())
    {
        let entry: TerminalEntry = db.get(tid)?;
        if let Some(att) = entry.agent {
            ensure!(
                att.project_id == project,
                "Agent execution belongs to a different Project"
            );
            return Ok(Origin {
                actor: format!("@{}", att.profile.id),
                card: Some(att.card_id),
                terminal_id: Some(entry.id),
            });
        }
    }
    let Some(execution) = p.get("execution").filter(|v| !v.is_null()) else {
        if let Some(profile) = p["profile_id"].as_str().filter(|s| !s.is_empty()) {
            let card = super::resolve_card(db, p).ok();
            if let Some(ref c) = card {
                ensure!(
                    c.project_id == project,
                    "Agent execution belongs to a different Project"
                );
            }
            return Ok(Origin {
                actor: format!("@{profile}"),
                card: card.map(|c| c.id),
                terminal_id: None,
            });
        }
        return Ok(Origin {
            actor: "user".into(),
            ..Default::default()
        });
    };
    let profile = execution["profile_id"]
        .as_str()
        .or_else(|| p["profile_id"].as_str())
        .filter(|s| !s.is_empty());
    let card = super::resolve_card(db, execution)
        .or_else(|_| super::resolve_card(db, p))
        .ok();
    if let Some(ref c) = card {
        ensure!(
            c.project_id == project,
            "Agent execution belongs to a different Project"
        );
    }
    let active_entry = card.as_ref().and_then(|c| {
        db.list::<TerminalEntry>().ok().and_then(|mut list| {
            list.retain(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == c.id
                        && !a.finished()
                        && profile.is_none_or(|pid| a.profile.id == pid)
                })
            });
            list.sort_by_key(|t| t.created_at);
            list.pop()
        })
    });
    let actor = profile
        .map(|pid| format!("@{pid}"))
        .or_else(|| {
            active_entry
                .as_ref()
                .and_then(|t| t.agent.as_ref())
                .map(|a| format!("@{}", a.profile.id))
        })
        .unwrap_or_else(|| "agent".into());
    Ok(Origin {
        actor,
        card: card.map(|c| c.id),
        terminal_id: active_entry.map(|t| t.id),
    })
}
#[allow(clippy::too_many_arguments)]
fn emit(
    db: Db<'_>,
    note: &Note,
    kind: &str,
    message: String,
    origin: &Origin,
    data: Value,
    now_ms: u64,
    id_gen: &dyn canban_types::IdGen,
) -> Result<()> {
    db.save(&NoteEvent {
        id: id_gen.next_id(),
        project_id: note.project_id.clone(),
        note_id: note.id.clone(),
        sequence: db.counter(&format!("note-timeline:{}", note.id))?,
        kind: kind.into(),
        actor: origin.actor.clone(),
        message,
        card_id: origin.card.clone(),
        revision: note.revision,
        data,
        created_at: now_ms,
    })
}
fn revision(db: Db<'_>, n: &Note, now_ms: u64) -> Result<()> {
    db.save(&NoteRevision {
        id: format!("{}:{}", n.id, n.revision),
        project_id: n.project_id.clone(),
        note_id: n.id.clone(),
        revision: n.revision,
        title: n.title.clone(),
        body: n.body.clone(),
        created_at: now_ms,
    })
}
/// Parse Markdown links rather than treating examples inside code as references.
fn links(body: &str) -> Result<Vec<String>> {
    use markdown::mdast::Node;
    fn definitions(node: &Node, out: &mut BTreeMap<String, String>) {
        if let Node::Definition(d) = node {
            out.insert(d.identifier.to_lowercase(), d.url.clone());
        }
        if let Some(children) = node.children() {
            for n in children {
                definitions(n, out);
            }
        }
    }
    fn visit(node: &Node, defs: &BTreeMap<String, String>, out: &mut BTreeSet<String>) {
        let url = match node {
            Node::Link(l) => Some(l.url.as_str()),
            Node::LinkReference(l) => defs.get(&l.identifier.to_lowercase()).map(String::as_str),
            _ => None,
        };
        if let Some(id) = url.and_then(|u| u.strip_prefix("note:"))
            && !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            out.insert(id.into());
        }
        if let Some(children) = node.children() {
            for n in children {
                visit(n, defs, out);
            }
        }
    }
    let root = markdown::to_mdast(body, &markdown::ParseOptions::default())
        .map_err(|e| anyhow::anyhow!("Invalid Markdown: {e}"))?;
    let mut defs = BTreeMap::new();
    definitions(&root, &mut defs);
    let mut ids = BTreeSet::new();
    visit(&root, &defs, &mut ids);
    Ok(ids.into_iter().collect())
}

pub(super) fn relate_card(
    db: Db<'_>,
    note_id: &str,
    card: &Card,
    linked: bool,
    now_ms: u64,
    id_gen: &dyn canban_types::IdGen,
) -> Result<()> {
    let mut note: Note = db.get(note_id)?;
    ensure!(
        note.project_id == card.project_id,
        "Note belongs to a different Project"
    );
    let contains = note.card_ids.contains(&card.id);
    if contains == linked {
        return Ok(());
    }
    if linked {
        note.card_ids.push(card.id.clone());
    } else {
        note.card_ids.retain(|id| id != &card.id);
    }
    db.save(&note)?;
    emit(
        db,
        &note,
        if linked {
            "note.card_linked"
        } else {
            "note.card_unlinked"
        },
        format!(
            "{} Card #{} · {}",
            if linked { "Linked" } else { "Unlinked" },
            card.number,
            card.title
        ),
        &Origin {
            actor: "user".into(),
            card: Some(card.id.clone()),
            ..Default::default()
        },
        json!({"card_id":card.id}),
        now_ms,
        id_gen,
    )
}

impl AppServer {
    pub(super) fn note_read(&self, method: &str, p: &Value) -> Result<Value> {
        let store = self.store.lock().unwrap();
        let db = store.db();
        let filter_pid = p["project_id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|raw| super::resolve_project_id(db, raw))
            .transpose()?;
        if method == "note.list" {
            let notes = db
                .list::<Note>()?
                .into_iter()
                .filter(|n| filter_pid.as_deref().is_none_or(|id| n.project_id == id))
                .collect::<Vec<_>>();
            return Ok(serde_json::to_value(notes)?);
        }
        let n: Note = db.get(strp(p, "note_id")?)?;
        if let Some(project) = filter_pid.as_deref() {
            ensure!(
                n.project_id == project,
                "Note belongs to a different Project"
            );
        }
        match method {
            "note.history" => Ok(serde_json::to_value(
                db.list::<NoteEvent>()?
                    .into_iter()
                    .filter(|e| e.note_id == n.id)
                    .collect::<Vec<_>>(),
            )?),
            "note.revision" => {
                let r: NoteRevision = db.get(&format!(
                    "{}:{}",
                    n.id,
                    p["revision"].as_u64().context("revision required")?
                ))?;
                Ok(serde_json::to_value(r)?)
            }
            _ => Ok(serde_json::to_value(n)?),
        }
    }
    pub(super) fn note_write(&self, method: &str, p: Value) -> Result<Value> {
        if method == "note.write" {
            ensure!(p["body"].is_string(), "Note body is required");
        }
        if method == "note.rename" {
            ensure!(p["title"].is_string(), "Note name is required");
        }
        self.mutate(method,&p,|db| {
            let old=p["note_id"].as_str().map(|id|db.get::<Note>(id)).transpose()?;
            let resolved_param_pid = p["project_id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|raw| super::resolve_project_id(db, raw))
                .transpose()?;
            let project=old.as_ref().map(|n|n.project_id.clone()).or(resolved_param_pid.clone()).context("project_id required for a new Note")?;
            let _:Project=db.get(&project)?;
            if let Some(scope)=resolved_param_pid.as_deref(){ensure!(scope==project,"Note belongs to a different Project");}
            let source=origin(db,&p,&project)?;
            let now_ms = self.now_ms();
            let mut n=old.clone().unwrap_or_else(||Note{id:self.next_id(),project_id:project.clone(),title:String::new(),body:String::new(),revision:0,created_at:now_ms,updated_at:now_ms,links:Vec::new(),card_ids:Vec::new()});
            if old.is_some(){ensure!(p["revision"].as_u64()==Some(n.revision),"Note revision conflict: reload the saved version; your draft has not been changed");}
            if let Some(title)=p["title"].as_str(){n.title=title.trim().into();}
            if let Some(body)=p["body"].as_str(){n.body=body.into();}
            ensure!(!n.title.is_empty(),"Note name is required");
            ensure!(!db.list::<Note>()?.iter().any(|other|other.project_id==project && other.id!=n.id && other.title.to_lowercase()==n.title.to_lowercase()),"A Note with this name already exists in the Project");
            n.links=links(&n.body)?;
            for target in &n.links {if let Ok(other)=db.get::<Note>(target){ensure!(other.project_id==project,"Linked Note belongs to a different Project");}}
            if old.as_ref().is_some_and(|o|o.title==n.title && o.body==n.body && o.links==n.links){return Ok(serde_json::to_value(n)?)}
            n.revision+=1;n.updated_at=now_ms;
            if let Some(card)=&source.card && !n.card_ids.contains(card){n.card_ids.push(card.clone());}
            db.save(&n)?;revision(db,&n,now_ms)?;
            let kind=if old.is_none(){"note.created"}else if old.as_ref().is_some_and(|o|o.title!=n.title){"note.renamed"}else{"note.updated"};
            emit(db,&n,kind,format!("{} {}",if old.is_none(){"Created"}else if kind=="note.renamed"{"Renamed"}else{"Updated"},n.title),&source,json!({"previous_revision":old.as_ref().map(|o|o.revision),"links":n.links}),now_ms,self.id_gen.as_ref())?;
            let previous=old.as_ref().map(|o|o.links.clone()).unwrap_or_default();
            for target in previous.iter().chain(&n.links).collect::<BTreeSet<_>>() {
                let before=previous.contains(target);let after=n.links.contains(target);
                if before==after {continue}
                if let Ok(other)=db.get::<Note>(target) {emit(db,&other,if after{"note.referenced"}else{"note.unreferenced"},format!("{} {}",if after{"Referenced by"}else{"Reference removed from"},n.title),&source,json!({"source_note_id":n.id}),now_ms,self.id_gen.as_ref())?;}
            }
            if let Some(card)=&source.card {let card:Card=db.get(card)?;log(db,&card,kind,&source.actor,&format!("Updated Note · {}",n.title),source.terminal_id.clone(),json!({"note_id":n.id,"revision":n.revision}),now_ms)?;}
            Ok(serde_json::to_value(n)?)
        })
    }
}
