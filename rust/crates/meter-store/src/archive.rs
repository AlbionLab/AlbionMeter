//! The history export/import file of HistoryEndpoints.cs: a ZIP holding one `history.json`
//! (`{"format":"albion-meter-history","version":1,"encounters":[...],"dungeons":[...]}`), or that JSON bare.

use crate::encounters::started_at;
use crate::{json, Result, StoreError, RETAINED_DUNGEONS, RETAINED_ENCOUNTERS};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::io::{Cursor, Read, Write};

pub const MAX_UPLOAD_BYTES: usize = 40_000_000;
const MAX_JSON_BYTES: usize = 128_000_000;
const FORMAT: &str = "albion-meter-history";
const TOO_LARGE: &str = "This history is too large. Export individual records instead.";
const UNSUPPORTED: &str = "The file is not a supported Albion Meter history.";
const INVALID_RECORDS: &str = "The history contains invalid or unsupported records.";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryArchive {
    pub format: String,
    pub version: i32,
    pub encounters: Vec<Value>,
    pub dungeons: Vec<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryImportResult {
    pub imported_encounters: i32,
    pub imported_dungeons: i32,
    pub skipped_encounters: i32,
    pub skipped_dungeons: i32,
}

fn invalid<T>(message: &str) -> Result<T> {
    Err(StoreError::Invalid(message.into()))
}

impl HistoryArchive {
    pub fn new(encounters: Vec<Value>, dungeons: Vec<Value>) -> Self {
        Self { format: FORMAT.into(), version: 1, encounters, dungeons }
    }

    /// The ZIP the export downloads.
    pub fn write(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let json = json::to_string(self)?;
        if json.len() > MAX_JSON_BYTES {
            return invalid(TOO_LARGE);
        }
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("history.json", options).map_err(|error| StoreError::Invalid(error.to_string()))?;
        zip.write_all(json.as_bytes())?;
        let bytes = zip.finish().map_err(|error| StoreError::Invalid(error.to_string()))?.into_inner();
        if bytes.len() > MAX_UPLOAD_BYTES {
            return invalid(TOO_LARGE);
        }
        Ok(bytes)
    }

    /// An uploaded history: the ZIP, or its JSON alone (with or without a BOM).
    pub fn read(bytes: &[u8]) -> Result<Self> {
        if bytes.is_empty() || bytes.len() > MAX_UPLOAD_BYTES {
            return invalid("Choose a history file smaller than 40 MB.");
        }
        let unzipped;
        let mut content = bytes;
        if bytes.starts_with(b"PK") {
            let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| StoreError::Invalid("Invalid history archive.".into()))?;
            if zip.len() != 1 {
                return invalid("Invalid history archive.");
            }
            let entry = zip.by_index(0).map_err(|_| StoreError::Invalid("Invalid history archive.".into()))?;
            if entry.name() != "history.json" || entry.size() as usize > MAX_JSON_BYTES {
                return invalid("Invalid history archive.");
            }
            let mut json = Vec::new();
            entry.take(MAX_JSON_BYTES as u64 + 1).read_to_end(&mut json).map_err(|_| StoreError::Invalid("Invalid history archive.".into()))?;
            if json.len() > MAX_JSON_BYTES {
                return invalid("History exceeds the import limit.");
            }
            unzipped = json;
            content = &unzipped;
        }
        let content = content.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(content);
        let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(content) else { return invalid(UNSUPPORTED) };
        let archive = Self::from_object(root)?;
        archive.validate()?;
        Ok(archive)
    }

    /// Property names match without case, as System.Text.Json's web defaults read them.
    fn from_object(root: Map<String, Value>) -> Result<Self> {
        let mut format = None;
        let mut version = 0;
        let (mut encounters, mut dungeons) = (None, None);
        for (key, value) in root {
            match key.to_ascii_lowercase().as_str() {
                "format" => format = match value { Value::String(text) => Some(text), Value::Null => None, _ => return invalid(UNSUPPORTED) },
                "version" => version = value.as_i64().and_then(|number| i32::try_from(number).ok()).map_or_else(|| invalid(UNSUPPORTED), Ok)?,
                "encounters" | "dungeons" => {
                    let list = match value { Value::Array(items) => Some(items), Value::Null => None, _ => return invalid(UNSUPPORTED) };
                    if key.eq_ignore_ascii_case("encounters") { encounters = list } else { dungeons = list }
                }
                _ => {}
            }
        }
        match (format, encounters, dungeons) {
            (Some(format), Some(encounters), Some(dungeons)) => Ok(Self { format, version, encounters, dungeons }),
            _ => invalid(INVALID_RECORDS),
        }
    }

    /// `HistoryArchiveFile.Validate`: every record the importer would store is checked before anything is written.
    pub fn validate(&self) -> Result<()> {
        if self.check() { Ok(()) } else { invalid(INVALID_RECORDS) }
    }

    fn check(&self) -> bool {
        let mut cells = 0usize;
        self.format == FORMAT
            && self.version == 1
            && self.encounters.len() <= RETAINED_ENCOUNTERS
            && self.dungeons.len() <= RETAINED_DUNGEONS
            && distinct(&self.encounters)
            && distinct(&self.dungeons)
            && self.encounters.iter().all(|encounter| snapshot(Some(encounter), &mut cells))
            && self.dungeons.iter().all(|run| dungeon(run, &mut cells))
    }
}

fn id(value: &Value) -> Option<&Value> {
    value.get("id").filter(|id| !id.is_null())
}

fn distinct(values: &[Value]) -> bool {
    let mut seen = HashSet::new();
    values.iter().all(|value| seen.insert(id(value).map(Value::to_string)))
}

fn is_text(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|text| {
        !text.trim().is_empty() && text.encode_utf16().count() <= 4096 && !text.chars().any(char::is_control)
    })
}

/// A missing number is the record's default 0; a null or a string is not a number.
fn is_number(value: Option<&Value>) -> bool {
    match value {
        None => true,
        Some(value) => value.as_f64().is_some_and(|number| number.is_finite() && (0.0..=1e15).contains(&number)),
    }
}

fn integer(value: Option<&Value>) -> Option<i64> {
    match value {
        None => Some(0),
        Some(value) => value.as_i64(),
    }
}

fn texts(value: &Value, keys: &[&str]) -> bool {
    keys.iter().all(|key| is_text(value.get(*key)))
}

fn numbers(value: &Value, keys: &[&str]) -> bool {
    keys.iter().all(|key| is_number(value.get(*key)))
}

/// An optional list: missing or null is empty.
fn optional_list<'a>(value: &'a Value, key: &str) -> Option<&'a [Value]> {
    match value.get(key) {
        None | Some(Value::Null) => Some(&[]),
        Some(Value::Array(items)) => Some(items),
        Some(_) => None,
    }
}

fn has_start(value: &Value) -> bool {
    started_at(value).is_ok_and(|start| start != Default::default())
}

fn snapshot(value: Option<&Value>, cells: &mut usize) -> bool {
    let Some(snapshot) = value.filter(|value| value.is_object()) else { return false };
    let elapsed = snapshot.get("elapsedSeconds").and_then(Value::as_f64).unwrap_or_default();
    if !texts(snapshot, &["id", "mode"])
        || snapshot.get("full").and_then(Value::as_bool) != Some(true)
        || !has_start(snapshot)
        || !integer(snapshot.get("eventCount")).is_some_and(|count| count >= 0)
        || !integer(snapshot.get("sequence")).is_some_and(|sequence| sequence >= 0)
        || !matches!(snapshot.get("status").and_then(Value::as_str), Some("idle" | "running" | "paused" | "completed"))
        || !numbers(snapshot, &["elapsedSeconds", "totalDamage", "totalHealing"])
    {
        return false;
    }
    let (Some(Value::Array(members)), Some(Value::Array(timeline)), Some(former)) =
        (snapshot.get("members"), snapshot.get("timeline"), optional_list(snapshot, "formerMembers"))
    else {
        return false;
    };
    let members: Vec<&Value> = members.iter().chain(former).collect();
    let mut ids = HashSet::new();
    if members.len() > 100 || !members.iter().all(|member| ids.insert(id(member).map(Value::to_string))) {
        return false;
    }
    let member_numbers = ["damage", "healing", "encounterDps", "encounterHps", "rollingDps", "rollingHps", "damageShare", "healingShare"];
    if !members.iter().all(|member| member.is_object() && texts(member, &["id", "name", "weapon", "color"]) && numbers(member, &member_numbers)) {
        return false;
    }
    let mut seconds = HashSet::new();
    for point in timeline {
        let Some(second) = point.get("second").map_or(Some(0), Value::as_i64) else { return false };
        if !point.is_object() || second < 0 || second as f64 > elapsed.ceil() + 1.0 || !seconds.insert(second) {
            return false;
        }
        if !point.get("damage").is_some_and(Value::is_object) || !point.get("healing").is_some_and(Value::is_object) {
            return false;
        }
        for key in ["damage", "healing", "weaponDamage", "weaponHealing", "itemDamage", "itemHealing"] {
            let map = match point.get(key) {
                None | Some(Value::Null) => continue,
                Some(Value::Object(map)) => map,
                Some(_) => return false,
            };
            *cells += map.len();
            if *cells > 10_000_000 || !map.iter().all(|(key, amount)| is_text(Some(&Value::String(key.clone()))) && amount.is_number() && is_number(Some(amount))) {
                return false;
            }
        }
    }
    let (Some(items), Some(weapons)) = (optional_list(snapshot, "itemContributions"), optional_list(snapshot, "weaponContributions")) else { return false };
    items.iter().all(|item| item.is_object() && texts(item, &["id", "playerId", "playerName", "item", "slot"]) && numbers(item, &["damage", "healing"]))
        && weapons.iter().all(|weapon| weapon.is_object() && texts(weapon, &["id", "playerId", "playerName", "weapon"]) && numbers(weapon, &["damage", "healing"]))
}

fn dungeon(run: &Value, cells: &mut usize) -> bool {
    if !run.is_object() || !texts(run, &["id", "name", "entranceMapId", "entranceMapName", "mode"]) || !has_start(run) {
        return false;
    }
    let Some(Value::Array(floors)) = run.get("floors") else { return false };
    if floors.len() > 100 || !snapshot(run.get("totals"), cells) {
        return false;
    }
    let (mut ids, mut numbers, mut encounters) = (HashSet::new(), HashSet::new(), HashSet::new());
    for floor in floors {
        if !floor.is_object() || !texts(floor, &["id", "mapId", "mapName"]) {
            return false;
        }
        let Some(number) = integer(floor.get("number")) else { return false };
        let Some(Value::Array(list)) = floor.get("encounters") else { return false };
        if !(1..=100).contains(&number) || !ids.insert(floor["id"].to_string()) || !numbers.insert(number) || !snapshot(floor.get("totals"), cells) {
            return false;
        }
        for encounter in list {
            if !snapshot(Some(encounter), cells) || !encounters.insert(encounter["id"].to_string()) || encounters.len() > 1000 {
                return false;
            }
        }
    }
    true
}
