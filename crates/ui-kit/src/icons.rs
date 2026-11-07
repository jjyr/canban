//! Small, original line icons and a shared accessible 40-point navigation action.
use super::{label, tokens::*};
use nsh_core::{
    canvas::{LineCap, LineJoin},
    prelude::*,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Search,
    Projects,
    Board,
    Note,
    Agent,
    Overview,
    Archive,
    Settings,
    Connection,
    Plus,
}
fn line(mut p: Painter, points: &[(f32, f32)]) -> Painter {
    p = p.begin_path().move_to(points[0].0, points[0].1);
    for &(x, y) in &points[1..] {
        p = p.line_to(x, y);
    }
    p.stroke()
}
fn circle(p: Painter, x: f32, y: f32, r: f32) -> Painter {
    p.begin_path()
        .arc(x, y, r, 0., std::f32::consts::TAU)
        .stroke()
}
fn rect(p: Painter, x: f32, y: f32, w: f32, h: f32) -> Painter {
    line(p, &[(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)])
}
pub fn icon(kind: Icon) -> impl IntoElement {
    canvas(move || {
        let p = Painter::new()
            .stroke_color(color(TEXT))
            .line_width(1.6)
            .line_cap(LineCap::Round)
            .line_join(LineJoin::Round);
        match kind {
            Icon::Search => line(circle(p, 9., 9., 5.5), &[(13., 13.), (18., 18.)]),
            Icon::Projects => line(
                p,
                &[
                    (2., 6.),
                    (8., 6.),
                    (10., 8.),
                    (18., 8.),
                    (18., 17.),
                    (2., 17.),
                    (2., 6.),
                ],
            ),
            Icon::Board => {
                let p = rect(p, 2., 3., 16., 14.);
                let p = line(p, &[(7.3, 3.), (7.3, 17.)]);
                line(p, &[(12.7, 3.), (12.7, 17.)])
            }
            Icon::Note => {
                let p = rect(p, 4., 2., 12., 16.);
                let p = line(p, &[(7., 7.), (13., 7.)]);
                let p = line(p, &[(7., 10.), (13., 10.)]);
                line(p, &[(7., 13.), (11., 13.)])
            }
            Icon::Agent => {
                let p = rect(p, 3., 5., 14., 12.);
                let p = circle(p, 7., 10., 0.8);
                let p = circle(p, 13., 10., 0.8);
                let p = line(p, &[(7., 14.), (13., 14.)]);
                line(p, &[(10., 2.), (10., 5.)])
            }
            Icon::Overview => {
                let p = rect(p, 2., 2., 6., 6.);
                let p = rect(p, 12., 2., 6., 6.);
                let p = rect(p, 2., 12., 6., 6.);
                rect(p, 12., 12., 6., 6.)
            }
            Icon::Archive => {
                let p = rect(p, 2., 3., 16., 4.);
                let p = rect(p, 4., 7., 12., 11.);
                line(p, &[(8., 11.), (12., 11.)])
            }
            Icon::Settings => {
                let p = circle(p, 10., 10., 5.);
                let p = circle(p, 10., 10., 1.5);
                [
                    (10., 1., 10., 4.),
                    (10., 16., 10., 19.),
                    (1., 10., 4., 10.),
                    (16., 10., 19., 10.),
                    (3.5, 3.5, 5.7, 5.7),
                    (14.3, 14.3, 16.5, 16.5),
                    (3.5, 16.5, 5.7, 14.3),
                    (14.3, 5.7, 16.5, 3.5),
                ]
                .into_iter()
                .fold(p, |p, (x, y, a, b)| line(p, &[(x, y), (a, b)]))
            }
            Icon::Connection => {
                let p = line(p, &[(7., 3.), (7., 7.), (13., 7.), (13., 3.)]);
                let p = line(p, &[(5., 7.), (5., 11.), (10., 15.), (15., 11.), (15., 7.)]);
                line(p, &[(10., 15.), (10., 19.)])
            }
            Icon::Plus => {
                let p = line(p, &[(3., 10.), (17., 10.)]);
                line(p, &[(10., 3.), (10., 17.)])
            }
        }
    })
    .w(px(20.))
    .h(px(20.))
    .pointer_events(PointerEvents::None)
}
pub fn icon_action(
    kind: Icon,
    title: String,
    selected: impl Fn() -> bool + 'static,
    count: impl Fn() -> usize + 'static,
    press: impl Fn(&()) + 'static,
) -> impl IntoElement {
    let hint = title.clone();
    let selected = computed(selected);
    let count = computed(count);
    view()
        .w(px(40.))
        .h(px(40.))
        .tooltip(
            nsh_base::tooltip()
                .placement(Placement::Right)
                .px_3()
                .py_2()
                .rounded(px(RADIUS))
                .bg(SURFACE)
                .border_1()
                .border_color(BORDER)
                .child(label(hint)),
        )
        .child(
            nsh_base::button()
                .w(px(40.))
                .h(px(40.))
                .items_center()
                .justify_center()
                .relative()
                .rounded(px(RADIUS))
                .bg(move || if selected.get() { SELECTED } else { CLEAR })
                .border_1()
                .border_color(CLEAR)
                .hover(|s| s.bg(INSET))
                .focus(|s| s.border_color(ACCENT))
                .selected(move || selected.get())
                .accessibility_label(move || {
                    let n = count.get();
                    Some(if n > 0 {
                        format!("{title} · {n} need you").into()
                    } else {
                        title.clone().into()
                    })
                })
                .child(icon(kind))
                .child(dynamic(move || {
                    let n = count.get();
                    if n == 0 {
                        view().into_element()
                    } else {
                        view()
                            .absolute()
                            .top_0()
                            .right_0()
                            .min_w(px(16.))
                            .h(px(16.))
                            .px_1()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.))
                            .bg(ATTENTION_SURFACE)
                            .pointer_events(PointerEvents::None)
                            .child(
                                text(if n > 99 { "99+".into() } else { n.to_string() })
                                    .text_size(px(10.))
                                    .text_color(ATTENTION),
                            )
                            .into_element()
                    }
                }))
                .on_press(press),
        )
}
