//! The local player's party list (`PartyRosterObservation.cs`).

use super::local::LocalCharacterSnapshot;
use crate::capture::protocol18::WireMessage;
use crate::cs::{Guid, is_blank, utf16_len};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyRosterMember {
    pub id: Guid,
    pub name: String,
}

impl PartyRosterMember {
    pub fn new(id: Guid, name: &str) -> Self {
        Self { id, name: name.into() }
    }
}

/// The C# record compares `Members` by reference, so the members are shared and [`PartyRosterSnapshot::same`] compares
/// them by pointer: the combat observation re-reads a roster only when it is a new one.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyRosterSnapshot {
    pub members: Arc<[PartyRosterMember]>,
    pub observed_at_microseconds: i64,
    pub complete: bool,
    pub source: String,
    pub listed_at_microseconds: Option<i64>,
    pub party_id: Option<i64>,
    /// The computed `Listed` property, which System.Text.Json writes too.
    #[serde(default, skip_deserializing)]
    listed: i64,
}

impl PartyRosterSnapshot {
    pub fn new(members: Vec<PartyRosterMember>, observed_at: i64, complete: bool) -> Self {
        Self::build(members.into(), observed_at, complete, "capture".into(), None, None)
    }
    pub fn build(members: Arc<[PartyRosterMember]>, observed_at: i64, complete: bool, source: String, listed_at: Option<i64>, party_id: Option<i64>) -> Self {
        Self {
            members,
            observed_at_microseconds: observed_at,
            complete,
            source,
            listed_at_microseconds: listed_at,
            party_id,
            listed: listed_at.unwrap_or(observed_at),
        }
    }
    pub fn with_source(&self, source: &str) -> Self {
        Self { source: source.into(), ..self.clone() }
    }
    pub fn listed(&self) -> i64 {
        self.listed_at_microseconds.unwrap_or(self.observed_at_microseconds)
    }
    pub fn is_confirmed_since(&self, capture_started: Option<i64>) -> bool {
        self.source == "user-confirmed" || self.source == "capture" && capture_started.is_some_and(|started| self.listed() >= started)
    }
    /// Record equality as C# evaluates it (members by reference).
    pub fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.members, &other.members)
            && self.observed_at_microseconds == other.observed_at_microseconds
            && self.complete == other.complete
            && self.source == other.source
            && self.listed_at_microseconds == other.listed_at_microseconds
            && self.party_id == other.party_id
    }
    /// Fixes the serialized `listed` after deserialization.
    pub fn normalized(mut self) -> Self {
        self.listed = self.listed();
        self
    }
}

pub fn same_roster(a: Option<&PartyRosterSnapshot>, b: Option<&PartyRosterSnapshot>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same(b),
        _ => false,
    }
}

#[derive(Default)]
pub struct PartyRosterObservation {
    current: Option<PartyRosterSnapshot>,
    local_id: Option<Guid>,
}

pub(crate) fn valid_name(name: &str) -> bool {
    !is_blank(name) && utf16_len(name) <= 100 && !name.chars().any(char::is_control)
}

impl PartyRosterObservation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Option<&PartyRosterSnapshot> {
        self.current.as_ref()
    }

    pub fn restore(&mut self, value: PartyRosterSnapshot) -> bool {
        if self.current.as_ref().is_some_and(|current| value.observed_at_microseconds <= current.observed_at_microseconds) {
            return false;
        }
        let distinct: HashSet<Guid> = value.members.iter().map(|m| m.id).collect();
        if value.members.len() > 100 || value.members.iter().any(|m| m.id.is_empty() || !valid_name(&m.name)) || distinct.len() != value.members.len() {
            return false;
        }
        if let Some(id) = self.local_id
            && !value.members.is_empty()
            && !value.members.iter().any(|m| m.id == id)
        {
            return false;
        }
        self.current = Some(value.normalized());
        true
    }

    fn without(&self, id: Guid, timestamp: i64) -> PartyRosterSnapshot {
        if Some(id) == self.local_id {
            return PartyRosterSnapshot::build(Arc::from([]), timestamp, true, "capture".into(), Some(timestamp), None);
        }
        let members: Vec<PartyRosterMember> = self.current.iter().flat_map(|c| c.members.iter()).filter(|m| m.id != id).cloned().collect();
        PartyRosterSnapshot::build(
            members.into(),
            timestamp,
            self.current.as_ref().is_some_and(|c| c.complete),
            "capture".into(),
            Some(self.current.as_ref().map_or(0, |c| c.listed())),
            self.current.as_ref().and_then(|c| c.party_id),
        )
    }

    pub fn observe(&mut self, message: &WireMessage, local: Option<&LocalCharacterSnapshot>, timestamp: i64) {
        if let Some(local) = local
            && self.local_id != Some(local.character_id)
        {
            if self.local_id.is_some() {
                self.current = None;
            }
            self.local_id = Some(local.character_id);
        }
        if message.kind != 4 || self.current.as_ref().is_some_and(|c| timestamp < c.observed_at_microseconds) {
            return;
        }
        let Some(code) = message.integer(252) else { return };
        match code {
            230 => {
                let Some(bytes) = message.get(8).and_then(|v| v.as_bytes()) else { return };
                if bytes.is_empty() || bytes.len() % 16 != 0 || bytes.len() > 1600 {
                    return;
                }
                let Some(names) = message.get(9).filter(|v| v.array_len() == Some(bytes.len() / 16)) else { return };
                let mut members = Vec::new();
                for (index, chunk) in bytes.as_chunks::<16>().0.iter().enumerate() {
                    let id = Guid(*chunk);
                    let element = names.element(index);
                    let Some(name) = element.as_str().filter(|name| !id.is_empty() && valid_name(name)) else { return };
                    members.push(PartyRosterMember::new(id, name));
                }
                if members.iter().map(|m| m.id).collect::<HashSet<_>>().len() != members.len() {
                    return;
                }
                if let Some(local) = self.local_id
                    && !members.iter().any(|m| m.id == local)
                {
                    return;
                }
                self.current = Some(PartyRosterSnapshot::build(members.into(), timestamp, true, "capture".into(), Some(timestamp), message.integer(0)));
            }
            231 => self.current = Some(PartyRosterSnapshot::build(Arc::from([]), timestamp, true, "capture".into(), Some(timestamp), None)),
            235 => {
                let Some(id) = message.get(1).and_then(|v| v.as_guid()).filter(|id| !id.is_empty()) else { return };
                self.current = Some(self.without(id, timestamp));
            }
            232 | 234 => {
                let Some(id) = message.get(1).and_then(|v| v.as_guid()) else { return };
                if id.is_empty() {
                    return;
                }
                if code == 234 {
                    self.current = Some(self.without(id, timestamp));
                    return;
                }
                let Some(name) = message.get(2).and_then(|v| v.as_str()).filter(|name| valid_name(name)) else { return };
                let mut members: Vec<PartyRosterMember> = self.current.iter().flat_map(|c| c.members.iter()).filter(|m| m.id != id).cloned().collect();
                members.push(PartyRosterMember::new(id, name));
                if members.len() <= 100 {
                    self.current = Some(PartyRosterSnapshot::build(
                        members.into(),
                        timestamp,
                        self.current.as_ref().is_some_and(|c| c.complete),
                        "capture".into(),
                        Some(self.current.as_ref().map_or(0, |c| c.listed())),
                        self.current.as_ref().and_then(|c| c.party_id),
                    ));
                }
            }
            _ => {}
        }
    }
}
