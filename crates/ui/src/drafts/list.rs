use super::*;

/// All unfinished tasks and edits, independent of the currently selected Board.
pub fn screen(ui: Ui) -> impl IntoElement {
    use canban_ui_kit::*;
    let query = signal(String::new());
    view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_4()
        .child(subpage_header("Back to Board", "Drafts", move |_| {
            ui.navigate("board")
        }))
        .child(
            field(query, "Search drafts…")
                .label("Search drafts")
                .max_w(px(1000.)),
        )
        .child(scroll().flex_1().min_h(px(0.)).child(dynamic(move || {
            let _ = ui.draft_epoch.get();
            let query = query.get().to_lowercase();
            let mut drafts = ui.drafts.with(|all| {
                all.iter()
                    .filter(|(key, d)| {
                        d.has_content(key)
                            && format!("{} {} {}", d.title, d.body, d.instructions)
                                .to_lowercase()
                                .contains(&query)
                    })
                    .map(|(k, d)| (k.clone(), d.clone()))
                    .collect::<Vec<_>>()
            });
            drafts.sort_by_key(|(_, d)| std::cmp::Reverse(d.updated_at));
            if drafts.is_empty() {
                return view().child(muted(
                    "No drafts. Your unfinished Cards and edits appear here.",
                ));
            }
            view().gap_3().children(
                drafts
                    .into_iter()
                    .map(|(key, d)| {
                        let open_key = key.clone();
                        let delete_key = key.clone();
                        let project = ui.snapshot.with_untracked(|s| {
                            s.projects
                                .iter()
                                .find(|p| p.id == d.project_id)
                                .map(|p| p.name.clone())
                                .unwrap_or_else(|| "Project unavailable".into())
                        });
                        let new = key.starts_with("new-card:");
                        let title = if d.title.trim().is_empty() {
                            d.selected_issue
                                .as_ref()
                                .and_then(|v| v["issue"]["title"].as_str())
                                .unwrap_or("Untitled Card")
                                .to_owned()
                        } else {
                            d.title.clone()
                        };
                        let preview = if new && d.card_kind == "github" {
                            &d.instructions
                        } else if !d.comment.is_empty() && !d.dirty() {
                            &d.comment
                        } else {
                            &d.body
                        };
                        panel()
                            .gap_2()
                            .child(label(title))
                            .child(muted(format!(
                                "{} · {project}",
                                if new { "New Card" } else { "Unsubmitted edit" }
                            )))
                            .child(muted(preview.chars().take(180).collect::<String>()))
                            .child(
                                view()
                                    .flex_row()
                                    .gap_2()
                                    .child(action(
                                        "Continue editing",
                                        Variant::Primary,
                                        move |_| {
                                            if new {
                                                ui.new_draft.set(open_key.clone());
                                                ui.card_target.set(d.project_id.clone());
                                                if ui.snapshot.with_untracked(|s| {
                                                    s.projects.iter().any(|p| p.id == d.project_id)
                                                }) {
                                                    ui.project.set(d.project_id.clone());
                                                }
                                                ui.navigate("new-card");
                                            } else if let Some(id) = open_key.strip_prefix("card:")
                                            {
                                                let mut d = d.clone();
                                                d.editing = d.dirty();
                                                put(ui, open_key.clone(), d);
                                                ui.open_card(id.into());
                                            }
                                        },
                                    ))
                                    .child(action("Discard draft", Variant::Quiet, move |_| {
                                        remove(ui, &delete_key)
                                    })),
                            )
                            .into_element()
                    })
                    .collect::<Vec<_>>(),
            )
        })))
}
