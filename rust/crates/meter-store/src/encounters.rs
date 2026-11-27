use crate::identity::Written;
use crate::time::DateTimeOffset;
use crate::{json, paths, Result, StoreError};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

pub const RETAINED_ENCOUNTERS: usize = 1000;
pub const LISTED_ENCOUNTERS: usize = 500;
pub const RETAINED_DUNGEONS: usize = 100;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncounterSummary {
    pub id: String,
    pub started_at: DateTimeOffset,
    pub duration: f64,
    pub damage: f64,
    pub healing: f64,
    pub members: i32,
    pub mode: String,
    #[serde(default)]
    pub location: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DungeonSummary {
    pub id: String,
    pub started_at: DateTimeOffset,
    pub name: String,
    pub entrance_map_id: String,
    pub entrance_map_name: String,
    pub mode: String,
    pub status: String,
    pub floors: i32,
    pub duration: f64,
    pub damage: f64,
    pub healing: f64,
}

pub(crate) fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

pub(crate) fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or_default()
}

pub(crate) fn count(value: &Value, key: &str) -> usize {
    value.get(key).and_then(Value::as_array).map_or(0, Vec::len)
}

pub(crate) fn started_at(value: &Value) -> Result<DateTimeOffset> {
    value.get("startedAt").and_then(Value::as_str).and_then(DateTimeOffset::parse)
        .ok_or_else(|| StoreError::Invalid("The record has no valid start time.".into()))
}

pub(crate) fn set(value: &mut Value, key: &str, field: Value) {
    if let Some(object) = value.as_object_mut() {
        object.insert(key.into(), field);
    }
}

impl EncounterSummary {
    pub fn of(snapshot: &Value) -> Result<Self> {
        Ok(Self {
            id: text(snapshot, "id").into(),
            started_at: started_at(snapshot)?,
            duration: number(snapshot, "elapsedSeconds"),
            damage: number(snapshot, "totalDamage"),
            healing: number(snapshot, "totalHealing"),
            members: (count(snapshot, "members") + count(snapshot, "formerMembers")) as i32,
            mode: text(snapshot, "mode").into(),
            location: snapshot.get("location").filter(|location| !location.is_null()).cloned(),
        })
    }
}

impl DungeonSummary {
    /// `EncounterStore.Summarize`.
    pub fn of(run: &Value) -> Result<Self> {
        let totals = run.get("totals").unwrap_or(&Value::Null);
        Ok(Self {
            id: text(run, "id").into(),
            started_at: started_at(run)?,
            name: text(run, "name").into(),
            entrance_map_id: text(run, "entranceMapId").into(),
            entrance_map_name: text(run, "entranceMapName").into(),
            mode: text(run, "mode").into(),
            status: text(run, "status").into(),
            floors: count(run, "floors") as i32,
            duration: number(totals, "elapsedSeconds"),
            damage: number(totals, "totalDamage"),
            healing: number(totals, "totalHealing"),
        })
    }
}

/// The SQLite history (`encounters.db`): saved encounters, dungeon runs and the identity evidence that names players
/// in them later. One connection, used from one thread at a time.
pub struct EncounterStore {
    connection: Mutex<Connection>,
    /// The id and JSON length of the last typed save: a live record is saved again and again, a little longer each
    /// time, so its next text is allocated at about its size at once.
    last_size: Mutex<(String, usize)>,
    pub(crate) written: Mutex<Written>,
    database_path: PathBuf,
}

impl EncounterStore {
    /// Opens (or creates) `encounters.db` in `directory`, running the same schema steps as the C# constructor. Without
    /// `debug` the debug-only packet and identity timeline data is dropped, as there.
    pub fn open(directory: &Path, debug: bool) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        let database_path = std::path::absolute(directory.join(paths::DATABASE_FILE))?;
        let connection = Connection::open(&database_path)?;
        connection.busy_timeout(Duration::from_secs(30))?;
        connection.execute("CREATE TABLE IF NOT EXISTS encounters (id TEXT PRIMARY KEY, started_at TEXT NOT NULL, summary TEXT NOT NULL, snapshot TEXT NOT NULL)", [])?;
        connection.execute("CREATE TABLE IF NOT EXISTS dungeon_runs (id TEXT PRIMARY KEY, started_at TEXT NOT NULL, summary TEXT NOT NULL, snapshot TEXT NOT NULL)", [])?;
        crate::identity::ensure_schema(&connection)?;
        if !debug {
            connection.execute_batch("DROP TABLE IF EXISTS packets; DELETE FROM identity_timeline")?;
        }
        Ok(Self { connection: Mutex::new(connection), last_size: Mutex::default(), written: Mutex::default(), database_path })
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub(crate) fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Saves a finished or live encounter as completed; one without events only when `allow_empty`.
    ///
    /// This goes through a `serde_json::Value` (to read the summary and mark it completed). For a typed snapshot,
    /// [`save_encounter`](Self::save_encounter) writes it straight to its JSON text instead.
    pub fn save(&self, snapshot: &impl Serialize, allow_empty: bool) -> Result<()> {
        let mut snapshot = serde_json::to_value(snapshot)?;
        if number(&snapshot, "eventCount") == 0.0 && !allow_empty {
            return Ok(());
        }
        set(&mut snapshot, "status", "completed".into());
        set(&mut snapshot, "full", true.into());
        let summary = EncounterSummary::of(&snapshot)?;
        self.write_encounter(&summary, &json::to_string(&snapshot)?)
    }

    /// Saves a typed (or `Arc`) encounter snapshot by serializing it directly to its JSON text, with no
    /// `serde_json::Value` in between. The snapshot must already be what C# saves, `snapshot with { Status =
    /// "completed", Full = true }`, and `summary` must be its [`EncounterSummary`]; `event_count` is its
    /// `eventCount` (nothing is saved at 0 unless `allow_empty`).
    pub fn save_encounter(&self, snapshot: &(impl Serialize + ?Sized), summary: &EncounterSummary, event_count: i64, allow_empty: bool) -> Result<()> {
        if event_count == 0 && !allow_empty {
            return Ok(());
        }
        let capacity = {
            let last = self.last_size.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if last.0 == summary.id { last.1 + last.1 / 16 + 4096 } else { 128 }
        };
        let text = json::to_string_with_capacity(snapshot, capacity)?;
        *self.last_size.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = (summary.id.clone(), text.len());
        self.write_encounter(summary, &text)
    }

    /// [`save_encounter`](Self::save_encounter) for a snapshot already written with [`json::to_string`].
    pub fn save_encounter_json(&self, snapshot: &str, summary: &EncounterSummary, event_count: i64, allow_empty: bool) -> Result<()> {
        if event_count == 0 && !allow_empty {
            return Ok(());
        }
        self.write_encounter(summary, snapshot)
    }

    fn write_encounter(&self, summary: &EncounterSummary, snapshot: &str) -> Result<()> {
        // Before the connection: identities take this lock first.
        self.written.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).labels.clear();
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        // Players named since an earlier save of this record are named again in the text in hand, before it is
        // written: C# wrote, read back and rewrote it, which is the same row at twice the work.
        let named = crate::identity::reapply(&transaction, &summary.id, snapshot)?;
        transaction.execute(
            "INSERT OR REPLACE INTO encounters VALUES ($id, $date, $summary, $snapshot)",
            params![summary.id, summary.started_at.to_round_trip(), json::to_string(summary)?, named.as_deref().unwrap_or(snapshot)],
        )?;
        transaction.execute(
            &format!("DELETE FROM encounters WHERE id IN (SELECT id FROM encounters ORDER BY (COALESCE(json_extract(summary, '$.damage'), 0) + COALESCE(json_extract(summary, '$.healing'), 0)) > 0, started_at LIMIT MAX(0, (SELECT COUNT(*) FROM encounters) - {RETAINED_ENCOUNTERS}))"),
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<EncounterSummary>> {
        self.summaries(&format!("SELECT summary FROM encounters ORDER BY started_at DESC LIMIT {LISTED_ENCOUNTERS}"))
    }

    /// The saved snapshot JSON exactly as stored.
    pub fn get(&self, id: &str) -> Result<Option<String>> {
        Ok(self.connection().query_row("SELECT snapshot FROM encounters WHERE id = $id", [id], |row| row.get(0)).optional()?)
    }

    /// Saves a dungeon run; one whose totals have no events only when `allow_empty`. Goes through a
    /// `serde_json::Value`; [`save_dungeon_run`](Self::save_dungeon_run) is the typed path.
    pub fn save_dungeon(&self, run: &impl Serialize, allow_empty: bool) -> Result<()> {
        let run = serde_json::to_value(run)?;
        if number(run.get("totals").unwrap_or(&Value::Null), "eventCount") == 0.0 && !allow_empty {
            return Ok(());
        }
        self.write_dungeon(&DungeonSummary::of(&run)?, &json::to_string(&run)?)
    }

    /// Saves a typed (or `Arc`) dungeon run by serializing it directly to its JSON text. `summary` must be its
    /// [`DungeonSummary`] and `event_count` its totals' `eventCount`.
    pub fn save_dungeon_run(&self, run: &(impl Serialize + ?Sized), summary: &DungeonSummary, event_count: i64, allow_empty: bool) -> Result<()> {
        if event_count == 0 && !allow_empty {
            return Ok(());
        }
        self.write_dungeon(summary, &json::to_string(run)?)
    }

    /// [`save_dungeon_run`](Self::save_dungeon_run) for a run already written with [`json::to_string`].
    pub fn save_dungeon_json(&self, run: &str, summary: &DungeonSummary, event_count: i64, allow_empty: bool) -> Result<()> {
        if event_count == 0 && !allow_empty {
            return Ok(());
        }
        self.write_dungeon(summary, run)
    }

    fn write_dungeon(&self, summary: &DungeonSummary, run: &str) -> Result<()> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR REPLACE INTO dungeon_runs VALUES ($id, $date, $summary, $snapshot)",
            params![summary.id, summary.started_at.to_round_trip(), json::to_string(summary)?, run],
        )?;
        transaction.execute(&format!("DELETE FROM dungeon_runs WHERE id NOT IN (SELECT id FROM dungeon_runs ORDER BY started_at DESC LIMIT {RETAINED_DUNGEONS})"), [])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn list_dungeons(&self) -> Result<Vec<DungeonSummary>> {
        self.summaries(&format!("SELECT summary FROM dungeon_runs ORDER BY started_at DESC LIMIT {RETAINED_DUNGEONS}"))
    }

    pub fn get_dungeon<T: DeserializeOwned>(&self, id: &str) -> Result<Option<T>> {
        let json: Option<String> = self.connection().query_row("SELECT snapshot FROM dungeon_runs WHERE id = $id", [id], |row| row.get(0)).optional()?;
        Ok(json.map(|json| serde_json::from_str(&json)).transpose()?)
    }

    fn summaries<T: DeserializeOwned>(&self, sql: &str) -> Result<Vec<T>> {
        let connection = self.connection();
        let mut statement = connection.prepare(sql)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|json| Ok(serde_json::from_str(&json?)?)).collect()
    }

    pub(crate) fn transaction<T>(&self, work: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let result = work(&transaction)?;
        transaction.commit()?;
        Ok(result)
    }
}
