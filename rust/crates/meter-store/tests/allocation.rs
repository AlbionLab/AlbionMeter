//! Peak heap of saving a 2-hour live record (10 players, every second with weapon and item sources), through the
//! `serde_json::Value` path and the typed path. Run with `--release -- --nocapture` for the numbers.

use meter_core::engine::{CombatEvent, CombatMetric, PartyMember};
use meter_core::CombatEngine;
use meter_store::{DateTimeOffset, EncounterStore, EncounterSummary};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

mod common;
use common::TempDir;

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Bytes allocated above the starting point at the worst moment of `work`.
fn peak(work: impl FnOnce()) -> usize {
    let start = CURRENT.load(Ordering::Relaxed);
    PEAK.store(start, Ordering::Relaxed);
    work();
    PEAK.load(Ordering::Relaxed) - start
}

#[test]
fn saving_a_two_hour_record_without_a_json_tree() {
    let members: Vec<PartyMember> = (0..10).map(|index| PartyMember::new(&format!("9c0b222a-86fa-443e-a9e7-c76e167c39b{index}"), &format!("Player{index}"), "Great Hammer", "hammer", "#eab978")).collect();
    let mut engine = CombatEngine::new(members.clone(), None, Some("two-hours".into())).unwrap();
    for second in 0..7200 {
        engine.advance(second as f64 + 1.0).unwrap();
        for (index, member) in members.iter().enumerate() {
            let (metric, weapon) = if index % 4 == 0 { (CombatMetric::Healing, 7108) } else { (CombatMetric::Damage, 9022 + index as i32) };
            let event = CombatEvent::new(&format!("{second}:{index}"), &member.id, second as f64 + 0.5, metric, 100.0 + (second * 7 + index) as f64 * 0.37)
                .weapon(weapon)
                .source(4308 + index as i32, if second % 3 == 0 { "chest" } else { "mainhand" }, "ability");
            engine.add(&event);
        }
    }
    let snapshot = engine.snapshot(true, "completed");
    assert!(snapshot.full && snapshot.timeline.len() >= 7200);
    let summary = EncounterSummary {
        id: snapshot.id.clone(),
        started_at: DateTimeOffset::parse(serde_json::to_value(snapshot.started_at).unwrap().as_str().unwrap()).unwrap(),
        duration: snapshot.elapsed_seconds,
        damage: snapshot.total_damage,
        healing: snapshot.total_healing,
        members: (snapshot.members.len() + snapshot.former_members.as_ref().map_or(0, Vec::len)) as i32,
        mode: snapshot.mode.clone(),
        location: None,
    };

    let directory = TempDir::new("allocation");
    let store = EncounterStore::open(directory.path(), false).unwrap();
    let tree = peak(|| store.save(&snapshot, false).unwrap());
    let saved = store.get("two-hours").unwrap().unwrap();
    let first = peak(|| store.save_encounter(&snapshot, &summary, snapshot.event_count, false).unwrap());
    assert_eq!(store.get("two-hours").unwrap().unwrap(), saved, "both paths save the same bytes");
    // The live record's next save (every 5 s) is sized from this one.
    let typed = peak(|| store.save_encounter(&snapshot, &summary, snapshot.event_count, false).unwrap());
    assert_eq!(store.get("two-hours").unwrap().unwrap(), saved);
    let summary_row: String = rusqlite::Connection::open(store.database_path()).unwrap().query_row("SELECT summary FROM encounters", [], |row| row.get(0)).unwrap();
    assert_eq!(summary_row, meter_store::json::to_string(&summary).unwrap());
    eprintln!("2 h record: {} KB of JSON; peak heap while saving: Value path {} KB, typed path {} KB (first save) and {} KB (next saves)", saved.len() / 1024, tree / 1024, first / 1024, typed / 1024);
    assert!(typed * 2 < tree, "the typed path should need well under half the memory ({typed} vs {tree})");
}
