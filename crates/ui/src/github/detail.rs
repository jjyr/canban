use super::*;
use canban_ui_kit::tokens::*;

pub fn load(ui: Ui, refresh: bool) {
    let b = ui.github;
    let Some(r) = b.reference.get_untracked() else {
        return;
    };
    let Some(backend) = crate::backend::get() else {
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
    ui.spawn(async move {
        let result = blocking(move || -> anyhow::Result<(Value, Value)> {
            // Resolve the displayed version from shared cache before loading any section.
            let overview = backend.call_local(
                "github.detail",
                json!({"reference": r, "section": "overview", "refresh": refresh}),
            )?;
            let head = overview["data"]["head"]["sha"].as_str().unwrap_or("");
            let page = if section == "files" && !previous.is_empty() && head != previous {
                1
            } else {
                page
            };
            let detail = if section == "overview" {
                overview.clone()
            } else {
                backend.call_local(
                    "github.detail",
                    json!({"reference": r, "section": section, "page": page, "refresh": refresh, "head": head}),
                )?
            };
            Ok((overview["data"].clone(), detail))
        })
        .await;
        if b.detail_ticket.try_get() != Some(ticket) {
            return;
        }
        b.detail_loading.set(false);
        match result {
            Ok(Ok((overview, detail))) => {
                b.overview.set(overview);
                if let Some(page) = detail["page"].as_u64()
                    && page != b.detail_page.get_untracked()
                {
                    b.detail_page.set(page);
                }
                b.detail.set(detail);
            }
            Ok(Err(e)) => b.detail_error.set(format!("{e:#}")),
            Err(e) => b.detail_error.set(e.to_string()),
        }
    });
}

fn select_section(ui: Ui, section: &str) {
    batch(move || {
        ui.github.section.set(section.into());
        ui.github.detail_page.set(1);
        ui.github.editing_comment.set(None);
        ui.github.edit_draft.set(String::new());
        ui.github.edit_error.set(String::new());
        ui.github.deleting_comment.set(None);
        ui.github.delete_error.set(String::new());
        ui.github.detail.set(json!({}));
    });
}

fn detail_status(v: &Value, kind: &str) -> (String, Tone) {
    if v["merged"] == true || v["pull_request"]["merged_at"].is_string() {
        ("Merged".into(), Tone::Info)
    } else if v["draft"] == true {
        ("Draft".into(), Tone::Neutral)
    } else {
        match v["state"].as_str().unwrap_or("") {
            "open" => ("Open".into(), Tone::Success),
            "closed" => ("Closed".into(), Tone::Neutral),
            other if !other.is_empty() => (other.to_owned(), Tone::Neutral),
            _ if kind == "commit" => ("Commit".into(), Tone::Neutral),
            _ => ("GitHub".into(), Tone::Neutral),
        }
    }
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let b = ui.github;
    dynamic(move || {
        let Some(r) = b.reference.get() else {
            return view().into_element();
        };
        let open = r.clone();
        let create = r.clone();
        let r_review = r.clone();
        let r_header = r.clone();
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

        let kind_label = match r.kind.as_str() {
            "pr" => "Pull request",
            "commit" => "Commit",
            _ => "Issue",
        };
        let ident = r
            .number
            .map(|n| format!(" #{n}"))
            .or_else(|| {
                r.sha
                    .as_deref()
                    .map(|s| format!(" {}", &s[..s.len().min(7)]))
            })
            .unwrap_or_default();

        let mut area = view()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .pl_3()
            .gap_3()
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        muted(format!("{} · {kind_label}{ident}", r.repository))
                            .font_weight(FontWeight::MEDIUM),
                    )
                    .child(view().flex_1())
                    .child(when(
                        {
                            let is_pr = r_review.kind == "pr";
                            move || is_pr
                        },
                        {
                            let r_rev = r_review.clone();
                            move || {
                                let repo = r_rev.repository.clone();
                                let num = r_rev.number.unwrap_or(0);
                                let url = r_rev.url.clone();
                                compact_action("Diff", Variant::Secondary, move |_| {
                                    let st = ui.review_state;
                                    let repo_val = repo.clone();
                                    let url_val = url.clone();
                                    batch(move || {
                                        ui.card.set(String::new());
                                        ui.review.set(None);
                                        st.review_card_id.set(String::new());
                                        st.pr_target.set(Some(json!({
                                            "repository": repo_val,
                                            "number": num,
                                            "url": url_val,
                                        })));
                                        st.mode.set("github_pr".into());
                                        st.selected_file.set(String::new());
                                        st.active_line.set(None);
                                        ui.navigate("review");
                                    });
                                    crate::review::refresh_active_diff(ui, false);
                                })
                            }
                        },
                    ))
                    .child(card_actions(ui, create))
                    .child(compact_action("GitHub ↗", Variant::Quiet, move |_| {
                        open_url(ui, open.url.clone())
                    }))
                    .child(chrome_button_disabled(
                        Icon::Refresh,
                        "Refresh details",
                        move || b.detail_loading.get(),
                        move |_| load(ui, true),
                    ))
                    .child(chrome_button(
                        Icon::Close,
                        "Close GitHub detail",
                        move |_| b.reference.set(None),
                    )),
            )
            .child(dynamic(move || {
                let v = b.overview.get();
                let title = v["title"]
                    .as_str()
                    .or(v["subject"]["title"].as_str())
                    .or_else(|| {
                        v["commit"]["message"]
                            .as_str()
                            .and_then(|m| m.lines().next())
                    })
                    .unwrap_or("Loading GitHub item…")
                    .to_owned();
                let (state_text, state_tone) = detail_status(&v, &r_header.kind);
                let author = v["user"]["login"]
                    .as_str()
                    .or(v["author"]["login"].as_str())
                    .or(v["commit"]["author"]["name"].as_str())
                    .unwrap_or("");
                let updated = v["updated_at"]
                    .as_str()
                    .or(v["created_at"].as_str())
                    .or(v["commit"]["author"]["date"].as_str())
                    .map(super::format_timestamp_relative)
                    .unwrap_or_default();
                let head_ref = v["head"]["ref"].as_str().unwrap_or("");
                let base_ref = v["base"]["ref"].as_str().unwrap_or("");

                let mut meta_row = view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(status(state_text, state_tone));

                if r_header.kind == "pr" && (!head_ref.is_empty() || !base_ref.is_empty()) {
                    meta_row = meta_row.child(badge(format!("{head_ref} → {base_ref}")));
                }
                if !author.is_empty() {
                    let who = if v["user"]["login"].is_string() || v["author"]["login"].is_string()
                    {
                        format!("@{author}")
                    } else {
                        author.to_owned()
                    };
                    meta_row = meta_row.child(muted(if updated.is_empty() {
                        who
                    } else {
                        format!("{who} · {updated}")
                    }));
                } else if !updated.is_empty() {
                    meta_row = meta_row.child(muted(updated));
                }

                if let Some(labels) = v["labels"].as_array().filter(|l| !l.is_empty()) {
                    for l in labels.iter().filter_map(|x| x["name"].as_str()).take(6) {
                        meta_row = meta_row.child(badge(l.to_owned()));
                    }
                }

                view()
                    .gap_1p5()
                    .pb_2()
                    .border_b_1()
                    .border_color(BORDER)
                    .child(
                        label(title)
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(meta_row)
                    .into_element()
            }));

        let mut tabs = segmented();
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
            tabs = tabs.child(compact_segment(
                title,
                None,
                move || b.section.get() == id,
                move |_| select_section(ui, id),
            ));
        }

        area = area
            .child(tabs)
            .child(when(
                move || !b.detail_error.get().is_empty(),
                move || notice(move || b.detail_error.get(), Tone::Attention),
            ))
            .child(scroll().flex_1().min_h(px(0.)).child(dynamic(move || {
                let response = b.detail.get();
                let data = response["data"].clone();
                let loading = b.detail_loading.get();
                match b.section.get().as_str() {
                    "overview" => overview_section(ui, data, loading),
                    "files" => files(ui, d, &data, loading),
                    "discussion" | "reviews" | "threads" => discussion(ui, d, &data, loading),
                    "checks" => checks_section(&data, loading),
                    _ => view().into_element(),
                }
            })))
            .child(when(
                move || {
                    b.section.get() != "overview"
                        && (b.detail_page.get() > 1 || b.detail.with(|v| v["has_next"] == true))
                },
                move || {
                    view()
                        .flex_shrink_0()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .pt_1()
                        .child(
                            compact_action("Previous", Variant::Quiet, move |_| {
                                b.detail_page.update(|p| *p = p.saturating_sub(1).max(1));
                            })
                            .disabled(move || b.detail_page.get() <= 1 || b.detail_loading.get()),
                        )
                        .child(muted(move || format!("Page {}", b.detail_page.get())))
                        .child(
                            compact_action("Next", Variant::Quiet, move |_| {
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

fn overview_section(ui: Ui, data: Value, loading: bool) -> AnyElement {
    let raw_body = data["body"]
        .as_str()
        .or(data["commit"]["message"].as_str())
        .or_else(|| {
            ui.github
                .overview
                .with_untracked(|v| {
                    v["body"]
                        .as_str()
                        .or(v["commit"]["message"].as_str())
                        .map(str::to_owned)
                })
                .as_deref()
                .map(|_| "")
        });
    let body_text = data["body"]
        .as_str()
        .or(data["commit"]["message"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            ui.github.overview.with_untracked(|v| {
                v["body"]
                    .as_str()
                    .or(v["commit"]["message"].as_str())
                    .unwrap_or("")
                    .to_owned()
            })
        });

    if body_text.trim().is_empty() {
        let _ = raw_body;
        return view()
            .py_4()
            .child(muted(if loading && data.is_null() {
                "Loading description…"
            } else {
                "No description provided."
            }))
            .into_element();
    }

    view()
        .py_1()
        .pr_2()
        .min_w(px(0.))
        .child(crate::editor::reader(ui, move || body_text.clone()))
        .into_element()
}

fn checks_section(data: &Value, loading: bool) -> AnyElement {
    let runs = data["check_runs"].as_array().cloned().unwrap_or_default();
    if runs.is_empty() {
        return view()
            .py_4()
            .child(muted(if loading {
                "Loading checks…"
            } else {
                "No check runs reported for this commit."
            }))
            .into_element();
    }
    view()
        .children(
            runs.into_iter()
                .map(|c| {
                    let name = c["name"].as_str().unwrap_or("Check").to_owned();
                    let raw_state = c["conclusion"]
                        .as_str()
                        .or(c["status"].as_str())
                        .unwrap_or("unknown");
                    let (tone, icon_kind) = match raw_state {
                        "success" => (Tone::Success, Icon::Check),
                        "failure" | "timed_out" | "cancelled" | "action_required" => {
                            (Tone::Danger, Icon::Close)
                        }
                        "in_progress" | "queued" | "pending" => (Tone::Info, Icon::History),
                        _ => (Tone::Neutral, Icon::Info),
                    };
                    let url = c["html_url"]
                        .as_str()
                        .or(c["details_url"].as_str())
                        .filter(|u| u.starts_with("https://github.com/"))
                        .map(str::to_owned);

                    let mut row = view()
                        .flex_row()
                        .items_center()
                        .gap_2p5()
                        .py_2()
                        .px_2()
                        .border_b_1()
                        .border_color(BORDER)
                        .child(
                            icon_tinted(
                                icon_kind,
                                match tone {
                                    Tone::Success => SUCCESS,
                                    Tone::Danger => DANGER,
                                    Tone::Info => ACCENT,
                                    _ => MUTED,
                                },
                            )
                            .w(px(14.))
                            .h(px(14.)),
                        )
                        .child(label(name).flex_1().min_w(px(0.)).truncate())
                        .child(status(raw_state.replace('_', " "), tone));
                    if let Some(u) = url {
                        row = row.child(compact_action("Details ↗", Variant::Quiet, move |_| {
                            let url = u.clone();
                            spawn(async move {
                                let _ =
                                    blocking(move || crate::platform::open_external(&url)).await;
                            });
                        }));
                    }
                    row.into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}

fn card_actions(ui: Ui, r: GitHubReference) -> impl IntoElement {
    dynamic(move || {
        let snapshot = ui.snapshot.get();
        let matches = super::matching_cards(&snapshot, &r);
        let mut area = view().flex_row().flex_wrap().items_center().gap_1p5();
        if let Some(first) = matches.first() {
            let first_id = first.id.clone();
            let multi_project = snapshot.projects.len() > 1;
            let proj_prefix = if multi_project {
                snapshot
                    .projects
                    .iter()
                    .find(|p| p.id == first.project_id)
                    .map(|p| format!("{} ", p.name))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let archived_suffix = if first.archived_at.is_some() {
                " (archived)"
            } else {
                ""
            };
            area = area.child(compact_action(
                format!(
                    "Open {proj_prefix}Card #{}{archived_suffix} →",
                    first.number
                ),
                Variant::Primary,
                move |_| ui.open_card(first_id.clone()),
            ));
            for extra in matches.iter().skip(1) {
                let extra_id = extra.id.clone();
                area = area.child(compact_action(
                    format!("#{} →", extra.number),
                    Variant::Secondary,
                    move |_| ui.open_card(extra_id.clone()),
                ));
            }
        } else {
            let target = r.clone();
            area = area.child(compact_action(
                "+ Add to Board",
                Variant::Primary,
                move |_| {
                    let title = ui.github.overview.with_untracked(|v| {
                        v["title"]
                            .as_str()
                            .or(v["subject"]["title"].as_str())
                            .or_else(|| {
                                v["commit"]["message"]
                                    .as_str()
                                    .and_then(|m| m.lines().next())
                            })
                            .unwrap_or("")
                            .to_owned()
                    });
                    super::add_to_board(ui, target.clone(), title);
                },
            ));
        }
        area.into_element()
    })
}

fn review_state_badge(state: &str) -> Option<(String, Tone)> {
    match state {
        "APPROVED" => Some(("Approved".into(), Tone::Success)),
        "CHANGES_REQUESTED" => Some(("Changes requested".into(), Tone::Danger)),
        "COMMENTED" => Some(("Reviewed".into(), Tone::Info)),
        "DISMISSED" => Some(("Dismissed".into(), Tone::Neutral)),
        other if !other.is_empty() => Some((other.to_owned(), Tone::Neutral)),
        _ => None,
    }
}

fn discussion(ui: Ui, d: Signal<Draft>, data: &Value, loading: bool) -> AnyElement {
    let section = ui.github.section.get_untracked();
    let item_url = ui
        .github
        .reference
        .get_untracked()
        .map(|r| r.url)
        .unwrap_or_default();
    let my_login = crate::github::active_login_from(&ui.snapshot.get(), &ui.github.gh_status.get());
    let active_edit = ui.github.editing_comment.get();
    let active_delete = ui.github.deleting_comment.get();
    let entries = data.as_array().cloned().unwrap_or_default();
    if entries.is_empty() {
        return view()
            .py_4()
            .child(muted(if loading {
                "Loading conversation…"
            } else {
                match section.as_str() {
                    "reviews" => "No reviews submitted yet.",
                    "threads" => "No inline review threads yet.",
                    _ => "No comments yet.",
                }
            }))
            .into_element();
    }

    let mut area = view().gap_4().py_1().pr_2();
    for entry in entries {
        let author = entry["user"]["login"]
            .as_str()
            .unwrap_or("GitHub")
            .to_owned();
        let comment_id = entry["id"].as_u64().unwrap_or(0);
        let web_url = super::comment_web_url(
            &entry,
            (!item_url.is_empty()).then_some(item_url.as_str()),
            &section,
        );
        let is_own = comment_id > 0
            && my_login
                .as_deref()
                .is_some_and(|me| author.eq_ignore_ascii_case(me));
        let created_raw = entry["created_at"]
            .as_str()
            .or(entry["submitted_at"].as_str());
        let updated_raw = entry["updated_at"].as_str();
        let was_edited = match (created_raw, updated_raw) {
            (Some(c), Some(u)) if !c.is_empty() && !u.is_empty() => c != u,
            _ => false,
        };
        let timestamp = updated_raw
            .or(entry["submitted_at"].as_str())
            .or(entry["created_at"].as_str())
            .map(super::format_timestamp_relative)
            .unwrap_or_default();
        let state_badge = entry["state"].as_str().and_then(review_state_badge);
        let association = entry["author_association"]
            .as_str()
            .filter(|a| !matches!(*a, "" | "NONE"))
            .map(|a| a.to_ascii_lowercase());
        let body = entry["body"].as_str().unwrap_or("").to_owned();
        let is_editing = active_edit
            .as_ref()
            .is_some_and(|(sec, cid)| sec == &section && *cid == comment_id);
        let is_deleting = active_delete
            .as_ref()
            .is_some_and(|(sec, cid)| sec == &section && *cid == comment_id);

        let mut header_row = view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(label(format!("@{author}")).font_weight(FontWeight::SEMIBOLD));

        if let Some((txt, tone)) = state_badge {
            header_row = header_row.child(status(txt, tone));
        } else if let Some(assoc) = association {
            header_row = header_row.child(badge(assoc));
        }
        if !timestamp.is_empty() {
            header_row = header_row.child(muted(timestamp));
        }
        if was_edited {
            header_row = header_row.child(muted("· edited").text_size(px(11.5)));
        }

        let mut trailing = view().flex_row().items_center().gap_1();
        let mut has_trailing = false;

        if section == "threads" {
            let path = entry["path"].as_str().unwrap_or("");
            let line = entry["line"]
                .as_u64()
                .or(entry["original_line"].as_u64())
                .unwrap_or(0);
            let outdated = entry["line"].is_null();
            if !path.is_empty() {
                header_row = header_row.child(badge(format!(
                    "{path}:{line}{}",
                    if outdated { " · outdated" } else { "" }
                )));
            }
            let reply_id = entry["in_reply_to_id"]
                .as_u64()
                .or(entry["id"].as_u64())
                .unwrap_or(0);
            if reply_id > 0 {
                has_trailing = true;
                trailing = trailing.child(compact_action("Reply", Variant::Quiet, move |_| {
                    d.update(|d| {
                        d.github_reply = reply_id;
                        d.github_compose = "comment".into();
                    })
                }));
            }
        }

        if let Some(jump_url) = web_url {
            has_trailing = true;
            trailing = trailing.child(
                view()
                    .tooltip(hint_tooltip("Open comment in GitHub"))
                    .child(
                        compact_action("GitHub ↗", Variant::Quiet, move |_| {
                            open_url(ui, jump_url.clone())
                        })
                        .accessibility_label(|| Some("Open comment in GitHub".into())),
                    ),
            );
        }

        if is_own && !is_editing && !is_deleting {
            has_trailing = true;
            let edit_sec = section.clone();
            let del_sec = section.clone();
            let initial_body = body.clone();
            let can_delete = matches!(section.as_str(), "discussion" | "threads");
            let anchor = element_ref();
            trailing = trailing.child(
                canban_ui_kit::press_target()
                    .element_ref(&anchor)
                    .w(px(26.))
                    .h(px(26.))
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .bg(CLEAR)
                    .hover(|s| s.bg(INSET))
                    .focus(|s| s.border_1().border_color(ACCENT))
                    .accessibility_label(|| Some("GitHub comment actions".into()))
                    .child(icon_tinted(Icon::More, MUTED).w(px(15.)).h(px(15.)))
                    .on_press(move |_: &()| {
                        use nsh_core::element::desktop::{MenuItem, popup_menu_at};
                        let sec_edit = edit_sec.clone();
                        let sec_del = del_sec.clone();
                        let body_val = initial_body.clone();
                        ui.spawn(async move {
                            let Some(bounds) = anchor.measure().await else {
                                return;
                            };
                            let mut items =
                                vec![MenuItem::new("Edit comment").on_select(move || {
                                    let s = sec_edit.clone();
                                    let b = body_val.clone();
                                    batch(move || {
                                        ui.github.edit_error.set(String::new());
                                        ui.github.delete_error.set(String::new());
                                        ui.github.deleting_comment.set(None);
                                        ui.github.edit_draft.set(b);
                                        ui.github.editing_comment.set(Some((s, comment_id)));
                                    });
                                })];
                            if can_delete {
                                items.push(MenuItem::new("Delete comment…").on_select(move || {
                                    let s = sec_del.clone();
                                    batch(move || {
                                        ui.github.delete_error.set(String::new());
                                        ui.github.edit_error.set(String::new());
                                        ui.github.editing_comment.set(None);
                                        ui.github.deleting_comment.set(Some((s, comment_id)));
                                    });
                                }));
                            }
                            popup_menu_at(
                                items,
                                point(bounds.page.x, bounds.page.y + bounds.frame.size.height),
                            )
                            .await;
                        });
                    }),
            );
        }

        if has_trailing {
            header_row = header_row.child(view().flex_1()).child(trailing);
        }

        let mut item_view = view()
            .gap_2()
            .pb_3()
            .border_b_1()
            .border_color(BORDER)
            .child(header_row);

        if is_deleting {
            let del_sec = section.clone();
            item_view = item_view.child(github_comment_delete_confirm(ui, d, del_sec, comment_id));
        }

        if is_editing {
            let save_sec = section.clone();
            item_view = item_view.child(github_comment_editor(ui, d, save_sec, comment_id));
        } else if !body.trim().is_empty() {
            item_view = item_view.child(crate::editor::reader(ui, move || body.clone()));
        }
        area = area.child(item_view);
    }
    area.into_element()
}

fn github_comment_delete_confirm(
    ui: Ui,
    d: Signal<Draft>,
    section: String,
    comment_id: u64,
) -> impl IntoElement {
    let b = ui.github;
    view()
        .gap_2()
        .px_3()
        .py_2()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(DANGER)
        .bg(DANGER_SURFACE)
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    label("Delete this comment on GitHub?")
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(DANGER),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .child(
                            compact_action("Cancel", Variant::Quiet, move |_| {
                                batch(move || {
                                    b.deleting_comment.set(None);
                                    b.delete_error.set(String::new());
                                });
                            })
                            .disabled(move || ui.is_method_pending("github.remote.comment.delete")),
                        )
                        .child(
                            compact_action("Delete", Variant::Danger, move |_| {
                                let Some(r) = b.reference.get_untracked() else {
                                    return;
                                };
                                b.delete_error.set(String::new());
                                if !crate::backend::is_connected() {
                                    b.detail.update(|det| {
                                        if let Some(arr) = det["data"].as_array_mut() {
                                            arr.retain(|x| x["id"].as_u64() != Some(comment_id));
                                        }
                                    });
                                    b.deleting_comment.set(None);
                                    ui.notify("Comment deleted");
                                    return;
                                }
                                let page = b.detail_page.get_untracked();
                                dispatch_complete(
                                    ui,
                                    "github.remote.comment.delete",
                                    json!({
                                        "reference": r,
                                        "section": section,
                                        "comment_id": comment_id,
                                        "page": page,
                                        "request_key": canban_types::id(),
                                    }),
                                    move |res| match res {
                                        Ok(val) if val["state"] == "published" => {
                                            d.update(|d| d.github_unknown = false);
                                            b.deleting_comment.set(None);
                                            b.delete_error.set(String::new());
                                            load(ui, true);
                                            ui.notify("Comment deleted");
                                        }
                                        Ok(val) if val["state"] == "unknown" => {
                                            d.update(|d| d.github_unknown = true);
                                            b.delete_error.set(
                                                val["message"]
                                                    .as_str()
                                                    .unwrap_or(
                                                        "Deletion outcome is unknown; check GitHub before retrying.",
                                                    )
                                                    .to_owned(),
                                            );
                                        }
                                        Ok(val) => {
                                            b.delete_error.set(
                                                val["message"]
                                                    .as_str()
                                                    .unwrap_or("Could not delete GitHub comment")
                                                    .to_owned(),
                                            );
                                        }
                                        Err(err) => {
                                            b.delete_error.set(err);
                                        }
                                    },
                                );
                            })
                            .disabled(move || {
                                ui.is_method_pending("github.remote.comment.delete")
                                    || d.get().github_unknown
                            }),
                        ),
                ),
        )
        .child(when(
            move || !b.delete_error.get().is_empty(),
            move || notice(move || b.delete_error.get(), Tone::Danger),
        ))
}

fn github_comment_editor(
    ui: Ui,
    d: Signal<Draft>,
    section: String,
    comment_id: u64,
) -> impl IntoElement {
    let b = ui.github;
    view()
        .gap_2()
        .p_2p5()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(ACCENT)
        .bg(SURFACE)
        .child(multiline_box(
            move || b.edit_draft.get(),
            move |v| b.edit_draft.set(v),
            "Edit GitHub comment",
            "Update your GitHub comment…",
            86.,
        ))
        .child(when(
            move || !b.edit_error.get().is_empty(),
            move || notice(move || b.edit_error.get(), Tone::Danger),
        ))
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_end()
                .gap_2()
                .child(compact_action("Cancel", Variant::Quiet, move |_| {
                    batch(move || {
                        b.editing_comment.set(None);
                        b.edit_draft.set(String::new());
                        b.edit_error.set(String::new());
                    });
                }))
                .child(
                    compact_action("Save", Variant::Primary, move |_| {
                        let next_body = b.edit_draft.get_untracked();
                        if next_body.trim().is_empty() {
                            b.edit_error.set("Comment cannot be empty".into());
                            return;
                        }
                        let Some(r) = b.reference.get_untracked() else {
                            return;
                        };
                        b.edit_error.set(String::new());
                        if !crate::backend::is_connected() {
                            let now_iso = "2026-04-19T12:00:00Z";
                            b.detail.update(|det| {
                                if let Some(arr) = det["data"].as_array_mut()
                                    && let Some(item) = arr
                                        .iter_mut()
                                        .find(|x| x["id"].as_u64() == Some(comment_id))
                                {
                                    item["body"] = json!(next_body.trim());
                                    item["updated_at"] = json!(now_iso);
                                }
                            });
                            b.editing_comment.set(None);
                            b.edit_draft.set(String::new());
                            ui.notify("Saved");
                            return;
                        }
                        let page = b.detail_page.get_untracked();
                        submit(
                            ui,
                            d,
                            "github.remote.comment.update",
                            json!({
                                "reference": r,
                                "section": section,
                                "comment_id": comment_id,
                                "body": next_body,
                                "page": page,
                            }),
                            move |_| {
                                b.editing_comment.set(None);
                                b.edit_draft.set(String::new());
                                b.edit_error.set(String::new());
                                load(ui, true);
                            },
                        );
                    })
                    .disabled(move || {
                        ui.is_method_pending("github.remote.comment.update")
                            || d.get().github_unknown
                            || b.edit_draft.with(|s| s.trim().is_empty())
                    }),
                ),
        )
}

struct ParsedDiffLine {
    kind: char,
    old_line: Option<u64>,
    new_line: Option<u64>,
    text: String,
}

fn parse_unified_patch(patch: &str) -> Vec<ParsedDiffLine> {
    let mut out = Vec::new();
    let mut old_no = 0u64;
    let mut new_no = 0u64;
    for raw in patch.lines() {
        if let Some(rest) = raw.strip_prefix("@@") {
            if let Some((header, _)) = rest.split_once("@@") {
                for token in header.split_whitespace() {
                    if let Some(o) = token.strip_prefix('-') {
                        old_no = o
                            .split(',')
                            .next()
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(0);
                    } else if let Some(n) = token.strip_prefix('+') {
                        new_no = n
                            .split(',')
                            .next()
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(0);
                    }
                }
            }
            out.push(ParsedDiffLine {
                kind: '@',
                old_line: None,
                new_line: None,
                text: raw.to_owned(),
            });
        } else if let Some(added) = raw.strip_prefix('+') {
            let line = new_no;
            new_no += 1;
            out.push(ParsedDiffLine {
                kind: '+',
                old_line: None,
                new_line: Some(line),
                text: added.to_owned(),
            });
        } else if let Some(removed) = raw.strip_prefix('-') {
            let line = old_no;
            old_no += 1;
            out.push(ParsedDiffLine {
                kind: '-',
                old_line: Some(line),
                new_line: None,
                text: removed.to_owned(),
            });
        } else if raw.starts_with('\\') {
            out.push(ParsedDiffLine {
                kind: '\\',
                old_line: None,
                new_line: None,
                text: raw.to_owned(),
            });
        } else {
            let o = old_no;
            let n = new_no;
            old_no += 1;
            new_no += 1;
            out.push(ParsedDiffLine {
                kind: ' ',
                old_line: Some(o),
                new_line: Some(n),
                text: raw.strip_prefix(' ').unwrap_or(raw).to_owned(),
            });
        }
    }
    out
}

fn files(ui: Ui, d: Signal<Draft>, data: &Value, loading: bool) -> AnyElement {
    let _ = ui;
    let files = data.as_array().cloned().unwrap_or_default();
    if files.is_empty() {
        return view()
            .py_4()
            .child(muted(if loading {
                "Loading changed files…"
            } else {
                "No files loaded. Refresh if the PR has changed."
            }))
            .into_element();
    }

    let mut area = view().gap_4().py_1().pr_2().child(muted(
        "Click any diff line (or + Comment on file) to attach an inline Review comment.",
    ));

    for file in files {
        let path = file["filename"].as_str().unwrap_or("").to_owned();
        let file_status = file["status"].as_str().unwrap_or("modified").to_owned();
        let additions = file["additions"].as_u64().unwrap_or(0);
        let deletions = file["deletions"].as_u64().unwrap_or(0);
        let patch = file["patch"].as_str().map(str::to_owned);
        let select_path = path.clone();

        let header = view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .bg(INSET)
            .border_b_1()
            .border_color(BORDER)
            .child(crate::review::diff::diff_header_path(
                path.clone(),
                file["previous_filename"].as_str(),
                FontWeight::SEMIBOLD,
                true,
            ))
            .child(badge(file_status))
            .child(
                label(format!("+{additions}"))
                    .text_size(px(11.5))
                    .font_family(MONO_FONT)
                    .text_color(SUCCESS),
            )
            .child(
                label(format!("−{deletions}"))
                    .text_size(px(11.5))
                    .font_family(MONO_FONT)
                    .text_color(DANGER),
            )
            .child(compact_action("+ Comment", Variant::Quiet, move |_| {
                d.update(|d| {
                    d.github_file = select_path.clone();
                    d.github_compose = "review".into();
                });
            }));

        let body = if let Some(patch_str) = patch {
            let parsed = parse_unified_patch(&patch_str);
            let mut rows = view();
            for dl in parsed.into_iter().take(400) {
                if dl.kind == '@' || dl.kind == '\\' {
                    rows = rows.child(
                        view().px_3().py_1().bg(INSET).child(
                            label(dl.text)
                                .font_family(MONO_FONT)
                                .text_size(px(11.5))
                                .text_color(MUTED),
                        ),
                    );
                    continue;
                }
                let comment_line = dl.new_line.or(dl.old_line).unwrap_or(1);
                let line_file = path.clone();
                let old_str = dl
                    .old_line
                    .map(|n| format!("{n:>4}"))
                    .unwrap_or_else(|| "    ".into());
                let new_str = dl
                    .new_line
                    .map(|n| format!("{n:>4}"))
                    .unwrap_or_else(|| "    ".into());
                let prefix = dl.kind;
                let text_col = match dl.kind {
                    '+' => SUCCESS,
                    '-' => DANGER,
                    _ => TEXT,
                };

                let mut line_btn = canban_ui_kit::press_target()
                    .w_full()
                    .px_2()
                    .py_0p5()
                    .flex_row()
                    .items_start()
                    .gap_2();
                line_btn = match dl.kind {
                    '+' => line_btn.bg(SUCCESS_SURFACE),
                    '-' => line_btn.bg(DANGER_SURFACE),
                    _ => line_btn.bg(CLEAR),
                };

                rows = rows.child(
                    line_btn
                        .hover(|s| s.bg(SELECTED))
                        .accessibility_label(format!("Comment on {path} line {comment_line}"))
                        .on_press(move |_: &()| {
                            d.update(|d| {
                                d.github_file = line_file.clone();
                                d.github_line = comment_line.to_string();
                                d.github_compose = "review".into();
                            });
                        })
                        .child(
                            label(format!("{old_str} {new_str}"))
                                .w(px(74.))
                                .flex_shrink_0()
                                .font_family(MONO_FONT)
                                .text_size(px(11.))
                                .text_color(MUTED),
                        )
                        .child(
                            label(format!("{prefix} {}", dl.text))
                                .flex_1()
                                .min_w(px(0.))
                                .font_family(MONO_FONT)
                                .text_size(px(12.))
                                .text_color(text_col),
                        ),
                );
            }
            rows.into_element()
        } else {
            view()
                .p_3()
                .child(muted(
                    "Binary file or large diff omitted. Open on GitHub for the full file.",
                ))
                .into_element()
        };

        area = area.child(
            view()
                .rounded(px(RADIUS))
                .border_1()
                .border_color(BORDER)
                .overflow_hidden()
                .child(header)
                .child(body),
        );
    }
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

fn multiline_box(
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
    label_text: &'static str,
    placeholder: &'static str,
    height: f32,
) -> View {
    let focused = signal(false);
    view()
        .h(px(height))
        .min_w(px(0.))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(move || if focused.get() { ACCENT } else { BORDER })
        .bg(INSET)
        .overflow_hidden()
        .child(
            nsh_base::editor()
                .value(move || Some(value().into()))
                .on_change(move |v: &SharedString| on_change(v.to_string()))
                .on_focus(move |_: &()| focused.set(true))
                .on_blur(move |_: &()| focused.set(false))
                .accessibility_label(label_text)
                .placeholder(placeholder)
                .line_numbers(false)
                .folding(false)
                .soft_wrap(true)
                .active_line(false)
                .indent_guides(false)
                .auto_close(false)
                .auto_indent(false)
                .font_size(px(13.))
                .line_height(px(19.))
                .text_color(TEXT)
                .px_2()
                .py_1()
                .size_full(),
        )
}

fn composer(ui: Ui, d: Signal<Draft>, r: GitHubReference) -> impl IntoElement {
    let is_pr = r.kind == "pr";
    let comment = r.clone();
    let mut buttons = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(compact_action(
            move || {
                if d.get().github_compose == "comment" {
                    "Hide comment ▾"
                } else {
                    "Comment on GitHub"
                }
            },
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
        buttons = buttons.child(compact_action(
            move || {
                let count = d.get().github_comments.len();
                if d.get().github_compose == "review" {
                    if count > 0 {
                        format!("Hide Review ({count} pending) ▾")
                    } else {
                        "Hide Review ▾".into()
                    }
                } else if count > 0 {
                    format!("Review ({count} pending)")
                } else {
                    "Review PR".into()
                }
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
        .flex_shrink_0()
        .border_t_1()
        .border_color(BORDER)
        .pt_2()
        .gap_2()
        .child(buttons)
        .child(when(
            move || d.get().github_unknown,
            move || {
                view()
                    .gap_1()
                    .child(notice(
                        "Publication outcome is unknown. Your draft is retained; check GitHub before submitting again.",
                        Tone::Attention,
                    ))
                    .child(
                        action("Check on GitHub", Variant::Quiet, {
                            let check = check.clone();
                            move |_| open_url(ui, check.clone())
                        })
                        .self_start(),
                    )
                    .child(
                        action(
                            "I checked GitHub · enable a new submission",
                            Variant::Secondary,
                            move |_| {
                                d.update(|d| {
                                    d.github_unknown = false;
                                    d.github_action_key.clear();
                                    d.github_action_payload.clear();
                                })
                            },
                        )
                        .self_start(),
                    )
            },
        ))
        .child(when(
            move || d.get().github_compose == "comment",
            move || comment_form(ui, d, comment.clone()),
        ))
        .child(when(
            move || d.get().github_compose == "review",
            move || {
                scroll()
                    .max_h(px(260.))
                    .min_h(px(0.))
                    .child(review_form(ui, d, r.clone()))
            },
        ))
}

fn comment_form(ui: Ui, d: Signal<Draft>, r: GitHubReference) -> impl IntoElement {
    panel()
        .p_3()
        .gap_2()
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(label("GitHub comment").font_weight(FontWeight::SEMIBOLD))
                .child(chrome_button(Icon::Close, "Close comment composer", move |_| {
                    d.update(|d| d.github_compose.clear())
                })),
        )
        .child(multiline_box(
            move || d.get().comment,
            move |v| d.update(|d| d.comment = v),
            "GitHub comment",
            "Write a comment to publish on GitHub…",
            82.,
        ))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    action(
                        move || {
                            if d.get().github_reply > 0 {
                                "Publish reply to thread"
                            } else {
                                "Publish to GitHub"
                            }
                        },
                        Variant::Primary,
                        move |_| {
                            let draft = d.get_untracked();
                            let method = if draft.github_reply > 0 {
                                "github.remote.reply"
                            } else {
                                "github.remote.comment"
                            };
                            submit(
                                ui,
                                d,
                                method,
                                json!({"reference": r, "body": draft.comment, "comment_id": draft.github_reply}),
                                move |_| {
                                    d.update(|d| {
                                        d.comment.clear();
                                        d.github_reply = 0;
                                        d.github_compose.clear();
                                    });
                                    load(ui, true);
                                },
                            );
                        },
                    )
                    .disabled(move || {
                        ui.is_method_pending("github.remote.comment")
                            || ui.is_method_pending("github.remote.reply")
                            || d.get().github_unknown
                            || d.with(|d| d.comment.trim().is_empty())
                    }),
                )
                .child(when(
                    move || d.get().github_reply > 0,
                    move || {
                        action("Cancel thread reply", Variant::Quiet, move |_| {
                            d.update(|d| d.github_reply = 0)
                        })
                    },
                )),
        )
}

fn review_form(ui: Ui, d: Signal<Draft>, r: GitHubReference) -> impl IntoElement {
    let event = signal("COMMENT".to_owned());
    let side = signal("RIGHT".to_owned());
    panel()
        .p_3()
        .gap_2()
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(label("Review · current PR version").font_weight(FontWeight::SEMIBOLD))
                .child(chrome_button(Icon::Close, "Close review composer", move |_| {
                    d.update(|d| d.github_compose.clear())
                })),
        )
        .child(when(
            move || {
                d.with(|d| !d.github_head.is_empty())
                    && d.get().github_head
                        != ui
                            .github
                            .overview
                            .with(|v| v["head"]["sha"].as_str().unwrap_or("").to_owned())
            },
            move || {
                view()
                    .gap_1()
                    .child(notice(
                        "The PR has a new version. Your Review draft still belongs to the previous head.",
                        Tone::Attention,
                    ))
                    .child(
                        action(
                            "Discard line comments and use latest version",
                            Variant::Secondary,
                            move |_| {
                                let head = ui.github.overview.get_untracked()["head"]["sha"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_owned();
                                d.update(|d| {
                                    d.github_comments.clear();
                                    d.github_file.clear();
                                    d.github_line.clear();
                                    d.instructions.clear();
                                    d.github_head = head;
                                });
                            },
                        )
                        .self_start(),
                    )
            },
        ))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(muted(move || {
                    let f = d.get().github_file;
                    if f.is_empty() {
                        "File: click a diff line above".into()
                    } else {
                        format!("File: {f}")
                    }
                }))
                .child(
                    bound_field(
                        move || d.get().github_line,
                        move |v| d.update(|d| d.github_line = v.into()),
                        "Line #",
                    )
                    .w(px(96.))
                    .key("github-review-line"),
                )
                .child(choice(
                    ui,
                    side,
                    vec![("RIGHT", "New side"), ("LEFT", "Old side")],
                )),
        )
        .child(multiline_box(
            move || d.get().instructions,
            move |v| d.update(|d| d.instructions = v),
            "Line comment",
            "Write an inline comment for the selected file and line…",
            62.,
        ))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    compact_action("Add line comment", Variant::Secondary, move |_| {
                        let draft = d.get_untracked();
                        if let Ok(line) = draft.github_line.parse::<u64>()
                            && line > 0
                            && !draft.github_file.is_empty()
                            && !draft.instructions.trim().is_empty()
                        {
                            d.update(|d| {
                                d.github_comments.push(json!({
                                    "path": draft.github_file,
                                    "line": line,
                                    "side": side.get_untracked(),
                                    "body": draft.instructions
                                }));
                                d.instructions.clear();
                            });
                        }
                    })
                    .disabled(move || {
                        d.with(|d| {
                            d.github_file.is_empty()
                                || d.github_line.parse::<u64>().is_err()
                                || d.instructions.trim().is_empty()
                        })
                    }),
                )
                .child(muted(move || {
                    format!("{} pending line comments", d.get().github_comments.len())
                })),
        )
        .child(dynamic(move || {
            let comments = d.get().github_comments;
            if comments.is_empty() {
                return view().into_element();
            }
            view()
                .gap_1()
                .children(
                    comments
                        .into_iter()
                        .enumerate()
                        .map(|(idx, c)| {
                            let loc = format!(
                                "{}:{}",
                                c["path"].as_str().unwrap_or(""),
                                c["line"].as_u64().unwrap_or(0)
                            );
                            let body = c["body"].as_str().unwrap_or("").to_owned();
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded(px(RADIUS))
                                .bg(INSET)
                                .child(badge(loc))
                                .child(label(body).flex_1().min_w(px(0.)).truncate())
                                .child(chrome_button(
                                    Icon::Close,
                                    "Remove pending line comment",
                                    move |_| {
                                        d.update(|d| {
                                            if idx < d.github_comments.len() {
                                                d.github_comments.remove(idx);
                                            }
                                        });
                                    },
                                ))
                                .into_element()
                        })
                        .collect::<Vec<_>>(),
                )
                .into_element()
        }))
        .child(multiline_box(
            move || d.get().body,
            move |v| d.update(|d| d.body = v),
            "Review summary",
            "Write overall review summary…",
            68.,
        ))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(choice(
                    ui,
                    event,
                    vec![
                        ("COMMENT", "Comment"),
                        ("APPROVE", "Approve"),
                        ("REQUEST_CHANGES", "Request changes"),
                    ],
                ))
                .child(
                    action("Submit Review to GitHub", Variant::Primary, move |_| {
                        let draft = d.get_untracked();
                        submit(
                            ui,
                            d,
                            "github.remote.review",
                            json!({
                                "reference": r,
                                "head": draft.github_head,
                                "body": draft.body,
                                "comments": draft.github_comments,
                                "event": event.get_untracked(),
                            }),
                            move |_| {
                                d.update(|d| {
                                    d.github_comments.clear();
                                    d.body.clear();
                                    d.github_compose.clear();
                                });
                                load(ui, true);
                            },
                        );
                    })
                    .disabled(move || {
                        ui.is_method_pending("github.remote.review")
                            || d.get().github_unknown
                            || (event.get() == "COMMENT"
                                && d.with(|d| {
                                    d.body.trim().is_empty() && d.github_comments.is_empty()
                                }))
                            || d.get().github_head.is_empty()
                            || d.get().github_head
                                != ui
                                    .github
                                    .overview
                                    .with(|v| v["head"]["sha"].as_str().unwrap_or("").to_owned())
                    }),
                ),
        )
}
