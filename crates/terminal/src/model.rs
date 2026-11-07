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
}
struct Shared {
    replies: Vec<u8>,
    size: GhosttySizeReportSize,
}
pub struct Model {
    term: GhosttyTerminal,
    state: GhosttyRenderState,
    rows: GhosttyRenderStateRowIterator,
    cells: GhosttyRenderStateRowCells,
    keys: GhosttyKeyEncoder,
    event: GhosttyKeyEvent,
    shared: Box<Shared>,
    pub cols: u16,
    pub height: u16,
    pub revision: u64,
}
// A model is never concurrently accessed: the viewer's worker and native host
// own it behind a mutex. libghostty-vt has no UI-thread affinity.
unsafe impl Send for Model {}
fn ok(r: GhosttyResult) {
    assert_eq!(r, GHOSTTY_SUCCESS, "libghostty-vt operation failed");
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
impl Model {
    pub fn new(cols: u16, rows: u16) -> Self {
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
            let mut shared = Box::new(Shared {
                replies: Vec::new(),
                size: GhosttySizeReportSize {
                    rows,
                    columns: cols,
                    cell_width: 8,
                    cell_height: 18,
                },
            });
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_USERDATA,
                &mut *shared as *mut _ as *const c_void,
            );
            let callback: GhosttyTerminalWritePtyFn = Some(reply);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                std::mem::transmute::<GhosttyTerminalWritePtyFn, *const c_void>(callback),
            );
            let callback: GhosttyTerminalSizeFn = Some(size);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_SIZE,
                std::mem::transmute::<GhosttyTerminalSizeFn, *const c_void>(callback),
            );
            let max: usize = 10_000_000;
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
                &max as *const _ as *const c_void,
            );
            Self {
                term,
                state,
                rows: rows_it,
                cells,
                keys,
                event,
                shared,
                cols,
                height: rows,
                revision: 0,
            }
        }
    }
    pub fn write(&mut self, data: &[u8]) {
        unsafe { ghostty_terminal_vt_write(self.term, data.as_ptr(), data.len()) };
        self.revision += 1;
    }
    pub fn replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.shared.replies)
    }
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 || (cols, rows) == (self.cols, self.height) {
            return;
        }
        self.cols = cols;
        self.height = rows;
        self.shared.size.columns = cols;
        self.shared.size.rows = rows;
        unsafe { ghostty_terminal_resize(self.term, cols, rows, 8, 18) };
        self.revision += 1;
    }
    pub fn scroll(&mut self, delta: isize) {
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
    pub fn paste(&self, text: &str) -> Vec<u8> {
        unsafe {
            let mut cfg = GhosttyTerminalModeConfig {
                mode: 2004,
                value: false,
            };
            ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_MODE,
                &mut cfg as *mut _ as *mut c_void,
            );
            let enabled = cfg.value;
            if enabled {
                [
                    b"\x1b[200~".as_slice(),
                    text.replace("\x1b[201~", "").as_bytes(),
                    b"\x1b[201~",
                ]
                .concat()
            } else {
                text.replace('\n', "\r").into_bytes()
            }
        }
    }
    pub fn key(&mut self, key: GhosttyKey, mods: GhosttyMods, text: Option<&str>) -> Vec<u8> {
        unsafe {
            ok(ghostty_key_encoder_set_from_terminal(self.keys, self.term));
            ghostty_key_event_set_action(self.event, GHOSTTY_KEY_ACTION_PRESS);
            ghostty_key_event_set_key(self.event, key);
            ghostty_key_event_set_mods(self.event, mods);
            ghostty_key_event_set_consumed_mods(
                self.event,
                if text.is_some() {
                    mods & GHOSTTY_MODS_SHIFT as u16
                } else {
                    0
                },
            );
            let text = text.unwrap_or("");
            ok(ghostty_key_event_set_utf8(
                self.event,
                GhosttyString {
                    ptr: text.as_ptr(),
                    len: text.len(),
                },
            ));
            ghostty_key_event_set_unshifted_codepoint(
                self.event,
                text.chars().next().map_or(0, |c| c as u32),
            );
            let mut buf = [0u8; 256];
            let mut len = 0;
            let encoded = if ghostty_key_encoder_encode(
                self.keys,
                self.event,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut len,
            ) == GHOSTTY_SUCCESS
            {
                buf[..len].to_vec()
            } else {
                vec![]
            };
            // Upstream borrows the UTF-8 bytes; do not retain a caller's text.
            ok(ghostty_key_event_set_utf8(
                self.event,
                GhosttyString::default(),
            ));
            encoded
        }
    }
    pub fn frame(&mut self) -> Frame {
        unsafe {
            ghostty_render_state_update(self.state, self.term);
            let mut colors = GhosttyRenderStateColors {
                size: std::mem::size_of::<GhosttyRenderStateColors>(),
                ..Default::default()
            };
            ghostty_render_state_get(
                self.state,
                GHOSTTY_RENDER_STATE_DATA_COLORS,
                &mut colors as *mut _ as *mut c_void,
            );
            let rgb = |c: GhosttyColorRgb| [c.r, c.g, c.b];
            let background = rgb(colors.background);
            let mut frame = Frame {
                background,
                ..Default::default()
            };
            ghostty_render_state_get(
                self.state,
                GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                &mut self.rows as *mut _ as *mut c_void,
            );
            while ghostty_render_state_row_iterator_next(self.rows) {
                ghostty_render_state_row_get(
                    self.rows,
                    GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                    &mut self.cells as *mut _ as *mut c_void,
                );
                let mut row = vec![];
                while ghostty_render_state_row_cells_next(self.cells) {
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
                    let mut style = GhosttyStyle::default();
                    ghostty_render_state_row_cells_get(
                        self.cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                        &mut style as *mut _ as *mut c_void,
                    );
                    let mut fg = colors.foreground;
                    ghostty_render_state_row_cells_get(
                        self.cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                        &mut fg as *mut _ as *mut c_void,
                    );
                    let mut bg = colors.background;
                    ghostty_render_state_row_cells_get(
                        self.cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                        &mut bg as *mut _ as *mut c_void,
                    );
                    if style.flags & CANBAN_STYLE_INVERSE != 0 {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                    let mut len = 0u32;
                    ghostty_render_state_row_cells_get(
                        self.cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
                        &mut len as *mut _ as *mut c_void,
                    );
                    let mut cps = vec![0u32; len as usize];
                    if len > 0 {
                        let mut graphemes = GhosttyCodepointBuffer {
                            ptr: cps.as_mut_ptr(),
                            cap: cps.len(),
                            len: 0,
                        };
                        ok(ghostty_render_state_row_cells_get(
                            self.cells,
                            GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
                            &mut graphemes as *mut _ as *mut c_void,
                        ));
                    }
                    let text = if style.flags & CANBAN_STYLE_INVISIBLE != 0
                        || wide == GHOSTTY_CELL_WIDE_SPACER_TAIL
                        || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD
                    {
                        String::new()
                    } else {
                        cps.into_iter().filter_map(char::from_u32).collect()
                    };
                    row.push(Cell {
                        text,
                        fg: rgb(fg),
                        bg: rgb(bg),
                        bold: style.flags & CANBAN_STYLE_BOLD != 0,
                        underline: style.flags & CANBAN_STYLE_UNDERLINE != 0,
                        tail: wide == GHOSTTY_CELL_WIDE_SPACER_TAIL,
                    });
                }
                frame.rows.push(row);
            }
            let mut cursor = GhosttyRenderStateCursor {
                size: std::mem::size_of::<GhosttyRenderStateCursor>(),
                ..Default::default()
            };
            ghostty_render_state_get(
                self.state,
                GHOSTTY_RENDER_STATE_DATA_CURSOR,
                &mut cursor as *mut _ as *mut c_void,
            );
            if cursor.visible && cursor.viewport_has_value {
                frame.cursor = Some((cursor.viewport_x, cursor.viewport_y));
            }
            ghostty_render_state_clean(self.state);
            frame
        }
    }
}
impl Drop for Model {
    fn drop(&mut self) {
        unsafe {
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
    fn style_flags_preserve_emphasis_inverse_and_hidden_text() {
        let mut model = Model::new(40, 8);
        model.write(b"\x1b[1;4mB\x1b[0m\x1b[7mI\x1b[0m\x1b[8mH\x1b[0m");
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
