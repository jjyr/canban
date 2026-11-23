//! Unified cross-platform UI entry point (`TerminalView`) plus shared binding/font/focus types.
use crate::session::PtyTransport;
use canban_types::Config;
use nsh_core::prelude::*;
use serde_json::Value;
#[cfg(target_os = "macos")]
use serde_json::json;
use std::{fmt, rc::Rc, sync::Arc};

/// Live Herdr PTY target for a mounted terminal surface.
#[derive(Clone)]
pub struct LiveBinding {
    pub config: Config,
    pub core_id: String,
    pub terminal: String,
    /// Optional explicit PTY transport (e.g. mobile `RemotePtyTransport` without local Herdr/SSH).
    pub transport: Option<Arc<dyn PtyTransport>>,
}

impl LiveBinding {
    pub fn new(config: Config, core_id: impl Into<String>, terminal: impl Into<String>) -> Self {
        Self {
            config,
            core_id: core_id.into(),
            terminal: terminal.into(),
            transport: None,
        }
    }

    pub fn with_transport(mut self, transport: Arc<dyn PtyTransport>) -> Self {
        self.transport = Some(transport);
        self
    }
}

impl fmt::Debug for LiveBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveBinding")
            .field("config", &self.config)
            .field("core_id", &self.core_id)
            .field("terminal", &self.terminal)
            .field("has_transport", &self.transport.is_some())
            .finish()
    }
}

impl PartialEq for LiveBinding {
    fn eq(&self, other: &Self) -> bool {
        let same_transport = match (&self.transport, &other.transport) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                std::ptr::eq(Arc::as_ptr(a) as *const (), Arc::as_ptr(b) as *const ())
            }
            _ => false,
        };
        self.config == other.config
            && self.core_id == other.core_id
            && self.terminal == other.terminal
            && same_transport
    }
}

/// Focus request mirroring macOS `{"ticket": u64, "enabled": bool}`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FocusRequest {
    pub ticket: u64,
    pub enabled: bool,
}

/// Parsed terminal font configuration (`family` + clamped point `size` + `line_height`).
#[derive(Clone, Debug, PartialEq)]
pub struct FontSpec {
    pub family: Option<SharedString>,
    pub size: f32,
    pub line_height: f32,
}

impl Default for FontSpec {
    fn default() -> Self {
        Self {
            family: None,
            size: 13.0,
            line_height: 18.0,
        }
    }
}

pub fn parse_font_value(value: &Value) -> FontSpec {
    let family = value["family"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| SharedString::from(s.to_owned()));
    let size = value["size"].as_f64().unwrap_or(13.0).clamp(9.0, 32.0) as f32;
    let line_height = (size * 1.385).round().max(size + 3.0);
    FontSpec {
        family,
        size,
        line_height,
    }
}

/// Decodes a `file://` URI (or bare absolute path) into a local UTF-8 filesystem path.
pub fn decode_file_uri(uri: &str) -> Option<String> {
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        return None;
    }
    let raw_path = if let Some(rest) = trimmed.strip_prefix("file://localhost/") {
        format!("/{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("file:///") {
        format!("/{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("file:/") {
        format!("/{rest}")
    } else if trimmed.starts_with('/') {
        trimmed.to_owned()
    } else {
        return None;
    };

    let bytes = raw_path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().filter(|s| !s.is_empty())
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

enum TerminalSurface {
    Live(Rc<dyn Fn() -> Option<LiveBinding>>),
    Preview(String),
}

/// Platform-unified terminal element builder (`native("canban/terminal")` on macOS,
/// `portable` `nsh_base::Terminal` + `GhosttyEmulator` on Linux/iOS/Android).
pub struct TerminalView {
    surface: TerminalSurface,
    theme: Rc<dyn Fn() -> Value>,
    font: Rc<dyn Fn() -> Value>,
    focus: Rc<dyn Fn() -> FocusRequest>,
    read_only: Rc<dyn Fn() -> bool>,
    input_status: Rc<dyn Fn() -> String>,
    notice_status: Rc<dyn Fn() -> String>,
    ui_command_keys: Option<Vec<String>>,
    hidden: Option<Rc<dyn Fn() -> bool>>,
    height: Option<Length>,
}

impl TerminalView {
    pub fn live(
        binding: impl Fn() -> Option<LiveBinding> + 'static,
        theme: impl Fn() -> Value + 'static,
        font: impl Fn() -> Value + 'static,
        focus: impl Fn() -> FocusRequest + 'static,
    ) -> Self {
        Self {
            surface: TerminalSurface::Live(Rc::new(binding)),
            theme: Rc::new(theme),
            font: Rc::new(font),
            focus: Rc::new(focus),
            read_only: Rc::new(|| false),
            input_status: Rc::new(String::new),
            notice_status: Rc::new(String::new),
            ui_command_keys: None,
            hidden: None,
            height: None,
        }
    }

    pub fn preview(
        content: impl Into<String>,
        theme: impl Fn() -> Value + 'static,
        font: impl Fn() -> Value + 'static,
        focus: impl Fn() -> FocusRequest + 'static,
    ) -> Self {
        Self {
            surface: TerminalSurface::Preview(content.into()),
            theme: Rc::new(theme),
            font: Rc::new(font),
            focus: Rc::new(focus),
            read_only: Rc::new(|| true),
            input_status: Rc::new(|| "Preview".to_owned()),
            notice_status: Rc::new(String::new),
            ui_command_keys: None,
            hidden: None,
            height: None,
        }
    }

    pub fn read_only(mut self, read_only: impl Fn() -> bool + 'static) -> Self {
        self.read_only = Rc::new(read_only);
        self
    }

    pub fn input_status(mut self, input_status: impl Fn() -> String + 'static) -> Self {
        self.input_status = Rc::new(input_status);
        self
    }

    pub fn notice_status(mut self, notice_status: impl Fn() -> String + 'static) -> Self {
        self.notice_status = Rc::new(notice_status);
        self
    }

    pub fn ui_command_keys<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.ui_command_keys = Some(keys.into_iter().map(Into::into).collect());
        self
    }

    pub fn hidden(mut self, hidden: impl Fn() -> bool + 'static) -> Self {
        self.hidden = Some(Rc::new(hidden));
        self
    }

    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = Some(height.into());
        self
    }
}

impl IntoElement for TerminalView {
    fn into_element(self) -> AnyElement {
        #[cfg(target_os = "macos")]
        {
            let theme = self.theme;
            let font = self.font;
            let focus = self.focus;
            let read_only = self.read_only;
            let input_status = self.input_status;
            let notice_status = self.notice_status;

            let mut el = native("canban/terminal")
                .prop("theme", move || theme())
                .prop("font", move || font())
                .prop("focus", move || {
                    let req = focus();
                    json!({
                        "ticket": req.ticket,
                        "enabled": req.enabled,
                    })
                });
            if let Some(keys) = self.ui_command_keys {
                el = el.prop("ui_command_keys", json!(keys));
            }
            el = match self.surface {
                TerminalSurface::Preview(content) => el
                    .prop("content", json!(content))
                    .prop("read_only", json!(true)),
                TerminalSurface::Live(binding) => el
                    .prop("binding", move || match binding() {
                        Some(b) => json!({
                            "config": b.config,
                            "terminal": b.terminal,
                            "core_id": b.core_id,
                        }),
                        None => Value::Null,
                    })
                    .prop("read_only", move || json!(read_only())),
            };
            let el = el
                .prop("input_status", move || json!(input_status()))
                .prop("notice_status", move || json!(notice_status()))
                .w_full();
            let el = match self.height {
                Some(h) => el.h(h),
                None => el.flex_1().min_h(px(0.)),
            };
            match self.hidden {
                Some(hidden) => el
                    .style_when(move || hidden(), |s| s.hidden())
                    .into_element(),
                None => el.into_element(),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = self.ui_command_keys;
            let theme = self.theme;
            let font = self.font;
            let focus = self.focus;
            let read_only = self.read_only;
            let input_status = self.input_status;
            let notice_status = self.notice_status;

            let el = match self.surface {
                TerminalSurface::Preview(content) => crate::portable::preview_terminal_with_status(
                    content,
                    move || theme(),
                    move || font(),
                    move || focus(),
                    move || input_status(),
                    move || notice_status(),
                ),
                TerminalSurface::Live(binding) => crate::portable::live_terminal(
                    move || binding(),
                    move || theme(),
                    move || font(),
                    move || focus(),
                    move || read_only(),
                    move || input_status(),
                    move || notice_status(),
                ),
            }
            .w_full();
            let el = match self.height {
                Some(h) => el.h(h),
                None => el.flex_1().min_h(px(0.)),
            };
            match self.hidden {
                Some(hidden) => el
                    .style_when(move || hidden(), |s| s.hidden())
                    .into_element(),
                None => el.into_element(),
            }
        }
    }
}
