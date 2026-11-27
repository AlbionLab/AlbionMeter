use meter_store::{json, local_capture_files, CaptureSettings, CharacterContextStore, EncounterStore, LISTED_ENCOUNTERS, RETAINED_ENCOUNTERS};
use serde_json::{json, Value};

mod common;
use common::{snapshot, TempDir};

#[test]
fn an_explicitly_archived_empty_record_is_saved_and_automatic_empty_ones_are_not() {
    let directory = TempDir::new("empty");
    let store = EncounterStore::open(directory.path(), false).unwrap();
    let empty = snapshot("empty", "2026-11-01T10:00:00.5+00:00", 0.0);
    store.save(&empty, false).unwrap();
    assert!(store.list().unwrap().is_empty());
    store.save(&empty, true).unwrap();
    assert_eq!(store.list().unwrap()[0].id, "empty");
    let saved: Value = serde_json::from_str(&store.get("empty").unwrap().unwrap()).unwrap();
    assert_eq!(saved["status"], "completed");
    assert!(store.get("' OR 1=1 --").unwrap().is_none());
    let started: String = rusqlite::Connection::open(store.database_path()).unwrap().query_row("SELECT started_at FROM encounters", [], |row| row.get(0)).unwrap();
    assert_eq!(started, "2026-11-01T10:00:00.5000000+00:00");
}

#[test]
fn history_persists_across_instances_and_retention_drops_empty_records_first() {
    let directory = TempDir::new("retention");
    let store = EncounterStore::open(directory.path(), false).unwrap();
    for index in 0..20 {
        store.save(&snapshot(&format!("empty-{index}"), &format!("2026-01-01T00:00:{index:02}+00:00"), 0.0), true).unwrap();
    }
    for index in 0..RETAINED_ENCOUNTERS {
        store.save(&snapshot(&format!("fight-{index}"), &format!("2026-02-01T{:02}:{:02}:00+00:00", index / 60, index % 60), 1.0), false).unwrap();
    }
    assert!((0..20).all(|index| store.get(&format!("empty-{index}")).unwrap().is_none()));
    let reopened = EncounterStore::open(directory.path(), false).unwrap();
    assert_eq!(reopened.list().unwrap().len(), LISTED_ENCOUNTERS);
    assert_eq!(reopened.list().unwrap()[0].id, format!("fight-{}", RETAINED_ENCOUNTERS - 1));

    let mut run = json!({"id": "run", "startedAt": "2026-03-01T00:00:00+00:00", "name": "Random dungeon", "entranceMapId": "1316",
        "entranceMapName": "Hightree Lake", "mode": "live", "status": "running", "floors": [], "totals": snapshot("totals", "2026-03-01T00:00:00+00:00", 0.0)});
    reopened.save_dungeon(&run, false).unwrap();
    assert!(reopened.list_dungeons().unwrap().is_empty());
    run["totals"] = snapshot("totals", "2026-03-01T00:00:00+00:00", 5.0);
    reopened.save_dungeon(&run, false).unwrap();
    let summary = &reopened.list_dungeons().unwrap()[0];
    assert_eq!((summary.damage, summary.floors, summary.name.as_str()), (5.0, 0, "Random dungeon"));
    assert_eq!(json::to_string(&reopened.get_dungeon::<Value>("run").unwrap().unwrap()).unwrap(), json::to_string(&run).unwrap());
}

#[test]
fn skill_weapons_round_trip_in_key_order() {
    let directory = TempDir::new("skills");
    let store = EncounterStore::open(directory.path(), false).unwrap();
    let skills = std::collections::HashMap::from([(2679, std::collections::BTreeMap::from([(9618, 1), (9617, 2)]))]);
    store.observe_skill_weapons(&skills).unwrap();
    let json: String = rusqlite::Connection::open(store.database_path()).unwrap().query_row("SELECT weapons FROM skill_weapons", [], |row| row.get(0)).unwrap();
    assert_eq!(json, r#"{"9617":2,"9618":1}"#);
    assert_eq!(store.known_skill_weapons().unwrap()[&2679][&9617], 2);
}

#[test]
fn character_context_files_round_trip() {
    let directory = TempDir::new("context");
    let character = json!({"characterId": "9c0b222a-86fa-443e-a9e7-c76e167c39b1", "objectId": null, "name": "KPey", "guild": null, "alliance": "",
        "mapId": "@ISLAND@15cf", "destinationMapId": null, "locationState": "last-observed", "observedAtMicroseconds": 10, "connection": "server:0:1"});
    let party = json!({"members": [{"id": "a1736e32-cfb4-4c2d-9d41-7ef907f9191d", "name": "Trochowskyy"}], "observedAtMicroseconds": 5, "complete": true, "source": "capture", "listedAtMicroseconds": null, "partyId": null});
    {
        let store = CharacterContextStore::open(directory.path());
        assert!(store.snapshot().is_none());
        store.observe(&character);
        store.observe_party(&party);
        let mut older = party.clone();
        older["observedAtMicroseconds"] = 5.into();
        older["members"] = json!([]);
        store.observe_party(&older);
        store.observe_players(&character, &[json!({"connection": "server:0:1", "objectId": 505, "id": "9c0b222a-86fa-443e-a9e7-c76e167c39b1", "name": "KPey", "weapon": 0, "equipment": [0, null]})]);
        assert!(store.storage_error().is_none());
    }
    let captures = directory.path().join("captures");
    assert_eq!(std::fs::read_to_string(captures.join("local-character.json")).unwrap(), json::to_string(&character).unwrap());
    let reopened = CharacterContextStore::open(directory.path());
    assert_eq!(reopened.snapshot().unwrap(), character);
    assert_eq!(reopened.party().unwrap(), party);
    assert_eq!(reopened.recent_players(&character).len(), 1);
    let mut elsewhere = character.clone();
    elsewhere["mapId"] = "1301".into();
    assert!(reopened.recent_players(&elsewhere).is_empty());

    // Another character drops the saved party.
    let mut other = character.clone();
    other["characterId"] = "c1b88e43-9a80-4329-9641-fa43b34d3909".into();
    other["observedAtMicroseconds"] = 20.into();
    reopened.observe(&other);
    assert!(reopened.party().is_none() && !captures.join("local-party.json").exists());

    std::fs::write(captures.join("local-character.json"), r#"{"characterId":"00000000-0000-0000-0000-000000000000","name":"x"}"#).unwrap();
    assert!(CharacterContextStore::open(directory.path()).snapshot().is_none());
}

#[test]
fn capture_settings_and_recordings() {
    let directory = TempDir::new("capture");
    assert_eq!(CaptureSettings::load(directory.path()).device, None);
    CaptureSettings { device: Some("raw:any".into()) }.save(directory.path()).unwrap();
    assert_eq!(std::fs::read_to_string(directory.path().join("capture-settings.json")).unwrap(), "{\n  \"device\": \"raw:any\"\n}");
    assert_eq!(CaptureSettings::load(directory.path()).device.as_deref(), Some("raw:any"));

    let captures = directory.path().join("captures");
    std::fs::create_dir_all(&captures).unwrap();
    for (name, source) in [("a", "live-capture"), ("b", "replay"), ("c", "live-capture")] {
        std::fs::write(captures.join(format!("{name}.pcap")), [0u8; 24]).unwrap();
        std::fs::write(captures.join(format!("{name}.pcap.json")), json::to_string(&json!({"session": {"source": source}})).unwrap()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    std::fs::write(captures.join("d.pcap"), [0u8; 24]).unwrap();
    let files: Vec<String> = local_capture_files(&captures).iter().map(|path| path.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(files, ["a.pcap", "c.pcap"]);
}
