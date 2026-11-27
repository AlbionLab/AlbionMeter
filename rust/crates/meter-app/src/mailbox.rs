//! How feed events reach the UI thread. The engine publishes as fast as it processes; a machine that draws slower than
//! that must not fall further and further behind. Events wait here, and a state that only matters as its latest value
//! (the record snapshot, the capture, AlbionLab and device status, an open dungeon run) replaces the one still waiting
//! instead of queueing behind it. The UI then always draws the newest state, at whatever pace it can; nothing the
//! engine counts is lost, since every snapshot is the whole state of the record. One wake-up is pending at a time.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::feed::FeedEvent;

#[derive(Default)]
pub struct Mailbox {
    pending: Mutex<Vec<FeedEvent>>,
    scheduled: AtomicBool,
}

/// True when `next` makes `waiting` pointless to show.
fn replaces(waiting: &FeedEvent, next: &FeedEvent) -> bool {
    match (waiting, next) {
        (FeedEvent::Snapshot(_), FeedEvent::Snapshot(_))
        | (FeedEvent::Capture(_), FeedEvent::Capture(_))
        | (FeedEvent::Lab(_), FeedEvent::Lab(_))
        | (FeedEvent::Devices(_), FeedEvent::Devices(_))
        | (FeedEvent::Dungeons(_), FeedEvent::Dungeons(_)) => true,
        (FeedEvent::Dungeon(waiting), FeedEvent::Dungeon(next)) => waiting.summary.id == next.summary.id,
        _ => false,
    }
}

fn push(pending: &mut Vec<FeedEvent>, event: FeedEvent) {
    pending.retain(|waiting| !replaces(waiting, &event));
    pending.push(event);
}

impl Mailbox {
    /// From the feed thread: leaves `event` for the UI and wakes it if it is not already due to look.
    pub fn post(self: &Arc<Self>, event: FeedEvent, deliver: fn(FeedEvent)) {
        push(&mut self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()), event);
        if !self.scheduled.swap(true, Ordering::AcqRel) {
            let mailbox = self.clone();
            let _ = slint::invoke_from_event_loop(move || {
                // Cleared before taking the events: one posted meanwhile schedules its own wake-up.
                mailbox.scheduled.store(false, Ordering::Release);
                let events = std::mem::take(&mut *mailbox.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
                for event in events {
                    deliver(event);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::{CaptureStatus, Snapshot};

    fn snapshot(sequence: u64) -> FeedEvent {
        FeedEvent::Snapshot(Arc::new(Snapshot { sequence, ..Snapshot::default() }))
    }

    #[test]
    fn a_waiting_state_is_replaced_and_everything_else_keeps_its_order() {
        let mut pending = vec![];
        push(&mut pending, snapshot(1));
        push(&mut pending, FeedEvent::CommandDone(None));
        push(&mut pending, FeedEvent::Capture(Arc::new(CaptureStatus::default())));
        push(&mut pending, snapshot(2));
        push(&mut pending, FeedEvent::CommandDone(Some("x".into())));
        push(&mut pending, snapshot(3));
        let kinds: Vec<String> = pending
            .iter()
            .map(|event| match event {
                FeedEvent::Snapshot(snapshot) => format!("snapshot {}", snapshot.sequence),
                FeedEvent::CommandDone(error) => format!("done {error:?}"),
                FeedEvent::Capture(_) => "capture".into(),
                _ => "other".into(),
            })
            .collect();
        assert_eq!(kinds, ["done None", "capture", "done Some(\"x\")", "snapshot 3"]);
    }
}
