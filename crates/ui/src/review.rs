//! Review is a separate route so Task/Comment drafts survive refreshes.
//! Contract: an immutable preview is accepted only with its revision and token.
//! Refresh is asynchronous; changing Card/target invalidates a pending preview.
use crate::presenter::*;
use canban_types::ReviewPreview;
use canban_ui_kit::*;
use nsh_core::prelude::*;
use serde_json::json;

pub fn diff_panel(p: ReviewPreview) -> impl IntoElement {
    // A longer fence keeps file content from becoming Markdown instructions.
    let fence = "`".repeat(
        p.patch
            .split(|c| c != '`')
            .map(str::len)
            .max()
            .unwrap_or(0)
            .max(2)
            + 1,
    );
    let code = format!("{fence}diff\n{}\n{fence}", p.patch);
    let mut body = panel()
        .child(label(format!(
            "{} → {} · {} files",
            &p.card_head[..p.card_head.len().min(10)],
            p.target_branch,
            p.files.len()
        )))
        .child(muted(p.files.join(" · ")));
    if let Some(reason) = p.reason {
        body = body.child(label(reason));
    }
    if p.truncated {
        body=body.child(label("Diff exceeds 2 MiB and is truncated. Inspect the complete checkout in the terminal before accepting."));
    }
    body.child(nsh_base::text_view().markdown(code))
}
fn refresh_diff(ui: Ui, target: Signal<String>) {
    let generation = ui.review_request.get_untracked() + 1;
    ui.review_request.set(generation);
    ui.review.set(None);
    ui.status.set("Loading Card diff…".into());
    let card = ui.card.get_untracked();
    let branch = target.get_untracked();
    let Some(app) = crate::APP.get().cloned() else {
        ui.status
            .set("Gallery preview: no AppServer connected".into());
        return;
    };
    ui.spawn(async move {
        let expected = card.clone();
        let expected_branch = branch.clone();
        let result = blocking(move || {
            app.call(
                "review.preview",
                json!({"card_id":card,"target_branch":branch}),
            )
        })
        .await;
        if ui.review_request.get_untracked() != generation
            || ui.page.get_untracked() != "review"
            || ui.card.get_untracked() != expected
            || target.try_get().as_deref() != Some(expected_branch.as_str())
        {
            return;
        }
        match result {
            Ok(Ok(value)) => match serde_json::from_value(value) {
                Ok(p) => {
                    ui.review.set(Some(p));
                    ui.status.set("Diff ready · refresh if files change".into());
                }
                Err(e) => ui
                    .operation_error
                    .set(format!("Invalid review preview: {e}")),
            },
            Ok(Err(e)) => ui
                .operation_error
                .set(format!("Could not load diff: {e:#}")),
            Err(e) => ui.operation_error.set(format!("Diff worker: {e}")),
        }
    });
}
pub fn screen(ui: Ui) -> impl IntoElement {
    let card = ui.current_card();
    let target_initial = ui
        .snapshot
        .with_untracked(|s| {
            card.as_ref()
                .and_then(|c| s.projects.iter().find(|p| p.id == c.project_id))
                .and_then(|p| p.target_branch.clone())
        })
        .unwrap_or_else(|| "main".into());
    let message_initial = card
        .as_ref()
        .map(|c| format!("Canban #{}: {}", c.number, c.title))
        .unwrap_or_default();
    let key = format!("review:{}", ui.card.get_untracked());
    let d = crate::drafts::document(
        ui,
        key,
        crate::drafts::Draft {
            title: target_initial,
            body: message_initial,
            ..Default::default()
        },
    );
    let target = signal(d.get_untracked().title);
    let message = signal(d.get_untracked().body);
    effect(move || {
        let title = target.get();
        let body = message.get();
        d.update(|d| {
            d.title = title;
            d.body = body;
        });
    });
    let mode = signal(
        if card
            .as_ref()
            .and_then(|c| c.delivery_mode.as_ref())
            .cloned()
            .unwrap_or_else(|| {
                ui.current_project()
                    .map(|p| p.delivery_mode)
                    .unwrap_or_default()
            })
            == canban_types::DeliveryMode::GitHubPr
        {
            "github_pr"
        } else {
            "local"
        }
        .to_owned(),
    );
    view().flex_1().min_h(px(0.0)).p_5().gap_3()
        .child(heading("Review and deliver"))
        .child(muted("Review the diff, then deliver through a local merge or a GitHub PR. Archive is separate."))
        .child(crate::github::pr_panel(ui))
        .child(view().flex_row().flex_wrap().gap_2()
            .child(action(if ui.return_overview.get_untracked() {"← Overview"} else {"← Board"},Variant::Quiet,move |_|ui.back_board()))
            .child(action("Return to Develop",Variant::Secondary,move |_|{if let Some(c)=ui.current_card(){let source=ui.location();dispatch_then(ui,"card.move",json!({"card_id":c.id,"revision":c.revision,"stage":"develop"}),move |_|{if ui.location()==source{ui.review.set(None);ui.page.set("card".into());}});}})))
        .child(panel().child(label("Delivery")).child(crate::github::choice(ui,mode,vec![("local","Local merge"),("github_pr","GitHub PR")])).child(label("Target branch")).child(field(target,"Existing local branch"))
            .child(action("Refresh diff",Variant::Secondary,move |_|refresh_diff(ui,target)))
            .child(label("Commit message")).child(field(message,"Commit message"))
            .child(action(move||if mode.get()=="github_pr"{"Publish · commit and push"}else{"Accept · commit and merge"},Variant::Primary,move |_|{
                if let Some(p)=ui.review.get_untracked() {
                    let source=ui.location();
                    dispatch_then(ui,if mode.get_untracked()=="github_pr"{"github.pr.publish"}else{"review.merge"},json!({"card_id":p.card_id,"revision":p.revision,"token":p.token,"target_branch":p.target_branch,"message":message.get_untracked()}),move |_|{if ui.location()==source{ui.review.set(None);}});
                }
            }).disabled(move ||ui.pending.get() || ui.review.get().is_none_or(|p|!p.merge_allowed || p.target_branch!=target.get() || ui.current_card().is_none_or(|c|c.revision!=p.revision || c.id!=p.card_id))))
            .child(dynamic(move ||ui.current_card().and_then(|c|c.delivery).map(|d|muted(format!("Last accepted delivery: {} · {}",d.target_branch,d.commit)).into_element()).unwrap_or_else(||view().into_element()))))
        .child(scroll().flex_1().child(dynamic(move || match ui.review.get() {
            Some(p)=>diff_panel(p).into_element(),
            None=>muted("Refresh the diff before accepting. Use the Card comment composer to ask an Agent or describe changes before returning to Develop.").into_element(),
        })))
}
