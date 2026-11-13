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
        | "review.upstream_sync_failed" => MilestoneStyle::Danger,
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

    if let Some(request) = event.request_id.clone()
        && matches!(style, MilestoneStyle::Danger | MilestoneStyle::Attention)
    {
        line = line.child(compact_action("Terminal ↗", Variant::Quiet, move |_| {
            crate::terminals::open_request(ui, &request)
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

    let body = signal(event.message.clone());
    let error = signal(String::new());
    let expanded = signal(false);
    let card = event.card_id.clone();
    if let Some(path) = event.data["path"].as_str().map(str::to_owned)
        && let Some(app) = crate::APP.get().cloned()
    {
        let card = card.clone();
        ui.spawn(async move {
            match blocking(move || app.call("resource.read", json!({"card_id":card,"path":path})))
                .await
            {
                Ok(Ok(value)) => {
                    if let Some(text) = value["body"].as_str().or(value["text"].as_str()) {
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
        .child(muted(crate::components::activity::age(event.created_at)))
        .child(view().flex_1());
    if let Some(request) = event.request_id.clone() {
        let details = request.clone();
        heading = heading
            .child(compact_action("Details", Variant::Quiet, move |_| {
                ui.request.set(details.clone())
            }))
            .child(compact_action("Terminal ↗", Variant::Quiet, move |_| {
                crate::terminals::open_request(ui, &request)
            }));
    }
    let measured = signal(0f32);
    let message_card = view()
        .min_w(px(0.))
        .border_1()
        .border_color(BORDER)
        .rounded(px(RADIUS))
        .overflow_hidden()
        .child(heading)
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
        ));

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

/// Agents use their provider icon; the local user uses the GitHub avatar when
/// signed in ("ME" while signed out or before the image loads); other actors
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
        let mark = ui
            .snapshot
            .with_untracked(|s| {
                event
                    .request_id
                    .as_ref()
                    .and_then(|id| s.requests.iter().find(|r| &r.id == id))
                    .map(|r| r.profile.clone())
            })
            .or_else(|| {
                ui.profiles
                    .with_untracked(|all| all.iter().find(|p| p.id == profile).cloned())
            })
            .map(|p| crate::profiles::provider(&p).0)
            .unwrap_or(Icon::Agent);
        return frame()
            .tooltip(hint_tooltip(format!("@{profile}")))
            .child(icon(mark).w(px(16.)).h(px(16.)))
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
