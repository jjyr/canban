use super::*;
use std::rc::Rc;

const AVATAR: f32 = 28.;
const BADGE: f32 = 18.;
const SPINE_WIDTH: f32 = 2.;

/// Vertical spine continuity for a node in a connected Card timeline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spine {
    pub first: bool,
    pub last: bool,
}

impl Spine {
    pub const ISOLATED: Self = Self {
        first: true,
        last: true,
    };
}

/// Disclosure control attached to the anchor row of a collapsible milestone group.
#[derive(Clone)]
pub struct MilestoneFold {
    pub hidden_count: usize,
    pub expanded: bool,
    pub hidden_alert: bool,
    pub toggle: Rc<dyn Fn()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MilestoneStyle {
    Stage,
    Success,
    Attention,
    Danger,
    PullRequest,
    Archive,
    Neutral,
}

pub fn is_alert_kind(kind: &str) -> bool {
    matches!(
        milestone_style(kind),
        MilestoneStyle::Attention | MilestoneStyle::Danger
    )
}

fn milestone_style(kind: &str) -> MilestoneStyle {
    match kind {
        "card.stage_changed" | "card.restored" | "card.auto_merge_resumed" => MilestoneStyle::Stage,
        "review.merged" | "github.pr_merged" | "github.pr_delivered" | "workflow.approved" => {
            MilestoneStyle::Success
        }
        "execution.failed"
        | "execution.missing"
        | "review.merge_failed"
        | "review.upstream_sync_failed"
        | "workflow.script_failed" => MilestoneStyle::Danger,
        "workflow.waiting_for_human"
        | "execution.input_required"
        | "review.merge_conflict"
        | "github.pr_merge_attention"
        | "github.pr_not_found"
        | "github.pr_publish_unknown"
        | "scheduler.blocked"
        | "card.auto_merge_held" => MilestoneStyle::Attention,
        "github.pr_published" => MilestoneStyle::PullRequest,
        "card.archived" | "card.removed" => MilestoneStyle::Archive,
        _ => MilestoneStyle::Neutral,
    }
}

fn capitalize_stage_word(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {
            let mut out = String::with_capacity(word.len());
            out.push(first.to_ascii_uppercase());
            out.push_str(chars.as_str());
            out
        }
        _ => word.to_owned(),
    }
}

pub(crate) fn format_milestone_message(event: &Event) -> String {
    let raw = event.message.trim();
    if event.kind == "card.stage_changed"
        && let Some((lhs, rest)) = raw.split_once(" → ")
    {
        let (rhs, suffix) = match rest.find(" (") {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => (rest, ""),
        };
        return format!(
            "{} → {}{}",
            capitalize_stage_word(lhs.trim()),
            capitalize_stage_word(rhs.trim()),
            suffix
        );
    }
    if event.kind == "execution.input_required" {
        let trimmed = raw.trim_end_matches([' ', '—', '-']);
        if let Some(detail) = trimmed.strip_prefix("Agent input required: Agent needs input") {
            let detail = detail.trim_start_matches([' ', '—', '-', ':']).trim();
            return if detail.is_empty() {
                "Agent input required".into()
            } else {
                format!("Agent input required · {detail}")
            };
        }
        return trimmed.to_owned();
    }
    raw.to_owned()
}

pub fn timeline_item(ui: Ui, event: Event) -> impl IntoElement {
    let narrative = matches!(
        event.kind.as_str(),
        "comment.created" | "comment.replied" | "execution.reported"
    );
    render_connected(ui, event, narrative, Spine::ISOLATED, None, None)
}

pub fn render(ui: Ui, event: Event, narrative: bool) -> impl IntoElement {
    render_connected(ui, event, narrative, Spine::ISOLATED, None, None)
}

pub fn render_connected(
    ui: Ui,
    event: Event,
    narrative: bool,
    spine: Spine,
    fold: Option<MilestoneFold>,
    trailing_detail: Option<AnyElement>,
) -> impl IntoElement {
    if !narrative {
        return render_milestone(ui, event, spine, fold, trailing_detail).into_element();
    }
    render_narrative(ui, event, spine, trailing_detail).into_element()
}

fn milestone_badge(kind: &str) -> AnyElement {
    let style = milestone_style(kind);
    let circle = |bg_color: ThemeColor, border_color: ThemeColor| {
        view()
            .flex_shrink_0()
            .w(px(BADGE))
            .h(px(BADGE))
            .rounded(px(BADGE / 2.))
            .bg(bg_color)
            .border_1()
            .border_color(border_color)
            .items_center()
            .justify_center()
    };
    match style {
        MilestoneStyle::Success => circle(SUCCESS_SURFACE, SUCCESS)
            .child(icon_tinted(Icon::Check, SUCCESS).w(px(11.)).h(px(11.)))
            .into_element(),
        MilestoneStyle::Danger => circle(DANGER_SURFACE, DANGER)
            .child(
                text("!")
                    .text_size(px(10.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(DANGER),
            )
            .into_element(),
        MilestoneStyle::Attention => circle(ATTENTION_SURFACE, ATTENTION)
            .child(
                text("!")
                    .text_size(px(10.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(ATTENTION),
            )
            .into_element(),
        MilestoneStyle::PullRequest => circle(SELECTED, BORDER)
            .child(icon_tinted(Icon::Branch, ACCENT).w(px(11.)).h(px(11.)))
            .into_element(),
        MilestoneStyle::Archive => circle(INSET, BORDER)
            .child(icon_tinted(Icon::Archive, MUTED).w(px(10.5)).h(px(10.5)))
            .into_element(),
        MilestoneStyle::Stage => circle(INSET, BORDER)
            .child(view().w(px(6.)).h(px(6.)).rounded(px(3.)).bg(ACCENT))
            .into_element(),
        MilestoneStyle::Neutral => circle(INSET, BORDER)
            .child(view().w(px(5.)).h(px(5.)).rounded(px(2.5)).bg(MUTED))
            .into_element(),
    }
}

fn fold_pill(fold: MilestoneFold) -> impl IntoElement {
    let count = fold.hidden_count;
    let label_text = if fold.expanded {
        format!("Hide {count} earlier")
    } else {
        format!("+{count} earlier")
    };
    let tooltip_text = if fold.expanded {
        "Collapse intermediate status updates".to_owned()
    } else {
        format!(
            "Show {count} earlier status update{}",
            if count == 1 { "" } else { "s" }
        )
    };
    let on_press = fold.toggle;
    let mut pill = press_target()
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded(px(99.))
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .hover(|s| s.bg(CARD_HOVER))
        .active(|s| s.opacity(0.75))
        .focus(|s| s.border_color(ACCENT))
        .accessibility_label(tooltip_text.clone())
        .on_press(move |_: &()| on_press());
    if !fold.expanded && fold.hidden_alert {
        pill = pill.child(
            view()
                .w(px(5.))
                .h(px(5.))
                .rounded(px(2.5))
                .bg(ATTENTION)
                .flex_shrink_0(),
        );
    }
    let pill = pill
        .child(
            text(label_text)
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(MUTED),
        )
        .child(
            icon_tinted(
                if fold.expanded {
                    Icon::Collapse
                } else {
                    Icon::ChevronDown
                },
                MUTED,
            )
            .w(px(10.))
            .h(px(10.)),
        );
    view()
        .flex_shrink_0()
        .tooltip(hint_tooltip(tooltip_text))
        .child(pill)
}

fn render_milestone(
    ui: Ui,
    event: Event,
    spine: Spine,
    fold: Option<MilestoneFold>,
    trailing_detail: Option<AnyElement>,
) -> impl IntoElement {
    let top_pad: f32 = if spine.first { 2. } else { 6. };
    let bottom_pad: f32 = if spine.last { 2. } else { 6. };
    let style = milestone_style(&event.kind);
    let msg_color = match style {
        MilestoneStyle::Danger => DANGER,
        MilestoneStyle::Attention | MilestoneStyle::Success | MilestoneStyle::PullRequest => TEXT,
        MilestoneStyle::Stage | MilestoneStyle::Archive | MilestoneStyle::Neutral => MUTED,
    };
    let msg_weight = match style {
        MilestoneStyle::Danger | MilestoneStyle::Attention | MilestoneStyle::Success => {
            FontWeight::MEDIUM
        }
        _ => FontWeight::NORMAL,
    };

    let mut line = view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_1p5()
                .flex_1()
                .min_w(px(0.))
                .child(
                    text(format_milestone_message(&event))
                        .text_size(px(CAPTION_SIZE))
                        .font_weight(msg_weight)
                        .text_color(msg_color),
                )
                .children(fold.map(fold_pill)),
        );

    if let Some(terminal_id) = event.data["terminal_id"].as_str().map(str::to_owned)
        && matches!(style, MilestoneStyle::Danger | MilestoneStyle::Attention)
    {
        line = line.child(compact_action("Terminal ↗", Variant::Quiet, move |_| {
            crate::terminals::open_request(ui, &terminal_id)
        }));
    }
    line = line.child(
        muted(crate::components::activity::age(event.created_at))
            .text_size(px(11.5))
            .flex_shrink_0(),
    );

    let mut right = view()
        .flex_1()
        .min_w(px(0.))
        .pt(px(top_pad.max(4.) - 1.))
        .pb(px(bottom_pad))
        .gap_2()
        .child(line);
    if let Some(detail) = trailing_detail {
        right = right.child(detail);
    }

    let track = view()
        .w(px(AVATAR))
        .flex_shrink_0()
        .items_center()
        .child(
            view()
                .w(px(SPINE_WIDTH))
                .h(px(top_pad))
                .flex_shrink_0()
                .bg(if spine.first { CLEAR } else { BORDER }),
        )
        .child(milestone_badge(&event.kind))
        .child(
            view()
                .w(px(SPINE_WIDTH))
                .flex_1()
                .min_h(px(0.))
                .bg(if spine.last { CLEAR } else { BORDER }),
        );

    view()
        .flex_row()
        .items_stretch()
        .gap_2p5()
        .min_w(px(0.))
        .child(track)
        .child(right)
}

fn render_narrative(
    ui: Ui,
    event: Event,
    spine: Spine,
    trailing_detail: Option<AnyElement>,
) -> impl IntoElement {
    let top_pad = if spine.first { 0. } else { 8. };
    let bottom_pad = if spine.last { 0. } else { 8. };

    let card = event.card_id.clone();
    let report_path = event.data["path"].as_str().map(str::to_owned);
    let report_version = crate::presenter::event_body_version(&event);
    let cached_report = report_path
        .as_deref()
        .and_then(|p| ui.cached_report_body(&card, p, report_version));
    let body = signal(
        cached_report
            .clone()
            .unwrap_or_else(|| event.message.clone()),
    );
    let error = signal(String::new());
    let expanded = signal(false);
    let project_id = event.project_id.clone();
    let comment_id = event.id.clone();
    let is_own_comment = event.kind == "comment.created" && event.actor == "user";
    let was_edited = is_own_comment
        && (event.data["edited_at"].is_u64() || event.data["revision"].as_u64().unwrap_or(1) > 1);
    let can_edit = computed({
        let card_id = card.clone();
        move || {
            is_own_comment
                && ui
                    .index
                    .with(|idx| idx.cards.get(&card_id).is_some_and(|c| c.active()))
        }
    });
    let is_editing = computed({
        let cid = comment_id.clone();
        move || can_edit.get() && ui.editing_comment.get().as_deref() == Some(cid.as_str())
    });
    let is_deleting = computed({
        let cid = comment_id.clone();
        move || can_edit.get() && ui.deleting_comment.get().as_deref() == Some(cid.as_str())
    });
    let edit_draft = signal(event.message.clone());
    let edit_error = signal(String::new());
    let delete_error = signal(String::new());
    if cached_report.is_none()
        && let Some(path) = report_path
        && crate::APP.get().is_some()
    {
        let card = card.clone();
        ui.spawn(async move {
            let fetch_card = card.clone();
            let fetch_path = path.clone();
            match blocking(move || {
                crate::routed_call(
                    "resource.read",
                    json!({"card_id": fetch_card, "path": fetch_path}),
                    None,
                )
            })
            .await
            {
                Ok(Ok(value)) => {
                    if let Some(text) = value["body"].as_str().or(value["text"].as_str()) {
                        ui.store_report_body(card, path, report_version, text.to_owned());
                        body.try_set(text.into());
                    }
                }
                Ok(Err(e)) => {
                    error.try_set(format!("Report unavailable: {e:#}"));
                }
                Err(e) => {
                    error.try_set(e.to_string());
                }
            }
        });
    }
    let actor = event
        .actor
        .strip_prefix("agent:")
        .map(|a| format!("@{a}"))
        .unwrap_or_else(|| {
            if event.actor == "user" {
                github_login(ui).unwrap_or_else(|| "Me".into())
            } else {
                event.actor.clone()
            }
        });
    let face = avatar(ui, &event);
    let mut heading = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .px_3()
        .py_1()
        .bg(INSET)
        .child(label(actor).font_weight(FontWeight::MEDIUM))
        .child(muted(crate::components::activity::age(event.created_at)));
    if was_edited {
        heading = heading.child(muted("· edited").text_size(px(11.5)));
    }
    heading = heading.child(view().flex_1());
    if is_own_comment {
        let menu_cid = comment_id.clone();
        heading = heading.child(when(
            move || can_edit.get() && !is_editing.get() && !is_deleting.get(),
            move || {
                let cid = menu_cid.clone();
                let anchor = element_ref();
                canban_ui_kit::press_target()
                    .element_ref(&anchor)
                    .w(px(26.))
                    .h(px(24.))
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .bg(CLEAR)
                    .hover(|s| s.bg(CARD_HOVER))
                    .focus(|s| s.border_1().border_color(ACCENT))
                    .accessibility_label(|| Some("Comment actions".into()))
                    .child(icon_tinted(Icon::More, MUTED).w(px(15.)).h(px(15.)))
                    .on_press(move |_: &()| {
                        use nsh_core::element::desktop::{MenuItem, popup_menu_at};
                        let cid_edit = cid.clone();
                        let cid_del = cid.clone();
                        ui.spawn(async move {
                            let Some(bounds) = anchor.measure().await else {
                                return;
                            };
                            popup_menu_at(
                                vec![
                                    MenuItem::new("Edit comment").on_select(move || {
                                        let id = cid_edit.clone();
                                        batch(move || {
                                            edit_draft.set(body.get_untracked());
                                            edit_error.set(String::new());
                                            delete_error.set(String::new());
                                            ui.deleting_comment.set(None);
                                            ui.card_terminal_focused.set(false);
                                            ui.editing_comment.set(Some(id));
                                        });
                                    }),
                                    MenuItem::new("Delete comment…").on_select(move || {
                                        let id = cid_del.clone();
                                        batch(move || {
                                            delete_error.set(String::new());
                                            ui.editing_comment.set(None);
                                            ui.card_terminal_focused.set(false);
                                            ui.deleting_comment.set(Some(id));
                                        });
                                    }),
                                ],
                                point(bounds.page.x, bounds.page.y + bounds.frame.size.height),
                            )
                            .await;
                        });
                    })
            },
        ));
    }
    if let Some(terminal_id) = event.data["terminal_id"].as_str().map(str::to_owned) {
        let details = terminal_id.clone();
        heading = heading
            .child(compact_action("Details", Variant::Quiet, move |_| {
                ui.request.set(details.clone())
            }))
            .child(compact_action("Terminal ↗", Variant::Quiet, move |_| {
                crate::terminals::open_request(ui, &terminal_id)
            }));
    }
    let measured = signal(0f32);
    let save_card_id = card.clone();
    let save_comment_id = comment_id.clone();
    let edit_project_id = project_id;
    let message_card = view()
        .min_w(px(0.))
        .border_1()
        .border_color(move || {
            if is_deleting.get() {
                DANGER
            } else if is_editing.get() {
                ACCENT
            } else {
                BORDER
            }
        })
        .rounded(px(RADIUS))
        .overflow_hidden()
        .child(heading)
        .child(when(move || is_deleting.get(), {
            let del_card = save_card_id.clone();
            let del_cid = save_comment_id.clone();
            move || {
                let cancel_cid = del_cid.clone();
                let confirm_card = del_card.clone();
                let confirm_cid = del_cid.clone();
                view()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(DANGER_SURFACE)
                    .border_b_1()
                    .border_color(BORDER)
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                label("Delete this comment?")
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
                                            delete_error.set(String::new());
                                            if ui.deleting_comment.get_untracked().as_deref()
                                                == Some(cancel_cid.as_str())
                                            {
                                                ui.deleting_comment.set(None);
                                            }
                                        })
                                        .disabled(move || ui.is_method_pending("comment.delete")),
                                    )
                                    .child(
                                        compact_action("Delete", Variant::Danger, move |_| {
                                            delete_error.set(String::new());
                                            if crate::APP.get().is_none() {
                                                let card_id = confirm_card.clone();
                                                let cid = confirm_cid.clone();
                                                ui.snapshot.update(|s| {
                                                    if let Some(ev) =
                                                        s.events.iter_mut().find(|e| {
                                                            e.card_id == card_id && e.id == cid
                                                        })
                                                    {
                                                        ev.kind = "comment.deleted".into();
                                                        ev.message = "Comment deleted".into();
                                                        ev.data = json!({
                                                            "comment_id": cid,
                                                            "deleted_at": canban_types::now(),
                                                        });
                                                    }
                                                });
                                                ui.deleting_comment.set(None);
                                                ui.notify("Comment deleted");
                                                return;
                                            }
                                            let cid_done = confirm_cid.clone();
                                            dispatch_complete(
                                                ui,
                                                "comment.delete",
                                                json!({
                                                    "card_id": confirm_card,
                                                    "comment_id": confirm_cid,
                                                }),
                                                move |result| match result {
                                                    Ok(_) => {
                                                        delete_error.try_set(String::new());
                                                        if ui
                                                            .deleting_comment
                                                            .get_untracked()
                                                            .as_deref()
                                                            == Some(cid_done.as_str())
                                                        {
                                                            ui.deleting_comment.set(None);
                                                        }
                                                        ui.notify("Comment deleted");
                                                    }
                                                    Err(err) => {
                                                        delete_error.try_set(err);
                                                    }
                                                },
                                            );
                                        })
                                        .disabled(move || ui.is_method_pending("comment.delete")),
                                    ),
                            ),
                    )
                    .child(when(
                        move || !delete_error.get().is_empty(),
                        move || notice(move || delete_error.get(), Tone::Danger),
                    ))
            }
        }))
        .child(dynamic(move || {
            if is_editing.get() {
                let save_card = save_card_id.clone();
                let save_cid = save_comment_id.clone();
                let proj_id = edit_project_id.clone();
                let editor = canban_ui_kit::markdown::MarkdownInput::new();
                let height = signal(150.);
                let submit_ticket = signal(0u64);
                let seen_submit = stored(0u64);
                editor.on_submit(
                    move || {
                        is_editing.get()
                            && !ui.quick_open.get()
                            && ui.utility_popup.get().is_empty()
                            && !ui.native_modal.get()
                    },
                    move || submit_ticket.update(|n| *n += 1),
                );
                let can_save = computed(move || {
                    !ui.is_method_pending("comment.update")
                        && !edit_draft.with(|s| s.trim().is_empty())
                });
                effect({
                    let save_card = save_card.clone();
                    let save_cid = save_cid.clone();
                    move || {
                        let t = submit_ticket.get();
                        if seen_submit.replace(t) == t || !can_save.get_untracked() {
                            return;
                        }
                        let updated = edit_draft.get_untracked();
                        if updated == body.get_untracked() {
                            edit_error.set(String::new());
                            ui.editing_comment.set(None);
                            return;
                        }
                        if crate::APP.get().is_none() {
                            body.set(updated.clone());
                            let card_id = save_card.clone();
                            let cid = save_cid.clone();
                            ui.snapshot.update(|s| {
                                if let Some(ev) = s
                                    .events
                                    .iter_mut()
                                    .find(|e| e.card_id == card_id && e.id == cid)
                                {
                                    let rev = ev.data["revision"].as_u64().unwrap_or(1) + 1;
                                    ev.message = updated.clone();
                                    ev.data["body"] = json!(updated);
                                    ev.data["revision"] = json!(rev);
                                    ev.data["edited_at"] = json!(canban_types::now());
                                }
                            });
                            edit_error.set(String::new());
                            ui.editing_comment.set(None);
                            return;
                        }
                        let mentions =
                            crate::profiles::addressed(&updated, &ui.profiles.get_untracked());
                        edit_error.set(String::new());
                        let cid_done = save_cid.clone();
                        dispatch_complete(
                            ui,
                            "comment.update",
                            json!({
                                "card_id": save_card,
                                "comment_id": save_cid,
                                "body": updated,
                                "mentions": mentions,
                            }),
                            move |result| match result {
                                Ok(val) => {
                                    if let Some(new_msg) = val["message"].as_str() {
                                        body.try_set(new_msg.into());
                                    }
                                    edit_error.try_set(String::new());
                                    if ui.editing_comment.get_untracked().as_deref()
                                        == Some(cid_done.as_str())
                                    {
                                        ui.editing_comment.set(None);
                                    }
                                }
                                Err(err) => {
                                    edit_error.try_set(err);
                                }
                            },
                        );
                    }
                });
                let (source, reference_error) = crate::references::source(
                    ui,
                    editor,
                    edit_draft,
                    computed(move || proj_id.clone()),
                    "Edit comment",
                    "Edit comment… @Agent, #Card or #tag",
                );
                let accept_proj = edit_project_id.clone();
                let source = source.on_completion_accept(
                    move |ev: &nsh_base::editor::EditorCompletionAcceptEvent| {
                        let _ = crate::references::accept_tag(ui, &accept_proj, ev);
                    },
                );
                let focused_once = stored(false);
                let cancel_cid = save_comment_id.clone();
                return view()
                    .p_2p5()
                    .gap_2()
                    .on_layout(move |_: &Bounds<Points>| {
                        if !focused_once.replace(true) {
                            editor.focus();
                        }
                    })
                    .child(editor.render_with_preview(
                        edit_draft,
                        source,
                        false,
                        height,
                        move || crate::links::reader(ui, move || edit_draft.get()).into_element(),
                    ))
                    .child(when(
                        move || !reference_error.get().is_empty(),
                        move || notice(move || reference_error.get(), Tone::Attention),
                    ))
                    .child(when(
                        move || !edit_error.get().is_empty(),
                        move || notice(move || edit_error.get(), Tone::Danger),
                    ))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                compact_action("Cancel", Variant::Quiet, move |_| {
                                    edit_error.set(String::new());
                                    if ui.editing_comment.get_untracked().as_deref()
                                        == Some(cancel_cid.as_str())
                                    {
                                        ui.editing_comment.set(None);
                                    }
                                })
                                .disabled(move || ui.is_method_pending("comment.update")),
                            )
                            .child(
                                compact_action("Save", Variant::Primary, move |_| {
                                    submit_ticket.update(|n| *n += 1)
                                })
                                .accessibility_label(|| {
                                    Some(
                                        format!(
                                            "Save comment · {}",
                                            crate::commands::shortcuts::SUBMIT_DISPLAY
                                        )
                                        .into(),
                                    )
                                })
                                .disabled(move || !can_save.get()),
                            ),
                    )
                    .into_element();
            }

            view()
                .child(
                    view()
                        .p_3()
                        .max_h(move || px(if expanded.get() { f32::MAX } else { 280. }))
                        .overflow_hidden()
                        .child(
                            view()
                                .flex_shrink_0()
                                .on_layout(move |b: &Bounds<Points>| measured.set(b.size.height.0))
                                .child(crate::links::reader(ui, move || body.get())),
                        ),
                )
                .child(when(
                    move || measured.get() > 256.,
                    move || {
                        action(
                            move || {
                                if expanded.get() {
                                    "Show less"
                                } else {
                                    "Read full report"
                                }
                            },
                            Variant::Quiet,
                            move |_| expanded.update(|v| *v = !*v),
                        )
                    },
                ))
                .child(when(
                    move || !error.get().is_empty(),
                    move || notice(error.get(), Tone::Attention),
                ))
                .into_element()
        }));

    let mut right = view()
        .flex_1()
        .min_w(px(0.))
        .pt(px(top_pad))
        .pb(px(bottom_pad))
        .gap_2()
        .child(message_card);
    if let Some(detail) = trailing_detail {
        right = right.child(detail);
    }

    let mut track = view().w(px(AVATAR)).flex_shrink_0().items_center();
    if !spine.first {
        track = track.child(
            view()
                .w(px(SPINE_WIDTH))
                .h(px(top_pad))
                .flex_shrink_0()
                .bg(BORDER),
        );
    }
    track = track.child(face).child(
        view()
            .w(px(SPINE_WIDTH))
            .flex_1()
            .min_h(px(0.))
            .bg(if spine.last { CLEAR } else { BORDER }),
    );

    view()
        .flex_row()
        .items_stretch()
        .gap_2p5()
        .min_w(px(0.))
        .child(track)
        .child(right)
}

/// The signed-in GitHub account, when connected.
fn github_login(ui: Ui) -> Option<String> {
    ui.snapshot.with_untracked(|s| {
        s.github
            .login
            .clone()
            .filter(|l| s.github.state == "connected" && !l.is_empty())
    })
}

fn agent_avatar_icon(ui: Ui, event: &Event, profile: &str) -> Icon {
    if let Some(mark) = ui.snapshot.with_untracked(|s| {
        event.data["terminal_id"]
            .as_str()
            .and_then(|id| s.terminals.iter().find(|t| t.id == id))
            .and_then(|t| t.agent.as_ref())
            .map(|a| crate::profiles::provider(&a.profile).0)
    }) {
        return mark;
    }
    let found = ui
        .all_profiles
        .with_untracked(|all| all.iter().find(|p| p.id == profile).cloned())
        .or_else(|| {
            ui.profiles
                .with_untracked(|all| all.iter().find(|p| p.id == profile).cloned())
        });
    if let Some(p) = found {
        return ui.snapshot.with_untracked(|s| {
            let project = s.projects.iter().find(|proj| proj.id == event.project_id);
            crate::profiles::effective_provider(&p, project).0
        });
    }
    crate::profiles::agent_icon(profile)
}

/// Agents use their colored provider icon; the local user uses the GitHub avatar
/// when signed in ("ME" while signed out or before the image loads); other actors
/// keep two-letter initials.
fn avatar(ui: Ui, event: &Event) -> AnyElement {
    let frame = || {
        view()
            .relative()
            .flex_shrink_0()
            .w(px(AVATAR))
            .h(px(AVATAR))
            .rounded(px(AVATAR / 2.))
            .overflow_hidden()
            .bg(INSET)
            .border_1()
            .border_color(BORDER)
            .items_center()
            .justify_center()
    };
    let initials = |value: &str| {
        text(value.to_owned())
            .text_size(px(10.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(MUTED)
    };
    if let Some(profile) = event.actor.strip_prefix("agent:") {
        let mark = agent_avatar_icon(ui, event, profile);
        return frame()
            .tooltip(hint_tooltip(format!("@{profile}")))
            .child(icon_colored(mark, 16.))
            .into_element();
    }
    if event.actor == "user" {
        let mut face = frame().child(initials("ME"));
        if let Some(login) = github_login(ui) {
            face = face.child(
                image(format!("https://github.com/{login}.png?size=64"))
                    .absolute()
                    .top(px(0.))
                    .left(px(0.))
                    .w(px(AVATAR))
                    .h(px(AVATAR)),
            );
        }
        return face.into_element();
    }
    let short = event
        .actor
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    frame().child(initials(&short)).into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_core::{ApproxTextEngine, Core, geometry::size};

    fn sample_event(actor: &str, terminal_id: Option<&str>) -> Event {
        Event {
            id: "ev-1".into(),
            project_id: "proj-1".into(),
            card_id: "card-1".into(),
            sequence: 1,
            kind: "comment.replied".into(),
            actor: actor.into(),
            message: "Done".into(),
            data: match terminal_id {
                Some(id) => json!({"terminal_id": id}),
                None => json!({}),
            },
            created_at: 0,
        }
    }

    #[test]
    fn agent_avatar_icon_resolves_terminal_override_disabled_and_direct_engine() {
        #[component]
        fn ProbeRoot() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let mut cfg = Config::default();
            if let Some(claude) = cfg.profiles.iter_mut().find(|p| p.id == "claude") {
                claude.enabled = false;
            }
            ui.apply_config(&cfg);

            // Disabled profile in `all_profiles` still keeps its provider icon on historical comments.
            assert_eq!(
                agent_avatar_icon(ui, &sample_event("agent:claude", None), "claude"),
                Icon::Claude
            );

            // Identity profile (`@developer`) defaults to Codex, then respects Project `agent_overrides`.
            assert_eq!(
                agent_avatar_icon(ui, &sample_event("agent:developer", None), "developer"),
                Icon::Codex
            );
            let mut project = Project {
                github: None,
                detected_github: None,
                github_disconnected: false,
                delivery_mode: DeliveryMode::default(),
                agent_overrides: Default::default(),
                tags: Vec::new(),
                id: "proj-1".into(),
                name: "Canban".into(),
                directory: "/tmp/proj".into(),
                location: None,
                workflow: Workflow::default(),
                target_branch: None,
                created_at: 0,
            };
            project.agent_overrides.insert(
                "developer".into(),
                canban_types::ProjectAgentOverride::from_agent("agy"),
            );
            let mut snap = Snapshot::default();
            snap.projects.push(project);
            ui.snapshot.set(snap);
            assert_eq!(
                agent_avatar_icon(ui, &sample_event("agent:developer", None), "developer"),
                Icon::Agy
            );

            // Explicit Agent terminal snapshot takes precedence over later Project overrides.
            let mut gemini_profile = cfg
                .profiles
                .iter()
                .find(|p| p.id == "developer")
                .unwrap()
                .clone();
            gemini_profile.preset = Some("gemini".into());
            let att = canban_scheduler::agent_run(
                "proj-1",
                "card-1",
                1,
                "develop",
                "comment",
                gemini_profile,
                "Reply".into(),
                None,
            );
            ui.snapshot.update(|s| {
                s.terminals.push(TerminalEntry {
                    id: "card-1:developer".into(),
                    conversation_id: Some("card-1:developer".into()),
                    number: 1,
                    name: None,
                    origin_project_id: None,
                    origin_card_id: Some("card-1".into()),
                    agent_engine: None,
                    runtime: None,
                    agent: Some(att),
                    created_at: 0,
                });
            });
            assert_eq!(
                agent_avatar_icon(
                    ui,
                    &sample_event("agent:developer", Some("card-1:developer")),
                    "developer"
                ),
                Icon::Gemini
            );

            // Direct engine actor without a registered Profile entry resolves via `agent_icon`,
            // and unknown custom names fall back to `Icon::Agent`.
            let empty_ui = Ui::new();
            assert_eq!(
                agent_avatar_icon(empty_ui, &sample_event("agent:copilot", None), "copilot"),
                Icon::Copilot
            );
            assert_eq!(
                agent_avatar_icon(
                    empty_ui,
                    &sample_event("agent:custom-bot", None),
                    "custom-bot"
                ),
                Icon::Agent
            );

            view()
                .child(timeline_item(ui, sample_event("agent:developer", None)))
                .child(timeline_item(ui, sample_event("agent:claude", None)))
                .child(timeline_item(empty_ui, sample_event("agent:copilot", None)))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(800.0), px(600.0)));
        core.mount_root(&nsh_core::element::Root::of::<ProbeRoot>());
        let batch = core.commit().expect("initial mount batch");
        assert!(!batch.items.is_empty());
    }
}
