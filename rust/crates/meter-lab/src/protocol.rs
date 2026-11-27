//! The wire types of AlbionLab session protocol v1 (SessionProtocol.cs), serialized as its `ProtocolJson` context:
//! camelCase, nulls left out.

use indexmap::IndexMap;
use meter_store::DateTimeOffset;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const VERSION: i32 = 1;
pub const HUB_PATH_PUBLISH: &str = "/hubs/publish";
pub const HUB_PATH_WATCH: &str = "/hubs/watch";
pub const MAXIMUM_PLAYERS: usize = 100;
pub const MAXIMUM_KEYS: usize = 5_000;
pub const MAXIMUM_SNAPS: usize = 500;
pub const MAXIMUM_SECONDS: i32 = 4 * 3600;
pub const MAXIMUM_ENTRIES_PER_SECOND: usize = 1_000;
pub const MAXIMUM_DUNGEONS: usize = 100;
pub const MAXIMUM_DELTA_ENTRIES: usize = 5_000;
pub const MAXIMUM_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerInfo {
    pub name: String,
    pub weapon: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub weapon_id: Option<String>,
    pub family: String,
    pub color: String,
}

/// One source of damage and healing: a player total ("p"), a player's weapon ("w") or one item and kind ("i").
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceKey {
    pub t: String,
    pub p: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub x: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub s: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub k: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub n: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub u: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub o: Option<bool>,
}

/// An entry of a second: [key index, damage, healing].
pub type Entry = [f64; 3];

/// One recorded segment: the whole record, or one dungeon encounter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snap {
    pub id: String,
    pub started_at: DateTimeOffset,
    pub elapsed: f64,
    pub status: String,
    pub events: i64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub members: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub membership: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub sec: Option<BTreeMap<i32, Vec<Entry>>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FloorInfo {
    pub id: String,
    pub number: i32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub map: Option<String>,
    pub snaps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DungeonInfo {
    pub id: String,
    pub name: String,
    pub started_at: DateTimeOffset,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub entrance: Option<String>,
    pub floors: Vec<FloorInfo>,
}

/// What changed since the last delta. With `reset`, it replaces the whole state (sent after a reconnect).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDelta {
    pub v: i32,
    pub seq: i64,
    pub reset: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub started_at: Option<DateTimeOffset>,
    #[serde(rename = "self", skip_serializing_if = "Option::is_none", default)]
    pub self_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub players: Option<IndexMap<String, PlayerInfo>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub keys: Option<BTreeMap<usize, SourceKey>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub snaps: Option<IndexMap<String, Snap>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub record: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dungeons: Option<Vec<DungeonInfo>>,
    /// Record second → a hash of the map the local player was on from then.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub places: Option<BTreeMap<i32, String>>,
}

impl Default for SessionDelta {
    fn default() -> Self {
        Self { v: VERSION, seq: 0, reset: false, status: None, started_at: None, self_id: None, players: None, keys: None, snaps: None, record: None, dungeons: None, places: None }
    }
}

impl SessionDelta {
    /// `SessionState.CountEntries`.
    pub fn count_entries(&self) -> usize {
        self.snaps.iter().flat_map(|snaps| snaps.values()).flat_map(|snap| snap.sec.iter().flat_map(|sec| sec.values())).map(Vec::len).sum()
    }
}
