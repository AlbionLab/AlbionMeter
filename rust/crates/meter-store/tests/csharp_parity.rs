//! Parity with the C# EncounterStore: the fixtures were written by the C# server (EncounterStore, HistoryArchiveFile)
//! from a throwaway harness; the Rust store must read them and write the same bytes.

use meter_store::{json, EncounterStore, HistoryArchive, HistoryImportResult, IdentitySighting, KnownCharacter, UnidentifiedPlayer};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

mod common;
use common::TempDir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn text(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap()
}

fn master(path: &Path) -> Vec<Value> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut statement = connection.prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY name").unwrap();
    statement
        .query_map([], |row| {
            Ok(serde_json::json!({"type": row.get::<_, String>(0)?, "name": row.get::<_, String>(1)?, "tbl_name": row.get::<_, String>(2)?, "sql": row.get::<_, Option<String>>(3)?}))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn a_fresh_database_has_the_csharp_schema() {
    let directory = TempDir::new("schema");
    let store = EncounterStore::open(directory.path(), false).unwrap();
    assert_eq!(store.database_path(), directory.path().join("encounters.db"));
    drop(store);
    // Opening an existing database again runs the migrations without changing anything.
    EncounterStore::open(directory.path(), false).unwrap();
    let expected: Vec<Value> = serde_json::from_str(&text("schema.json")).unwrap();
    assert_eq!(master(&directory.path().join("encounters.db")), expected);
}

#[test]
fn a_csharp_history_file_imports_to_the_same_rows() {
    let directory = TempDir::new("import");
    let store = EncounterStore::open(directory.path(), false).unwrap();
    let archive = HistoryArchive::read(&std::fs::read(fixture("history.zip")).unwrap()).unwrap();
    let result = store.import_history(&archive).unwrap();
    assert_eq!(result, serde_json::from_str::<HistoryImportResult>(&text("import.json")).unwrap());
    let encounter: Value = serde_json::from_str(&text("imported-encounter.json")).unwrap();
    let id = encounter["id"].as_str().unwrap();
    assert_eq!(store.get(id).unwrap().unwrap(), text("imported-encounter.json"));
    let dungeon: Value = serde_json::from_str(&text("imported-dungeon.json")).unwrap();
    let run: Value = store.get_dungeon(dungeon["id"].as_str().unwrap()).unwrap().unwrap();
    assert_eq!(json::to_string(&run).unwrap(), text("imported-dungeon.json"));
    assert_eq!(json::to_string(&store.list().unwrap()).unwrap(), text("list.json"));
    assert_eq!(json::to_string(&store.list_dungeons().unwrap()).unwrap().replace("\"completed\"", "\"running\""), text("dungeons.json"));

    let again = store.import_history(&archive).unwrap();
    assert_eq!((again.imported_encounters, again.imported_dungeons, again.skipped_encounters, again.skipped_dungeons), (0, 0, 1, 1));
    assert!(store.export_history(Some(id), None).unwrap().dungeons.is_empty());
    assert!(store.export_history(None, Some(dungeon["id"].as_str().unwrap())).unwrap().encounters.is_empty());
    assert!(matches!(store.export_history(Some("missing"), None), Err(meter_store::StoreError::NotFound(_))));
    assert!(matches!(store.export_history(Some(id), Some("x")), Err(meter_store::StoreError::Argument(_))));

    // The Rust export reads back (as ZIP and as bare JSON with a BOM) to the same archive.
    let exported = store.export_history(None, None).unwrap();
    let bytes = exported.write().unwrap();
    assert_eq!(&bytes[..2], b"PK");
    assert_eq!(HistoryArchive::read(&bytes).unwrap(), exported);
    let mut bom = vec![0xEF, 0xBB, 0xBF];
    bom.extend(json::to_string(&exported).unwrap().into_bytes());
    assert_eq!(HistoryArchive::read(&bom).unwrap().dungeons[0]["id"], dungeon["id"]);
    if let Some(out) = std::env::var_os("METER_STORE_EXPORT") {
        std::fs::write(out, bytes).unwrap();
    }
}

#[test]
fn invalid_history_never_partially_imports_or_replaces_records() {
    let directory = TempDir::new("invalid");
    let store = EncounterStore::open(directory.path(), false).unwrap();
    let valid: Value = serde_json::from_str(&text("encounter.json")).unwrap();
    let mut negative = valid.clone();
    negative["id"] = "other".into();
    negative["totalDamage"] = (-1).into();
    let archive = HistoryArchive::new(vec![valid.clone(), negative], vec![]);
    assert!(matches!(store.import_history(&archive), Err(meter_store::StoreError::Invalid(_))));
    assert!(store.list().unwrap().is_empty());
    let mut version = HistoryArchive::new(vec![valid.clone()], vec![]);
    version.version = 2;
    assert!(version.write().is_err());
    assert!(HistoryArchive::read(b"{}").is_err());
    assert!(HistoryArchive::read(b"invalid").is_err());
    let mut duplicate = valid.clone();
    duplicate["members"][1]["id"] = "a".into();
    assert!(HistoryArchive::new(vec![duplicate], vec![]).validate().is_err());
    let mut control = valid.clone();
    control["timeline"][0]["damage"] = serde_json::json!({"a\u{1}": 1});
    assert!(HistoryArchive::new(vec![control], vec![]).validate().is_err());

    store.save(&valid, false).unwrap();
    let mut changed = valid.clone();
    changed["totalDamage"] = 999.into();
    store.import_history(&HistoryArchive::new(vec![changed], vec![])).unwrap();
    let saved: Value = serde_json::from_str(&store.get(valid["id"].as_str().unwrap()).unwrap().unwrap()).unwrap();
    assert_eq!(saved["totalDamage"], 320);

    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("../history.json", zip::write::SimpleFileOptions::default()).unwrap();
    std::io::Write::write_all(&mut zip, b"{}").unwrap();
    assert!(HistoryArchive::read(&zip.finish().unwrap().into_inner()).is_err());
}

fn guid(n: u32) -> Uuid {
    Uuid::parse_str(&format!("00000000-0000-0000-0000-{n:012}")).unwrap()
}

fn character(name: &str, equipment: Option<Vec<Option<i32>>>, skills: Option<Vec<i32>>) -> KnownCharacter {
    KnownCharacter { name: name.into(), equipment, skills }
}

fn player(member: &str, record: &str, object: i64, candidates: Vec<Uuid>, timestamp: i64, skills: Option<Vec<i32>>, visit: Option<&str>) -> UnidentifiedPlayer {
    UnidentifiedPlayer { member_id: member.into(), record_id: Some(record.into()), object_id: object, equipment: None, candidates, timestamp, skills, visit: visit.map(Into::into) }
}

fn sighting(character: Uuid, object: i64, visit: &str, timestamp: i64) -> IdentitySighting {
    IdentitySighting { character_id: character, object_id: object, visit: visit.into(), timestamp, method: "chat".into() }
}

/// Saves the scenario's records as the C# store saved them, runs `observe`, and compares the resolutions and the
/// rewritten records with the C# ones byte for byte.
fn scenario(store: &EncounterStore, scenarios: &Value, name: &str, observe: impl FnOnce(&EncounterStore) -> Vec<meter_store::IdentityResolution>) {
    let scenario = &scenarios[name];
    for (id, before) in scenario["before"].as_object().unwrap() {
        let snapshot: Value = serde_json::from_str(before.as_str().unwrap()).unwrap();
        store.save(&snapshot, false).unwrap();
        assert_eq!(store.get(id).unwrap().unwrap(), before.as_str().unwrap(), "{name}: saved {id}");
    }
    let resolved: Vec<Value> = observe(store)
        .into_iter()
        .map(|resolution| serde_json::json!({"memberId": resolution.member_id, "characterId": resolution.character_id.to_string(), "name": resolution.name, "method": resolution.method}))
        .collect();
    assert_eq!(Value::Array(resolved), scenario["resolved"], "{name}: resolutions");
    if let Some(resave) = scenario["resave"].as_str() {
        store.save(&serde_json::from_str::<Value>(resave).unwrap(), false).unwrap();
    }
    for (id, after) in scenario["after"].as_object().unwrap() {
        assert_eq!(store.get(id).unwrap().unwrap(), after.as_str().unwrap(), "{name}: {id} after");
    }
}

#[test]
fn identities_name_saved_players_as_the_csharp_store_does() {
    let directory = TempDir::new("identity");
    let store = EncounterStore::open(directory.path(), false).unwrap();
    let scenarios: Value = serde_json::from_str(&text("identity.json")).unwrap();
    let none = std::iter::empty::<&IdentitySighting>();

    scenario(&store, &scenarios, "elimination", |store| {
        let (delta, echo) = (guid(1), guid(2));
        let gear = vec![Some(7696), Some(0), Some(4224), Some(5452), Some(5472), Some(2909), Some(2587), Some(3098)];
        let characters = HashMap::from([(delta, character("Delta", Some(gear), None)), (echo, character("Echo", None, None))]);
        let mut first = player("unidentified:5:1000", "rec-elim-1", 5, vec![delta, echo], 1_000_000, None, None);
        first.equipment = Some(vec![Some(7696), Some(0), Some(4224), Some(5452), Some(5472), Some(2909), Some(2587), Some(1111)]);
        let second = player("unidentified:6:1000", "rec-elim-2", 6, vec![delta, echo], 1_000_000, None, None);
        store.observe_identities(&characters, [&first, &second], none.clone()).unwrap()
    });
    scenario(&store, &scenarios, "merge", |store| {
        let characters = HashMap::from([(guid(3), character("Sick", None, None))]);
        let unnamed = player("unidentified:355:0", "rec-merge", 355, vec![], 1, None, Some("merge-visit"));
        store.observe_identities(&characters, [&unnamed], [&sighting(guid(3), 355, "merge-visit", 2)]).unwrap()
    });
    scenario(&store, &scenarios, "group", |store| {
        let (a, b, c, mei) = (guid(11), guid(12), guid(13), guid(14));
        let archer = vec![3053, 3054, 3074];
        let characters: Vec<(Uuid, KnownCharacter)> = vec![
            (a, character("lusphu", None, Some(archer.clone()))),
            (b, character("FlameKings", None, Some(archer.clone()))),
            (c, character("LupaMagica", None, Some(archer.clone()))),
            (mei, character("MistressMei", None, Some(vec![214, 3215, 3216, 3270]))),
        ];
        let players: Vec<UnidentifiedPlayer> = (0..4)
            .map(|index| {
                let candidates = if index < 3 { vec![a, b, c, mei] } else { vec![mei] };
                let skills = if index < 3 { archer.clone() } else { vec![3516, 3518, 3524, 3554] };
                player(&format!("unidentified:{}:1", 21 + index), "rec-group", 21 + index, candidates, 1, Some(skills), None)
            })
            .collect();
        store.observe_identities(characters.iter().map(|(id, character)| (id, character)), &players, none.clone()).unwrap()
    });
    scenario(&store, &scenarios, "resave", |store| {
        let characters = HashMap::from([(guid(4), character("Later", None, None))]);
        let unnamed = player("unidentified:77:0", "rec-resave", 77, vec![], 1, None, Some("resave-visit"));
        store.observe_identities(&characters, [&unnamed], [&sighting(guid(4), 77, "resave-visit", 2)]).unwrap()
    });

    let known = store.known_characters().unwrap();
    let expected: serde_json::Map<String, Value> = serde_json::from_str(&text("characters.json")).unwrap();
    assert_eq!(known.len(), expected.len());
    for (id, value) in expected {
        let character = &known[&Uuid::parse_str(&id).unwrap()];
        assert_eq!(serde_json::json!({"name": character.name, "equipment": character.equipment, "skills": character.skills}), value, "{id}");
    }
    // Nothing new: no reconciliation, nothing written.
    assert!(store.observe_identities(&HashMap::from([(guid(4), character("Later", None, None))]), [], none).unwrap().is_empty());
}
