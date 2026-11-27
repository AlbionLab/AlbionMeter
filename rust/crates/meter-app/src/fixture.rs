//! The parity fixture (`fixtures/parity.json`): the same JSON the React app is served by `rust/scripts/ui-parity.mjs`
//! (`/api/*` shapes of the C# server), turned into feed events. A screen is the base document with that screen's
//! overrides merged over it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use crate::feed::*;

pub fn default_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/parity.json")
}

/// `over` merged into `base`: objects key by key, anything else replaced (null included).
fn merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(base), Value::Object(over)) => {
            for (key, value) in over {
                match base.get_mut(key) {
                    Some(target) if value.is_object() && target.is_object() => merge(target, value),
                    _ => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, over) => *base = over.clone(),
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct JsonPlayer {
    membership: Option<String>,
    id: String,
    name: String,
    weapon: String,
    weapon_family: String,
    color: String,
    damage: f64,
    healing: f64,
    encounter_dps: f64,
    encounter_hps: f64,
    damage_share: f64,
    healing_share: f64,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct JsonItem {
    kind: Option<String>,
    tier_observed: Option<bool>,
    id: String,
    player_id: String,
    player_name: String,
    item_index: Option<i32>,
    slot: String,
    item: String,
    damage: f64,
    healing: f64,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct JsonPoint {
    second: u32,
    damage: std::collections::HashMap<String, f64>,
    healing: std::collections::HashMap<String, f64>,
    item_damage: std::collections::HashMap<String, f64>,
    item_healing: std::collections::HashMap<String, f64>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct JsonSnapshot {
    id: String,
    started_at: String,
    elapsed_seconds: f64,
    status: String,
    mode: String,
    sequence: u64,
    event_count: u64,
    total_damage: f64,
    total_healing: f64,
    members: Vec<JsonPlayer>,
    former_members: Vec<JsonPlayer>,
    item_contributions: Vec<JsonItem>,
    timeline: Vec<JsonPoint>,
}

/// An item definition: `/api/catalog` indexedWeapons and `/api/items/{index}` (web/src/weapons.ts, ItemIcon.tsx).
#[derive(Deserialize, Clone)]
struct ItemDefinition {
    id: String,
    name: String,
    tier: Option<u8>,
    #[serde(default)]
    enchantment: u8,
}

impl ItemDefinition {
    fn tier(&self) -> Option<String> {
        self.tier.map(|tier| format!("T{tier}.{}", self.enchantment))
    }
}

#[derive(Default)]
struct Lookup {
    weapons: Vec<ItemDefinition>,
    items: std::collections::HashMap<i32, ItemDefinition>,
}

fn values(map: std::collections::HashMap<String, f64>) -> Values {
    map.into_iter().map(|(key, value)| (Arc::from(key), value)).collect()
}

/// weapons.ts: a player's weapon string is matched against the indexed weapons by "Name · T8.3".
fn player(json: JsonPlayer, lookup: &Lookup) -> Player {
    let weapon_id = lookup
        .weapons
        .iter()
        .find(|weapon| weapon.tier().is_some_and(|tier| format!("{} · {tier}", weapon.name) == json.weapon))
        .map(|weapon| Arc::from(weapon.id.as_str()));
    Player {
        membership: match json.membership.as_deref() {
            Some("left") => Membership::Left,
            Some("unconfirmed") => Membership::Unconfirmed,
            _ => Membership::Active,
        },
        id: json.id.into(),
        name: json.name.into(),
        weapon: json.weapon.into(),
        weapon_id,
        weapon_family: json.weapon_family.into(),
        color: json.color.into(),
        damage: json.damage,
        healing: json.healing,
        encounter_dps: json.encounter_dps,
        encounter_hps: json.encounter_hps,
        damage_share: json.damage_share,
        healing_share: json.healing_share,
    }
}

fn snapshot(value: &Value, lookup: &Lookup) -> Option<Snapshot> {
    let json: JsonSnapshot = serde_json::from_value(value.clone()).ok()?;
    Some(Snapshot {
        id: json.id.into(),
        started_at: json.started_at.into(),
        elapsed_seconds: json.elapsed_seconds,
        status: match json.status.as_str() {
            "running" => RecordStatus::Running,
            "paused" => RecordStatus::Paused,
            "completed" => RecordStatus::Completed,
            _ => RecordStatus::Idle,
        },
        mode: json.mode.into(),
        sequence: json.sequence,
        event_count: json.event_count,
        total_damage: json.total_damage,
        total_healing: json.total_healing,
        members: json.members.into_iter().map(|json| player(json, lookup)).collect(),
        former_members: json.former_members.into_iter().map(|json| player(json, lookup)).collect(),
        item_contributions: json
            .item_contributions
            .into_iter()
            .map(|item| {
                let definition = item.item_index.and_then(|index| lookup.items.get(&index));
                ItemContribution {
                kind: item.kind.map(Into::into),
                tier_observed: item.tier_observed.unwrap_or(true),
                id: item.id.into(),
                player_id: item.player_id.into(),
                player_name: item.player_name.into(),
                item_index: item.item_index,
                item_id: definition.map(|definition| definition.id.as_str().into()),
                slot: item.slot.into(),
                item: item.item.into(),
                tier: definition.and_then(ItemDefinition::tier).map(Into::into),
                damage: item.damage,
                healing: item.healing,
            }})
            .collect(),
        timeline: json
            .timeline
            .into_iter()
            .map(|point| Point { second: point.second, damage: values(point.damage), healing: values(point.healing), item_damage: values(point.item_damage), item_healing: values(point.item_healing) })
            .collect(),
        full: true,
    })
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(String::from)
}

fn lab(value: &Value) -> LabStatus {
    let session = value.get("session").filter(|session| session.is_object()).map(|session| LabSession {
        id: text(session, "id").unwrap_or_default().into(),
        url: text(session, "url").unwrap_or_default().into(),
        visibility: text(session, "visibility").unwrap_or_default().into(),
        anonymize: session.get("anonymize").and_then(Value::as_bool).unwrap_or(false),
        state: text(session, "state").unwrap_or_default().into(),
        error: text(session, "error"),
        title: text(session, "title"),
    });
    LabStatus {
        server: text(value, "server").unwrap_or_default().into(),
        connected: value.get("connected").and_then(Value::as_bool).unwrap_or(false),
        account: value.get("account").filter(|account| account.is_object()).map(|account| (text(account, "name").unwrap_or_default(), None)),
        pairing: value.get("pairing").filter(|pairing| pairing.is_object()).map(|pairing| (text(pairing, "code").unwrap_or_default(), text(pairing, "verifyUrl").unwrap_or_default())),
        pairing_error: text(value, "pairingError").map(Into::into),
        session,
        share_location: value.get("shareLocation").and_then(Value::as_bool).unwrap_or(false),
        visibility: text(value, "visibility").unwrap_or_else(|| "unlisted".into()).into(),
        anonymize: value.get("anonymize").and_then(Value::as_bool).unwrap_or(false),
        suggested_title: text(value, "suggestedTitle"),
    }
}

fn counters(value: Option<&Value>) -> Vec<(Arc<str>, f64)> {
    value.and_then(Value::as_object).map(|map| map.iter().map(|(key, value)| (Arc::from(key.as_str()), value.as_f64().unwrap_or(0.0))).collect()).unwrap_or_default()
}

fn capture(value: &Value) -> CaptureStatus {
    let report = value.get("report").filter(|report| report.is_object()).map(|report| {
        let mut all = counters(report.get("counters"));
        let diagnostics = counters(report.get("combatDiagnostics"));
        let pending = diagnostics.iter().find(|(name, _)| &**name == "pendingUnidentified").map_or(0.0, |(_, value)| *value);
        let transport = all.iter().find(|(name, _)| &**name == "transportMessages").map_or(0.0, |(_, value)| *value);
        // { ...counters, ...combatDiagnostics }: a later key replaces an earlier one in place.
        for (name, value) in diagnostics {
            match all.iter_mut().find(|(known, _)| *known == name) {
                Some(entry) => entry.1 = value,
                None => all.push((name, value)),
            }
        }
        CaptureReport {
            frames: report.get("frames").and_then(Value::as_u64).unwrap_or(0),
            bytes: report.get("bytes").and_then(Value::as_u64).unwrap_or(0),
            counters: all,
            pending_unidentified: pending as u64,
            transport_messages: transport as u64,
        }
    });
    CaptureStatus {
        state: text(value, "state").unwrap_or_default().into(),
        source: text(value, "source").unwrap_or_else(|| "none".into()).into(),
        file: text(value, "file"),
        error: text(value, "error"),
        stop_reason: text(value, "stopReason").map(Into::into),
        library: text(value, "library"),
        adapter_dropped: value.get("adapter").and_then(|adapter| adapter.get("dropped")).and_then(Value::as_u64),
        report,
    }
}

fn devices(value: &Value) -> DeviceList {
    let list = |key: &str| value.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    DeviceList {
        library: text(value, "library"),
        debug_enabled: value.get("debugEnabled").and_then(Value::as_bool).unwrap_or(false),
        selected_device: text(value, "selectedDevice").map(Into::into),
        methods: list("methods")
            .iter()
            .map(|method| CaptureMethod { id: text(method, "id").unwrap_or_default().into(), name: text(method, "name").unwrap_or_default().into(), available: method.get("available").and_then(Value::as_bool).unwrap_or(false) })
            .collect(),
        devices: list("devices")
            .iter()
            .map(|device| CaptureDevice {
                name: text(device, "name").unwrap_or_default().into(),
                description: text(device, "description").unwrap_or_default().into(),
                method: text(device, "method").unwrap_or_default().into(),
                flags: device.get("flags").and_then(Value::as_u64).unwrap_or(0) as u32,
            })
            .collect(),
        error: text(value, "error"),
    }
}

fn catalog(value: &Value) -> Catalog {
    let entries = |key: &str, group: &str| {
        value
            .get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| CatalogEntry {
                        id: text(item, "id").unwrap_or_default().into(),
                        name: text(item, "name").unwrap_or_default().into(),
                        group: text(item, group).unwrap_or_default().into(),
                        tier: item.get("tier").and_then(Value::as_u64).map(|tier| tier as u8),
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    Catalog { game_version: text(value, "gameVersion"), imported_at: text(value, "importedAt"), weapons: entries("weapons", "family"), maps: entries("maps", "type") }
}

fn update(value: &Value) -> UpdateState {
    let version = text(value, "version").unwrap_or_default();
    match text(value, "state").as_deref() {
        Some("downloading") => UpdateState::Downloading {
            version,
            percent: value.get("percent").and_then(Value::as_u64).unwrap_or(0) as u8,
            transferred: value.get("transferred").and_then(Value::as_u64),
            total: value.get("total").and_then(Value::as_u64),
        },
        Some("ready") => UpdateState::Ready { version },
        Some("installing") => UpdateState::Installing { version },
        Some("checking") => UpdateState::Checking,
        Some("current") => UpdateState::Current,
        _ => UpdateState::Idle,
    }
}

fn summary(value: &Value) -> DungeonSummary {
    let number = |key: &str| value.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    DungeonSummary {
        id: text(value, "id").unwrap_or_default().into(),
        started_at: text(value, "startedAt").unwrap_or_default().into(),
        name: text(value, "name").unwrap_or_default().into(),
        entrance_map_name: text(value, "entranceMapName").unwrap_or_default().into(),
        mode: text(value, "mode").unwrap_or_default().into(),
        status: text(value, "status").unwrap_or_default().into(),
        floors: number("floors") as u32,
        duration: number("duration"),
        damage: number("damage"),
        healing: number("healing"),
    }
}

fn dungeon(value: &Value, lookup: &Lookup) -> Option<DungeonRun> {
    let totals = Arc::new(snapshot(value.get("totals")?, lookup)?);
    let floors: Vec<DungeonFloor> = value
        .get("floors")
        .and_then(Value::as_array)
        .map(|floors| {
            floors
                .iter()
                .filter_map(|floor| {
                    Some(DungeonFloor {
                        id: text(floor, "id")?.into(),
                        number: floor.get("number").and_then(Value::as_u64)? as u32,
                        map_id: text(floor, "mapId").unwrap_or_default().into(),
                        map_name: text(floor, "mapName").unwrap_or_default().into(),
                        totals: Arc::new(snapshot(floor.get("totals")?, lookup)?),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut summary = summary(value);
    summary.floors = floors.len() as u32;
    summary.duration = totals.elapsed_seconds;
    summary.damage = totals.total_damage;
    summary.healing = totals.total_healing;
    Some(DungeonRun { summary, floors, totals })
}

/// What the UI itself is showing on a screen (the clicks the React side makes).
#[derive(Default)]
pub struct Ui {
    pub view: String,
    pub metric: String,
    pub language: String,
    pub confirm: String,
    /// Journal: the floor tab, "player" / "item", the timeline range and a key pressed on the chart.
    pub floor: u32,
    pub grouping: String,
    pub range: String,
    pub key: String,
    pub copy_menu: bool,
    /// The accessible label of the control the pointer rests on.
    pub hover: String,
    pub search: String,
    pub tab: String,
    /// "damage" / "healing": the screenshot is the copied ranking picture instead of the window.
    pub card: String,
    /// How far the main area is scrolled down, in pixels.
    pub scroll: f32,
}

pub struct Screen {
    pub events: Vec<FeedEvent>,
    pub ui: Ui,
    pub width: f32,
    pub height: f32,
}

pub fn screen(path: &Path, name: &str) -> Result<Screen, String> {
    let text_file = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut root: Value = serde_json::from_str(&text_file).map_err(|error| format!("{}: {error}", path.display()))?;
    let over = root.get("screens").and_then(|screens| screens.get(name)).cloned().ok_or_else(|| format!("no screen {name} in {}", path.display()))?;
    merge(&mut root, &over);
    let folder = path.parent().unwrap_or(Path::new("."));
    let lookup = Lookup {
        weapons: root.get("catalog").and_then(|catalog| catalog.get("indexedWeapons")).cloned().and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default(),
        items: root
            .get("items")
            .and_then(Value::as_object)
            .map(|items| items.iter().filter_map(|(index, item)| Some((index.parse().ok()?, serde_json::from_value(item.clone()).ok()?))).collect())
            .unwrap_or_default(),
    };
    let mut events = Vec::new();
    let connected = root.get("connected").and_then(Value::as_bool).unwrap_or(true);
    if let Some(version) = root.get("health").and_then(|health| text(health, "version")) {
        events.push(FeedEvent::Version(version));
    }
    if let Some(icons) = root.get("icons").and_then(Value::as_object) {
        for (id, file) in icons {
            if let Some(file) = file.as_str() {
                events.push(FeedEvent::Icon(id.as_str().into(), folder.join(file)));
            }
        }
    }
    if connected {
        events.push(FeedEvent::Connection(Connection::Connected));
        if let Some(snapshot) = root.get("snapshot").filter(|value| value.is_object()).and_then(|value| snapshot(value, &lookup)) {
            events.push(FeedEvent::Snapshot(Arc::new(snapshot)));
        }
        if let Some(value) = root.get("albionlab").filter(|value| value.is_object()) {
            events.push(FeedEvent::Lab(Arc::new(lab(value))));
        }
        if let Some(value) = root.get("devices").filter(|value| value.is_object()) {
            events.push(FeedEvent::Devices(Arc::new(devices(value))));
        }
        if let Some(value) = root.get("captureStatus").filter(|value| value.is_object()) {
            events.push(FeedEvent::Capture(Arc::new(capture(value))));
        }
        if let Some(value) = root.get("catalog").filter(|value| value.is_object()) {
            events.push(FeedEvent::Catalog(Arc::new(catalog(value))));
        }
        if let Some(path) = root.get("storage").and_then(|storage| text(storage, "databasePath")) {
            events.push(FeedEvent::StoragePath(path));
        }
        if let Some(runs) = root.get("dungeons").and_then(Value::as_array) {
            events.push(FeedEvent::Dungeons(Arc::new(runs.iter().map(summary).collect())));
        }
        if let Some(run) = root.get("dungeon").filter(|value| value.is_object()).and_then(|value| dungeon(value, &lookup)) {
            events.push(FeedEvent::Dungeon(Arc::new(run)));
        }
        if let Some(error) = text(&root, "commandError") {
            events.push(FeedEvent::CommandDone(Some(error)));
        }
    }
    if let Some(result) = root.get("historyResult").filter(|value| value.is_object()) {
        // HistoryTransfer.tsx's message for an import.
        let count = |key: &str| result.get(key).and_then(Value::as_u64).unwrap_or(0);
        let skipped = count("skippedEncounters") + count("skippedDungeons");
        let mut text = format!("Imported {} encounters and {} dungeons.", count("importedEncounters"), count("importedDungeons"));
        if skipped > 0 {
            text.push_str(&format!(" {skipped} existing records or records over the storage limit were skipped."));
        }
        events.push(FeedEvent::HistoryMessage { text, error: false });
    }
    if let Some(value) = root.get("update") {
        events.push(FeedEvent::Update(update(value)));
    }
    let ui = root.get("ui").cloned().unwrap_or_default();
    let window = root.get("window").cloned().unwrap_or_default();
    Ok(Screen {
        events,
        ui: Ui {
            view: text(&ui, "view").unwrap_or_else(|| "meter".into()),
            metric: text(&ui, "metric").unwrap_or_else(|| "damage".into()),
            language: text(&ui, "language").unwrap_or_else(|| "en".into()),
            confirm: text(&ui, "confirm").unwrap_or_default(),
            floor: ui.get("floor").and_then(Value::as_u64).unwrap_or(0) as u32,
            grouping: text(&ui, "grouping").unwrap_or_else(|| "player".into()),
            range: text(&ui, "range").unwrap_or_default(),
            key: text(&ui, "key").unwrap_or_default(),
            copy_menu: ui.get("copyMenu").and_then(Value::as_bool).unwrap_or(false),
            hover: text(&ui, "hover").unwrap_or_default(),
            search: text(&ui, "search").unwrap_or_default(),
            tab: text(&ui, "tab").unwrap_or_default(),
            card: text(&ui, "card").unwrap_or_default(),
            scroll: ui.get("scroll").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        },
        width: window.get("width").and_then(Value::as_f64).unwrap_or(1280.0) as f32,
        height: window.get("height").and_then(Value::as_f64).unwrap_or(820.0) as f32,
    })
}

/// Every screen named in the fixture.
#[cfg(test)]
pub fn screens(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|root| root.get("screens").and_then(Value::as_object).map(|screens| screens.keys().cloned().collect()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_screen_of_the_fixture_loads() {
        let path = default_path();
        let names = screens(&path);
        assert!(names.len() >= 15, "{names:?}");
        for name in names {
            let screen = screen(&path, &name).unwrap();
            assert!(!screen.events.is_empty(), "{name}");
        }
        let damage = screen(&path, "meter-damage").unwrap();
        let snapshot = damage.events.iter().find_map(|event| if let FeedEvent::Snapshot(snapshot) = event { Some(snapshot.clone()) } else { None }).unwrap();
        assert_eq!(snapshot.members[0].weapon_id.as_deref(), Some("T8_MAIN_FROSTSTAFF_AVALON@4"));
        assert_eq!(snapshot.members.iter().find(|player| &*player.name == "VoxBR1").unwrap().weapon_id, None);
    }

    #[test]
    fn overrides_merge_into_the_base() {
        let mut base = serde_json::json!({ "a": { "b": 1, "c": 2 }, "d": [1] });
        merge(&mut base, &serde_json::json!({ "a": { "c": 3 }, "d": null }));
        assert_eq!(base, serde_json::json!({ "a": { "b": 1, "c": 3 }, "d": null }));
    }
}
