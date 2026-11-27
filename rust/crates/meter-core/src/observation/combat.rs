//! Turns captured health updates into a combat record, naming the game objects as the party's characters
//! (`CapturedCombatObservation.cs`). A faithful port: same evidence rules, same order of operations, same counters.

use super::MessageId;
use super::local::LocalCharacterSnapshot;
use super::roster::{PartyRosterSnapshot, same_roster};
use crate::capture::photon::Connection;
use crate::capture::protocol18::{Element, Value, WireMessage};
use crate::catalog::{CombatSourceCatalog, ItemAttribution};
use crate::cs::{CsMap, CsSet, DateTimeOffset, Guid};
use crate::engine::{CombatEngine, CombatEvent, CombatMetric, CombatSnapshot, EventId, MAXIMUM_SECONDS, PartyMember, PlayerStats};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

/// `int?[]`, shared like the C# arrays so that reference comparisons keep their meaning.
pub type Gear = Arc<[Option<i32>]>;
type Key = (Connection, i64);
type InspectionKey = (Connection, Guid);

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownCharacter {
    pub name: String,
    pub equipment: Option<Gear>,
    pub skills: Option<Vec<i32>>,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnidentifiedPlayer {
    pub member_id: String,
    pub record_id: Option<String>,
    pub object_id: i64,
    pub equipment: Option<Gear>,
    pub candidates: Vec<Guid>,
    pub timestamp: i64,
    pub skills: Option<Vec<i32>>,
    pub visit: Option<String>,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentitySighting {
    pub character_id: Guid,
    pub object_id: i64,
    pub visit: String,
    pub timestamp: i64,
    pub method: String,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityTimelineEntry {
    pub timestamp: i64,
    pub visit: String,
    pub kind: String,
    pub character_id: Guid,
    pub name: Option<String>,
    pub object_id: Option<i64>,
    pub equipment: Option<Gear>,
}

/// A player the meter knows on the current map: which game object is which character.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownPlayer {
    #[serde(with = "arc_str")]
    pub connection: Connection,
    pub object_id: i64,
    pub id: Guid,
    pub name: String,
    pub weapon: Option<i32>,
    pub equipment: Option<Gear>,
}

mod arc_str {
    use std::sync::Arc;
    pub fn serialize<S: serde::Serializer>(value: &Arc<str>, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(value)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Arc<str>, D::Error> {
        Ok(<String as serde::Deserialize>::deserialize(deserializer)?.into())
    }
}

#[derive(Clone, Debug)]
struct Entity {
    id: Option<Guid>,
    name: Arc<str>,
    kind: &'static str,
    weapon: Option<i32>,
    revision: i64,
    equipment: Option<Gear>,
    spells: Option<Gear>,
    weapon_seen: i64,
    gear_seen: i64,
}

impl Entity {
    fn player(id: Guid, name: impl Into<Arc<str>>, previous: Option<&Entity>, revision: i64) -> Self {
        Entity {
            id: Some(id),
            name: name.into(),
            kind: "player",
            weapon: previous.and_then(|p| p.weapon),
            revision,
            equipment: previous.and_then(|p| p.equipment.clone()),
            spells: previous.and_then(|p| p.spells.clone()),
            weapon_seen: previous.map_or(0, |p| p.weapon_seen),
            gear_seen: previous.map_or(0, |p| p.gear_seen),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Pending {
    id: EventId,
    time: f64,
    delta: f64,
    spell: Option<i32>,
    kind: Option<i32>,
    target_kind: &'static str,
    at: i64,
}

/// An event counted without an exact source or weapon, kept so later equipment can prove it (`_unsourced`): the
/// counted event and its effect folded into 48 bytes. Its id is not kept: it is only ever moved within the engine that
/// counted it, named by `engine`.
#[derive(Clone, Copy, Debug)]
struct Unsourced {
    time: f64,
    delta: f64,
    at: i64,
    spell: i32,
    kind: i32,
    weapon: i32,
    source: i32,
    engine: u32,
    /// Bits: spell, kind, weapon, source present.
    present: u8,
    slot: u8,
    source_kind: u8,
    exact: bool,
}

fn pack(value: Option<i32>, bit: u8, present: &mut u8) -> i32 {
    if value.is_some() {
        *present |= bit;
    }
    value.unwrap_or(0)
}

fn unpack(value: i32, bit: u8, present: u8) -> Option<i32> {
    (present & bit != 0).then_some(value)
}

fn placeholder() -> EventId {
    static PLACEHOLDER: std::sync::OnceLock<EventId> = std::sync::OnceLock::new();
    PLACEHOLDER.get_or_init(|| EventId::Text("".into())).clone()
}

impl Unsourced {
    fn new(counted: &CombatEvent, effect: &Pending, engine: u64) -> Self {
        let mut present = 0;
        Unsourced {
            time: counted.time,
            delta: effect.delta,
            at: effect.at,
            spell: pack(effect.spell, 1, &mut present),
            kind: pack(effect.kind, 2, &mut present),
            weapon: pack(counted.weapon_index, 4, &mut present),
            source: pack(counted.source_item_index, 8, &mut present),
            engine: engine as u32,
            present,
            slot: crate::cs::intern_code(counted.source_slot),
            source_kind: crate::cs::intern_code(counted.source_kind),
            exact: counted.source_exact,
        }
    }
    fn effect(&self) -> Pending {
        Pending {
            id: placeholder(),
            time: self.time,
            delta: self.delta,
            spell: unpack(self.spell, 1, self.present),
            kind: unpack(self.kind, 2, self.present),
            target_kind: "unknown",
            at: self.at,
        }
    }
    fn counted(&self, actor: Arc<str>) -> CombatEvent {
        CombatEvent {
            id: placeholder(),
            actor_id: actor,
            time: self.time,
            metric: if self.delta < 0.0 { CombatMetric::Damage } else { CombatMetric::Healing },
            amount: self.delta.abs(),
            weapon_index: self.weapon(),
            weapon_name: None,
            source_item_index: self.source(),
            source_slot: crate::cs::interned(self.slot),
            source_kind: crate::cs::interned(self.source_kind),
            source_exact: self.exact,
        }
    }
    fn weapon(&self) -> Option<i32> {
        unpack(self.weapon, 4, self.present)
    }
    fn source(&self) -> Option<i32> {
        unpack(self.source, 8, self.present)
    }
    fn prove(&mut self, proven: &CombatEvent) {
        let mut present = self.present & 3;
        self.weapon = pack(proven.weapon_index, 4, &mut present);
        self.source = pack(proven.source_item_index, 8, &mut present);
        self.present = present;
        self.slot = crate::cs::intern_code(proven.source_slot);
        self.source_kind = crate::cs::intern_code(proven.source_kind);
        self.exact = proven.source_exact;
    }
}

struct Awaiting {
    serial: u64,
    actor: Entity,
    effect: Pending,
    key: Option<Key>,
    visit: i64,
}

type SettledState = (i64, i64, Option<i32>, Option<Gear>, i32);

fn same_state(a: &SettledState, b: &SettledState) -> bool {
    let spells = match (&a.3, &b.3) {
        (None, None) => true,
        (Some(x), Some(y)) => Arc::ptr_eq(x, y),
        _ => false,
    };
    a.0 == b.0 && a.1 == b.1 && a.2 == b.2 && spells && a.4 == b.4
}

const MAXIMUM_UNRESOLVED: i64 = 100_000;
const SKILL_SLOTS: [i32; 14] = [0, 0, 0, 3, 2, 4, -1, -1, -1, -1, -1, -1, 8, 9];
const COLORS: [&str; 6] = ["#9ebbf3", "#b69aff", "#67cbd7", "#eab978", "#6dd1ad", "#eb95b7"];

pub(crate) fn integer(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::integer)
}

fn number(value: Option<Element<'_, '_>>) -> Option<f64> {
    match value? {
        Element::Value(Value::Float(f)) if f.is_finite() => Some(f64::from(*f)),
        Element::Value(Value::Double(d)) if d.is_finite() => Some(*d),
        Element::Value(v) => v.integer().map(|i| i as f64),
        Element::Byte(b) => Some(f64::from(b)),
        Element::Other => None,
    }
}

fn weapon(value: Option<&Value>) -> Option<i32> {
    let value = value?;
    if value.array_len()? == 0 {
        return None;
    }
    value.element(0).integer().filter(|&i| (0..=i64::from(i32::MAX)).contains(&i)).map(|i| i as i32)
}

fn numbers(value: Option<&Value>) -> Gear {
    match value.and_then(|v| v.elements()) {
        Some(elements) => elements.take(32).map(|e| e.integer().filter(|&i| i32::try_from(i).is_ok()).map(|i| i as i32)).collect(),
        None => Arc::from([]),
    }
}

fn at(gear: &[Option<i32>], slot: usize) -> Option<i32> {
    gear.get(slot).copied().flatten()
}

fn positive(value: Option<i32>) -> Option<i32> {
    value.filter(|&v| v > 0)
}

fn slot_index(slot: Option<&str>) -> i32 {
    match slot {
        Some("mainhand") => 0,
        Some("offhand") => 1,
        Some("head") => 2,
        Some("armor") => 3,
        Some("shoes") => 4,
        Some("bag") => 5,
        Some("cape") => 6,
        Some("mount") => 7,
        Some("potion") => 8,
        Some("food") => 9,
        _ => -1,
    }
}

fn signature(skills: impl Iterator<Item = i32>) -> Vec<i32> {
    let mut values: Vec<i32> = skills.collect::<HashSet<_>>().into_iter().collect();
    values.sort_unstable();
    values.truncate(32);
    values
}

fn visit_name(visit: i64) -> String {
    format!("v{visit:x}")
}

fn color(hash: u32) -> String {
    COLORS[(hash % COLORS.len() as u32) as usize].into()
}

#[derive(Default)]
pub struct CapturedCombatObservation {
    sources: Option<Arc<CombatSourceCatalog>>,
    entities: CsMap<Key, Entity>,
    characters: CsMap<Guid, Entity>,
    pending_equipment: CsMap<Key, (i32, i64, Gear, Gear)>,
    diagnostics: CsMap<String, i64>,
    item_types: CsMap<Key, (i32, i64)>,
    item_spells: CsMap<Key, Gear>,
    item_durability: CsMap<Key, i64>,
    worn_items: CsSet<Key>,
    equipment_changes: CsMap<Key, i64>,
    equipment_reports: Vec<(i64, Connection, i64, Gear)>,
    item_moves: Vec<(i64, Connection, i64, Guid, i32)>,
    move_requests: Vec<(i64, Guid, Guid)>,
    equipment_container: Option<Guid>,
    container_items: CsSet<Key>,
    local_items: Vec<Option<i64>>,
    identities_since: i64,
    unresolved: CsMap<Key, Vec<Pending>>,
    unresolved_count: i64,
    own_casts: CsMap<i64, i64>,
    unresolved_gear: CsMap<Key, (Gear, Gear, i64)>,
    weapon_by_skill: CsMap<i32, CsMap<i32, i32>>,
    fingerprints: CsMap<Guid, Vec<Gear>>,
    party_objects: CsMap<Key, i64>,
    skills: CsMap<Guid, CsSet<i32>>,
    /// The zone visit, named `v{timestamp:x}` in sightings and timelines; kept as the timestamp so dedup allocates nothing.
    visit: i64,
    sightings: VecDeque<IdentitySighting>,
    sighted: HashSet<(Guid, i64, i64)>,
    leaving_at: Option<i64>,
    connection_seen: CsMap<Connection, i64>,
    kept: VecDeque<UnidentifiedPlayer>,
    roster_confirmed: bool,
    roster_history: VecDeque<(i64, CsSet<Guid>, Option<i64>)>,
    party_action_at: Option<i64>,
    first_list_joined: bool,
    left_party: Option<i64>,
    local_left_at: i64,
    left_party_settled: bool,
    joins_seen: CsSet<Guid>,
    member_evidence: CsMap<Guid, i64>,
    left_at: CsMap<Guid, i64>,
    unknown_time_party_objects: CsSet<Key>,
    awaiting_roster: Vec<Awaiting>,
    awaiting_serial: u64,
    last_proof: i64,
    timeline: VecDeque<IdentityTimelineEntry>,
    logged_gear: CsMap<Guid, Gear>,
    engine: Option<CombatEngine>,
    start: i64,
    paused_at: Option<i64>,
    paused_duration: i64,
    local_id: Option<Guid>,
    connection: Option<Connection>,
    party: Option<PartyRosterSnapshot>,
    roster: CsMap<Guid, String>,
    inspections: CsMap<(Connection, Guid), (Vec<i64>, i64)>,
    unsourced: CsMap<Guid, Vec<Unsourced>>,
    settled: CsMap<Guid, SettledState>,
    gear_history: CsMap<Guid, Vec<Gear>>,
    /// Each character's member id text, formatted once.
    member_keys: HashMap<Guid, Arc<str>>,
}

impl CapturedCombatObservation {
    pub fn new(sources: Option<Arc<CombatSourceCatalog>>) -> Self {
        Self { sources, ..Self::default() }
    }

    fn member_key(&mut self, id: Guid) -> Arc<str> {
        self.member_keys.entry(id).or_insert_with(|| id.to_string().into()).clone()
    }

    #[doc(hidden)]
    pub fn debug_sizes(&self) -> String {
        format!(
            "unsourced={} unresolved={} awaiting={} kept={} sightings={} timeline={} entities={} characters={} fingerprints={} gear_history={} | {}",
            self.unsourced.values().map(Vec::len).sum::<usize>(), self.unresolved_count, self.awaiting_roster.len(), self.kept.len(), self.sightings.len(), self.timeline.len(), self.entities.len(), self.characters.len(), self.fingerprints.len(), self.gear_history.values().map(Vec::len).sum::<usize>(),
            self.engine.as_ref().map_or(String::new(), |e| e.debug_sizes())
        )
    }

    pub fn known_players(&self) -> Vec<KnownPlayer> {
        self.entities
            .iter()
            .filter(|(_, e)| e.kind == "player" && e.id.is_some())
            .map(|(key, e)| KnownPlayer {
                connection: key.0.clone(),
                object_id: key.1,
                id: e.id.unwrap(),
                name: e.name.to_string(),
                weapon: e.weapon,
                equipment: e.equipment.clone(),
            })
            .collect()
    }

    /// The players known before the meter was closed, on the same map and game connection: their objects are named again
    /// at once instead of waiting for the next zone change. They come back as old observations (revision 0), so a zone
    /// change still replaces them with what the game announces.
    pub fn restore_players(&mut self, players: &[KnownPlayer]) {
        for player in players {
            let key = (player.connection.clone(), player.object_id);
            if self.entities.contains_key(&key) {
                continue;
            }
            let entity = Entity {
                id: Some(player.id),
                name: player.name.as_str().into(),
                kind: "player",
                weapon: player.weapon,
                revision: 0,
                equipment: player.equipment.clone(),
                spells: None,
                weapon_seen: 0,
                gear_seen: 0,
            };
            self.entities.insert(key, entity.clone());
            self.characters.try_add(player.id, entity);
        }
    }

    pub fn skill_weapons(&self) -> CsMap<i32, CsMap<i32, i32>> {
        self.weapon_by_skill.compacted()
    }

    pub fn sightings(&self) -> Vec<IdentitySighting> {
        self.sightings.iter().cloned().collect()
    }

    fn new_visit(&mut self, timestamp: i64) {
        self.visit = timestamp;
    }

    pub(crate) fn observe_connection(&mut self, connection: &Connection, timestamp: i64) {
        if !self.connection_seen.contains_key(connection) && self.connection_seen.len() < 256 {
            self.connection_seen.insert(connection.clone(), timestamp);
        }
    }

    fn zone_visit(&mut self, timestamp: i64, connection: &Connection, local: Option<&LocalCharacterSnapshot>) {
        let opened = self.connection_seen.get(connection).copied().unwrap_or(timestamp);
        let since = match self.leaving_at {
            Some(leaving) if (0..=60_000_000).contains(&(timestamp - leaving)) => opened.max(leaving),
            _ if (0..=5_000_000).contains(&(timestamp - opened)) => opened,
            _ => timestamp,
        };
        self.leaving_at = None;
        if self.awaiting_proof() {
            self.settle_awaiting(since, false);
        }
        let arrived: Vec<(Key, Entity)> =
            self.entities.iter().filter(|(k, e)| k.0 == *connection && e.revision >= since).map(|(k, e)| (k.clone(), e.clone())).collect();
        let arrived_party: Vec<(Key, i64)> =
            self.party_objects.iter().filter(|(k, v)| k.0 == *connection && **v >= since).map(|(k, v)| (k.clone(), *v)).collect();
        let arrived_unknown: Vec<Key> = arrived_party.iter().filter(|(k, _)| self.unknown_time_party_objects.contains(k)).map(|(k, _)| k.clone()).collect();
        let effects_since = if local.is_some_and(|l| l.connection == *connection && l.observed_at_microseconds == timestamp) { since } else { timestamp };
        let arrived_effects: Vec<(Key, Vec<Pending>)> = self
            .unresolved
            .iter()
            .filter(|(k, _)| k.0 == *connection)
            .map(|(k, effects)| (k.clone(), effects.iter().filter(|e| e.at >= effects_since).cloned().collect::<Vec<_>>()))
            .filter(|(_, effects)| !effects.is_empty())
            .collect();
        let arrived_gear: Vec<(Key, (Gear, Gear, i64))> =
            self.unresolved_gear.iter().filter(|(k, g)| k.0 == *connection && g.2 >= effects_since).map(|(k, g)| (k.clone(), g.clone())).collect();
        for (key, effects) in &arrived_effects {
            self.unresolved.get_mut(key).unwrap().retain(|e| e.at < effects_since);
            self.unresolved_count -= effects.len() as i64;
        }
        let previous = visit_name(self.visit);
        self.flush_unidentified();
        self.new_visit(since);
        self.entities.clear();
        self.party_objects.clear();
        self.unknown_time_party_objects.clear();
        for (key, at) in arrived_party {
            self.party_objects.insert(key, at);
        }
        for key in arrived_unknown {
            self.unknown_time_party_objects.insert(key);
        }
        for (key, effects) in arrived_effects {
            self.unresolved_count += effects.len() as i64;
            self.unresolved.insert(key, effects);
        }
        for (key, gear) in arrived_gear {
            self.unresolved_gear.insert(key, gear);
        }
        self.pending_equipment.retain(|k, v| !(k.0 != *connection || v.1 < since));
        for (key, entity) in arrived {
            self.entities.insert(key.clone(), entity.clone());
            if let Some(character) = entity.id {
                self.sightings
                    .retain(|s| !(s.visit == previous && s.character_id == character && s.object_id == key.1 && s.timestamp >= since));
                self.sight(character, key.1, entity.revision, "announced");
            }
        }
        self.diagnostics.remove("unresolvedSinceZoneChange");
        self.diagnostics.remove("attributedSinceZoneChange");
    }

    fn sight(&mut self, character: Guid, object_id: i64, timestamp: i64, method: &str) {
        if self.sighted.len() > 20_000 {
            self.sighted.clear();
        }
        if !self.sighted.insert((character, object_id, self.visit)) {
            return;
        }
        if self.sightings.len() >= 2000 {
            self.sightings.pop_front();
        }
        self.sightings.push_back(IdentitySighting { character_id: character, object_id, visit: visit_name(self.visit), timestamp, method: method.into() });
    }

    pub fn unidentified(&self) -> Vec<UnidentifiedPlayer> {
        self.kept.iter().cloned().collect()
    }

    pub fn characters(&self) -> CsMap<Guid, KnownCharacter> {
        self.characters
            .iter()
            .map(|(id, e)| {
                (*id, KnownCharacter { name: e.name.to_string(), equipment: e.equipment.clone(), skills: self.skills.get(id).map(|s| signature(s.iter().copied())) })
            })
            .collect()
    }

    pub fn seed_skill_weapons(&mut self, skills: &[(i32, CsMap<i32, i32>)]) {
        for (skill, weapons) in skills {
            if self.weapon_by_skill.len() < 8192 && !self.weapon_by_skill.contains_key(skill) {
                self.weapon_by_skill.insert(*skill, weapons.compacted());
            }
        }
    }

    fn awaiting_proof(&self) -> bool {
        !self.awaiting_roster.is_empty() && (!self.roster_confirmed || self.left_party.is_some() && !self.left_party_settled)
    }

    pub fn timeline(&self) -> Vec<IdentityTimelineEntry> {
        self.timeline.iter().cloned().collect()
    }

    fn log(&mut self, entry: IdentityTimelineEntry) {
        if self.timeline.len() >= 4000 {
            self.timeline.pop_front();
        }
        self.timeline.push_back(entry);
    }

    fn learn_weapon_skills(&mut self, gear: &[Option<i32>], spells: &[Option<i32>]) {
        for (index, &slot) in SKILL_SLOTS.iter().enumerate().take(spells.len()) {
            if slot < 0 || slot as usize >= gear.len() {
                continue;
            }
            let (Some(item), Some(skill)) = (positive(gear[slot as usize]), positive(spells[index])) else { continue };
            if self.weapon_by_skill.len() >= 8192 && !self.weapon_by_skill.contains_key(&skill) {
                continue;
            }
            let items = self.weapon_by_skill.get_or_insert_with(skill, CsMap::new);
            if items.len() < 32 || items.contains_key(&item) {
                let count = items.get(&item).copied().unwrap_or(0) + 1;
                items.insert(item, count);
            }
        }
    }

    fn owned_item_casting(&self, effect: i32) -> Option<i32> {
        let sources = self.sources.as_ref()?;
        let mut types: Vec<i32> = Vec::new();
        for (key, spells) in self.item_spells.iter() {
            if !(self.worn_items.contains(key) || self.local_items.contains(&Some(key.1))) || !self.item_types.contains_key(key) {
                continue;
            }
            if !spells.iter().any(|skill| positive(*skill).is_some_and(|skill| sources.produces(skill, effect))) {
                continue;
            }
            let kind = self.item_types.get(key).unwrap().0;
            if !types.contains(&kind) {
                types.push(kind);
                if types.len() == 2 {
                    break;
                }
            }
        }
        if let [only] = types[..] { Some(only) } else { None }
    }

    fn resolve_learned(&mut self, worn: Option<&Gear>) -> ItemAttribution {
        self.count(if worn.is_none() { "unknownSource:noEquipment" } else { "unknownSource:unproven" }, 1);
        ItemAttribution::unknown()
    }

    pub fn seed_characters(&mut self, characters: &[(Guid, KnownCharacter)]) {
        for (id, character) in characters {
            if let Some(gear) = character.equipment.as_ref().filter(|g| g.len() >= 7)
                && !self.fingerprints.contains_key(id)
                && self.fingerprints.len() < 4096
            {
                self.fingerprints.insert(*id, vec![gear.clone()]);
            }
            if let Some(skills) = character.skills.as_ref().filter(|s| !s.is_empty())
                && !self.skills.contains_key(id)
                && self.skills.len() < 4096
            {
                self.skills.insert(*id, skills.iter().copied().collect());
            }
        }
        for (id, character) in characters {
            if !self.characters.contains_key(id) && self.characters.len() < 4096 {
                self.characters.insert(
                    *id,
                    Entity {
                        id: Some(*id),
                        name: character.name.as_str().into(),
                        kind: "player",
                        weapon: character.equipment.as_ref().and_then(|g| g.first().copied().flatten()),
                        revision: 0,
                        equipment: character.equipment.clone(),
                        spells: None,
                        weapon_seen: 0,
                        gear_seen: 0,
                    },
                );
            }
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub fn clock_timestamp(&self) -> Option<i64> {
        let engine = self.engine.as_ref()?;
        Some(self.paused_at.unwrap_or(self.start + self.paused_duration + (engine.elapsed_seconds() * 1_000_000.0) as i64))
    }

    pub fn diagnostics(&self) -> CsMap<String, i64> {
        let mut copy = self.diagnostics.compacted();
        copy.insert("pendingUnidentified".into(), self.unresolved_count);
        copy
    }

    pub fn weapons(&self) -> CsMap<Guid, Option<i32>> {
        self.characters.iter().map(|(id, e)| (*id, self.fresh_weapon(e))).collect()
    }

    pub fn engine(&self) -> Option<&CombatEngine> {
        self.engine.as_ref()
    }

    fn adjust(&self, mut snapshot: CombatSnapshot) -> CombatSnapshot {
        snapshot.mode = "live".into();
        snapshot.status = if self.is_paused() { "paused" } else { "running" }.into();
        snapshot.started_at = DateTimeOffset::from_unix_milliseconds(self.start / 1000);
        snapshot
    }

    pub fn snapshot(&self) -> Option<CombatSnapshot> {
        self.engine.as_ref().map(|engine| self.adjust(engine.full_snapshot("running")))
    }

    /// The live record's changes since `since` (see [`CombatEngine::delta`]), with the same header as [`Self::snapshot`].
    pub fn delta(&self, since: i64) -> Option<crate::engine::CombatDelta> {
        let engine = self.engine.as_ref()?;
        let mut delta = engine.delta(since, "running");
        delta.snapshot = self.adjust(delta.snapshot);
        Some(delta)
    }

    pub fn forget_identities(&mut self, timestamp: i64) {
        self.flush_unidentified();
        self.new_visit(timestamp);
        self.entities.clear();
        self.party_objects.clear();
        self.unknown_time_party_objects.clear();
        self.pending_equipment.clear();
        self.item_types.clear();
        self.item_spells.clear();
        self.item_durability.clear();
        self.worn_items.clear();
        self.equipment_changes.clear();
        self.equipment_reports.clear();
        self.item_moves.clear();
        self.move_requests.clear();
        self.container_items.clear();
        self.inspections.clear();
        self.local_items.clear();
        self.unsourced.clear();
        self.settled.clear();
        self.gear_history.clear();
        self.logged_gear.clear();
        self.roster_confirmed = false;
        self.roster_history.clear();
        self.party_action_at = None;
        self.first_list_joined = false;
        self.left_party = None;
        self.left_party_settled = false;
        self.awaiting_roster.clear();
        self.joins_seen.clear();
        self.member_evidence.clear();
        self.left_at.clear();
        self.roster.clear();
        self.party = None;
        self.identities_since = timestamp;
        self.diagnostics.remove("unresolvedSinceZoneChange");
        self.diagnostics.remove("attributedSinceZoneChange");
    }

    /// Lists these players in the current session (with nothing counted yet) when they are not in it already.
    pub fn carry(&mut self, members: &[PlayerStats]) {
        let Some(engine) = self.engine.as_mut() else { return };
        let mut known: HashSet<String> = engine.active_members().map(|m| m.id.clone()).collect();
        for member in members {
            if known.insert(member.id.clone()) {
                let _ = engine.register_member(PartyMember::new(&member.id, &member.name, &member.weapon, &member.weapon_family, &member.color));
            }
        }
    }

    /// Starts counting again from zero in the same session (same id), keeping everyone who was in it listed.
    pub fn restart(&mut self, timestamp: i64) {
        let previous = self.engine.as_ref().map(|engine| (engine.id().to_owned(), engine.full_snapshot("running").members));
        self.begin_session(Some(timestamp), previous.as_ref().map(|p| p.0.clone()));
        if let Some((_, members)) = previous {
            self.carry(&members);
        }
    }

    pub fn begin_session(&mut self, timestamp: Option<i64>, record_id: Option<String>) {
        if self.engine.is_some() {
            self.flush_unidentified();
        }
        self.clear_unidentified();
        self.awaiting_roster.clear();
        self.unsourced.clear();
        self.settled.clear();
        self.engine = timestamp.map(|_| CombatEngine::empty(record_id));
        self.start = timestamp.unwrap_or(0);
        self.paused_at = None;
        self.paused_duration = 0;
        self.diagnostics.clear();
        self.synchronize();
    }

    pub fn advance_to(&mut self, timestamp: i64) {
        let Some(engine) = self.engine.as_mut() else { return };
        let effective = match self.paused_at {
            Some(paused) => timestamp.min(paused),
            None => timestamp,
        };
        let time = ((effective - self.start - self.paused_duration) as f64 / 1_000_000.0).clamp(engine.elapsed_seconds(), MAXIMUM_SECONDS);
        let _ = engine.advance(time);
    }

    pub fn set_paused(&mut self, paused: bool, timestamp: i64) {
        if paused == self.is_paused() {
            return;
        }
        if paused {
            self.advance_to(timestamp);
            let elapsed = self.engine.as_ref().map_or(0.0, |e| e.elapsed_seconds());
            self.paused_at = Some(timestamp.max(self.start + self.paused_duration + (elapsed * 1_000_000.0) as i64));
        } else {
            self.paused_duration += (timestamp - self.paused_at.unwrap()).max(0);
            self.paused_at = None;
        }
    }

    fn count(&mut self, name: &str, amount: i64) {
        if amount <= 0 {
            return;
        }
        match self.diagnostics.get_mut(name) {
            Some(value) => *value += amount,
            None => self.diagnostics.insert(name.to_owned(), amount),
        }
    }

    pub fn observe(&mut self, wire: &WireMessage, connection: &Connection, identity: &MessageId, timestamp: i64, local: Option<&LocalCharacterSnapshot>, party: Option<&PartyRosterSnapshot>) {
        let field = |key: u8| wire.get(key);
        let op = integer(field(253));
        let joined = wire.kind == 3 && wire.return_code == Some(0) && op == Some(2);
        self.advance_to(timestamp);
        self.observe_connection(connection, timestamp);
        if joined {
            self.zone_visit(timestamp, connection, local);
        }
        if joined
            && let Some(joined_object) = integer(field(0))
        {
            let key = (connection.clone(), joined_object);
            if self.equipment_changes.get(&key).is_some_and(|&announced| announced >= timestamp - 2_000_000) {
                self.equipment_changes.remove(&key);
                self.equipment_reports.retain(|r| !(r.1 == *connection && r.2 == joined_object && r.0 >= timestamp - 2_000_000));
            }
        }
        if joined && let Some(container) = field(51).and_then(Value::as_guid) {
            self.equipment_container = Some(container);
        }
        let code = integer(field(252));
        if wire.kind == 4
            && matches!(code, Some(234 | 235))
            && field(1).and_then(Value::as_guid).is_some_and(|leaver| Some(leaver) == self.local_id)
            && self.left_party.is_none()
            && self.roster_history.iter().all(|entry| entry.1.iter().all(|m| Some(*m) == self.local_id))
            && let Some(left) = integer(field(0))
        {
            self.left_party = Some(left);
            self.local_left_at = timestamp;
        }
        if !self.unresolved.is_empty() && timestamp - self.last_proof > 5_000_000 {
            self.last_proof = timestamp;
            self.possible_identities();
        }
        if let Some(local) = local
            && self.local_id != Some(local.character_id)
        {
            if self.local_id.is_some() {
                self.entities.clear();
                self.characters.clear();
                self.roster.clear();
                self.engine = None;
            }
            self.local_id = Some(local.character_id);
            // A character known from before (restored) is listed right away, without waiting for a zone change.
            if self.characters.contains_key(&local.character_id) {
                self.synchronize();
            }
        }
        // A character restored from the last session has no connection yet; that is not a move to another server.
        if let Some(local) = local
            && !local.connection.is_empty()
            && self.connection.as_ref() != Some(&local.connection)
        {
            self.connection = Some(local.connection.clone());
            self.entities.retain(|k, _| k.0 == local.connection);
        }
        self.pending_equipment.retain(|_, v| timestamp - v.1 <= 15_000_000);
        if let Some(local) = local
            && let Some(object_id) = local.object_id
            && local.location_state == "observed"
            && local.observed_at_microseconds >= self.identities_since
        {
            let previous = self.characters.get(&local.character_id).cloned();
            // The name is shared with the previous observation while it is unchanged, so this per-message refresh allocates nothing.
            let name = previous.as_ref().filter(|p| *p.name == *local.name).map_or_else(|| Arc::from(local.name.as_str()), |p| p.name.clone());
            let mut entity = Entity::player(local.character_id, name, previous.as_ref(), timestamp);
            let key = (local.connection.clone(), object_id);
            if let Some(pending) = self.pending_equipment.remove(&key)
                && timestamp >= pending.1
            {
                entity.weapon = Some(pending.0);
                entity.equipment = Some(pending.2);
                entity.spells = Some(pending.3);
                entity.weapon_seen = pending.1;
                entity.gear_seen = pending.1;
            }
            let mapped = self.entities.contains_key(&key) && self.characters.contains_key(&local.character_id);
            self.sight(local.character_id, object_id, timestamp, "join");
            if self.unresolved.contains_key(&key) {
                self.identify(key, entity, "observed");
            } else {
                self.entities.insert(key, entity.clone());
                self.characters.insert(local.character_id, entity.clone());
                if !mapped {
                    self.synchronize();
                }
                self.settle(local.character_id, &entity);
            }
        }
        self.set_party(party);
        if wire.kind == 3 && wire.return_code != Some(0) && matches!(op, Some(41 | 237)) {
            self.leaving_at = None;
        }
        if wire.kind == 3
            && wire.return_code == Some(0)
            && op == Some(143)
            && let Some(contents) = field(1).filter(|v| v.array_len().is_some())
        {
            for item in contents.elements().unwrap().filter_map(Element::integer).filter(|&item| item > 0).take(512) {
                if self.container_items.len() < 8192 {
                    self.container_items.insert((connection.clone(), item));
                    self.worn_items.remove(&(connection.clone(), item));
                }
            }
            if let Some(inspected) = field(0).and_then(Value::as_guid)
                && contents.array_len() == Some(10)
                && self.inspections.len() < 256
            {
                let items = contents.elements().unwrap().map(|e| e.integer().unwrap_or(0)).collect();
                self.inspections.insert((connection.clone(), inspected), (items, timestamp));
                self.complete_inspections(connection);
            }
        }
        if joined && let Some(slots) = field(52) {
            let stale: Vec<Key> = self.item_types.iter().filter(|(_, v)| v.1 < timestamp - 5_000_000).map(|(k, _)| k.clone()).collect();
            for key in stale {
                self.item_types.remove(&key);
                self.item_spells.remove(&key);
                self.item_durability.remove(&key);
                self.worn_items.remove(&key);
                self.container_items.remove(&key);
            }
            self.local_items = match slots {
                Value::Bytes(bytes) => bytes.iter().map(|&b| Some(i64::from(b))).collect(),
                other => match other.elements() {
                    Some(elements) => elements.take(16).map(Element::integer).collect(),
                    None => Vec::new(),
                },
            };
            self.apply_local_equipment(connection, timestamp, local);
        }
        if wire.kind == 4
            && code == Some(30)
            && let Some(item_object) = integer(field(0))
            && let Some(item_type) = integer(field(1)).filter(|&t| (0..=i64::from(i32::MAX)).contains(&t))
        {
            let key = (connection.clone(), item_object);
            if self.item_types.len() < 4096 || self.item_types.contains_key(&key) {
                self.item_types.insert(key.clone(), (item_type as i32, timestamp));
            }
            if let Some(spells) = field(9).filter(|v| v.array_len().is_some())
                && self.item_types.contains_key(&key)
            {
                self.item_spells.insert(key.clone(), numbers(Some(spells)));
            }
            if !self.inspections.is_empty() {
                self.complete_inspections(connection);
            }
            if let Some(durability) = integer(field(8))
                && self.item_types.contains_key(&key)
            {
                if self.item_durability.get(&key).is_some_and(|&before| durability < before)
                    && !self.container_items.contains(&key)
                    && self.worn_items.insert(key.clone())
                    && let Some(owner) = self.local_id
                    && let Some(wearer) = self.characters.get(&owner).cloned()
                {
                    self.settle(owner, &wearer);
                }
                self.item_durability.insert(key, durability);
            }
            if self.local_items.contains(&Some(item_object)) {
                self.apply_local_equipment(connection, timestamp, local);
            }
            return;
        }
        if wire.kind == 3 && op == Some(41) {
            if wire.return_code == Some(0) {
                self.leaving_at = Some(timestamp);
            }
            return;
        }
        if wire.kind != 4 {
            return;
        }
        if code == Some(26)
            && let Some(moved) = integer(field(0))
            && let Some(destination) = field(2).and_then(Value::as_guid)
        {
            let slot = integer(field(1)).unwrap_or(0) as i32;
            if self.item_moves.len() < 2048 {
                self.item_moves.push((timestamp, connection.clone(), moved, destination, slot));
            }
            if let Some(worn) = self.equipment_container
                && !self.local_items.is_empty()
            {
                for item in self.local_items.iter_mut() {
                    if *item == Some(moved) {
                        *item = Some(0);
                    }
                }
                if destination == worn && (0..16).contains(&slot) {
                    if slot as usize >= self.local_items.len() {
                        self.local_items.resize(slot as usize + 1, None);
                    }
                    self.local_items[slot as usize] = Some(moved);
                }
            }
            if let Some(mover) = self.local_id
                && let Some(entity) = self.characters.get(&mover).cloned()
            {
                self.settle(mover, &entity);
            }
            return;
        }
        if matches!(code, Some(29 | 123))
            && let Some(observed) = integer(field(0))
        {
            let player = code == Some(29);
            let id = if player { field(7).and_then(Value::as_guid) } else { None };
            let name = if player {
                field(1).and_then(Value::as_str).unwrap_or("Unknown player").to_owned()
            } else {
                format!("Mob {}", integer(field(1)).map(|i| i.to_string()).unwrap_or_default())
            };
            let entity = Entity {
                id,
                name: name.into(),
                kind: if player { "player" } else { "mob" },
                weapon: if player { weapon(field(40)) } else { None },
                revision: timestamp,
                equipment: Some(numbers(field(40))),
                spells: Some(numbers(field(43))),
                weapon_seen: timestamp,
                gear_seen: if player { timestamp } else { 0 },
            };
            let key = (connection.clone(), observed);
            if self.entities.get(&key).is_some_and(|old| old.revision > timestamp) {
                return;
            }
            if id.is_some() {
                self.identify(key, entity, "announced");
                return;
            }
            if self.entities.len() < 4096 || self.entities.contains_key(&key) {
                self.entities.insert(key.clone(), entity);
            }
            if let Some(discarded) = self.unresolved.remove(&key) {
                self.unresolved_count -= discarded.len() as i64;
            }
            self.synchronize();
            return;
        }
        if code == Some(244)
            && let Some(member) = field(0).and_then(Value::as_guid)
            && let Some(gear) = field(2).filter(|v| v.array_len().is_some())
        {
            if self.member_evidence.len() < 4096 && !self.roster_confirmed {
                self.member_evidence.try_add(member, timestamp);
            }
            self.reported_gear(connection, member, numbers(Some(gear)), timestamp);
            if self.awaiting_proof() {
                self.settle_awaiting(timestamp, false);
            }
            return;
        }
        if code == Some(18)
            && let (Some(caster), Some(echoed)) = (integer(field(0)), integer(field(5)))
        {
            self.observe_cast_echo(connection, caster, echoed, timestamp, local);
        }
        if code == Some(232)
            && let Some(joined) = field(1).and_then(Value::as_guid)
        {
            self.joins_seen.insert(joined);
        }
        if matches!(code, Some(234 | 235))
            && let Some(removed) = field(1).and_then(Value::as_guid)
            && !self.roster_confirmed
        {
            self.left_at.insert(removed, timestamp);
        }
        if matches!(code, Some(228 | 239)) {
            self.party_action_at = Some(timestamp);
        }
        if code == Some(243)
            && let Some(stated) = field(1).and_then(Value::as_guid)
            && self.member_evidence.len() < 4096
            && !self.roster_confirmed
        {
            self.member_evidence.try_add(stated, timestamp);
            if self.awaiting_proof() {
                self.settle_awaiting(timestamp, false);
            }
        }
        if code == Some(670)
            && let Some(party_object) = integer(field(0))
        {
            // Field 1 is not the object's owner: captured 670s pair each member's object with the other member's id. The local
            // player is named only by the Join, never by a guess from here.
            let key = (connection.clone(), party_object);
            if self.party_objects.len() < 1024 || self.party_objects.contains_key(&key) {
                let added = !self.party_objects.contains_key(&key);
                self.party_objects.insert(key.clone(), timestamp);
                if added && self.unresolved_gear.contains_key(&key) {
                    self.rematch(connection, timestamp);
                }
            }
            if !self.roster_confirmed && self.unknown_time_party_objects.len() < 1024 {
                self.unknown_time_party_objects.insert(key);
            }
            if self.awaiting_proof() {
                self.settle_awaiting(timestamp, false);
            }
            return;
        }
        if code == Some(44)
            && field(15).is_some()
            && let Some(looter) = integer(field(3))
            && let Some(character) = field(6).and_then(Value::as_guid)
        {
            self.name_by_kill((connection.clone(), looter), character, timestamp);
            return;
        }
        if code == Some(165)
            && let Some(killer) = integer(field(4))
            && let Some(name) = field(5).and_then(Value::as_str)
        {
            let mut named: Vec<Guid> = Vec::new();
            for id in self.roster.iter().filter(|(_, n)| n.as_str() == name).map(|(id, _)| *id).chain(self.characters.iter().filter(|(_, e)| &*e.name == name).map(|(id, _)| *id)) {
                if !named.contains(&id) {
                    named.push(id);
                }
            }
            if let [only] = named[..] {
                self.name_by_kill((connection.clone(), killer), only, timestamp);
            }
            return;
        }
        if code == Some(74)
            && let Some(speaker) = integer(field(4))
            && let Some(guid) = field(5).and_then(Value::as_guid)
        {
            let key = (connection.clone(), speaker);
            if self.entities.get(&key).is_some_and(|e| e.id == Some(guid)) {
                return;
            }
            let previous = self.characters.get(&guid).cloned();
            let name = self
                .roster
                .get(&guid)
                .cloned()
                .or_else(|| previous.as_ref().map(|p| p.name.to_string()))
                .or_else(|| field(0).and_then(Value::as_str).map(Into::into))
                .unwrap_or_else(|| "Unknown player".into());
            let mut entity = Entity::player(guid, name, previous.as_ref(), timestamp);
            entity.gear_seen = 0;
            self.identify(key, entity, "chat");
            self.count("identifiedByChat", 1);
            return;
        }
        if code == Some(90)
            && let Some(equipment_object) = integer(field(0))
            && let Some(worn_weapon) = weapon(field(2))
        {
            let key = (connection.clone(), equipment_object);
            if self.equipment_changes.len() < 4096 || self.equipment_changes.contains_key(&key) {
                self.equipment_changes.insert(key.clone(), timestamp);
            }
            if self.equipment_reports.len() < 1024 {
                self.equipment_reports.push((timestamp, connection.clone(), equipment_object, numbers(field(2))));
            }
            if self.equipment_container.is_none() && self.entities.get(&key).is_some_and(|e| e.id == self.local_id) {
                self.local_items.clear();
            }
            self.learn_weapon_skills(&numbers(field(2)), &numbers(field(7)));
            if let Some(entity) = self.entities.get(&key).cloned()
                && let Some(id) = entity.id
                && timestamp >= entity.revision
            {
                let entity = Entity {
                    weapon: Some(worn_weapon),
                    revision: timestamp,
                    equipment: Some(numbers(field(2))),
                    spells: if field(7).is_none() { entity.spells.clone() } else { Some(numbers(field(7))) },
                    weapon_seen: timestamp,
                    gear_seen: timestamp,
                    ..entity
                };
                self.gear_changed(id, timestamp);
                self.entities.insert(key, entity.clone());
                self.characters.insert(id, entity.clone());
                self.synchronize();
                self.settle(id, &entity);
            } else if let Some(matched) = self.fingerprint(&key, &numbers(field(2))) {
                let entity = Entity {
                    weapon: Some(worn_weapon),
                    revision: timestamp,
                    equipment: Some(numbers(field(2))),
                    spells: if field(7).is_none() { matched.spells.clone() } else { Some(numbers(field(7))) },
                    weapon_seen: timestamp,
                    gear_seen: timestamp,
                    ..matched
                };
                self.identify(key, entity, "equipmentProof");
                self.count("identifiedByEquipment", 1);
            } else {
                if self.pending_equipment.len() < 256 {
                    self.pending_equipment.insert(key.clone(), (worn_weapon, timestamp, numbers(field(2)), numbers(field(7))));
                }
                if self.unresolved_gear.len() < 1024 || self.unresolved_gear.contains_key(&key) {
                    self.unresolved_gear.insert(key, (numbers(field(2)), numbers(field(7)), timestamp));
                }
                self.count("unresolvedEquipment", 1);
            }
            return;
        }
        if !matches!(code, Some(6 | 7)) {
            return;
        }
        if self.is_paused() {
            self.count("pausedHealthUpdates", 1);
            return;
        }
        let single = code == Some(6);
        let count = if single { 1 } else { field(2).and_then(Value::array_len).unwrap_or(0) };
        for index in 0..count {
            let part = |key: u8| -> Option<Element<'_, '_>> {
                if single {
                    field(key).map(Element::Value)
                } else {
                    field(key).filter(|v| v.array_len().is_some_and(|len| index < len)).map(|v| v.element(index))
                }
            };
            let (Some(delta), Some(source)) = (number(part(2)).filter(|&d| d != 0.0), part(6).and_then(Element::integer)) else {
                self.count("incompleteHealthUpdate", 1);
                continue;
            };
            let target = integer(field(0));
            if self.engine.is_none() {
                self.engine = Some(CombatEngine::empty(None));
                self.start = timestamp;
                self.synchronize();
            }
            let time = ((timestamp - self.start - self.paused_duration) as f64 / 1_000_000.0).max(0.0);
            if time > MAXIMUM_SECONDS {
                self.count("sessionLimit", 1);
                continue;
            }
            let target_kind = target.and_then(|t| self.entities.get(&(connection.clone(), t))).map_or("unknown", |e| e.kind);
            let effect = Pending {
                id: identity.event(index as u32),
                time,
                delta,
                spell: part(7).and_then(Element::integer).map(|i| i as i32),
                kind: part(5).and_then(Element::integer).map(|i| i as i32),
                target_kind,
                at: timestamp,
            };
            let key = (connection.clone(), source);
            match self.entities.get(&key).filter(|actor| actor.id.is_some()).cloned() {
                Some(actor) => {
                    self.attribute(actor, effect, Some(key), false);
                }
                None => {
                    self.count("unresolvedActor", 1);
                    self.count("unresolvedSinceZoneChange", 1);
                    if self.unresolved_count < MAXIMUM_UNRESOLVED {
                        self.unresolved.get_or_insert_with(key, Vec::new).push(effect);
                        self.unresolved_count += 1;
                    } else {
                        self.count("unresolvedDropped", 1);
                    }
                }
            }
        }
    }

    fn fresh_gear(&self, actor: &Entity) -> Option<Gear> {
        if actor.gear_seen >= self.identities_since {
            return actor.equipment.clone();
        }
        let mut gear: Option<Vec<Option<i32>>> = self.worn_gear(actor).map(|g| g.to_vec());
        if actor.weapon_seen >= self.identities_since && positive(actor.weapon).is_some() {
            gear.get_or_insert_with(|| vec![None; 10])[0] = actor.weapon;
        }
        gear.map(Into::into)
    }

    fn fresh_weapon(&self, actor: &Entity) -> Option<i32> {
        if actor.weapon_seen >= self.identities_since { actor.weapon } else { self.worn_gear(actor).and_then(|g| g[0]) }
    }

    fn start_gear(&self) -> Option<(i64, Gear)> {
        let sources = self.sources.as_ref()?;
        let local = self.local_id?;
        let equipment = self.equipment_container?;
        let own: HashSet<&Key> = self.entities.iter().filter(|(_, e)| e.id == Some(local)).map(|(k, _)| k).collect();
        let mut changes: Vec<&(i64, Connection, i64, Gear)> = self
            .equipment_reports
            .iter()
            .filter(|r| r.0 >= self.identities_since && own.contains(&(r.1.clone(), r.2)))
            .collect();
        changes.sort_by_key(|r| r.0);
        let first = *changes.first()?;
        let together = |one: i64, other: i64| (one - other).abs() <= 100_000;
        let moves: Vec<&(i64, Connection, i64, Guid, i32)> = self.item_moves.iter().filter(|m| m.1 == first.1 && together(m.0, first.0)).collect();
        if moves.is_empty() {
            return None;
        }
        let mut gear: Vec<Option<i32>> = vec![None; 10];
        let entered: HashSet<i32> = moves.iter().filter(|m| m.3 == equipment).map(|m| m.4).collect();
        for (slot, item) in gear.iter_mut().enumerate().take(8) {
            if let Some(worn) = positive(at(&first.3, slot))
                && !entered.contains(&(slot as i32))
            {
                *item = Some(worn);
            }
        }
        for m in moves.iter().filter(|m| m.3 != equipment) {
            if !self.move_requests.iter().any(|r| r.1 == equipment && r.2 == m.3 && r.0 <= m.0 && m.0 - r.0 <= 3_000_000) {
                continue;
            }
            let mut kind = self.item_types.get(&(m.1.clone(), m.2)).map(|t| t.0);
            if kind.is_none() {
                kind = self
                    .item_moves
                    .iter()
                    .filter(|back| back.1 == m.1 && back.2 == m.2 && back.3 == equipment && back.0 > m.0)
                    .map(|back| changes.iter().find(|r| together(r.0, back.0)).and_then(|r| usize::try_from(back.4).ok().and_then(|s| at(&r.3, s))))
                    .find(|found| positive(*found).is_some())
                    .flatten();
            }
            let Some(kind) = positive(kind) else { continue };
            let slot = slot_index(sources.find_item(kind).map(|i| i.slot.as_str()));
            if slot < 0 || positive(at(&first.3, slot as usize)).is_some() {
                continue;
            }
            let slot = slot as usize;
            gear[slot] = if gear[slot].is_some_and(|other| other != kind) { None } else { Some(kind) };
        }
        gear.iter().any(|t| positive(*t).is_some()).then(|| (first.0, gear.into()))
    }

    fn worn_gear(&self, actor: &Entity) -> Option<Gear> {
        let sources = self.sources.as_ref()?;
        if actor.id.is_none() || actor.id != self.local_id || self.worn_items.is_empty() {
            return None;
        }
        if self
            .entities
            .iter()
            .any(|(k, e)| e.id == self.local_id && self.equipment_changes.get(k).is_some_and(|&changed| changed >= self.identities_since))
        {
            return None;
        }
        let mut gear: Vec<Option<i32>> = vec![None; 10];
        let mut conflicts = HashSet::new();
        for item in self.worn_items.iter() {
            let Some(kind) = self.item_types.get(item) else { continue };
            let slot = slot_index(sources.find_item(kind.0).map(|i| i.slot.as_str()));
            if slot < 0 {
                continue;
            }
            let slot = slot as usize;
            if gear[slot].is_some_and(|other| other != kind.0) {
                conflicts.insert(slot);
            }
            gear[slot] = Some(kind.0);
        }
        for slot in conflicts {
            gear[slot] = None;
        }
        gear.iter().any(Option::is_some).then(|| gear.into())
    }

    fn source_of_actor(&mut self, actor: &Entity, effect: &Pending) -> ItemAttribution {
        let gear = self.fresh_gear(actor);
        let spells = if actor.gear_seen >= self.identities_since { actor.spells.clone() } else { None };
        self.source_of(actor.id, gear.as_ref(), spells.as_ref(), effect)
    }

    fn source_of(&mut self, actor_id: Option<Guid>, gear: Option<&Gear>, spells: Option<&Gear>, effect: &Pending) -> ItemAttribution {
        let Some(sources) = self.sources.clone() else { return ItemAttribution::unknown() };
        let source = sources.resolve(effect.spell, effect.kind, effect.delta < 0.0, gear.map_or(&[], |g| g), spells.map_or(&[], |s| s));
        let Some(spell) = positive(effect.spell) else { return source };
        if source.index.is_some() {
            return source;
        }
        if actor_id.is_some()
            && actor_id == self.local_id
            && let Some(owned) = self.owned_item_casting(spell)
            && let Some(item) = sources.find_item(owned)
        {
            self.count("sourceFromOwnedItem", 1);
            return ItemAttribution::new(Some(owned), &item.slot, "ability");
        }
        if let Some(gear) = gear
            && let Some(character) = actor_id
            && !sources.could_produce(spell, gear)
            && let Some(history) = self.gear_history.get(&character)
        {
            let mut earlier: Vec<ItemAttribution> = Vec::new();
            for previous in history.iter().filter(|previous| previous[..] != gear[..]) {
                let found = sources.resolve(effect.spell, effect.kind, effect.delta < 0.0, previous, &[]);
                if found.index.is_some() && !earlier.iter().any(|e| e.index == found.index) {
                    earlier.push(found);
                    if earlier.len() == 2 {
                        break;
                    }
                }
            }
            if earlier.len() == 1 {
                self.count("sourceFromEarlierEquipment", 1);
                return earlier.pop().unwrap();
            }
        }
        if let Some(family) = sources.unique_family(spell) {
            let slot = slot_index(Some(family.slot));
            if slot >= 0
                && let Some(worn) = positive(gear.and_then(|g| at(g, slot as usize)))
                && let Some(family_index) = family.index
                && sources.same_family(worn, family_index)
            {
                self.count("sourceFromItemFamily", 1);
                return ItemAttribution { index: Some(worn), exact: true, ..family };
            }
            // The effect is exclusive to one weapon family, but this player's mainhand is known to be a different weapon. A player
            // wields a single weapon, so a weapon-exclusive effect credited to them comes from the weapon they actually hold — not
            // a tier-less phantom of a weapon they are not wielding (frost-staff artifacts share abilities the game data lists under
            // only one of them). Credit the worn weapon instead of inventing a base-tier source under a different name.
            if slot == 0
                && let Some(mainhand) = positive(gear.and_then(|g| at(g, 0)))
            {
                self.count("sourceFromWornWeaponOverFamily", 1);
                return ItemAttribution { index: Some(mainhand), slot: "mainhand", kind: family.kind, exact: true };
            }
            self.count("sourceFromItemFamilyTierUnknown", 1);
            return family;
        }
        self.resolve_learned(gear)
    }

    fn log_gear(&mut self, actor_id: Guid, actor: &Entity, object_id: Option<i64>) {
        let Some(gear) = actor.equipment.as_ref() else { return };
        if actor.gear_seen < self.identities_since || self.logged_gear.get(&actor_id).is_some_and(|logged| logged[..] == gear[..]) {
            return;
        }
        let object_id = object_id.unwrap_or_else(|| self.entities.iter().find(|(_, e)| e.id == Some(actor_id)).map_or(0, |(k, _)| k.1));
        self.logged_gear.insert(actor_id, gear.clone());
        self.log(IdentityTimelineEntry {
            timestamp: actor.gear_seen,
            visit: visit_name(self.visit),
            kind: "gear".into(),
            character_id: actor_id,
            name: Some(actor.name.to_string()),
            object_id: (object_id > 0).then_some(object_id),
            equipment: Some(gear.clone()),
        });
        let history = self.gear_history.get_or_insert_with(actor_id, Vec::new);
        if history.len() >= 16 {
            history.remove(0);
        }
        history.push(gear.clone());
    }

    fn settle(&mut self, actor_id: Guid, actor: &Entity) {
        self.log_gear(actor_id, actor, None);
        if self.unsourced.get(&actor_id).is_none_or(Vec::is_empty) || self.engine.is_none() {
            return;
        }
        let local = Some(actor_id) == self.local_id;
        let counter = if local {
            ((self.worn_items.len() as i32).wrapping_mul(4096).wrapping_add(self.item_spells.len() as i32))
                .wrapping_mul(4096)
                .wrapping_add(self.item_moves.len() as i32)
                .wrapping_add((self.equipment_reports.len() as i32).wrapping_mul(2048))
        } else {
            0
        };
        let state: SettledState = (actor.gear_seen, actor.weapon_seen, actor.weapon, actor.spells.clone(), counter);
        if self.settled.get(&actor_id).is_some_and(|settled| same_state(settled, &state)) {
            return;
        }
        self.settled.insert(actor_id, state);
        let changed_at = self.entities.iter().filter(|(_, e)| e.id == Some(actor_id)).map(|(k, _)| self.equipment_changes.get(k).copied().unwrap_or(0)).max().unwrap_or(0);
        let start = if local { self.start_gear() } else { None };
        let mut pending = std::mem::take(self.unsourced.get_mut(&actor_id).unwrap());
        let serial = self.engine.as_ref().map_or(0, |engine| engine.serial() as u32);
        let member = self.member_key(actor_id);
        for index in (0..pending.len()).rev() {
            let entry = pending[index];
            let effect = entry.effect();
            let current = entry.at >= changed_at;
            let gear = if current {
                self.fresh_gear(actor)
            } else {
                start.as_ref().filter(|(until, _)| entry.at < *until).map(|(_, before)| before.clone())
            };
            if !current && gear.is_none() {
                continue;
            }
            let source = if current { self.source_of_actor(actor, &effect) } else { self.source_of(Some(actor_id), gear.as_ref(), None, &effect) };
            let (counted_source, counted_slot, counted_kind) = (entry.source(), crate::cs::interned(entry.slot), crate::cs::interned(entry.source_kind));
            let better = source.index.is_some() && (counted_source.is_none() || !entry.exact && source.exact);
            let weapon = match entry.weapon() {
                Some(weapon) => Some(weapon),
                None if current => self.fresh_weapon(actor),
                None => positive(gear.as_ref().and_then(|g| g[0])),
            };
            let changed = weapon != entry.weapon()
                || better && (source.index != counted_source || source.slot != counted_slot || source.kind != counted_kind || source.exact != entry.exact);
            if changed {
                let counted = entry.counted(member.clone());
                let mut proven = counted.clone();
                proven.weapon_index = weapon;
                if better {
                    proven.source_item_index = source.index;
                    proven.source_slot = source.slot;
                    proven.source_kind = source.kind;
                    proven.source_exact = source.exact;
                }
                // C# checks the id is in the engine's set: it is exactly when the engine that counted it is current.
                if entry.engine == serial && self.engine.as_mut().unwrap().reattribute_counted(&counted, &proven) {
                    self.count("reattributedFromTimeline", 1);
                    pending[index].prove(&proven);
                }
            }
            let entry = &pending[index];
            if entry.source().is_some() && entry.exact && entry.weapon().is_some() {
                pending.remove(index);
            }
        }
        *self.unsourced.get_mut(&actor_id).unwrap() = pending;
    }

    fn settle_awaiting(&mut self, until: i64, final_: bool) {
        if self.awaiting_roster.is_empty() || self.engine.is_none() {
            return;
        }
        let judged: Vec<(u64, Entity, Pending, Option<Key>, i64)> = self
            .awaiting_roster
            .iter()
            .filter(|entry| entry.effect.at <= until)
            .map(|entry| (entry.serial, entry.actor.clone(), entry.effect.clone(), entry.key.clone(), entry.visit))
            .collect();
        let mut credited = 0;
        for (serial, actor, effect, key, visit) in judged {
            let latest = match self.characters.get(&actor.id.unwrap()) {
                Some(known) if known.gear_seen >= actor.gear_seen => Entity {
                    equipment: known.equipment.clone(),
                    spells: known.spells.clone(),
                    weapon: known.weapon,
                    weapon_seen: known.weapon_seen,
                    gear_seen: known.gear_seen,
                    ..actor
                },
                _ => actor,
            };
            let key = if visit == self.visit { key } else { None };
            if self.credit(latest, effect, key) {
                credited += 1;
                self.awaiting_roster.retain(|entry| entry.serial != serial);
            } else if final_ {
                self.count("outsideParty", 1);
                self.awaiting_roster.retain(|entry| entry.serial != serial);
            }
        }
        self.count("creditedAfterRoster", credited);
        self.synchronize();
    }

    fn roster_at(&self, timestamp: i64) -> Option<&CsSet<Guid>> {
        self.roster_history.iter().rev().find(|entry| entry.0 <= timestamp).map(|entry| &entry.1)
    }

    fn first_list(&self) -> Option<&CsSet<Guid>> {
        let first = self.roster_history.front()?;
        (!self.first_list_joined && first.1.iter().any(|m| Some(*m) != self.local_id)).then_some(&first.1)
    }

    fn left_party_list(&self) -> Option<CsSet<Guid>> {
        let left = self.left_party?;
        let entry = self.roster_history.iter().find(|entry| entry.2 == Some(left))?;
        Some(entry.1.iter().filter(|m| !self.joins_seen.contains(*m)).copied().collect())
    }

    fn proven_member(&self, member: Guid, key: Option<&Key>, timestamp: i64) -> bool {
        if Some(member) == self.local_id {
            return true;
        }
        if let Some(roster) = self.roster_at(timestamp) {
            return roster.contains(&member);
        }
        if self.joins_seen.contains(&member) || self.left_at.get(&member).is_some_and(|&left| left <= timestamp) {
            return false;
        }
        key.is_some_and(|listed| self.unknown_time_party_objects.contains(listed))
            || self.member_evidence.contains_key(&member)
            || self.left_at.contains_key(&member)
            || self.first_list().is_some_and(|list| list.contains(&member))
            || self.left_party_list().is_some_and(|list| list.contains(&member))
    }

    fn credit(&mut self, actor: Entity, effect: Pending, key: Option<Key>) -> bool {
        let actor_id = actor.id.unwrap();
        if self.engine.is_none() || !self.proven_member(actor_id, key.as_ref(), effect.at) {
            return false;
        }
        let id = self.member_key(actor_id);
        if !self.engine.as_ref().unwrap().knows(&id) {
            let name = self.roster.get(&actor_id).cloned().unwrap_or_else(|| actor.name.to_string());
            let _ = self.engine.as_mut().unwrap().register_member_since(
                PartyMember { id: id.to_string(), name, weapon: "Weapon not observed".into(), weapon_family: "unknown".into(), color: color(actor_id.dotnet_hash() as u32) },
                effect.time,
            );
        }
        self.engine.as_mut().unwrap().admit(&id, effect.time);
        self.attribute(actor, effect, key, true)
    }

    fn open_candidates(&self, effects: &[Pending]) -> Vec<Guid> {
        let present: HashSet<Option<Guid>> = self.entities.values().map(|e| e.id).collect();
        let mut members: Option<CsSet<Guid>> = None;
        let mut seen = HashSet::new();
        let mut moments: Vec<i64> = Vec::new();
        for effect in effects {
            if seen.insert(effect.at) {
                moments.push(effect.at);
                if moments.len() == 512 {
                    break;
                }
            }
        }
        for at in moments {
            let left = self.left_party_list();
            let empty = CsSet::new();
            let current = self.roster_at(at).or_else(|| self.first_list()).or(left.as_ref()).unwrap_or(&empty);
            match members.as_mut() {
                None => members = Some(current.clone()),
                Some(members) => members.retain(|m| current.contains(m)),
            }
        }
        members.map_or_else(Vec::new, |m| m.iter().filter(|m| Some(**m) != self.local_id && !present.contains(&Some(**m))).copied().collect())
    }

    fn possible_identities(&mut self) -> CsMap<Key, Vec<Guid>> {
        let mut result: CsMap<Key, Vec<Guid>> = CsMap::new();
        let Some(sources) = self.sources.clone() else { return result };
        if self.engine.is_none() {
            return result;
        }
        let objects: Vec<(Key, Vec<i32>)> = self
            .unresolved
            .iter()
            .filter(|(k, effects)| !effects.is_empty() && self.party_objects.contains_key(*k))
            .map(|(k, effects)| {
                let mut skills: Vec<i32> = Vec::new();
                for spell in effects.iter().filter_map(|e| positive(e.spell)) {
                    if !skills.contains(&spell) {
                        skills.push(spell);
                        if skills.len() == 64 {
                            break;
                        }
                    }
                }
                (k.clone(), skills)
            })
            .collect();
        for (key, skills) in objects {
            let candidates: Vec<Guid> = self
                .open_candidates(self.unresolved.get(&key).unwrap())
                .into_iter()
                .filter(|member| {
                    let Some(character) = self.characters.get(member) else { return true };
                    let Some(equipment) = character.equipment.as_ref().filter(|_| character.gear_seen >= self.identities_since) else { return true };
                    let history = self.gear_history.get(member);
                    skills.iter().all(|&skill| history.into_iter().flatten().chain(std::iter::once(equipment)).any(|gear| sources.could_produce(skill, gear)))
                })
                .collect();
            result.insert(key, candidates);
        }
        let mut changed = true;
        while changed {
            changed = false;
            let singles: Vec<(Key, Guid)> = result.iter().filter(|(_, m)| m.len() == 1).map(|(k, m)| (k.clone(), m[0])).collect();
            for (key, member) in singles {
                if self.entities.get(&key).is_some_and(|e| e.id.is_some()) {
                    continue;
                }
                let known = self.characters.get(&member).cloned();
                let mut entity = Entity::player(member, self.roster_name(member), known.as_ref(), 0);
                entity.weapon_seen = known.as_ref().map_or(0, |k| k.weapon_seen);
                self.identify(key.clone(), entity, "skillProof");
                self.count("identifiedBySkillProof", 1);
                result.remove(&key);
                let others: Vec<Key> = result.keys().cloned().collect();
                for other in others {
                    let list = result.get_mut(&other).unwrap();
                    if list.contains(&member) {
                        list.retain(|candidate| *candidate != member);
                        changed = true;
                    }
                }
            }
        }
        result
    }

    /// `_roster[member]`: C# throws when the member is no longer listed; the port names it as the record knows it.
    fn roster_name(&self, member: Guid) -> String {
        self.roster.get(&member).cloned().or_else(|| self.characters.get(&member).map(|c| c.name.to_string())).unwrap_or_else(|| "Unknown player".into())
    }

    fn candidate_gear(&self, candidates: &[Guid]) -> Vec<Option<Gear>> {
        candidates
            .iter()
            .map(|c| self.characters.get(c).filter(|ch| ch.gear_seen >= self.identities_since).and_then(|ch| ch.equipment.clone()))
            .collect()
    }

    fn common_gear(&mut self, candidates: &[Guid]) -> Option<Gear> {
        if candidates.is_empty() {
            return None;
        }
        let worn: Vec<Gear> = self.candidate_gear(candidates).into_iter().collect::<Option<_>>()?;
        let length = worn.iter().map(|g| g.len()).min().unwrap_or(0);
        let common: Vec<Option<i32>> =
            (0..length).map(|slot| if worn.iter().all(|g| g[slot] == worn[0][slot]) { worn[0][slot] } else { None }).collect();
        if common.iter().all(Option::is_none) {
            return None;
        }
        self.count("gearCommonToCandidates", 1);
        Some(common.into())
    }

    fn source_for_any(&mut self, candidates: &[Guid], effect: &Pending) -> Option<ItemAttribution> {
        let gear = self.candidate_gear(candidates);
        let sources = self.sources.clone()?;
        if gear.is_empty() || gear.iter().any(Option::is_none) {
            return None;
        }
        let found: Vec<ItemAttribution> = gear.iter().map(|worn| self.source_of(None, worn.as_ref(), None, effect)).collect();
        let first = &found[0];
        if found.iter().any(|s| {
            s.index.is_none() || s.slot != first.slot || s.kind != first.kind || !sources.same_family(s.index.unwrap(), first.index.unwrap())
        }) {
            return None;
        }
        self.count("sourceCommonToCandidates", 1);
        let first = found[0].clone();
        if found.iter().all(|s| s.index == first.index && s.exact) {
            Some(first)
        } else {
            Some(ItemAttribution { index: Some(sources.family_representative(first.index.unwrap())), exact: false, ..first })
        }
    }

    fn family_for_any(&self, candidates: &[Guid], slot: usize) -> Option<i32> {
        let sources = self.sources.as_ref()?;
        let worn: Vec<Option<i32>> = self.candidate_gear(candidates).iter().map(|g| g.as_ref().and_then(|g| at(g, slot))).collect();
        if worn.is_empty() || worn.iter().any(|item| positive(*item).is_none()) {
            return None;
        }
        let first = worn[0].unwrap();
        if worn.iter().any(|item| !sources.same_family(item.unwrap(), first)) {
            return None;
        }
        Some(first)
    }

    fn gear_changed(&mut self, actor_id: Guid, timestamp: i64) {
        if Some(actor_id) == self.local_id {
            return;
        }
        let at = (timestamp - self.start - self.paused_duration) as f64 / 1_000_000.0;
        if let Some(pending) = self.unsourced.get_mut(&actor_id) {
            pending.retain(|entry| entry.time >= at);
        }
    }

    fn attribute(&mut self, mut actor: Entity, effect: Pending, key: Option<Key>, proven_member: bool) -> bool {
        let actor_id = actor.id.unwrap();
        let local = Some(actor_id) == self.local_id;
        if !local && !proven_member {
            if self.proven_member(actor_id, key.as_ref(), effect.at) {
                return self.credit(actor, effect, key);
            }
            self.await_membership(actor, effect, key);
            return false;
        }
        self.settle(actor_id, &actor);
        self.log_gear(actor_id, &actor, key.as_ref().map(|k| k.1));
        let source_item = self.source_of_actor(&actor, &effect);
        if local
            && source_item.kind == "ability"
            && source_item.slot == "mainhand"
            && let Some(owned) = source_item.index
            && actor.weapon != Some(owned)
            && self.fresh_weapon(&actor).is_none()
            && let Some(own_key) = key.clone()
        {
            actor.weapon = Some(owned);
            actor.weapon_seen = effect.at;
            self.entities.insert(own_key, actor.clone());
            self.characters.insert(actor_id, actor.clone());
            self.synchronize();
            self.count("weaponFromOwnedItem", 1);
            self.settle(actor_id, &actor);
        }
        if let Some(skill) = positive(effect.spell)
            && (self.skills.len() < 4096 || self.skills.contains_key(&actor_id))
        {
            let known = self.skills.get_or_insert_with(actor_id, CsSet::new);
            if known.len() < 64 {
                known.insert(skill);
            }
        }
        let counted = CombatEvent {
            id: effect.id.clone(),
            actor_id: self.member_key(actor_id),
            time: effect.time,
            metric: if effect.delta < 0.0 { CombatMetric::Damage } else { CombatMetric::Healing },
            amount: effect.delta.abs(),
            weapon_index: self.fresh_weapon(&actor),
            weapon_name: None,
            source_item_index: source_item.index,
            source_slot: source_item.slot,
            source_kind: source_item.kind,
            source_exact: source_item.exact,
        };
        let Some(engine) = self.engine.as_mut() else { return false };
        if !engine.add(&counted) {
            return false;
        }
        if (counted.source_item_index.is_none() || !counted.source_exact || counted.weapon_index.is_none())
            && (self.unsourced.len() < 256 || self.unsourced.contains_key(&actor_id))
        {
            let entry = Unsourced::new(&counted, &effect, engine.serial());
            let pending = self.unsourced.get_or_insert_with(actor_id, Vec::new);
            if pending.len() < 8192 {
                pending.push(entry);
            }
        }
        let name = if effect.delta < 0.0 {
            match effect.target_kind {
                "player" => "damageTo:player",
                "mob" => "damageTo:mob",
                _ => "damageTo:unknown",
            }
        } else {
            "positiveHealthUpdates"
        };
        self.count(name, 1);
        self.count("attributedSinceZoneChange", 1);
        true
    }

    fn await_membership(&mut self, actor: Entity, effect: Pending, key: Option<Key>) {
        let open = !self.roster_confirmed || self.left_party.is_some() && !self.left_party_settled && self.roster_at(effect.at).is_none();
        if open && self.awaiting_roster.len() < 20_000 {
            self.awaiting_serial += 1;
            self.awaiting_roster.push(Awaiting { serial: self.awaiting_serial, actor, effect, key, visit: self.visit });
            self.count("awaitingRoster", 1);
        } else {
            self.count("outsideParty", 1);
        }
    }

    fn name_by_kill(&mut self, key: Key, character: Guid, timestamp: i64) {
        if let Some(existing) = self.entities.get(&key).and_then(|e| e.id) {
            if existing != character {
                self.count("identityConflict", 1);
            }
            return;
        }
        let previous = self.characters.get(&character).cloned();
        let name = self.roster.get(&character).cloned().or_else(|| previous.as_ref().map(|p| p.name.to_string())).unwrap_or_else(|| "Unknown player".into());
        self.identify(key, Entity::player(character, name, previous.as_ref(), timestamp), "kill");
        self.count("identifiedByKill", 1);
    }

    fn identify(&mut self, key: Key, mut entity: Entity, method: &str) {
        if entity.id.is_some()
            && let Some((gear, spells, _)) = self.unresolved_gear.remove(&key)
        {
            entity.weapon = positive(gear.first().copied().flatten()).or(entity.weapon);
            entity.spells = if spells.is_empty() { entity.spells } else { Some(spells) };
            entity.equipment = Some(gear);
            entity.revision = entity.revision.max(self.identities_since);
            entity.weapon_seen = entity.weapon_seen.max(self.identities_since);
            entity.gear_seen = entity.gear_seen.max(self.identities_since);
        }
        if entity.id.is_some()
            && let Some(pending) = self.pending_equipment.remove(&key)
        {
            entity.weapon = Some(pending.0);
            entity.equipment = Some(pending.2);
            entity.spells = Some(pending.3);
            entity.revision = entity.revision.max(pending.1);
            entity.weapon_seen = pending.1;
        }
        if let Some(sighted) = entity.id {
            self.sight(sighted, key.1, entity.revision, method);
        }
        if self.entities.len() < 4096 || self.entities.contains_key(&key) {
            self.entities.insert(key.clone(), entity.clone());
        }
        if let Some(guid) = entity.id
            && (self.characters.len() < 4096 || self.characters.contains_key(&guid))
        {
            self.characters.insert(guid, entity.clone());
        }
        self.synchronize();
        let Some(id) = entity.id else { return };
        if self.engine.is_none() {
            return;
        }
        let Some(effects) = self.unresolved.remove(&key) else { return };
        self.unresolved_count -= effects.len() as i64;
        let local = Some(id) == self.local_id;
        if !effects.is_empty() && local {
            let earliest = effects.iter().map(|e| e.time).fold(f64::INFINITY, f64::min);
            self.engine.as_mut().unwrap().extend_membership(&id.to_string(), earliest);
        }
        for effect in effects {
            let current = self.entities.get(&key).cloned().unwrap_or_else(|| entity.clone());
            let credited = if local { self.attribute(current, effect.clone(), Some(key.clone()), false) } else { self.credit(current, effect.clone(), Some(key.clone())) };
            if credited {
                self.count("creditedAfterIdentification", 1);
            } else if !local {
                let current = self.entities.get(&key).cloned().unwrap_or_else(|| entity.clone());
                self.await_membership(current, effect, Some(key.clone()));
            }
        }
    }

    pub fn observe_own_cast(&mut self, wire: &WireMessage, timestamp: i64) {
        if wire.kind != 2 {
            return;
        }
        let op = wire.integer(253);
        if matches!(op, Some(225 | 227)) {
            self.party_action_at = Some(timestamp);
        }
        if op == Some(237) {
            self.leaving_at = Some(timestamp);
        }
        if op == Some(30)
            && let (Some(from), Some(to)) = (wire.get(1).and_then(Value::as_guid), wire.get(4).and_then(Value::as_guid))
            && self.move_requests.len() < 1024
        {
            self.move_requests.push((timestamp, from, to));
        }
        let Some(cast) = wire.integer(0).filter(|_| op == Some(24)) else { return };
        self.own_casts.retain(|_, at| timestamp - *at <= 5_000_000);
        if self.own_casts.len() < 256 {
            self.own_casts.insert(cast, timestamp);
        }
    }

    fn observe_cast_echo(&mut self, connection: &Connection, caster: i64, cast: i64, timestamp: i64, local: Option<&LocalCharacterSnapshot>) {
        if self.own_casts.remove(&cast).is_none() {
            return;
        }
        let Some(local_id) = self.local_id else { return };
        let key = (connection.clone(), caster);
        if self.entities.get(&key).is_some_and(|e| e.id == Some(local_id)) {
            return;
        }
        if self.entities.get(&key).is_some_and(|e| e.id.is_some()) {
            self.count("identityConflict", 1);
            return;
        }
        let previous = self.characters.get(&local_id).cloned();
        let name = local
            .map(|l| l.name.clone())
            .or_else(|| self.roster.get(&local_id).cloned())
            .or_else(|| previous.as_ref().map(|p| p.name.to_string()))
            .unwrap_or_else(|| "You".into());
        self.identify(key, Entity::player(local_id, name, previous.as_ref(), timestamp), "ownCast");
        self.count("identifiedByOwnCast", 1);
    }

    fn reported_gear(&mut self, connection: &Connection, member: Guid, gear: Gear, timestamp: i64) {
        if gear.len() < 7 || self.fingerprints.len() >= 4096 && !self.fingerprints.contains_key(&member) {
            return;
        }
        let known = self.fingerprints.get_or_insert_with(member, Vec::new);
        if !known.iter().any(|f| f[..] == gear[..]) {
            known.insert(0, gear.clone());
            if known.len() > 8 {
                known.remove(8);
            }
        }
        let previous = self.characters.get(&member).cloned();
        if self.characters.len() < 4096 || previous.is_some() {
            let name = self.roster.get(&member).cloned().or_else(|| previous.as_ref().map(|p| p.name.to_string())).unwrap_or_else(|| "Unknown player".into());
            self.characters.insert(
                member,
                Entity {
                    id: Some(member),
                    name: name.into(),
                    kind: "player",
                    weapon: gear[0].or(previous.as_ref().and_then(|p| p.weapon)),
                    revision: timestamp,
                    equipment: Some(gear.clone()),
                    spells: previous.as_ref().and_then(|p| p.spells.clone()),
                    weapon_seen: timestamp,
                    gear_seen: timestamp,
                },
            );
        }
        let stale: Vec<(Key, Entity)> =
            self.entities.iter().filter(|(_, e)| e.id == Some(member) && e.gear_seen < timestamp).map(|(k, e)| (k.clone(), e.clone())).collect();
        for (key, entity) in stale {
            let same = entity.equipment.as_ref().is_some_and(|e| e[..] == gear[..]);
            let updated = Entity {
                weapon: gear[0].or(entity.weapon),
                equipment: Some(gear.clone()),
                spells: if same { entity.spells.clone() } else { None },
                weapon_seen: timestamp,
                gear_seen: timestamp,
                ..entity
            };
            self.characters.insert(member, updated.clone());
            self.entities.insert(key, updated);
        }
        self.rematch(connection, timestamp);
    }

    fn complete_inspections(&mut self, connection: &Connection) {
        let candidates: Vec<(InspectionKey, (Vec<i64>, i64))> =
            self.inspections.iter().filter(|(k, _)| k.0 == *connection).map(|(k, v)| (k.clone(), v.clone())).collect();
        for ((owner, character), (items, at)) in candidates {
            if items.iter().take(8).any(|&item| item > 0 && !self.item_types.contains_key(&(connection.clone(), item))) {
                continue;
            }
            self.inspections.remove(&(owner, character));
            let gear: Gear = items
                .iter()
                .map(|&item| if item == 0 { Some(0) } else { self.item_types.get(&(connection.clone(), item)).map(|t| t.0) })
                .collect();
            self.reported_gear(connection, character, gear, at);
            self.count("gearFromInspection", 1);
        }
    }

    fn fingerprint(&self, key: &Key, equipment: &[Option<i32>]) -> Option<Entity> {
        if !self.party_objects.contains_key(key) || equipment.iter().take(8).filter(|slot| slot.is_some()).count() < 7 {
            return None;
        }
        let candidates = self.complete_candidates(self.clock_timestamp().unwrap_or(self.identities_since))?;
        if candidates.is_empty() {
            return None;
        }
        let worn = self.candidate_gear(&candidates);
        if worn.iter().any(Option::is_none) {
            return None;
        }
        let same = |a: &[Option<i32>], b: &[Option<i32>]| {
            (0..8).all(|slot| match (at(a, slot), at(b, slot)) {
                (Some(first), Some(second)) => first == second,
                _ => true,
            }) && (0..8).filter(|&slot| at(a, slot).is_some() && at(b, slot).is_some()).count() >= 7
        };
        let matches: Vec<Guid> = candidates.iter().zip(&worn).filter(|(_, gear)| same(equipment, gear.as_ref().unwrap())).map(|(c, _)| *c).collect();
        if let [only] = matches[..] { self.characters.get(&only).cloned() } else { None }
    }

    fn complete_candidates(&self, timestamp: i64) -> Option<Vec<Guid>> {
        let left;
        let members = match self.roster_at(timestamp) {
            Some(roster) => roster,
            None if self.roster_history.is_empty() => return None,
            None => match self.first_list() {
                Some(first) => first,
                None => {
                    left = self.left_party_list()?;
                    &left
                }
            },
        };
        let present: HashSet<Option<Guid>> = self.entities.values().map(|e| e.id).collect();
        Some(members.iter().filter(|m| Some(**m) != self.local_id && !present.contains(&Some(**m))).copied().collect())
    }

    fn rematch(&mut self, connection: &Connection, timestamp: i64) {
        let _ = connection;
        let snapshot: Vec<(Key, Gear)> = self.unresolved_gear.iter().map(|(k, v)| (k.clone(), v.0.clone())).collect();
        for (key, gear) in snapshot {
            if self.entities.contains_key(&key) {
                continue;
            }
            let Some(matched) = self.fingerprint(&key, &gear) else { continue };
            let entity = Entity {
                revision: timestamp,
                weapon: gear.first().copied().flatten().or(matched.weapon),
                equipment: Some(gear),
                weapon_seen: timestamp,
                gear_seen: timestamp,
                ..matched
            };
            self.identify(key, entity, "equipmentProof");
            self.count("identifiedByEquipment", 1);
        }
    }

    pub fn flush_unidentified(&mut self) {
        if self.awaiting_proof() {
            self.settle_awaiting(i64::MAX, false);
        }
        let actors: Vec<Guid> = self.unsourced.keys().copied().collect();
        for actor_id in actors {
            if let Some(known) = self.characters.get(&actor_id).cloned() {
                self.settle(actor_id, &known);
            }
        }
        let open_objects: Vec<Key> =
            self.party_objects.keys().filter(|k| self.entities.get(*k).is_none_or(|e| e.id.is_none())).cloned().collect();
        let open_members = match &open_objects[..] {
            [only] => self.unresolved.get(only).map(|waiting| self.open_candidates(waiting)).unwrap_or_default(),
            _ => Vec::new(),
        };
        if self.engine.is_some() && open_objects.len() == 1 && open_members.len() == 1 {
            let member = open_members[0];
            let previous = self.characters.get(&member).cloned();
            let mut entity = Entity::player(member, self.roster_name(member), previous.as_ref(), 0);
            entity.revision = 0;
            self.identify(open_objects[0].clone(), entity, "elimination");
            self.count("identifiedByElimination", 1);
        }
        let possible = self.possible_identities();
        if self.engine.is_some() {
            let unresolved: Vec<(Key, Vec<Pending>)> = self.unresolved.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for (key, effects) in unresolved {
                let party_member = self.party_objects.contains_key(&key);
                if !party_member || effects.is_empty() {
                    if !effects.is_empty() {
                        self.count("droppedOutsidePartyObjects", 1);
                    }
                    continue;
                }
                let object_id = key.1;
                let id = format!("unidentified:{object_id}:{}", (effects[0].time * 1000.0) as i64);
                let member: Arc<str> = id.as_str().into();
                let candidates = match possible.get(&key) {
                    Some(consistent) => consistent.clone(),
                    None => self.open_candidates(&effects),
                };
                let observed = self.unresolved_gear.get(&key).cloned();
                let gear = match &observed {
                    Some(observed) => Some(observed.0.clone()),
                    None => self.common_gear(&candidates),
                };
                let worn_weapon = positive(gear.as_ref().and_then(|g| g.first().copied().flatten()));
                let family = if worn_weapon.is_none() { self.family_for_any(&candidates, 0) } else { None };
                let weapon_name = match (worn_weapon, family) {
                    (Some(weapon), _) => format!("Item #{weapon}"),
                    (None, Some(family)) => self.sources.as_ref().unwrap().family_name(Some(family)),
                    _ => "Weapon not observed".into(),
                };
                let engine = self.engine.as_mut().unwrap();
                let _ = engine.register_member_since(
                    PartyMember {
                        id: id.clone(),
                        name: format!("Unidentified player (obj {object_id})"),
                        weapon: weapon_name,
                        weapon_family: "unknown".into(),
                        color: color(object_id as u32),
                    },
                    effects[0].time,
                );
                let record_id = engine.id().to_owned();
                if self.kept.len() >= 1000 {
                    self.kept.pop_front();
                }
                self.kept.push_back(UnidentifiedPlayer {
                    member_id: id.clone(),
                    record_id: Some(record_id),
                    object_id,
                    equipment: gear.clone(),
                    candidates: candidates.clone(),
                    timestamp: self.start + (effects[0].time * 1_000_000.0) as i64,
                    skills: Some(signature(effects.iter().filter_map(|e| positive(e.spell)))),
                    visit: Some(visit_name(self.visit)),
                });
                let spells = observed.as_ref().map(|o| o.1.clone());
                for effect in &effects {
                    let mut source = if effect.spell == Some(-1) && matches!(effect.kind, Some(0 | 1)) {
                        ItemAttribution::new(worn_weapon, "mainhand", "autoAttack")
                    } else {
                        self.source_of(None, gear.as_ref(), spells.as_ref(), effect)
                    };
                    if source.index.is_none()
                        && let Some(shared) = self.source_for_any(&candidates, effect)
                    {
                        source = shared;
                    }
                    self.engine.as_mut().unwrap().add(&CombatEvent {
                        id: effect.id.clone(),
                        actor_id: member.clone(),
                        time: effect.time,
                        metric: if effect.delta < 0.0 { CombatMetric::Damage } else { CombatMetric::Healing },
                        amount: effect.delta.abs(),
                        weapon_index: worn_weapon,
                        weapon_name: None,
                        source_item_index: source.index,
                        source_slot: source.slot,
                        source_kind: source.kind,
                        source_exact: source.exact,
                    });
                }
                self.count("unidentifiedPlayersKept", 1);
            }
        }
        self.clear_unidentified();
    }

    fn clear_unidentified(&mut self) {
        self.unresolved.clear();
        self.unresolved_count = 0;
        self.unresolved_gear.clear();
    }

    fn apply_local_equipment(&mut self, connection: &Connection, timestamp: i64, local: Option<&LocalCharacterSnapshot>) {
        let Some(local) = local else { return };
        let Some(object_id) = local.object_id else { return };
        if local.location_state != "observed" || self.local_items.is_empty() {
            return;
        }
        let equipment: Vec<Option<i32>> = self
            .local_items
            .iter()
            .map(|item| match item {
                None | Some(0) => Some(0),
                Some(item) => self.item_types.get(&(connection.clone(), *item)).map(|t| t.0),
            })
            .collect();
        let previous = self.characters.get(&local.character_id).cloned();
        let fresh = previous.as_ref().filter(|p| p.gear_seen >= self.identities_since).and_then(|p| p.spells.clone());
        let mut spells: Vec<Option<i32>> = vec![None; SKILL_SLOTS.len()];
        for (index, &slot) in SKILL_SLOTS.iter().enumerate() {
            let order = SKILL_SLOTS[..index].iter().filter(|&&earlier| earlier == slot).count();
            let chosen = if slot >= 0 {
                self.local_items.get(slot as usize).copied().flatten().and_then(|item| self.item_spells.get(&(connection.clone(), item)))
            } else {
                None
            };
            spells[index] = match chosen {
                Some(chosen) => at(chosen, order),
                None => fresh.as_ref().and_then(|f| at(f, index)),
            };
        }
        let entity = Entity {
            id: Some(local.character_id),
            name: local.name.as_str().into(),
            kind: "player",
            weapon: equipment[0].or(previous.as_ref().and_then(|p| p.weapon)),
            revision: timestamp,
            equipment: Some(
                equipment
                    .iter()
                    .enumerate()
                    .map(|(slot, kind)| kind.or_else(|| previous.as_ref().and_then(|p| p.equipment.as_ref()).and_then(|e| at(e, slot))))
                    .collect(),
            ),
            spells: Some(spells.into()),
            weapon_seen: if equipment[0].is_none() { previous.as_ref().map_or(0, |p| p.weapon_seen) } else { timestamp },
            gear_seen: if equipment.iter().any(Option::is_some) { timestamp } else { previous.as_ref().map_or(0, |p| p.gear_seen) },
        };
        self.entities.insert((local.connection.clone(), object_id), entity.clone());
        self.characters.insert(local.character_id, entity.clone());
        self.synchronize();
        self.settle(local.character_id, &entity);
    }

    pub fn set_party(&mut self, party: Option<&PartyRosterSnapshot>) {
        if same_roster(party, self.party.as_ref()) {
            return;
        }
        self.party = party.cloned();
        let Some(party) = party.filter(|p| p.is_confirmed_since(Some(self.identities_since))) else {
            for member in party.iter().flat_map(|p| p.members.iter()) {
                if let Some(named) = self.characters.get_mut(&member.id)
                    && *named.name != *member.name
                {
                    named.name = member.name.as_str().into();
                }
            }
            return;
        };
        let at = party.observed_at_microseconds;
        let first = !self.roster_confirmed;
        self.roster_confirmed = true;
        if self.roster_history.len() >= 1000 {
            self.roster_history.pop_front();
        }
        self.roster_history.push_back((at, party.members.iter().map(|m| m.id).collect(), party.party_id));
        if first {
            self.first_list_joined = self.party_action_at.is_some_and(|action| at - action <= 10_000_000);
        }
        for member in party.members.iter() {
            if !self.roster.contains_key(&member.id) {
                self.log(IdentityTimelineEntry {
                    timestamp: at,
                    visit: visit_name(self.visit),
                    kind: "joined".into(),
                    character_id: member.id,
                    name: Some(member.name.clone()),
                    object_id: None,
                    equipment: None,
                });
            }
        }
        let left: Vec<(Guid, String)> =
            self.roster.iter().filter(|(id, _)| party.members.iter().all(|c| c.id != **id)).map(|(id, name)| (*id, name.clone())).collect();
        for (member, name) in left {
            self.log(IdentityTimelineEntry { timestamp: at, visit: visit_name(self.visit), kind: "left".into(), character_id: member, name: Some(name), object_id: None, equipment: None });
        }
        self.roster.clear();
        for member in party.members.iter() {
            self.roster.insert(member.id, member.name.clone());
            if let Some(known) = self.characters.get_mut(&member.id)
                && *known.name != *member.name
            {
                known.name = member.name.as_str().into();
            }
        }
        self.synchronize();
        if first {
            self.settle_awaiting(at, self.left_party.is_none());
        }
        if !self.left_party_settled
            && let Some(left) = self.left_party
            && party.party_id == Some(left)
        {
            self.left_party_settled = true;
            self.settle_awaiting(self.local_left_at, true);
        }
        if let Some(connection) = self.connection.clone() {
            self.rematch(&connection, party.observed_at_microseconds);
        }
    }

    fn synchronize(&mut self) {
        let Some(engine) = self.engine.as_mut() else { return };
        let mut active = self.roster.compacted();
        if let Some(local) = self.local_id
            && let Some(own) = self.characters.get(&local)
        {
            let name = self.roster.get(&local).cloned().unwrap_or_else(|| own.name.to_string());
            active.insert(local, name);
        }
        if self.roster_confirmed {
            let listed: Vec<String> = engine.active_members().map(|m| m.id.clone()).collect();
            for member in listed {
                if let Some(id) = Guid::parse(&member)
                    && !active.contains_key(&id)
                {
                    let _ = engine.set_membership(&member, false);
                }
            }
        }
        for (id, name) in active.iter() {
            let weapon = self.characters.get(id).filter(|c| c.weapon_seen >= self.identities_since).and_then(|c| c.weapon);
            let weapon = match weapon {
                None => "Weapon not observed".into(),
                Some(0) => "Unarmed".into(),
                Some(weapon) => format!("Item #{weapon}"),
            };
            let _ = engine.register_member(PartyMember { id: id.to_string(), name: name.clone(), weapon, weapon_family: "unknown".into(), color: color(id.dotnet_hash() as u32) });
        }
    }
}
