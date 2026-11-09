//! Named theme choice: saved selection, hover preview intent, native keyboard confirmation.
use super::{label, tokens::*};
use canban_themes::Theme;
use nsh_core::prelude::*;

pub fn theme_choice(
    theme: &'static Theme,
    selected: impl Fn() -> bool + 'static,
    preview: impl Fn(bool) + Copy + 'static,
    disabled: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> View {
    let selected = computed(selected);
    view()
        .w(Percentage(0.48))
        .on_mouse_enter(move |_: &MouseEvent| preview(true))
        .on_mouse_leave(move |_: &MouseEvent| preview(false))
        .child(
            nsh_base::button()
                .w_full()
                .disabled(disabled)
                .min_h(px(60.))
                .p_3()
                .gap_2()
                .rounded(px(RADIUS))
                .bg(move || if selected.get() { SELECTED } else { INSET })
                .border_1()
                .border_color(move || if selected.get() { ACCENT } else { BORDER })
                .hover(|s| s.bg(SURFACE))
                .focus(|s| s.border_color(ACCENT))
                .selected(move || selected.get())
                .accessibility_label(move || Some(theme.name.into()))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(label(theme.name).flex_1())
                        .child(
                            text(move || if selected.get() { "✓" } else { "" }).text_color(ACCENT),
                        ),
                )
                .child(
                    view().flex_row().gap_1().children(
                        [
                            theme.background,
                            theme.foreground,
                            theme.ansi[1],
                            theme.ansi[2],
                            theme.ansi[3],
                            theme.ansi[4],
                            theme.ansi[5],
                            theme.ansi[6],
                        ]
                        .into_iter()
                        .map(|color| {
                            view()
                                .flex_1()
                                .h(px(6.))
                                .rounded(px(2.))
                                .bg(rgb(color))
                                .into_element()
                        })
                        .collect::<Vec<_>>(),
                    ),
                )
                .on_press(press),
        )
}
