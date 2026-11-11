use super::*;
use canban_ui_kit::tokens::*;
pub fn load(ui: Ui, refresh: bool) {
    let b = ui.github;
    let Some(r) = b.reference.get_untracked() else {
        return;
    };
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    let section = b.section.get_untracked();
    let page = b.detail_page.get_untracked();
    let previous = b.overview.get_untracked()["head"]["sha"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    let ticket = b.detail_ticket.get_untracked() + 1;
    b.detail_ticket.set(ticket);
    b.detail_loading.set(true);
    b.detail_error.set(String::new());
    ui.spawn(async move{
        let result=blocking(move||->anyhow::Result<(Value,Value)>{
            // Resolve the displayed version from shared cache before loading any section.
            let overview=app.call("github.detail",json!({"reference":r,"section":"overview","refresh":refresh}))?;
            let head=overview["data"]["head"]["sha"].as_str().unwrap_or("");
            let page=if section=="files"&&!previous.is_empty()&&head!=previous{1}else{page};
            let detail=if section=="overview"{overview.clone()}else{app.call("github.detail",json!({"reference":r,"section":section,"page":page,"refresh":refresh,"head":head}))?};
            Ok((overview["data"].clone(),detail))
        }).await;
        if b.detail_ticket.try_get()!=Some(ticket){return}
        b.detail_loading.set(false);match result{
            Ok(Ok((overview,detail)))=>{b.overview.set(overview);if let Some(page)=detail["page"].as_u64()&&page!=b.detail_page.get_untracked(){b.detail_page.set(page);}b.detail.set(detail);},
            Ok(Err(e))=>b.detail_error.set(format!("{e:#}")),Err(e)=>b.detail_error.set(e.to_string()),
        }
    });
}
fn select_section(ui: Ui, section: &str) {
    batch(move || {
        ui.github.section.set(section.into());
        ui.github.detail_page.set(1);
        ui.github.detail.set(json!({}));
    });
}
pub fn screen(ui: Ui) -> impl IntoElement {
    let b = ui.github;
    dynamic(move || {
        let Some(r) = b.reference.get() else {
            return view().into_element();
        };
        let open = r.clone();
        let create = r.clone();
        let r_title = r.clone();
        let d = drafts::document(ui, format!("github-review:{}", r.url), Draft::default());
        effect(move || {
            let head = b
                .overview
                .with(|v| v["head"]["sha"].as_str().unwrap_or("").to_owned());
            if !head.is_empty()
                && d.with_untracked(|d| {
                    d.github_head.is_empty()
                        || (d.github_comments.is_empty()
                            && d.body.trim().is_empty()
                            && d.instructions.trim().is_empty())
                })
            {
                d.update(|d| {
                    if d.github_head != head {
                        d.github_file.clear();
                        d.github_line.clear();
                    }
                    d.github_head = head;
                });
            }
        });
        let mut area = view()
            .flex_1()
            .min_h(px(0.))
            .pl_2()
            .gap_2()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(label(format!(
                        "{} · {}{}",
                        r.repository,
                        r.kind,
                        r.number.map(|n| format!(" #{n}")).unwrap_or_default()
                    )))
                    .child(view().flex_1())
                    .child(chrome_button(
                        Icon::Close,
                        "Close GitHub detail",
                        move |_| b.reference.set(None),
                    )),
            )
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(compact_action(
                        "Open on GitHub",
                        Variant::Quiet,
                        move |_| open_url(ui, open.url.clone()),
                    ))
                    .child(chrome_button_disabled(
                        Icon::Refresh,
                        "Refresh details",
                        move || b.detail_loading.get(),
                        move |_| load(ui, true),
                    ))
                    .child(card_actions(ui, create)),
            )
            .child(dynamic(move || {
                let v = b.overview.get();
                let title = v["title"]
                    .as_str()
                    .or(v["commit"]["message"].as_str())
                    .unwrap_or("GitHub item")
                    .to_owned();
                let status = if v["merged"] == true {
                    "Merged"
                } else if v["draft"] == true {
                    "Draft"
                } else {
                    v["state"].as_str().unwrap_or("")
                };
                view()
                    .gap_1()
                    .child(label(title).font_weight(FontWeight::SEMIBOLD))
                    .child(
                        view()
                            .flex_row()
                            .gap_2()
                            .child(badge(status.to_owned()))
                            .child(muted(format!(
                                "{} → {}",
                                v["head"]["ref"].as_str().unwrap_or(""),
                                v["base"]["ref"].as_str().unwrap_or("")
                            ))),
                    )
                    .into_element()
            }));
        let mut tabs = view().flex_row().flex_wrap().gap_1();
        for (id, title) in [
            ("overview", "Overview"),
            ("discussion", "Discussion"),
            ("files", "Files changed"),
            ("reviews", "Reviews"),
            ("threads", "Review threads"),
            ("checks", "Checks"),
        ] {
            if r.kind == "commit" && id != "overview" {
                continue;
            }
            if r.kind != "pr" && !matches!(id, "overview" | "discussion") {
                continue;
            }
            tabs = tabs.child(
                action(title, Variant::Quiet, move |_| select_section(ui, id)).bg(move || {
                    if b.section.get() == id {
                        SELECTED.resolve()
                    } else {
                        CLEAR.resolve()
                    }
                }),
            );
        }
        area = area
            .child(tabs)
            .child(when(
                move || !b.detail_error.get().is_empty(),
                move || notice(move || b.detail_error.get(), Tone::Attention),
            ))
            .child(scroll().flex_1().min_h(px(0.)).child(dynamic(move || {
                let response = b.detail.get();
                let data = &response["data"];
                match b.section.get().as_str() {
                    "overview" => nsh_base::text_view()
                        .markdown(
                            data["body"]
                                .as_str()
                                .or(data["commit"]["message"].as_str())
                                .unwrap_or(if b.detail_loading.get() {
                                    "Loading…"
                                } else {
                                    "No description."
                                })
                                .to_owned(),
                        )
                        .into_element(),
                    "files" => files(ui, d, data),
                    "discussion" | "reviews" | "threads" => discussion(ui, d, data),
                    "checks" => view()
                        .gap_2()
                        .children(
                            data["check_runs"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|c| {
                                    panel()
                                        .child(label(
                                            c["name"].as_str().unwrap_or("Check").to_owned(),
                                        ))
                                        .child(badge(
                                            c["conclusion"]
                                                .as_str()
                                                .or(c["status"].as_str())
                                                .unwrap_or("unknown")
                                                .to_owned(),
                                        ))
                                        .into_element()
                                })
                                .collect::<Vec<_>>(),
                        )
                        .into_element(),
                    _ => view().into_element(),
                }
            })))
            .child(when(
                move || b.section.get() != "overview",
                move || {
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            action("Previous", Variant::Quiet, move |_| {
                                b.detail_page.update(|p| *p = p.saturating_sub(1).max(1));
                            })
                            .disabled(move || b.detail_page.get() <= 1 || b.detail_loading.get()),
                        )
                        .child(muted(move || format!("Page {}", b.detail_page.get())))
                        .child(
                            action("Next", Variant::Quiet, move |_| {
                                b.detail_page.update(|p| *p += 1)
                            })
                            .disabled(move || {
                                b.detail.with(|v| v["has_next"] != true) || b.detail_loading.get()
                            }),
                        )
                },
            ));
        if r.kind != "commit" {
            area = area.child(composer(ui, d, r_title));
        }
        area.into_element()
    })
}
fn card_actions(ui: Ui, r: GitHubReference) -> impl IntoElement {
    dynamic(move || {
        let snapshot = ui.snapshot.get();
        let target = r.clone();
        let matches: Vec<_> = snapshot
            .cards
            .iter()
            .filter(|c| {
                c.github_issue().is_some_and(|s| {
                    s.repository.eq_ignore_ascii_case(&r.repository)
                        && Some(s.number) == r.number
                        && c.source_kind() == r.kind
                }) || c.pull_request.as_ref().is_some_and(|p| {
                    r.kind == "pr"
                        && p.repository.eq_ignore_ascii_case(&r.repository)
                        && Some(p.number) == r.number
                })
            })
            .cloned()
            .collect();
        let mut area = view().flex_row().flex_wrap().gap_1();
        for c in &matches {
            let id = c.id.clone();
            let name = snapshot
                .projects
                .iter()
                .find(|p| p.id == c.project_id)
                .map(|p| p.name.as_str())
                .unwrap_or("Project");
            area = area.child(action(
                format!(
                    "Open {} Card #{}, {}",
                    name,
                    c.number,
                    if c.archived_at.is_some() {
                        "archived"
                    } else {
                        "active"
                    }
                ),
                Variant::Secondary,
                move |_| ui.open_card(id.clone()),
            ));
        }
        if matches.is_empty() && r.kind != "commit" {
            area = area.child(action("Create sync Card", Variant::Primary, move |_| {
                let projects: Vec<_> = ui.snapshot.with_untracked(|s| {
                    s.projects
                        .iter()
                        .filter(|p| {
                            p.github.as_ref().is_some_and(|g| {
                                g.repository.eq_ignore_ascii_case(&target.repository)
                            })
                        })
                        .cloned()
                        .collect()
                });
                let r = target.clone();
                if projects.len() == 1 {
                    dispatch(
                        ui,
                        "github.item.add",
                        json!({"reference":r,"project_id":projects[0].id}),
                    );
                } else if projects.is_empty() {
                    ui.operation_error
                        .set("Connect a Project to this repository before adding a Card.".into());
                    ui.navigate("github-repository");
                } else {
                    ui.spawn(async move {
                        nsh_core::element::desktop::popup_menu(
                            projects
                                .into_iter()
                                .map(|p| {
                                    let r = r.clone();
                                    nsh_core::element::desktop::MenuItem::new(p.name).on_select(
                                        move || {
                                            dispatch(
                                                ui,
                                                "github.item.add",
                                                json!({"reference":r,"project_id":p.id}),
                                            )
                                        },
                                    )
                                })
                                .collect(),
                        )
                        .await;
                    });
                }
            }));
        }
        area.into_element()
    })
}
fn discussion(ui: Ui, d: Signal<Draft>, data: &Value) -> AnyElement {
    let section = ui.github.section.get_untracked();
    let mut area = view().gap_3();
    for entry in data.as_array().into_iter().flatten() {
        let mut row = panel()
            .child(label(format!(
                "{} · {}",
                entry["user"]["login"].as_str().unwrap_or("GitHub"),
                entry["state"].as_str().unwrap_or("Comment")
            )))
            .child(nsh_base::text_view().markdown(entry["body"].as_str().unwrap_or("").to_owned()));
        if section == "threads" {
            let id = entry["in_reply_to_id"]
                .as_u64()
                .or(entry["id"].as_u64())
                .unwrap_or(0);
            row = row
                .child(muted(format!(
                    "{}:{}{}",
                    entry["path"].as_str().unwrap_or(""),
                    entry["line"]
                        .as_u64()
                        .or(entry["original_line"].as_u64())
                        .unwrap_or(0),
                    if entry["line"].is_null() {
                        " · Outdated"
                    } else {
                        ""
                    }
                )))
                .child(action("Reply to thread", Variant::Quiet, move |_| {
                    d.update(|d| {
                        d.github_reply = id;
                        d.github_compose = "comment".into();
                    })
                }));
        }
        area = area.child(row);
    }
    area.into_element()
}
fn files(ui: Ui, d: Signal<Draft>, data: &Value) -> AnyElement {
    let files = data.as_array().cloned().unwrap_or_default();
    if files.is_empty() {
        return muted("No files loaded. Refresh if the PR has changed.").into_element();
    }
    let mut area = view().gap_3().child(muted(
        "GitHub PR diff · select a file to add a line comment to this Review.",
    ));
    for file in files {
        let path = file["filename"].as_str().unwrap_or("").to_owned();
        let selected = path.clone();
        let patch = file["patch"].as_str().map(str::to_owned);
        let code = patch
            .as_ref()
            .map(|patch| {
                let fence = "`".repeat(
                    patch
                        .split(|c| c != '`')
                        .map(str::len)
                        .max()
                        .unwrap_or(0)
                        .max(2)
                        + 1,
                );
                format!("{fence}diff\n{patch}\n{fence}")
            })
            .unwrap_or_else(|| {
                "Binary file or patch unavailable. Open on GitHub for the complete file.".into()
            });
        let mut panel = panel()
            .child(
                view()
                    .flex_row()
                    .gap_2()
                    .child(action(path, Variant::Quiet, move |_| {
                        d.update(|d| {
                            d.github_file = selected.clone();
                            d.github_compose = "review".into();
                        })
                    }))
                    .child(badge(
                        file["status"].as_str().unwrap_or("modified").to_owned(),
                    ))
                    .child(muted(format!(
                        "+{} −{}",
                        file["additions"], file["deletions"]
                    ))),
            )
            .child(nsh_base::text_view().markdown(code));
        if patch.is_some() {
            panel = panel.child(muted(
                "Select the file above, then enter a line number below to add a Review comment.",
            ));
        }
        area = area.child(panel);
    }
    let _ = ui;
    area.into_element()
}
fn submit(
    ui: Ui,
    d: Signal<Draft>,
    method: &'static str,
    mut params: Value,
    after: impl FnOnce(Value) + 'static,
) {
    if d.get_untracked().github_unknown {
        return;
    }
    let payload = serde_json::to_string(&json!([method, params])).unwrap();
    let mut saved = d.get_untracked();
    if saved.github_action_key.is_empty() || saved.github_action_payload != payload {
        saved.github_action_key = id();
        saved.github_action_payload = payload;
    }
    params["request_key"] = json!(saved.github_action_key);
    d.set(saved);
    dispatch_complete(ui, method, params, move |result| {
        if let Ok(value) = result {
            if value["state"] == "published" {
                d.update(|d| {
                    d.github_action_key.clear();
                    d.github_action_payload.clear();
                });
                after(value);
            } else if value["state"] == "unknown" {
                d.update(|d| d.github_unknown = true);
            } else if value["state"] == "failed" {
                d.update(|d| {
                    d.github_action_key.clear();
                    d.github_action_payload.clear();
                });
            }
        }
    });
}
fn composer(ui: Ui, d: Signal<Draft>, r: GitHubReference) -> impl IntoElement {
    let is_pr = r.kind == "pr";
    let comment = r.clone();
    let mut buttons = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(action(
            "Write a GitHub comment",
            Variant::Secondary,
            move |_| {
                d.update(|d| {
                    d.github_compose = if d.github_compose == "comment" {
                        String::new()
                    } else {
                        "comment".into()
                    }
                })
            },
        ));
    if is_pr {
        buttons = buttons.child(action(
            move || {
                format!(
                    "Review · {} pending comments",
                    d.get().github_comments.len()
                )
            },
            Variant::Secondary,
            move |_| {
                d.update(|d| {
                    d.github_compose = if d.github_compose == "review" {
                        String::new()
                    } else {
                        "review".into()
                    }
                })
            },
        ));
    }
    let check = r.url.clone();
    view()
        .gap_2()
        .child(buttons)
        .child(when(move||d.get().github_unknown,move||view().gap_1().child(notice("Publication outcome is unknown. Your draft is retained; check GitHub before submitting again.",Tone::Attention)).child(action("Check on GitHub",Variant::Quiet,{let check=check.clone();move |_|open_url(ui,check.clone())})).child(action("I checked GitHub · enable a new submission",Variant::Secondary,move |_|d.update(|d|{d.github_unknown=false;d.github_action_key.clear();d.github_action_payload.clear();})))))
        .child(when(
            move || d.get().github_compose == "comment",
            move || comment_form(ui, d, comment.clone()),
        ))
        .child(when(
            move || d.get().github_compose == "review",
            move || {
                scroll()
                    .max_h(px(300.))
                    .min_h(px(0.))
                    .child(review_form(ui, d, r.clone()))
            },
        ))
}
fn comment_form(ui: Ui, d: Signal<Draft>, r: GitHubReference) -> impl IntoElement {
    panel().gap_2().child(label("GitHub comment"))
        .child(bound_field(move||d.get().comment,move|v|d.update(|d|d.comment=v.into()),"Write a comment to publish on GitHub…").key(format!("github-comment:{}",r.url)))
        .child(view().flex_row().gap_2()
            .child(action(move||if d.get().github_reply>0{"Publish reply to thread"}else{"Publish to GitHub"},Variant::Primary,move |_|{
                let draft=d.get_untracked();let method=if draft.github_reply>0{"github.remote.reply"}else{"github.remote.comment"};
                submit(ui,d,method,json!({"reference":r,"body":draft.comment,"comment_id":draft.github_reply}),move|_|{d.update(|d|{d.comment.clear();d.github_reply=0;d.github_compose.clear();});load(ui,true);});
            }).disabled(move||ui.pending.get()||d.get().github_unknown||d.with(|d|d.comment.trim().is_empty())))
            .child(when(move||d.get().github_reply>0,move||action("Cancel thread reply",Variant::Quiet,move |_|d.update(|d|d.github_reply=0)))))
}
fn review_form(ui: Ui, d: Signal<Draft>, r: GitHubReference) -> impl IntoElement {
    let event = signal("COMMENT".to_owned());
    let side = signal("RIGHT".to_owned());
    panel().gap_2().child(label("Review · current PR version"))
        .child(when(move||d.with(|d|!d.github_head.is_empty()) && d.get().github_head!=ui.github.overview.with(|v|v["head"]["sha"].as_str().unwrap_or("").to_owned()),move||view().gap_1().child(notice("The PR has a new version. Your Review draft still belongs to the previous head.",Tone::Attention)).child(action("Discard line comments and use latest version",Variant::Secondary,move |_|{let head=ui.github.overview.get_untracked()["head"]["sha"].as_str().unwrap_or("").to_owned();d.update(|d|{d.github_comments.clear();d.github_file.clear();d.github_line.clear();d.instructions.clear();d.github_head=head;});}))))
        .child(view().flex_row().flex_wrap().gap_1().child(muted(move||format!("File: {}",d.get().github_file)))
            .child(bound_field(move||d.get().github_line,move|v|d.update(|d|d.github_line=v.into()),"Line number").key("github-review-line"))
            .child(choice(ui,side,vec![("RIGHT","New side"),("LEFT","Old side")])))
        .child(bound_field(move||d.get().instructions,move|v|d.update(|d|d.instructions=v.into()),"Line comment").key("github-review-line-body"))
        .child(action("Add to pending Review",Variant::Secondary,move |_|{let draft=d.get_untracked();if let Ok(line)=draft.github_line.parse::<u64>() && line>0 && !draft.github_file.is_empty() && !draft.instructions.trim().is_empty(){d.update(|d|{d.github_comments.push(json!({"path":draft.github_file,"line":line,"side":side.get_untracked(),"body":draft.instructions}));d.instructions.clear();});}}).disabled(move||d.with(|d|d.github_file.is_empty()||d.github_line.parse::<u64>().is_err()||d.instructions.trim().is_empty())))
        .child(muted(move||format!("{} pending line comments",d.get().github_comments.len())))
        .child(bound_field(move||d.get().body,move|v|d.update(|d|d.body=v.into()),"Review summary").key("github-review-summary"))
        .child(view().flex_row().gap_2().child(choice(ui,event,vec![("COMMENT","Comment"),("APPROVE","Approve"),("REQUEST_CHANGES","Request changes")]))
            .child(action("Submit Review to GitHub",Variant::Primary,move |_|{let draft=d.get_untracked();submit(ui,d,"github.remote.review",json!({"reference":r,"head":draft.github_head,"body":draft.body,"comments":draft.github_comments,"event":event.get_untracked()}),move|_|{d.update(|d|{d.github_comments.clear();d.body.clear();d.github_compose.clear();});load(ui,true);});}).disabled(move||ui.pending.get()||d.get().github_unknown||(event.get()=="COMMENT"&&d.with(|d|d.body.trim().is_empty()&&d.github_comments.is_empty()))||d.get().github_head.is_empty()||d.get().github_head!=ui.github.overview.with(|v|v["head"]["sha"].as_str().unwrap_or("").to_owned()))))
}
