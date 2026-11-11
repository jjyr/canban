use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Rules {
    intake: String,
    pr_intake: String,
    authors: String,
    labels: String,
    drafts: bool,
    paused: bool,
    delivery: String,
}
impl Default for Rules {
    fn default() -> Self {
        Self {
            intake: "all".into(),
            pr_intake: "off".into(),
            authors: String::new(),
            labels: String::new(),
            drafts: false,
            paused: false,
            delivery: "local".into(),
        }
    }
}
fn rules(d: Signal<Draft>) -> Rules {
    serde_json::from_str(&d.get().body).unwrap_or_default()
}
fn edit(d: Signal<Draft>, f: impl FnOnce(&mut Rules)) {
    let mut r = rules(d);
    f(&mut r);
    d.update(|d| d.body = serde_json::to_string(&r).unwrap());
}
fn enum_name<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .unwrap()
        .as_str()
        .unwrap_or("")
        .into()
}
pub fn settings(ui: Ui, id: String, name: String) -> impl IntoElement {
    let source = id.clone();
    let saved = computed(move || {
        ui.snapshot.with(|s| {
            s.projects
                .iter()
                .find(|p| p.id == source)
                .map(|p| {
                    let mut r = Rules {
                        delivery: enum_name(&p.delivery_mode),
                        ..Default::default()
                    };
                    let repo = if let Some(g) = &p.github {
                        r.intake = enum_name(&g.intake);
                        r.pr_intake = enum_name(&g.pr_intake);
                        r.authors = g.authors.join(", ");
                        r.labels = g.labels.join(", ");
                        r.drafts = g.include_drafts;
                        r.paused = g.paused;
                        g.repository.clone()
                    } else {
                        String::new()
                    };
                    (repo, r)
                })
                .unwrap_or_default()
        })
    });
    let initial = saved.get_untracked();
    let body = serde_json::to_string(&initial.1).unwrap();
    let d = drafts::document(
        ui,
        format!("github-repository:{id}"),
        Draft {
            title: initial.0.clone(),
            base_title: initial.0,
            body: body.clone(),
            base_body: body,
            ..Default::default()
        },
    );
    let baseline = signal(initial.1);
    effect(move || {
        let (repository, remote) = saved.get();
        let old = baseline.get_untracked();
        let mut current =
            serde_json::from_str::<Rules>(&d.get_untracked().body).unwrap_or_default();
        macro_rules! merge{($($field:ident),*)=>{$(if current.$field==old.$field{current.$field=remote.$field.clone();})*}}
        merge!(intake, pr_intake, authors, labels, drafts, paused, delivery);
        d.update(|d| {
            if d.title == d.base_title {
                d.title = repository.clone();
            }
            d.base_title = repository;
            d.body = serde_json::to_string(&current).unwrap();
        });
        baseline.set(remote);
    });
    let intake = signal(rules(d).intake);
    let pr = signal(rules(d).pr_intake);
    let delivery = signal(rules(d).delivery);
    effect(move || {
        let r = rules(d);
        if intake.get_untracked() != r.intake {
            intake.set(r.intake)
        }
        if pr.get_untracked() != r.pr_intake {
            pr.set(r.pr_intake)
        }
        if delivery.get_untracked() != r.delivery {
            delivery.set(r.delivery)
        }
    });
    effect(move || {
        let v = intake.get();
        untracked(|| edit(d, |r| r.intake = v));
    });
    effect(move || {
        let v = pr.get();
        untracked(|| edit(d, |r| r.pr_intake = v));
    });
    effect(move || {
        let v = delivery.get();
        untracked(|| edit(d, |r| r.delivery = v));
    });
    let save = id.clone();
    let sync = id.clone();
    let disconnect = id.clone();
    view().gap_3().key(format!("github-repository:{id}"))
        .child(label(format!("Project: {name}")))
        .child(bound_field(move||d.get().title,move|v|d.update(|d|d.title=v.into()),"owner/repository").key("github-repository"))
        .child(label("Automatically add Open Issues"))
        .child(choice(ui,intake,vec![("off","Off"),("all","All authors"),("collaborators","Repository collaborators")]))
        .child(label("Automatically add Open pull requests"))
        .child(choice(ui,pr,vec![("off","Off"),("all","All authors"),("collaborators","Repository collaborators"),("review_requested","Review requested from me"),("assigned","Assigned to me"),("authored","Created by me")]))
        .child(bound_field(move||rules(d).authors,move|v|edit(d,|r|r.authors=v.into()),"Authors: comma-separated logins, or empty for everyone").key("github-rule-authors"))
        .child(bound_field(move||rules(d).labels,move|v|edit(d,|r|r.labels=v.into()),"Labels: match any of these, or empty for all").key("github-rule-labels"))
        .child(action(move||if rules(d).drafts{"✓ Include Draft PRs"}else{"Include Draft PRs"},Variant::Quiet,move |_|edit(d,|r|r.drafts = !r.drafts)))
        .child(action(move||if rules(d).paused{"✓ Synchronization paused"}else{"Pause synchronization"},Variant::Quiet,move |_|edit(d,|r|r.paused = !r.paused)))
        .child(label("Default code delivery")).child(choice(ui,delivery,vec![("local","Local merge"),("github_pr","GitHub PR")]))
        .child(muted("New matches enter Backlog without starting an Agent. Changing rules keeps existing Cards."))
        .child(action("Save",Variant::Primary,move |_|{let r=rules(d);let split=|s:String|s.split(',').map(str::trim).filter(|s|!s.is_empty()).map(str::to_owned).collect::<Vec<_>>();dispatch(ui,"project.github",json!({"project_id":save,"repository":d.get_untracked().title.trim(),"intake":r.intake,"pr_intake":r.pr_intake,"authors":split(r.authors),"labels":split(r.labels),"include_drafts":r.drafts,"paused":r.paused,"delivery_mode":r.delivery}));}).disabled(move||ui.pending.get()||d.get().title.trim().is_empty()))
        .child(view().flex_row().items_center().gap_2().child(chrome_button(Icon::Refresh,"Sync now",move |_|dispatch(ui,"github.sync",json!({"project_id":sync})))).child(compact_action("Disconnect",Variant::Quiet,move |_|dispatch(ui,"github.source.disconnect",json!({"project_id":disconnect})))))
        .child(dynamic(move||{let g=ui.snapshot.with(|s|s.projects.iter().find(|p|p.id==id).and_then(|p|p.github.clone()));let mut area=view().gap_1();if let Some(g)=g{area=area.child(muted(g.synced_at.map(|t|format!("Last sync: {}s ago",now().saturating_sub(t)/1000)).unwrap_or_else(||"Not synced yet".into())));if let Some(e)=g.error{area=area.child(notice(e,Tone::Attention));}}area.into_element()}))
}
pub fn screen(ui: Ui) -> impl IntoElement {
    view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(chrome_button(Icon::Back, "Back to Board", move |_| {
                    ui.navigate("board")
                }))
                .child(heading("GitHub repository").flex_1()),
        )
        .child(scroll().flex_1().child(dynamic(move || {
            let _ = ui.project.get();
            if let Some(p) = ui.current_project() {
                settings(ui, p.id, p.name).into_element()
            } else {
                muted("Select or create a Project first.").into_element()
            }
        })))
}
