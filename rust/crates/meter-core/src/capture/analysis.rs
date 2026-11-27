//! The whole decode pipeline for one capture and its report (`CaptureAnalysis.cs`).

use super::network::{Flow, NetworkDecoder, is_game_port};
use super::photon::PhotonTransport;
use super::protocol18;
use crate::catalog::CombatSourceCatalog;
use crate::cs::{CsMap, DateTimeOffset, Guid};
use crate::dungeon::DungeonRunSnapshot;
use crate::engine::{CombatDelta, CombatSnapshot};
use crate::observation::MessageId;
use crate::observation::combat::{CapturedCombatObservation, IdentitySighting, IdentityTimelineEntry, KnownCharacter, KnownPlayer, UnidentifiedPlayer};
use crate::observation::dungeon::DungeonCombatObservation;
use crate::observation::local::{LocalCharacterObservation, LocalCharacterSnapshot};
use crate::observation::roster::{PartyRosterObservation, PartyRosterSnapshot};
use meter_capture::CaptureFrame;
use serde::Serialize;
use std::fmt;
use std::sync::Arc;

/// The identity side of a report: characters, unidentified players, sightings, skill weapons, timeline.
#[derive(Clone, Debug, Default)]
pub struct Identities {
    pub characters: CsMap<Guid, KnownCharacter>,
    pub unidentified: Vec<UnidentifiedPlayer>,
    pub sightings: Vec<IdentitySighting>,
    pub skill_weapons: CsMap<i32, CsMap<i32, i32>>,
    pub timeline: Vec<IdentityTimelineEntry>,
}

/// `CaptureReport`. The fields after `record_timestamp_microseconds` are `[JsonIgnore]` in C# and skipped here too.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureReport {
    pub frames: i64,
    pub bytes: i64,
    pub first_timestamp_microseconds: Option<i64>,
    pub last_timestamp_microseconds: Option<i64>,
    pub counters: CsMap<String, i64>,
    pub flows: CsMap<String, i64>,
    pub combat_decoded: bool,
    pub character: Option<LocalCharacterSnapshot>,
    pub party: Option<PartyRosterSnapshot>,
    pub combat: Option<CombatSnapshot>,
    pub combat_diagnostics: Option<CsMap<String, i64>>,
    pub weapons: Option<CsMap<Guid, Option<i32>>>,
    pub context_frames: i64,
    pub dungeons: Option<Vec<Arc<DungeonRunSnapshot>>>,
    pub record_paused: bool,
    pub record_timestamp_microseconds: Option<i64>,
    #[serde(skip)]
    pub characters: Option<CsMap<Guid, KnownCharacter>>,
    #[serde(skip)]
    pub unidentified: Option<Vec<UnidentifiedPlayer>>,
    #[serde(skip)]
    pub sightings: Option<Vec<IdentitySighting>>,
    #[serde(skip)]
    pub skill_weapons: Option<CsMap<i32, CsMap<i32, i32>>>,
    #[serde(skip)]
    pub timeline: Option<Vec<IdentityTimelineEntry>>,
    pub record_state: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordState {
    Idle,
    Running,
    Paused,
    Completed,
}

impl RecordState {
    pub fn as_str(self) -> &'static str {
        match self {
            RecordState::Idle => "idle",
            RecordState::Running => "running",
            RecordState::Paused => "paused",
            RecordState::Completed => "completed",
        }
    }
    fn active(self) -> bool {
        matches!(self, RecordState::Running | RecordState::Paused)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RecordError {
    AlreadyRecording,
    NothingToStop,
    NothingToReset,
    NotStarted,
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RecordError::AlreadyRecording => "Stop the current record before starting another one.",
            RecordError::NothingToStop => "There is no active record to stop.",
            RecordError::NothingToReset => "There is no active record to reset.",
            RecordError::NotStarted => "Press Play to start a record first.",
        })
    }
}

impl std::error::Error for RecordError {}

/// Counter names are kept structured until a report is built, so counting a message allocates nothing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Counter {
    Named(&'static str),
    MessageMagic(u8),
    MessageType(u8),
    WireCode(u8, u8),
    ApplicationCode(u8, i64),
}

impl fmt::Display for Counter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Counter::Named(name) => f.write_str(name),
            Counter::MessageMagic(magic) => write!(f, "messageMagic:{magic:02X}"),
            Counter::MessageType(kind) => write!(f, "messageType:{kind}"),
            Counter::WireCode(kind, code) => write!(f, "wireCode:{kind}:{code}"),
            Counter::ApplicationCode(kind, code) => write!(f, "applicationCode:{kind}:{code}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum FlowKey {
    Flow(Flow),
    Other,
}

fn increment(counters: &mut CsMap<Counter, i64>, mut name: Counter) {
    if !counters.contains_key(&name) && counters.len() >= 512 {
        name = Counter::Named("otherCounters");
    }
    match counters.get_mut(&name) {
        Some(value) => *value += 1,
        None => counters.insert(name, 1),
    }
}

fn now_us() -> i64 {
    DateTimeOffset::now().unix_milliseconds() * 1000
}

pub struct CaptureAnalysis {
    manual_recording: bool,
    character: LocalCharacterObservation,
    party: PartyRosterObservation,
    combat: CapturedCombatObservation,
    dungeons: DungeonCombatObservation,
    context_frames: i64,
    record_state: RecordState,
    restored_players: Option<Vec<KnownPlayer>>,
    network: NetworkDecoder,
    photon: PhotonTransport,
    /// Holds the arrays and parameter lists of the message being observed; reset before each one.
    arena: bumpalo::Bump,
    counters: CsMap<Counter, i64>,
    flows: CsMap<FlowKey, i64>,
    frames: i64,
    bytes: i64,
    first: Option<i64>,
    last: Option<i64>,
}

impl CaptureAnalysis {
    pub fn new(sources: Option<Arc<CombatSourceCatalog>>, manual_recording: bool) -> Self {
        Self {
            manual_recording,
            character: LocalCharacterObservation::new(),
            party: PartyRosterObservation::new(),
            combat: CapturedCombatObservation::new(sources.clone()),
            dungeons: DungeonCombatObservation::new(sources),
            context_frames: 0,
            record_state: if manual_recording { RecordState::Idle } else { RecordState::Running },
            restored_players: None,
            network: NetworkDecoder::new(),
            photon: PhotonTransport::new(),
            arena: bumpalo::Bump::new(),
            counters: CsMap::new(),
            flows: CsMap::new(),
            frames: 0,
            bytes: 0,
            first: None,
            last: None,
        }
    }

    pub fn record_state(&self) -> RecordState {
        self.record_state
    }

    pub fn start_record(&mut self, timestamp: i64) -> Result<(), RecordError> {
        if self.record_state.active() {
            return Err(RecordError::AlreadyRecording);
        }
        self.combat.begin_session(Some(timestamp), None);
        let id = self.combat.engine().unwrap().id().to_owned();
        self.dungeons.reset_recording(timestamp, &id);
        self.record_state = RecordState::Running;
        Ok(())
    }

    pub fn stop_record(&mut self, timestamp: i64) -> Result<(), RecordError> {
        if !self.record_state.active() {
            return Err(RecordError::NothingToStop);
        }
        self.combat.set_paused(true, timestamp);
        self.combat.flush_unidentified();
        self.dungeons.set_recording(false, timestamp);
        self.record_state = RecordState::Completed;
        Ok(())
    }

    /// Reset: damage and healing start again from zero in the same record, which keeps its id, its players and its
    /// pause state (and so its AlbionLab session); the dungeon runs start again from the floor the party is on.
    pub fn reset_counts(&mut self, timestamp: i64) -> Result<(), RecordError> {
        if !self.record_state.active() {
            return Err(RecordError::NothingToReset);
        }
        let paused = self.record_state == RecordState::Paused;
        self.combat.restart(timestamp);
        let id = self.combat.engine().unwrap().id().to_owned();
        self.dungeons.reset_counts(timestamp, &id);
        if paused {
            self.combat.set_paused(true, timestamp);
            self.dungeons.set_paused(true, timestamp);
        }
        Ok(())
    }

    /// `keep_identities`: the capture came back by itself after a short drop on the same game connection: the players,
    /// their objects and the dungeon floor are still the same, so they are kept instead of learned again.
    pub fn begin_capture(&mut self, timestamp: Option<i64>, preserve_record: bool, keep_identities: bool) {
        self.context_frames = self.frames;
        self.frames = 0;
        self.bytes = 0;
        self.first = None;
        self.last = None;
        self.counters.clear();
        self.flows.clear();
        let since = timestamp.unwrap_or_else(now_us);
        if !keep_identities {
            self.combat.forget_identities(since);
            self.dungeons.forget_identities(since);
            if let Some(restored) = &self.restored_players {
                self.combat.restore_players(restored);
                self.dungeons.restore_players(restored);
            }
        }
        self.restored_players = None;
        if !preserve_record {
            let paused = self.combat.is_paused();
            let at = timestamp.unwrap_or_else(now_us);
            self.combat.begin_session(Some(at), None);
            if paused {
                self.combat.set_paused(true, at);
            }
        }
        if self.manual_recording && matches!(self.record_state, RecordState::Idle | RecordState::Completed) {
            self.combat.set_paused(true, since);
            self.dungeons.set_recording(false, since);
        }
    }

    pub fn begin_record(&mut self, timestamp: Option<i64>) {
        let at = timestamp.unwrap_or_else(now_us);
        self.combat.begin_session(Some(at), None);
        self.dungeons.begin_record(at);
    }

    pub fn seed_skill_weapons(&mut self, skills: &[(i32, CsMap<i32, i32>)]) {
        self.combat.seed_skill_weapons(skills);
        self.dungeons.seed_skill_weapons(skills);
    }

    pub fn seed_characters(&mut self, characters: &[(Guid, KnownCharacter)]) {
        self.combat.seed_characters(characters);
        self.dungeons.seed_characters(characters);
    }

    pub fn seal_record(&mut self) {
        self.combat.flush_unidentified();
        self.dungeons.flush_unidentified();
    }

    pub fn set_paused(&mut self, paused: bool, timestamp: i64) -> Result<(), RecordError> {
        if self.manual_recording && !self.record_state.active() {
            return Err(RecordError::NotStarted);
        }
        self.combat.set_paused(paused, timestamp);
        self.dungeons.set_paused(paused, timestamp);
        self.record_state = if paused { RecordState::Paused } else { RecordState::Running };
        Ok(())
    }

    /// Moves the record's clock to the given time without a packet: live, the encounter time runs from Play even while the
    /// game sends nothing (login screen, loading, standing still), and a paused record stays frozen.
    pub fn advance_clock(&mut self, timestamp: i64) {
        if self.record_state == RecordState::Running {
            self.combat.advance_to(timestamp);
        }
    }

    pub fn restore_character(&mut self, character: LocalCharacterSnapshot) {
        self.character.restore(character);
    }

    pub fn known_players(&self) -> Vec<KnownPlayer> {
        self.combat.known_players()
    }

    /// Players restored at startup survive the first capture start, which otherwise forgets every identity.
    pub fn restore_players(&mut self, players: Vec<KnownPlayer>) {
        if players.is_empty() {
            return;
        }
        self.combat.restore_players(&players);
        self.dungeons.restore_players(&players);
        self.restored_players = Some(players);
    }

    pub fn restore_party(&mut self, value: PartyRosterSnapshot) -> bool {
        if !self.party.restore(value) {
            return false;
        }
        self.combat.set_party(self.party.snapshot());
        self.dungeons.restore_party(self.party.snapshot());
        true
    }

    pub fn character(&self) -> Option<&LocalCharacterSnapshot> {
        self.character.snapshot()
    }

    pub fn record_paused(&self) -> bool {
        self.combat.is_paused()
    }

    /// The live record's changes since a sequence, for a UI that keeps its own copy (see [`crate::engine`]).
    pub fn combat_delta(&self, since: i64) -> Option<CombatDelta> {
        self.combat.delta(since)
    }

    pub fn add(&mut self, frame: &CaptureFrame) {
        let Self { character, party, combat, dungeons, network, photon, arena, counters, flows, frames, bytes, first, last, .. } = self;
        *frames += 1;
        *bytes += frame.data.len() as i64;
        if last.is_some_and(|last| frame.timestamp_us < last) {
            increment(counters, Counter::Named("backwardTimestamps"));
        }
        *first = Some(first.map_or(frame.timestamp_us, |f| f.min(frame.timestamp_us)));
        *last = Some(last.map_or(frame.timestamp_us, |l| l.max(frame.timestamp_us)));
        let result = network.decode(frame);
        increment(counters, Counter::Named(result.status));
        let Some(datagram) = result.datagram else { return };
        let flow = if flows.contains_key(&FlowKey::Flow(datagram.flow())) || flows.len() < 64 { FlowKey::Flow(datagram.flow()) } else { FlowKey::Other };
        match flows.get_mut(&flow) {
            Some(count) => *count += 1,
            None => flows.insert(flow, 1),
        }
        let status = photon.decode(&datagram);
        increment(counters, Counter::Named(status));
        let inbound = is_game_port(datagram.source_port) && !is_game_port(datagram.destination_port);
        let outbound = is_game_port(datagram.destination_port) && !is_game_port(datagram.source_port);
        for message in photon.messages() {
            increment(counters, Counter::Named("transportMessages"));
            let payload = photon.payload(message);
            if payload.len() < 2 {
                increment(counters, Counter::Named("shortMessages"));
                continue;
            }
            increment(counters, Counter::MessageMagic(payload[0]));
            if payload[1] & 0x80 != 0 {
                increment(counters, Counter::Named("encryptedMessages"));
            } else {
                increment(counters, Counter::MessageType(payload[1] & 0x7f));
            }
            arena.reset();
            let decoded = protocol18::decode_in(payload, arena);
            increment(counters, Counter::Named(decoded.status));
            let Some(wire) = decoded.message else { continue };
            if inbound {
                let connection = &message.connection;
                let identity = MessageId::Command(message.identity());
                character.observe(&wire, connection, message.timestamp_us);
                party.observe(&wire, character.snapshot(), message.timestamp_us);
                dungeons.observe(&wire, connection, &identity, message.timestamp_us, character.snapshot(), party.snapshot());
                combat.observe(&wire, connection, &identity, message.timestamp_us, character.snapshot(), party.snapshot());
            } else if outbound {
                combat.observe_own_cast(&wire, message.timestamp_us);
                dungeons.observe_own_cast(&wire, message.timestamp_us);
            }
            increment(counters, Counter::WireCode(wire.kind, wire.code));
            let key = if wire.kind == 4 { 252 } else { 253 };
            if let Some(code) = wire.integer(key).filter(|code| (0..=65_535).contains(code)) {
                increment(counters, Counter::ApplicationCode(wire.kind, code));
            }
        }
    }

    #[doc(hidden)]
    pub fn debug_sizes(&self) -> String {
        format!("record: {}\ndungeon: {}", self.combat.debug_sizes(), self.dungeons.debug_sizes())
    }

    pub fn frames(&self) -> i64 {
        self.frames
    }
    pub fn bytes(&self) -> i64 {
        self.bytes
    }
    pub fn first_timestamp(&self) -> Option<i64> {
        self.first
    }
    pub fn last_timestamp(&self) -> Option<i64> {
        self.last
    }
    pub fn party(&self) -> Option<&PartyRosterSnapshot> {
        self.party.snapshot()
    }
    /// The report's `counters`.
    pub fn counters(&self) -> CsMap<String, i64> {
        let mut counters: CsMap<String, i64> = self.counters.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        for (name, value) in [
            ("duplicateCommands", self.photon.duplicates),
            ("pendingIpFragments", self.network.pending_fragments() as i64),
            ("expiredIpFragments", self.network.expired_fragments),
            ("evictedIpFragments", self.network.evicted_fragments),
            ("pendingPhotonAssemblies", self.photon.pending_assemblies() as i64),
            ("expiredPhotonAssemblies", self.photon.expired_assemblies),
            ("evictedPhotonAssemblies", self.photon.evicted_assemblies),
        ] {
            counters.insert(name.into(), value);
        }
        counters
    }
    /// The report's `combatDiagnostics`.
    pub fn combat_diagnostics(&self) -> CsMap<String, i64> {
        self.combat.diagnostics()
    }
    /// The report's `weapons`.
    pub fn weapons(&self) -> CsMap<Guid, Option<i32>> {
        self.combat.weapons()
    }
    pub fn record_timestamp(&self) -> Option<i64> {
        self.combat.clock_timestamp()
    }
    /// The report's `recordState` (manual recording only).
    pub fn reported_record_state(&self) -> Option<&'static str> {
        self.manual_recording.then(|| self.record_state.as_str())
    }
    /// The live record, full (the report's `combat`).
    pub fn combat(&self) -> Option<CombatSnapshot> {
        self.combat.snapshot()
    }
    /// The report's `dungeons`, each run shared until it changes.
    pub fn dungeons(&mut self) -> Vec<Arc<DungeonRunSnapshot>> {
        self.dungeons.snapshot_shared()
    }
    /// What the meter learned about who is who (the report's `[JsonIgnore]` fields), for the identity store.
    pub fn identities(&self) -> Identities {
        let mut unidentified = self.combat.unidentified();
        unidentified.extend(self.dungeons.unidentified());
        let mut sightings = self.combat.sightings();
        sightings.extend(self.dungeons.sightings());
        Identities {
            characters: self.combat.characters(),
            unidentified,
            sightings,
            skill_weapons: self.combat.skill_weapons(),
            timeline: self.combat.timeline(),
        }
    }

    /// The whole C# report. It costs the size of the record; a live loop reads the accessors above instead.
    pub fn snapshot(&mut self) -> CaptureReport {
        let flows = self
            .flows
            .iter()
            .map(|(k, v)| {
                (
                    match k {
                        FlowKey::Flow(flow) => flow.to_string(),
                        FlowKey::Other => "other".into(),
                    },
                    *v,
                )
            })
            .collect();
        let combat = self.combat();
        let identities = self.identities();
        CaptureReport {
            frames: self.frames,
            bytes: self.bytes,
            first_timestamp_microseconds: self.first,
            last_timestamp_microseconds: self.last,
            counters: self.counters(),
            flows,
            combat_decoded: combat.is_some(),
            character: self.character.snapshot().cloned(),
            party: self.party.snapshot().cloned(),
            combat,
            combat_diagnostics: Some(self.combat_diagnostics()),
            weapons: Some(self.weapons()),
            context_frames: self.context_frames,
            dungeons: Some(self.dungeons()),
            record_paused: self.combat.is_paused(),
            record_timestamp_microseconds: self.record_timestamp(),
            characters: Some(identities.characters),
            unidentified: Some(identities.unidentified),
            sightings: Some(identities.sightings),
            skill_weapons: Some(identities.skill_weapons),
            timeline: Some(identities.timeline),
            record_state: self.reported_record_state(),
        }
    }
}
