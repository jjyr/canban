use crate::drafts::Draft;
use canban_types::*;
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub page: String,
    pub project: String,
    pub card: String,
    pub request: String,
    pub note: String,
    pub return_overview: bool,
}

#[derive(Clone, Copy)]
pub struct Ui {
    /// Commands outlive transient buttons/lists, but end with the window.
    pub owner: Owner,
    pub terminal_selection: Signal<String>,
    pub terminal_tabs: Signal<Vec<String>>,
    pub terminal_details: Signal<bool>,
    pub terminal_focus: Signal<u64>,
    pub terminal_rename: Signal<Option<String>>,
    pub operation_error: Signal<String>,
    pub terminal_origin: Signal<Option<Location>>,
    pub terminal_card_return: Signal<bool>,
    pub terminal_sidebar: Signal<bool>,
    pub terminal_closed: Signal<bool>,
    pub terminal_folds: Signal<BTreeSet<String>>,
    pub terminal_width: Signal<f32>,
    pub quick_open: Signal<bool>,
    pub project_picker: Signal<bool>,
    pub search_query: Signal<String>,
    pub recent_destinations: Stored<Vec<String>>,
    pub snapshot: Signal<Snapshot>,
    pub profiles: Signal<Vec<Profile>>,
    pub machines: Signal<Value>,
    pub machines_loading: Signal<bool>,
    pub max_agents: Signal<usize>,
    pub idle_minutes: Signal<u64>,
    pub overview_selection: Signal<String>,
    pub overview_selected: Signal<Option<canban_notifications::OverviewRow>>,
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
    pub snapshot_epoch: Signal<u64>,
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
    pub fn spawn<T: 'static>(self, future: impl std::future::Future<Output = T> + 'static) {
        self.owner.run(|| {
            spawn(future);
        });
    }
    pub fn clear_output(self) {
        self.output_request
            .set(self.output_request.get_untracked() + 1);
        self.output.set(String::new());
        self.output_observed_at.set(None);
    }
    pub fn navigate(self, page: &str) {
        self.terminal_card_return.set(false);
        if page == "terminals" {
            self.terminal_focus.update(|v| *v += 1);
            self.terminal_origin.set(None);
            crate::terminals::restore_selection(self);
            if self.narrow.get_untracked() && !self.terminal_selection.get_untracked().is_empty() {
                self.terminal_sidebar.set(false);
            }
        }
        self.return_overview.set(false);
        self.clear_output();
        self.review_request
            .set(self.review_request.get_untracked() + 1);
        self.page.set(page.into());
    }
    pub fn new() -> Self {
        Self {
            owner: Owner::current(),
            machines: signal(json!({"machines":[]})),
            machines_loading: signal(false),
            terminal_selection: signal(String::new()),
            terminal_tabs: signal(Vec::new()),
            terminal_details: signal(false),
            terminal_focus: signal(0),
            terminal_rename: signal(None),
            operation_error: signal(String::new()),
            terminal_origin: signal(None),
            terminal_card_return: signal(false),
            terminal_sidebar: signal(true),
            terminal_closed: signal(false),
            terminal_folds: signal(BTreeSet::new()),
            terminal_width: signal(260.),
            snapshot: signal(Snapshot::default()),
            quick_open: signal(false),
            project_picker: signal(false),
            search_query: signal(String::new()),
            recent_destinations: stored(Vec::new()),
            idle_minutes: signal(
                crate::APP
                    .get()
                    .map(|a| a.config().scheduler.idle_minutes)
                    .unwrap_or(15),
            ),
            overview_selection: signal(String::new()),
            overview_selected: signal(None),
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
            status: signal(String::new()),
            pending: signal(false),
            refreshing: signal(false),
            snapshot_epoch: signal(0),
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
    pub fn location(self) -> Location {
        Location {
            page: self.page.get_untracked(),
            project: self.project.get_untracked(),
            card: self.card.get_untracked(),
            request: self.request.get_untracked(),
            note: self.note.get_untracked(),
            return_overview: self.return_overview.get_untracked(),
        }
    }
    pub fn open_terminal(self, conversation: String) {
        if self.page.get_untracked() != "terminals" && !self.terminal_card_return.get_untracked() {
            self.terminal_origin.set(Some(self.location()));
        }
        self.terminal_card_return.set(false);
        self.clear_output();
        self.review_request
            .set(self.review_request.get_untracked() + 1);
        self.terminal_details.set(false);
        self.terminal_focus.update(|v| *v += 1);
        self.terminal_tabs.update(|tabs| {
            if !tabs.contains(&conversation) {
                tabs.push(conversation.clone());
            }
        });
        self.terminal_selection.set(conversation.clone());
        crate::terminals::reveal(self, &conversation);
        self.page.set("terminals".into());
    }
    pub fn terminal_source(self) {
        if let Some(source) = self.terminal_origin.get_untracked() {
            self.clear_output();
            self.review_request
                .set(self.review_request.get_untracked() + 1);
            self.project.set(source.project);
            self.card.set(source.card);
            self.request.set(source.request);
            self.note.set(source.note);
            self.return_overview.set(source.return_overview);
            self.page.set(source.page);
            self.terminal_origin.set(None);
        }
    }
    pub fn terminal_card(self, card: String) {
        self.return_overview.set(false);
        self.open_card(card);
        self.terminal_card_return.set(true);
    }
    pub fn return_terminal(self) {
        self.terminal_card_return.set(false);
        self.clear_output();
        self.review_request
            .set(self.review_request.get_untracked() + 1);
        self.terminal_focus.update(|v| *v += 1);
        self.page.set("terminals".into());
    }
    pub fn show_search(self, projects: bool) {
        self.search_query.set(String::new());
        self.project_picker.set(projects);
        self.quick_open.set(true);
    }
    pub fn select_project(self, id: String) {
        self.project.set(id);
        self.card.set(String::new());
        self.note.set(String::new());
        self.navigate("board");
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
        self.terminal_card_return.set(false);
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
        if self.terminal_card_return.get_untracked() {
            self.return_terminal();
            return;
        }
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
        self.terminal_card_return.set(false);
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
    let epoch = ui.snapshot_epoch.get_untracked();
    ui.spawn(async move {
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
        if ui.snapshot_epoch.get_untracked() != epoch {
            ui.refreshing.set(false);
            return;
        }
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
            }
            Ok(Err(e)) => ui.operation_error.set(format!("Could not refresh: {e:#}")),
            Err(e) => ui.operation_error.set(format!("Refresh worker: {e}")),
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
    ui.operation_error.set(String::new());
    ui.status.set(format!("{method}…"));
    if params.get("request_key").is_none() {
        params["request_key"] = json!(id());
    }
    let source = ui.location();
    ui.spawn(async move {
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
        ui.pending.set(false);
        match result {
            Ok(Ok((value, snapshot, profiles, max_agents, idle_minutes))) => {
                ui.snapshot_epoch.update(|v| *v += 1);
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
                let active = ui.location() == source;
                match method {
                    "project.create" if active => {
                        if let Some(id) = value["id"].as_str() {
                            ui.project.set(id.into());
                            ui.card.set(String::new());
                            ui.page.set("board".into());
                        }
                    }
                    "card.create" if active => {
                        if let Some(id) = value["id"].as_str() {
                            ui.open_card(id.into());
                            ui.source_note.set(String::new());
                        }
                    }
                    "card.archive" if active => ui.page.set("archive".into()),
                    "herdr.status" => ui.status.set(format!(
                        "Herdr connected · core {}",
                        value["core"]["id"].as_str().unwrap_or("")
                    )),
                    _ => {}
                }
                if matches!(value["state"].as_str(), Some("failed" | "unknown")) {
                    ui.operation_error.set(ui.status.get_untracked());
                }
                after(Ok(value));
            }
            Ok(Err(e)) => {
                let error = format!("{e:#}");
                ui.status.set(error.clone());
                ui.operation_error.set(error.clone());
                after(Err(error));
            }
            Err(e) => {
                let error = e.to_string();
                ui.status.set(error.clone());
                ui.operation_error.set(error.clone());
                after(Err(error));
            }
        }
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
    ui.spawn(async move {
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
    ui.spawn(async move {
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
            Ok(Err(e)) => ui.operation_error.set(format!("Attachment save failed: {e:#}")),
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
    ui.spawn(async move {
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
            Ok(Err(e)) => ui
                .operation_error
                .set(format!("Unable to open attachment: {e:#}")),
            Err(e) => ui.status.set(e.to_string()),
        }
    });
}

pub fn copy_text(ui: Ui, value: String) {
    ui.spawn(async move {
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
