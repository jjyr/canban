//! Inline path combobox: editable directory input with live anchored segment
//! autocomplete, fuzzy filtering, Tab drill-down and Enter/click selection.
//! No product, AppServer or storage dependencies.
use super::{
    Tone, field,
    icons::{Icon, icon, icon_tinted},
    label, muted, notice,
    tokens::*,
};
use nsh_core::{keymap::When, prelude::*};
use std::rc::Rc;

/// One directory candidate returned by a directory listing provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathEntry {
    pub name: String,
    pub path: String,
}

impl PathEntry {
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
        }
    }
}

/// Snapshot of the currently browsed base directory supplied by the caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathListing {
    /// Requested or resolved base directory path (`""` for default home).
    pub base_dir: String,
    /// Resolved home directory on the target host, if known (used to expand/compact `~`).
    pub home: String,
    /// Whether `base_dir` currently exists on disk.
    pub exists: bool,
    /// Immediate subdirectories of `base_dir`.
    pub entries: Vec<PathEntry>,
    /// Whether an asynchronous directory listing is in flight.
    pub loading: bool,
    /// Optional listing error message.
    pub error: String,
}

/// Parsed `(base_dir, segment_filter)` for a typed path query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathQuery {
    /// Directory to list (`""` means default/home directory).
    pub base_dir: String,
    /// Trailing segment after the last `/` used to filter `base_dir`'s subdirectories.
    pub segment_filter: String,
    /// Expanded absolute target path when the user typed a concrete path (`~` expanded if `home` is known).
    pub expanded_path: String,
}

/// Splits raw user input into `(base_dir, segment_filter, expanded_path)`.
pub fn parse_path_query(input: &str, home: &str) -> PathQuery {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == "~" {
        return PathQuery {
            base_dir: if home.is_empty() {
                String::new()
            } else {
                home.to_owned()
            },
            segment_filter: String::new(),
            expanded_path: if trimmed == "~" && !home.is_empty() {
                home.to_owned()
            } else {
                trimmed.to_owned()
            },
        };
    }

    let expanded = expand_tilde(trimmed, home);
    if !expanded.starts_with('/') && !expanded.starts_with("~/") {
        // Relative fragment before leading `/` or `~/`: search inside `home` (or default base_dir).
        return PathQuery {
            base_dir: home.to_owned(),
            segment_filter: expanded.clone(),
            expanded_path: if home.is_empty() {
                expanded
            } else if home == "/" {
                format!("/{expanded}")
            } else {
                format!("{home}/{expanded}")
            },
        };
    }

    if let Some((parent, segment)) = expanded.rsplit_once('/') {
        let base_dir = if parent.is_empty() {
            "/".to_owned()
        } else if parent == "~" && !home.is_empty() {
            home.to_owned()
        } else {
            parent.to_owned()
        };
        let clean_expanded = if expanded.len() > 1 {
            expanded.trim_end_matches('/').to_owned()
        } else {
            expanded.clone()
        };
        PathQuery {
            base_dir,
            segment_filter: segment.to_owned(),
            expanded_path: clean_expanded,
        }
    } else {
        PathQuery {
            base_dir: home.to_owned(),
            segment_filter: expanded.clone(),
            expanded_path: expanded,
        }
    }
}

/// Expands `~` or `~/...` when `home` is non-empty.
pub fn expand_tilde(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return path.to_owned();
    }
    if path == "~" {
        return if home.is_empty() {
            "/".to_owned()
        } else {
            home.to_owned()
        };
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return format!("{home}/{rest}");
    }
    path.to_owned()
}

/// Compacts a leading `home` prefix to `~` for display details.
pub fn compact_home(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    if !home.is_empty() {
        if path == home {
            return "~".to_owned();
        }
        if let Some(rest) = path.strip_prefix(home)
            && rest.starts_with('/')
        {
            return format!("~{rest}");
        }
    }
    path.to_owned()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathCandidateKind {
    /// Existing subdirectory inside `base_dir`.
    Directory,
    /// Select the current `base_dir` itself (when `segment_filter` is empty and `exists` is true).
    CurrentDirectory,
    /// Create/use a new directory path that does not yet exist on disk.
    NewDirectory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathCandidate {
    pub id: String,
    pub path: String,
    pub label: String,
    pub detail: String,
    pub kind: PathCandidateKind,
}

/// Builds ranked autocomplete rows for `input` against `listing`.
///
/// - Dot-prefixed hidden directories are omitted unless `segment_filter` starts with `.`.
/// - Non-empty `segment_filter` ranks directory names via `search::score_field` (exact -> prefix -> substring -> fuzzy).
/// - Appends a `NewDirectory` fallback when the typed path does not match an existing candidate,
///   so users can select or create new directories seamlessly.
pub fn filter_path_candidates(input: &str, listing: &PathListing) -> Vec<PathCandidate> {
    let query = parse_path_query(input, &listing.home);
    let show_hidden = query.segment_filter.starts_with('.');
    let filter_lower = query.segment_filter.to_lowercase();
    let base_display = compact_home(
        if listing.base_dir.is_empty() {
            &query.base_dir
        } else {
            &listing.base_dir
        },
        &listing.home,
    );

    let mut out = Vec::new();

    if query.segment_filter.is_empty() {
        if listing.exists && !query.expanded_path.is_empty() {
            let current = query.expanded_path.clone();
            out.push(PathCandidate {
                id: format!("current:{current}"),
                path: current.clone(),
                label: format!("Use \"{}\"", compact_home(&current, &listing.home)),
                detail: "Current directory".into(),
                kind: PathCandidateKind::CurrentDirectory,
            });
        }
        for entry in &listing.entries {
            if !show_hidden && entry.name.starts_with('.') {
                continue;
            }
            out.push(PathCandidate {
                id: format!("dir:{}", entry.path),
                path: entry.path.clone(),
                label: entry.name.clone(),
                detail: base_display.clone(),
                kind: PathCandidateKind::Directory,
            });
        }
    } else {
        let mut scored: Vec<(usize, usize, &PathEntry)> = listing
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| show_hidden || !e.name.starts_with('.'))
            .filter_map(|(idx, e)| {
                let name_lower = e.name.to_lowercase();
                super::search::score_field(&name_lower, &filter_lower).map(|s| (s, idx, e))
            })
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        for (_, _, entry) in scored {
            out.push(PathCandidate {
                id: format!("dir:{}", entry.path),
                path: entry.path.clone(),
                label: entry.name.clone(),
                detail: base_display.clone(),
                kind: PathCandidateKind::Directory,
            });
        }
    }

    let trimmed = input.trim();
    if !trimmed.is_empty() && !query.expanded_path.is_empty() {
        let target = query.expanded_path.clone();
        let exact_exists = out.iter().any(|c| c.path == target);
        if !exact_exists && (target.starts_with('/') || target.starts_with("~/")) {
            out.push(PathCandidate {
                id: format!("new:{target}"),
                path: target.clone(),
                label: format!("Use \"{}\"", compact_home(&target, &listing.home)),
                detail: "New directory".into(),
                kind: PathCandidateKind::NewDirectory,
            });
        }
    }

    out
}

/// Formats `path` with a single trailing `/` for drilling down into subdirectories via `Tab`.
pub fn drill_down_path(path: &str) -> String {
    if path == "/" {
        "/".to_owned()
    } else {
        format!("{}/", path.trim_end_matches('/'))
    }
}

/// Formats `path` as a finalized directory selection (without redundant trailing `/`).
pub fn finalize_path(path: &str) -> String {
    if path == "/" {
        "/".to_owned()
    } else {
        path.trim_end_matches('/').to_owned()
    }
}

/// Inline path combobox with live local directory filtering (for standalone / Gallery fixtures).
pub fn directory_field(
    value: Signal<String>,
    selected: impl Fn(&std::path::Path) + 'static,
) -> impl IntoElement {
    let home = std::env::var("HOME").unwrap_or_default();
    let listing = signal(PathListing {
        home: home.clone(),
        ..Default::default()
    });
    let selected = Rc::new(selected);
    path_combobox(
        value,
        move || listing.get(),
        move |base_dir| {
            let resolved_base = if base_dir.is_empty() {
                if home.is_empty() {
                    "/".to_owned()
                } else {
                    home.clone()
                }
            } else {
                expand_tilde(&base_dir, &home)
            };
            let p = std::path::Path::new(&resolved_base);
            let mut entries = Vec::new();
            let exists = p.is_dir();
            if exists && let Ok(rd) = std::fs::read_dir(p) {
                for item in rd.flatten() {
                    let path = item.path();
                    if path.is_dir()
                        && let Some(name) = item.file_name().to_str()
                    {
                        entries.push(PathEntry::new(name, path.to_string_lossy().into_owned()));
                    }
                }
                entries.sort_by(|a, b| a.name.cmp(&b.name));
            }
            listing.set(PathListing {
                base_dir: resolved_base,
                home: home.clone(),
                exists,
                entries,
                loading: false,
                error: String::new(),
            });
        },
        move |chosen| {
            selected(std::path::Path::new(&chosen));
        },
    )
}

/// Shared inline path combobox:
/// - Single editable text input (no external `Choose…` button).
/// - Focusing or typing opens an anchored dropdown matching the input's width and requests
///   `on_query_dir(base_dir)` whenever the parent directory segment changes.
/// - `↑` / `↓` navigate candidates while keeping typing/IME focus inside the text input.
/// - `Tab` drills down into the highlighted directory (appending `/` and refreshing subdirectories).
/// - `Enter` or clicking a candidate finalizes the directory selection and closes the dropdown.
/// - `Escape` or clicking outside dismisses the dropdown while preserving the typed path.
pub fn path_combobox(
    value: Signal<String>,
    listing: impl Fn() -> PathListing + 'static,
    on_query_dir: impl Fn(String) + 'static,
    on_select: impl Fn(String) + 'static,
) -> impl IntoElement {
    let open = signal(false);
    let focused = signal(false);
    let width = signal(None::<Points>);
    let highlighted = signal(None::<String>);
    let last_queried_base = stored(None::<String>);
    let anchor = element_ref();
    let input_ref = element_ref();
    let command_ref = component_ref::<nsh_base::Command>();

    let listing = Rc::new(listing);
    let on_query_dir = Rc::new(on_query_dir);
    let on_select = Rc::new(on_select);

    let listing_memo = {
        let listing = listing.clone();
        computed(move || listing())
    };

    let candidates = computed(move || {
        let current_input = value.get();
        let current_listing = listing_memo.get();
        filter_path_candidates(&current_input, &current_listing)
    });

    // Request directory listing whenever dropdown is open and `base_dir` changes.
    {
        let on_query_dir = on_query_dir.clone();
        effect(move || {
            let is_open = open.get();
            let raw = value.get();
            if !is_open {
                last_queried_base.replace(None);
                return;
            }
            let home = listing_memo.with_untracked(|l| l.home.clone());
            let base = parse_path_query(&raw, &home).base_dir;
            if last_queried_base.with(|prev| prev.as_ref() != Some(&base)) {
                last_queried_base.replace(Some(base.clone()));
                on_query_dir(base);
            }
        });
    }

    // Keep first candidate highlighted as filter results update.
    effect(move || {
        if !open.get() {
            return;
        }
        let rows = candidates.get();
        let current_hl = highlighted.get_untracked();
        let target = current_hl
            .as_deref()
            .and_then(|id| rows.iter().find(|r| r.id == id))
            .or_else(|| rows.first())
            .map(|r| r.id.clone());
        highlighted.set(target.clone());
        if let Some(id) = target
            && let Some(api) = command_ref.api::<nsh_base::CommandApi>()
        {
            api.highlight(id);
        }
    });

    let choose_final = {
        let on_select = on_select.clone();
        Rc::new(move |raw_path: String| {
            let clean = finalize_path(&raw_path);
            value.set(clean.clone());
            open.set(false);
            let u16_len = clean.encode_utf16().count() as u32;
            input_ref.set_selection(u16_len, u16_len);
            on_select(clean);
        })
    };

    let drill_into = Rc::new(move |raw_path: String| {
        let next = drill_down_path(&raw_path);
        last_queried_base.replace(None);
        value.set(next.clone());
        open.set(true);
        input_ref.focus();
        let u16_len = next.encode_utf16().count() as u32;
        input_ref.set_selection(u16_len, u16_len);
    });

    let active_candidate = move || -> Option<PathCandidate> {
        let rows = candidates.get_untracked();
        highlighted
            .get_untracked()
            .and_then(|id| rows.iter().find(|r| r.id == id).cloned())
            .or_else(|| rows.first().cloned())
    };

    // Keyboard bindings while input is focused.
    {
        let choose_final = choose_final.clone();
        let drill_into = drill_into.clone();
        effect(move || {
            if !focused.get() {
                return;
            }
            let is_open = open.get();
            if !is_open {
                for key in ["down", "up"] {
                    on_key_with(
                        key,
                        KeyOptions {
                            when: When::Always,
                            repeat: false,
                            ..Default::default()
                        },
                        move |_| {
                            last_queried_base.replace(None);
                            open.set(true);
                        },
                    );
                }
                return;
            }

            for (key, forward) in [("down", true), ("up", false)] {
                on_key_with(
                    key,
                    KeyOptions {
                        when: When::Always,
                        repeat: true,
                        ..Default::default()
                    },
                    move |_| {
                        let rows = candidates.get_untracked();
                        if rows.is_empty() {
                            return;
                        }
                        let current = highlighted
                            .get_untracked()
                            .and_then(|id| rows.iter().position(|r| r.id == id))
                            .unwrap_or(0);
                        let next = if forward {
                            (current + 1) % rows.len()
                        } else {
                            (current + rows.len() - 1) % rows.len()
                        };
                        let next_id = rows[next].id.clone();
                        highlighted.set(Some(next_id.clone()));
                        if let Some(api) = command_ref.api::<nsh_base::CommandApi>() {
                            api.highlight(next_id);
                        }
                    },
                );
            }

            {
                let drill_into = drill_into.clone();
                let choose_final = choose_final.clone();
                on_key_with(
                    "tab",
                    KeyOptions {
                        when: When::Always,
                        repeat: false,
                        ..Default::default()
                    },
                    move |_| {
                        if let Some(cand) = active_candidate() {
                            match cand.kind {
                                PathCandidateKind::Directory
                                | PathCandidateKind::CurrentDirectory => {
                                    drill_into(cand.path);
                                }
                                PathCandidateKind::NewDirectory => {
                                    choose_final(cand.path);
                                }
                            }
                        } else {
                            open.set(false);
                        }
                    },
                );
            }

            {
                let choose_final = choose_final.clone();
                on_key_with(
                    "enter",
                    KeyOptions {
                        when: When::Always,
                        repeat: false,
                        ..Default::default()
                    },
                    move |_| {
                        if let Some(cand) = active_candidate() {
                            choose_final(cand.path);
                        } else {
                            let typed = value.get_untracked();
                            let home = listing_memo.with_untracked(|l| l.home.clone());
                            let expanded = expand_tilde(typed.trim(), &home);
                            if !expanded.is_empty() {
                                choose_final(expanded);
                            } else {
                                open.set(false);
                            }
                        }
                    },
                );
            }

            on_key_with(
                "escape",
                KeyOptions {
                    when: When::Always,
                    repeat: false,
                    ..Default::default()
                },
                move |_| {
                    open.set(false);
                },
            );
        });
    }

    let submit_enter = {
        let choose_final = choose_final.clone();
        move |_: &str| {
            if open.get_untracked() {
                if let Some(cand) = active_candidate() {
                    choose_final(cand.path);
                } else {
                    open.set(false);
                }
            }
        }
    };

    view()
        .min_w(px(0.))
        .gap_2()
        .child(
            view()
                .element_ref(&anchor)
                .min_w(px(0.))
                .relative()
                .on_layout(move |frame: &Bounds<Points>| {
                    width.set(Some(frame.size.width));
                })
                .child(
                    field(value, "~/Workspace/my-project or /absolute/path")
                        .element_ref(&input_ref)
                        .font_family(MONO_FONT)
                        .pr_8()
                        .label("Directory")
                        .role(AccessibilityRole::Combobox)
                        .submit_behavior(SubmitBehavior::Submit)
                        .auto_correct(false)
                        .accessibility_state(move || AccessibilityState {
                            expanded: Some(open.get()),
                            has_popup: true,
                            ..AccessibilityState::default()
                        })
                        .on_focus(move || {
                            focused.set(true);
                            last_queried_base.replace(None);
                            open.set(true);
                        })
                        .on_blur(move || {
                            focused.set(false);
                        })
                        .on_press(move || {
                            if !open.get_untracked() {
                                last_queried_base.replace(None);
                                open.set(true);
                            }
                        })
                        .on_change(move |v: &str| {
                            value.set(v.to_owned());
                            if !open.get_untracked() {
                                open.set(true);
                            }
                        })
                        .on_submit(submit_enter),
                )
                .child(
                    view()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_2p5()
                        .items_center()
                        .justify_center()
                        .pointer_events(PointerEvents::None)
                        .child(icon(Icon::ChevronDown).w(px(13.)).h(px(13.))),
                ),
        )
        .child({
            let dropdown_open = computed(move || open.get() && width.with(Option::is_some));
            let presence = super::motion::Presence::new(dropdown_open);
            when(
                move || presence.mounted.get(),
                move || {
                    let choose_final = choose_final.clone();
                    overlay()
                        .anchor(&anchor)
                        .placement(Placement::Bottom)
                        .align(Align::Start)
                        .offset(px(4.))
                        .margin(px(8.))
                        .w(move || Length::from(width.get().unwrap_or(px(360.))))
                        .on_press_outside(move |_: &Point<Points>| {
                            open.set(false);
                        })
                        .on_cancel(move || {
                            open.set(false);
                        })
                        .child(
                            view()
                                .element_ref(&presence.region)
                                .bind_opacity(&presence.opacity)
                                .w_full()
                                .bg(SURFACE)
                                .rounded(px(10.))
                                .border_1()
                                .border_color(BORDER)
                                .shadow_lg()
                                .overflow_hidden()
                                .child(
                                nsh_base::command()
                                    .component_ref(&command_ref)
                                    .searchable(false)
                                    .filterable(false)
                                    .loading(move || {
                                        listing_memo.with(|l| l.loading && l.entries.is_empty())
                                    })
                                    .accessibility_label(Some("Directory suggestions".into()))
                                    .item_class("group")
                                    .list_class("px-1.5 py-1.5")
                                    .items(move || {
                                        candidates
                                            .get()
                                            .iter()
                                            .map(|c| {
                                                nsh_base::CommandItem::new(
                                                    c.id.clone(),
                                                    format!("{} · {}", c.label, c.detail),
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                    })
                                    .render_item(move |e: &nsh_base::CommandEntry| {
                                        let cand = candidates.with_untracked(|cs| {
                                            cs.iter().find(|c| c.id == e.id.as_str()).cloned()
                                        });
                                        cand.map(|cand| {
                                            let id = cand.id.clone();
                                            let row_icon = match cand.kind {
                                                PathCandidateKind::Directory => Icon::Folder,
                                                PathCandidateKind::CurrentDirectory => Icon::Check,
                                                PathCandidateKind::NewDirectory => Icon::Plus,
                                            };
                                            let is_new =
                                                cand.kind == PathCandidateKind::NewDirectory;
                                            view()
                                                .cursor_pointer()
                                                .rounded(px(6.))
                                                .px_2()
                                                .py_1p5()
                                                .bg(move || {
                                                    if highlighted.get().as_ref() == Some(&id) {
                                                        SELECTED
                                                    } else {
                                                        CLEAR
                                                    }
                                                })
                                                .flex_row()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    icon_tinted(
                                                        row_icon,
                                                        if is_new { ACCENT } else { MUTED },
                                                    )
                                                    .w(px(13.))
                                                    .h(px(13.)),
                                                )
                                                .child(
                                                    view()
                                                        .flex_1()
                                                        .min_w(px(0.))
                                                        .child(
                                                            label(cand.label)
                                                                .font_family(MONO_FONT)
                                                                .text_size(px(12.))
                                                                .truncate()
                                                                .min_w(px(0.)),
                                                        ),
                                                )
                                                .child(
                                                    muted(cand.detail)
                                                        .text_size(px(11.))
                                                        .flex_shrink_0(),
                                                )
                                                .into_element()
                                        })
                                        .unwrap_or_else(|| view().into_element())
                                    })
                                    .on_highlight_change(move |id: &Option<SharedString>| {
                                        highlighted.set(id.as_ref().map(ToString::to_string));
                                    })
                                    .on_select(move |id: &SharedString| {
                                        let chosen = candidates.with_untracked(|cs| {
                                            cs.iter()
                                                .find(|c| c.id == id.as_str())
                                                .map(|c| c.path.clone())
                                        });
                                        if let Some(path) = chosen {
                                            choose_final(path);
                                        }
                                    })
                                    .on_cancel(move || {
                                        open.set(false);
                                    })
                                    .empty(
                                        view()
                                            .p_3()
                                            .items_center()
                                            .child(
                                                muted(move || {
                                                    if listing_memo.with(|l| l.loading) {
                                                        "Loading directories…".to_owned()
                                                    } else {
                                                        "No matching subdirectories — type a full path to create one".to_owned()
                                                    }
                                                })
                                                .text_size(px(11.5)),
                                            ),
                                    )
                                    .h(move || {
                                        let rows = candidates.get();
                                        if rows.is_empty() {
                                            return px(44.);
                                        }
                                        let content_h = rows.len() as f32 * 31.0 + 16.0;
                                        px(content_h.min(264.))
                                    }),
                            )
                            .child(
                                view()
                                    .px_2p5()
                                    .py_1p5()
                                    .border_t_1()
                                    .border_color(BORDER)
                                    .bg(INSET)
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        muted(move || {
                                            if listing_memo.with(|l| l.loading) {
                                                "Loading directories…".to_owned()
                                            } else {
                                                "↑↓ navigate · Tab open subfolder · Enter select · Esc close"
                                                    .to_owned()
                                            }
                                        })
                                        .text_size(px(10.5)),
                                    ),
                            ),
                    )
                    .into_element()
            },
        )
        })
        .child(dynamic(move || {
            let err = listing_memo.with(|l| l.error.clone());
            if err.is_empty() {
                view().into_element()
            } else {
                notice(err, Tone::Attention).into_element()
            }
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_listing() -> PathListing {
        PathListing {
            base_dir: "/Users/jjy/Workspace".into(),
            home: "/Users/jjy".into(),
            exists: true,
            loading: false,
            error: String::new(),
            entries: vec![
                PathEntry::new(".git", "/Users/jjy/Workspace/.git"),
                PathEntry::new(".hidden-spike", "/Users/jjy/Workspace/.hidden-spike"),
                PathEntry::new("Machines", "/Users/jjy/Workspace/Machines"),
                PathEntry::new("canban", "/Users/jjy/Workspace/canban"),
                PathEntry::new("canban-site", "/Users/jjy/Workspace/canban-site"),
                PathEntry::new("sheprd", "/Users/jjy/Workspace/sheprd"),
            ],
        }
    }

    #[test]
    fn parse_path_query_splits_base_and_segment_with_tilde_expansion() {
        let q = parse_path_query("", "/Users/jjy");
        assert_eq!(q.base_dir, "/Users/jjy");
        assert_eq!(q.segment_filter, "");

        let q = parse_path_query("~/Work", "/Users/jjy");
        assert_eq!(q.base_dir, "/Users/jjy");
        assert_eq!(q.segment_filter, "Work");
        assert_eq!(q.expanded_path, "/Users/jjy/Work");

        let q = parse_path_query("~/Workspace/", "/Users/jjy");
        assert_eq!(q.base_dir, "/Users/jjy/Workspace");
        assert_eq!(q.segment_filter, "");
        assert_eq!(q.expanded_path, "/Users/jjy/Workspace");

        let q = parse_path_query("/Users/jjy/Workspace/can", "/Users/jjy");
        assert_eq!(q.base_dir, "/Users/jjy/Workspace");
        assert_eq!(q.segment_filter, "can");
        assert_eq!(q.expanded_path, "/Users/jjy/Workspace/can");

        let q = parse_path_query("/", "/Users/jjy");
        assert_eq!(q.base_dir, "/");
        assert_eq!(q.segment_filter, "");
        assert_eq!(q.expanded_path, "/");
    }

    #[test]
    fn filter_hides_dotfiles_by_default_and_reveals_when_segment_starts_with_dot() {
        let listing = fixture_listing();
        let normal = filter_path_candidates("~/Workspace/", &listing);
        assert!(normal.iter().all(|c| !c.label.starts_with('.')));
        assert_eq!(normal[0].kind, PathCandidateKind::CurrentDirectory);
        assert_eq!(normal[0].path, "/Users/jjy/Workspace");
        assert!(normal.iter().any(|c| c.label == "canban"));

        let dotfiles = filter_path_candidates("~/Workspace/.", &listing);
        assert!(dotfiles.iter().any(|c| c.label == ".git"));
        assert!(dotfiles.iter().any(|c| c.label == ".hidden-spike"));
    }

    #[test]
    fn filter_ranks_prefix_and_fuzzy_matches_and_offers_new_directory_fallback() {
        let listing = fixture_listing();
        let filtered = filter_path_candidates("~/Workspace/can", &listing);
        assert_eq!(filtered[0].path, "/Users/jjy/Workspace/canban");
        assert_eq!(filtered[1].path, "/Users/jjy/Workspace/canban-site");
        assert_eq!(filtered[2].kind, PathCandidateKind::NewDirectory);
        assert_eq!(filtered[2].path, "/Users/jjy/Workspace/can");

        // Exact match omits redundant "New directory" row.
        let exact = filter_path_candidates("~/Workspace/canban", &listing);
        assert_eq!(exact[0].path, "/Users/jjy/Workspace/canban");
        assert!(
            !exact
                .iter()
                .any(|c| c.kind == PathCandidateKind::NewDirectory)
        );
    }

    #[test]
    fn drill_down_and_finalize_preserve_root_and_normalize_trailing_slash() {
        assert_eq!(drill_down_path("/"), "/");
        assert_eq!(
            drill_down_path("/Users/jjy/Workspace"),
            "/Users/jjy/Workspace/"
        );
        assert_eq!(
            drill_down_path("/Users/jjy/Workspace/"),
            "/Users/jjy/Workspace/"
        );

        assert_eq!(finalize_path("/"), "/");
        assert_eq!(
            finalize_path("/Users/jjy/Workspace/"),
            "/Users/jjy/Workspace"
        );
        assert_eq!(
            finalize_path("/Users/jjy/Workspace"),
            "/Users/jjy/Workspace"
        );
    }

    #[test]
    fn path_combobox_mounts_single_combobox_input_without_external_choose_button() {
        use nsh_core::{ApproxTextEngine, Core, MountItem, geometry::size};

        #[component]
        fn ProbeRoot() -> impl IntoElement {
            crate::tokens::install_theme();
            let value = signal("/Users/jjy/Workspace/can".to_owned());
            let listing = fixture_listing();
            view().w(px(640.)).h(px(360.)).p_4().child(path_combobox(
                value,
                move || listing.clone(),
                |_| {},
                |_| {},
            ))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(640.0), px(360.0)));
        core.mount_root(&nsh_core::element::Root::of::<ProbeRoot>());
        let batch = core.commit().expect("initial mount batch");

        let mut combobox_count = 0usize;
        let mut button_labels = Vec::new();
        for item in &batch.items {
            if let MountItem::ViewProps { props, .. } = item {
                let a11y = &props.accessibility;
                if a11y.role == AccessibilityRole::Combobox {
                    combobox_count += 1;
                    assert_eq!(a11y.label.as_deref(), Some("Directory"));
                }
                if a11y.role == AccessibilityRole::Button {
                    button_labels.push(a11y.label.clone());
                }
            }
        }
        assert_eq!(combobox_count, 1, "expected exactly one Combobox input");
        assert!(
            button_labels.is_empty(),
            "expected no external Choose button on initial mount, got: {button_labels:?}"
        );
    }
}
