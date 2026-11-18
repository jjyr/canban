//! Shared Markdown reading and source editing. NSH owns the rope, selections,
//! undo stack and caret completion; changing height/preview never remounts it.
use super::tokens::*;
use nsh_base::{Editor, EditorApi, EditorElement, TextViewElement, editor::EditorLanguage};
use nsh_core::prelude::*;

static PROSE_MARKDOWN: EditorLanguage = EditorLanguage::new("canban-markdown")
    .word_chars("-")
    .auto_close(&[('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('`', '`')])
    .string_quotes("")
    .tab_size(2);

pub fn install_editor_language() {
    nsh_base::editor::register_language(&PROSE_MARKDOWN);
}

/// Read-only Markdown with native text selection and clipboard shortcuts.
pub fn reader(body: impl Fn() -> String + 'static) -> TextViewElement {
    nsh_base::text_view()
        .markdown(move || Some(body().into()))
        .selectable(true)
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
    /// reclaims Cmd+Enter for the host's single submit intent while preserving
    /// NSH's Enter/Tab completion acceptance and Enter/Shift+Enter newlines.
    /// The timer and binding belong to this effect and disappear on blur, hold or disposal.
    pub fn on_submit(self, allowed: impl Fn() -> bool + 'static, submit: impl Fn() + 'static) {
        let submit = std::rc::Rc::new(submit);
        effect(move || {
            let _ = self.focus_epoch.get();
            if !self.active.get() || self.held.get() || !allowed() {
                return;
            }
            let submit = submit.clone();
            set_timeout(std::time::Duration::ZERO, move || {
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
    /// Answers retain NSH's caret filtering, Enter/Tab/click acceptance and stale-request checks.
    pub fn completions(
        self,
        id: u32,
        items: Vec<nsh_base::editor::EditorCompletionItem>,
        incomplete: bool,
    ) {
        if let Some(api) = self.editor.api::<EditorApi>() {
            api.present_completions(id, items, Some(incomplete));
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
        self.active.set(false);
        if self.focused.get_untracked()
            && let Some(api) = self.editor.api::<EditorApi>()
        {
            api.blur();
        }
    }
    fn base_editor(self, name: &str, placeholder: &str) -> EditorElement {
        nsh_base::editor()
            .language("canban-markdown")
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
            .active_line(false)
            .indent_guides(false)
            .auto_close(false)
            .auto_indent(false)
            .highlighting(false)
            .font_family(MONO_FONT)
            .font_size(px(13.))
            .line_height(px(20.))
            .px_2()
            .component_ref(&self.editor)
    }
    pub fn source(self, value: Signal<String>, name: &str, placeholder: &str) -> EditorElement {
        self.base_editor(name, placeholder)
            .value(move || Some(value.get().into()))
            .on_change(move |v: &SharedString| value.set(v.to_string()))
            .soft_wrap(true)
            .read_only(move || self.preview.get())
            .text_color(TEXT)
            .py_1()
            .size_full()
    }
    /// Single-line rope editor source supporting caret completion (`@` / `#`)
    /// without newlines or Tab indentation.
    pub fn line_source(
        self,
        value: Signal<String>,
        name: &str,
        placeholder: &str,
        disabled: impl Fn() -> bool + 'static,
    ) -> EditorElement {
        let disabled = std::rc::Rc::new(disabled);
        let disabled_ro = disabled.clone();
        let disabled_color = disabled.clone();
        let disabled_keys = disabled;
        effect(move || {
            let _ = self.focus_epoch.get();
            if !self.active.get() || self.held.get() || disabled_keys() {
                return;
            }
            set_timeout(std::time::Duration::ZERO, move || {
                for key in ["enter", "shift+enter", "mod+shift+enter", "tab"] {
                    on_key_with(
                        key,
                        KeyOptions {
                            when: nsh_core::keymap::When::Always,
                            ..Default::default()
                        },
                        |_| {},
                    );
                }
            });
        });
        self.base_editor(name, placeholder)
            .value(move || Some(value.get().into()))
            .on_change(move |v: &SharedString| {
                let raw = v.as_str();
                let clean = if raw.contains(['\r', '\n']) {
                    raw.replace("\r\n", " ").replace(['\r', '\n'], " ")
                } else {
                    raw.to_owned()
                };
                if value.get_untracked() != clean {
                    value.set(clean);
                }
            })
            .soft_wrap(false)
            .min_rows(Some(1))
            .max_rows(Some(1))
            .scroll_beyond_last_line(false)
            .read_only(move || disabled_ro())
            .text_color(move || {
                if disabled_color() {
                    MUTED.resolve()
                } else {
                    TEXT.resolve()
                }
            })
            .py_1p5()
            .w_full()
    }
    /// Styled single-line container matching `bound_field` chrome for a `line_source`.
    pub fn render_line(
        self,
        source: impl IntoElement,
        disabled: impl Fn() -> bool + 'static,
    ) -> View {
        let disabled = std::rc::Rc::new(disabled);
        let disabled_bg = disabled.clone();
        let disabled_touch = disabled;
        let root = element_ref();
        root.on_focus_in(move || self.active.set(true));
        root.on_focus_out(move || {
            if !self.held.get_untracked() {
                self.active.set(false);
            }
        });
        view()
            .element_ref(&root)
            .on_touch(move |e: &nsh_core::TouchEvent| {
                if e.phase == nsh_core::TouchPhase::Start
                    && !disabled_touch()
                    && !self.focused.get_untracked()
                {
                    self.focus();
                }
            })
            .min_w(px(0.))
            .min_h(px(CONTROL_HEIGHT))
            .rounded(px(RADIUS))
            .border_1()
            .border_color(move || if self.focused.get() { ACCENT } else { BORDER })
            .bg(move || if disabled_bg() { INSET } else { SURFACE })
            .overflow_hidden()
            .justify_center()
            .child(source)
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
            (76. + lines.saturating_sub(2) as f32 * 20.).min(max_height.get().max(76.))
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
            .border_color(move || if self.focused.get() { ACCENT } else { BORDER })
            .bg(INSET)
            .overflow_hidden()
            .child(
                view()
                    .h(move || px(if expanded.get() { 32. } else { 0. }))
                    .style_when(move || !expanded.get(), |s| s.hidden())
                    .px_2()
                    .pt_1()
                    .flex_row()
                    .items_end()
                    .gap_1()
                    .bg(SURFACE)
                    .border_b_1()
                    .border_color(BORDER)
                    .overflow_hidden()
                    .child(self.tab("Write", false))
                    .child(self.tab("Preview", true)),
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
    fn tab(self, title: &'static str, is_preview: bool) -> impl IntoElement {
        crate::press_target()
            .h(px(27.))
            .px(px(10.))
            .items_center()
            .justify_center()
            .rounded_t(px(6.))
            .border_t_1()
            .border_l_1()
            .border_r_1()
            .border_color(move || {
                if self.preview.get() == is_preview {
                    BORDER
                } else {
                    CLEAR
                }
            })
            .bg(move || {
                if self.preview.get() == is_preview {
                    INSET
                } else {
                    CLEAR
                }
            })
            .hover(move |s| {
                if self.preview.get() == is_preview {
                    s
                } else {
                    s.text_color(TEXT)
                }
            })
            .selected(move || self.preview.get() == is_preview)
            .accessibility_label(title)
            .child(
                text(title)
                    .text_size(px(CAPTION_SIZE))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(move || {
                        if self.preview.get() == is_preview {
                            TEXT
                        } else {
                            MUTED
                        }
                    })
                    .pointer_events(PointerEvents::None),
            )
            .on_press(move |_: &()| {
                if is_preview {
                    self.preview.set(true);
                    self.active.set(true);
                } else {
                    self.focus();
                }
            })
    }
}
