use super::workspace::BoardFilter;
use super::*;
pub fn matches(ui: Ui, c: &Card, snapshot: &Snapshot) -> bool {
    let filter = ui
        .github
        .filters
        .with_untracked(|m| m.get(&c.project_id).cloned().unwrap_or_default());
    if !filter.tags.is_empty() {
        let local_tags = ui.local_tags.get_untracked();
        let known = crate::tags::known_map(snapshot, &local_tags, Some(&c.project_id));
        let card_tags = crate::tags::card_tags(c, &known);
        if !filter.tags.iter().all(|wanted| {
            let w = wanted.to_lowercase();
            card_tags.iter().any(|t| t.to_lowercase() == w)
        }) {
            return false;
        }
    }
    let source = c.github_issue();
    if filter.source == "local" && source.is_some() || filter.source == "github" && source.is_none()
    {
        return false;
    }
    if !filter.kind.is_empty()
        && filter.kind != "all"
        && (source.is_none() || filter.kind != c.source_kind())
    {
        return false;
    }
    let cached = snapshot.github_issues.iter().find(|v| {
        v.project_id == c.project_id
            && source.is_some_and(|s| {
                s.issue_id == v.source.issue_id && s.repository_id == v.source.repository_id
            })
    });
    if !filter.author.is_empty() {
        let author = cached
            .map(|v| v.author_login.as_str())
            .unwrap_or(&c.created_by);
        if !author.eq_ignore_ascii_case(&filter.author) {
            return false;
        }
    }
    filter.state.is_empty()
        || filter.state == "all"
        || cached.is_some_and(|v| v.state == filter.state)
}
pub fn toolbar(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let id = ui.project.get();
        let initial = ui
            .github
            .filters
            .with_untracked(|m| m.get(&id).cloned().unwrap_or_default());
        let source = signal(if initial.source.is_empty() {
            "all".into()
        } else {
            initial.source.clone()
        });
        let kind = signal(if initial.kind.is_empty() {
            "all".into()
        } else {
            initial.kind.clone()
        });
        let state = signal(if initial.state.is_empty() {
            "all".into()
        } else {
            initial.state.clone()
        });
        let author = signal(initial.author);
        let save = id.clone();
        let clear_id = id.clone();
        let tag_btn_id = id.clone();
        let chips_id = id.clone();
        effect(move || {
            let s = source.get();
            let k = kind.get();
            let st = state.get();
            let a = author.get();
            ui.github.filters.update(|m| {
                let existing_tags = m.get(&save).map(|f| f.tags.clone()).unwrap_or_default();
                m.insert(
                    save.clone(),
                    BoardFilter {
                        source: s,
                        kind: k,
                        state: st,
                        author: a,
                        tags: existing_tags,
                    },
                );
            });
        });
        let mut source_seg = segmented();
        for (key, title) in [("all", "All"), ("local", "Local"), ("github", "GitHub")] {
            source_seg = source_seg.child(compact_segment(
                title,
                None,
                move || source.get() == key,
                move |_| source.set(key.into()),
            ));
        }
        let row = view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(source_seg);
        row.child(choice(
            ui,
            kind,
            vec![
                ("all", "All types"),
                ("issue", "Issues"),
                ("pr", "Pull requests"),
            ],
        ))
        .child(action(
            move || {
                if author.get().is_empty() {
                    "Author ▾".to_owned()
                } else {
                    format!("Author: {} ▾", author.get())
                }
            },
            Variant::Secondary,
            move |_| {
                let mut authors: std::collections::BTreeSet<_> = ui.snapshot.with_untracked(|s| {
                    s.github_issues
                        .iter()
                        .filter(|v| v.project_id == id)
                        .map(|v| v.author_login.clone())
                        .chain(
                            s.cards
                                .iter()
                                .filter(|c| c.project_id == id)
                                .map(|c| c.created_by.clone()),
                        )
                        .filter(|a| !a.is_empty())
                        .collect()
                });
                authors.insert(String::new());
                ui.spawn(async move {
                    nsh_core::element::desktop::popup_menu(
                        authors
                            .into_iter()
                            .map(|name| {
                                nsh_core::element::desktop::MenuItem::new(if name.is_empty() {
                                    "All authors".into()
                                } else {
                                    name.clone()
                                })
                                .on_select(move || author.set(name.clone()))
                            })
                            .collect(),
                    )
                    .await;
                });
            },
        ))
        .child(choice(
            ui,
            state,
            vec![
                ("all", "All GitHub states"),
                ("open", "Open"),
                ("closed", "Closed"),
                ("draft", "Draft"),
                ("merged", "Merged"),
            ],
        ))
        .child(tag_picker_button(ui, tag_btn_id))
        .child(active_tag_chips(ui, chips_id))
        .child(action("Clear", Variant::Quiet, move |_| {
            let cid = clear_id.clone();
            batch(move || {
                source.set("all".into());
                kind.set("all".into());
                state.set("all".into());
                author.set(String::new());
                crate::tags::clear_board_tags(ui, &cid);
            })
        }))
        .into_element()
    })
}

fn tag_picker_button(ui: Ui, project_id: String) -> impl IntoElement {
    let label_pid = project_id.clone();
    let visible_pid = project_id.clone();
    when(
        move || {
            let s = ui.snapshot.get();
            let lt = ui.local_tags.get();
            let _ = ui.github.filters.get();
            !crate::tags::ordered_known(&s, &lt, Some(&visible_pid)).is_empty()
                || !crate::tags::active_board_tags(ui, &visible_pid).is_empty()
        },
        move || {
            let label_pid = label_pid.clone();
            let menu_pid = project_id.clone();
            compact_action(
                move || {
                    let _ = ui.github.filters.get();
                    let active = crate::tags::active_board_tags(ui, &label_pid);
                    if active.is_empty() {
                        "Tag ▾".to_owned()
                    } else {
                        format!("Tag ({}) ▾", active.len())
                    }
                },
                Variant::Secondary,
                move |_| {
                    let pid = menu_pid.clone();
                    let tags = crate::tags::ordered_known(
                        &ui.snapshot.get_untracked(),
                        &ui.local_tags.get_untracked(),
                        Some(&pid),
                    );
                    let active = crate::tags::active_board_tags(ui, &pid);
                    let mut items = Vec::new();
                    let clear_pid = pid.clone();
                    items.push(
                        nsh_core::element::desktop::MenuItem::new("All tags")
                            .on_select(move || crate::tags::clear_board_tags(ui, &clear_pid)),
                    );
                    if !tags.is_empty() {
                        items.push(nsh_core::element::desktop::MenuItem::separator());
                    }
                    for tag in tags {
                        let is_active = active
                            .iter()
                            .any(|t| t.to_lowercase() == tag.to_lowercase());
                        let item_label = if is_active {
                            format!("✓ #{tag}")
                        } else {
                            format!("#{tag}")
                        };
                        let item_pid = pid.clone();
                        items.push(
                            nsh_core::element::desktop::MenuItem::new(item_label).on_select(
                                move || crate::tags::toggle_board_filter(ui, &item_pid, &tag),
                            ),
                        );
                    }
                    ui.spawn(async move {
                        nsh_core::element::desktop::popup_menu(items).await;
                    });
                },
            )
        },
    )
}

fn active_tag_chips(ui: Ui, project_id: String) -> impl IntoElement {
    dynamic(move || {
        let _ = ui.github.filters.get();
        let active = crate::tags::active_board_tags(ui, &project_id);
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1p5()
            .children(active.into_iter().map(|tag| {
                let pid = project_id.clone();
                let remove_tag = tag.clone();
                compact_action(format!("#{tag} ×"), Variant::Secondary, move |_| {
                    crate::tags::toggle_board_filter(ui, &pid, &remove_tag)
                })
                .into_element()
            }))
    })
}

/// Compact Board filter bar for local Projects whenever one or more `#tag` filters are active.
pub fn tag_bar(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let id = ui.project.get();
        let _ = ui.github.filters.get();
        let active = crate::tags::active_board_tags(ui, &id);
        if active.is_empty() {
            return view().into_element();
        }
        let clear_id = id.clone();
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(muted("Filtered by"))
            .child(active_tag_chips(ui, id.clone()))
            .child(tag_picker_button(ui, id))
            .child(compact_action("Clear", Variant::Quiet, move |_| {
                crate::tags::clear_board_tags(ui, &clear_id)
            }))
            .into_element()
    })
}
