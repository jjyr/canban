//! Shared GitHub account status, Board filter state, and Account & Access subpage.
use super::*;

#[derive(Clone, Copy)]
pub struct Browser {
    pub epoch: Signal<u64>,
    pub gh_status: Signal<Value>,
    pub cache_bytes: Signal<u64>,
    pub filter_account: Signal<Option<String>>,
    pub filter_pending: Signal<std::collections::BTreeMap<String, Value>>,
    pub filter_saving: Signal<bool>,
    pub filters: Signal<std::collections::BTreeMap<String, BoardFilter>>,
}

pub use canban_types::BoardFilter;

impl Default for Browser {
    fn default() -> Self {
        Self::new()
    }
}

impl Browser {
    pub fn new() -> Self {
        Self {
            epoch: signal(0),
            gh_status: signal(json!({})),
            cache_bytes: signal(0),
            filter_account: signal(None),
            filter_pending: signal(Default::default()),
            filter_saving: signal(false),
            filters: signal(Default::default()),
        }
    }
}

pub fn init(ui: Ui) {
    if crate::backend::is_connected() {
        check_cli(ui);
    }
    effect(move || {
        let page = ui.page.get();
        if matches!(page.as_str(), "inbox" | "github-account") {
            check_cli(ui);
        }
        if page == "github-account" {
            check_cache(ui);
        }
    });
}

pub fn launch_gh_login(ui: Ui) {
    if !crate::backend::is_connected() {
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
    let Some(backend) = crate::backend::get() else {
        return;
    };
    ui.spawn(async move {
        let result = blocking(move || backend.call_local("github.cli.status", json!({}))).await;
        ui.github.gh_status.set(match result {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => json!({"state":"unavailable","message":format!("{e:#}")}),
            Err(e) => json!({"state":"unavailable","message":e.to_string()}),
        });
    });
}

pub fn check_cache(ui: Ui) {
    let Some(backend) = crate::backend::get() else {
        return;
    };
    ui.spawn(async move {
        if let Ok(Ok(v)) =
            blocking(move || backend.call_local("github.cache.status", json!({}))).await
            && let Some(bytes) = v["bytes"].as_u64()
        {
            ui.github.cache_bytes.set(bytes);
        }
    });
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
                        let app_auth = ui.github_account.get();
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
                                "Primary GitHub connection. Powers Inbox notifications, PR diffs, inline comments, and repository sync without copying tokens.",
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
                                    #[cfg(target_os = "macos")]
                                    let missing_msg = "GitHub CLI (`gh`) was not found in `/opt/homebrew/bin`, `/usr/local/bin`, or `PATH`. Install `gh` (for example `brew install gh`), then sign in.";
                                    #[cfg(not(target_os = "macos"))]
                                    let missing_msg = "GitHub CLI (`gh`) was not found in `~/.local/bin`, `/usr/local/bin`, `/usr/bin`, or `PATH`. Install `gh` from your package manager or cli.github.com, then sign in.";
                                    body = body
                                        .child(muted(missing_msg))
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
                                                        spawn(async {
                                                            let _ = blocking(|| {
                                                                crate::platform::open_external(
                                                                    "https://cli.github.com/",
                                                                )
                                                            })
                                                            .await;
                                                        });
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
                                        ui.inbox.github_notifications.set(Vec::new());
                                        ui.inbox.github_item_bodies.set(Default::default());
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
