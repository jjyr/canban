//! Client-local attachments: native picker/drop/paste -> bytes -> AppServer.
//! Upload state and references survive navigation with the Card draft.
use super::*;
use canban_types::id;
use canban_ui_kit::markdown::MarkdownInput;
use nsh_core::{PastedFile, file_dialog::OpenDialog};
use serde_json::json;
use std::path::PathBuf;

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum State {
    Uploading,
    Ready(String),
    Failed(String),
}
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Upload {
    key: String,
    name: String,
    #[serde(with = "pasted")]
    file: Option<std::sync::Arc<PastedFile>>,
    path: Option<PathBuf>,
    comment: bool,
    state: State,
}
pub fn busy(draft: Signal<Draft>, comment: bool) -> bool {
    draft.with(|d| {
        d.uploads
            .iter()
            .any(|a| a.comment == comment && matches!(a.state, State::Uploading | State::Failed(_)))
    })
}
impl Upload {
    pub fn is_comment(&self) -> bool {
        self.comment
    }
}
fn update(ui: Ui, draft: Signal<Draft>, card: &str, change: impl FnOnce(&mut Draft)) {
    let key = format!("card:{card}");
    let mut value = draft
        .try_get()
        .unwrap_or_else(|| ui.drafts.with(|m| m.get(&key).cloned().unwrap_or_default()));
    change(&mut value);
    crate::drafts::put(ui, key, value.clone());
    draft.try_set(value);
}
fn upload(ui: Ui, draft: Signal<Draft>, card: String, item: Upload) {
    ui.spawn(async move {
        let saved = item.clone();
        let target = card.clone();
        let app = crate::APP.get().cloned();
        let result = blocking(move || -> anyhow::Result<String> {
            let app = app.ok_or_else(|| anyhow::anyhow!("Gallery: no AppServer connected"))?;
            let file = if let Some(file) = saved.file {
                anyhow::ensure!(file.data.len() <= 32 * 1024 * 1024, "Attachment exceeds 32 MiB");
                file
            } else {
                let path = saved.path.ok_or_else(|| anyhow::anyhow!("Missing local file"))?;
                anyhow::ensure!(std::fs::metadata(&path)?.len() <= 32 * 1024 * 1024, "Attachment exceeds 32 MiB");
                let data = std::fs::read(&path)?;
                anyhow::ensure!(data.len() <= 32 * 1024 * 1024, "Attachment exceeds 32 MiB");
                let mime = nsh_core::platform::clipboard::Image::sniff(&data)
                    .map(|image| image.mime)
                    .unwrap_or_else(|| match path.extension().and_then(|s| s.to_str()) {
                        Some("md") => "text/markdown",
                        Some("txt") => "text/plain",
                        Some("pdf") => "application/pdf",
                        _ => "application/octet-stream",
                    });
                std::sync::Arc::new(PastedFile::new(saved.name.clone(), mime.to_owned(), data))
            };
            let value = app.call("attachment.add", json!({"card_id":target,"name":file.name,"media_type":file.mime,"bytes":file.data,"request_key":saved.key}))?;
            Ok(format!("[{}](attachment:{})", escape_name(value["name"].as_str().unwrap_or("Attachment")), value["id"].as_str().ok_or_else(|| anyhow::anyhow!("Missing attachment id"))?))
        }).await;
        update(ui, draft, &card, |d| {
            if let Some(a) = d.uploads.iter_mut().find(|a| a.key == item.key) {
                match result {
                    Ok(Ok(reference)) => {
                        a.state = State::Ready(reference.clone());
                        a.file = None;
                        let body = if a.comment { &mut d.comment } else { &mut d.body };
                        if !body.is_empty() { body.push_str("\n\n"); }
                        body.push_str(&reference);
                    }
                    Ok(Err(error)) => a.state = State::Failed(format!("{error:#}")),
                    Err(error) => a.state = State::Failed(error.to_string()),
                }
            }
        });
        refresh(ui);
    });
}
fn escape_name(name: &str) -> String {
    name.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace(['\n', '\r'], " ")
}
pub fn add(
    ui: Ui,
    draft: Signal<Draft>,
    card: String,
    files: Vec<PastedFile>,
    paths: Vec<PathBuf>,
    comment: bool,
) {
    let mut items = files
        .into_iter()
        .map(|file| Upload {
            key: id(),
            name: file.name.clone(),
            file: Some(std::sync::Arc::new(file)),
            path: None,
            comment,
            state: State::Uploading,
        })
        .collect::<Vec<_>>();
    items.extend(paths.into_iter().map(|path| {
        Upload {
            key: id(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Attachment".into()),
            file: None,
            path: Some(path),
            comment,
            state: State::Uploading,
        }
    }));
    update(ui, draft, &card, |d| d.uploads.extend(items.clone()));
    for item in items {
        upload(ui, draft, card.clone(), item);
    }
}
pub fn paths(uris: &[String]) -> Vec<PathBuf> {
    uris.iter()
        .filter_map(|uri| url::Url::parse(uri).ok()?.to_file_path().ok())
        .collect()
}
pub fn controls(
    ui: Ui,
    draft: Signal<Draft>,
    editor: MarkdownInput,
    comment: bool,
    card: String,
) -> impl IntoElement {
    let requested = signal(false);
    let picker_card = card.clone();
    effect(move || {
        if !requested.get() || editor.focused.get() {
            return;
        }
        requested.set(false);
        let card = picker_card.clone();
        ui.spawn(async move {
            ui.native_modal.set(true);
            // Let the native host apply the cleared editor keymap before
            // entering the system sheet's nested event loop.
            next_frame().await;
            let result = pick_paths(
                OpenDialog::files()
                    .multiple(true)
                    .title("Attach files")
                    .button_label("Attach"),
            )
            .await;
            ui.native_modal.set(false);
            if !editor.held.try_set(false) {
                return;
            }
            match result {
                Ok(Some(files)) => add(ui, draft, card, Vec::new(), files, comment),
                Ok(None) => {}
                Err(error) => ui.operation_error.set(error),
            }
            editor.focus();
        });
    });
    view()
        .flex_row()
        .items_center()
        .min_w(px(0.))
        .gap_1()
        .child(chrome_button_disabled(
            Icon::Plus,
            "Attach files",
            move || editor.held.get() || ui.current_card().is_none_or(|c| c.archived_at.is_some()),
            move |_| {
                // Wait for the real sink Blur event before opening the native
                // sheet; a frame/timer does not guarantee its keymap was released.
                editor.held.set(true);
                requested.set(true);
                if let Some(api) = editor.editor.api::<nsh_base::EditorApi>() {
                    api.blur();
                }
            },
        ))
        .child(
            scroll()
                .horizontal()
                .flex_1()
                .min_w(px(0.))
                .h(px(32.))
                .child(view().flex_row().gap_1().child(each(
                    move || {
                        draft
                            .get()
                            .uploads
                            .into_iter()
                            .filter(|a| a.comment == comment)
                            .collect::<Vec<_>>()
                    },
                    |a| a.key.clone(),
                    move |item, _| {
                        let remove_card = card.clone();
                        let retry_card = card.clone();
                        let key = item.get_untracked().key;
                        let remove_key = key.clone();
                        view()
                            .flex_row()
                            .items_center()
                            .flex_shrink_0()
                            .max_w(px(240.))
                            .px_2()
                            .rounded(px(6.))
                            .bg(INSET)
                            .tooltip(nsh_base::tooltip().child(label(
                                move || match item.get().state {
                                    State::Failed(e) => e,
                                    _ => item.get().name,
                                },
                            )))
                            .gap_1()
                            .child(
                                muted(move || {
                                    let a = item.get();
                                    match a.state {
                                        State::Uploading => format!("{} · uploading…", a.name),
                                        State::Ready(_) => a.name,
                                        State::Failed(_) => format!("{} · failed", a.name),
                                    }
                                })
                                .flex_1()
                                .min_w(px(0.))
                                .truncate(),
                            )
                            .child(when(
                                move || matches!(item.get().state, State::Failed(_)),
                                move || {
                                    let card = retry_card.clone();
                                    let key = key.clone();
                                    action("Retry", Variant::Quiet, move |_| {
                                        let mut next = None;
                                        update(ui, draft, &card, |d| {
                                            if let Some(a) =
                                                d.uploads.iter_mut().find(|a| a.key == key)
                                            {
                                                a.state = State::Uploading;
                                                next = Some(a.clone());
                                            }
                                        });
                                        if let Some(a) = next {
                                            upload(ui, draft, card.clone(), a);
                                        }
                                    })
                                },
                            ))
                            .child(
                                action("×", Variant::Quiet, move |_| {
                                    update(ui, draft, &remove_card, |d| {
                                        if let Some(a) =
                                            d.uploads.iter().find(|a| a.key == remove_key)
                                            && let State::Ready(reference) = &a.state
                                        {
                                            let body =
                                                if comment { &mut d.comment } else { &mut d.body };
                                            *body =
                                                body.replace(reference, "").trim_end().to_owned();
                                        }
                                        d.uploads.retain(|a| a.key != remove_key);
                                    });
                                    editor.focus();
                                })
                                .accessibility_label(|| Some("Remove attachment from draft".into()))
                                .disabled(move || matches!(item.get().state, State::Uploading)),
                            )
                    },
                ))),
        )
}

// Upgrade-session persistence retains pasted bytes as well as OS file paths.
mod pasted {
    use super::*;
    use serde::{Deserialize, Serialize};
    pub fn serialize<S: serde::Serializer>(
        file: &Option<std::sync::Arc<PastedFile>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        file.as_ref()
            .map(|f| (&f.name, &f.mime, f.data.to_vec()))
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<std::sync::Arc<PastedFile>>, D::Error> {
        Ok(
            Option::<(String, String, Vec<u8>)>::deserialize(deserializer)?
                .map(|(name, mime, data)| std::sync::Arc::new(PastedFile::new(name, mime, data))),
        )
    }
}
pub fn pause_after_upgrade(draft: &mut Draft) {
    for item in &mut draft.uploads {
        if matches!(item.state, State::Uploading) {
            item.state = State::Failed(
                "Upload interrupted by update. Retry to finish attaching the file.".into(),
            );
        }
    }
}
#[cfg(test)]
mod upgrade_tests {
    use super::*;
    #[test]
    fn pending_paste_bytes_survive_upgrade_and_keep_the_upload_identity() {
        let upload = Upload {
            key: "stable-upload-key".into(),
            name: "paste.txt".into(),
            file: Some(std::sync::Arc::new(PastedFile::new(
                "paste.txt",
                "text/plain",
                b"retain bytes".to_vec(),
            ))),
            path: None,
            comment: true,
            state: State::Uploading,
        };
        let bytes = serde_json::to_vec(&upload).unwrap();
        let restored: Upload = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.key, upload.key);
        assert_eq!(
            restored.file.as_ref().unwrap().data.as_slice(),
            b"retain bytes"
        );
        let mut draft = Draft {
            uploads: vec![restored],
            ..Default::default()
        };
        pause_after_upgrade(&mut draft);
        assert!(matches!(draft.uploads[0].state, State::Failed(_)));
    }
}
