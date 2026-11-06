use canban_types::*;
use nsh_core::prelude::*;
use serde_json::{Value, json};
#[derive(Clone, Copy)]
pub struct Ui {
    pub snapshot: Signal<Snapshot>,
    pub profiles: Signal<Vec<Profile>>,
    pub project: Signal<String>,
    pub card: Signal<String>,
    pub request: Signal<String>,
    pub page: Signal<String>,
    pub status: Signal<String>,
    pub pending: Signal<bool>,
    pub refreshing: Signal<bool>,
    pub output: Signal<String>,
    pub output_request: Signal<u64>,
}
impl Ui {
    pub fn new() -> Self {
        Self {
            snapshot: signal(Snapshot::default()),
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
            output_request: signal(0),
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
        self.card.set(id);
        self.output.set(String::new());
        self.output_request
            .set(self.output_request.get_untracked() + 1);
        self.page.set("card".into());
    }
}
pub fn refresh(ui: Ui) {
    if ui.refreshing.get_untracked() {
        return;
    }
    ui.refreshing.set(true);
    let app = crate::APP.get().unwrap().clone();
    spawn(async move {
        let result = blocking(move || {
            app.snapshot()
                .map(|snapshot| (snapshot, app.config().profiles))
        })
        .await;
        match result {
            Ok(Ok((snapshot, profiles))) => {
                ui.profiles.set(profiles);
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
    mut params: Value,
    after: impl FnOnce(Value) + 'static,
) {
    if ui.pending.get_untracked() {
        ui.status
            .set("Please wait for the current operation".into());
        return;
    }
    ui.pending.set(true);
    ui.status.set(format!("{method}…"));
    if params.get("request_key").is_none() {
        params["request_key"] = json!(id());
    }
    let app = crate::APP.get().unwrap().clone();
    spawn(async move {
        let result = blocking(move || {
            let value = app.call(method, params)?;
            Ok::<_, anyhow::Error>((value, app.snapshot()?, app.config().profiles))
        })
        .await;
        match result {
            Ok(Ok((value, snapshot, profiles))) => {
                ui.profiles.set(profiles);
                ui.snapshot.set(snapshot);
                ui.status.set(format!("{method} saved"));
                match method {
                    "project.create" => {
                        if let Some(id) = value["id"].as_str() {
                            ui.project.set(id.into());
                            ui.page.set("board".into());
                        }
                    }
                    "card.create" => {
                        if let Some(id) = value["id"].as_str() {
                            ui.open_card(id.into());
                        }
                    }
                    "card.archive" => ui.page.set("archive".into()),
                    "herdr.status" => ui.status.set(format!(
                        "Herdr {} connected · core {} · {} live terminals",
                        value["version"].as_str().unwrap_or(""),
                        value["core"]["id"].as_str().unwrap_or(""),
                        value["live"]
                    )),
                    _ => {}
                }
                after(value);
            }
            Ok(Err(e)) => ui.status.set(format!("Error: {e:#}")),
            Err(e) => ui.status.set(format!("Worker error: {e}")),
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
    let app = crate::APP.get().unwrap().clone();
    spawn(async move {
        let result = blocking(move || app.call(method, params)).await;
        if ui.output_request.get_untracked() != generation {
            return;
        }
        ui.output.set(match result {
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
) {
    if event.files.is_empty() {
        return;
    }
    let files = event.files.clone();
    let app = crate::APP.get().unwrap().clone();
    spawn(async move {
        let result=blocking(move||{let mut refs=Vec::new();for file in files{let value=app.call("attachment.add",json!({"card_id":card_id,"name":file.name,"media_type":file.mime,"bytes":file.data,"request_key":id()}))?;refs.push(format!("[{}](attachment:{})",value["name"].as_str().unwrap_or("Attachment"),value["id"].as_str().unwrap_or("")));}Ok::<_,anyhow::Error>(refs)}).await;
        match result {
            Ok(Ok(refs)) => {
                draft.update(|body| {
                    body.push_str("\n\n");
                    body.push_str(&refs.join("\n"));
                });
                ui.status.set("Pasted attachments saved".into());
                refresh(ui);
            }
            Ok(Err(e)) => ui.status.set(format!("Attachment save failed: {e:#}")),
            Err(e) => ui.status.set(e.to_string()),
        }
    });
}

pub fn open_resource(ui: Ui, card_id: String, path: String) {
    let app = crate::APP.get().unwrap().clone();
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
