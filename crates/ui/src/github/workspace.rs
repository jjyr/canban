//! Window-owned GitHub browser state. Reads use AppServer's shared cache.
use super::*;
use canban_ui_kit::tokens::*;

#[derive(Clone, Copy)]
pub struct Browser {
    pub tab: Signal<String>,
    pub query: Signal<String>,
    pub repository: Signal<String>,
    pub relation: Signal<String>,
    pub state: Signal<String>,
    pub page: Signal<u64>,
    pub list: Signal<Value>,
    pub error: Signal<String>,
    pub loading: Signal<bool>,
    pub epoch: Signal<u64>,
    pub ticket: Signal<u64>,
    pub reference: Signal<Option<GitHubReference>>,
    pub section: Signal<String>,
    pub detail: Signal<Value>,
    pub overview: Signal<Value>,
    pub detail_page: Signal<u64>,
    pub detail_error: Signal<String>,
    pub detail_loading: Signal<bool>,
    pub detail_ticket: Signal<u64>,
    pub gh_status: Signal<Value>,
    pub cache_bytes: Signal<u64>,
    pub filters: Signal<std::collections::BTreeMap<String, BoardFilter>>,
    pub width: Signal<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoardFilter {
    pub source: String,
    pub kind: String,
    pub author: String,
    pub state: String,
}

impl Default for Browser {
    fn default() -> Self {
        Self::new()
    }
}

impl Browser {
    pub fn new() -> Self {
        Self {
            tab: signal("notifications".into()),
            query: signal(String::new()),
            repository: signal(String::new()),
            relation: signal("all".into()),
            state: signal("inbox".into()),
            page: signal(1),
            list: signal(json!({})),
            error: signal(String::new()),
            loading: signal(false),
            epoch: signal(0),
            ticket: signal(0),
            reference: signal(None),
            section: signal("overview".into()),
            detail: signal(json!({})),
            overview: signal(json!({})),
            detail_page: signal(1),
            detail_error: signal(String::new()),
            detail_loading: signal(false),
            detail_ticket: signal(0),
            gh_status: signal(json!({})),
            cache_bytes: signal(0),
            filters: signal(Default::default()),
            width: signal(460.),
        }
    }
}

pub fn open(ui: Ui, r: GitHubReference) {
    let b = ui.github;
    if b.reference.get_untracked().as_ref() != Some(&r) {
        b.reference.set(Some(r.clone()));
        b.section.set("overview".into());
        b.detail_page.set(1);
        // Seed overview from current list row if available so title/status render immediately.
        let seeded = b.list.with_untracked(|v| {
            v["items"]
                .as_array()
                .and_then(|rows| {
                    rows.iter().find_map(|row| {
                        let row_ref: Option<GitHubReference> =
                            serde_json::from_value(row["reference"].clone()).ok();
                        (row_ref.as_ref() == Some(&r)).then(|| row["item"].clone())
                    })
                })
                .unwrap_or_else(|| json!({}))
        });
        b.overview.set(seeded);
        b.detail.set(json!({}));
    }
    ui.navigate("github");
}

pub fn init(ui: Ui) {
    let b = ui.github;
    let scope = computed(move || {
        (
            b.tab.get(),
            b.query.get(),
            b.repository.get(),
            b.relation.get(),
            b.state.get(),
        )
    });
    let previous_scope = stored(scope.get_untracked());
    effect(move || {
        let current = scope.get();
        if previous_scope.replace(current.clone()) != current {
            b.page.set(1);
            b.list.set(json!({}));
        }
    });
    let account = computed(move || {
        let (app_state, app_login) = ui
            .snapshot
            .with(|s| (s.github.state.clone(), s.github.login.clone()));
        let gh_login = b.gh_status.with(|v| {
            (
                v["state"].as_str().unwrap_or_default().to_owned(),
                v["login"].as_str().map(str::to_owned),
            )
        });
        (app_state, app_login, gh_login)
    });
    effect(move || {
        let active = ui.page.get() == "github";
        let _ = (
            b.tab.get(),
            b.query.get(),
            b.repository.get(),
            b.relation.get(),
            b.state.get(),
            b.page.get(),
            b.epoch.get(),
            account.get(),
        );
        if !active {
            return;
        }
        let ticket = b.ticket.get_untracked() + 1;
        b.ticket.set(ticket);
        set_timeout(std::time::Duration::from_millis(200), move || {
            if b.ticket.try_get() == Some(ticket) {
                load(ui, false)
            }
        });
    });
    effect(move || {
        let _ = (
            b.reference.get(),
            b.section.get(),
            b.detail_page.get(),
            b.epoch.get(),
        );
        if ui.page.get() == "github" && b.reference.get_untracked().is_some() {
            super::detail::load(ui, false);
        }
    });
    if crate::APP.get().is_some() {
        check_cli(ui);
    }
    effect(move || {
        let page = ui.page.get();
        if matches!(page.as_str(), "github" | "github-account") {
            check_cli(ui);
        }
        if page == "github-account" {
            check_cache(ui);
        }
    });
}

pub fn launch_gh_login(ui: Ui) {
    if crate::APP.get().is_none() {
        ui.notify("Fixture: opens a local terminal running `gh auth login`");
        return;
    }
    dispatch_then(ui, "github.cli.login", json!({}), move |v| {
        if let Some(id) = v["id"].as_str() {
            ui.open_terminal(id.into());
        }
    });
}

pub fn check_cli(ui: Ui) {
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    ui.spawn(async move {
        let result = blocking(move || app.call("github.cli.status", json!({}))).await;
        ui.github.gh_status.set(match result {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => json!({"state":"unavailable","message":format!("{e:#}")}),
            Err(e) => json!({"state":"unavailable","message":e.to_string()}),
        });
    });
}

pub fn check_cache(ui: Ui) {
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    ui.spawn(async move {
        if let Ok(Ok(v)) = blocking(move || app.call("github.cache.status", json!({}))).await
            && let Some(bytes) = v["bytes"].as_u64()
        {
            ui.github.cache_bytes.set(bytes);
        }
    });
}

pub fn load(ui: Ui, refresh: bool) {
    let b = ui.github;
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    let params = json!({
        "kind": b.tab.get_untracked(),
        "query": b.query.get_untracked(),
        "repository": b.repository.get_untracked(),
        "relation": b.relation.get_untracked(),
        "state": b.state.get_untracked(),
        "page": b.page.get_untracked(),
        "refresh": refresh,
    });
    let ticket = b.ticket.get_untracked() + 1;
    b.ticket.set(ticket);
    b.loading.set(true);
    b.error.set(String::new());
    ui.spawn(async move {
        let result = blocking(move || app.call("github.browse", params)).await;
        if b.ticket.try_get() != Some(ticket) {
            return;
        }
        b.loading.set(false);
        match result {
            Ok(Ok(v)) => b.list.set(v),
            Ok(Err(e)) => b.error.set(format!("{e:#}")),
            Err(e) => b.error.set(e.to_string()),
        }
    });
}

fn change_tab(ui: Ui, tab: &str) {
    let b = ui.github;
    batch(move || {
        b.tab.set(tab.into());
        b.state.set(
            if tab == "notifications" {
                "inbox"
            } else {
                "open"
            }
            .into(),
        );
        b.relation.set(
            if tab == "notifications" {
                "all"
            } else if tab == "prs" {
                "review_requested"
            } else {
                "assigned"
            }
            .into(),
        );
        b.page.set(1);
        b.list.set(json!({}));
        b.reference.set(None);
    });
}

fn account_button(ui: Ui) -> impl IntoElement {
    let b = ui.github;
    dynamic(move || {
        let app_auth = ui.snapshot.with(|s| s.github.clone());
        let gh = b.gh_status.get();
        let app_login = app_auth
            .login
            .as_deref()
            .filter(|_| app_auth.state == "connected");
        let gh_login = gh["login"].as_str().filter(|_| gh["state"] == "connected");
        let mismatch = match (app_login, gh_login) {
            (Some(a), Some(g)) => !a.eq_ignore_ascii_case(g),
            _ => false,
        };
        let (dot_color, text, tip) = if mismatch {
            (
                ATTENTION,
                format!(
                    "@{} / @{} · Mismatch",
                    app_login.unwrap_or(""),
                    gh_login.unwrap_or("")
                ),
                "GitHub App and gh CLI are signed into different accounts. Click to manage Account & access."
                    .to_owned(),
            )
        } else if let (Some(a), Some(_)) = (app_login, gh_login) {
            (
                SUCCESS,
                format!("@{a} · App + gh"),
                format!(
                    "Signed in as @{a} via GitHub CLI and GitHub App. Click for Account & access."
                ),
            )
        } else if let Some(g) = gh_login {
            (
                SUCCESS,
                format!("@{g}"),
                format!(
                    "Connected to GitHub as @{g} via gh CLI (Notifications, Issues, PRs & Sync). Click for Account & access."
                ),
            )
        } else if let Some(a) = app_login {
            (
                ATTENTION,
                format!("@{a} · No gh CLI"),
                format!(
                    "Signed in as @{a} via GitHub App only; install & sign into gh CLI to enable Notifications. Click for Account & access."
                ),
            )
        } else if app_auth.state == "authorizing" {
            (
                ACCENT,
                "Authorizing…".into(),
                "Waiting for GitHub browser sign-in. Click for Account & access.".into(),
            )
        } else {
            (
                MUTED,
                "Connect GitHub".into(),
                "Install and sign into GitHub CLI (`gh`) to connect GitHub.".into(),
            )
        };

        view()
            .flex_shrink_0()
            .window_drag(false)
            .tooltip(
                nsh_base::tooltip()
                    .placement(Placement::Bottom)
                    .px_3()
                    .py_2()
                    .rounded(px(RADIUS))
                    .bg(SURFACE)
                    .border_1()
                    .border_color(BORDER)
                    .child(label(tip)),
            )
            .child(
                nsh_base::button()
                    .h(px(28.))
                    .px_2p5()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(if mismatch { ATTENTION } else { BORDER })
                    .bg(INSET)
                    .hover(|s| s.bg(CARD_HOVER))
                    .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                    .accessibility_label(move || Some("GitHub Account & access".into()))
                    .on_press(move |_: &()| ui.navigate("github-account"))
                    .child(view().w(px(7.)).h(px(7.)).rounded(px(99.)).bg(dot_color))
                    .child(
                        label(text)
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM),
                    ),
            )
            .into_element()
    })
}

pub fn chrome_bar(ui: Ui) -> impl IntoElement {
    let b = ui.github;
    view()
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .flex_row()
        .items_center()
        .gap_2()
        .pl_2()
        .window_drag(true)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .window_drag(false)
                .child(icon(Icon::GitHub).w(px(14.)).h(px(14.)))
                .child(
                    label("GitHub")
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(13.5)),
                ),
        )
        .child(
            view().window_drag(false).child(
                segmented().children(
                    [
                        ("notifications", "Notifications"),
                        ("issues", "Issues"),
                        ("prs", "Pull requests"),
                    ]
                    .into_iter()
                    .map(|(id, title)| {
                        compact_segment(
                            title,
                            None,
                            move || b.tab.get() == id,
                            move |_| change_tab(ui, id),
                        )
                        .into_element()
                    })
                    .collect::<Vec<_>>(),
                ),
            ),
        )
        .child(view().flex_1().h_full().window_drag(true))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .flex_shrink_0()
                .window_drag(false)
                .child(account_button(ui))
                .child(chrome_button_disabled(
                    Icon::Refresh,
                    "Refresh GitHub",
                    move || b.loading.get() || b.detail_loading.get(),
                    move |_| {
                        b.page.set(1);
                        load(ui, true);
                        check_cli(ui);
                        if b.reference.get_untracked().is_some() {
                            super::detail::load(ui, true)
                        }
                    },
                )),
        )
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let b = ui.github;
    view()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .p_4()
        .gap_3()
        .child(filter_bar(ui))
        .child(when(
            move || {
                let err = b.error.get();
                let gh_connected = b.gh_status.with(|v| v["state"] == "connected");
                let app_connected = ui.snapshot.with(|s| s.github.state == "connected");
                let tab = b.tab.get();
                let needs_onboarding = if tab == "notifications" {
                    !gh_connected
                } else {
                    !gh_connected && !app_connected
                };
                !err.is_empty() && !needs_onboarding
            },
            move || notice(move || b.error.get(), Tone::Attention),
        ))
        .child(dynamic(move || {
            let has_detail = b.reference.get().is_some();
            let narrow = ui.narrow.get();
            untracked(|| {
                if has_detail {
                    if narrow {
                        super::detail::screen(ui).into_element()
                    } else {
                        canban_ui_kit::split(
                            list_panel(ui).into_element(),
                            super::detail::screen(ui).into_element(),
                            b.width,
                            300.,
                            380.,
                        )
                        .into_element()
                    }
                } else {
                    list_panel(ui).into_element()
                }
            })
        }))
}

fn filter_bar(ui: Ui) -> impl IntoElement {
    let b = ui.github;
    dynamic(move || {
        let tab = b.tab.get();
        if tab == "notifications" {
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .pb_2()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    segmented().children(
                        [("inbox", "Inbox"), ("done", "Done")]
                            .into_iter()
                            .map(|(id, title)| {
                                compact_segment(
                                    title,
                                    None,
                                    move || b.state.get() == id,
                                    move |_| b.state.set(id.into()),
                                )
                                .into_element()
                            })
                            .collect::<Vec<_>>(),
                    ),
                )
                .child(
                    segmented().children(
                        [
                            ("all", "All notifications"),
                            ("participating", "Participating"),
                        ]
                        .into_iter()
                        .map(|(id, title)| {
                            compact_segment(
                                title,
                                None,
                                move || b.relation.get() == id,
                                move |_| b.relation.set(id.into()),
                            )
                            .into_element()
                        })
                        .collect::<Vec<_>>(),
                    ),
                )
                .child(view().flex_1())
                .child(muted(
                    "Click an item to inspect its overview, discussion, or diff.",
                ))
                .into_element()
        } else {
            let state_options = if tab == "prs" {
                vec![
                    ("open", "Open"),
                    ("merged", "Merged"),
                    ("closed", "Closed"),
                    ("all", "All states"),
                ]
            } else {
                vec![
                    ("open", "Open"),
                    ("closed", "Closed"),
                    ("all", "All states"),
                ]
            };
            let relation_options = if tab == "prs" {
                vec![
                    ("review_requested", "Review requested"),
                    ("authored", "Created by me"),
                    ("assigned", "Assigned to me"),
                    ("mentions", "Mentioning me"),
                    ("all", "All accessible"),
                ]
            } else {
                vec![
                    ("assigned", "Assigned to me"),
                    ("authored", "Created by me"),
                    ("mentions", "Mentioning me"),
                    ("all", "All accessible"),
                ]
            };
            let project_repo = ui.snapshot.with(|s| {
                let current = ui.project.get();
                s.projects
                    .iter()
                    .find(|p| p.id == current)
                    .and_then(|p| p.github.as_ref().map(|g| g.repository.clone()))
            });
            let mut row = view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .pb_2()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    bound_field(
                        move || b.query.get(),
                        move |v| {
                            b.query.set(v.into());
                            b.page.set(1);
                            b.list.set(json!({}));
                        },
                        if tab == "prs" {
                            "Filter pull requests…"
                        } else {
                            "Filter issues…"
                        },
                    )
                    .w(px(220.))
                    .key("github-query"),
                )
                .child(
                    bound_field(
                        move || b.repository.get(),
                        move |v| {
                            b.repository.set(v.into());
                            b.page.set(1);
                            b.list.set(json!({}));
                        },
                        "owner/repo (optional)",
                    )
                    .w(px(190.))
                    .key("github-repository-filter"),
                );
            if let Some(repo) = project_repo {
                let repo_active = repo.clone();
                let repo_toggle = repo.clone();
                row = row.child(compact_action(
                    move || {
                        if b.repository.get().eq_ignore_ascii_case(&repo_active) {
                            format!("{repo_active} ×")
                        } else {
                            repo_active.clone()
                        }
                    },
                    Variant::Quiet,
                    move |_| {
                        if b.repository
                            .get_untracked()
                            .eq_ignore_ascii_case(&repo_toggle)
                        {
                            b.repository.set(String::new());
                        } else {
                            b.repository.set(repo_toggle.clone());
                        }
                        b.page.set(1);
                        b.list.set(json!({}));
                    },
                ));
            }
            row.child(choice(ui, b.state, state_options))
                .child(choice(ui, b.relation, relation_options))
                .into_element()
        }
    })
}

fn list_panel(ui: Ui) -> impl IntoElement {
    let b = ui.github;
    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(280.))
        .gap_2()
        .child(dynamic(move || {
            let key = format!(
                "github-list:{}:{}:{}:{}:{}:{}",
                b.tab.get(),
                b.query.get(),
                b.repository.get(),
                b.relation.get(),
                b.state.get(),
                b.page.get()
            );
            let position = drafts::scroll_position(ui, &key);
            scroll()
                .flex_1()
                .min_h(px(0.))
                .content_offset(position)
                .on_scroll(move |e: &ScrollEvent| drafts::capture_scroll(ui, key.clone(), e))
                .child(dynamic(move || {
                    let rows = b.list.get()["items"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    if rows.is_empty() {
                        if b.loading.get() {
                            return view()
                                .py_6()
                                .items_center()
                                .child(muted("Loading GitHub…"))
                                .into_element();
                        }
                        let gh = b.gh_status.get();
                        let gh_connected = gh["state"] == "connected";
                        let app_connected = ui.snapshot.with(|s| s.github.state == "connected");
                        let tab = b.tab.get();
                        let needs_gh = if tab == "notifications" {
                            !gh_connected
                        } else {
                            !gh_connected && !app_connected
                        };
                        if needs_gh {
                            let missing = gh["state"] == "missing";
                            return onboarding_card(
                                ui,
                                missing,
                                tab == "notifications" && app_connected,
                            )
                            .into_element();
                        }
                        return view()
                            .py_6()
                            .items_center()
                            .child(muted("No matching items. Adjust filters or click Refresh."))
                            .into_element();
                    }
                    view()
                        .children(
                            rows.into_iter()
                                .map(|row| list_row(ui, row))
                                .collect::<Vec<_>>(),
                        )
                        .into_element()
                }))
                .into_element()
        }))
        .child(
            view()
                .flex_shrink_0()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .pt_2()
                .border_t_1()
                .border_color(BORDER)
                .child(muted(move || {
                    if b.loading.get() {
                        "Refreshing…".into()
                    } else {
                        b.list.with(|v| {
                            v["validated_at"]
                                .as_u64()
                                .map(|t| {
                                    let secs = now().saturating_sub(t) / 1000;
                                    if secs < 5 {
                                        "Updated just now".into()
                                    } else if secs < 60 {
                                        format!("Updated {secs}s ago")
                                    } else {
                                        format!("Updated {}m ago", secs / 60)
                                    }
                                })
                                .unwrap_or_default()
                        })
                    }
                }))
                .child(view().flex_1())
                .child(
                    compact_action("Previous", Variant::Quiet, move |_| {
                        b.page.update(|p| *p = p.saturating_sub(1).max(1));
                        b.list.set(json!({}));
                    })
                    .disabled(move || b.page.get() <= 1 || b.loading.get()),
                )
                .child(muted(move || format!("Page {}", b.page.get())))
                .child(
                    compact_action("Next", Variant::Quiet, move |_| {
                        b.page.update(|p| *p += 1);
                        b.list.set(json!({}));
                    })
                    .disabled(move || b.list.with(|v| v["has_next"] != true) || b.loading.get()),
                ),
        )
}

fn fallback_web_url(item: &Value) -> Option<String> {
    if let Some(url) = item["html_url"]
        .as_str()
        .filter(|u| u.starts_with("https://github.com/"))
    {
        return Some(url.to_owned());
    }
    let repo = item["repository"]["full_name"].as_str()?;
    let subject_type = item["subject"]["type"].as_str().unwrap_or("");
    let suffix = match subject_type {
        "Release" => "/releases",
        "Discussion" => "/discussions",
        "CheckSuite" => "/actions",
        "RepositoryVulnerabilityAlert" | "RepositoryDependabotAlertsThread" => "/security",
        _ => "",
    };
    Some(format!("https://github.com/{repo}{suffix}"))
}

fn humanize_reason(reason: &str) -> &'static str {
    match reason {
        "review_requested" => "review requested",
        "mention" => "mentioned",
        "assign" => "assigned",
        "author" => "authored",
        "comment" => "commented",
        "ci_activity" => "CI activity",
        "state_change" => "state changed",
        "subscribed" => "subscribed",
        "team_mention" => "team mentioned",
        "approval_requested" => "approval requested",
        _ => "",
    }
}

fn item_status_badge(item: &Value, r: Option<&GitHubReference>) -> Option<(String, Tone)> {
    if item["merged"] == true || item["pull_request"]["merged_at"].is_string() {
        return Some(("Merged".into(), Tone::Info));
    }
    if item["draft"] == true {
        return Some(("Draft".into(), Tone::Neutral));
    }
    if let Some(state) = item["state"].as_str().filter(|s| !s.is_empty()) {
        return Some(match state {
            "open" => ("Open".into(), Tone::Success),
            "closed" => ("Closed".into(), Tone::Neutral),
            other => (other.to_owned(), Tone::Neutral),
        });
    }
    if let Some(r) = r {
        return Some(match r.kind.as_str() {
            "pr" => ("PR".into(), Tone::Info),
            "commit" => ("Commit".into(), Tone::Neutral),
            _ => ("Issue".into(), Tone::Success),
        });
    }
    item["subject"]["type"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| (s.to_owned(), Tone::Neutral))
}

fn list_row(ui: Ui, row: Value) -> AnyElement {
    let item = &row["item"];
    let r: Option<GitHubReference> = serde_json::from_value(row["reference"].clone()).ok();
    let title = item["title"]
        .as_str()
        .or(item["subject"]["title"].as_str())
        .unwrap_or("GitHub activity")
        .to_owned();
    let is_unread = item["unread"] == true;
    let subtitle_base = if let Some(r) = &r {
        let kind_name = match r.kind.as_str() {
            "pr" => "PR",
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
        format!("{} · {kind_name}{ident}", r.repository)
    } else {
        let repo = item["repository"]["full_name"].as_str().unwrap_or("GitHub");
        if let Some(kind) = item["subject"]["type"].as_str().filter(|s| !s.is_empty()) {
            format!("{repo} · {kind}")
        } else {
            repo.to_owned()
        }
    };

    let author = item["user"]["login"].as_str().unwrap_or("");
    let reason = item["reason"].as_str().map(humanize_reason).unwrap_or("");
    let raw_updated = item["updated_at"].as_str().unwrap_or("").to_owned();
    let age = if raw_updated.is_empty() {
        String::new()
    } else {
        super::format_timestamp_relative(&raw_updated)
    };
    let mut meta_parts = vec![subtitle_base];
    if !author.is_empty() {
        meta_parts.push(format!("@{author}"));
    } else if !reason.is_empty() {
        meta_parts.push(reason.to_owned());
    }
    if !age.is_empty() {
        meta_parts.push(age);
    }
    let meta_line = meta_parts.join(" · ");

    let badge_info = item_status_badge(item, r.as_ref());
    let labels: Vec<String> = item["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["name"].as_str().map(str::to_owned))
        .take(4)
        .collect();
    let thread_id = item["id"]
        .as_str()
        .filter(|_| item["subject"].is_object())
        .map(str::to_owned);
    let web_fallback = fallback_web_url(item);

    let open_ref = r.clone();
    let selected_ref = r.clone();
    let card_ref = r.clone();
    let fallback_title = title.clone();
    let b = ui.github;

    let mut left_body = view().flex_1().min_w(px(0.)).gap_1().items_start().child(
        view()
            .w_full()
            .min_w(px(0.))
            .flex_row()
            .items_center()
            .gap_2()
            .child(if is_unread {
                view()
                    .w(px(7.))
                    .h(px(7.))
                    .flex_shrink_0()
                    .rounded(px(99.))
                    .bg(ACCENT)
                    .into_element()
            } else {
                icon_tinted(Icon::GitHub, MUTED)
                    .w(px(13.))
                    .h(px(13.))
                    .flex_shrink_0()
                    .into_element()
            })
            .child(
                label(title.clone())
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .font_weight(if is_unread {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    }),
            )
            .children(
                badge_info
                    .into_iter()
                    .map(|(txt, tone)| status(txt, tone).into_element()),
            ),
    );

    let meta_el = if raw_updated.is_empty() {
        muted(meta_line).into_element()
    } else {
        muted(meta_line)
            .tooltip(nsh_base::tooltip().child(label(raw_updated)))
            .into_element()
    };
    left_body = left_body.child(meta_el);

    if !labels.is_empty() {
        left_body = left_body.child(
            view()
                .flex_row()
                .flex_wrap()
                .gap_1()
                .children(labels.into_iter().map(|l| badge(l).into_element())),
        );
    }

    let main_click = nsh_base::button()
        .flex_1()
        .min_w(px(0.))
        .py_2p5()
        .px_2p5()
        .items_start()
        .rounded(px(RADIUS))
        .bg(CLEAR)
        .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
        .accessibility_label(format!("Open GitHub item: {title}"))
        .on_press(move |_: &()| {
            if let Some(r) = &open_ref {
                open(ui, r.clone());
            } else if let Some(url) = &web_fallback {
                open_url(ui, url.clone());
            }
        })
        .child(left_body);

    let mut trailing = view()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .pr_2();

    if let Some(r_for_card) = card_ref {
        let title_for_add = fallback_title.clone();
        trailing = trailing.child(dynamic(move || {
            let cards = ui.snapshot.with(|s| super::matching_cards(s, &r_for_card));
            if let Some(first) = cards.first() {
                let card_id = first.id.clone();
                let btn_text = if cards.len() > 1 {
                    format!("Card #{} (+{}) ↗", first.number, cards.len() - 1)
                } else {
                    format!("Card #{} ↗", first.number)
                };
                compact_action(btn_text, Variant::Secondary, move |_| {
                    ui.open_card(card_id.clone())
                })
                .into_element()
            } else if r_for_card.kind != "commit" {
                let r_add = r_for_card.clone();
                let t_add = title_for_add.clone();
                compact_action("+ Card", Variant::Quiet, move |_| {
                    super::add_to_board(ui, r_add.clone(), t_add.clone())
                })
                .into_element()
            } else {
                view().into_element()
            }
        }));
    } else if let Some(url) = fallback_web_url(item) {
        trailing = trailing.child(compact_action("GitHub ↗", Variant::Quiet, move |_| {
            open_url(ui, url.clone())
        }));
    }

    if let Some(thread) = thread_id {
        if is_unread {
            let read_id = thread.clone();
            trailing = trailing.child(compact_action("Read", Variant::Quiet, move |_| {
                dispatch_then(
                    ui,
                    "github.notification.read",
                    json!({"thread_id": read_id}),
                    move |_| load(ui, false),
                )
            }));
        }
        let done_id = thread;
        trailing = trailing.child(compact_action("Done", Variant::Quiet, move |_| {
            dispatch_then(
                ui,
                "github.notification.done",
                json!({"thread_id": done_id}),
                move |_| load(ui, false),
            )
        }));
    }

    view()
        .w_full()
        .flex_row()
        .items_center()
        .gap_2()
        .border_b_1()
        .border_color(BORDER)
        .bg(move || {
            if selected_ref.is_some() && b.reference.get() == selected_ref {
                SELECTED.resolve()
            } else {
                CLEAR.resolve()
            }
        })
        .hover(|s| s.bg(CARD_HOVER))
        .child(main_click)
        .child(trailing)
        .into_element()
}

fn onboarding_card(ui: Ui, missing_binary: bool, notifications_only: bool) -> View {
    let title = if missing_binary {
        "Install GitHub CLI (`gh`) to connect GitHub"
    } else if notifications_only {
        "Sign in with GitHub CLI (`gh`) to read Notifications"
    } else {
        "Sign in with GitHub CLI (`gh`) to connect GitHub"
    };
    let desc = if missing_binary {
        "Canban uses your local `gh` CLI session to power Notifications, Issues, pull requests, and repository sync without copying tokens. Install `gh` (e.g. `brew install gh`), then sign in."
    } else if notifications_only {
        "GitHub Notifications require `gh` CLI authentication. Sign in once with `gh auth login` so Notifications, Issues, and PRs share the same session."
    } else {
        "One `gh auth login` session powers everything in Canban: Notifications, Issues, pull requests, inline reviews, and repository sync — no separate app sign-in needed."
    };

    let mut actions = view().flex_row().flex_wrap().items_center().gap_2();
    if missing_binary {
        actions = actions
            .child(action(
                "Install GitHub CLI ↗",
                Variant::Primary,
                move |_| {
                    let _ = std::process::Command::new("/usr/bin/open")
                        .arg("https://cli.github.com/")
                        .status();
                },
            ))
            .child(action(
                "Open local terminal",
                Variant::Secondary,
                move |_| crate::terminals::create(ui, ui.project.get_untracked(), None),
            ));
    } else {
        actions = actions.child(action(
            "Sign in with gh CLI →",
            Variant::Primary,
            move |_| launch_gh_login(ui),
        ));
    }
    actions = actions
        .child(action("Check status", Variant::Secondary, move |_| {
            check_cli(ui);
            load(ui, true);
        }))
        .child(action("Account & access", Variant::Quiet, move |_| {
            ui.navigate("github-account")
        }));

    view().py_8().items_center().child(
        panel()
            .max_w(px(560.))
            .gap_3()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(icon(Icon::GitHub))
                    .child(label(title).font_weight(FontWeight::SEMIBOLD)),
            )
            .child(muted(desc))
            .child(actions),
    )
}

fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        "Empty".into()
    } else if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

pub fn account(ui: Ui) -> impl IntoElement {
    view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_4()
        .child(
            scroll().flex_1().min_h(px(0.)).child(
                view()
                    .max_w(px(820.))
                    .gap_6()
                    .pb_6()
                    .child(dynamic(move || {
                        let app_auth = ui.snapshot.with(|s| s.github.clone());
                        let gh = ui.github.gh_status.get();
                        let app_login = app_auth
                            .login
                            .as_deref()
                            .filter(|_| app_auth.state == "connected");
                        let gh_login =
                            gh["login"].as_str().filter(|_| gh["state"] == "connected");
                        let mismatch = match (app_login, gh_login) {
                            (Some(a), Some(g)) => !a.eq_ignore_ascii_case(g),
                            _ => false,
                        };

                        let mut summary = view().gap_2();
                        if mismatch {
                            summary = summary.child(notice(
                                format!(
                                    "Account mismatch: GitHub App is signed in as @{} while gh CLI is signed in as @{}. Disconnect GitHub App or align both accounts so Notifications and repository reads match.",
                                    app_login.unwrap_or(""),
                                    gh_login.unwrap_or("")
                                ),
                                Tone::Attention,
                            ));
                        } else if let (Some(a), Some(_)) = (app_login, gh_login) {
                            summary = summary.child(
                                view()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .child(status(
                                        format!("Connected as @{a} · gh CLI + GitHub App"),
                                        Tone::Success,
                                    )),
                            );
                        } else if let Some(g) = gh_login {
                            summary = summary.child(
                                view()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .child(status(
                                        format!(
                                            "All GitHub features active via gh CLI (@{g}) · no extra sign-in required"
                                        ),
                                        Tone::Success,
                                    )),
                            );
                        } else if let Some(a) = app_login {
                            summary = summary.child(
                                view()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .child(status(
                                        format!("Issues, PRs & Sync: GitHub App (@{a})"),
                                        Tone::Success,
                                    ))
                                    .child(status(
                                        "Notifications: sign in with `gh` CLI",
                                        Tone::Attention,
                                    )),
                            );
                        }
                        summary.into_element()
                    }))
                    .child(
                        section_group("GitHub CLI (gh)")
                            .description(
                                "Primary GitHub connection. Powers Notifications, Issues, pull requests, inline reviews, and repository sync without copying tokens.",
                            )
                            .trailing(dynamic(move || {
                                ui.github.gh_status.with(|v| match v["state"].as_str() {
                                    Some("connected") => status(
                                        format!(
                                            "Connected · @{}",
                                            v["login"].as_str().unwrap_or("gh")
                                        ),
                                        Tone::Success,
                                    )
                                    .into_element(),
                                    Some("missing") => {
                                        status("Not installed", Tone::Attention).into_element()
                                    }
                                    Some("signed_out") => {
                                        status("Not signed in", Tone::Neutral).into_element()
                                    }
                                    _ => status("Not detected", Tone::Neutral).into_element(),
                                })
                            }))
                            .child(dynamic(move || {
                                let gh = ui.github.gh_status.get();
                                let state = gh["state"].as_str().unwrap_or("");
                                let mut body = view().gap_2();
                                if state == "connected" {
                                    let login = gh["login"].as_str().unwrap_or("gh");
                                    body = body
                                        .child(muted(format!(
                                            "Signed into GitHub CLI as @{login}. Canban invokes `gh` locally on demand — you do not need any separate GitHub login."
                                        )))
                                        .child(
                                            view()
                                                .flex_row()
                                                .flex_wrap()
                                                .items_center()
                                                .gap_2()
                                                .child(action(
                                                    "Check gh status",
                                                    Variant::Secondary,
                                                    move |_| check_cli(ui),
                                                ))
                                                .child(action(
                                                    "Configure Project repository",
                                                    Variant::Secondary,
                                                    move |_| ui.navigate("github-repository"),
                                                )),
                                        );
                                } else if state == "missing" {
                                    body = body
                                        .child(muted(
                                            "GitHub CLI (`gh`) was not found in `/opt/homebrew/bin`, `/usr/local/bin`, or `PATH`. Install `gh` (for example `brew install gh`), then sign in.",
                                        ))
                                        .child(
                                            view()
                                                .flex_row()
                                                .flex_wrap()
                                                .items_center()
                                                .gap_2()
                                                .child(action(
                                                    "Install GitHub CLI ↗",
                                                    Variant::Primary,
                                                    move |_| {
                                                        let _ = std::process::Command::new(
                                                            "/usr/bin/open",
                                                        )
                                                        .arg("https://cli.github.com/")
                                                        .status();
                                                    },
                                                ))
                                                .child(action(
                                                    "Open local terminal",
                                                    Variant::Secondary,
                                                    move |_| {
                                                        crate::terminals::create(
                                                            ui,
                                                            ui.project.get_untracked(),
                                                            None,
                                                        )
                                                    },
                                                ))
                                                .child(action(
                                                    "Check gh status",
                                                    Variant::Secondary,
                                                    move |_| check_cli(ui),
                                                )),
                                        );
                                } else {
                                    body = body
                                        .child(muted(
                                            "GitHub CLI (`gh`) is installed, or waiting to be checked. Sign in once with `gh auth login` to enable all GitHub features.",
                                        ))
                                        .child(
                                            view()
                                                .flex_row()
                                                .flex_wrap()
                                                .items_center()
                                                .gap_2()
                                                .child(action(
                                                    "Sign in with gh CLI →",
                                                    Variant::Primary,
                                                    move |_| launch_gh_login(ui),
                                                ))
                                                .child(action(
                                                    "Check gh status",
                                                    Variant::Secondary,
                                                    move |_| check_cli(ui),
                                                )),
                                        );
                                }
                                body.into_element()
                            })),
                    )
                    .child(
                        section_group("Local Cache")
                            .description(
                                "Fetched GitHub timelines, diffs, and notifications stay cached locally until refreshed or cleared.",
                            )
                            .trailing(muted(move || format_bytes(ui.github.cache_bytes.get())))
                            .child(
                                action("Clear GitHub cache", Variant::Secondary, move |_| {
                                    dispatch_then(ui, "github.cache.clear", json!({}), move |_| {
                                        ui.github.list.set(json!({}));
                                        ui.github.detail.set(json!({}));
                                        ui.github.overview.set(json!({}));
                                        ui.github.cache_bytes.set(0);
                                    })
                                })
                                .self_start(),
                            ),
                    )
                    .child(super::settings(ui)),
            ),
        )
}
