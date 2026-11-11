//! Shared Markdown reading and source editing. NSH owns the rope, selections,
//! undo stack and caret completion; changing height/preview never remounts it.
use super::{Variant, action, tokens::*};
use nsh_base::{Editor, EditorApi, EditorCommand, EditorElement, TextViewElement};
use nsh_core::prelude::*;

pub fn reader(body: impl Fn() -> String + 'static) -> TextViewElement {
    nsh_base::text_view()
        .markdown(move || Some(body().into()))
        .text_size(px(13.))
        .text_color(TEXT)
        .min_w(px(0.))
}

#[derive(Clone, Copy)]
pub struct MarkdownInput {
    pub editor: ComponentRef<Editor>,
    pub active: Signal<bool>,
    pub focused: Signal<bool>,
    pub held: Signal<bool>,
    pub focus_epoch: Signal<u64>,
    preview: Signal<bool>,
    keys_epoch: Signal<u64>,
}
impl Default for MarkdownInput {
    fn default() -> Self {
        Self::new()
    }
}
impl MarkdownInput {
    pub fn new() -> Self {
        Self {
            editor: component_ref(),
            active: signal(false),
            focused: signal(false),
            held: signal(false),
            focus_epoch: signal(0),
            preview: signal(false),
            keys_epoch: signal(0),
        }
    }
    /// Expansion changes space without switching the user's Write/Preview mode.
    pub fn restore_focus(self) {
        if !self.preview.get_untracked() {
            self.focus();
        }
    }
    pub fn focus(self) {
        self.active.set(true);
        self.preview.set(false);
        if let Some(api) = self.editor.api::<EditorApi>() {
            api.focus();
        }
    }
    /// Restore a focused editor after a host overlay closes. NSH's editor sink
    /// rejects pointer targeting, so native modal restoration can skip it.
    /// Navigating away or selecting another document must never reclaim focus.
    pub fn restore_after(self, covered: Memo<bool>, current: Memo<bool>) {
        let was_covered = stored(false);
        let restore = stored(false);
        effect(move || {
            let covered_now = covered.get();
            if covered_now {
                if !was_covered.replace(true) {
                    restore.replace(self.focused.get_untracked());
                }
            } else if was_covered.replace(false) && restore.replace(false) {
                spawn(async move {
                    next_frame().await;
                    if covered.try_get() == Some(false) && current.try_get() == Some(true) {
                        self.focus();
                    }
                });
            }
        });
    }
    /// Bind after the native editor has installed its code-editing keys.
    /// Every native focus acquisition (including returning from a system sheet)
    /// reclaims Cmd+Enter for the host's single submit intent. The timer and
    /// binding belong to this effect and disappear on blur, hold or disposal.
    pub fn on_submit(self, allowed: impl Fn() -> bool + 'static, submit: impl Fn() + 'static) {
        let submit = std::rc::Rc::new(submit);
        effect(move || {
            let _ = self.focus_epoch.get();
            let _ = self.keys_epoch.get();
            if !self.active.get() || self.held.get() || !allowed() {
                return;
            }
            let submit = submit.clone();
            set_timeout(std::time::Duration::ZERO, move || {
                for key in ["enter", "shift+enter"] {
                    on_key_with(
                        key,
                        KeyOptions {
                            when: nsh_core::keymap::When::Always,
                            repeat: true,
                            ..Default::default()
                        },
                        move |_| {
                            if !self.preview.get_untracked()
                                && let Some(api) = self.editor.api::<EditorApi>()
                            {
                                api.hide_completions();
                                api.run(EditorCommand::Newline);
                            }
                        },
                    );
                }
                on_key_with(
                    "mod+enter",
                    KeyOptions {
                        when: nsh_core::keymap::When::Always,
                        ..Default::default()
                    },
                    move |_| {
                        if let Some(api) = self.editor.api::<EditorApi>() {
                            api.hide_completions();
                        }
                        submit();
                    },
                );
            });
        });
    }
    /// Answers retain NSH's caret filtering, Tab/click and stale-request checks.
    /// Reclaim Enter after NSH installs its completion-menu bindings.
    pub fn completions(
        self,
        id: u32,
        items: Vec<nsh_base::editor::EditorCompletionItem>,
        incomplete: bool,
    ) {
        if let Some(api) = self.editor.api::<EditorApi>() {
            api.present_completions(id, items, Some(incomplete));
            self.keys_epoch.update(|n| *n += 1);
        }
    }
    /// NSH Editor's native sink currently consumes text only. Read file/image
    /// payloads on the explicit Paste shortcut through NSH's clipboard service;
    /// leave plain text as one normal editor insertion/undo step.
    pub fn on_paste_files(
        self,
        receive: impl Fn(Vec<nsh_core::PastedFile>, Vec<String>) + 'static,
    ) {
        let receive = std::rc::Rc::new(receive);
        effect(move || {
            if !self.active.get() || self.held.get() || self.preview.get() {
                return;
            }
            let receive = receive.clone();
            on_key_with(
                "mod+v",
                KeyOptions {
                    when: nsh_core::keymap::When::Always,
                    ..Default::default()
                },
                move |_| {
                    let receive = receive.clone();
                    let files = clipboard_files();
                    let image = clipboard_image();
                    let text = clipboard_string();
                    let api = self.editor.api::<EditorApi>();
                    let original = api.as_ref().map(|a| (a.value(), a.selections()));
                    spawn(async move {
                        let files = files.await;
                        let image = image.await;
                        let text = text.await;
                        if self.active.try_get() != Some(true) {
                            return;
                        }
                        let Some(api) = self.editor.api::<EditorApi>() else {
                            return;
                        };
                        if original
                            .as_ref()
                            .is_none_or(|(v, s)| *v != api.value() || *s != api.selections())
                        {
                            return;
                        }
                        if !files.is_empty() {
                            receive(Vec::new(), files);
                        } else if let Some(image) = image {
                            if let Some((mime, data)) =
                                nsh_core::platform::clipboard::decode_data_uri(&image.url)
                            {
                                let mut file = nsh_core::PastedFile::image(data);
                                file.mime = mime.into();
                                receive(vec![file], Vec::new());
                            }
                        } else {
                            api.insert(text);
                        }
                    });
                },
            );
        });
    }
    pub fn blur(self) {
        self.preview.set(false);
        self.active.set(false);
        if let Some(api) = self.editor.api::<EditorApi>() {
            api.blur();
        }
    }
    pub fn source(self, value: Signal<String>, name: &str, placeholder: &str) -> EditorElement {
        nsh_base::editor()
            .value(move || Some(value.get().into()))
            .on_change(move |v: &SharedString| value.set(v.to_string()))
            .on_focus(move |_: &()| {
                self.active.set(true);
                self.focused.set(true);
                self.focus_epoch.update(|n| *n += 1);
            })
            .on_blur(move |_: &()| self.focused.set(false))
            .accessibility_label(name.to_owned())
            .placeholder(placeholder.to_owned())
            .line_numbers(false)
            .folding(false)
            .soft_wrap(true)
            .active_line(false)
            .indent_guides(false)
            .auto_close(false)
            .auto_indent(false)
            .read_only(move || self.preview.get())
            .font_size(px(13.))
            .line_height(px(20.))
            .component_ref(&self.editor)
            .size_full()
    }
    pub fn render(
        self,
        value: Signal<String>,
        source: impl IntoElement,
        compact: bool,
        max_height: Signal<f32>,
    ) -> View {
        self.render_with_preview(value, source, compact, max_height, move || {
            reader(move || value.get()).into_element()
        })
    }
    pub fn render_with_preview(
        self,
        value: Signal<String>,
        source: impl IntoElement,
        compact: bool,
        max_height: Signal<f32>,
        preview: impl Fn() -> AnyElement + 'static,
    ) -> View {
        let preview = std::rc::Rc::new(preview);
        let root = element_ref();
        root.on_focus_in(move || self.active.set(true));
        root.on_focus_out(move || {
            if !self.held.get_untracked() {
                self.active.set(false);
            }
        });
        let expanded = computed(move || {
            !compact || self.active.get() || self.held.get() || value.with(|v| !v.is_empty())
        });
        let height = computed(move || {
            if !expanded.get() {
                return 48.;
            }
            if !compact {
                return max_height.get().max(120.);
            }
            let lines = value.with(|v| v.lines().count());
            (100. + lines.saturating_sub(3) as f32 * 20.).min(max_height.get().max(64.))
        });
        let previous_size = stored(None::<(bool, f32)>);
        effect(move || {
            let next = (expanded.get(), height.get());
            if let Some(before) = previous_size.replace(Some(next))
                && (before.0 != next.0 || (before.1 - next.1).abs() > 0.5)
            {
                super::motion::layout(root, super::motion::COMPOSER);
            }
        });
        view()
            .element_ref(&root)
            .min_w(px(0.))
            .rounded(px(RADIUS))
            .border_1()
            .border_color(BORDER)
            .bg(INSET)
            .child(
                view()
                    .h(move || px(if expanded.get() { 34. } else { 0. }))
                    .style_when(move || !expanded.get(), |s| s.hidden())
                    .overflow_hidden()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .child(
                                action("Write", Variant::Quiet, move |_| self.focus())
                                    .selected(move || !self.preview.get()),
                            )
                            .child(
                                action("Preview", Variant::Quiet, move |_| {
                                    self.preview.set(true);
                                    self.active.set(true);
                                })
                                .selected(move || self.preview.get()),
                            )
                            .child(view().flex_1())
                            .children([
                                self.format("B", "Bold", "**", "**"),
                                self.format("I", "Italic", "_", "_"),
                                self.format("‹›", "Code", "`", "`"),
                                self.format("↗", "Link", "[", "](https://)"),
                                self.command("↶", "Undo", EditorCommand::Undo),
                                self.command("↷", "Redo", EditorCommand::Redo),
                            ]),
                    ),
            )
            .child(
                view()
                    .h(move || {
                        px(if self.preview.get() && expanded.get() {
                            0.
                        } else {
                            height.get()
                        })
                    })
                    .overflow_hidden()
                    .child(source),
            )
            .child(when(
                move || self.preview.get() && expanded.get(),
                move || {
                    scroll()
                        .h(move || px(height.get()))
                        .child(view().p_3().child(preview()))
                },
            ))
    }
    fn format(
        self,
        title: &str,
        name: &str,
        before: &'static str,
        after: &'static str,
    ) -> AnyElement {
        action(title.to_owned(), Variant::Quiet, move |_| {
            if let Some(api) = self.editor.api::<EditorApi>() {
                let selected = api.selected_text();
                api.insert(format!("{before}{selected}{after}"));
                self.focus();
            }
        })
        .accessibility_label({
            let name = name.to_owned();
            move || Some(name.clone().into())
        })
        .into_element()
    }
    fn command(self, title: &str, name: &str, command: EditorCommand) -> AnyElement {
        action(title.to_owned(), Variant::Quiet, move |_| {
            if let Some(api) = self.editor.api::<EditorApi>() {
                api.run(command);
                self.focus();
            }
        })
        .accessibility_label({
            let name = name.to_owned();
            move || Some(name.clone().into())
        })
        .into_element()
    }
}
