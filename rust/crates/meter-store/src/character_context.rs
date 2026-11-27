//! CharacterContextStore.cs: the local character, its party and the players around it, kept in `captures/` so a
//! restarted meter knows them before the game says so again. The snapshots stay JSON values until meter-core has
//! `LocalCharacterSnapshot`/`PartyRosterSnapshot`.

use crate::time::DateTimeOffset;
use crate::{json, paths};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

/// `known-players.json`: who is who on the current map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownPlayers {
    pub saved_at: DateTimeOffset,
    pub character_id: Uuid,
    pub map_id: Option<String>,
    pub connection: String,
    pub players: Vec<Value>,
}

#[derive(Default)]
struct State {
    character: Option<Value>,
    party: Option<Value>,
    storage_error: Option<String>,
}

pub struct CharacterContextStore {
    character_path: PathBuf,
    party_path: PathBuf,
    players_path: PathBuf,
    state: Mutex<State>,
}

fn character_id(value: &Value) -> Option<Uuid> {
    value.get("characterId").and_then(Value::as_str).and_then(|id| Uuid::parse_str(id).ok())
}

fn observed_at(value: &Value) -> i64 {
    value.get("observedAtMicroseconds").and_then(Value::as_i64).unwrap_or_default()
}

fn valid_character(value: &Value) -> bool {
    let length = value.get("name").and_then(Value::as_str).map_or(0, |name| name.encode_utf16().count());
    character_id(value).is_some_and(|id| !id.is_nil()) && (1..=100).contains(&length)
}

fn valid_party(value: &Value) -> bool {
    value.get("members").and_then(Value::as_array).is_some_and(|members| {
        members.len() <= 100
            && members.iter().all(|member| {
                member.get("id").and_then(Value::as_str).and_then(|id| Uuid::parse_str(id).ok()).is_some_and(|id| !id.is_nil())
                    && member.get("name").and_then(Value::as_str).is_some_and(|name| !name.trim().is_empty())
            })
    })
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

impl CharacterContextStore {
    pub fn open(storage: &Path) -> Self {
        let captures = paths::captures_directory(storage);
        let character_path = captures.join("local-character.json");
        let party_path = captures.join("local-party.json");
        let state = State {
            character: read(&character_path).filter(valid_character),
            party: read(&party_path).filter(valid_party),
            storage_error: None,
        };
        Self { players_path: captures.join("known-players.json"), character_path, party_path, state: Mutex::new(state) }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn snapshot(&self) -> Option<Value> {
        self.state().character.clone()
    }

    pub fn party(&self) -> Option<Value> {
        self.state().party.clone()
    }

    pub fn storage_error(&self) -> Option<String> {
        self.state().storage_error.clone()
    }

    pub fn observe_players(&self, character: &Value, players: &[Value]) {
        if players.is_empty() {
            return;
        }
        let value = KnownPlayers {
            saved_at: DateTimeOffset::now(),
            character_id: character_id(character).unwrap_or_default(),
            map_id: character.get("mapId").and_then(Value::as_str).map(str::to_owned),
            connection: character.get("connection").and_then(Value::as_str).unwrap_or_default().to_owned(),
            players: players.iter().take(200).cloned().collect(),
        };
        if let Err(error) = write(&self.players_path, &value) {
            self.state().storage_error = Some(error.to_string());
        }
    }

    /// The saved players, only when they are recent and from the map and connection the character was last seen on.
    pub fn recent_players(&self, character: &Value) -> Vec<Value> {
        let Some(value) = std::fs::read_to_string(&self.players_path).ok().and_then(|text| serde_json::from_str::<KnownPlayers>(&text).ok()) else { return vec![] };
        let age = DateTimeOffset::now().utc_ticks - value.saved_at.utc_ticks;
        if age > 5 * 60 * 10_000_000
            || Some(value.character_id) != character_id(character)
            || value.map_id.as_deref() != character.get("mapId").and_then(Value::as_str)
            || value.connection != character.get("connection").and_then(Value::as_str).unwrap_or_default()
        {
            return vec![];
        }
        value.players
    }

    pub fn observe_party(&self, value: &Value) {
        let mut state = self.state();
        if state.party.as_ref().is_some_and(|party| observed_at(value) <= observed_at(party)) {
            return;
        }
        state.party = Some(value.clone());
        if let Err(error) = write(&self.party_path, value) {
            state.storage_error = Some(error.to_string());
        }
    }

    pub fn observe(&self, value: &Value) {
        let mut state = self.state();
        if let Some(current) = &state.character
            && (current == value || observed_at(value) < observed_at(current))
        {
            return;
        }
        let changed_character = state.character.as_ref().is_some_and(|current| character_id(current) != character_id(value));
        state.character = Some(value.clone());
        if changed_character {
            state.party = None;
        }
        let result = (|| {
            if changed_character && self.party_path.exists() {
                std::fs::remove_file(&self.party_path)?;
            }
            write(&self.character_path, value)
        })();
        state.storage_error = result.err().map(|error| error.to_string());
    }
}

fn write(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    paths::write_atomic(path, json::to_string(value).map_err(std::io::Error::other)?.as_bytes())
}
