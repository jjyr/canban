//! Editable directory path + native folder chooser. Cancellation preserves input;
//! disposed fields ignore asynchronous answers. No product or storage knowledge.
use super::{Tone, Variant, action, field, notice};
use nsh_core::{file_dialog::OpenDialog, prelude::*};
use std::{path::Path, rc::Rc};
pub fn directory_field(
    value: Signal<String>,
    selected: impl Fn(&Path) + 'static,
) -> impl IntoElement {
    let picking = signal(false);
    let error = signal(String::new());
    let selected = Rc::new(selected);
    view()
        .min_w(px(0.))
        .gap_2()
        .child(directory_input(
            value,
            move || picking.get(),
            move |_| {
                picking.set(true);
                error.set(String::new());
                let mut dialog = OpenDialog::folders()
                    .title("Choose directory")
                    .button_label("Choose");
                let current = value.get_untracked();
                if let Some(parent) = Path::new(&current)
                    .ancestors()
                    .find(|p| p.is_absolute() && p.is_dir())
                {
                    dialog = dialog.directory(parent);
                }
                let selected = selected.clone();
                spawn(async move {
                    let result = pick_paths(dialog).await;
                    if !picking.try_set(false) {
                        return;
                    }
                    match result {
                        Ok(Some(paths)) => {
                            if let Some(path) = paths.first()
                                && value.try_set(path.to_string_lossy().into_owned())
                            {
                                selected(path);
                            }
                        }
                        Ok(None) => {}
                        Err(message) => {
                            error.try_set(message);
                        }
                    }
                });
            },
        ))
        .child(dynamic(move || {
            if error.get().is_empty() {
                view().into_element()
            } else {
                notice(error.get(), Tone::Danger).into_element()
            }
        }))
}

/// One path field and one Choose action; the caller supplies the browser behavior.
pub fn directory_input(
    value: Signal<String>,
    busy: impl Fn() -> bool + 'static,
    choose: impl Fn(&()) + 'static,
) -> View {
    view()
        .min_w(px(0.))
        .flex_row()
        .items_center()
        .gap_2()
        .child(
            field(value, "/absolute/path/to/project")
                .label("Directory")
                .flex_1()
                .min_w(px(0.)),
        )
        .child(
            action("Choose…", Variant::Secondary, choose)
                .flex_shrink_0()
                .disabled(busy),
        )
}
