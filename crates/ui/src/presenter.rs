use crate::drafts::Draft;
use canban_types::*;
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[derive(Clone, Copy)]
pub struct Ui {
    pub snapshot: Signal<Snapshot>,
    pub profiles: Signal<Vec<Profile>>,
    pub max_agents: Signal<usize>,
    pub idle_minutes: Signal<u64>,
    pub overview_selection: Signal<String>,
    pub overview_selected: Signal<Option<canban_notifications::OverviewRow>>,
    pub overview_filter: Signal<String>,
    pub overview_closed: Signal<bool>,
    pub overview_width: Signal<f32>,
    pub return_overview: Signal<bool>,
    pub project: Signal<String>,
    pub card: Signal<String>,
    pub request: Signal<String>,
    pub page: Signal<String>,
    pub status: Signal<String>,
    pub pending: Signal<bool>,
    pub refreshing: Signal<bool>,
    pub output: Signal<String>,
    pub output_observed_at: Signal<Option<u64>>,
    pub overview_history: Signal<bool>,
    pub output_request: Signal<u64>,
    pub review: Signal<Option<ReviewPreview>>,
    pub review_request: Signal<u64>,
    pub board_filter: Signal<String>,
    pub note: Signal<String>,
    pub source_note: Signal<String>,
    pub drafts: Stored<BTreeMap<String, Draft>>,
    pub scrolls: Stored<BTreeMap<String, Point<Points>>>,
    pub narrow: Signal<bool>,
    pub expanded: Signal<bool>,
    pub focused_card: Signal<bool>,
    pub board_width: Signal<f32>,
    pub focus_width: Signal<f32>,
}
impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}
impl Ui {
    pub fn clear_output(self) {
        self.output_request
            .set(self.output_request.get_untracked() + 1);
        self.output.set(String::new());
        self.output_observed_at.set(None);
    }
    pub fn navigate(self, page: &str) {
        self.return_overview.set(false);
        self.clear_output();
        self.review_request
            .set(self.review_request.get_untracked() + 1);
        self.page.set(page.into());
    }
    pub fn new() -> Self {
        Self {
            snapshot: signal(Snapshot::default()),
            idle_minutes: signal(
                crate::APP
                    .get()
                    .map(|a| a.config().scheduler.idle_minutes)
                    .unwrap_or(15),
            ),
            overview_selection: signal(String::new()),
            overview_selected: signal(None),
            overview_filter: signal("All".into()),
            overview_closed: signal(false),
            overview_width: signal(700.0),
            return_overview: signal(false),
            max_agents: signal(
                crate::APP
                    .get()
                    .map(|a| a.config().scheduler.max_agents)
                    .unwrap_or(8),
            ),
            profiles: signal(
                crate::APP
                    .get()
                    .map(|app| app.config().profiles)
                    .unwrap_or_default(),
            ),
            project: signal(String::new()),
            card: signal(String::new()),
            request: signal(String::new()),
            page: signal("board".into()),
            status: signal("Opening local workspace…".into()),
            pending: signal(false),
            refreshing: signal(false),
            output: signal(String::new()),
            output_observed_at: signal(None),
            overview_history: signal(false),
            output_request: signal(0),
            review: signal(None),
            review_request: signal(0),
            board_filter: signal("all".into()),
            note: signal(String::new()),
            source_note: signal(String::new()),
            drafts: stored(BTreeMap::new()),
            scrolls: stored(BTreeMap::new()),
            narrow: signal(false),
            expanded: signal(false),
            focused_card: signal(false),
            board_width: signal(700.0),
            focus_width: signal(420.0),
        }
    }
    pub fn current_card(self) -> Option<Card> {
        self.snapshot.with(|s| {
            s.cards
                .iter()
                .find(|c| c.id == self.card.get_untracked())
                .cloned()
        })
    }
    pub fn current_project(self) -> Option<Project> {
        self.snapshot.with(|s| {
            s.projects
                .iter()
                .find(|p| p.id == self.project.get_untracked())
                .cloned()
        })
    }
    pub fn open_card(self, id: String) {
        self.review.set(None);
        self.review_request
            .set(self.review_request.get_untracked() + 1);
        if let Some(project) = self.snapshot.with_untracked(|s| {
            s.cards
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.project_id.clone())
        }) {
            self.project.set(project);
        }
        self.card.set(id);
        self.expanded.set(false);
        self.focused_card.set(false);
        self.output.set(String::new());
        self.output_observed_at.set(None);
        self.output_request
            .set(self.output_request.get_untracked() + 1);
        self.page.set("card".into());
    }
    pub fn back_board(self) {
        if self.return_overview.get_untracked() {
            self.clear_output();
            self.page.set("overview".into());
            self.expanded.set(false);
            self.focused_card.set(false);
            return;
        }
        self.review_request
            .set(self.review_request.get_untracked() + 1);
        self.page.set(
            if self.card.get_untracked().is_empty() || self.narrow.get_untracked() {
                "board".into()
            } else {
                "card".into()
            },
        );
        self.expanded.set(false);
        self.focused_card.set(false);
    }
    pub fn open_note(self, id: String) {
        if let Some(project) = self.snapshot.with_untracked(|s| {
            s.notes
                .iter()
                .find(|n| n.id == id)
                .map(|n| n.project_id.clone())
        }) {
            self.project.set(project);
        }
        self.output_request
            .set(self.output_request.get_untracked() + 1);
        self.output.set(String::new());
        self.output_observed_at.set(None);
        self.note.set(id);
        self.page.set("notes".into());
    }
}
pub fn refresh(ui: Ui) {
    if ui.refreshing.get_untracked() {
        return;
    }
    ui.refreshing.set(true);
    let Some(app) = crate::APP.get().cloned() else {
        ui.status
            .set("Gallery preview: no AppServer connected".into());
        return;
    };
    spawn(async move {
        let result = blocking(move || {
            app.snapshot().map(|snapshot| {
                (
                    snapshot,
                    app.config().profiles,
                    app.config().scheduler.max_agents,
                    app.config().scheduler.idle_minutes,
                )
            })
        })
        .await;
        match result {
            Ok(Ok((snapshot, profiles, max_agents, idle_minutes))) => {
                ui.profiles.set(profiles);
                ui.max_agents.set(max_agents);
                ui.idle_minutes.set(idle_minutes);
                if ui.project.get_untracked().is_empty()
                    && let Some(p) = snapshot.projects.first()
                {
                    ui.project.set(p.id.clone());
                }
                let different = ui.snapshot.with_untracked(|old| {
                    serde_json::to_value(old).ok() != serde_json::to_value(&snapshot).ok()
                });
                if different {
                    ui.snapshot.set(snapshot);
                }
                if ui.status.get_untracked() == "Opening local workspace…" {
                    ui.status.set("Local workspace ready · SQLite WAL".into());
                }
            }
            Ok(Err(e)) => ui.status.set(format!("Could not refresh: {e:#}")),
            Err(e) => ui.status.set(format!("Refresh worker: {e}")),
        }
        ui.refreshing.set(false);
    });
}
pub fn dispatch(ui: Ui, method: &'static str, params: Value) {
    dispatch_then(ui, method, params, |_| {});
}
pub fn dispatch_then(
    ui: Ui,
    method: &'static str,
    params: Value,
    after: impl FnOnce(Value) + 'static,
) {
    dispatch_complete(ui, method, params, move |result| {
        if let Ok(value) = result {
            after(value);
        }
    });
}
pub fn dispatch_complete(
    ui: Ui,
    method: &'static str,
    mut params: Value,
    after: impl FnOnce(Result<Value, String>) + 'static,
) {
    if ui.pending.get_untracked() {
        after(Err(
            "Another operation is pending; your draft is retained".into()
        ));
        return;
    }
    let Some(app) = crate::APP.get().cloned() else {
        after(Err("Gallery preview: no AppServer connected".into()));
        return;
    };
    ui.pending.set(true);
    ui.status.set(format!("{method}…"));
    if params.get("request_key").is_none() {
        params["request_key"] = json!(id());
    }
    spawn(async move {
        let result = blocking(move || {
            let value = app.call(method, params)?;
            Ok::<_, anyhow::Error>((
                value,
                app.snapshot()?,
                app.config().profiles,
                app.config().scheduler.max_agents,
                app.config().scheduler.idle_minutes,
            ))
        })
        .await;
        match result {
            Ok(Ok((value, snapshot, profiles, max_agents, idle_minutes))) => {
                ui.profiles.set(profiles);
                ui.max_agents.set(max_agents);
                ui.idle_minutes.set(idle_minutes);
                ui.snapshot.set(snapshot);
                ui.status.set(match value["state"].as_str() {
                    Some("failed" | "unknown") => format!(
                        "{}: {}",
                        method,
                        value["message"]
                            .as_str()
                            .unwrap_or("Inspect before retrying")
                    ),
                    _ => "Saved".into(),
                });
                match method {
                    "project.create" => {
                        if let Some(id) = value["id"].as_str() {
                            ui.project.set(id.into());
                            ui.card.set(String::new());
                            ui.page.set("board".into());
                        }
                    }
                    "card.create" => {
                        if let Some(id) = value["id"].as_str() {
                            ui.open_card(id.into());
                            ui.source_note.set(String::new());
                        }
                    }
                    "card.archive" => ui.page.set("archive".into()),
                    "herdr.status" => ui.status.set(format!(
                        "Herdr connected · core {}",
                        value["core"]["id"].as_str().unwrap_or("")
                    )),
                    _ => {}
                }
                after(Ok(value));
            }
            Ok(Err(e)) => {
                let error = format!("{e:#}");
                ui.status.set(error.clone());
                after(Err(error));
            }
            Err(e) => {
                let error = e.to_string();
                ui.status.set(error.clone());
                after(Err(error));
            }
        }
        ui.pending.set(false);
    });
}
pub fn move_card(ui: Ui, card_id: &str, target: &str) {
    let card = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card_id).cloned());
    if let Some(c) = card {
        dispatch(
            ui,
            "card.move",
            json!({"card_id":c.id,"revision":c.revision,"stage":target}),
        );
    }
}
pub fn load_text(ui: Ui, method: &'static str, params: Value) {
    let generation = ui.output_request.get_untracked() + 1;
    ui.output_request.set(generation);
    ui.output.set("Loading…".into());
    let Some(app) = crate::APP.get().cloned() else {
        ui.status
            .set("Gallery preview: no AppServer connected".into());
        return;
    };
    spawn(async move {
        let result = blocking(move || app.call(method, params)).await;
        if ui.output_request.get_untracked() != generation {
            return;
        }
        ui.output_observed_at.set(if method == "overview.preview" {
            result
                .as_ref()
                .ok()
                .and_then(|r| r.as_ref().ok())
                .and_then(|v| v["observed_at"].as_u64())
        } else {
            None
        });
        ui.output.set(match result {
            Ok(Ok(v)) if method == "overview.preview" => format!(
                "Herdr screen · {}\n\n{}",
                v["terminal"].as_str().unwrap_or(""),
                v["text"].as_str().unwrap_or("")
            ),
            Ok(Ok(v)) => v["body"]
                .as_str()
                .or(v["text"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string()),
            Ok(Err(e)) => format!("Unable to load: {e:#}"),
            Err(e) => e.to_string(),
        });
    });
}

pub fn paste_resources(
    ui: Ui,
    card_id: String,
    event: &nsh_core::PasteEvent,
    draft: Signal<String>,
    comment: bool,
) {
    if event.files.is_empty() {
        return;
    }
    let files = event.files.clone();
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    let owner = format!("card:{card_id}");
    spawn(async move {
        let result=blocking(move||{let mut refs=Vec::new();for file in files{let value=app.call("attachment.add",json!({"card_id":card_id,"name":file.name,"media_type":file.mime,"bytes":file.data,"request_key":id()}))?;refs.push(format!("[{}](attachment:{})",value["name"].as_str().unwrap_or("Attachment"),value["id"].as_str().unwrap_or("")));}Ok::<_,anyhow::Error>(refs)}).await;
        match result {
            Ok(Ok(refs)) => {
                let addition = format!("\n\n{}", refs.join("\n"));
                if let Some(mut body) = draft.try_get() {
                    body.push_str(&addition);
                    draft.try_set(body);
                } else {
                    ui.drafts.with(|m| {
                        if let Some(value) = m.get_mut(&owner) {
                            if comment {
                                value.comment.push_str(&addition);
                            } else {
                                value.body.push_str(&addition);
                            }
                        }
                    });
                }
                ui.status.set("Pasted attachments saved".into());
                refresh(ui);
            }
            Ok(Err(e)) => ui.status.set(format!("Attachment save failed: {e:#}")),
            Err(e) => ui.status.set(e.to_string()),
        }
    });
}

pub fn open_resource(ui: Ui, card_id: String, path: String) {
    let Some(app) = crate::APP.get().cloned() else {
        ui.status
            .set("Gallery preview: no AppServer connected".into());
        return;
    };
    spawn(async move {
        match blocking(move || {
            let value = app.call(
                "resource.materialize",
                json!({"card_id":card_id,"path":path}),
            )?;
            let path = value["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Attachment path is missing"))?;
            // NSH intentionally refuses file: URLs. Opening a validated local
            // resource belongs to the native desktop host, off the render thread.
            let result = std::process::Command::new("/usr/bin/open")
                .arg(path)
                .status()?;
            anyhow::ensure!(result.success(), "macOS could not open this attachment");
            Ok::<_, anyhow::Error>(())
        })
        .await
        {
            Ok(Ok(())) => ui.status.set("Attachment opened".into()),
            Ok(Err(e)) => ui.status.set(format!("Unable to open attachment: {e:#}")),
            Err(e) => ui.status.set(e.to_string()),
        }
    });
}

pub fn copy_text(ui: Ui, value: String) {
    spawn(async move {
        let result = blocking(move || -> anyhow::Result<()> {
            use std::io::Write;
            let mut command = std::process::Command::new("/usr/bin/pbcopy")
                .stdin(std::process::Stdio::piped())
                .spawn()?;
            command.stdin.take().unwrap().write_all(value.as_bytes())?;
            anyhow::ensure!(command.wait()?.success(), "Clipboard copy failed");
            Ok(())
        })
        .await;
        ui.status.set(match result {
            Ok(Ok(())) => "Copied to clipboard".into(),
            Ok(Err(e)) => e.to_string(),
            Err(e) => e.to_string(),
        });
    });
}
