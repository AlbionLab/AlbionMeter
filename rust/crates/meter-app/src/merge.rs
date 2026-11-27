//! web/src/state.ts mergeSnapshot: which snapshot the UI shows.
//!
//! state.ts also grew the record's whole timeline from the points each snapshot carries; no Slint screen draws the
//! live record's timeline (the journal reads the dungeon runs'), so it is not kept: that copy grew with every second of
//! the fight.

use std::sync::Arc;

use crate::feed::Snapshot;

/// The record the UI shows: the latest snapshot.
#[derive(Default)]
pub struct Record {
    pub snapshot: Option<Arc<Snapshot>>,
}

impl Record {
    /// Applies `next`; false when it is older than what is shown and was dropped.
    pub fn merge(&mut self, next: Arc<Snapshot>) -> bool {
        // The placeholder before any record ("unavailable"), and a record of the other kind (a replay after live, or live
        // after a replay), never hold a snapshot back: a replayed capture starts at the capture's own, earlier, times.
        if let Some(previous) = self.snapshot.as_ref().filter(|previous| &*previous.mode != "unavailable" && previous.mode == next.mode) {
            // A snapshot that started earlier is older, whatever its id: an older record, or the same one before a Reset.
            if previous.started_at > next.started_at {
                return false;
            }
            // Reset keeps the record id but starts it again (a new start time and sequence): that snapshot is not an old one.
            if previous.id == next.id && previous.started_at == next.started_at && previous.sequence > next.sequence {
                return false;
            }
        }
        self.snapshot = Some(next);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(id: &str, started: &str, sequence: u64) -> Arc<Snapshot> {
        Arc::new(Snapshot { id: id.into(), started_at: started.into(), sequence, ..Snapshot::default() })
    }

    fn shown(record: &Record) -> (String, u64) {
        record.snapshot.as_ref().map(|snapshot| (snapshot.id.to_string(), snapshot.sequence)).unwrap_or_default()
    }

    #[test]
    fn a_replay_of_an_older_capture_replaces_the_placeholder_and_the_live_record() {
        let mut record = Record::default();
        let mut placeholder = (*snapshot("p", "2026-11-27T03:55:00Z", 0)).clone();
        placeholder.mode = "unavailable".into();
        assert!(record.merge(Arc::new(placeholder)));
        let mut replay = (*snapshot("r", "1970-01-01T00:00:02Z", 49)).clone();
        replay.mode = "replay".into();
        assert!(record.merge(Arc::new(replay.clone())));
        assert_eq!(shown(&record), ("r".into(), 49));
        let mut live = (*snapshot("l", "2026-11-27T03:56:00Z", 1)).clone();
        live.mode = "live".into();
        assert!(record.merge(Arc::new(live)));
        assert!(record.merge(Arc::new(replay)), "a replay started after live shows");
    }

    #[test]
    fn drops_stale_snapshots() {
        let mut record = Record::default();
        assert!(record.merge(snapshot("a", "2026-01-01T10:00:00Z", 1)));
        assert!(record.merge(snapshot("a", "2026-01-01T10:00:00Z", 2)));
        assert!(!record.merge(snapshot("a", "2026-01-01T10:00:00Z", 1)));
        assert_eq!(shown(&record), ("a".into(), 2));
        // Reset: same id, later start, lower sequence.
        assert!(record.merge(snapshot("a", "2026-01-01T10:05:00Z", 1)));
        assert_eq!(shown(&record), ("a".into(), 1));
        assert!(!record.merge(snapshot("b", "2026-01-01T09:00:00Z", 9)));
    }
}
