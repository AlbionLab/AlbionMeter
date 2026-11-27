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
        Ok(Self { connection: Mutex::new(connection), written: Mutex::default(), database_path })
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub(crate) fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Saves a finished or live encounter as completed; one without events only when `allow_empty`.
    pub fn save(&self, snapshot: &impl Serialize, allow_empty: bool) -> Result<()> {
        let mut snapshot = serde_json::to_value(snapshot)?;
        if number(&snapshot, "eventCount") == 0.0 && !allow_empty {
            return Ok(());
        }
        set(&mut snapshot, "status", "completed".into());
        set(&mut snapshot, "full", true.into());
        let summary = EncounterSummary::of(&snapshot)?;
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR REPLACE INTO encounters VALUES ($id, $date, $summary, $snapshot)",
            params![summary.id, summary.started_at.to_round_trip(), json::to_string(&summary)?, json::to_string(&snapshot)?],
        )?;
        crate::identity::reapply(&transaction, &summary.id)?;
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

    pub fn save_dungeon(&self, run: &impl Serialize, allow_empty: bool) -> Result<()> {
        let run = serde_json::to_value(run)?;
        if number(run.get("totals").unwrap_or(&Value::Null), "eventCount") == 0.0 && !allow_empty {
            return Ok(());
        }
        let summary = DungeonSummary::of(&run)?;
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR REPLACE INTO dungeon_runs VALUES ($id, $date, $summary, $snapshot)",
            params![summary.id, summary.started_at.to_round_trip(), json::to_string(&summary)?, json::to_string(&run)?],
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
