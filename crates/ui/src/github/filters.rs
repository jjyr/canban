//! One controlled Board filter model for toolbar, content tags and commands.
use super::*;
use canban_types::BoardFilter;
use canban_ui_kit::{PickerOption, search_popover, tokens::*};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
// Keep pending writes alive after native owners are disposed; orderly Quit flushes them.
static PENDING: Mutex<BTreeMap<String, Value>> = Mutex::new(BTreeMap::new());
static WRITER: Mutex<()> = Mutex::new(());
pub fn flush() {
    let Some(app) = crate::APP.get() else {
        return;
    };
    let _writer = WRITER.lock().unwrap();
    let patches = PENDING.lock().unwrap().clone();
    for (key, patch) in patches {
        if app
            .call("utilities.configure", json!({"board_filter":patch}))
            .is_ok()
        {
            let mut pending = PENDING.lock().unwrap();
            if pending.get(&key) == Some(&patch) {
                pending.remove(&key);
            }
        }
    }
}

pub fn current(ui: Ui, pid: &str) -> BoardFilter {
    ui.github
        .filters
        .with(|m| m.get(pid).cloned().unwrap_or_default())
}

/// Restore only after preferences and account discovery, before accepting edits.
pub fn init(ui: Ui) {
    effect(move || {
        let ready = ui.snapshot_ready.get();
        let prefs = ui.utility.get();
        let gh = ui.github.gh_status.get();
        let snapshot = ui.snapshot.get();
        if !ready || prefs.get("platform").is_none() || gh.get("state").is_none() {
            return;
        }
        let login = active_login_from(&snapshot, &gh);
        // Transport failures are not account changes. Keep the selected scope and
        // Mine visible; do not silently replace it with an unfiltered local scope.
        if login.is_none()
            && gh["state"] == "unavailable"
            && ui.github.filter_account.get_untracked().is_some()
        {
            return;
        }
        let account = login
            .or_else(|| gh["last_login"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "Local".into())
            .to_lowercase();
        if ui.github.filter_account.get_untracked().as_ref() != Some(&account) {
            let filters = restored(&prefs, &account, &ui.github.filter_pending.get_untracked());
            batch(|| {
                ui.github.filter_account.set(Some(account));
                ui.github.filters.set(filters);
            });
        }
    });
}

fn restored(
    prefs: &Value,
    account: &str,
    pending: &BTreeMap<String, Value>,
) -> BTreeMap<String, BoardFilter> {
    let mut filters: BTreeMap<String, BoardFilter> =
        serde_json::from_value(prefs["preferences"]["board_filters"][account].clone())
            .unwrap_or_default();
    for patch in pending
        .values()
        .filter(|p| p["account"].as_str() == Some(account))
    {
        if let (Some(pid), Ok(filter)) = (
            patch["project_id"].as_str(),
            serde_json::from_value::<BoardFilter>(patch["value"].clone()),
        ) {
            filters.insert(pid.into(), filter);
        }
    }
    filters
}

pub fn edit(ui: Ui, pid: &str, change: impl FnOnce(&mut BoardFilter)) {
    if pid.is_empty() {
        return;
    }
    if crate::APP.get().is_some() && ui.github.filter_account.get_untracked().is_none() {
        return;
    }
    let mut filter = current(ui, pid);
    change(&mut filter);
    if filter == current(ui, pid) {
        return;
    }
    ui.github.filters.update(|m| {
        m.insert(pid.into(), filter.clone());
    });
    let Some(account) = ui.github.filter_account.get_untracked() else {
        return;
    };
    let key = serde_json::to_string(&(&account, pid)).unwrap();
    let patch = json!({"account":account,"project_id":pid,"value":filter});
    ui.utility.update(|v| {
        v["preferences"]["board_filters"][&account][pid] = json!(filter);
    });
    PENDING.lock().unwrap().insert(key.clone(), patch.clone());
    ui.github.filter_pending.update(|pending| {
        pending.insert(key, patch);
    });
    save_next(ui);
}

/// Serialize writes; coalesce pending changes to the same account/Project.
fn save_next(ui: Ui) {
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    if ui.github.filter_saving.get_untracked() {
        return;
    }
    let next = ui.github.filter_pending.with_untracked(|pending| {
        pending
            .first_key_value()
            .map(|(k, v)| (k.clone(), v.clone()))
    });
    let Some((key, patch)) = next else {
        return;
    };
    ui.github.filter_saving.set(true);
    ui.utility_epoch.update(|n| *n += 1);
    ui.spawn(async move {
        let sent = patch.clone();
        let write_key = key.clone();
        let result = blocking(move || {
            let _writer = WRITER.lock().unwrap();
            if PENDING.lock().unwrap().get(&write_key) != Some(&patch) {
                return Ok(json!({}));
            }
            let result = app.call("utilities.configure", json!({"board_filter":patch}));
            if result.is_ok() {
                let mut pending = PENDING.lock().unwrap();
                if pending.get(&write_key) == Some(&patch) {
                    pending.remove(&write_key);
                }
            }
            result
        })
        .await;
        ui.github.filter_saving.set(false);
        ui.utility_epoch.update(|n| *n += 1);
        if matches!(result, Ok(Ok(_))) {
            ui.github.filter_pending.update(|pending| {
                if pending.get(&key) == Some(&sent) {
                    pending.remove(&key);
                }
            });
            // Commit only the submitted field; preserve edits made during this write.
            let account = sent["account"].as_str().unwrap();
            let pid = sent["project_id"].as_str().unwrap();
            if !ui
                .github
                .filter_pending
                .with_untracked(|p| p.contains_key(&key))
            {
                ui.utility.update(|v| {
                    v["preferences"]["board_filters"][account][pid] = sent["value"].clone();
                });
            }
            save_next(ui);
        } else {
            ui.operation_error
                .set("Could not remember Board filters. Change a filter to retry saving.".into());
        }
    });
}

pub fn reset(ui: Ui) {
    let pid = ui.project.get_untracked();
    edit(ui, &pid, |f| *f = BoardFilter::default());
}
pub fn mine(ui: Ui) {
    let pid = ui.project.get_untracked();
    edit(ui, &pid, |f| {
        f.author_mode = "mine".into();
        f.author.clear();
    });
}

pub fn matches(ui: Ui, c: &Card) -> bool {
    let filter = current(ui, &c.project_id);
    if filter.author_mode == "mine"
        && active_login_untracked(ui).is_none()
        && ui
            .github
            .filter_account
            .get_untracked()
            .as_deref()
            .is_some_and(|a| a != "local")
    {
        return false;
    }
    let matches = ui.index.with_untracked(|idx| {
        let cached = if filter.author_mode.is_empty() && filter.state.is_empty() {
            None
        } else {
            source_snapshot(c, idx)
        };
        filter.matches_card(c, cached, active_login_untracked(ui).as_deref())
    });
    if !matches {
        return false;
    }
    if filter.tags.is_empty() {
        return true;
    }
    let card_tags = ui.known_tags.with_untracked(|kt| {
        let indexed_matches = ui
            .index
            .with_untracked(|idx| idx.cards.get(&c.id) == Some(c));
        if indexed_matches && c.removed_at.is_none() {
            kt.tags_for_card(&c.id)
        } else {
            crate::tags::card_tags(c, &kt.for_project(Some(&c.project_id)))
        }
    });
    filter
        .tags
        .iter()
        .all(|wanted| card_tags.iter().any(|t| t.eq_ignore_ascii_case(wanted)))
}
fn source_snapshot<'a>(
    c: &Card,
    index: &'a crate::index::SnapshotIndex,
) -> Option<&'a GitHubIssueSnapshot> {
    let r = c.github_issue()?;
    index.github_issue(&c.project_id, r.repository_id, r.issue_id)
}
fn trigger(title: String, active: bool) -> nsh_base::ButtonElement {
    compact_action(title, Variant::Secondary, |_| {})
        .border_color(if active { ACCENT } else { BORDER })
        .max_w(px(260.))
}
fn type_title(kind: &str) -> &'static str {
    match kind {
        "local" => "Local cards",
        "github" => "GitHub cards",
        "issue" => "GitHub Issues",
        "pr" => "GitHub PRs",
        _ => "All cards",
    }
}

pub fn toolbar(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let pid = ui.project.get();
        if pid.is_empty() {
            return view().into_element();
        }
        if crate::APP.get().is_some() && ui.github.filter_account.get().is_none() {
            return muted("Restoring Board filters…").into_element();
        }
        let filter = current(ui, &pid);
        let type_pid = pid.clone();
        let type_edit_pid = pid.clone();
        let author_pid = pid.clone();
        let author_edit_pid = pid.clone();
        let tag_pid = pid.clone();
        let tag_edit_pid = pid.clone();
        let more_pid = pid.clone();
        let more = usize::from(filter.scheduled) + usize::from(!filter.state.is_empty());
        let author_title = match filter.author_mode.as_str() {
            "mine" => "Mine".into(),
            "author" => format!("@{}", filter.author),
            _ => "Anyone".into(),
        };
        let row = view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                search_popover(
                    trigger(
                        format!("Type: {} ▾", type_title(&filter.kind)),
                        !filter.kind.is_empty(),
                    ),
                    move || {
                        let f = current(ui, &type_pid);
                        [
                            ("", "All cards"),
                            ("local", "Local cards"),
                            ("github", "GitHub cards"),
                            ("issue", "GitHub Issues"),
                            ("pr", "GitHub PRs"),
                        ]
                        .into_iter()
                        .map(|(id, title)| PickerOption::new(id, title).selected(f.kind == id))
                        .collect()
                    },
                    move |id| edit(ui, &type_edit_pid, |f| f.set_kind(&id)),
                )
                .placeholder("Search card types…")
                .width(260.),
            )
            .child(
                search_popover(
                    trigger(
                        format!("Author: {author_title} ▾"),
                        !filter.author_mode.is_empty(),
                    ),
                    move || {
                        let f = current(ui, &author_pid);
                        let mut authors: BTreeSet<_> = ui.snapshot.with(|s| {
                            s.github_issues
                                .iter()
                                .filter(|v| v.project_id == author_pid)
                                .map(|v| v.author_login.clone())
                                .chain(
                                    s.cards
                                        .iter()
                                        .filter(|c| {
                                            c.project_id == author_pid && c.github_issue().is_none()
                                        })
                                        .map(|c| c.created_by.clone()),
                                )
                                .filter(|a| !a.is_empty())
                                .collect()
                        });
                        if !f.author.is_empty() {
                            authors.insert(f.author.clone());
                        }
                        let mut options = vec![
                            PickerOption::new("", "Anyone").selected(f.author_mode.is_empty()),
                            PickerOption::new("mine", "Mine")
                                .selected(f.author_mode == "mine")
                                .keywords(["me", "my", "created by me"]),
                        ];
                        options.extend(authors.into_iter().map(|a| {
                            PickerOption::new(format!("author:{a}"), format!("@{a}"))
                                .group("Authors")
                                .selected(
                                    f.author_mode == "author" && f.author.eq_ignore_ascii_case(&a),
                                )
                        }));
                        options
                    },
                    move |id| {
                        edit(ui, &author_edit_pid, |f| {
                            f.author_mode = if id.starts_with("author:") {
                                "author".into()
                            } else {
                                id.clone()
                            };
                            f.author = id.strip_prefix("author:").unwrap_or_default().into();
                        })
                    },
                )
                .placeholder("Search authors…")
                .width(280.),
            )
            .child(
                search_popover(
                    trigger(
                        if filter.tags.is_empty() {
                            "Tags ▾".into()
                        } else {
                            format!("Tags ({}) ▾", filter.tags.len())
                        },
                        !filter.tags.is_empty(),
                    ),
                    move || {
                        let f = current(ui, &tag_pid);
                        let mut tags = crate::tags::ordered_known(
                            &ui.snapshot.get(),
                            &ui.local_tags.get(),
                            Some(&tag_pid),
                        );
                        for t in &f.tags {
                            if !tags.iter().any(|a| a.eq_ignore_ascii_case(t)) {
                                tags.push(t.clone());
                            }
                        }
                        let mut options =
                            vec![PickerOption::new("", "All tags").selected(f.tags.is_empty())];
                        options.extend(tags.into_iter().map(|t| {
                            PickerOption::new(format!("tag:{t}"), format!("#{t}"))
                                .selected(f.tags.iter().any(|a| a.eq_ignore_ascii_case(&t)))
                        }));
                        options
                    },
                    move |id| {
                        if let Some(tag) = id.strip_prefix("tag:") {
                            crate::tags::toggle_board_filter(ui, &tag_edit_pid, tag);
                        } else {
                            crate::tags::clear_board_tags(ui, &tag_edit_pid);
                        }
                    },
                )
                .placeholder("Search tags…")
                .width(260.),
            )
            .child(
                compact_action(
                    if more == 0 {
                        "More ▾".into()
                    } else {
                        format!("More ({more}) ▾")
                    },
                    Variant::Secondary,
                    move |_| open_more(ui, more_pid.clone()),
                )
                .border_color(if more > 0 { ACCENT } else { BORDER }),
            );
        let row = row.children(
            filter
                .active()
                .then(|| compact_action("Reset", Variant::Quiet, move |_| reset(ui))),
        );
        let mut chips = view().flex_row().flex_wrap().items_center().gap_1p5();
        for tag in filter.tags {
            let pid = pid.clone();
            chips = chips.child(compact_action(
                format!("#{tag} ×"),
                Variant::Secondary,
                move |_| crate::tags::toggle_board_filter(ui, &pid, &tag),
            ));
        }
        if !filter.state.is_empty() {
            let pid = pid.clone();
            chips = chips.child(compact_action(
                format!("GitHub state: {} ×", filter.state),
                Variant::Secondary,
                move |_| edit(ui, &pid, |f| f.set_state("")),
            ));
        }
        if filter.scheduled {
            let pid = pid.clone();
            chips = chips.child(compact_action(
                "Scheduled ×",
                Variant::Secondary,
                move |_| edit(ui, &pid, |f| f.scheduled = false),
            ));
        }
        view()
            .gap_2()
            .child(row)
            .child(chips)
            .child(notice(ui, pid))
            .into_element()
    })
}
pub fn open_more(ui: Ui, pid: String) {
    use nsh_core::element::desktop::{MenuItem, popup_menu};
    let f = current(ui, &pid);
    let mut items = vec![];
    for (id, title) in [
        ("", "All GitHub states"),
        ("open", "Open"),
        ("closed", "Closed"),
        ("draft", "Draft"),
        ("merged", "Merged"),
    ] {
        let pid = pid.clone();
        items.push(
            MenuItem::new(format!(
                "{}{}",
                if f.state == id { "✓ " } else { "" },
                title
            ))
            .on_select(move || edit(ui, &pid, |f| f.set_state(id))),
        );
    }
    items.push(MenuItem::separator());
    items.push(
        MenuItem::new(if f.scheduled {
            "✓ Scheduled only"
        } else {
            "Scheduled only"
        })
        .on_select(move || edit(ui, &pid, |f| f.scheduled = !f.scheduled)),
    );
    ui.spawn(async move {
        popup_menu(items).await;
    });
}
fn notice(ui: Ui, pid: String) -> impl IntoElement {
    let f = current(ui, &pid);
    if !f.active() {
        return view().into_element();
    }
    // Account checks can change independently of the project snapshot.
    let _ = ui.github.gh_status.get();
    let s = ui.snapshot.get();
    let mut messages = vec![];
    if f.author_mode == "mine" {
        if active_login_untracked(ui).is_none() {
            messages.push(if ui.github.filter_account.with_untracked(|a| a.as_deref().is_some_and(|a| a != "local")) {
                "GitHub identity is unavailable. Mine remains selected; sign in to confirm authorship."
            } else {
                "GitHub identity is unavailable. Mine matches only this server's Local creator until an account is available."
            });
        } else if s
            .cards
            .iter()
            .any(|c| c.project_id == pid && c.github_issue().is_none() && c.created_by == "Local")
        {
            messages.push("Older Local cards have no account identity and are excluded from Mine while signed in.");
        }
    }
    if (!f.author_mode.is_empty() || !f.state.is_empty())
        && s.cards.iter().any(|c| {
            c.project_id == pid
                && c.active()
                && c.github_issue().is_some()
                && ui.index.with(|idx| source_snapshot(c, idx).is_none())
        })
    {
        messages.push("Some GitHub source details are unavailable; authorship/state filters exclude those cards until details are available.");
    }
    let mut v = view().gap_2();
    for message in messages {
        v = v.child(muted(message));
    }
    if s.cards.iter().any(|c| c.project_id == pid && c.active())
        && !s
            .cards
            .iter()
            .any(|c| c.project_id == pid && c.active() && matches(ui, c))
    {
        v = v.child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(muted("No cards match the current filters."))
                .child(compact_action("Reset filters", Variant::Quiet, move |_| {
                    reset(ui)
                })),
        );
    }
    v.into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restoring_accounts_and_hosts_keeps_pending_edits_and_reset() {
        let prefs = json!({"preferences":{"board_filters":{
            "alice":{"p":{"author_mode":"mine"},"remote:p":{"kind":"pr"}},
            "bob":{"p":{"kind":"local"}}
        }}});
        assert_eq!(
            restored(&prefs, "alice", &BTreeMap::new())["p"].author_mode,
            "mine"
        );
        assert_eq!(restored(&prefs, "bob", &BTreeMap::new())["p"].kind, "local");
        assert!(restored(&prefs, "new-user", &BTreeMap::new()).is_empty());
        let pending = BTreeMap::from([(
            "key".into(),
            json!({"account":"alice","project_id":"p","value":BoardFilter::default()}),
        )]);
        let filters = restored(&prefs, "alice", &pending);
        assert!(!filters["p"].active());
        assert_eq!(filters["remote:p"].kind, "pr");
        assert_eq!(restored(&prefs, "bob", &pending)["p"].kind, "local");
    }
    #[test]
    fn local_projects_render_the_same_mine_and_tag_filter_controls() {
        use nsh_core::{ApproxTextEngine, Core, MountItem, geometry::size};
        #[component]
        fn FilterProbe() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            ui.project.set("local-project".into());
            mine(ui);
            crate::tags::toggle_board_filter(ui, "local-project", "bug");
            toolbar(ui)
        }
        let mut core = Core::new(ApproxTextEngine, size(px(700.), px(400.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<FilterProbe>());
        let batch = core.commit().unwrap();
        let texts: Vec<_> = batch
            .items
            .into_iter()
            .filter_map(|item| match item {
                MountItem::Text { text, .. } => Some(text.text.to_string()),
                _ => None,
            })
            .collect();
        for wanted in [
            "Type: All cards ▾",
            "Author: Mine ▾",
            "Tags (1) ▾",
            "#bug ×",
            "Reset",
        ] {
            assert!(
                texts.iter().any(|t| t == wanted),
                "Missing {wanted}: {texts:?}"
            );
        }
    }
}
