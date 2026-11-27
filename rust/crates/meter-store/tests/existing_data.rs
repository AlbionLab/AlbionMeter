//! The user's real history (`~/.local/share/AlbionMeter/encounters.db`, or `METER_REAL_DB`) opens unchanged. The
//! original is only ever copied; every check runs on copies in a temporary directory. Skipped when there is none.

use meter_store::{json, paths, EncounterStore, HistoryArchive, LISTED_ENCOUNTERS};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::path::PathBuf;

mod common;
use common::TempDir;

fn real_database() -> Option<PathBuf> {
    let path = std::env::var_os("METER_REAL_DB").map(PathBuf::from).unwrap_or_else(|| paths::local_application_data().join("AlbionMeter").join(paths::DATABASE_FILE));
    path.is_file().then_some(path)
}

fn rows(connection: &Connection, sql: &str) -> Vec<(String, String)> {
    let mut statement = connection.prepare(sql).unwrap();
    statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
}

#[test]
fn the_existing_history_reads_and_rewrites_byte_for_byte() {
    let Some(original) = real_database() else {
        eprintln!("no existing encounters.db; skipped");
        return;
    };
    let directory = TempDir::new("real");
    let (raw_dir, store_dir) = (directory.path().join("raw"), directory.path().join("store"));
    for target in [&raw_dir, &store_dir] {
        std::fs::create_dir_all(target).unwrap();
        for suffix in ["", "-wal", "-shm"] {
            let source = PathBuf::from(format!("{}{suffix}", original.display()));
            if source.is_file() {
                std::fs::copy(&source, target.join(format!("{}{suffix}", paths::DATABASE_FILE))).unwrap();
            }
        }
    }
    let raw = Connection::open_with_flags(raw_dir.join(paths::DATABASE_FILE), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let store = EncounterStore::open(&store_dir, false).unwrap();

    let summaries = rows(&raw, &format!("SELECT id, summary FROM encounters ORDER BY started_at DESC LIMIT {LISTED_ENCOUNTERS}"));
    let listed = store.list().unwrap();
    assert_eq!(listed.len(), summaries.len());
    for (summary, (id, raw_summary)) in listed.iter().zip(&summaries) {
        assert_eq!(&summary.id, id);
        assert_eq!(&json::to_string(summary).unwrap(), raw_summary, "summary of {id}");
    }
    let mut rewritten = 0;
    for (id, snapshot) in rows(&raw, "SELECT id, snapshot FROM encounters") {
        assert_eq!(store.get(&id).unwrap().as_deref(), Some(snapshot.as_str()));
        let value: Value = serde_json::from_str(&snapshot).unwrap();
        // A record the identity reconciliation rewrote went through JsonNode, which escapes the dates' '+'.
        let again = if snapshot.contains("\\u002B") { json::to_string_node(&value) } else { json::to_string(&value) }.unwrap();
        assert_eq!(again, snapshot, "snapshot {id} does not re-serialize to the same bytes");
        rewritten += 1;
    }
    eprintln!("{rewritten} encounters re-serialized identically");

    let dungeon_rows = rows(&raw, "SELECT id, summary FROM dungeon_runs ORDER BY started_at DESC LIMIT 100");
    let dungeons = store.list_dungeons().unwrap();
    assert_eq!(dungeons.len(), dungeon_rows.len());
    for (summary, (id, raw_summary)) in dungeons.iter().zip(&dungeon_rows) {
        assert_eq!(&json::to_string(summary).unwrap(), raw_summary, "dungeon summary {id}");
        let run: Value = store.get_dungeon(id).unwrap().unwrap();
        let snapshot: String = raw.query_row("SELECT snapshot FROM dungeon_runs WHERE id = ?", [id], |row| row.get(0)).unwrap();
        let again = if snapshot.contains("\\u002B") { json::to_string_node(&run) } else { json::to_string(&run) }.unwrap();
        assert_eq!(again, snapshot, "dungeon {id}");
    }

    let characters: i64 = raw.query_row("SELECT COUNT(*) FROM characters", [], |row| row.get(0)).unwrap();
    let known = store.known_characters().unwrap();
    assert_eq!(known.len() as i64, characters);
    let (id, name, fingerprints): (String, String, String) = raw.query_row("SELECT id, name, fingerprints FROM characters LIMIT 1", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    let first = &known[&uuid::Uuid::parse_str(&id).unwrap()];
    assert_eq!(first.name, name);
    assert_eq!(first.equipment, serde_json::from_str::<Vec<Vec<Option<i32>>>>(&fingerprints).unwrap().into_iter().next());
    let skills: i64 = raw.query_row("SELECT COUNT(*) FROM skill_weapons", [], |row| row.get(0)).unwrap();
    assert_eq!(store.known_skill_weapons().unwrap().len() as i64, skills);

    // The whole history exports, passes the import validation and imports into an empty store.
    let archive = store.export_history(None, None).unwrap();
    let bytes = archive.write().unwrap();
    let read = HistoryArchive::read(&bytes).unwrap();
    let fresh = EncounterStore::open(&directory.path().join("fresh"), false).unwrap();
    let result = fresh.import_history(&read).unwrap();
    assert_eq!(result.imported_encounters as usize, archive.encounters.len());
    assert_eq!(result.imported_dungeons as usize, archive.dungeons.len());
}
