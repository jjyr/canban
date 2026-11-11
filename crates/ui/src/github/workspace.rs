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
            filters: signal(Default::default()),
            width: signal(480.),
        }
    }
}
pub fn open(ui: Ui, r: GitHubReference) {
    let b = ui.github;
    if b.reference.get_untracked().as_ref() != Some(&r) {
        b.reference.set(Some(r));
        b.section.set("overview".into());
        b.detail_page.set(1);
        b.detail.set(json!({}));
        b.overview.set(json!({}));
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
        ui.snapshot
            .with(|s| (s.github.state.clone(), s.github.login.clone()))
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
    effect(move || {
        if ui.page.get() == "github" && b.gh_status.get_untracked()["state"].is_null() {
            check_cli(ui)
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
pub fn load(ui: Ui, refresh: bool) {
    let b = ui.github;
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    let params = json!({"kind":b.tab.get_untracked(),"query":b.query.get_untracked(),"repository":b.repository.get_untracked(),"relation":b.relation.get_untracked(),"state":b.state.get_untracked(),"page":b.page.get_untracked(),"refresh":refresh});
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
pub fn screen(ui: Ui) -> impl IntoElement {
    let b = ui.github;
    view()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .p_4()
        .gap_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(icon(Icon::GitHub))
                .child(heading("GitHub"))
                .child(muted(move || {
                    ui.snapshot.with(|s| {
                        s.github
                            .login
                            .as_ref()
                            .map(|l| format!("{l} · App repository access"))
                            .unwrap_or_else(|| "GitHub CLI access".into())
                    })
                }))
                .child(view().flex_1())
                .child(action("Account & access", Variant::Secondary, move |_| {
                    ui.navigate("github-account")
                }))
                .child(
                    action("Refresh", Variant::Secondary, move |_| {
                        b.page.set(1);
                        load(ui, true);
                        if b.reference.get_untracked().is_some() {
                            super::detail::load(ui, true)
                        }
                    })
                    .disabled(move || b.loading.get() || b.detail_loading.get()),
                ),
        )
        .child(
            view().flex_row().gap_2().children(
                [
                    ("notifications", "Notifications"),
                    ("issues", "Issues"),
                    ("prs", "Pull requests"),
                ]
                .into_iter()
                .map(|(id, title)| {
                    action(title, Variant::Quiet, move |_| change_tab(ui, id))
                        .bg(move || {
                            if b.tab.get() == id {
                                SELECTED.resolve()
                            } else {
                                CLEAR.resolve()
                            }
                        })
                        .into_element()
                })
                .collect::<Vec<_>>(),
            ),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap_2()
                .child(
                    bound_field(
                        move || b.query.get(),
                        move |v| {
                            b.query.set(v.into());
                            b.page.set(1);
                            b.list.set(json!({}));
                        },
                        "Search GitHub…",
                    )
                    .key("github-query")
                    .disabled(move || b.tab.get() == "notifications"),
                )
                .child(
                    bound_field(
                        move || b.repository.get(),
                        move |v| {
                            b.repository.set(v.into());
                            b.page.set(1);
                            b.list.set(json!({}));
                        },
                        "owner/repository (optional)",
                    )
                    .key("github-repository-filter")
                    .disabled(move || b.tab.get() == "notifications"),
                )
                .child(dynamic(move || {
                    let options = if b.tab.get() == "notifications" {
                        vec![("inbox", "Inbox"), ("done", "Done")]
                    } else {
                        let mut options = vec![("open", "Open"), ("closed", "Closed")];
                        if b.tab.get() == "prs" {
                            options.push(("merged", "Merged"));
                        }
                        options.push(("all", "All states"));
                        options
                    };
                    choice(ui, b.state, options).into_element()
                }))
                .child(dynamic(move || {
                    let options = if b.tab.get() == "notifications" {
                        vec![
                            ("all", "All notifications"),
                            ("participating", "Participating"),
                        ]
                    } else if b.tab.get() == "prs" {
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
                    choice(ui, b.relation, options).into_element()
                })),
        )
        .child(when(
            move || !b.error.get().is_empty(),
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
                            280.,
                            340.,
                        )
                        .into_element()
                    }
                } else {
                    list_panel(ui).into_element()
                }
            })
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(muted(move || {
                    if b.loading.get() {
                        "Refreshing…".into()
                    } else {
                        b.list.with(|v| {
                            v["validated_at"]
                                .as_u64()
                                .map(|t| format!("Updated {}s ago", now().saturating_sub(t) / 1000))
                                .unwrap_or_default()
                        })
                    }
                }))
                .child(view().flex_1())
                .child(
                    action("Previous", Variant::Secondary, move |_| {
                        b.page.update(|p| *p = p.saturating_sub(1).max(1));
                        b.list.set(json!({}));
                    })
                    .disabled(move || b.page.get() <= 1 || b.loading.get()),
                )
                .child(label(move || format!("Page {}", b.page.get())))
                .child(
                    action("Next", Variant::Secondary, move |_| {
                        b.page.update(|p| *p += 1);
                        b.list.set(json!({}));
                    })
                    .disabled(move || b.list.with(|v| v["has_next"] != true) || b.loading.get()),
                ),
        )
}
fn list_panel(ui: Ui) -> impl IntoElement {
    let b = ui.github;
    dynamic(move || {
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
            .min_w(px(260.))
            .content_offset(position)
            .on_scroll(move |e: &ScrollEvent| drafts::capture_scroll(ui, key.clone(), e))
            .child(dynamic(move || {
                let rows = b.list.get()["items"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if rows.is_empty() {
                    return muted(if b.loading.get() {
                        "Loading GitHub…"
                    } else {
                        "No items. Change filters or refresh."
                    })
                    .into_element();
                }
                view()
                    .gap_1()
                    .children(
                        rows.into_iter()
                            .map(|row| list_row(ui, row))
                            .collect::<Vec<_>>(),
                    )
                    .into_element()
            }))
            .into_element()
    })
}
fn list_row(ui: Ui, row: Value) -> AnyElement {
    let item = &row["item"];
    let r: Option<GitHubReference> = serde_json::from_value(row["reference"].clone()).ok();
    let title = item["title"]
        .as_str()
        .or(item["subject"]["title"].as_str())
        .unwrap_or("GitHub activity")
        .to_owned();
    let subtitle = if let Some(r) = &r {
        format!(
            "{} · {}{}",
            r.repository,
            r.kind,
            r.number.map(|n| format!(" #{n}")).unwrap_or_default()
        )
    } else {
        item["repository"]["full_name"]
            .as_str()
            .unwrap_or("")
            .into()
    };
    let status = if item["unread"] == true {
        "Unread"
    } else if item["draft"] == true {
        "Draft"
    } else {
        item["state"].as_str().unwrap_or("")
    };
    let open_ref = r.clone();
    let selected_ref = r.clone();
    let b = ui.github;
    let mut result = nsh_base::button()
        .w_full()
        .p_3()
        .gap_1()
        .items_start()
        .rounded(px(RADIUS))
        .bg(move || {
            if selected_ref.is_some() && b.reference.get() == selected_ref {
                SELECTED.resolve()
            } else {
                SURFACE.resolve()
            }
        })
        .border_b_1()
        .border_color(BORDER)
        .hover(|s| s.bg(INSET))
        .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
        .accessibility_label(format!("Open GitHub item: {title}"))
        .on_press(move |_: &()| {
            if let Some(r) = &open_ref {
                open(ui, r.clone());
            }
        })
        .child(
            view()
                .flex_row()
                .gap_2()
                .child(icon(Icon::GitHub).w(px(14.)).h(px(14.)))
                .child(label(title))
                .child(badge(status.to_owned())),
        )
        .child(muted(subtitle));
    let author = item["user"]["login"].as_str().unwrap_or("");
    let updated = item["updated_at"].as_str().unwrap_or("");
    result = result.child(muted(format!("{author}  {updated}")));
    if let Some(labels) = item["labels"].as_array() {
        result = result.child(
            view().flex_row().flex_wrap().gap_1().children(
                labels
                    .iter()
                    .filter_map(|l| l["name"].as_str())
                    .take(5)
                    .map(|l| badge(l.to_owned()).into_element())
                    .collect::<Vec<_>>(),
            ),
        );
    }
    if let Some(thread) = item["id"].as_str().filter(|_| item["subject"].is_object()) {
        let read = thread.to_owned();
        let done = read.clone();
        return view()
            .border_b_1()
            .border_color(BORDER)
            .child(result)
            .child(
                view()
                    .flex_row()
                    .gap_1()
                    .child(action("Mark as read", Variant::Quiet, move |_| {
                        dispatch_then(
                            ui,
                            "github.notification.read",
                            json!({"thread_id":read}),
                            move |_| load(ui, false),
                        )
                    }))
                    .child(action("Done", Variant::Quiet, move |_| {
                        dispatch_then(
                            ui,
                            "github.notification.done",
                            json!({"thread_id":done}),
                            move |_| load(ui, false),
                        )
                    })),
            )
            .into_element();
    }
    result.into_element()
}
pub fn account(ui: Ui) -> impl IntoElement {
    view().flex_1().min_h(px(0.)).p_5().gap_3().child(view().flex_row().child(heading("GitHub account & access")).child(view().flex_1()).child(action("Back to GitHub",Variant::Quiet,move |_|ui.navigate("github"))))
        .child(scroll().flex_1().child(view().max_w(px(880.)).gap_4().child(super::settings(ui))
            .child(settings_section("GitHub CLI notifications","Use the account already signed in with gh. Canban does not copy its credentials.")
                .child(muted(move||ui.github.gh_status.with(|v|v["login"].as_str().map(|s|format!("GitHub CLI: {s}")).unwrap_or_else(||v["message"].as_str().unwrap_or("Run gh auth login, then check the connection.").into()))))
                .child(action("Check gh connection",Variant::Secondary,move |_|check_cli(ui))))
            .child(settings_section("Cache","Remote content stays available until a refresh updates it or cache cleanup removes it.")
                .child(action("Clear GitHub cache",Variant::Secondary,move |_|dispatch_then(ui,"github.cache.clear",json!({}),move |_|{ui.github.list.set(json!({}));ui.github.detail.set(json!({}));ui.github.overview.set(json!({}));}))))))
}
