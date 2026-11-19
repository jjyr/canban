//! Local presentation/delivery of the shared durable reminder projection.
use crate::presenter::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use canban_types::{CardRead, Snapshot};
use nsh_core::{platform::Lifecycle, prelude::*};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn foreground() -> bool {
    lifecycle() == Lifecycle::Active
}

pub fn sound_enabled(ui: Ui) -> bool {
    ui.utility.with(|v| {
        v["preferences"]["notification_sound"]
            .as_bool()
            .unwrap_or(true)
    })
}
pub fn init(ui: Ui) {
    let sound = stored(None::<Sound>);
    spawn(async move {
        let uri = format!(
            "data:audio/wav;base64,{}",
            STANDARD.encode(include_bytes!("../assets/notification.wav"))
        );
        match load_sound(&uri).await {
            Ok(value) => {
                if std::env::var_os("CANBAN_NOTIFICATION_TRACE").is_some() {
                    eprintln!(
                        "notification sound ready duration_ms={}",
                        value.duration().as_millis()
                    );
                }
                sound.replace(Some(value));
            }
            Err(e) => eprintln!("Canban notification sound unavailable: {e:?}"),
        }
    });
    let pending = stored(BTreeSet::<String>::new());
    let scheduled = stored(false);
    effect(move || {
        let items = ui.sound_items.get();
        let new = ui.notification_delivery.with(|t| t.observe(&items));
        if new.is_empty() {
            return;
        }
        // Consume muted deliveries too; re-enabling sound cannot replay them.
        if !sound_enabled(ui) {
            return;
        }
        pending.with(|ids| ids.extend(new));
        if scheduled.replace(true) {
            return;
        }
        ui.owner.run(move || {
            set_timeout(std::time::Duration::from_millis(650), move || {
                scheduled.replace(false);
                let ids = pending.replace(BTreeSet::new());
                let remains = ui
                    .sound_items
                    .with_untracked(|items| items.iter().any(|id| ids.contains(id)));
                if remains && sound_enabled(ui) {
                    sound.with(|s| {
                        if let Some(s) = s {
                            s.play(0.45, 1.);
                            if std::env::var_os("CANBAN_NOTIFICATION_TRACE").is_some() {
                                eprintln!("notification sound delivered items={}", ids.len());
                            }
                        }
                    });
                }
            })
        });
    });
}

#[derive(Clone, Copy)]
pub(crate) struct Reader {
    owner: Owner,
    rows: Stored<BTreeMap<u64, Bounds<Points>>>,
    viewport: Stored<(f32, f32)>,
    scheduled: Stored<bool>,
    writing: Signal<bool>,
}
impl Reader {
    pub fn new(offset: f32) -> Self {
        Self {
            owner: Owner::current(),
            rows: stored(BTreeMap::new()),
            viewport: stored((offset, 0.)),
            scheduled: stored(false),
            writing: signal(false),
        }
    }
    pub fn row(self, ui: Ui, sequence: u64, bounds: Bounds<Points>) {
        self.rows.with(|rows| {
            rows.insert(sequence, bounds);
        });
        self.check(ui);
    }
    pub fn viewport(self, ui: Ui, offset: f32, height: f32) {
        self.viewport.replace((offset, height));
        self.check(ui);
    }
    pub fn height(self, ui: Ui, height: f32) {
        self.viewport.with(|v| v.1 = height);
        self.check(ui);
    }
    pub fn check(self, ui: Ui) {
        if self.scheduled.replace(true) {
            return;
        }
        // Event-triggered dwell, not a scan. Wait past the panel reveal and
        // batch scroll/layout/foreground changes before publishing a position.
        self.owner.run(move || {
            set_timeout(std::time::Duration::from_millis(260), move || {
                self.scheduled.replace(false);
                if std::env::var_os("CANBAN_NOTIFICATION_TRACE").is_some() {
                    eprintln!(
                        "notification reader card={} lifecycle={:?} viewport={:?} rows={:?}",
                        untracked(|| ui.card_id()),
                        lifecycle(),
                        self.viewport.get(),
                        self.rows.with(|r| r
                            .iter()
                            .map(|(s, b)| (*s, b.origin.y.0, b.size.height.0))
                            .collect::<Vec<_>>())
                    );
                }
                if !foreground()
                    || !matches!(ui.page.get_untracked().as_str(), "card" | "review")
                    || (ui.page.get_untracked() == "card"
                        && ui.narrow.get_untracked()
                        && ui.card_terminal_open.get_untracked())
                    || untracked(|| ui.card_id()) != ui.card.get_untracked()
                    || ui.quick_open.get_untracked()
                    || !ui.utility_popup.get_untracked().is_empty()
                    || ui.native_modal.get_untracked()
                    || self.writing.get_untracked()
                {
                    return;
                }
                let card = untracked(|| ui.card_id());
                let (offset, height) = self.viewport.get();
                if height <= 0. {
                    return;
                }
                let sequence = ui.snapshot.with_untracked(|s| {
                    let through = canban_notifications::read_sequence(s, &card);
                    ui.card_timelines.with_untracked(|timelines| {
                        let card_events = timelines
                            .get(&card)
                            .map(|st| st.events.as_slice())
                            .unwrap_or(&[]);
                        self.rows.with(|rows| {
                            rows.iter()
                                .filter(|(seq, b)| {
                                    **seq > through
                                        && visible(b.origin.y.0, b.size.height.0, offset, height)
                                })
                                .filter(|(seq, _)| {
                                    card_events.iter().any(|e| {
                                        e.sequence == **seq
                                            && canban_notifications::message_id_in_card(
                                                card_events,
                                                e,
                                            )
                                            .is_some()
                                    })
                                })
                                .map(|(seq, _)| *seq)
                                .max()
                        })
                    })
                });
                let Some(sequence) = sequence else {
                    return;
                };
                self.writing.set(true);
                if crate::APP.get().is_none() {
                    ui.snapshot.update(|s| {
                        let project = s
                            .cards
                            .iter()
                            .find(|c| c.id == card)
                            .map(|c| c.project_id.clone())
                            .unwrap_or_default();
                        apply_read(
                            s,
                            CardRead {
                                id: card.clone(),
                                project_id: project,
                                card_id: card,
                                sequence,
                            },
                        );
                    });
                    self.writing.set(false);
                    return;
                }
                ui.spawn(async move {
                    let result = blocking(move || {
                        crate::routed_call(
                            "notifications.read",
                            serde_json::json!({"card_id":card,"sequence":sequence}),
                            None,
                        )
                    })
                    .await;
                    self.writing.try_set(false);
                    match result {
                        Ok(Ok(value)) => {
                            if let Ok(read) = serde_json::from_value(value) {
                                ui.snapshot.update(|s| apply_read(s, read));
                            }
                        }
                        other => eprintln!("Canban could not save read position: {other:?}"),
                    }
                });
            })
        });
    }
}
fn visible(y: f32, h: f32, offset: f32, height: f32) -> bool {
    (y + h).min(offset + height) - y.max(offset) >= h.clamp(1., 24.)
}
fn apply_read(snapshot: &mut Snapshot, read: CardRead) {
    if let Some(old) = snapshot
        .card_reads
        .iter_mut()
        .find(|r| r.card_id == read.card_id)
    {
        old.sequence = old.sequence.max(read.sequence);
    } else {
        snapshot.card_reads.push(read);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visible_message_requires_real_viewport_overlap() {
        assert!(!visible(800., 100., 0., 500.));
        assert!(!visible(0., 100., 200., 500.));
        assert!(visible(350., 300., 200., 500.));
        assert!(!visible(499., 100., 0., 500.));
    }
}
