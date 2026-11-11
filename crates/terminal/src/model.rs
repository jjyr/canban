//! libghostty-vt screen/input adapter. Algorithms follow the public C API;
//! terminal frame/input integration references Herdr (Apache-2.0), see notices.
use crate::ffi::*;
use std::ffi::c_void;

#[derive(Clone, Debug, Default)]
pub struct Cell {
    pub text: String,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub bold: bool,
    pub underline: bool,
    pub tail: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub rows: Vec<Vec<Cell>>,
    pub cursor: Option<(u16, u16)>,
    pub background: [u8; 3],
    pub cursor_color: [u8; 3],
}

/// Complete platform-neutral key event description for `libghostty-vt`.
#[derive(Clone, Copy, Debug)]
pub struct KeyInput<'a> {
    pub action: GhosttyKeyAction,
    pub key: GhosttyKey,
    pub mods: GhosttyMods,
    pub consumed_mods: GhosttyMods,
    pub composing: bool,
    pub utf8: &'a str,
    pub unshifted_codepoint: u32,
}

/// Complete platform-neutral mouse event description for `libghostty-vt`.
#[derive(Clone, Copy, Debug)]
pub struct MouseInput {
    pub action: GhosttyMouseAction,
    pub button: Option<GhosttyMouseButton>,
    pub mods: GhosttyMods,
    pub x_px: f32,
    pub y_px: f32,
    pub any_button_pressed: bool,
}

/// Outcome of a scroll wheel step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WheelOutput {
    Pty(Vec<u8>),
    ViewportScrolled,
    None,
}

/// Detected hyperlink span on a single viewport row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkMatch {
    pub uri: String,
    pub row: u16,
    pub start_col: u16,
    pub end_col: u16,
}

/// Zero-JSON C-ABI cell snapshot shared with native renderers.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderCell {
    pub cp: u32,
    pub utf16_offset: u32,
    pub utf16_len: u16,
    pub flags: u16,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub ul_color: [u8; 3],
    pub has_ul_color: u8,
    pub wide: u8,
    pub selected: u8,
}

/// Zero-JSON C-ABI cursor snapshot.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderCursor {
    pub x: u16,
    pub y: u16,
    pub visible: u8,
    pub blinking: u8,
    pub wide_tail: u8,
    pub style: u8,
    pub color: [u8; 3],
    pub text_color: [u8; 3],
}

/// Zero-JSON C-ABI scrollbar snapshot.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderScrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

/// Zero-JSON C-ABI frame header passed across FFI to native host views.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RenderSnapshot {
    pub cells: *const RenderCell,
    pub utf16: *const u16,
    pub dirty_rows: *const u8,
    pub status_ptr: *const u8,
    pub status_len: usize,
    pub cols: u16,
    pub rows: u16,
    pub dirty_state: u8,
    pub mouse_tracking: u8,
    pub background: [u8; 3],
    pub foreground: [u8; 3],
    pub cursor: RenderCursor,
    pub scrollbar: RenderScrollbar,
}

#[derive(Clone, Copy, Debug)]
struct SurfaceGeometry {
    cell_width: u32,
    cell_height: u32,
    pad_left: u32,
    pad_right: u32,
    pad_top: u32,
    pad_bottom: u32,
}

impl Default for SurfaceGeometry {
    fn default() -> Self {
        Self {
            cell_width: 8,
            cell_height: 18,
            pad_left: 6,
            pad_right: 6,
            pad_top: 6,
            pad_bottom: 6,
        }
    }
}

struct Shared {
    replies: Vec<u8>,
    clipboard: Option<String>,
    bell: bool,
    size: GhosttySizeReportSize,
}

struct SnapshotStorage {
    cells: Vec<RenderCell>,
    utf16_rows: Vec<Vec<u16>>,
    utf16_flat: Vec<u16>,
    dirty_rows: Vec<u8>,
    cols: u16,
    rows: u16,
    background: [u8; 3],
    foreground: [u8; 3],
    cursor: RenderCursor,
    scrollbar: RenderScrollbar,
}

impl Default for SnapshotStorage {
    fn default() -> Self {
        Self {
            cells: Vec::new(),
            utf16_rows: Vec::new(),
            utf16_flat: Vec::new(),
            dirty_rows: Vec::new(),
            cols: 0,
            rows: 0,
            background: [24, 24, 28],
            foreground: [224, 228, 236],
            cursor: RenderCursor::default(),
            scrollbar: RenderScrollbar::default(),
        }
    }
}

pub struct Model {
    term: GhosttyTerminal,
    state: GhosttyRenderState,
    rows: GhosttyRenderStateRowIterator,
    cells: GhosttyRenderStateRowCells,
    keys: GhosttyKeyEncoder,
    event: GhosttyKeyEvent,
    mouse: GhosttyMouseEncoder,
    mouse_event: GhosttyMouseEvent,
    gesture: GhosttySelectionGesture,
    gesture_press: GhosttySelectionGestureEvent,
    gesture_drag: GhosttySelectionGestureEvent,
    gesture_release: GhosttySelectionGestureEvent,
    gesture_autoscroll: GhosttySelectionGestureEvent,
    formatter: GhosttyFormatter,
    shared: Box<Shared>,
    geom: SurfaceGeometry,
    snap: SnapshotStorage,
    grapheme_scratch: Vec<u32>,
    pub cols: u16,
    pub height: u16,
    pub revision: u64,
}

// A model is never concurrently accessed: the viewer's worker and native host
// own it behind a mutex. libghostty-vt has no UI-thread affinity.
unsafe impl Send for Model {}

fn ok(r: GhosttyResult) {
    assert_eq!(r, GHOSTTY_SUCCESS, "libghostty-vt operation failed: {r}");
}

const fn dec_mode(value: u16) -> GhosttyMode {
    value & 0x7FFF
}

unsafe extern "C" fn reply(_: GhosttyTerminal, ud: *mut c_void, data: *const u8, len: usize) {
    let shared = unsafe { &mut *(ud as *mut Shared) };
    shared
        .replies
        .extend_from_slice(unsafe { std::slice::from_raw_parts(data, len) });
}

unsafe extern "C" fn size(
    _: GhosttyTerminal,
    ud: *mut c_void,
    out: *mut GhosttySizeReportSize,
) -> bool {
    unsafe { *out = (*(ud as *mut Shared)).size };
    true
}

unsafe extern "C" fn bell(_: GhosttyTerminal, ud: *mut c_void) {
    let shared = unsafe { &mut *(ud as *mut Shared) };
    shared.bell = true;
}

unsafe extern "C" fn clipboard_write(
    _: GhosttyTerminal,
    ud: *mut c_void,
    write: *const GhosttyClipboardWrite,
) {
    if write.is_null() {
        return;
    }
    let req = unsafe { &*write };
    let shared = unsafe { &mut *(ud as *mut Shared) };
    let mut status = GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
    if req.contents_len == 0 {
        shared.clipboard = Some(String::new());
        status = GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS;
    } else if !req.contents.is_null() {
        let items = unsafe { std::slice::from_raw_parts(req.contents, req.contents_len) };
        for item in items {
            let mime = if item.mime.ptr.is_null() || item.mime.len == 0 {
                b"" as &[u8]
            } else {
                unsafe { std::slice::from_raw_parts(item.mime.ptr, item.mime.len) }
            };
            if mime.starts_with(b"text/plain") || mime.is_empty() {
                let bytes = if item.data.ptr.is_null() || item.data.len == 0 {
                    b"" as &[u8]
                } else {
                    unsafe { std::slice::from_raw_parts(item.data.ptr, item.data.len) }
                };
                shared.clipboard = Some(String::from_utf8_lossy(bytes).into_owned());
                status = GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS;
                break;
            }
        }
    }
    if let Some(reply_fn) = req.reply {
        let reply = GhosttyClipboardWriteReply {
            size: std::mem::size_of::<GhosttyClipboardWriteReply>(),
            result: status,
            remember: req.can_remember && status == GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS,
        };
        unsafe { reply_fn(write, &reply) };
    }
}

fn sanitize_key_utf8(text: &str) -> &str {
    if text.is_empty() {
        return "";
    }
    if text
        .chars()
        .any(|c| (c as u32) < 0x20 || c == '\x7f' || ('\u{F700}'..='\u{F8FF}').contains(&c))
    {
        return "";
    }
    text
}

impl Model {
    pub fn new(cols: u16, rows: u16) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        unsafe {
            let mut term = std::ptr::null_mut();
            ok(ghostty_terminal_new(
                std::ptr::null(),
                &mut term,
                cols,
                rows,
            ));
            let mut state = std::ptr::null_mut();
            ok(ghostty_render_state_new(std::ptr::null(), &mut state));
            let mut rows_it = std::ptr::null_mut();
            ok(ghostty_render_state_row_iterator_new(
                std::ptr::null(),
                &mut rows_it,
            ));
            let mut cells = std::ptr::null_mut();
            ok(ghostty_render_state_row_cells_new(
                std::ptr::null(),
                &mut cells,
            ));
            let mut keys = std::ptr::null_mut();
            ok(ghostty_key_encoder_new(std::ptr::null(), &mut keys));
            let mut event = std::ptr::null_mut();
            ok(ghostty_key_event_new(std::ptr::null(), &mut event));
            let mut mouse = std::ptr::null_mut();
            ok(ghostty_mouse_encoder_new(std::ptr::null(), &mut mouse));
            let track_last_cell = true;
            ok(ghostty_mouse_encoder_set(
                mouse,
                GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL,
                &track_last_cell as *const _ as *const c_void,
            ));
            let mut mouse_event = std::ptr::null_mut();
            ok(ghostty_mouse_event_new(std::ptr::null(), &mut mouse_event));

            let mut gesture = std::ptr::null_mut();
            ok(ghostty_selection_gesture_new(
                std::ptr::null(),
                &mut gesture,
            ));
            let mut gesture_press = std::ptr::null_mut();
            ok(ghostty_selection_gesture_event_new(
                std::ptr::null(),
                &mut gesture_press,
                GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS,
            ));
            let mut gesture_drag = std::ptr::null_mut();
            ok(ghostty_selection_gesture_event_new(
                std::ptr::null(),
                &mut gesture_drag,
                GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG,
            ));
            let mut gesture_release = std::ptr::null_mut();
            ok(ghostty_selection_gesture_event_new(
                std::ptr::null(),
                &mut gesture_release,
                GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE,
            ));
            let mut gesture_autoscroll = std::ptr::null_mut();
            ok(ghostty_selection_gesture_event_new(
                std::ptr::null(),
                &mut gesture_autoscroll,
                GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_AUTOSCROLL_TICK,
            ));

            let mut formatter = std::ptr::null_mut();
            ok(ghostty_formatter_terminal_new(
                std::ptr::null(),
                &mut formatter,
                term,
            ));
            let unwrap = true;
            let trim = true;
            ok(ghostty_formatter_set(
                formatter,
                GHOSTTY_FORMATTER_OPT_UNWRAP,
                &unwrap as *const _ as *const c_void,
            ));
            ok(ghostty_formatter_set(
                formatter,
                GHOSTTY_FORMATTER_OPT_TRIM,
                &trim as *const _ as *const c_void,
            ));

            let geom = SurfaceGeometry::default();
            let mut shared = Box::new(Shared {
                replies: Vec::new(),
                clipboard: None,
                bell: false,
                size: GhosttySizeReportSize {
                    rows,
                    columns: cols,
                    cell_width: geom.cell_width,
                    cell_height: geom.cell_height,
                },
            });
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_USERDATA,
                &mut *shared as *mut _ as *const c_void,
            );
            let write_cb: GhosttyTerminalWritePtyFn = Some(reply);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                std::mem::transmute::<GhosttyTerminalWritePtyFn, *const c_void>(write_cb),
            );
            let size_cb: GhosttyTerminalSizeFn = Some(size);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_SIZE,
                std::mem::transmute::<GhosttyTerminalSizeFn, *const c_void>(size_cb),
            );
            let bell_cb: GhosttyTerminalBellFn = Some(bell);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_BELL,
                std::mem::transmute::<GhosttyTerminalBellFn, *const c_void>(bell_cb),
            );
            let clip_cb: GhosttyTerminalClipboardWriteFn = Some(clipboard_write);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,
                std::mem::transmute::<GhosttyTerminalClipboardWriteFn, *const c_void>(clip_cb),
            );
            let max: usize = 10_000_000;
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
                &max as *const _ as *const c_void,
            );
            let alt_esc = GhosttyTerminalModeConfig {
                mode: dec_mode(1036),
                value: true,
            };
            let _ = ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_MODE_DEFAULT,
                &alt_esc as *const _ as *const c_void,
            );
            let tn = b"xterm-256color";
            let tn_str = GhosttyString {
                ptr: tn.as_ptr(),
                len: tn.len(),
            };
            let _ = ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_TERMINFO_NAME,
                &tn_str as *const _ as *const c_void,
            );

            Self {
                term,
                state,
                rows: rows_it,
                cells,
                keys,
                event,
                mouse,
                mouse_event,
                gesture,
                gesture_press,
                gesture_drag,
                gesture_release,
                gesture_autoscroll,
                formatter,
                shared,
                geom,
                snap: SnapshotStorage::default(),
                grapheme_scratch: Vec::with_capacity(16),
                cols,
                height: rows,
                revision: 0,
            }
        }
    }

    /// Change display defaults without resetting the terminal or its scrollback.
    pub fn theme(&mut self, value: &serde_json::Value) {
        let parse = |v: &serde_json::Value| -> Option<GhosttyColorRgb> {
            let a = v.as_array()?;
            if a.len() != 3 {
                return None;
            }
            Some(GhosttyColorRgb {
                r: u8::try_from(a[0].as_u64()?).ok()?,
                g: u8::try_from(a[1].as_u64()?).ok()?,
                b: u8::try_from(a[2].as_u64()?).ok()?,
            })
        };
        for (key, option) in [
            ("foreground", GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND),
            ("background", GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND),
            ("cursor", GHOSTTY_TERMINAL_OPT_COLOR_CURSOR),
        ] {
            if let Some(color) = parse(&value[key]) {
                unsafe {
                    ok(ghostty_terminal_set(
                        self.term,
                        option,
                        &color as *const _ as *const c_void,
                    ));
                }
            }
        }
        let first: Vec<_> = value["ansi"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(parse)
            .collect();
        if first.len() == 16 {
            let mut colors = first;
            for r in [0, 95, 135, 175, 215, 255] {
                for g in [0, 95, 135, 175, 215, 255] {
                    for b in [0, 95, 135, 175, 215, 255] {
                        colors.push(GhosttyColorRgb { r, g, b });
                    }
                }
            }
            for n in 0..24 {
                let c = 8 + n * 10;
                colors.push(GhosttyColorRgb { r: c, g: c, b: c });
            }
            unsafe {
                ok(ghostty_terminal_set(
                    self.term,
                    GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
                    colors.as_ptr().cast(),
                ));
            }
        }
        self.revision += 1;
    }

    pub fn write(&mut self, data: &[u8]) {
        unsafe { ghostty_terminal_vt_write(self.term, data.as_ptr(), data.len()) };
        self.revision += 1;
    }

    pub fn replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.shared.replies)
    }

    pub fn take_clipboard(&mut self) -> Option<String> {
        self.shared.clipboard.take()
    }

    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.shared.bell)
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.resize_with_metrics(
            cols,
            rows,
            self.geom.cell_width,
            self.geom.cell_height,
            self.geom.pad_left,
            self.geom.pad_right,
            self.geom.pad_top,
            self.geom.pad_bottom,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn resize_with_metrics(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width: u32,
        cell_height: u32,
        pad_left: u32,
        pad_right: u32,
        pad_top: u32,
        pad_bottom: u32,
    ) -> bool {
        if cols == 0 || rows == 0 {
            return false;
        }
        let cell_width = cell_width.max(1);
        let cell_height = cell_height.max(1);
        let grid_changed = (cols, rows) != (self.cols, self.height);
        let metrics_changed = (
            cell_width,
            cell_height,
            pad_left,
            pad_right,
            pad_top,
            pad_bottom,
        ) != (
            self.geom.cell_width,
            self.geom.cell_height,
            self.geom.pad_left,
            self.geom.pad_right,
            self.geom.pad_top,
            self.geom.pad_bottom,
        );
        if !grid_changed && !metrics_changed {
            return false;
        }
        self.cols = cols;
        self.height = rows;
        self.geom = SurfaceGeometry {
            cell_width,
            cell_height,
            pad_left,
            pad_right,
            pad_top,
            pad_bottom,
        };
        self.shared.size = GhosttySizeReportSize {
            rows,
            columns: cols,
            cell_width,
            cell_height,
        };
        unsafe {
            ok(ghostty_terminal_resize(
                self.term,
                cols,
                rows,
                cell_width,
                cell_height,
            ));
        }
        self.revision += 1;
        grid_changed
    }

    pub fn scroll(&mut self, delta: isize) {
        if delta == 0 {
            return;
        }
        unsafe {
            let mut s = GhosttyTerminalScrollViewport {
                tag: GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA,
                ..Default::default()
            };
            s.value.delta = delta;
            ghostty_terminal_scroll_viewport(self.term, s);
        }
        self.revision += 1;
    }

    pub fn scroll_to_bottom(&mut self) {
        let mut active = true;
        unsafe {
            let _ = ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE,
                &mut active as *mut _ as *mut c_void,
            );
            if !active {
                let s = GhosttyTerminalScrollViewport {
                    tag: GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM,
                    ..Default::default()
                };
                ghostty_terminal_scroll_viewport(self.term, s);
                self.revision += 1;
            }
        }
    }

    pub fn mode(&self, mode: GhosttyMode) -> bool {
        let mut cfg = GhosttyTerminalModeConfig { mode, value: false };
        unsafe {
            let _ = ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_MODE,
                &mut cfg as *mut _ as *mut c_void,
            );
        }
        cfg.value
    }

    pub fn mouse_tracking(&self) -> bool {
        let mut enabled = false;
        unsafe {
            let _ = ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING,
                &mut enabled as *mut _ as *mut c_void,
            );
        }
        enabled
    }

    pub fn active_screen(&self) -> GhosttyTerminalScreen {
        let mut screen = GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        unsafe {
            let _ = ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                &mut screen as *mut _ as *mut c_void,
            );
        }
        screen
    }

    pub fn focus_sequence(&self, focused: bool) -> Vec<u8> {
        if self.mode(dec_mode(1004)) {
            if focused {
                b"\x1b[I".to_vec()
            } else {
                b"\x1b[O".to_vec()
            }
        } else {
            Vec::new()
        }
    }

    pub fn paste(&mut self, text: &str) -> Vec<u8> {
        let bracketed = self.mode(dec_mode(2004));
        let mut input = text.as_bytes().to_vec();
        let data_ptr = if input.is_empty() {
            std::ptr::null_mut()
        } else {
            input.as_mut_ptr().cast::<std::ffi::c_char>()
        };
        let encoded = unsafe {
            let mut needed = 0usize;
            let res = ghostty_paste_encode(
                data_ptr,
                input.len(),
                bracketed,
                std::ptr::null_mut(),
                0,
                &mut needed,
            );
            if res == GHOSTTY_SUCCESS && needed == 0 {
                Vec::new()
            } else if res == GHOSTTY_OUT_OF_SPACE {
                let mut out = vec![0u8; needed];
                let mut written = 0usize;
                if ghostty_paste_encode(
                    data_ptr,
                    input.len(),
                    bracketed,
                    out.as_mut_ptr(),
                    out.len(),
                    &mut written,
                ) == GHOSTTY_SUCCESS
                {
                    out.truncate(written);
                    out
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        };
        if !encoded.is_empty() {
            self.clear_selection();
            self.scroll_to_bottom();
        }
        encoded
    }

    pub fn encode_key(&mut self, input: KeyInput<'_>) -> Vec<u8> {
        let utf8 = sanitize_key_utf8(input.utf8);
        let unshifted = if (0x20..0x7f).contains(&input.unshifted_codepoint)
            || (input.unshifted_codepoint >= 0x80
                && !(0xF700..=0xF8FF).contains(&input.unshifted_codepoint))
        {
            input.unshifted_codepoint
        } else {
            utf8.chars()
                .next()
                .map(|c| c.to_lowercase().next().unwrap_or(c) as u32)
                .unwrap_or(0)
        };
        let encoded = unsafe {
            ok(ghostty_key_encoder_set_from_terminal(self.keys, self.term));
            let opt_alt = GHOSTTY_OPTION_AS_ALT_TRUE;
            ok(ghostty_key_encoder_set(
                self.keys,
                GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
                &opt_alt as *const _ as *const c_void,
            ));
            ok(ghostty_key_event_set_action(self.event, input.action));
            ok(ghostty_key_event_set_key(self.event, input.key));
            ok(ghostty_key_event_set_mods(self.event, input.mods));
            ok(ghostty_key_event_set_consumed_mods(
                self.event,
                input.consumed_mods,
            ));
            ok(ghostty_key_event_set_composing(self.event, input.composing));
            ok(ghostty_key_event_set_utf8(
                self.event,
                if utf8.is_empty() {
                    GhosttyString::default()
                } else {
                    GhosttyString {
                        ptr: utf8.as_ptr(),
                        len: utf8.len(),
                    }
                },
            ));
            ok(ghostty_key_event_set_unshifted_codepoint(
                self.event, unshifted,
            ));

            let mut buf = [0u8; 256];
            let mut len = 0usize;
            let res = ghostty_key_encoder_encode(
                self.keys,
                self.event,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut len,
            );
            let bytes = if res == GHOSTTY_SUCCESS {
                buf[..len].to_vec()
            } else if res == GHOSTTY_OUT_OF_SPACE && len > 0 {
                let mut heap = vec![0u8; len];
                let mut written = 0usize;
                if ghostty_key_encoder_encode(
                    self.keys,
                    self.event,
                    heap.as_mut_ptr().cast(),
                    heap.len(),
                    &mut written,
                ) == GHOSTTY_SUCCESS
                {
                    heap.truncate(written);
                    heap
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            ok(ghostty_key_event_set_utf8(
                self.event,
                GhosttyString::default(),
            ));
            ok(ghostty_key_event_set_composing(self.event, false));
            bytes
        };
        if !encoded.is_empty() && input.action != GHOSTTY_KEY_ACTION_RELEASE {
            self.clear_selection();
            self.scroll_to_bottom();
        }
        encoded
    }

    pub fn key(&mut self, key: GhosttyKey, mods: GhosttyMods, text: Option<&str>) -> Vec<u8> {
        let utf8 = text.map(sanitize_key_utf8).unwrap_or("");
        let consumed_mods = if !utf8.is_empty() {
            mods & (GHOSTTY_MODS_SHIFT as GhosttyMods)
        } else {
            0
        };
        self.encode_key(KeyInput {
            action: GHOSTTY_KEY_ACTION_PRESS,
            key,
            mods,
            consumed_mods,
            composing: false,
            utf8,
            unshifted_codepoint: 0,
        })
    }

    pub fn commit_text(&mut self, text: &str) -> Vec<u8> {
        if text.is_empty() {
            return Vec::new();
        }
        self.clear_selection();
        self.scroll_to_bottom();
        text.as_bytes().to_vec()
    }

    pub fn encode_mouse(&mut self, input: MouseInput) -> Vec<u8> {
        if !self.mouse_tracking() {
            return Vec::new();
        }
        unsafe {
            if input.action != GHOSTTY_MOUSE_ACTION_MOTION {
                ok(ghostty_mouse_encoder_set_from_terminal(
                    self.mouse, self.term,
                ));
            } else {
                let tracking = if self.mode(dec_mode(1003)) {
                    GHOSTTY_MOUSE_TRACKING_ANY
                } else if self.mode(dec_mode(1002)) {
                    GHOSTTY_MOUSE_TRACKING_BUTTON
                } else if self.mode(dec_mode(1000)) {
                    GHOSTTY_MOUSE_TRACKING_NORMAL
                } else if self.mode(dec_mode(9)) {
                    GHOSTTY_MOUSE_TRACKING_X10
                } else {
                    GHOSTTY_MOUSE_TRACKING_NONE
                };
                let format = if self.mode(dec_mode(1016)) {
                    GHOSTTY_MOUSE_FORMAT_SGR_PIXELS
                } else if self.mode(dec_mode(1006)) {
                    GHOSTTY_MOUSE_FORMAT_SGR
                } else if self.mode(dec_mode(1015)) {
                    GHOSTTY_MOUSE_FORMAT_URXVT
                } else if self.mode(dec_mode(1005)) {
                    GHOSTTY_MOUSE_FORMAT_UTF8
                } else {
                    GHOSTTY_MOUSE_FORMAT_X10
                };
                let _ = ghostty_mouse_encoder_set(
                    self.mouse,
                    GHOSTTY_MOUSE_ENCODER_OPT_EVENT,
                    &tracking as *const _ as *const c_void,
                );
                let _ = ghostty_mouse_encoder_set(
                    self.mouse,
                    GHOSTTY_MOUSE_ENCODER_OPT_FORMAT,
                    &format as *const _ as *const c_void,
                );
            }
            let screen_width = self.geom.pad_left
                + u32::from(self.cols) * self.geom.cell_width
                + self.geom.pad_right;
            let screen_height = self.geom.pad_top
                + u32::from(self.height) * self.geom.cell_height
                + self.geom.pad_bottom;
            let enc_size = GhosttyMouseEncoderSize {
                size: std::mem::size_of::<GhosttyMouseEncoderSize>(),
                screen_width: screen_width.max(1),
                screen_height: screen_height.max(1),
                cell_width: self.geom.cell_width.max(1),
                cell_height: self.geom.cell_height.max(1),
                padding_top: self.geom.pad_top,
                padding_bottom: self.geom.pad_bottom,
                padding_right: self.geom.pad_right,
                padding_left: self.geom.pad_left,
            };
            ok(ghostty_mouse_encoder_set(
                self.mouse,
                GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                &enc_size as *const _ as *const c_void,
            ));
            ok(ghostty_mouse_encoder_set(
                self.mouse,
                GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                &input.any_button_pressed as *const _ as *const c_void,
            ));
            ok(ghostty_mouse_event_set_action(
                self.mouse_event,
                input.action,
            ));
            if let Some(btn) = input.button {
                ok(ghostty_mouse_event_set_button(self.mouse_event, btn));
            } else {
                ok(ghostty_mouse_event_clear_button(self.mouse_event));
            }
            ok(ghostty_mouse_event_set_mods(self.mouse_event, input.mods));
            ok(ghostty_mouse_event_set_position(
                self.mouse_event,
                GhosttyMousePosition {
                    x: input.x_px.max(0.0),
                    y: input.y_px.max(0.0),
                },
            ));
            let mut buf = [0u8; 128];
            let mut written = 0usize;
            if ghostty_mouse_encoder_encode(
                self.mouse,
                self.mouse_event,
                buf.as_mut_ptr(),
                buf.len(),
                &mut written,
            ) == GHOSTTY_SUCCESS
                && written > 0
            {
                buf[..written].to_vec()
            } else {
                Vec::new()
            }
        }
    }

    pub fn wheel(
        &mut self,
        delta_rows: isize,
        delta_cols: isize,
        x_px: f32,
        y_px: f32,
        mods: GhosttyMods,
        alt_override: bool,
    ) -> WheelOutput {
        if delta_rows == 0 && delta_cols == 0 {
            return WheelOutput::None;
        }
        if !alt_override && self.mouse_tracking() {
            let mut out = Vec::new();
            if delta_rows != 0 {
                let btn = if delta_rows < 0 {
                    GHOSTTY_MOUSE_BUTTON_FOUR
                } else {
                    GHOSTTY_MOUSE_BUTTON_FIVE
                };
                for _ in 0..delta_rows.unsigned_abs().min(32) {
                    out.extend_from_slice(&self.encode_mouse(MouseInput {
                        action: GHOSTTY_MOUSE_ACTION_PRESS,
                        button: Some(btn),
                        mods,
                        x_px,
                        y_px,
                        any_button_pressed: false,
                    }));
                }
            }
            if delta_cols != 0 {
                let btn = if delta_cols < 0 {
                    GHOSTTY_MOUSE_BUTTON_SIX
                } else {
                    GHOSTTY_MOUSE_BUTTON_SEVEN
                };
                for _ in 0..delta_cols.unsigned_abs().min(32) {
                    out.extend_from_slice(&self.encode_mouse(MouseInput {
                        action: GHOSTTY_MOUSE_ACTION_PRESS,
                        button: Some(btn),
                        mods,
                        x_px,
                        y_px,
                        any_button_pressed: false,
                    }));
                }
            }
            return if out.is_empty() {
                WheelOutput::None
            } else {
                WheelOutput::Pty(out)
            };
        }

        if !alt_override
            && delta_rows != 0
            && self.active_screen() == GHOSTTY_TERMINAL_SCREEN_ALTERNATE
            && self.mode(dec_mode(1007))
        {
            let arrow = if delta_rows < 0 {
                GHOSTTY_KEY_ARROW_UP
            } else {
                GHOSTTY_KEY_ARROW_DOWN
            };
            let step = self.key(arrow, 0, None);
            if !step.is_empty() {
                let count = delta_rows.unsigned_abs().min(32);
                let mut out = Vec::with_capacity(step.len() * count);
                for _ in 0..count {
                    out.extend_from_slice(&step);
                }
                return WheelOutput::Pty(out);
            }
        }

        if delta_rows != 0 {
            self.scroll(delta_rows);
            return WheelOutput::ViewportScrolled;
        }
        WheelOutput::None
    }

    fn viewport_grid_ref(&self, col: u16, row: u16) -> Option<GhosttyGridRef> {
        if self.cols == 0 || self.height == 0 {
            return None;
        }
        let point = GhosttyPoint {
            tag: GHOSTTY_POINT_TAG_VIEWPORT,
            value: GhosttyPointValue {
                coordinate: GhosttyPointCoordinate {
                    x: col.min(self.cols - 1),
                    y: u32::from(row.min(self.height - 1)),
                },
            },
        };
        let mut r = GhosttyGridRef {
            node: std::ptr::null_mut(),
            x: 0,
            y: 0,
        };
        if unsafe { ghostty_terminal_grid_ref(self.term, point, &mut r) } == GHOSTTY_SUCCESS {
            Some(r)
        } else {
            None
        }
    }

    fn pixel_to_viewport_cell(&self, x_px: f64, y_px: f64) -> (u16, u16) {
        let cw = f64::from(self.geom.cell_width.max(1));
        let ch = f64::from(self.geom.cell_height.max(1));
        let col = ((x_px - f64::from(self.geom.pad_left)) / cw)
            .floor()
            .clamp(0.0, f64::from(self.cols.saturating_sub(1))) as u16;
        let row = ((y_px - f64::from(self.geom.pad_top)) / ch)
            .floor()
            .clamp(0.0, f64::from(self.height.saturating_sub(1))) as u16;
        (col, row)
    }

    fn gesture_geometry(&self) -> GhosttySelectionGestureGeometry {
        let screen_height = self.geom.pad_top
            + u32::from(self.height) * self.geom.cell_height
            + self.geom.pad_bottom;
        GhosttySelectionGestureGeometry {
            columns: u32::from(self.cols.max(1)),
            cell_width: self.geom.cell_width.max(1),
            padding_left: self.geom.pad_left,
            screen_height: screen_height.max(1),
        }
    }

    pub fn select_press(
        &mut self,
        x_px: f64,
        y_px: f64,
        time_ns: u64,
        repeat_interval_ns: u64,
        shift_extend: bool,
        rectangle: bool,
    ) {
        let (col, row) = self.pixel_to_viewport_cell(x_px, y_px);
        let Some(grid_ref) = self.viewport_grid_ref(col, row) else {
            return;
        };
        unsafe {
            if shift_extend {
                let mut existing = GhosttySelection {
                    start: GhosttyGridRef {
                        node: std::ptr::null_mut(),
                        x: 0,
                        y: 0,
                    },
                    end: GhosttyGridRef {
                        node: std::ptr::null_mut(),
                        x: 0,
                        y: 0,
                    },
                    rectangle: false,
                };
                if ghostty_terminal_get(
                    self.term,
                    GHOSTTY_TERMINAL_DATA_SELECTION,
                    &mut existing as *mut _ as *mut c_void,
                ) == GHOSTTY_SUCCESS
                {
                    existing.end = grid_ref;
                    existing.rectangle = rectangle;
                    let _ = ghostty_terminal_set(
                        self.term,
                        GHOSTTY_TERMINAL_OPT_SELECTION,
                        &existing as *const _ as *const c_void,
                    );
                    self.revision += 1;
                    return;
                }
            }

            let pos = GhosttySurfacePosition { x: x_px, y: y_px };
            let repeat_dist: f64 = 6.0;
            ok(ghostty_selection_gesture_event_set(
                self.gesture_press,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                &grid_ref as *const _ as *const c_void,
            ));
            ok(ghostty_selection_gesture_event_set(
                self.gesture_press,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION,
                &pos as *const _ as *const c_void,
            ));
            ok(ghostty_selection_gesture_event_set(
                self.gesture_press,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_TIME_NS,
                &time_ns as *const _ as *const c_void,
            ));
            ok(ghostty_selection_gesture_event_set(
                self.gesture_press,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_INTERVAL_NS,
                &repeat_interval_ns as *const _ as *const c_void,
            ));
            ok(ghostty_selection_gesture_event_set(
                self.gesture_press,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_DISTANCE,
                &repeat_dist as *const _ as *const c_void,
            ));

            let mut selection = GhosttySelection {
                start: GhosttyGridRef {
                    node: std::ptr::null_mut(),
                    x: 0,
                    y: 0,
                },
                end: GhosttyGridRef {
                    node: std::ptr::null_mut(),
                    x: 0,
                    y: 0,
                },
                rectangle: false,
            };
            let res = ghostty_selection_gesture_event(
                self.gesture,
                self.term,
                self.gesture_press,
                &mut selection,
            );
            let _ = ghostty_selection_gesture_event_set(
                self.gesture_press,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                std::ptr::null(),
            );
            if res == GHOSTTY_SUCCESS {
                let _ = ghostty_terminal_set(
                    self.term,
                    GHOSTTY_TERMINAL_OPT_SELECTION,
                    &selection as *const _ as *const c_void,
                );
            } else {
                let _ = ghostty_terminal_set(
                    self.term,
                    GHOSTTY_TERMINAL_OPT_SELECTION,
                    std::ptr::null(),
                );
            }
            self.revision += 1;
        }
    }

    pub fn select_drag(&mut self, x_px: f64, y_px: f64, rectangle: bool) -> i8 {
        let (col, row) = self.pixel_to_viewport_cell(x_px, y_px);
        let Some(grid_ref) = self.viewport_grid_ref(col, row) else {
            return 0;
        };
        let geom = self.gesture_geometry();
        let pos = GhosttySurfacePosition { x: x_px, y: y_px };
        unsafe {
            ok(ghostty_selection_gesture_event_set(
                self.gesture_drag,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                &grid_ref as *const _ as *const c_void,
            ));
            ok(ghostty_selection_gesture_event_set(
                self.gesture_drag,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY,
                &geom as *const _ as *const c_void,
            ));
            ok(ghostty_selection_gesture_event_set(
                self.gesture_drag,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION,
                &pos as *const _ as *const c_void,
            ));
            ok(ghostty_selection_gesture_event_set(
                self.gesture_drag,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_RECTANGLE,
                &rectangle as *const _ as *const c_void,
            ));
            let mut selection = GhosttySelection {
                start: GhosttyGridRef {
                    node: std::ptr::null_mut(),
                    x: 0,
                    y: 0,
                },
                end: GhosttyGridRef {
                    node: std::ptr::null_mut(),
                    x: 0,
                    y: 0,
                },
                rectangle: false,
            };
            let res = ghostty_selection_gesture_event(
                self.gesture,
                self.term,
                self.gesture_drag,
                &mut selection,
            );
            let _ = ghostty_selection_gesture_event_set(
                self.gesture_drag,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                std::ptr::null(),
            );
            if res == GHOSTTY_SUCCESS {
                let _ = ghostty_terminal_set(
                    self.term,
                    GHOSTTY_TERMINAL_OPT_SELECTION,
                    &selection as *const _ as *const c_void,
                );
                self.revision += 1;
            }
            self.selection_autoscroll_direction()
        }
    }

    pub fn select_autoscroll_tick(&mut self, x_px: f64, y_px: f64, rectangle: bool) -> i8 {
        let (col, row) = self.pixel_to_viewport_cell(x_px, y_px);
        let viewport = GhosttyPointCoordinate {
            x: col,
            y: u32::from(row),
        };
        let geom = self.gesture_geometry();
        let pos = GhosttySurfacePosition { x: x_px, y: y_px };
        unsafe {
            ok(ghostty_selection_gesture_event_set(
                self.gesture_autoscroll,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_VIEWPORT,
                &viewport as *const _ as *const c_void,
            ));
            ok(ghostty_selection_gesture_event_set(
                self.gesture_autoscroll,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY,
                &geom as *const _ as *const c_void,
            ));
            ok(ghostty_selection_gesture_event_set(
                self.gesture_autoscroll,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION,
                &pos as *const _ as *const c_void,
            ));
            ok(ghostty_selection_gesture_event_set(
                self.gesture_autoscroll,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_RECTANGLE,
                &rectangle as *const _ as *const c_void,
            ));
            let mut selection = GhosttySelection {
                start: GhosttyGridRef {
                    node: std::ptr::null_mut(),
                    x: 0,
                    y: 0,
                },
                end: GhosttyGridRef {
                    node: std::ptr::null_mut(),
                    x: 0,
                    y: 0,
                },
                rectangle: false,
            };
            if ghostty_selection_gesture_event(
                self.gesture,
                self.term,
                self.gesture_autoscroll,
                &mut selection,
            ) == GHOSTTY_SUCCESS
            {
                let _ = ghostty_terminal_set(
                    self.term,
                    GHOSTTY_TERMINAL_OPT_SELECTION,
                    &selection as *const _ as *const c_void,
                );
                self.revision += 1;
            }
            self.selection_autoscroll_direction()
        }
    }

    pub fn select_release(&mut self, x_px: f64, y_px: f64) {
        let (col, row) = self.pixel_to_viewport_cell(x_px, y_px);
        let grid_ref = self.viewport_grid_ref(col, row);
        unsafe {
            if let Some(ref r) = grid_ref {
                let _ = ghostty_selection_gesture_event_set(
                    self.gesture_release,
                    GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                    r as *const _ as *const c_void,
                );
            } else {
                let _ = ghostty_selection_gesture_event_set(
                    self.gesture_release,
                    GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                    std::ptr::null(),
                );
            }
            let _ = ghostty_selection_gesture_event(
                self.gesture,
                self.term,
                self.gesture_release,
                std::ptr::null_mut(),
            );
            let _ = ghostty_selection_gesture_event_set(
                self.gesture_release,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                std::ptr::null(),
            );
        }
    }

    fn selection_autoscroll_direction(&self) -> i8 {
        let mut dir = GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_NONE;
        if unsafe {
            ghostty_selection_gesture_get(
                self.gesture,
                self.term,
                GHOSTTY_SELECTION_GESTURE_DATA_AUTOSCROLL,
                &mut dir as *mut _ as *mut c_void,
            )
        } == GHOSTTY_SUCCESS
        {
            match dir {
                GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_UP => -1,
                GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_DOWN => 1,
                _ => 0,
            }
        } else {
            0
        }
    }

    pub fn select_all(&mut self) -> bool {
        let mut selection = GhosttySelection {
            start: GhosttyGridRef {
                node: std::ptr::null_mut(),
                x: 0,
                y: 0,
            },
            end: GhosttyGridRef {
                node: std::ptr::null_mut(),
                x: 0,
                y: 0,
            },
            rectangle: false,
        };
        let ok_sel = unsafe {
            if ghostty_terminal_select_all(self.term, &mut selection) == GHOSTTY_SUCCESS {
                ghostty_terminal_set(
                    self.term,
                    GHOSTTY_TERMINAL_OPT_SELECTION,
                    &selection as *const _ as *const c_void,
                ) == GHOSTTY_SUCCESS
            } else {
                false
            }
        };
        if ok_sel {
            self.revision += 1;
        }
        ok_sel
    }

    pub fn clear_selection(&mut self) {
        if self.has_selection() {
            unsafe {
                ghostty_selection_gesture_reset(self.gesture, self.term);
                let _ = ghostty_terminal_set(
                    self.term,
                    GHOSTTY_TERMINAL_OPT_SELECTION,
                    std::ptr::null(),
                );
            }
            self.revision += 1;
        }
    }

    pub fn has_selection(&self) -> bool {
        let mut selection = GhosttySelection {
            start: GhosttyGridRef {
                node: std::ptr::null_mut(),
                x: 0,
                y: 0,
            },
            end: GhosttyGridRef {
                node: std::ptr::null_mut(),
                x: 0,
                y: 0,
            },
            rectangle: false,
        };
        unsafe {
            ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_SELECTION,
                &mut selection as *mut _ as *mut c_void,
            ) == GHOSTTY_SUCCESS
        }
    }

    pub fn selected_text(&mut self) -> Option<String> {
        let mut selection = GhosttySelection {
            start: GhosttyGridRef {
                node: std::ptr::null_mut(),
                x: 0,
                y: 0,
            },
            end: GhosttyGridRef {
                node: std::ptr::null_mut(),
                x: 0,
                y: 0,
            },
            rectangle: false,
        };
        unsafe {
            if ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_SELECTION,
                &mut selection as *mut _ as *mut c_void,
            ) != GHOSTTY_SUCCESS
            {
                return None;
            }
            if ghostty_formatter_set(
                self.formatter,
                GHOSTTY_FORMATTER_OPT_SELECTION,
                &selection as *const _ as *const c_void,
            ) != GHOSTTY_SUCCESS
            {
                return None;
            }
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut len: usize = 0;
            let res = ghostty_formatter_format_alloc(
                self.formatter,
                std::ptr::null(),
                &mut ptr,
                &mut len,
            );
            let _ = ghostty_formatter_set(
                self.formatter,
                GHOSTTY_FORMATTER_OPT_SELECTION,
                std::ptr::null(),
            );
            if res != GHOSTTY_SUCCESS {
                return None;
            }
            let text = if ptr.is_null() || len == 0 {
                String::new()
            } else {
                let s = String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned();
                ghostty_free(std::ptr::null(), ptr, len);
                s
            };
            if text.is_empty() { None } else { Some(text) }
        }
    }

    pub fn viewport_text(&mut self) -> String {
        unsafe {
            let _ = ghostty_formatter_set(
                self.formatter,
                GHOSTTY_FORMATTER_OPT_SELECTION,
                std::ptr::null(),
            );
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut len: usize = 0;
            if ghostty_formatter_format_alloc(self.formatter, std::ptr::null(), &mut ptr, &mut len)
                != GHOSTTY_SUCCESS
                || ptr.is_null()
                || len == 0
            {
                return String::new();
            }
            let s = String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned();
            ghostty_free(std::ptr::null(), ptr, len);
            s
        }
    }

    fn osc8_uri_at(&self, col: u16, row: u16) -> Option<String> {
        let grid_ref = self.viewport_grid_ref(col, row)?;
        unsafe {
            let mut cell = GhosttyCell::default();
            if ghostty_grid_ref_cell(&grid_ref, &mut cell) != GHOSTTY_SUCCESS {
                return None;
            }
            let mut has_link = false;
            let _ = ghostty_cell_get(
                cell,
                GHOSTTY_CELL_DATA_HAS_HYPERLINK,
                &mut has_link as *mut _ as *mut c_void,
            );
            if !has_link {
                return None;
            }
            let mut len = 0usize;
            let res = ghostty_grid_ref_hyperlink_uri(&grid_ref, std::ptr::null_mut(), 0, &mut len);
            if res != GHOSTTY_OUT_OF_SPACE || len == 0 {
                return None;
            }
            let mut buf = vec![0u8; len];
            if ghostty_grid_ref_hyperlink_uri(&grid_ref, buf.as_mut_ptr(), len, &mut len)
                != GHOSTTY_SUCCESS
            {
                return None;
            }
            buf.truncate(len);
            String::from_utf8(buf).ok().filter(|s| !s.is_empty())
        }
    }

    pub fn link_at(&self, col: u16, row: u16) -> Option<LinkMatch> {
        if col >= self.cols || row >= self.height {
            return None;
        }
        if let Some(uri) = self.osc8_uri_at(col, row) {
            let mut start = col;
            while start > 0 && self.osc8_uri_at(start - 1, row).as_deref() == Some(uri.as_str()) {
                start -= 1;
            }
            let mut end = col;
            while end + 1 < self.cols
                && self.osc8_uri_at(end + 1, row).as_deref() == Some(uri.as_str())
            {
                end += 1;
            }
            return Some(LinkMatch {
                uri,
                row,
                start_col: start,
                end_col: end,
            });
        }

        let mut chars: Vec<(u16, u16, char)> = Vec::with_capacity(self.cols as usize);
        let mut stack = [0u32; 8];
        for x in 0..self.cols {
            let Some(grid_ref) = self.viewport_grid_ref(x, row) else {
                continue;
            };
            unsafe {
                let mut cell = GhosttyCell::default();
                if ghostty_grid_ref_cell(&grid_ref, &mut cell) != GHOSTTY_SUCCESS {
                    continue;
                }
                let mut wide = GHOSTTY_CELL_WIDE_NARROW;
                let _ = ghostty_cell_get(
                    cell,
                    GHOSTTY_CELL_DATA_WIDE,
                    &mut wide as *mut _ as *mut c_void,
                );
                if wide == GHOSTTY_CELL_WIDE_SPACER_TAIL || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD {
                    continue;
                }
                let end_x = if wide == GHOSTTY_CELL_WIDE_WIDE {
                    (x + 1).min(self.cols - 1)
                } else {
                    x
                };
                let mut count = 0usize;
                let res = ghostty_grid_ref_graphemes(
                    &grid_ref,
                    stack.as_mut_ptr(),
                    stack.len(),
                    &mut count,
                );
                if res == GHOSTTY_SUCCESS && count > 0 {
                    for &cp in &stack[..count] {
                        if let Some(ch) = char::from_u32(cp) {
                            chars.push((x, end_x, ch));
                        }
                    }
                } else {
                    chars.push((x, end_x, ' '));
                }
            }
        }
        detect_url_in_row(&chars, col, row)
    }

    pub fn link_at_px(&self, x_px: f64, y_px: f64) -> Option<LinkMatch> {
        let (col, row) = self.pixel_to_viewport_cell(x_px, y_px);
        self.link_at(col, row)
    }

    pub fn render_snapshot(&mut self, status: &str, force_full: bool) -> RenderSnapshot {
        unsafe {
            ok(ghostty_render_state_update(self.state, self.term));

            let mut cols = 0u16;
            let mut rows = 0u16;
            let mut dirty = GHOSTTY_RENDER_STATE_DIRTY_FULL;
            ok(ghostty_render_state_get(
                self.state,
                GHOSTTY_RENDER_STATE_DATA_COLS,
                &mut cols as *mut _ as *mut c_void,
            ));
            ok(ghostty_render_state_get(
                self.state,
                GHOSTTY_RENDER_STATE_DATA_ROWS,
                &mut rows as *mut _ as *mut c_void,
            ));
            ok(ghostty_render_state_get(
                self.state,
                GHOSTTY_RENDER_STATE_DATA_DIRTY,
                &mut dirty as *mut _ as *mut c_void,
            ));

            let mut colors = GhosttyRenderStateColors {
                size: std::mem::size_of::<GhosttyRenderStateColors>(),
                ..Default::default()
            };
            ok(ghostty_render_state_get(
                self.state,
                GHOSTTY_RENDER_STATE_DATA_COLORS,
                &mut colors as *mut _ as *mut c_void,
            ));
            let rgb = |c: GhosttyColorRgb| [c.r, c.g, c.b];
            let bg_default = rgb(colors.background);
            let fg_default = rgb(colors.foreground);

            let resized = (cols, rows) != (self.snap.cols, self.snap.rows)
                || self.snap.cells.len() != usize::from(cols) * usize::from(rows);
            let palette_changed =
                bg_default != self.snap.background || fg_default != self.snap.foreground;
            let full = force_full
                || resized
                || palette_changed
                || dirty == GHOSTTY_RENDER_STATE_DIRTY_FULL;

            if resized {
                self.snap.cols = cols;
                self.snap.rows = rows;
                self.snap
                    .cells
                    .resize(usize::from(cols) * usize::from(rows), RenderCell::default());
                self.snap.utf16_rows.resize(usize::from(rows), Vec::new());
                self.snap.dirty_rows.resize(usize::from(rows), 1);
            } else {
                self.snap.dirty_rows.fill(u8::from(full));
            }
            self.snap.background = bg_default;
            self.snap.foreground = fg_default;

            let mut any_grapheme_changed = false;
            if full || dirty != GHOSTTY_RENDER_STATE_DIRTY_FALSE {
                ok(ghostty_render_state_get(
                    self.state,
                    GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                    &mut self.rows as *mut _ as *mut c_void,
                ));
                let mut y = 0usize;
                while ghostty_render_state_row_iterator_next(self.rows) && y < usize::from(rows) {
                    let mut row_dirty = true;
                    if !full {
                        let _ = ghostty_render_state_row_get(
                            self.rows,
                            GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
                            &mut row_dirty as *mut _ as *mut c_void,
                        );
                    }
                    if row_dirty {
                        self.snap.dirty_rows[y] = 1;
                        let mut sel = GhosttyRenderStateRowSelection {
                            size: std::mem::size_of::<GhosttyRenderStateRowSelection>(),
                            start_x: 0,
                            end_x: 0,
                        };
                        let has_sel = ghostty_render_state_row_get(
                            self.rows,
                            GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                            &mut sel as *mut _ as *mut c_void,
                        ) == GHOSTTY_SUCCESS;

                        ok(ghostty_render_state_row_get(
                            self.rows,
                            GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                            &mut self.cells as *mut _ as *mut c_void,
                        ));
                        let row_utf16 = &mut self.snap.utf16_rows[y];
                        let had_utf16 = !row_utf16.is_empty();
                        row_utf16.clear();

                        let row_base = y * usize::from(cols);
                        let mut x = 0usize;
                        while ghostty_render_state_row_cells_next(self.cells)
                            && x < usize::from(cols)
                        {
                            let mut raw = GhosttyCell::default();
                            ghostty_render_state_row_cells_get(
                                self.cells,
                                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                                &mut raw as *mut _ as *mut c_void,
                            );
                            let mut wide = GHOSTTY_CELL_WIDE_NARROW;
                            ghostty_cell_get(
                                raw,
                                GHOSTTY_CELL_DATA_WIDE,
                                &mut wide as *mut _ as *mut c_void,
                            );
                            let mut has_styling = false;
                            ghostty_render_state_row_cells_get(
                                self.cells,
                                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING,
                                &mut has_styling as *mut _ as *mut c_void,
                            );
                            let mut style = GhosttyStyle::default();
                            if has_styling {
                                ghostty_render_state_row_cells_get(
                                    self.cells,
                                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                                    &mut style as *mut _ as *mut c_void,
                                );
                            }
                            let mut fg = colors.foreground;
                            let _ = ghostty_render_state_row_cells_get(
                                self.cells,
                                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                                &mut fg as *mut _ as *mut c_void,
                            );
                            let mut bg = colors.background;
                            let _ = ghostty_render_state_row_cells_get(
                                self.cells,
                                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                                &mut bg as *mut _ as *mut c_void,
                            );
                            if style.flags & CANBAN_STYLE_INVERSE != 0 {
                                std::mem::swap(&mut fg, &mut bg);
                            }

                            let (has_ul_color, ul_color) =
                                match GhosttyStyleColorTag::from(style.underline_color.tag) {
                                    GHOSTTY_STYLE_COLOR_PALETTE => {
                                        let idx = usize::from(style.underline_color.value.palette);
                                        (1u8, rgb(colors.palette[idx]))
                                    }
                                    GHOSTTY_STYLE_COLOR_RGB => {
                                        (1u8, rgb(style.underline_color.value.rgb))
                                    }
                                    _ => (0u8, [0, 0, 0]),
                                };

                            let invisible = style.flags & CANBAN_STYLE_INVISIBLE != 0
                                || wide == GHOSTTY_CELL_WIDE_SPACER_TAIL
                                || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD;

                            let mut cp = 0u32;
                            let mut utf16_offset = 0u32;
                            let mut utf16_len = 0u16;

                            if !invisible {
                                let mut g_len = 0u32;
                                ghostty_render_state_row_cells_get(
                                    self.cells,
                                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
                                    &mut g_len as *mut _ as *mut c_void,
                                );
                                if g_len == 1 {
                                    let mut single = 0u32;
                                    let mut buf = GhosttyCodepointBuffer {
                                        ptr: &mut single,
                                        cap: 1,
                                        len: 0,
                                    };
                                    if ghostty_render_state_row_cells_get(
                                        self.cells,
                                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
                                        &mut buf as *mut _ as *mut c_void,
                                    ) == GHOSTTY_SUCCESS
                                    {
                                        cp = single;
                                        if single > 0xFFFF
                                            && let Some(ch) = char::from_u32(single)
                                        {
                                            let mut tmp = [0u16; 2];
                                            let enc = ch.encode_utf16(&mut tmp);
                                            utf16_offset = row_utf16.len() as u32;
                                            utf16_len = enc.len() as u16;
                                            row_utf16.extend_from_slice(enc);
                                        }
                                    }
                                } else if g_len > 1 {
                                    self.grapheme_scratch.resize(g_len as usize, 0);
                                    let mut buf = GhosttyCodepointBuffer {
                                        ptr: self.grapheme_scratch.as_mut_ptr(),
                                        cap: self.grapheme_scratch.len(),
                                        len: 0,
                                    };
                                    if ghostty_render_state_row_cells_get(
                                        self.cells,
                                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
                                        &mut buf as *mut _ as *mut c_void,
                                    ) == GHOSTTY_SUCCESS
                                    {
                                        cp = self.grapheme_scratch[0];
                                        utf16_offset = row_utf16.len() as u32;
                                        let start = row_utf16.len();
                                        for &scalar in &self.grapheme_scratch[..buf.len] {
                                            if let Some(ch) = char::from_u32(scalar) {
                                                let mut tmp = [0u16; 2];
                                                row_utf16
                                                    .extend_from_slice(ch.encode_utf16(&mut tmp));
                                            }
                                        }
                                        utf16_len = (row_utf16.len() - start) as u16;
                                    }
                                }
                            }

                            let selected = u8::from(
                                has_sel && (x as u16) >= sel.start_x && (x as u16) <= sel.end_x,
                            );

                            self.snap.cells[row_base + x] = RenderCell {
                                cp,
                                utf16_offset,
                                utf16_len,
                                flags: style.flags,
                                fg: rgb(fg),
                                bg: rgb(bg),
                                ul_color,
                                has_ul_color,
                                wide: wide as u8,
                                selected,
                            };
                            x += 1;
                        }
                        if had_utf16 || !row_utf16.is_empty() {
                            any_grapheme_changed = true;
                        } else {
                            // Ensure clean rows don't leave stale offsets if utf16_flat is NOT rebuilt.
                        }
                    }
                    y += 1;
                }
            }

            if any_grapheme_changed || full || !self.snap.utf16_flat.is_empty() {
                self.snap.utf16_flat.clear();
                for y in 0..usize::from(rows) {
                    let row_utf16 = &self.snap.utf16_rows[y];
                    if row_utf16.is_empty() {
                        continue;
                    }
                    let base = self.snap.utf16_flat.len() as u32;
                    self.snap.utf16_flat.extend_from_slice(row_utf16);
                    let row_cells =
                        &mut self.snap.cells[y * usize::from(cols)..(y + 1) * usize::from(cols)];
                    let mut local = 0u32;
                    for cell in row_cells {
                        if cell.utf16_len > 0 {
                            cell.utf16_offset = base + local;
                            local += u32::from(cell.utf16_len);
                        }
                    }
                }
            }

            let mut cur = GhosttyRenderStateCursor {
                size: std::mem::size_of::<GhosttyRenderStateCursor>(),
                ..Default::default()
            };
            ok(ghostty_render_state_get(
                self.state,
                GHOSTTY_RENDER_STATE_DATA_CURSOR,
                &mut cur as *mut _ as *mut c_void,
            ));
            let prev_cursor = self.snap.cursor;
            let cursor_color = rgb(if colors.cursor_has_value {
                colors.cursor
            } else {
                colors.foreground
            });
            let next_cursor = if cur.visible && cur.viewport_has_value {
                let mut cx = cur.viewport_x.min(cols.saturating_sub(1));
                let cy = cur.viewport_y.min(rows.saturating_sub(1));
                if cur.wide_tail && cx > 0 {
                    cx -= 1;
                }
                let text_color =
                    if usize::from(cy) < usize::from(rows) && usize::from(cx) < usize::from(cols) {
                        self.snap.cells[usize::from(cy) * usize::from(cols) + usize::from(cx)].bg
                    } else {
                        bg_default
                    };
                RenderCursor {
                    x: cx,
                    y: cy,
                    visible: 1,
                    blinking: u8::from(cur.blinking),
                    wide_tail: u8::from(cur.wide_tail),
                    style: cur.visual_style as u8,
                    color: cursor_color,
                    text_color,
                }
            } else {
                RenderCursor {
                    color: cursor_color,
                    text_color: bg_default,
                    ..Default::default()
                }
            };
            if prev_cursor != next_cursor {
                if prev_cursor.visible != 0
                    && usize::from(prev_cursor.y) < self.snap.dirty_rows.len()
                {
                    self.snap.dirty_rows[usize::from(prev_cursor.y)] = 1;
                }
                if next_cursor.visible != 0
                    && usize::from(next_cursor.y) < self.snap.dirty_rows.len()
                {
                    self.snap.dirty_rows[usize::from(next_cursor.y)] = 1;
                }
            }
            self.snap.cursor = next_cursor;

            let mut sb = GhosttyTerminalScrollbar::default();
            let _ = ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_SCROLLBAR,
                &mut sb as *mut _ as *mut c_void,
            );
            let next_sb = RenderScrollbar {
                total: sb.total,
                offset: sb.offset,
                len: sb.len,
            };
            let sb_changed = next_sb != self.snap.scrollbar;
            self.snap.scrollbar = next_sb;

            ok(ghostty_render_state_clean(self.state));

            let any_row_dirty = self.snap.dirty_rows.iter().any(|&d| d != 0);
            let dirty_state = if full {
                2u8
            } else if any_row_dirty || sb_changed {
                1u8
            } else {
                0u8
            };

            RenderSnapshot {
                cells: self.snap.cells.as_ptr(),
                utf16: self.snap.utf16_flat.as_ptr(),
                dirty_rows: self.snap.dirty_rows.as_ptr(),
                status_ptr: status.as_ptr(),
                status_len: status.len(),
                cols,
                rows,
                dirty_state,
                mouse_tracking: u8::from(self.mouse_tracking()),
                background: self.snap.background,
                foreground: self.snap.foreground,
                cursor: self.snap.cursor,
                scrollbar: self.snap.scrollbar,
            }
        }
    }

    pub fn frame(&mut self) -> Frame {
        let snap = self.render_snapshot("", true);
        let cols = usize::from(snap.cols);
        let rows_len = usize::from(snap.rows);
        let mut rows = Vec::with_capacity(rows_len);
        for y in 0..rows_len {
            let mut row = Vec::with_capacity(cols);
            for x in 0..cols {
                let c = self.snap.cells[y * cols + x];
                let text = if c.utf16_len > 0 {
                    let start = c.utf16_offset as usize;
                    let end = start + usize::from(c.utf16_len);
                    String::from_utf16_lossy(&self.snap.utf16_flat[start..end])
                } else if c.cp != 0 {
                    char::from_u32(c.cp)
                        .map(|ch| ch.to_string())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                row.push(Cell {
                    text,
                    fg: c.fg,
                    bg: c.bg,
                    bold: c.flags & CANBAN_STYLE_BOLD != 0,
                    underline: c.flags & CANBAN_STYLE_UNDERLINE != 0,
                    tail: c.wide == GHOSTTY_CELL_WIDE_SPACER_TAIL as u8,
                });
            }
            rows.push(row);
        }
        Frame {
            rows,
            cursor: (snap.cursor.visible != 0).then_some((snap.cursor.x, snap.cursor.y)),
            background: snap.background,
            cursor_color: snap.cursor.color,
        }
    }
}

fn detect_url_in_row(chars: &[(u16, u16, char)], col: u16, row: u16) -> Option<LinkMatch> {
    if chars.is_empty() {
        return None;
    }
    let mut byte_map: Vec<(usize, usize, u16, u16)> = Vec::with_capacity(chars.len());
    let mut line = String::with_capacity(chars.len());
    for &(sx, ex, ch) in chars {
        let start_b = line.len();
        line.push(ch);
        let end_b = line.len();
        byte_map.push((start_b, end_b, sx, ex));
    }

    let mut search_from = 0usize;
    while search_from < line.len() {
        let rest = &line[search_from..];
        let rel = match (rest.find("https://"), rest.find("http://")) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let Some(rel) = rel else {
            break;
        };
        let url_start = search_from + rel;
        let after = &line[url_start..];
        let mut url_len = 0usize;
        for ch in after.chars() {
            if ch.is_whitespace()
                || ch.is_control()
                || matches!(ch, '"' | '\'' | '<' | '>' | '`' | '│' | '┃')
            {
                break;
            }
            url_len += ch.len_utf8();
        }
        let mut candidate = &after[..url_len];
        while let Some(last) = candidate.chars().next_back() {
            if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | ']' | '}') {
                candidate = &candidate[..candidate.len() - last.len_utf8()];
            } else if last == ')' {
                let opens = candidate.chars().filter(|&c| c == '(').count();
                let closes = candidate.chars().filter(|&c| c == ')').count();
                if closes > opens {
                    candidate = &candidate[..candidate.len() - 1];
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        let url_end = url_start + candidate.len();
        if candidate.len() > "http://x".len() {
            let first_cell = byte_map.iter().find(|&&(sb, _, _, _)| sb >= url_start);
            let last_cell = byte_map.iter().rfind(|&&(_, eb, _, _)| eb <= url_end);
            if let (Some(&(_, _, start_col, _)), Some(&(_, _, _, end_col))) =
                (first_cell, last_cell)
                && col >= start_col
                && col <= end_col
            {
                return Some(LinkMatch {
                    uri: candidate.to_owned(),
                    row,
                    start_col,
                    end_col,
                });
            }
        }
        search_from = (url_start + 7).min(line.len());
    }
    None
}

impl Drop for Model {
    fn drop(&mut self) {
        unsafe {
            ghostty_formatter_free(self.formatter);
            ghostty_selection_gesture_free(self.gesture, self.term);
            ghostty_selection_gesture_event_free(self.gesture_press);
            ghostty_selection_gesture_event_free(self.gesture_drag);
            ghostty_selection_gesture_event_free(self.gesture_release);
            ghostty_selection_gesture_event_free(self.gesture_autoscroll);
            ghostty_mouse_event_free(self.mouse_event);
            ghostty_mouse_encoder_free(self.mouse);
            ghostty_key_event_free(self.event);
            ghostty_key_encoder_free(self.keys);
            ghostty_render_state_row_cells_free(self.cells);
            ghostty_render_state_row_iterator_free(self.rows);
            ghostty_render_state_free(self.state);
            ghostty_terminal_free(self.term);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(model: &mut Model) -> String {
        model
            .frame()
            .rows
            .into_iter()
            .map(|r| r.into_iter().map(|c| c.text).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn long_output_scrolls_back_and_returns_to_live_view_after_resize() {
        let mut model = Model::new(40, 8);
        for n in 0..12000 {
            model.write(format!("row {n:05}\r\n").as_bytes());
        }
        assert!(text(&mut model).contains("row 11999"));
        model.scroll(-50);
        let older = text(&mut model);
        assert!(!older.contains("row 11999"));
        assert!(older.contains("row 11949"));
        model.resize(55, 12);
        assert_eq!(model.frame().rows.len(), 12);
        model.scroll(12000);
        assert!(text(&mut model).contains("row 11999"));
        model.write("still responsive 世界".as_bytes());
        assert!(text(&mut model).contains("still responsive 世界"));
    }

    #[test]
    fn keyboard_encoding_tracks_application_cursor_mode() {
        let mut model = Model::new(40, 8);
        assert_eq!(
            model.key(GHOSTTY_KEY_UNIDENTIFIED, 0, Some("世界")),
            "世界".as_bytes()
        );
        assert_eq!(model.key(GHOSTTY_KEY_ENTER, 0, None), b"\r");
        assert_eq!(
            model.key(GHOSTTY_KEY_C, GHOSTTY_MODS_CTRL as GhosttyMods, Some("c")),
            b"\x03"
        );
        assert_eq!(model.key(GHOSTTY_KEY_ARROW_UP, 0, None), b"\x1b[A");
        model.write(b"\x1b[?1h");
        assert_eq!(model.key(GHOSTTY_KEY_ARROW_UP, 0, None), b"\x1bOA");
        model.write(b"\x1b[?1l");
        assert_eq!(model.key(GHOSTTY_KEY_ARROW_UP, 0, None), b"\x1b[A");
    }

    #[test]
    fn both_left_and_right_option_encode_as_alt_and_kitty_shift_enter_works() {
        let mut model = Model::new(40, 8);
        let left_alt = GHOSTTY_MODS_ALT as GhosttyMods;
        let right_alt = (GHOSTTY_MODS_ALT | GHOSTTY_MODS_ALT_SIDE) as GhosttyMods;

        for mods in [left_alt, right_alt] {
            assert_eq!(
                model.encode_key(KeyInput {
                    action: GHOSTTY_KEY_ACTION_PRESS,
                    key: GHOSTTY_KEY_B,
                    mods,
                    consumed_mods: 0,
                    composing: false,
                    utf8: "b",
                    unshifted_codepoint: 'b' as u32,
                }),
                b"\x1bb"
            );
            assert_eq!(
                model.encode_key(KeyInput {
                    action: GHOSTTY_KEY_ACTION_PRESS,
                    key: GHOSTTY_KEY_F,
                    mods,
                    consumed_mods: 0,
                    composing: false,
                    utf8: "f",
                    unshifted_codepoint: 'f' as u32,
                }),
                b"\x1bf"
            );
            assert_eq!(
                model.encode_key(KeyInput {
                    action: GHOSTTY_KEY_ACTION_PRESS,
                    key: GHOSTTY_KEY_BACKSPACE,
                    mods,
                    consumed_mods: 0,
                    composing: false,
                    utf8: "",
                    unshifted_codepoint: 0,
                }),
                b"\x1b\x7f"
            );
        }

        assert_eq!(
            model.encode_key(KeyInput {
                action: GHOSTTY_KEY_ACTION_PRESS,
                key: GHOSTTY_KEY_ARROW_UP,
                mods: 0,
                consumed_mods: 0,
                composing: false,
                utf8: "\u{F700}",
                unshifted_codepoint: 0xF700,
            }),
            b"\x1b[A"
        );

        assert!(
            model
                .encode_key(KeyInput {
                    action: GHOSTTY_KEY_ACTION_PRESS,
                    key: GHOSTTY_KEY_BACKSPACE,
                    mods: 0,
                    consumed_mods: 0,
                    composing: true,
                    utf8: "",
                    unshifted_codepoint: 0,
                })
                .is_empty()
        );
        assert!(
            model
                .encode_key(KeyInput {
                    action: GHOSTTY_KEY_ACTION_PRESS,
                    key: GHOSTTY_KEY_ESCAPE,
                    mods: 0,
                    consumed_mods: 0,
                    composing: true,
                    utf8: "",
                    unshifted_codepoint: 0,
                })
                .is_empty()
        );

        model.write(b"\x1b[>1u");
        assert_eq!(
            model.encode_key(KeyInput {
                action: GHOSTTY_KEY_ACTION_PRESS,
                key: GHOSTTY_KEY_ENTER,
                mods: GHOSTTY_MODS_SHIFT as GhosttyMods,
                consumed_mods: 0,
                composing: false,
                utf8: "",
                unshifted_codepoint: 0,
            }),
            b"\x1b[13;2u"
        );
    }

    #[test]
    fn style_flags_preserve_emphasis_inverse_and_hidden_text() {
        let mut model = Model::new(40, 8);
        model.write(b"\x1b[1;3;2;9;4:3;58;2;255;80;40mB\x1b[0m\x1b[7mI\x1b[0m\x1b[8mH\x1b[0m");
        let snap = model.render_snapshot("", true);
        let cells = unsafe { std::slice::from_raw_parts(snap.cells, usize::from(snap.cols)) };
        assert_eq!(cells[0].cp, u32::from(b'B'));
        assert_ne!(cells[0].flags & CANBAN_STYLE_BOLD, 0);
        assert_ne!(cells[0].flags & CANBAN_STYLE_ITALIC, 0);
        assert_ne!(cells[0].flags & CANBAN_STYLE_FAINT, 0);
        assert_ne!(cells[0].flags & CANBAN_STYLE_STRIKETHROUGH, 0);
        let ul = (cells[0].flags & CANBAN_STYLE_UNDERLINE) >> CANBAN_STYLE_UNDERLINE_SHIFT;
        assert_eq!(ul, GHOSTTY_SGR_UNDERLINE_CURLY as u16);
        assert_eq!(cells[0].has_ul_color, 1);
        assert_eq!(cells[0].ul_color, [255, 80, 40]);

        let frame = model.frame();
        let row = &frame.rows[0];
        assert_eq!(row[0].text, "B");
        assert!(row[0].bold && row[0].underline);
        assert_eq!(row[1].text, "I");
        assert!(!row[1].bold && !row[1].underline);
        assert_eq!(row[1].fg, row[0].bg);
        assert_eq!(row[1].bg, row[0].fg);
        assert!(row[2].text.is_empty());
    }

    #[test]
    fn selection_unwraps_soft_wrapped_lines_and_osc52_writes_clipboard() {
        let mut model = Model::new(10, 4);
        model.write(b"abcdefghijklmno\r\nsecond");
        assert!(model.select_all());
        assert_eq!(
            model.selected_text().as_deref(),
            Some("abcdefghijklmno\nsecond")
        );
        model.clear_selection();
        assert!(!model.has_selection());

        model.write(b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(model.take_clipboard().as_deref(), Some("hello"));
    }

    #[test]
    fn mouse_sgr_alternate_scroll_and_link_detection_work() {
        let mut model = Model::new(60, 8);
        model.resize_with_metrics(60, 8, 8, 18, 6, 6, 6, 6);
        model.write(
            b"Visit https://example.com/docs(v1). Or \x1b]8;;https://canban.dev\x07Canban\x1b]8;;\x07!",
        );
        let plain = model.link_at(10, 0).expect("plain URL");
        assert_eq!(plain.uri, "https://example.com/docs(v1)");
        assert_eq!((plain.start_col, plain.end_col), (6, 33));

        let osc8 = model.link_at(41, 0).expect("osc8 link");
        assert_eq!(osc8.uri, "https://canban.dev");
        assert_eq!((osc8.start_col, osc8.end_col), (39, 44));

        model.write(b"\x1b[?1049h\x1b[?1007h");
        assert_eq!(
            model.wheel(-2, 0, 20.0, 20.0, 0, false),
            WheelOutput::Pty(b"\x1b[A\x1b[A".to_vec())
        );

        model.write(b"\x1b[?1000h\x1b[?1006h");
        assert!(model.mouse_tracking());
        let press = model.encode_mouse(MouseInput {
            action: GHOSTTY_MOUSE_ACTION_PRESS,
            button: Some(GHOSTTY_MOUSE_BUTTON_LEFT),
            mods: 0,
            x_px: 10.0,
            y_px: 10.0,
            any_button_pressed: true,
        });
        assert_eq!(press, b"\x1b[<0;1;1M");
    }

    #[test]
    fn theme_updates_defaults_without_resetting_content_or_geometry() {
        let mut model = Model::new(40, 8);
        model.write(b"Keep this output\r\nsecond line");
        model.theme(&serde_json::json!({"foreground":[220,230,240],"background":[16,25,20],"cursor":[80,190,130]}));
        assert!(text(&mut model).contains("Keep this output"));
        assert_eq!((model.cols, model.height), (40, 8));
        assert_eq!(model.frame().background, [16, 25, 20]);
        assert_eq!(model.frame().cursor_color, [80, 190, 130]);
        model.theme(&serde_json::json!({"foreground":[27,37,51],"background":[255,255,255]}));
        assert!(text(&mut model).contains("second line"));
        assert_eq!(model.frame().background, [255, 255, 255]);
    }

    #[test]
    fn redraw_unicode_alternate_screen_and_bracketed_paste() {
        let mut model = Model::new(40, 8);
        model.write("hello 世界".as_bytes());
        assert!(text(&mut model).contains("hello 世界"));
        model.write(b"\r\x1b[2Kchanged");
        assert!(text(&mut model).contains("changed"));
        assert!(!text(&mut model).contains("hello"));
        model.write(b"\x1b[?1049hother");
        assert!(!text(&mut model).contains("changed"));
        model.write(b"\x1b[?1049l");
        assert!(text(&mut model).contains("changed"));
        model.write(b"\x1b[?2004h");
        assert_eq!(model.paste("a\nb"), b"\x1b[200~a\nb\x1b[201~");
        model.write(b"\x1b[?25l");
        assert!(model.frame().cursor.is_none());
        model.resize(60, 12);
        assert_eq!(model.frame().rows.len(), 12);
    }
}
