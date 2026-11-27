//! Splits the captured combat into dungeon runs and floors (`DungeonCombatObservation.cs`).

use super::MessageId;
use super::combat::{CapturedCombatObservation, IdentitySighting, IdentityTimelineEntry, KnownCharacter, KnownPlayer, UnidentifiedPlayer};
use super::local::LocalCharacterSnapshot;
use super::roster::PartyRosterSnapshot;
use crate::capture::photon::Connection;
use crate::capture::protocol18::WireMessage;
use crate::catalog::CombatSourceCatalog;
use crate::cs::{CsMap, DateTimeOffset, Guid, is_blank};
use crate::dungeon::{DungeonRun, DungeonRunSnapshot};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone)]
struct Arrival {
    message: WireMessage<'static>,
    connection: Connection,
    identity: MessageId,
    timestamp: i64,
    paused: bool,
}

struct Run {
    serial: u64,
    character: Guid,
    entrance: String,
    run: DungeonRun,
}

const MAXIMUM_ARRIVALS: usize = 8192;
const ARRIVAL_WINDOW: i64 = 5_000_000;
const CAPTURE_GAP_MICROSECONDS: i64 = 30_000_000;

pub fn kind(map: Option<&str>) -> Option<&'static str> {
    let map = map?;
    let starts = |prefix: &str| map.get(..prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(prefix));
    if starts("@RANDOMDUNGEON@") {
        Some("Random dungeon")
    } else if starts("@EXPEDITION@") {
        Some("Expedition")
    } else {
        None
    }
}

fn is_dungeon(map: Option<&str>) -> bool {
    kind(map).is_some()
}

fn identity(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

pub struct DungeonCombatObservation {
    combat: CapturedCombatObservation,
    runs: Vec<Run>,
    serial: u64,
    active: Option<u64>,
    character: Option<Guid>,
    map: Option<String>,
    outside_map: Option<String>,
    latest_timestamp: i64,
    paused: bool,
    recording: bool,
    record_id: Option<String>,
    last_location: Option<(Guid, String, String)>,
    arrivals: Vec<Arrival>,
    templates: CsMap<String, String>,
    zone_connection: Option<Connection>,
    transition_at: Option<i64>,
    last_frame: i64,
}

impl DungeonCombatObservation {
    pub fn new(sources: Option<Arc<CombatSourceCatalog>>) -> Self {
        Self {
            combat: CapturedCombatObservation::new(sources),
            runs: Vec::new(),
            serial: 0,
            active: None,
            character: None,
            map: None,
            outside_map: None,
            latest_timestamp: 0,
            paused: false,
            recording: true,
            record_id: None,
            last_location: None,
            arrivals: Vec::new(),
            templates: CsMap::new(),
            zone_connection: None,
            transition_at: None,
            last_frame: 0,
        }
    }

    fn active_run(&mut self) -> Option<&mut DungeonRun> {
        let serial = self.active?;
        self.runs.iter_mut().find(|r| r.serial == serial).map(|r| &mut r.run)
    }

    fn add_run(&mut self, character: Guid, entrance: String, run: DungeonRun) {
        self.serial += 1;
        self.runs.push(Run { serial: self.serial, character, entrance, run });
        self.active = Some(self.serial);
    }

    pub fn set_recording(&mut self, enabled: bool, timestamp: i64) {
        if !enabled {
            self.arrivals.clear();
            let at = self.bounded(timestamp);
            self.combat.set_paused(true, at);
            self.checkpoint(at, "completed");
            self.active = None;
            self.map = None;
        }
        self.recording = enabled;
    }

    pub fn reset_recording(&mut self, timestamp: i64, record_id: &str) {
        self.arrivals.clear();
        self.runs.clear();
        self.active = None;
        self.map = None;
        self.paused = false;
        self.recording = true;
        self.record_id = Some(record_id.into());
        self.combat.begin_session(Some(timestamp), None);
        let Some((character, map, entrance)) = self.last_location.clone().filter(|l| is_dungeon(Some(&l.1))) else { return };
        self.character = Some(character);
        self.map = Some(map.clone());
        let run = DungeonRun::new(
            kind(Some(&map)).unwrap(),
            &entrance,
            &entrance,
            "live",
            Some(identity(&format!("{record_id}:{character}:{map}"))),
            Some(DateTimeOffset::from_unix_milliseconds(timestamp / 1000)),
        );
        self.add_run(character, entrance, run);
        let floor_name = self.floor_name(&map);
        let run = self.active_run().unwrap();
        let _ = run.enter_floor(&map, &map, &floor_name);
        let session = identity(&format!("{}:{timestamp}", run.id()));
        self.combat.begin_session(Some(timestamp), Some(session));
    }

    /// Reset: the runs start again from the floor the party is on, keeping that floor's players listed.
    pub fn reset_counts(&mut self, timestamp: i64, record_id: &str) {
        let members = self.combat.snapshot().map(|s| s.members).unwrap_or_default();
        self.reset_recording(timestamp, record_id);
        self.combat.carry(&members);
    }

    pub fn restore_players(&mut self, players: &[KnownPlayer]) {
        self.combat.restore_players(players);
    }

    fn floor_name(&self, map: &str) -> String {
        self.templates.get(map).cloned().unwrap_or_else(|| kind(Some(map)).unwrap_or_default().into())
    }

    fn bounded(&self, timestamp: i64) -> i64 {
        if self.last_frame > 0 && timestamp - self.last_frame > CAPTURE_GAP_MICROSECONDS { self.last_frame } else { timestamp }
    }

    pub fn observe(&mut self, wire: &WireMessage, connection: &Connection, message_id: &MessageId, timestamp: i64, local: Option<&LocalCharacterSnapshot>, party: Option<&PartyRosterSnapshot>) {
        let ok = wire.kind == 3 && wire.return_code == Some(0);
        let op = wire.integer(253);
        if ok
            && op == Some(41)
            && let Some(destination) = wire.get(0).and_then(|v| v.as_str()).filter(|d| is_dungeon(Some(d)))
            && let Some(template) = wire.get(1).and_then(|v| v.as_str()).filter(|t| !is_blank(t))
        {
            if self.templates.len() >= 256 {
                self.templates.clear();
            }
            self.templates.insert(destination.into(), template.into());
        }
        let join = ok
            && op == Some(2)
            && local.is_some_and(|l| {
                l.location_state == "observed"
                    && l.map_id.is_some()
                    && wire.get(8).and_then(|v| v.as_str()) == l.map_id.as_deref()
                    && wire.get(1).and_then(|v| v.as_guid()) == Some(l.character_id)
            });
        if join {
            let local = local.unwrap();
            let mut entrance = wire.get(66).and_then(|v| v.as_str()).map(str::to_owned);
            if !is_dungeon(local.map_id.as_deref()) {
                self.outside_map = local.map_id.clone();
            }
            if entrance.as_deref().is_none_or(is_blank) || is_dungeon(entrance.as_deref()) {
                entrance = Some(self.outside_map.clone().unwrap_or_else(|| "unknown".into()));
            }
            self.last_location = Some((local.character_id, local.map_id.clone().unwrap(), entrance.unwrap()));
        }
        if !self.recording {
            self.latest_timestamp = self.latest_timestamp.max(timestamp);
            self.last_frame = self.last_frame.max(timestamp);
            self.combat.observe(wire, connection, message_id, timestamp, local, party);
            return;
        }
        if self.transition_at.is_some() && wire.kind == 3 && wire.return_code != Some(0) && matches!(op, Some(41 | 237)) {
            let zone = self.zone_connection.clone();
            let arrivals: Vec<Arrival> = self.arrivals.iter().filter(|a| Some(&a.connection) == zone.as_ref()).cloned().collect();
            for arrival in &arrivals {
                self.replay_arrival(arrival, local, party);
            }
            self.arrivals.clear();
            self.transition_at = None;
            self.combat.set_paused(self.paused, timestamp);
        }
        let zone = self.zone_connection.clone().or_else(|| local.map(|l| l.connection.clone()));
        if !join && (Some(connection) != zone.as_ref() || self.transition_at.is_some_and(|leaving| timestamp >= leaving)) {
            self.combat.observe_connection(connection, timestamp);
            self.arrivals.retain(|a| timestamp - a.timestamp <= ARRIVAL_WINDOW);
            if self.arrivals.len() == MAXIMUM_ARRIVALS {
                self.arrivals.remove(0);
            }
            self.arrivals.push(Arrival { message: wire.into_owned(), connection: connection.clone(), identity: message_id.clone(), timestamp, paused: self.paused });
            return;
        }
        self.latest_timestamp = self.latest_timestamp.max(timestamp);
        self.last_frame = self.last_frame.max(timestamp);
        let arrivals: Vec<Arrival> = if join {
            self.arrivals
                .iter()
                .filter(|a| {
                    a.connection == *connection
                        && a.timestamp <= timestamp
                        && timestamp - a.timestamp <= ARRIVAL_WINDOW
                        && self.transition_at.is_none_or(|transition| a.timestamp >= transition)
                })
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        if join {
            self.arrivals.clear();
            self.zone_connection = Some(connection.clone());
            self.transition_at = None;
        }
        let earliest = arrivals.iter().map(|a| a.timestamp).min().unwrap_or(timestamp);
        let changed = join && {
            let local = local.unwrap();
            self.character != Some(local.character_id) || self.map != local.map_id
        };
        if changed {
            self.combat.flush_unidentified();
            self.checkpoint(earliest, "completed");
        }
        self.combat.observe(wire, connection, message_id, timestamp, local, party);
        if !changed {
            for arrival in &arrivals {
                self.replay_arrival(arrival, local, party);
            }
            if !arrivals.is_empty() {
                self.combat.set_paused(self.paused, timestamp);
            }
            if ok && op == Some(41) {
                self.transition_at = Some(timestamp);
            }
            return;
        }
        let local = local.unwrap();
        if self.character != Some(local.character_id) {
            self.outside_map = None;
            self.active = None;
        }
        self.character = Some(local.character_id);
        let previous_map = self.map.take();
        self.map = local.map_id.clone();
        if !is_dungeon(self.map.as_deref()) {
            self.active = None;
            self.outside_map = self.map.clone();
            return;
        }
        let map = self.map.clone().unwrap();
        let mut entrance = wire.get(66).and_then(|v| v.as_str()).map(str::to_owned);
        if entrance.as_deref().is_none_or(is_blank) || is_dungeon(entrance.as_deref()) {
            entrance = Some(self.outside_map.clone().unwrap_or_else(|| "unknown".into()));
        }
        let entrance = entrance.unwrap();
        if let Some(known) = self.runs.iter().find(|r| r.character == local.character_id && r.run.contains_floor(&map)) {
            self.active = Some(known.serial);
        } else if self.active.is_none()
            || !is_dungeon(previous_map.as_deref())
            || kind(Some(&map)) == Some("Expedition")
            || self.runs.iter().find(|r| Some(r.serial) == self.active).is_none_or(|r| r.entrance != entrance)
        {
            if self.runs.len() >= 100 {
                self.runs.remove(0);
            }
            let id = identity(&match &self.record_id {
                None => format!("{}:{map}", local.character_id),
                Some(record) => format!("{record}:{}:{map}", local.character_id),
            });
            let run = DungeonRun::new(kind(Some(&map)).unwrap(), &entrance, &entrance, "live", Some(id), Some(DateTimeOffset::from_unix_milliseconds(timestamp / 1000)));
            self.add_run(local.character_id, entrance, run);
        }
        let floor_name = self.floor_name(&map);
        let run = self.active_run().unwrap();
        let _ = run.enter_floor(&map, &map, &floor_name);
        let session = identity(&format!("{}:{map}:{timestamp}", run.id()));
        self.combat.begin_session(Some(earliest), Some(session));
        for arrival in &arrivals {
            self.replay_arrival(arrival, Some(local), party);
        }
        self.combat.set_paused(self.paused, timestamp);
        self.checkpoint(timestamp, if self.paused { "paused" } else { "running" });
    }

    fn replay_arrival(&mut self, arrival: &Arrival, local: Option<&LocalCharacterSnapshot>, party: Option<&PartyRosterSnapshot>) {
        self.combat.set_paused(arrival.paused, arrival.timestamp);
        if arrival.paused && arrival.message.kind == 4 && matches!(arrival.message.integer(252), Some(6 | 7)) {
            return;
        }
        self.combat.observe(&arrival.message, &arrival.connection, &arrival.identity, arrival.timestamp, local, party);
    }

    fn checkpoint(&mut self, timestamp: i64, status: &str) {
        if self.active.is_none() {
            return;
        }
        self.combat.advance_to(timestamp);
        let Some(mut snapshot) = self.combat.snapshot() else { return };
        let run = self.active_run().unwrap();
        let Ok(location) = run.location() else { return };
        snapshot.status = status.into();
        let floor = location.floor_id.clone();
        snapshot.location = Some(location);
        let _ = run.record(&floor, snapshot);
    }

    pub fn observe_own_cast(&mut self, wire: &WireMessage, timestamp: i64) {
        self.combat.observe_own_cast(wire, timestamp);
        if wire.kind == 2 && wire.integer(253) == Some(237) {
            self.transition_at = Some(timestamp);
        }
    }

    pub fn forget_identities(&mut self, timestamp: i64) {
        self.arrivals.clear();
        self.transition_at = None;
        if self.active.is_some() {
            self.combat.flush_unidentified();
            let at = self.bounded(timestamp);
            self.checkpoint(at, "completed");
        }
        self.active = None;
        self.map = None;
        self.combat.forget_identities(timestamp);
    }

    pub fn unidentified(&self) -> Vec<UnidentifiedPlayer> {
        self.combat.unidentified()
    }
    pub fn sightings(&self) -> Vec<IdentitySighting> {
        self.combat.sightings()
    }
    pub fn timeline(&self) -> Vec<IdentityTimelineEntry> {
        self.combat.timeline()
    }
    pub fn seed_skill_weapons(&mut self, skills: &[(i32, CsMap<i32, i32>)]) {
        self.combat.seed_skill_weapons(skills);
    }
    pub fn seed_characters(&mut self, characters: &[(Guid, KnownCharacter)]) {
        self.combat.seed_characters(characters);
    }

    pub fn flush_unidentified(&mut self) {
        self.combat.flush_unidentified();
        let at = self.bounded(self.latest_timestamp);
        self.checkpoint(at, if self.paused { "paused" } else { "running" });
    }

    pub fn begin_record(&mut self, timestamp: i64) {
        self.arrivals.clear();
        self.combat.flush_unidentified();
        let at = self.bounded(timestamp);
        self.checkpoint(at, "completed");
        self.paused = false;
        let map = self.map.clone().unwrap_or_default();
        if let Some(run) = self.active_run() {
            let session = identity(&format!("{}:{map}:{timestamp}", run.id()));
            self.combat.begin_session(Some(timestamp), Some(session));
        }
    }

    pub fn set_paused(&mut self, paused: bool, timestamp: i64) {
        let at = if paused { self.bounded(timestamp) } else { timestamp };
        self.paused = paused;
        self.combat.set_paused(paused, at);
        self.latest_timestamp = self.latest_timestamp.max(at);
        self.checkpoint(at, if paused { "paused" } else { "running" });
    }

    pub fn restore_party(&mut self, party: Option<&PartyRosterSnapshot>) {
        self.combat.set_party(party);
    }

    #[doc(hidden)]
    pub fn debug_sizes(&self) -> String {
        format!("runs={} arrivals={} combat: {}", self.runs.len(), self.arrivals.len(), self.combat.debug_sizes())
    }

    pub fn snapshot(&mut self) -> Vec<DungeonRunSnapshot> {
        self.snapshot_shared().into_iter().map(|run| (*run).clone()).collect()
    }

    /// The runs, each shared until it changes: a finished run is the same `Arc` every time, and the running one is
    /// rebuilt only when its record moved.
    pub fn snapshot_shared(&mut self) -> Vec<Arc<DungeonRunSnapshot>> {
        let at = self.bounded(self.latest_timestamp);
        self.checkpoint(at, if self.paused { "paused" } else { "running" });
        let status = if self.paused { "paused" } else { "running" };
        let active = self.active;
        self.runs.iter_mut().map(|r| r.run.snapshot_shared(if Some(r.serial) == active { status } else { "completed" })).collect()
    }
}
