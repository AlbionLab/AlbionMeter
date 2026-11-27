use crate::encounters::{set, started_at, text, DungeonSummary, EncounterSummary};
use crate::{json, EncounterStore, HistoryArchive, HistoryImportResult, Result, StoreError, RETAINED_DUNGEONS, RETAINED_ENCOUNTERS};
use rusqlite::{params, Transaction};
use serde_json::Value;

impl EncounterStore {
    /// Everything (newest first), or the one encounter or dungeon run asked for.
    pub fn export_history(&self, encounter_id: Option<&str>, dungeon_id: Option<&str>) -> Result<HistoryArchive> {
        if encounter_id.is_some() && dungeon_id.is_some() {
            return Err(StoreError::Argument("Choose one record to export.".into()));
        }
        self.transaction(|transaction| {
            let read = |table: &str, id: Option<&str>| -> Result<Vec<Value>> {
                let sql = format!("SELECT snapshot FROM {table}") + if id.is_none() { " ORDER BY started_at DESC" } else { " WHERE id = $id" };
                let mut statement = transaction.prepare(&sql)?;
                let rows: Vec<String> = match id {
                    Some(id) => statement.query_map([id], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?,
                    None => statement.query_map([], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?,
                };
                if id.is_some() && rows.is_empty() {
                    return Err(StoreError::NotFound("The record was not found.".into()));
                }
                rows.iter().map(|json| Ok(serde_json::from_str(json)?)).collect()
            };
            let encounters = if dungeon_id.is_none() { read("encounters", encounter_id)? } else { vec![] };
            let dungeons = if encounter_id.is_none() { read("dungeon_runs", dungeon_id)? } else { vec![] };
            Ok(HistoryArchive::new(encounters, dungeons))
        })
    }

    /// Adds the archive's records that are not here yet, as completed, up to the retention limits; nothing is replaced.
    pub fn import_history(&self, archive: &HistoryArchive) -> Result<HistoryImportResult> {
        archive.validate()?;
        self.transaction(|transaction| {
            let count = |table: &str| -> Result<usize> {
                Ok(transaction.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get::<_, i64>(0))? as usize)
            };
            let mut encounter_count = count("encounters")?;
            let mut dungeon_count = count("dungeon_runs")?;
            let (mut added_encounters, mut added_dungeons) = (0, 0);
            for source in &archive.encounters {
                let mut snapshot = source.clone();
                set(&mut snapshot, "status", "completed".into());
                let summary = json::to_string(&EncounterSummary::of(&snapshot)?)?;
                if insert(transaction, "encounters", &snapshot, &summary, encounter_count, RETAINED_ENCOUNTERS)? {
                    encounter_count += 1;
                    added_encounters += 1;
                }
            }
            for source in &archive.dungeons {
                let run = completed_run(source);
                let summary = json::to_string(&DungeonSummary::of(&run)?)?;
                if insert(transaction, "dungeon_runs", &run, &summary, dungeon_count, RETAINED_DUNGEONS)? {
                    dungeon_count += 1;
                    added_dungeons += 1;
                }
            }
            Ok(HistoryImportResult {
                imported_encounters: added_encounters,
                imported_dungeons: added_dungeons,
                skipped_encounters: archive.encounters.len() as i32 - added_encounters,
                skipped_dungeons: archive.dungeons.len() as i32 - added_dungeons,
            })
        })
    }
}

fn insert(transaction: &Transaction, table: &str, snapshot: &Value, summary: &str, count: usize, limit: usize) -> Result<bool> {
    if count >= limit {
        return Ok(false);
    }
    let changed = transaction.execute(
        &format!("INSERT OR IGNORE INTO {table} (id, started_at, summary, snapshot) VALUES ($id, $date, $summary, $snapshot)"),
        params![text(snapshot, "id"), started_at(snapshot)?.to_round_trip(), summary, json::to_string(snapshot)?],
    )?;
    Ok(changed > 0)
}

/// The run, its totals, every floor's totals and every floor encounter marked completed.
fn completed_run(source: &Value) -> Value {
    let mut run = source.clone();
    set(&mut run, "status", "completed".into());
    if let Some(totals) = run.get_mut("totals") {
        set(totals, "status", "completed".into());
    }
    for floor in run.get_mut("floors").and_then(Value::as_array_mut).into_iter().flatten() {
        if let Some(totals) = floor.get_mut("totals") {
            set(totals, "status", "completed".into());
        }
        for encounter in floor.get_mut("encounters").and_then(Value::as_array_mut).into_iter().flatten() {
            set(encounter, "status", "completed".into());
        }
    }
    run
}
