//! A random dungeon or expedition run split into floors (`DungeonRun.cs`).

use crate::cs::{CsMap, DateTimeOffset, Guid};
use crate::engine::{CombatSnapshot, ItemContribution, PlayerStats, TimelinePoint, Values, WeaponContribution};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DungeonLocation {
    pub run_id: String,
    pub name: String,
    pub entrance_map_id: String,
    pub entrance_map_name: String,
    pub floor_id: String,
    pub floor_number: i32,
    pub map_id: String,
    pub map_name: String,
}

#[derive(Clone, PartialEq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DungeonFloor {
    pub id: String,
    pub number: i32,
    pub map_id: String,
    pub map_name: String,
    pub encounters: Vec<CombatSnapshot>,
    pub totals: CombatSnapshot,
}

#[derive(Clone, PartialEq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DungeonRunSnapshot {
    pub id: String,
    pub started_at: DateTimeOffset,
    pub name: String,
    pub entrance_map_id: String,
    pub entrance_map_name: String,
    pub mode: String,
    pub status: String,
    pub floors: Vec<DungeonFloor>,
    pub totals: CombatSnapshot,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DungeonError {
    FloorLimit,
    NoFloor,
    NotFull,
    MixedSources,
    UnknownFloor,
    TwoFloors,
    EncounterLimit,
}

impl std::fmt::Display for DungeonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DungeonError::FloorLimit => "The dungeon floor limit was reached.",
            DungeonError::NoFloor => "Enter a floor before recording combat.",
            DungeonError::NotFull => "Dungeon checkpoints require a full snapshot.",
            DungeonError::MixedSources => "Encounter sources cannot be mixed.",
            DungeonError::UnknownFloor => "The floor has not been observed.",
            DungeonError::TwoFloors => "An encounter cannot belong to two floors.",
            DungeonError::EncounterLimit => "The dungeon encounter limit was reached.",
        })
    }
}

impl std::error::Error for DungeonError {}

struct Floor {
    id: String,
    number: i32,
    map_id: String,
    map_name: String,
    encounters: CsMap<String, CombatSnapshot>,
}

pub struct DungeonRun {
    name: String,
    entrance_map_id: String,
    entrance_map_name: String,
    mode: String,
    floors: Vec<Floor>,
    encounter_floors: CsMap<String, String>,
    current: Option<usize>,
    id: String,
    started_at: DateTimeOffset,
    /// Moves whenever a snapshot would change; [`DungeonRun::snapshot_shared`] reuses its last result until then.
    version: u64,
    cached: Option<(u64, String, Arc<DungeonRunSnapshot>)>,
    shifts: Shifts,
}

/// The timeline points of an encounter moved to its place in a total (`floor(offset + second)`), by scope (floor or
/// run) and encounter id, kept while the encounter and its offset stay the same.
type Shifts = HashMap<(bool, String), (i64, u64, Vec<Arc<TimelinePoint>>)>;

impl DungeonRun {
    pub fn new(name: &str, entrance_map_id: &str, entrance_map_name: &str, mode: &str, id: Option<String>, started_at: Option<DateTimeOffset>) -> Self {
        Self {
            name: name.into(),
            entrance_map_id: entrance_map_id.into(),
            entrance_map_name: entrance_map_name.into(),
            mode: mode.into(),
            floors: Vec::new(),
            encounter_floors: CsMap::new(),
            current: None,
            id: id.unwrap_or_else(|| Guid::new_v4().simple()),
            started_at: started_at.unwrap_or_else(DateTimeOffset::now),
            version: 0,
            cached: None,
            shifts: HashMap::new(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn contains_floor(&self, map_id: &str) -> bool {
        self.floors.iter().any(|floor| floor.map_id == map_id)
    }

    pub fn enter_floor(&mut self, cluster_id: &str, map_id: &str, map_name: &str) -> Result<DungeonLocation, DungeonError> {
        let index = match self.floors.iter().position(|floor| floor.id == cluster_id) {
            Some(index) => index,
            None => {
                if self.floors.len() >= 100 {
                    return Err(DungeonError::FloorLimit);
                }
                self.floors.push(Floor {
                    id: cluster_id.into(),
                    number: self.floors.len() as i32 + 1,
                    map_id: map_id.into(),
                    map_name: map_name.into(),
                    encounters: CsMap::new(),
                });
                self.version += 1;
                self.floors.len() - 1
            }
        };
        self.current = Some(index);
        self.location()
    }

    pub fn location(&self) -> Result<DungeonLocation, DungeonError> {
        let floor = &self.floors[self.current.ok_or(DungeonError::NoFloor)?];
        Ok(DungeonLocation {
            run_id: self.id.clone(),
            name: self.name.clone(),
            entrance_map_id: self.entrance_map_id.clone(),
            entrance_map_name: self.entrance_map_name.clone(),
            floor_id: floor.id.clone(),
            floor_number: floor.number,
            map_id: floor.map_id.clone(),
            map_name: floor.map_name.clone(),
        })
    }

    pub fn record(&mut self, cluster_id: &str, encounter: CombatSnapshot) -> Result<(), DungeonError> {
        if !encounter.full {
            return Err(DungeonError::NotFull);
        }
        if encounter.mode != self.mode {
            return Err(DungeonError::MixedSources);
        }
        let index = self.floors.iter().position(|floor| floor.id == cluster_id).ok_or(DungeonError::UnknownFloor)?;
        if self.encounter_floors.get(&encounter.id).is_some_and(|owner| owner != cluster_id) {
            return Err(DungeonError::TwoFloors);
        }
        if self.floors[index].encounters.get(&encounter.id).is_some_and(|previous| previous.sequence > encounter.sequence) {
            return Ok(());
        }
        // The same record at the same sequence and status is the same snapshot: storing it again changes nothing.
        if self.floors[index].encounters.get(&encounter.id).is_some_and(|previous| {
            previous.sequence == encounter.sequence && previous.status == encounter.status && previous.location == encounter.location && previous.started_at == encounter.started_at
        }) {
            return Ok(());
        }
        if self.encounter_floors.len() >= 1000 && !self.encounter_floors.contains_key(&encounter.id) {
            return Err(DungeonError::EncounterLimit);
        }
        self.encounter_floors.insert(encounter.id.clone(), cluster_id.into());
        self.floors[index].encounters.insert(encounter.id.clone(), encounter);
        self.version += 1;
        Ok(())
    }

    pub fn snapshot(&self, status: &str) -> DungeonRunSnapshot {
        self.build(status, &mut None)
    }

    /// The snapshot, shared until the run changes; totals reuse the timeline points of encounters that did not move,
    /// so a fight on the last floor costs that floor, not the whole run.
    pub fn snapshot_shared(&mut self, status: &str) -> Arc<DungeonRunSnapshot> {
        if let Some((version, cached_status, snapshot)) = &self.cached
            && *version == self.version
            && cached_status == status
        {
            return snapshot.clone();
        }
        let mut shifts = Some(std::mem::take(&mut self.shifts));
        let snapshot = Arc::new(self.build(status, &mut shifts));
        self.shifts = shifts.unwrap();
        self.cached = Some((self.version, status.to_owned(), snapshot.clone()));
        snapshot
    }

    fn build(&self, status: &str, shifts: &mut Option<Shifts>) -> DungeonRunSnapshot {
        let floors: Vec<DungeonFloor> = self
            .floors
            .iter()
            .map(|floor| {
                let encounters: Vec<CombatSnapshot> = floor.encounters.values().cloned().collect();
                let totals = self.aggregate(&encounters.iter().collect::<Vec<_>>(), format!("{}:{}", self.id, floor.id), status, shifts, false);
                DungeonFloor { id: floor.id.clone(), number: floor.number, map_id: floor.map_id.clone(), map_name: floor.map_name.clone(), encounters, totals }
            })
            .collect();
        let all: Vec<&CombatSnapshot> = floors.iter().flat_map(|floor| floor.encounters.iter()).collect();
        let totals = self.aggregate(&all, self.id.clone(), status, shifts, true);
        DungeonRunSnapshot {
            id: self.id.clone(),
            started_at: self.started_at,
            name: self.name.clone(),
            entrance_map_id: self.entrance_map_id.clone(),
            entrance_map_name: self.entrance_map_name.clone(),
            mode: self.mode.clone(),
            status: status.into(),
            floors,
            totals,
        }
    }

    fn aggregate(&self, encounters: &[&CombatSnapshot], id: String, status: &str, shifts: &mut Option<Shifts>, run: bool) -> CombatSnapshot {
        let seconds: f64 = encounters.iter().map(|e| e.elapsed_seconds).sum();
        let damage: f64 = encounters.iter().map(|e| e.total_damage).sum();
        let healing: f64 = encounters.iter().map(|e| e.total_healing).sum();
        let mut groups: CsMap<&str, Vec<&PlayerStats>> = CsMap::new();
        for player in encounters.iter().flat_map(|e| e.members.iter().chain(e.former_members.iter().flatten())) {
            groups.get_or_insert_with(player.id.as_str(), Vec::new).push(player);
        }
        let members = groups
            .values()
            .map(|group| {
                let member = *group.last().unwrap();
                let player_damage: f64 = group.iter().map(|p| p.damage).sum();
                let player_healing: f64 = group.iter().map(|p| p.healing).sum();
                PlayerStats {
                    damage: player_damage,
                    healing: player_healing,
                    encounter_dps: if seconds > 0.0 { player_damage / seconds } else { 0.0 },
                    encounter_hps: if seconds > 0.0 { player_healing / seconds } else { 0.0 },
                    rolling_dps: 0.0,
                    rolling_hps: 0.0,
                    damage_share: if damage > 0.0 { player_damage / damage * 100.0 } else { 0.0 },
                    healing_share: if healing > 0.0 { player_healing / healing * 100.0 } else { 0.0 },
                    ..member.clone()
                }
            })
            .collect();
        let mut weapons: CsMap<&str, Vec<&WeaponContribution>> = CsMap::new();
        for weapon in encounters.iter().flat_map(|e| e.weapon_contributions.iter().flatten()) {
            weapons.get_or_insert_with(weapon.id.as_str(), Vec::new).push(weapon);
        }
        let weapons = weapons
            .values()
            .map(|group| WeaponContribution {
                damage: group.iter().map(|w| w.damage).sum(),
                healing: group.iter().map(|w| w.healing).sum(),
                ..(*group.last().unwrap()).clone()
            })
            .collect();
        let mut items: CsMap<&str, Vec<&ItemContribution>> = CsMap::new();
        for item in encounters.iter().flat_map(|e| e.item_contributions.iter().flatten()) {
            items.get_or_insert_with(item.id.as_str(), Vec::new).push(item);
        }
        let items = items
            .values()
            .map(|group| ItemContribution {
                damage: group.iter().map(|i| i.damage).sum(),
                healing: group.iter().map(|i| i.healing).sum(),
                ..(*group.last().unwrap()).clone()
            })
            .collect();
        // A second fed by one encounter point is that point (moved to its second): C# builds an equal copy by adding
        // it to empty maps. Only seconds where encounters meet are added up.
        enum Slot {
            One(Arc<TimelinePoint>),
            Many(TimelinePoint),
        }
        fn add(output: &mut Values, input: Option<&Values>) {
            for (key, value) in input.into_iter().flat_map(|values| values.iter()) {
                let total = output.get(&key).copied().unwrap_or(0.0) + value;
                output.insert(key, total);
            }
        }
        fn add_point(target: &mut TimelinePoint, point: &TimelinePoint) {
            add(&mut target.damage, Some(&point.damage));
            add(&mut target.healing, Some(&point.healing));
            add(target.weapon_damage.as_mut().unwrap(), point.weapon_damage.as_ref());
            add(target.weapon_healing.as_mut().unwrap(), point.weapon_healing.as_ref());
            add(target.item_damage.as_mut().unwrap(), point.item_damage.as_ref());
            add(target.item_healing.as_mut().unwrap(), point.item_healing.as_ref());
        }
        fn moved(encounter: &CombatSnapshot, offset: f64) -> Vec<Arc<TimelinePoint>> {
            encounter
                .timeline
                .iter()
                .map(|point| {
                    let second = (offset + f64::from(point.second)).floor() as i32;
                    if second == point.second { point.clone() } else { Arc::new(TimelinePoint { second, ..(**point).clone() }) }
                })
                .collect()
        }
        // Encounters come in start order and each one's seconds after the last one's (they meet on at most one
        // boundary second), so the totals are merged in one pass; any other layout goes through a map.
        // One encounter at offset 0 is its own timeline: every point lands on its own second.
        let single = match encounters {
            [only] => Some(only.timeline.clone()),
            _ => None,
        };
        let mut timeline: Vec<Slot> = Vec::new();
        let mut unordered: Option<BTreeMap<i32, Slot>> = None;
        let mut ordered: Vec<&&CombatSnapshot> = encounters.iter().collect();
        ordered.sort_by_key(|e| e.started_at);
        let mut offset = 0.0;
        for encounter in ordered.into_iter().filter(|_| single.is_none()) {
            let points = match shifts {
                Some(shifts) => {
                    let entry = shifts.entry((run, encounter.id.clone())).or_insert_with(|| (-1, 0, Vec::new()));
                    if entry.0 != encounter.sequence || entry.1 != f64::to_bits(offset) || entry.2.len() != encounter.timeline.len() {
                        *entry = (encounter.sequence, offset.to_bits(), moved(encounter, offset));
                    }
                    entry.2.clone()
                }
                None => moved(encounter, offset),
            };
            fn second(slot: &Slot) -> i32 {
                match slot {
                    Slot::One(point) => point.second,
                    Slot::Many(point) => point.second,
                }
            }
            fn join(slot: &mut Slot, point: &TimelinePoint) {
                if let Slot::One(first) = slot {
                    let mut target = TimelinePoint::empty(first.second);
                    add_point(&mut target, first);
                    *slot = Slot::Many(target);
                }
                if let Slot::Many(target) = slot {
                    add_point(target, point);
                }
            }
            for point in points {
                if unordered.is_none() && timeline.last().is_some_and(|last| second(last) > point.second) {
                    unordered = Some(timeline.drain(..).map(|slot| (second(&slot), slot)).collect());
                }
                match &mut unordered {
                    Some(map) => match map.entry(point.second) {
                        std::collections::btree_map::Entry::Vacant(slot) => {
                            slot.insert(Slot::One(point));
                        }
                        std::collections::btree_map::Entry::Occupied(mut slot) => join(slot.get_mut(), &point),
                    },
                    None => match timeline.last_mut() {
                        Some(last) if second(last) == point.second => join(last, &point),
                        _ => timeline.push(Slot::One(point)),
                    },
                }
            }
            offset += encounter.elapsed_seconds;
        }
        let slots: Vec<Slot> = match unordered {
            Some(map) => map.into_values().collect(),
            None => timeline,
        };
        let timeline = single.unwrap_or_else(|| {
            slots
                .into_iter()
                .map(|slot| match slot {
                    Slot::One(point) => point,
                    Slot::Many(point) => Arc::new(point),
                })
                .collect()
        });
        CombatSnapshot {
            id,
            started_at: self.started_at,
            elapsed_seconds: seconds,
            status: status.into(),
            mode: self.mode.clone(),
            sequence: encounters.iter().map(|e| e.sequence).sum(),
            event_count: encounters.iter().map(|e| e.event_count).sum(),
            total_damage: damage,
            total_healing: healing,
            members,
            timeline,
            full: true,
            location: encounters.first().and_then(|e| e.location.clone()),
            former_members: None,
            party_id: None,
            weapon_contributions: Some(weapons),
            item_contributions: Some(items),
        }
    }
}
