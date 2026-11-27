//! web/src/state.ts mergeSnapshot: snapshots after the first carry only the timeline points that changed.

use std::sync::Arc;

use crate::feed::{Point, Snapshot};

/// The record the UI shows: the latest snapshot and its whole timeline, grown in place.
#[derive(Default)]
pub struct Record {
    pub snapshot: Option<Arc<Snapshot>>,
    pub timeline: Vec<Point>,
}

impl Record {
    /// Applies `next`; false when it is older than what is shown and was dropped.
    pub fn merge(&mut self, next: Arc<Snapshot>) -> bool {
        if let Some(previous) = &self.snapshot {
            // A snapshot that started earlier is older, whatever its id: an older record, or the same one before a Reset.
            if previous.started_at > next.started_at {
                return false;
            }
            // Reset keeps the record id but starts it again (a new start time and sequence): that snapshot is not an old one.
            if previous.id == next.id && previous.started_at == next.started_at && previous.sequence > next.sequence {
                return false;
            }
        }
        let same = self.snapshot.as_ref().is_some_and(|previous| previous.id == next.id && previous.started_at == next.started_at);
        if next.full || !same {
            self.timeline.clear();
        }
        for point in &next.timeline {
            match self.timeline.binary_search_by_key(&point.second, |known| known.second) {
                Ok(index) => self.timeline[index] = point.clone(),
                Err(index) => self.timeline.insert(index, point.clone()),
            }
        }
        self.snapshot = Some(next);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(id: &str, started: &str, sequence: u64, full: bool, seconds: &[u32]) -> Arc<Snapshot> {
        Arc::new(Snapshot {
            id: id.into(),
            started_at: started.into(),
            sequence,
            full,
            timeline: seconds.iter().map(|&second| Point { second, ..Point::default() }).collect(),
            ..Snapshot::default()
        })
    }

    fn seconds(record: &Record) -> Vec<u32> {
        record.timeline.iter().map(|point| point.second).collect()
    }

    #[test]
    fn grows_the_timeline_and_drops_stale_snapshots() {
        let mut record = Record::default();
        assert!(record.merge(snapshot("a", "2026-01-01T10:00:00Z", 1, true, &[0, 1])));
        assert!(record.merge(snapshot("a", "2026-01-01T10:00:00Z", 2, false, &[1, 2])));
        assert_eq!(seconds(&record), [0, 1, 2]);
        assert!(!record.merge(snapshot("a", "2026-01-01T10:00:00Z", 1, false, &[5])));
        // Reset: same id, later start, lower sequence.
        assert!(record.merge(snapshot("a", "2026-01-01T10:05:00Z", 1, false, &[0])));
        assert_eq!(seconds(&record), [0]);
        assert!(!record.merge(snapshot("b", "2026-01-01T09:00:00Z", 9, true, &[])));
    }
}
