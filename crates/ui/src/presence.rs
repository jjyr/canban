//! Active viewer presence lease synchronization (`viewer.presence`).
//! Protects viewed Cards and managed Terminals from auto-reclaim without SQLite writes.

use crate::presenter::Ui;
use canban_types::Snapshot;
use nsh_core::{platform::Lifecycle, prelude::*};
use serde_json::json;
use std::{
    sync::{Mutex, OnceLock},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresenceTarget {
    pub host_id: String,
    pub card_id: Option<String>,
    pub terminal_id: Option<String>,
}

fn client_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(canban_types::uuid_v4).as_str()
}

fn active_target_slot() -> &'static Mutex<Option<PresenceTarget>> {
    static TARGET: OnceLock<Mutex<Option<PresenceTarget>>> = OnceLock::new();
    TARGET.get_or_init(Default::default)
}

fn last_sent_slot() -> &'static Mutex<Option<PresenceTarget>> {
    static SENT: OnceLock<Mutex<Option<PresenceTarget>>> = OnceLock::new();
    SENT.get_or_init(Default::default)
}

fn publish_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(Default::default)
}

/// Resolves which Card and/or Terminal the active window is currently viewing.
/// Returns `None` when the window is inactive/blurred or on non-Card/non-Terminal pages.
#[allow(clippy::too_many_arguments)]
pub fn resolve_presence(
    snapshot: &Snapshot,
    page: &str,
    card: &str,
    card_terminal_open: bool,
    card_tab: &str,
    terminal_selection: &str,
    narrow: bool,
    terminal_sidebar: bool,
    active_window: bool,
) -> Option<PresenceTarget> {
    if !active_window {
        return None;
    }
    match page {
        "card" => {
            let cid = card.trim();
            if cid.is_empty() {
                return None;
            }
            let tid = card_tab.trim();
            Some(PresenceTarget {
                host_id: snapshot.card_host(cid).to_owned(),
                card_id: Some(cid.to_owned()),
                terminal_id: (card_terminal_open && !tid.is_empty()).then(|| tid.to_owned()),
            })
        }
        "terminals" | "terminal" => {
            if narrow && terminal_sidebar {
                return None;
            }
            let tid = terminal_selection.trim();
            if tid.is_empty() {
                return None;
            }
            Some(PresenceTarget {
                host_id: snapshot.terminal_host(tid).to_owned(),
                card_id: None,
                terminal_id: Some(tid.to_owned()),
            })
        }
        _ => None,
    }
}

fn send_presence(host_id: &str, card_id: Option<&str>, terminal_id: Option<&str>) {
    if !crate::backend::is_connected() {
        return;
    }
    let payload = json!({
        "client_id": client_id(),
        "host_id": host_id,
        "card_id": card_id,
        "terminal_id": terminal_id,
    });
    let _ = crate::routed_call("viewer.presence", payload, None);
}

fn flush_presence_sync(heartbeat: bool) {
    if !crate::backend::is_connected() {
        return;
    }
    let _guard = publish_lock().lock().unwrap_or_else(|e| e.into_inner());
    let desired = active_target_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let prev = last_sent_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if !heartbeat && prev == desired {
        return;
    }
    if let Some(old) = &prev
        && desired.as_ref().is_none_or(|d| d.host_id != old.host_id)
    {
        send_presence(&old.host_id, None, None);
    }
    if let Some(next) = &desired {
        send_presence(
            &next.host_id,
            next.card_id.as_deref(),
            next.terminal_id.as_deref(),
        );
    }
    *last_sent_slot().lock().unwrap_or_else(|e| e.into_inner()) = desired;
}

pub fn init(ui: Ui) {
    if !crate::backend::is_connected() {
        return;
    }
    let current_target = computed(move || {
        if !ui.snapshot_ready.get() {
            return None;
        }
        let page = ui.page.get();
        let card = ui.card.get();
        let card_terminal_open = ui.card_terminal_open.get();
        let card_tab = ui.card_tab.get();
        let terminal_selection = ui.terminal_selection.get();
        let narrow = ui.narrow.get();
        let terminal_sidebar = ui.terminal_sidebar.get();
        let active_window = lifecycle() == Lifecycle::Active;
        ui.snapshot.with_untracked(|s| {
            resolve_presence(
                s,
                &page,
                &card,
                card_terminal_open,
                &card_tab,
                &terminal_selection,
                narrow,
                terminal_sidebar,
                active_window,
            )
        })
    });

    effect(move || {
        let next = current_target.get();
        let changed = {
            let mut slot = active_target_slot()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if *slot == next {
                false
            } else {
                *slot = next;
                true
            }
        };
        if changed {
            ui.spawn(async move {
                let _ = blocking(move || flush_presence_sync(false)).await;
            });
        }
    });

    set_interval(Duration::from_secs(10), move || {
        if lifecycle() != Lifecycle::Active {
            return;
        }
        let has_target = active_target_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if has_target {
            ui.spawn(async move {
                let _ = blocking(move || flush_presence_sync(true)).await;
            });
        }
    });
}

#[cfg(feature = "local-host")]
pub(crate) fn clear_blocking() {
    *active_target_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    flush_presence_sync(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_presence_covers_card_split_collapsed_terminals_narrow_and_blur() {
        let mut s = Snapshot::default();
        s.entity_hosts.insert("card-1".into(), "remote-a".into());
        s.entity_hosts.insert("term-2".into(), "remote-b".into());

        // Card open with split pane collapsed still protects the Card's managed terminals.
        assert_eq!(
            resolve_presence(&s, "card", "card-1", false, "t1", "", false, false, true),
            Some(PresenceTarget {
                host_id: "remote-a".into(),
                card_id: Some("card-1".into()),
                terminal_id: None,
            })
        );

        // Card open with split pane expanded protects both Card and active split terminal.
        assert_eq!(
            resolve_presence(&s, "card", "card-1", true, "t1", "", false, false, true),
            Some(PresenceTarget {
                host_id: "remote-a".into(),
                card_id: Some("card-1".into()),
                terminal_id: Some("t1".into()),
            })
        );

        // Terminals page protects the selected terminal on its owning host.
        assert_eq!(
            resolve_presence(&s, "terminals", "", false, "", "term-2", false, true, true),
            Some(PresenceTarget {
                host_id: "remote-b".into(),
                card_id: None,
                terminal_id: Some("term-2".into()),
            })
        );

        // Narrow sidebar covering the terminal canvas drops terminal presence protection.
        assert_eq!(
            resolve_presence(&s, "terminals", "", false, "", "term-2", true, true, true),
            None
        );

        // Board page or blurred/inactive window releases presence protection.
        assert_eq!(
            resolve_presence(
                &s, "board", "card-1", true, "t1", "term-2", false, false, true
            ),
            None
        );
        assert_eq!(
            resolve_presence(&s, "card", "card-1", true, "t1", "", false, false, false),
            None
        );
    }
}
