use super::workspace::BoardFilter;
use super::*;
use canban_ui_kit::tokens::*;
pub fn matches(ui: Ui, c: &Card, snapshot: &Snapshot) -> bool {
    let filter = ui
        .github
        .filters
        .with_untracked(|m| m.get(&c.project_id).cloned().unwrap_or_default());
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
        effect(move || {
            let filter = BoardFilter {
                source: source.get(),
                kind: kind.get(),
                state: state.get(),
                author: author.get(),
            };
            ui.github.filters.update(|m| {
                m.insert(save.clone(), filter);
            });
        });
        let mut row = view().flex_row().flex_wrap().items_center().gap_2();
        for (key, title) in [("all", "All"), ("local", "Local"), ("github", "GitHub")] {
            row = row.child(
                action(title, Variant::Quiet, move |_| source.set(key.into())).bg(move || {
                    if source.get() == key {
                        SELECTED.resolve()
                    } else {
                        CLEAR.resolve()
                    }
                }),
            );
        }
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
        .child(action("Clear", Variant::Quiet, move |_| {
            batch(move || {
                source.set("all".into());
                kind.set("all".into());
                state.set("all".into());
                author.set(String::new());
            })
        }))
        .into_element()
    })
}
