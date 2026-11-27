//! EncounterStore.Identity.cs: characters seen with a name, players recorded before their name was known, and the
//! evidence (same-zone sightings, elimination) that names them later in every saved record.

use crate::time::DateTimeOffset;
use crate::{json, EncounterStore, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use uuid::Uuid;

const FINGERPRINTS_PER_CHARACTER: usize = 8;
const NOT_OBSERVED: &str = "Weapon not observed";

pub type Equipment = Vec<Option<i32>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownCharacter {
    pub name: String,
    pub equipment: Option<Equipment>,
    pub skills: Option<Vec<i32>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnidentifiedPlayer {
    pub member_id: String,
    pub record_id: Option<String>,
    pub object_id: i64,
    pub equipment: Option<Equipment>,
    pub candidates: Vec<Uuid>,
    /// Capture microseconds.
    pub timestamp: i64,
    pub skills: Option<Vec<i32>>,
    pub visit: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentitySighting {
    pub character_id: Uuid,
    pub object_id: i64,
    pub visit: String,
    pub timestamp: i64,
    pub method: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityTimelineEntry {
    pub timestamp: i64,
    pub visit: String,
    pub kind: String,
    pub character_id: Uuid,
    pub name: Option<String>,
    pub object_id: Option<i64>,
    pub equipment: Option<Equipment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityResolution {
    pub member_id: String,
    pub character_id: Uuid,
    pub name: String,
    pub method: String,
}

/// What this store already wrote, so an unchanged character or a known piece of evidence is not written again.
#[derive(Default)]
pub(crate) struct Written {
    characters: HashMap<Uuid, String>,
    evidence: HashSet<String>,
    skills: HashMap<i32, String>,
}

pub(crate) fn ensure_schema(connection: &Connection) -> Result<()> {
    // The statements as the C# raw string literal leaves them (sqlite_master keeps the text, line breaks included).
    connection.execute_batch(concat!(
        "CREATE TABLE IF NOT EXISTS characters (id TEXT PRIMARY KEY, name TEXT NOT NULL, fingerprints TEXT NOT NULL, seen_at TEXT NOT NULL);\n",
        "CREATE TABLE IF NOT EXISTS unidentified_players (member_id TEXT PRIMARY KEY, record_id TEXT, object_id INTEGER NOT NULL, equipment TEXT,\n",
        "    candidates TEXT NOT NULL, observed_at TEXT NOT NULL, resolved_id TEXT, resolved_name TEXT, method TEXT);\n",
        "CREATE TABLE IF NOT EXISTS identity_sightings (character_id TEXT NOT NULL, object_id INTEGER NOT NULL, visit TEXT NOT NULL, seen_at TEXT NOT NULL, method TEXT NOT NULL,\n",
        "    PRIMARY KEY (character_id, object_id, visit));\n",
        "CREATE INDEX IF NOT EXISTS identity_sightings_object ON identity_sightings (visit, object_id);\n",
        "CREATE TABLE IF NOT EXISTS skill_weapons (skill INTEGER PRIMARY KEY, weapons TEXT NOT NULL);\n",
        "DROP TABLE IF EXISTS effect_items;\n",
        "CREATE TABLE IF NOT EXISTS identity_timeline (character_id TEXT NOT NULL, at TEXT NOT NULL, kind TEXT NOT NULL, visit TEXT NOT NULL, name TEXT, object_id INTEGER, equipment TEXT,\n",
        "    PRIMARY KEY (character_id, at, kind));\n",
        "CREATE INDEX IF NOT EXISTS identity_timeline_at ON identity_timeline (at);",
    ))?;
    for (table, column) in [("characters", "skills"), ("unidentified_players", "skills"), ("unidentified_players", "visit")] {
        // Fails with "duplicate column" once the column exists, which is the point.
        let _ = connection.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} TEXT"), []);
    }
    Ok(())
}

fn parse<T: serde::de::DeserializeOwned + Default>(json: Option<String>) -> T {
    json.and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
}

fn milliseconds(microseconds: i64) -> String {
    DateTimeOffset::from_unix_milliseconds(microseconds / 1000).to_round_trip()
}

fn joined<T: ToString>(values: impl IntoIterator<Item = T>) -> String {
    values.into_iter().map(|value| value.to_string()).collect::<Vec<_>>().join(",")
}

impl EncounterStore {
    pub fn known_skill_weapons(&self) -> Result<HashMap<i32, BTreeMap<i32, i32>>> {
        let connection = self.connection();
        let mut statement = connection.prepare("SELECT skill, weapons FROM skill_weapons")?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?)))?;
        rows.map(|row| row.map(|(skill, weapons)| (skill, parse(Some(weapons)))).map_err(Into::into)).collect()
    }

    pub fn observe_skill_weapons<'a>(&self, skills: impl IntoIterator<Item = (&'a i32, &'a BTreeMap<i32, i32>)>) -> Result<()> {
        let mut written = self.written.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.transaction(|transaction| {
            for (skill, weapons) in skills {
                let json = json::to_string(weapons)?;
                if written.skills.get(skill) == Some(&json) {
                    continue;
                }
                transaction.execute("INSERT OR REPLACE INTO skill_weapons VALUES ($skill, $weapons)", params![skill, json])?;
                written.skills.insert(*skill, json);
            }
            Ok(())
        })
    }

    pub fn observe_timeline<'a>(&self, timeline: impl IntoIterator<Item = &'a IdentityTimelineEntry>) -> Result<()> {
        self.transaction(|transaction| {
            let mut write = transaction.prepare("INSERT OR IGNORE INTO identity_timeline VALUES ($character, $at, $kind, $visit, $name, $object, $equipment)")?;
            for entry in timeline {
                let equipment = entry.equipment.as_ref().map(json::to_string).transpose()?;
                write.execute(params![entry.character_id.to_string(), milliseconds(entry.timestamp), entry.kind, entry.visit, entry.name, entry.object_id, equipment])?;
            }
            Ok(())
        })
    }

    /// Every character seen with a name, with its latest equipment fingerprint and skills.
    pub fn known_characters(&self) -> Result<HashMap<Uuid, KnownCharacter>> {
        let connection = self.connection();
        let mut statement = connection.prepare("SELECT id, name, fingerprints, skills FROM characters")?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, Option<String>>(3)?)))?;
        let mut result = HashMap::new();
        for row in rows {
            let (id, name, fingerprints, skills) = row?;
            if let Ok(id) = Uuid::parse_str(&id) {
                let fingerprints: Vec<Equipment> = parse(Some(fingerprints));
                let skills = skills.and_then(|skills| serde_json::from_str(&skills).ok());
                result.insert(id, KnownCharacter { name, equipment: fingerprints.into_iter().next(), skills });
            }
        }
        Ok(result)
    }

    /// Stores what the capture learned and, when anything new arrived, names the players that evidence now proves,
    /// in every saved record they appear in.
    pub fn observe_identities<'a>(
        &self,
        characters: impl IntoIterator<Item = (&'a Uuid, &'a KnownCharacter)>,
        unidentified: impl IntoIterator<Item = &'a UnidentifiedPlayer>,
        sightings: impl IntoIterator<Item = &'a IdentitySighting>,
    ) -> Result<Vec<IdentityResolution>> {
        let mut written = self.written.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let mut changed = false;
        for (id, character) in characters {
            let key = format!(
                "{}|{}|{}",
                character.name,
                character.equipment.as_ref().map_or(String::new(), |equipment| equipment.iter().map(|slot| slot.map_or(String::new(), |item| item.to_string())).collect::<Vec<_>>().join(",")),
                character.skills.as_ref().map_or(String::new(), joined)
            );
            if written.characters.get(id) == Some(&key) {
                continue;
            }
            let stored: Option<(String, Option<String>)> = transaction
                .query_row("SELECT fingerprints, skills FROM characters WHERE id = $id", [id.to_string()], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let (mut fingerprints, mut skills): (Vec<Equipment>, BTreeSet<i32>) = match stored {
                Some((fingerprints, skills)) => (parse(Some(fingerprints)), parse::<Vec<i32>>(skills).into_iter().collect()),
                None => Default::default(),
            };
            skills.extend(character.skills.iter().flatten());
            if let Some(equipment) = character.equipment.as_ref().filter(|equipment| equipment.len() >= 7)
                && !fingerprints.contains(equipment)
            {
                fingerprints.insert(0, equipment.clone());
                fingerprints.truncate(FINGERPRINTS_PER_CHARACTER);
            }
            let skills = (!skills.is_empty()).then(|| json::to_string(&skills.iter().take(64).collect::<Vec<_>>())).transpose()?;
            transaction.execute(
                "INSERT OR REPLACE INTO characters (id, name, fingerprints, seen_at, skills) VALUES ($id, $name, $fingerprints, $seen, $skills)",
                params![id.to_string(), character.name, json::to_string(&fingerprints)?, DateTimeOffset::now().to_round_trip(), skills],
            )?;
            written.characters.insert(*id, key);
            changed = true;
        }
        for player in unidentified {
            if !written.evidence.insert(player.member_id.clone()) {
                continue;
            }
            let skills = player.skills.as_ref().filter(|skills| !skills.is_empty()).map(json::to_string).transpose()?;
            let equipment = player.equipment.as_ref().map(json::to_string).transpose()?;
            let candidates: Vec<String> = player.candidates.iter().map(Uuid::to_string).collect();
            changed |= transaction.execute(
                "INSERT OR IGNORE INTO unidentified_players (member_id, record_id, object_id, equipment, candidates, observed_at, skills, visit) VALUES ($member, $record, $object, $equipment, $candidates, $observed, $skills, $visit)",
                params![player.member_id, player.record_id, player.object_id, equipment, json::to_string(&candidates)?, milliseconds(player.timestamp), skills, player.visit],
            )? > 0;
        }
        for sighting in sightings {
            if !written.evidence.insert(format!("sighting:{}:{}:{}", sighting.character_id, sighting.object_id, sighting.visit)) {
                continue;
            }
            changed |= transaction.execute(
                "INSERT OR IGNORE INTO identity_sightings VALUES ($character, $object, $visit, $seen, $method)",
                params![sighting.character_id.to_string(), sighting.object_id, sighting.visit, milliseconds(sighting.timestamp.max(0)), sighting.method],
            )? > 0;
        }
        transaction.commit()?;
        if changed { reconcile(&connection) } else { Ok(vec![]) }
    }
}

struct Pending {
    member: String,
    record: Option<String>,
    candidates: Vec<Uuid>,
    visit: Option<String>,
    object: i64,
}

/// .NET's default (culture) string order for names: case is a tie-breaker, lowercase first.
fn culture_order(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| b.cmp(a))
}

fn reconcile(connection: &Connection) -> Result<Vec<IdentityResolution>> {
    let mut characters: HashMap<Uuid, String> = HashMap::new();
    {
        let mut statement = connection.prepare("SELECT id, name FROM characters")?;
        for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
            let (id, name) = row?;
            if let Ok(id) = Uuid::parse_str(&id) {
                characters.insert(id, name);
            }
        }
    }
    let mut pending = Vec::new();
    {
        let mut statement = connection.prepare("SELECT member_id, record_id, candidates, visit, object_id FROM unidentified_players WHERE resolved_id IS NULL")?;
        for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, String>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, i64>(4)?)))? {
            let (member, record, candidates, visit, object) = row?;
            let candidates: Vec<String> = parse(Some(candidates));
            pending.push(Pending { member, record, candidates: candidates.iter().filter_map(|id| Uuid::parse_str(id).ok()).collect(), visit, object });
        }
    }
    let mut record_members: HashMap<String, HashSet<Uuid>> = HashMap::new();
    for record in pending.iter().filter_map(|player| player.record.as_ref()) {
        if !record_members.contains_key(record) {
            record_members.insert(record.clone(), self::record_members(connection, record)?);
        }
    }
    {
        let mut statement = connection.prepare("SELECT record_id, resolved_id FROM unidentified_players WHERE resolved_id IS NOT NULL AND record_id IS NOT NULL")?;
        for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
            let (record, resolved) = row?;
            if let (Some(members), Ok(resolved)) = (record_members.get_mut(&record), Uuid::parse_str(&resolved)) {
                members.insert(resolved);
            }
        }
    }
    let mut resolutions: Vec<IdentityResolution> = Vec::new();
    let mut labels: Vec<(String, String)> = Vec::new();
    for player in pending.iter().filter(|player| player.visit.is_some()) {
        let mut statement = connection.prepare_cached("SELECT DISTINCT character_id FROM identity_sightings WHERE visit = $visit AND object_id = $object")?;
        let seen: Vec<Uuid> = statement
            .query_map(params![player.visit, player.object], |row| row.get::<_, String>(0))?
            .filter_map(|id| id.ok().and_then(|id| Uuid::parse_str(&id).ok()))
            .collect();
        if let [only] = seen[..]
            && let Some(name) = characters.get(&only)
        {
            resolutions.push(IdentityResolution { member_id: player.member.clone(), character_id: only, name: name.clone(), method: "same-zone".into() });
        }
    }
    let named = |record: &Option<String>, resolutions: &[IdentityResolution]| -> HashSet<Uuid> {
        let Some(record) = record else { return HashSet::new() };
        let mut named = record_members[record].clone();
        named.extend(resolutions.iter().filter(|resolution| pending.iter().any(|player| player.member == resolution.member_id && player.record.as_ref() == Some(record))).map(|resolution| resolution.character_id));
        named
    };
    let mut changed = true;
    while changed {
        changed = false;
        for player in &pending {
            if resolutions.iter().any(|resolution| resolution.member_id == player.member) {
                continue;
            }
            let named = named(&player.record, &resolutions);
            let open: Vec<Uuid> = player.candidates.iter().copied().filter(|candidate| !named.contains(candidate) && characters.contains_key(candidate)).collect();
            if player.record.is_some() && open.len() == 1 {
                resolutions.push(IdentityResolution { member_id: player.member.clone(), character_id: open[0], name: characters[&open[0]].clone(), method: "elimination".into() });
                changed = true;
            } else if (2..=4).contains(&open.len()) {
                let mut names: Vec<&str> = open.iter().map(|candidate| characters[candidate].as_str()).collect();
                names.sort_by(|a, b| culture_order(a, b));
                let (last, rest) = names.split_last().expect("two or more names");
                let label = format!("{} or {last} (obj {})", rest.join(", "), player.object);
                match labels.iter_mut().find(|(member, _)| *member == player.member) {
                    Some(entry) => entry.1 = label,
                    None => labels.push((player.member.clone(), label)),
                }
            }
        }
    }
    for resolution in &resolutions {
        apply(connection, resolution)?;
    }
    for (member, label) in &labels {
        if resolutions.iter().all(|resolution| resolution.member_id != *member) {
            update_records(connection, member, |node| rename(node, member, label))?;
        }
    }
    Ok(resolutions)
}

/// The named members (by character id) who did damage or healing in an encounter.
fn record_members(connection: &Connection, record: &str) -> Result<HashSet<Uuid>> {
    let json: Option<String> = connection.query_row("SELECT snapshot FROM encounters WHERE id = $id", [record], |row| row.get(0)).optional()?;
    let Some(Value::Object(snapshot)) = json.and_then(|json| serde_json::from_str(&json).ok()) else { return Ok(HashSet::new()) };
    Ok(["members", "formerMembers"]
        .iter()
        .filter_map(|key| snapshot.get(*key).and_then(Value::as_array))
        .flatten()
        .filter(|member| contribution(member) > 0.0)
        .filter_map(|member| member.get("id").and_then(Value::as_str).and_then(|id| Uuid::parse_str(id).ok()))
        .collect())
}

fn contribution(member: &Value) -> f64 {
    member.get("damage").and_then(Value::as_f64).unwrap_or_default() + member.get("healing").and_then(Value::as_f64).unwrap_or_default()
}

/// Rewrites every encounter and dungeon run whose JSON mentions the member, when `change` changed it.
fn update_records(connection: &Connection, member: &str, mut change: impl FnMut(&mut Value) -> bool) -> Result<()> {
    for table in ["encounters", "dungeon_runs"] {
        let mut updates = Vec::new();
        {
            let mut select = connection.prepare(&format!("SELECT id, snapshot FROM {table} WHERE instr(snapshot, $member) > 0"))?;
            for row in select.query_map([member], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
                let (id, json) = row?;
                if let Ok(mut node) = serde_json::from_str::<Value>(&json)
                    && change(&mut node)
                {
                    updates.push((id, json::to_string_node(&node)?));
                }
            }
        }
        for (id, snapshot) in updates {
            connection.execute(&format!("UPDATE {table} SET snapshot = $snapshot WHERE id = $id"), params![snapshot, id])?;
        }
    }
    Ok(())
}

/// Names a re-saved live record's players again from what was already resolved for it.
pub(crate) fn reapply(connection: &Connection, record: &str) -> Result<()> {
    let resolved: Vec<IdentityResolution> = {
        let mut statement = connection.prepare("SELECT member_id, resolved_id, resolved_name, method FROM unidentified_players WHERE record_id = $record AND resolved_id IS NOT NULL")?;
        statement
            .query_map([record], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)))?
            .filter_map(|row| match row {
                Ok((member_id, id, name, method)) => Uuid::parse_str(&id).ok().map(|character_id| Ok(IdentityResolution { member_id, character_id, name, method })),
                Err(error) => Some(Err(error)),
            })
            .collect::<rusqlite::Result<_>>()?
    };
    if resolved.is_empty() {
        return Ok(());
    }
    let json: Option<String> = connection.query_row("SELECT snapshot FROM encounters WHERE id = $id", [record], |row| row.get(0)).optional()?;
    let Some(mut node) = json.and_then(|json| serde_json::from_str::<Value>(&json).ok()) else { return Ok(()) };
    let mut changed = false;
    for resolution in &resolved {
        changed |= merge(&mut node, &resolution.member_id, &resolution.character_id.to_string(), &resolution.name);
    }
    if changed {
        connection.execute("UPDATE encounters SET snapshot = $snapshot WHERE id = $id", params![json::to_string_node(&node)?, record])?;
    }
    Ok(())
}

fn apply(connection: &Connection, resolution: &IdentityResolution) -> Result<()> {
    let character = resolution.character_id.to_string();
    update_records(connection, &resolution.member_id, |node| merge(node, &resolution.member_id, &character, &resolution.name))?;
    let (member_skills, character_skills): (Option<String>, Option<String>) = connection.query_row(
        "SELECT (SELECT skills FROM unidentified_players WHERE member_id = $member), (SELECT skills FROM characters WHERE id = $id)",
        params![resolution.member_id, character],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if member_skills.is_some() {
        let mut skills: BTreeSet<i32> = parse::<Vec<i32>>(member_skills).into_iter().collect();
        skills.extend(parse::<Vec<i32>>(character_skills));
        connection.execute(
            "UPDATE characters SET skills = $skills WHERE id = $id",
            params![json::to_string(&skills.iter().take(64).collect::<Vec<_>>())?, character],
        )?;
    }
    connection.execute(
        "UPDATE unidentified_players SET resolved_id = $id, resolved_name = $name, method = $method WHERE member_id = $member",
        params![character, resolution.name, resolution.method, resolution.member_id],
    )?;
    Ok(())
}

fn lists<'a>(snapshot: &'a Map<String, Value>, keys: &[&'static str]) -> Vec<(&'static str, &'a Vec<Value>)> {
    keys.iter().filter_map(|key| snapshot.get(*key).and_then(Value::as_array).map(|list| (*key, list))).collect()
}

fn member_index(lists: &[(&str, &Vec<Value>)], id: &str) -> Option<(usize, usize)> {
    lists.iter().enumerate().find_map(|(list, (_, members))| {
        members.iter().position(|member| member.get("id").and_then(Value::as_str) == Some(id)).map(|index| (list, index))
    })
}

/// Gives an unnamed member its character: the member's entries become the character's, and an empty roster entry the
/// character already had is dropped so it is not listed twice. A character that did fight under its own id keeps
/// both entries; the unnamed one only gets the name.
fn merge(node: &mut Value, member: &str, character: &str, name: &str) -> bool {
    let mut merged = false;
    for_each_snapshot(node, &mut |snapshot| {
        let keys: Vec<&'static str> = ["members", "formerMembers"].into_iter().filter(|key| snapshot.get(*key).is_some_and(Value::is_array)).collect();
        let Some(unnamed) = member_index(&lists(snapshot, &keys), member) else { return };
        if let Some((list, index)) = member_index(&lists(snapshot, &keys), character) {
            let existing = &snapshot[keys[list]][index];
            if contribution(existing) > 0.0 {
                merged |= rename_object(snapshot, member, name);
                return;
            }
            let weapon = existing.get("weapon").and_then(Value::as_str).filter(|weapon| *weapon != NOT_OBSERVED).map(str::to_owned);
            let target = &mut snapshot[keys[unnamed.0]][unnamed.1];
            if target.get("weapon").and_then(Value::as_str) == Some(NOT_OBSERVED)
                && let Some(weapon) = weapon
            {
                target["weapon"] = weapon.into();
            }
            if let Some(Value::Array(members)) = snapshot.get_mut(keys[list]) {
                members.remove(index);
            }
        }
        let mut whole = Value::Object(std::mem::take(snapshot));
        rekey(&mut whole, member, character);
        rename(&mut whole, character, name);
        *snapshot = match whole {
            Value::Object(object) => object,
            _ => unreachable!(),
        };
        merged = true;
    });
    merged
}

/// Every combat snapshot (an object with `members` and `timeline`) in a record: the encounter itself, or a dungeon
/// run's totals, floor totals and floor encounters.
fn for_each_snapshot(node: &mut Value, visit: &mut impl FnMut(&mut Map<String, Value>)) {
    match node {
        Value::Object(item) => {
            if item.contains_key("members") && item.contains_key("timeline") {
                visit(item);
            }
            for child in item.values_mut().filter(|child| !child.is_null()) {
                for_each_snapshot(child, visit);
            }
        }
        Value::Array(array) => {
            for child in array.iter_mut().filter(|child| !child.is_null()) {
                for_each_snapshot(child, visit);
            }
        }
        _ => {}
    }
}

/// `from`, or `from:...` (a weapon or item key of the member), with `to` in its place.
fn replace(value: &str, from: &str, to: &str) -> Option<String> {
    if value == from {
        Some(to.into())
    } else {
        value.strip_prefix(from).filter(|rest| rest.starts_with(':')).map(|rest| format!("{to}{rest}"))
    }
}

/// Moves every property key and string property value naming `from` to `to` (strings inside arrays are left alone).
fn rekey(node: &mut Value, from: &str, to: &str) {
    match node {
        Value::Object(item) => {
            let keys: Vec<String> = item.keys().cloned().collect();
            for key in keys {
                let original = item[&key].clone();
                match item.get_mut(&key) {
                    Some(Value::String(text)) => {
                        if let Some(replaced) = replace(text, from, to) {
                            *text = replaced;
                        }
                    }
                    Some(value) if !value.is_null() => rekey(value, from, to),
                    _ => {}
                }
                if let Some(renamed) = replace(&key, from, to) {
                    // C# re-adds the value it read before replacing a string value, so a key and its string value
                    // both naming the member keep the old value.
                    let value = item.shift_remove(&key).expect("key present");
                    item.insert(renamed, if original.is_string() { original } else { value });
                }
            }
        }
        Value::Array(array) => {
            for child in array.iter_mut().filter(|child| child.is_object() || child.is_array()) {
                rekey(child, from, to);
            }
        }
        _ => {}
    }
}

fn rename_object(item: &mut Map<String, Value>, member: &str, name: &str) -> bool {
    let mut changed = false;
    if item.get("id").and_then(Value::as_str) == Some(member) && item.contains_key("name") {
        item.insert("name".into(), name.into());
        changed = true;
    }
    if item.get("playerId").and_then(Value::as_str) == Some(member) && item.contains_key("playerName") {
        item.insert("playerName".into(), name.into());
        changed = true;
    }
    for child in item.values_mut() {
        changed |= rename(child, member, name);
    }
    changed
}

/// Sets the name of every entry (`id`/`name`, `playerId`/`playerName`) of the member.
fn rename(node: &mut Value, member: &str, name: &str) -> bool {
    match node {
        Value::Object(item) => rename_object(item, member, name),
        Value::Array(array) => array.iter_mut().fold(false, |changed, child| rename(child, member, name) | changed),
        _ => false,
    }
}
