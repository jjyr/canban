//! Platform-neutral terminal input, selection, link-matching, and session-state helpers
//! shared across native Apple views (`apple::macos`) and portable views (`portable`).
use crate::{
    ffi::*,
    model::{KeyInput, LinkMatch, Model, MouseInput, WheelOutput},
    session::{self, Input, State, display_facts_for_model},
};
use serde_json::Value;
use std::sync::{Condvar, Mutex, mpsc};

/// Forwards user-initiated input bytes to the live PTY worker.
pub fn send_bytes(input: Option<&mpsc::Sender<Input>>, bytes: Vec<u8>) {
    if bytes.is_empty() {
        return;
    }
    if let Some(tx) = input {
        let _ = tx.send(Input::Bytes(bytes));
    }
}

/// Forwards automatic VT query responses (`DA`, `DSR`, Kitty replies, etc.) to the PTY worker.
pub fn send_auto_bytes(input: Option<&mpsc::Sender<Input>>, bytes: Vec<u8>) {
    if bytes.is_empty() {
        return;
    }
    if let Some(tx) = input {
        let _ = tx.send(Input::AutoBytes(bytes));
    }
}

/// Updates the session model theme palette, drains any queued VT replies, and forwards
/// updated `DisplayFacts` when connected. Returns whether the theme actually changed.
pub fn apply_theme(
    state: &Mutex<State>,
    input: Option<&mpsc::Sender<Input>>,
    next: &Value,
) -> bool {
    let (changed, facts_to_send) = {
        let mut s = state.lock().unwrap();
        if s.theme.as_ref() == Some(next) {
            return false;
        }
        s.model.theme(next);
        s.theme = Some(next.clone());
        let _ = s.model.replies();
        let facts = s
            .connected
            .then(|| display_facts_for_model(&s.model, s.theme.as_ref()));
        (true, facts)
    };
    if let Some(facts) = facts_to_send
        && let Some(tx) = input
    {
        let _ = tx.send(Input::Facts(facts));
    }
    changed
}

/// Applies focus state to a session model and emits DEC focus reporting events if enabled.
/// Returns whether focus state changed.
pub fn apply_focus(
    state: &Mutex<State>,
    input: Option<&mpsc::Sender<Input>>,
    read_only: bool,
    focused: bool,
) -> bool {
    let (changed, focus_seq) = {
        let mut s = state.lock().unwrap();
        let changed = s.focused != focused;
        s.focused = focused;
        let seq = if changed && session::accepts_pty_input(&s, read_only) {
            s.model.focus_sequence(focused)
        } else {
            Vec::new()
        };
        (changed, seq)
    };
    if !changed {
        return false;
    }
    if let Some(tx) = input {
        let _ = tx.send(Input::Focus(focused));
        if !focus_seq.is_empty() {
            let _ = tx.send(Input::Bytes(focus_seq));
        }
    }
    true
}

/// Applies viewport geometry (`cols`, `rows`, cell size, padding) and explicit millipoint cell
/// metrics to the shared session model, unblocks the attach worker when size becomes ready, and
/// forwards `Resize` / `Facts` / `AutoBytes`. Returns whether the grid dimensions changed.
#[allow(clippy::too_many_arguments)]
pub fn apply_resize_with_cell_milli(
    state: &Mutex<State>,
    size_cv: &Condvar,
    input: Option<&mpsc::Sender<Input>>,
    cols: u16,
    rows: u16,
    cell_width: u32,
    cell_height: u32,
    cell_milli: (u32, u32),
    pads: (u32, u32, u32, u32),
) -> bool {
    let cols = cols.clamp(2, 500);
    let rows = rows.clamp(1, 200);
    session::record_viewport_size(cols, rows);
    session::record_cell_metrics_milli(
        cell_milli.0.max(cell_width.max(1) * 1000),
        cell_milli.1.max(cell_height.max(1) * 1000),
    );
    let (grid_changed, facts, became_ready, replies) = {
        let mut s = state.lock().unwrap();
        let became_ready = !s.size_ready;
        let prev_cell = s.model.cell_size();
        s.size_ready = true;
        let grid_changed = s.model.resize_with_metrics(
            cols,
            rows,
            cell_width.max(1),
            cell_height.max(1),
            pads.0,
            pads.1,
            pads.2,
            pads.3,
        );
        let facts = if s.connected && s.model.cell_size() != prev_cell {
            Some(display_facts_for_model(&s.model, s.theme.as_ref()))
        } else {
            None
        };
        let replies = s.model.replies();
        (grid_changed, facts, became_ready, replies)
    };
    if became_ready {
        size_cv.notify_all();
    }
    if let Some(tx) = input {
        if grid_changed {
            let _ = tx.send(Input::Resize(cols, rows));
        }
        if let Some(facts) = facts {
            let _ = tx.send(Input::Facts(facts));
        }
        send_auto_bytes(Some(tx), replies);
    }
    grid_changed
}

/// Applies viewport geometry (`cols`, `rows`, cell size, padding) to the shared session model,
/// unblocks the attach worker when size becomes ready, and forwards `Resize` / `Facts` / `AutoBytes`.
/// Returns whether the grid dimensions changed.
#[allow(clippy::too_many_arguments)]
pub fn apply_resize(
    state: &Mutex<State>,
    size_cv: &Condvar,
    input: Option<&mpsc::Sender<Input>>,
    cols: u16,
    rows: u16,
    cell_width: u32,
    cell_height: u32,
    pads: (u32, u32, u32, u32),
) -> bool {
    apply_resize_with_cell_milli(
        state,
        size_cv,
        input,
        cols,
        rows,
        cell_width,
        cell_height,
        (cell_width.max(1) * 1000, cell_height.max(1) * 1000),
        pads,
    )
}

/// Commits UTF-8 text (IME / direct text input) into the session if writable.
pub fn commit_text(
    state: &Mutex<State>,
    input: Option<&mpsc::Sender<Input>>,
    read_only: bool,
    text: &str,
) -> bool {
    if text.is_empty() || read_only {
        return false;
    }
    let bytes = {
        let mut s = state.lock().unwrap();
        if !session::accepts_pty_input(&s, read_only) {
            return false;
        }
        s.clear_attach_settle();
        s.model.commit_text(text)
    };
    let sent = !bytes.is_empty();
    send_bytes(input, bytes);
    sent
}

/// Encodes pasted text (respecting bracketed paste mode) and sends it to the PTY if writable.
pub fn paste_text(
    state: &Mutex<State>,
    input: Option<&mpsc::Sender<Input>>,
    read_only: bool,
    text: &str,
) -> bool {
    if text.is_empty() || read_only {
        return false;
    }
    let bytes = {
        let mut s = state.lock().unwrap();
        if !session::accepts_pty_input(&s, read_only) {
            return false;
        }
        s.clear_attach_settle();
        s.model.paste(text)
    };
    let sent = !bytes.is_empty();
    send_bytes(input, bytes);
    sent
}

/// Shell-escapes local filesystem paths and pastes them into the PTY if writable.
pub fn paste_paths<I, S>(
    state: &Mutex<State>,
    input: Option<&mpsc::Sender<Input>>,
    read_only: bool,
    paths: I,
) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let formatted = crate::shell_escape_paths(paths);
    if formatted.is_empty() {
        return false;
    }
    paste_text(state, input, read_only, &formatted)
}

/// Clears selection and sends Ctrl+L (`\x0c`) to the PTY when writable.
pub fn clear_screen(
    state: &Mutex<State>,
    input: Option<&mpsc::Sender<Input>>,
    read_only: bool,
) -> bool {
    let can_send = {
        let mut s = state.lock().unwrap();
        s.model.clear_selection();
        session::accepts_pty_input(&s, read_only)
    };
    if can_send {
        // Send Ctrl+L (form feed) to clear active shell/TUI cleanly without corrupting prompt state.
        send_bytes(input, b"\x0c".to_vec());
    }
    can_send
}

/// Converts wire key action byte (`0` release, `1` press, `2` repeat) into `GhosttyKeyAction`.
pub fn decode_key_action(action: u8) -> GhosttyKeyAction {
    match action {
        0 => GHOSTTY_KEY_ACTION_RELEASE,
        2 => GHOSTTY_KEY_ACTION_REPEAT,
        _ => GHOSTTY_KEY_ACTION_PRESS,
    }
}

/// Encodes and dispatches a structured key event to the PTY if writable.
pub fn send_key(
    state: &Mutex<State>,
    input: Option<&mpsc::Sender<Input>>,
    read_only: bool,
    key: KeyInput<'_>,
) -> bool {
    let encoded = {
        let mut s = state.lock().unwrap();
        if !session::accepts_pty_input(&s, read_only) {
            return false;
        }
        s.clear_attach_settle();
        s.model.encode_key(key)
    };
    if encoded.is_empty() {
        return false;
    }
    send_bytes(input, encoded);
    true
}

/// Converts wire mouse action byte (`0` press, `1` release, `2` motion) into `GhosttyMouseAction`.
pub fn decode_mouse_action(action: u8) -> GhosttyMouseAction {
    match action {
        1 => GHOSTTY_MOUSE_ACTION_RELEASE,
        2 => GHOSTTY_MOUSE_ACTION_MOTION,
        _ => GHOSTTY_MOUSE_ACTION_PRESS,
    }
}

/// Converts wire mouse button byte (`1` left, `2` right, `3` middle) into `GhosttyMouseButton`.
pub fn decode_mouse_button(button: u8) -> Option<GhosttyMouseButton> {
    match button {
        1 => Some(GHOSTTY_MOUSE_BUTTON_LEFT),
        2 => Some(GHOSTTY_MOUSE_BUTTON_RIGHT),
        3 => Some(GHOSTTY_MOUSE_BUTTON_MIDDLE),
        _ => None,
    }
}

/// Encodes and forwards a mouse event to the PTY when mouse tracking and input are enabled.
pub fn send_mouse(
    state: &Mutex<State>,
    input: Option<&mpsc::Sender<Input>>,
    read_only: bool,
    mouse: MouseInput,
) -> bool {
    let bytes = {
        let mut s = state.lock().unwrap();
        if !session::accepts_pty_input(&s, read_only) || !s.model.mouse_tracking() {
            return false;
        }
        s.model.encode_mouse(mouse)
    };
    send_bytes(input, bytes);
    true
}

/// Processes a scroll wheel step against mouse tracking / alternate scroll / viewport scrollback.
/// Returns `true` if bytes were sent to the PTY.
#[allow(clippy::too_many_arguments)]
pub fn send_wheel(
    state: &Mutex<State>,
    input: Option<&mpsc::Sender<Input>>,
    read_only: bool,
    delta_rows: isize,
    delta_cols: isize,
    x_px: f32,
    y_px: f32,
    mods: u16,
    alt_override: bool,
) -> bool {
    let out = {
        let mut s = state.lock().unwrap();
        s.clear_attach_settle();
        let can_send = session::accepts_pty_input(&s, read_only);
        s.model.wheel(
            delta_rows,
            delta_cols,
            x_px,
            y_px,
            mods,
            alt_override || !can_send,
        )
    };
    if let WheelOutput::Pty(bytes) = out {
        let sent = !bytes.is_empty();
        send_bytes(input, bytes);
        sent
    } else {
        false
    }
}

/// Dispatches selection gesture actions (`0` press, `1` drag, `2` release, `3` autoscroll, `4` all, `5` clear).
#[allow(clippy::too_many_arguments)]
pub fn apply_select(
    model: &mut Model,
    kind: u8,
    x_px: f64,
    y_px: f64,
    time_ns: u64,
    repeat_interval_ns: u64,
    shift_extend: bool,
    rectangle: bool,
) -> i8 {
    match kind {
        0 => {
            model.select_press(
                x_px,
                y_px,
                time_ns,
                repeat_interval_ns,
                shift_extend,
                rectangle,
            );
            0
        }
        1 => model.select_drag(x_px, y_px, rectangle),
        2 => {
            model.select_release(x_px, y_px);
            0
        }
        3 => model.select_autoscroll_tick(x_px, y_px, rectangle),
        4 => {
            model.select_all();
            0
        }
        5 => {
            model.clear_selection();
            0
        }
        _ => 0,
    }
}

/// Resolves an OSC 8 or implicit URL hyperlink span at the given pixel coordinate.
pub fn link_at_px(state: &Mutex<State>, x_px: f64, y_px: f64) -> Option<LinkMatch> {
    state.lock().unwrap().model.link_at_px(x_px, y_px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn commit_paste_paths_clear_and_focus_respect_read_only_and_modes() {
        let mut s = State::new(80, 24);
        s.connected = true;
        s.model.write(b"\x1b[?2004h\x1b[?1004h");
        let _ = s.model.replies();
        let state = Mutex::new(s);
        let (tx, rx) = mpsc::channel();

        assert!(commit_text(&state, Some(&tx), false, "hello"));
        assert!(matches!(rx.try_recv(), Ok(Input::Bytes(b)) if b == b"hello"));

        assert!(!commit_text(&state, Some(&tx), true, "blocked"));
        assert!(rx.try_recv().is_err());

        assert!(paste_text(&state, Some(&tx), false, "ls -la"));
        assert!(matches!(rx.try_recv(), Ok(Input::Bytes(b)) if b == b"\x1b[200~ls -la\x1b[201~"));

        assert!(paste_paths(
            &state,
            Some(&tx),
            false,
            ["/tmp/a.rs", "/tmp/my dir/b.txt"],
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(Input::Bytes(b)) if b == b"\x1b[200~/tmp/a.rs '/tmp/my dir/b.txt' \x1b[201~"
        ));

        assert!(apply_focus(&state, Some(&tx), false, true));
        assert!(matches!(rx.try_recv(), Ok(Input::Focus(true))));
        assert!(matches!(rx.try_recv(), Ok(Input::Bytes(b)) if b == b"\x1b[I"));
        assert!(!apply_focus(&state, Some(&tx), false, true));

        assert!(clear_screen(&state, Some(&tx), false));
        assert!(matches!(rx.try_recv(), Ok(Input::Bytes(b)) if b == b"\x0c"));
    }

    #[test]
    fn resize_theme_key_mouse_and_selection_helpers_emit_expected_events() {
        let mut s = State::new(80, 24);
        s.connected = true;
        s.model.write(b"\x1b[?1000;1006h\x1b[?1007h");
        let _ = s.model.replies();
        let state = Mutex::new(s);
        let cv = Condvar::new();
        let (tx, rx) = mpsc::channel();

        assert!(apply_resize(
            &state,
            &cv,
            Some(&tx),
            100,
            40,
            9,
            19,
            (6, 6, 6, 6)
        ));
        assert!(matches!(rx.try_recv(), Ok(Input::Resize(100, 40))));
        assert!(matches!(
            rx.try_recv(),
            Ok(Input::Facts(f)) if f.cell == Some((9, 19))
        ));

        let theme = json!({
            "foreground": [220, 225, 235],
            "background": [18, 20, 26],
            "cursor": [90, 200, 250]
        });
        assert!(apply_theme(&state, Some(&tx), &theme));
        assert!(matches!(
            rx.try_recv(),
            Ok(Input::Facts(f)) if f.fg == Some([220, 225, 235]) && f.bg == Some([18, 20, 26])
        ));
        assert!(!apply_theme(&state, Some(&tx), &theme));

        assert!(send_key(
            &state,
            Some(&tx),
            false,
            KeyInput {
                action: decode_key_action(1),
                key: GHOSTTY_KEY_ARROW_UP,
                mods: 0,
                consumed_mods: 0,
                composing: false,
                utf8: "",
                unshifted_codepoint: 0,
            }
        ));
        assert!(matches!(rx.try_recv(), Ok(Input::Bytes(b)) if b == b"\x1b[A"));

        assert!(send_mouse(
            &state,
            Some(&tx),
            false,
            MouseInput {
                action: decode_mouse_action(0),
                button: decode_mouse_button(1),
                mods: 0,
                x_px: 12.0,
                y_px: 24.0,
                any_button_pressed: false,
            }
        ));
        assert!(matches!(rx.try_recv(), Ok(Input::Bytes(_))));

        {
            let mut guard = state.lock().unwrap();
            guard.model.write(b"\r\nhello world");
            apply_select(&mut guard.model, 4, 0.0, 0.0, 0, 0, false, false);
            assert!(guard.model.has_selection());
            apply_select(&mut guard.model, 5, 0.0, 0.0, 0, 0, false, false);
            assert!(!guard.model.has_selection());
        }
    }
}
