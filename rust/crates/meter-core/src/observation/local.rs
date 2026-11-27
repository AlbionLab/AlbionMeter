//! The local character, its map and guild (`LocalCharacterObservation.cs`).

use crate::capture::photon::Connection;
use crate::capture::protocol18::{Value, WireMessage};
use crate::cs::{Guid, is_blank, utf16_len};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalCharacterSnapshot {
    pub character_id: Guid,
    pub object_id: Option<i64>,
    pub name: String,
    pub guild: Option<String>,
    pub alliance: Option<String>,
    pub map_id: Option<String>,
    pub destination_map_id: Option<String>,
    pub location_state: String,
    pub observed_at_microseconds: i64,
    #[serde(with = "connection")]
    pub connection: Connection,
}

mod connection {
    use super::Connection;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &Connection, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(value)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Connection, D::Error> {
        Ok(String::deserialize(deserializer)?.into())
    }
}

impl LocalCharacterSnapshot {
    /// A snapshot as the C# tests build them: `new(id, objectId, name, null, null, map, null, state, at, connection)`.
    pub fn new(character_id: Guid, object_id: Option<i64>, name: &str, map_id: Option<&str>, location_state: &str, observed_at: i64, connection: &str) -> Self {
        Self {
            character_id,
            object_id,
            name: name.into(),
            guild: None,
            alliance: None,
            map_id: map_id.map(Into::into),
            destination_map_id: None,
            location_state: location_state.into(),
            observed_at_microseconds: observed_at,
            connection: connection.into(),
        }
    }
}

#[derive(Default)]
pub struct LocalCharacterObservation {
    current: Option<LocalCharacterSnapshot>,
}

/// `value is string text && text.Length <= maximum && !text.Any(char.IsControl) ? text.Trim() : null`
pub(crate) fn text(value: Option<&Value>, maximum: usize) -> Option<String> {
    let text = value?.as_str()?;
    (utf16_len(text) <= maximum && !text.chars().any(char::is_control)).then(|| text.trim().to_owned())
}

fn map(value: Option<&Value>) -> Option<String> {
    let text = text(value, 512).or_else(|| value.and_then(Value::integer).map(|i| i.to_string()))?;
    (!is_blank(&text)).then_some(text)
}

fn identity(value: Option<&Value>) -> Option<Guid> {
    value?.as_guid().filter(|id| !id.is_empty())
}

impl LocalCharacterObservation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Option<&LocalCharacterSnapshot> {
        self.current.as_ref()
    }

    pub fn restore(&mut self, character: LocalCharacterSnapshot) {
        if character.character_id.is_empty() || is_blank(&character.name) {
            return;
        }
        self.current = Some(LocalCharacterSnapshot { object_id: None, connection: "".into(), location_state: "last-observed".into(), ..character });
    }

    pub fn observe(&mut self, message: &WireMessage, connection: &Connection, timestamp: i64) {
        if self.current.as_ref().is_some_and(|current| timestamp < current.observed_at_microseconds) {
            return;
        }
        if message.kind == 3 && message.return_code == Some(0) && let Some(operation) = message.integer(253) {
            if operation == 2 {
                let (Some(id), Some(name)) = (identity(message.get(1)), text(message.get(2), 100)) else { return };
                if is_blank(&name) {
                    return;
                }
                self.current = Some(LocalCharacterSnapshot {
                    character_id: id,
                    object_id: message.integer(0),
                    name,
                    guild: text(message.get(58), 200),
                    alliance: text(message.get(79), 100),
                    map_id: map(message.get(8)),
                    destination_map_id: None,
                    location_state: "observed".into(),
                    observed_at_microseconds: timestamp,
                    connection: connection.clone(),
                });
            } else if operation == 41 && let Some(known) = self.current.as_mut().filter(|known| known.connection == *connection) {
                let Some(destination) = map(message.get(0)) else { return };
                known.map_id = None;
                known.destination_map_id = Some(destination);
                known.location_state = "changing".into();
                known.observed_at_microseconds = timestamp;
            }
        } else if message.kind == 4
            && message.integer(252) == Some(29)
            && let Some(local) = self.current.as_mut()
            && local.connection == *connection
            && identity(message.get(7)) == Some(local.character_id)
        {
            local.object_id = message.integer(0).or(local.object_id);
            local.guild = text(message.get(8), 200);
            local.alliance = text(message.get(51), 100);
            local.observed_at_microseconds = timestamp;
        }
    }
}
