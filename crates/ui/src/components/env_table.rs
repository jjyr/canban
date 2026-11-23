//! Shared multi-layer Secrets & Variables two-section editor.
//!
//! Write-only secret handling: UI only receives `EnvEntryMeta` (`hint` + `configured`),
//! never decrypts or persists secret values, and clears local draft signals immediately on save.
use crate::presenter::*;
use canban_types::{
    EnvEntryMeta, EnvWriteEntry, is_likely_secret_key, is_valid_env_key, parse_dotenv_lines,
};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::{collections::BTreeMap, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Plain,
    Secret,
}

#[derive(Clone, PartialEq)]
struct EnvRowDraft {
    uid: u64,
    key: Signal<String>,
    kind: Signal<RowKind>,
    plain_value: Signal<String>,
    secret_value: Signal<String>,
    secret_revealed: Signal<bool>,
    has_existing_secret: Signal<bool>,
    existing_configured: Signal<bool>,
    existing_hint: Signal<String>,
    replacing_secret: Signal<bool>,
}

impl EnvRowDraft {
    fn from_meta(owner: Owner, uid: u64, key: &str, meta: &EnvEntryMeta) -> Option<Self> {
        owner.run(|| match meta {
            EnvEntryMeta::Plain { value } => Some(Self {
                uid,
                key: signal(key.to_owned()),
                kind: signal(RowKind::Plain),
                plain_value: signal(value.clone()),
                secret_value: signal(String::new()),
                secret_revealed: signal(false),
                has_existing_secret: signal(false),
                existing_configured: signal(false),
                existing_hint: signal(String::new()),
                replacing_secret: signal(false),
            }),
            EnvEntryMeta::Secret { configured, hint } => Some(Self {
                uid,
                key: signal(key.to_owned()),
                kind: signal(RowKind::Secret),
                plain_value: signal(String::new()),
                secret_value: signal(String::new()),
                secret_revealed: signal(false),
                has_existing_secret: signal(true),
                existing_configured: signal(*configured),
                existing_hint: signal(hint.clone()),
                replacing_secret: signal(false),
            }),
            EnvEntryMeta::Unset => None,
        })
    }

    fn empty_plain(owner: Owner, uid: u64) -> Self {
        owner.run(|| Self {
            uid,
            key: signal(String::new()),
            kind: signal(RowKind::Plain),
            plain_value: signal(String::new()),
            secret_value: signal(String::new()),
            secret_revealed: signal(false),
            has_existing_secret: signal(false),
            existing_configured: signal(false),
            existing_hint: signal(String::new()),
            replacing_secret: signal(false),
        })
    }

    fn empty_secret(owner: Owner, uid: u64) -> Self {
        owner.run(|| Self {
            uid,
            key: signal(String::new()),
            kind: signal(RowKind::Secret),
            plain_value: signal(String::new()),
            secret_value: signal(String::new()),
            secret_revealed: signal(false),
            has_existing_secret: signal(false),
            existing_configured: signal(false),
            existing_hint: signal(String::new()),
            replacing_secret: signal(true),
        })
    }
}

pub type SaveCallback =
    Rc<dyn Fn(BTreeMap<String, EnvWriteEntry>, Box<dyn FnOnce(Result<(), String>) + 'static>)>;

/// Shared `"env.save"` RPC dispatcher for Global, Agent/@Identity, and Project scopes.
pub fn env_save_callback(
    ui: Ui,
    host_id: Signal<String>,
    scope: &'static str,
    target_id: impl Fn() -> Option<String> + 'static,
) -> SaveCallback {
    Rc::new(move |entries, done| {
        let mut payload = json!({
            "host_id": host_id.get_untracked(),
            "scope": scope,
            "entries": entries,
        });
        if let Some(tid) = target_id() {
            payload["target_id"] = Value::String(tid);
        }
        let cb = std::cell::RefCell::new(Some(done));
        dispatch_complete(ui, "env.save", payload, move |res| {
            if let Some(done) = cb.borrow_mut().take() {
                done(res.map(|_| ()));
            }
        });
    })
}

#[derive(Clone)]
pub struct EnvTableConfig {
    pub host_id: Signal<String>,
    pub project_id: Option<Signal<String>>,
    pub scope_key: Memo<String>,
    pub source_meta: Memo<BTreeMap<String, EnvEntryMeta>>,
    pub save_label: &'static str,
    pub on_save: SaveCallback,
    pub dirty_out: Option<Signal<bool>>,
}

fn build_drafts_from_meta(
    owner: Owner,
    next_uid: Stored<u64>,
    meta: &BTreeMap<String, EnvEntryMeta>,
) -> Vec<EnvRowDraft> {
    meta.iter()
        .filter_map(|(k, v)| {
            let uid = next_uid.get() + 1;
            next_uid.replace(uid);
            EnvRowDraft::from_meta(owner, uid, k, v)
        })
        .collect()
}

pub fn build_write_entries_from_rows(
    rows: &[EnvRowDraftSnapshot],
) -> Result<BTreeMap<String, EnvWriteEntry>, String> {
    let mut out = BTreeMap::new();
    for row in rows {
        let key = row.key.trim();
        if key.is_empty() && row.plain_value.is_empty() && row.secret_value.is_empty() {
            continue;
        }
        if !is_valid_env_key(key) {
            return Err(format!("Invalid environment variable name '{key}'"));
        }
        if out.contains_key(key) {
            return Err(format!("Duplicate environment variable '{key}'"));
        }
        let entry = match row.kind {
            RowKind::Plain => {
                if row.plain_value.contains('\0') {
                    return Err(format!("Variable '{key}' cannot contain NUL bytes"));
                }
                EnvWriteEntry::Plain {
                    value: row.plain_value.clone(),
                }
            }
            RowKind::Secret => {
                if (row.has_existing_secret && !row.replacing_secret)
                    || (row.secret_value.is_empty() && row.has_existing_secret)
                {
                    EnvWriteEntry::Secret {
                        value: None,
                        keep_existing: true,
                    }
                } else {
                    if row.secret_value.is_empty() {
                        return Err(format!("Secret '{key}' requires a non-empty value"));
                    }
                    if row.secret_value.contains('\0') {
                        return Err(format!("Secret '{key}' cannot contain NUL bytes"));
                    }
                    EnvWriteEntry::Secret {
                        value: Some(row.secret_value.clone()),
                        keep_existing: false,
                    }
                }
            }
        };
        out.insert(key.to_owned(), entry);
    }
    Ok(out)
}

#[derive(Clone, Debug)]
pub struct EnvRowDraftSnapshot {
    pub key: String,
    pub kind: RowKind,
    pub plain_value: String,
    pub secret_value: String,
    pub has_existing_secret: bool,
    pub replacing_secret: bool,
}

fn snapshot_rows(rows: &[EnvRowDraft]) -> Vec<EnvRowDraftSnapshot> {
    rows.iter()
        .map(|r| EnvRowDraftSnapshot {
            key: r.key.get_untracked(),
            kind: r.kind.get_untracked(),
            plain_value: r.plain_value.get_untracked(),
            secret_value: r.secret_value.get_untracked(),
            has_existing_secret: r.has_existing_secret.get_untracked(),
            replacing_secret: r.replacing_secret.get_untracked(),
        })
        .collect()
}

pub fn env_table(ui: Ui, cfg: EnvTableConfig) -> impl IntoElement {
    // Anchor all row draft signals on the table owner so rows created inside ephemeral `when(...)`
    // branches (`Paste .env`, `Discard`) or async `popup_menu` callbacks (`From Shell`) survive.
    let table_owner = Owner::current();
    let next_uid = stored(0_u64);
    let initial_scope = cfg.scope_key.get_untracked();
    let initial_meta = cfg.source_meta.get_untracked();
    let last_scope = stored(initial_scope);
    let last_synced_meta = stored(initial_meta.clone());
    let rows: Signal<Vec<EnvRowDraft>> =
        signal(build_drafts_from_meta(table_owner, next_uid, &initial_meta));
    let dirty = cfg.dirty_out.unwrap_or_else(|| signal(false));
    dirty.try_set(false);
    let save_epoch = signal(0_u64);
    let last_save_epoch = stored(0_u64);
    let error = signal(String::new());
    let note = signal(String::new());
    let saving = signal(false);
    let paste_open = signal(false);
    let paste_text = signal(String::new());
    let shell_loading = signal(false);

    let scope_key = cfg.scope_key;
    let source_meta = cfg.source_meta;
    effect(move || {
        let cur_scope = scope_key.get();
        let cur_meta = source_meta.get();
        let is_dirty = dirty.get();
        let cur_save = save_epoch.get();
        let scope_changed = cur_scope != last_scope.get();
        let meta_changed = cur_meta != last_synced_meta.get();
        let save_changed = cur_save != last_save_epoch.get();
        if scope_changed || (!is_dirty && (meta_changed || save_changed)) {
            last_scope.replace(cur_scope);
            last_synced_meta.replace(cur_meta.clone());
            last_save_epoch.replace(cur_save);
            rows.set(build_drafts_from_meta(table_owner, next_uid, &cur_meta));
            dirty.try_set(false);
            error.try_set(String::new());
            if scope_changed {
                paste_open.try_set(false);
                paste_text.try_set(String::new());
                note.try_set(String::new());
            }
        }
    });

    let add_secret = move |_: &()| {
        error.set(String::new());
        note.set(String::new());
        let uid = next_uid.get() + 1;
        next_uid.replace(uid);
        rows.update(|list| list.push(EnvRowDraft::empty_secret(table_owner, uid)));
        dirty.set(true);
    };

    let add_variable = move |_: &()| {
        error.set(String::new());
        note.set(String::new());
        let uid = next_uid.get() + 1;
        next_uid.replace(uid);
        rows.update(|list| list.push(EnvRowDraft::empty_plain(table_owner, uid)));
        dirty.set(true);
    };

    let host_sig = cfg.host_id;
    let proj_sig = cfg.project_id;
    let import_from_shell = move |_: &()| {
        if shell_loading.get_untracked() {
            return;
        }
        error.set(String::new());
        note.set(String::new());
        let apply_selection = move |key: String, is_secret: bool, preview: String| {
            rows.update(|list| {
                if let Some(existing) = list.iter().find(|r| r.key.get_untracked().trim() == key) {
                    if is_secret {
                        existing.kind.set(RowKind::Secret);
                        existing.replacing_secret.set(true);
                        existing.secret_value.set(String::new());
                    } else {
                        existing.kind.set(RowKind::Plain);
                        existing.plain_value.set(preview.clone());
                    }
                } else {
                    let uid = next_uid.get() + 1;
                    next_uid.replace(uid);
                    let meta = if is_secret {
                        EnvEntryMeta::Secret {
                            configured: false,
                            hint: String::new(),
                        }
                    } else {
                        EnvEntryMeta::Plain {
                            value: preview.clone(),
                        }
                    };
                    if let Some(row) = EnvRowDraft::from_meta(table_owner, uid, &key, &meta) {
                        if is_secret {
                            row.has_existing_secret.set(false);
                            row.replacing_secret.set(true);
                        }
                        list.push(row);
                    }
                }
            });
            dirty.set(true);
            if is_secret {
                note.set(format!(
                    "Added {key} to Secrets. Enter or paste its value so it can be encrypted into the host store."
                ));
            }
        };

        if !crate::backend::is_connected() {
            use nsh_core::element::desktop::{MenuItem, popup_menu};
            let sample = [
                ("RUST_LOG", false, "info"),
                ("CARGO_TARGET_DIR", false, "/tmp/target"),
                ("OPENAI_API_KEY", true, "••••9a2f"),
            ];
            ui.spawn(async move {
                let items: Vec<MenuItem> = sample
                    .into_iter()
                    .map(|(k, is_sec, prev)| {
                        let key = k.to_owned();
                        let pv = prev.to_owned();
                        let label_txt = if is_sec {
                            format!("{k} · Secret ({prev})")
                        } else {
                            format!("{k} = {prev}")
                        };
                        MenuItem::new(label_txt).on_select(move || {
                            apply_selection(key.clone(), is_sec, pv.clone());
                        })
                    })
                    .collect();
                popup_menu(items).await;
            });
            return;
        }

        let hid = host_sig.get_untracked();
        let pid = proj_sig.map(|s| s.get_untracked());
        shell_loading.set(true);
        ui.spawn(async move {
            let res = blocking(move || {
                crate::backend::call_routed(
                    "host.env.keys",
                    json!({ "host_id": hid }),
                    pid.as_deref(),
                )
            })
            .await;
            shell_loading.try_set(false);
            match res {
                Ok(Ok(val)) => {
                    let keys = val["keys"].as_array().cloned().unwrap_or_default();
                    if keys.is_empty() {
                        note.try_set("No importable host shell variables were found.".into());
                        return;
                    }
                    use nsh_core::element::desktop::{MenuItem, popup_menu};
                    let items: Vec<MenuItem> = keys
                        .into_iter()
                        .filter_map(|item| {
                            let k = item["key"].as_str()?.to_owned();
                            let is_sec = item["is_secret"].as_bool().unwrap_or(false);
                            let prev = item["preview"].as_str().unwrap_or("").to_owned();
                            let title = if is_sec {
                                format!("{k} · Secret ({prev})")
                            } else if prev.is_empty() {
                                k.clone()
                            } else {
                                let short: String = prev.chars().take(48).collect();
                                format!("{k} = {short}")
                            };
                            Some(MenuItem::new(title).on_select(move || {
                                apply_selection(k.clone(), is_sec, prev.clone());
                            }))
                        })
                        .collect();
                    popup_menu(items).await;
                }
                Ok(Err(e)) => {
                    error.try_set(format!("Could not read host shell keys: {e:#}"));
                }
                Err(e) => {
                    error.try_set(format!("Could not read host shell keys: {e}"));
                }
            }
        });
    };

    let save_label = cfg.save_label;
    let on_save = cfg.on_save.clone();

    view()
        .gap_2p5()
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(compact_action(
                    move || {
                        if paste_open.get() {
                            "Close .env paste"
                        } else {
                            "Paste .env"
                        }
                    },
                    Variant::Secondary,
                    move |_| {
                        error.set(String::new());
                        paste_open.update(|v| *v = !*v);
                    },
                ))
                .child(
                    compact_action(
                        move || {
                            if shell_loading.get() {
                                "Loading shell…"
                            } else {
                                "From Shell ▾"
                            }
                        },
                        Variant::Quiet,
                        import_from_shell,
                    )
                    .disabled(move || shell_loading.get()),
                ),
        )
        .child(when(
            move || paste_open.get(),
            move || {
                let editor = canban_ui_kit::markdown::MarkdownInput::new();
                panel()
                    .gap_2()
                    .child(
                        muted("Paste KEY=VALUE lines (keys matching SECRET / TOKEN / KEY / PASSWORD import into Secrets).")
                            .text_size(px(12.)),
                    )
                    .child(
                        view()
                            .min_h(px(108.))
                            .bg(INSET)
                            .rounded(px(RADIUS))
                            .p_2()
                            .child(
                                editor
                                    .source(paste_text, "Paste .env lines", "API_URL=https://api.example.com\nOPENAI_API_KEY=sk-...")
                                    .size_full(),
                            ),
                    )
                    .child(
                        view()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(compact_action("Cancel", Variant::Quiet, move |_| {
                                paste_open.set(false);
                            }))
                            .child(compact_action("Import lines", Variant::Primary, move |_| {
                                match parse_dotenv_lines(&paste_text.get_untracked()) {
                                    Ok(parsed) => {
                                        if parsed.is_empty() {
                                            error.set("No KEY=VALUE lines found to import.".into());
                                            return;
                                        }
                                        let mut sec_count = 0_usize;
                                        let mut var_count = 0_usize;
                                        rows.update(|list| {
                                            for (k, v) in parsed {
                                                let is_sec = is_likely_secret_key(&k);
                                                if is_sec {
                                                    sec_count += 1;
                                                } else {
                                                    var_count += 1;
                                                }
                                                if let Some(existing) =
                                                    list.iter().find(|r| r.key.get_untracked().trim() == k)
                                                {
                                                    if is_sec {
                                                        existing.kind.set(RowKind::Secret);
                                                        existing.replacing_secret.set(true);
                                                        existing.secret_value.set(v);
                                                    } else {
                                                        existing.kind.set(RowKind::Plain);
                                                        existing.plain_value.set(v);
                                                    }
                                                } else {
                                                    let uid = next_uid.get() + 1;
                                                    next_uid.replace(uid);
                                                    if let Some(row) = EnvRowDraft::from_meta(
                                                        table_owner,
                                                        uid,
                                                        &k,
                                                        &if is_sec {
                                                            EnvEntryMeta::Secret {
                                                                configured: false,
                                                                hint: String::new(),
                                                            }
                                                        } else {
                                                            EnvEntryMeta::Plain { value: v.clone() }
                                                        },
                                                    ) {
                                                        if is_sec {
                                                            row.has_existing_secret.set(false);
                                                            row.replacing_secret.set(true);
                                                            row.secret_value.set(v);
                                                        }
                                                        list.push(row);
                                                    }
                                                }
                                            }
                                        });
                                        error.set(String::new());
                                        note.set(format!(
                                            "Imported {sec_count} {} and {var_count} {}.",
                                            if sec_count == 1 { "secret" } else { "secrets" },
                                            if var_count == 1 { "variable" } else { "variables" },
                                        ));
                                        paste_text.set(String::new());
                                        paste_open.set(false);
                                        dirty.set(true);
                                    }
                                    Err(e) => error.set(e),
                                }
                            })),
                    )
            },
        ))
        .child(when(
            move || !note.get().is_empty(),
            move || notice(move || note.get(), Tone::Info),
        ))
        .child(when(
            move || !error.get().is_empty(),
            move || notice(move || error.get(), Tone::Danger),
        ))
        .child(env_section_panel(
            "Secrets",
            "Add secret",
            "No secrets configured yet.",
            RowKind::Secret,
            rows,
            dirty,
            add_secret,
        ))
        .child(env_section_panel(
            "Variables",
            "Add variable",
            "No variables configured yet.",
            RowKind::Plain,
            rows,
            dirty,
            add_variable,
        ))
        .child(when(
            move || dirty.get(),
            move || {
                let save_cb = on_save.clone();
                view()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .pt_1()
                    .child(
                        compact_action("Discard", Variant::Quiet, move |_| {
                            let cur = source_meta.get_untracked();
                            last_synced_meta.replace(cur.clone());
                            rows.set(build_drafts_from_meta(table_owner, next_uid, &cur));
                            dirty.set(false);
                            error.set(String::new());
                            note.set(String::new());
                        })
                        .disabled(move || saving.get()),
                    )
                    .child(
                        compact_action(
                            move || if saving.get() { "Saving…" } else { save_label },
                            Variant::Primary,
                            move |_| {
                                if saving.get_untracked() {
                                    return;
                                }
                                let snap = rows.with_untracked(|list| snapshot_rows(list));
                                match build_write_entries_from_rows(&snap) {
                                    Ok(entries) => {
                                        error.set(String::new());
                                        note.set(String::new());
                                        saving.set(true);
                                        save_cb(
                                            entries,
                                            Box::new(move |res| {
                                                saving.try_set(false);
                                                match res {
                                                    Ok(()) => {
                                                        // Scrub typed secrets immediately and force a clean reload from synced meta.
                                                        rows.with_untracked(|list| {
                                                            for r in list {
                                                                r.secret_value.try_set(String::new());
                                                                r.secret_revealed.try_set(false);
                                                                r.replacing_secret.try_set(false);
                                                            }
                                                        });
                                                        dirty.try_set(false);
                                                        if let Some(n) = save_epoch.try_get() {
                                                            save_epoch.try_set(n + 1);
                                                        }
                                                    }
                                                    Err(msg) => {
                                                        error.try_set(msg);
                                                    }
                                                }
                                            }),
                                        );
                                    }
                                    Err(msg) => error.set(msg),
                                }
                            },
                        )
                        .disabled(move || saving.get()),
                    )
            },
        ))
}

fn env_section_panel(
    title: &'static str,
    add_label: &'static str,
    empty_text: &'static str,
    section_kind: RowKind,
    rows: Signal<Vec<EnvRowDraft>>,
    dirty: Signal<bool>,
    on_add: impl Fn(&()) + 'static,
) -> impl IntoElement {
    panel()
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .child(label(title).font_weight(FontWeight::SEMIBOLD))
                        .child(dynamic(move || {
                            let count = rows
                                .get()
                                .into_iter()
                                .filter(|r| r.kind.get() == section_kind)
                                .count();
                            if count == 0 {
                                view().into_element()
                            } else {
                                badge(count.to_string()).into_element()
                            }
                        })),
                )
                .child(compact_action_with_icon(
                    Icon::Plus,
                    add_label,
                    Variant::Secondary,
                    on_add,
                )),
        )
        .child(dynamic(move || {
            let matching: Vec<EnvRowDraft> = rows
                .get()
                .into_iter()
                .filter(|r| r.kind.get() == section_kind)
                .collect();
            if matching.is_empty() {
                return view()
                    .py_1()
                    .child(muted(empty_text).text_size(px(12.)))
                    .into_element();
            }
            let mut list = view().gap_2();
            for row in matching {
                list = list.child(env_row_view(row, section_kind, rows, dirty));
            }
            list.into_element()
        }))
}

fn env_row_view(
    row: EnvRowDraft,
    section_kind: RowKind,
    rows: Signal<Vec<EnvRowDraft>>,
    dirty: Signal<bool>,
) -> impl IntoElement {
    let uid = row.uid;
    let key_sig = row.key;
    let kind_sig = row.kind;
    let plain_sig = row.plain_value;
    let sec_sig = row.secret_value;
    let rev_sig = row.secret_revealed;
    let has_existing_sig = row.has_existing_secret;
    let configured_sig = row.existing_configured;
    let hint_sig = row.existing_hint;
    let replacing_sig = row.replacing_secret;

    let (key_placeholder, key_aria, remove_aria) = match section_kind {
        RowKind::Secret => ("SECRET_KEY", "Secret name", "Remove secret"),
        RowKind::Plain => ("KEY_NAME", "Variable name", "Remove variable"),
    };

    let value_area = match section_kind {
        RowKind::Plain => view()
            .flex_1()
            .min_w(px(180.))
            .child(
                bound_field(
                    move || plain_sig.get(),
                    move |v| {
                        plain_sig.set(v.to_owned());
                        dirty.set(true);
                    },
                    "Value",
                )
                .font_family(MONO_FONT)
                .label("Plain value"),
            )
            .into_element(),
        RowKind::Secret => view()
            .flex_1()
            .min_w(px(180.))
            .child(dynamic(move || {
                if has_existing_sig.get() && !replacing_sig.get() {
                    let configured = configured_sig.get();
                    let hint = hint_sig.get();
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(if configured {
                            badge(if hint.is_empty() {
                                "••••".into()
                            } else {
                                hint
                            })
                            .into_element()
                        } else {
                            notice("Missing on this host", Tone::Attention).into_element()
                        })
                        .child(compact_action(
                            if configured { "Replace" } else { "Set value" },
                            Variant::Secondary,
                            move |_| {
                                sec_sig.set(String::new());
                                replacing_sig.set(true);
                                dirty.set(true);
                            },
                        ))
                        .into_element()
                } else {
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .child(
                            view().flex_1().min_w(px(120.)).child(
                                secret_field(sec_sig, rev_sig, "Enter secret value…")
                                    .on_change(move |v: &str| {
                                        sec_sig.set(v.to_owned());
                                        dirty.set(true);
                                    })
                                    .font_family(MONO_FONT)
                                    .label("Secret value"),
                            ),
                        )
                        .child(compact_action_with_icon(
                            Icon::Eye,
                            move || if rev_sig.get() { "Hide" } else { "Show" },
                            Variant::Quiet,
                            move |_| rev_sig.update(|r| *r = !*r),
                        ))
                        .children(has_existing_sig.get().then(|| {
                            compact_action("Keep", Variant::Quiet, move |_| {
                                sec_sig.set(String::new());
                                rev_sig.set(false);
                                replacing_sig.set(false);
                            })
                        }))
                        .into_element()
                }
            }))
            .into_element(),
    };

    view()
        .p_2()
        .rounded(px(RADIUS))
        .bg(INSET)
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(
            view().w(px(210.)).min_w(px(140.)).child(
                bound_field(
                    move || key_sig.get(),
                    move |v| {
                        key_sig.set(v.to_owned());
                        dirty.set(true);
                    },
                    key_placeholder,
                )
                .font_family(MONO_FONT)
                .label(key_aria),
            ),
        )
        .child(value_area)
        .children((section_kind == RowKind::Plain).then(|| {
            when(
                move || is_likely_secret_key(&key_sig.get()),
                move || {
                    compact_action("→ Move to Secrets", Variant::Secondary, move |_| {
                        let cur = plain_sig.get_untracked();
                        plain_sig.set(String::new());
                        sec_sig.set(cur);
                        has_existing_sig.set(false);
                        replacing_sig.set(true);
                        kind_sig.set(RowKind::Secret);
                        dirty.set(true);
                    })
                },
            )
        }))
        .child(chrome_button(Icon::Close, remove_aria, move |_| {
            rows.update(|list| list.retain(|r| r.uid != uid));
            dirty.set(true);
        }))
}
