//! MeterSession.cs `ResolveLive` and `LiveDungeons`: weapon, item and map names from the game catalogs, membership
//! from the roster, and the record's mode and status; plus the conversions to the UI's feed types.

use crate::feed;
use meter_core::cs::Guid;
use meter_core::dungeon::{DungeonLocation, DungeonRunSnapshot};
use meter_core::engine::{ItemContribution, PlayerStats, TimelinePoint};
use meter_core::maps::MapCatalog;
use meter_core::{CaptureReport, CombatSnapshot, CombatSourceCatalog};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// The game catalogs the names come from. The engine thread owns it; an import replaces it whole.
pub(crate) struct Names {
    pub maps: Arc<MapCatalog>,
    pub sources: Arc<CombatSourceCatalog>,
    /// "Name · T8.3" → item id, the weapons.ts match the UI uses for weapon icons.
    weapon_ids: HashMap<String, Arc<str>>,
}

impl Names {
    pub fn new(maps: Arc<MapCatalog>, sources: Arc<CombatSourceCatalog>) -> Self {
        let weapon_ids = maps
            .data()
            .and_then(|data| data.indexed_weapons.as_ref())
            .into_iter()
            .flatten()
            .filter_map(|weapon| weapon.tier.map(|tier| (format!("{} · T{tier}.{}", weapon.name, weapon.enchantment), Arc::from(weapon.id.as_str()))))
            .collect();
        Self { maps, sources, weapon_ids }
    }
}

/// What resolving a record needs from the capture report besides the record itself.
#[derive(Clone, Default)]
pub(crate) struct LiveContext {
    pub character: Option<Guid>,
    pub roster_confirmed: bool,
    pub weapons: HashMap<Guid, Option<i32>>,
    /// replay | live
    pub mode: &'static str,
    pub status: &'static str,
}

/// `status.Report?.RecordState ?? (RecordPaused ? "paused" : state is running or starting ? "running" : "completed")`.
pub(crate) fn record_status(report_state: Option<&'static str>, paused: bool, capture_running: bool) -> &'static str {
    report_state.unwrap_or(if paused {
        "paused"
    } else if capture_running {
        "running"
    } else {
        "completed"
    })
}

impl LiveContext {
    pub fn of(report: &CaptureReport, replay: bool, capture_running: bool) -> Self {
        Self {
            character: report.character.as_ref().map(|character| character.character_id),
            roster_confirmed: report.party.as_ref().is_some_and(|party| party.is_confirmed_since(report.first_timestamp_microseconds)),
            weapons: report.weapons.iter().flat_map(|weapons| weapons.iter().map(|(id, weapon)| (*id, *weapon))).collect(),
            mode: if replay { "replay" } else { "live" },
            status: record_status(report.record_state, report.record_paused, capture_running),
        }
    }
}

fn item_number(weapon: &str) -> Option<i32> {
    weapon.strip_prefix("Item #").and_then(|index| index.parse().ok())
}

/// The resolved columns of a member row.
struct Member {
    membership: &'static str,
    weapon: String,
    family: String,
}

impl Names {
    fn member(&self, context: &LiveContext, player: &PlayerStats) -> Member {
        let id = Guid::parse(&player.id);
        let weapon = match id {
            Some(id) => context.weapons.get(&id).copied().flatten(),
            None => item_number(&player.weapon),
        };
        let name = match (id, item_number(&player.weapon)) {
            (Some(_), _) => self.maps.weapon_name(weapon),
            (None, Some(index)) => self.maps.weapon_name(Some(index)),
            (None, None) => player.weapon.clone(),
        };
        Member {
            membership: if (id.is_some() && id == context.character) || context.roster_confirmed { "active" } else { "unconfirmed" },
            weapon: name,
            family: self.maps.weapon_family(weapon).unwrap_or_else(|| player.weapon_family.clone()),
        }
    }

    fn item_name(&self, item: &ItemContribution) -> String {
        if item.tier_observed { self.sources.item_name(item.item_index) } else { self.sources.family_name(item.item_index) }
    }

    /// `ResolveLive`: the record as the UI, the history and AlbionLab see it.
    pub fn resolve_live(&self, context: &LiveContext, mut combat: CombatSnapshot) -> CombatSnapshot {
        combat.mode = context.mode.into();
        combat.status = context.status.into();
        for player in &mut combat.members {
            let member = self.member(context, player);
            (player.membership, player.weapon, player.weapon_family) = (member.membership.into(), member.weapon, member.family);
        }
        for player in combat.former_members.iter_mut().flatten() {
            let member = self.member(context, player);
            (player.membership, player.weapon, player.weapon_family) = ("left".into(), member.weapon, member.family);
        }
        for item in combat.item_contributions.iter_mut().flatten() {
            item.item = self.item_name(item);
        }
        for weapon in combat.weapon_contributions.iter_mut().flatten() {
            weapon.weapon = self.maps.weapon_name(weapon.weapon_index);
        }
        combat
    }

    fn resolve_location(&self, location: &mut DungeonLocation) {
        location.entrance_map_name = self.maps.resolve_name(&location.entrance_map_id);
        if let Some(map) = self.maps.find(&location.map_id).or_else(|| self.maps.find(&location.map_name)) {
            location.map_name = map.name.clone();
        }
    }

    fn resolve_floor_snapshot(&self, snapshot: &mut CombatSnapshot) {
        for player in snapshot.members.iter_mut().chain(snapshot.former_members.iter_mut().flatten()) {
            let index = item_number(&player.weapon);
            if let Some(index) = index {
                player.weapon = self.maps.weapon_name(Some(index));
            }
            if let Some(family) = self.maps.weapon_family(index) {
                player.weapon_family = family;
            }
        }
        if let Some(location) = &mut snapshot.location {
            self.resolve_location(location);
        }
        for weapon in snapshot.weapon_contributions.iter_mut().flatten() {
            weapon.weapon = self.maps.weapon_name(weapon.weapon_index);
        }
        for item in snapshot.item_contributions.iter_mut().flatten() {
            item.item = self.item_name(item);
        }
    }

    /// `LiveDungeons`: a run still "running" while the capture is not is shown paused.
    pub fn resolve_dungeons(&self, runs: Vec<DungeonRunSnapshot>, capture_running: bool) -> Vec<DungeonRunSnapshot> {
        runs.into_iter()
            .map(|mut run| {
                run.entrance_map_name = self.maps.resolve_name(&run.entrance_map_id);
                if run.status == "running" && !capture_running {
                    run.status = "paused".into();
                }
                self.resolve_floor_snapshot(&mut run.totals);
                run.totals.location = None;
                for floor in &mut run.floors {
                    if let Some(map) = self.maps.find(&floor.map_name) {
                        floor.map_name = map.name.clone();
                    }
                    self.resolve_floor_snapshot(&mut floor.totals);
                    for encounter in &mut floor.encounters {
                        self.resolve_floor_snapshot(encounter);
                    }
                }
                run
            })
            .collect()
    }

    pub fn weapon_id(&self, weapon: &str) -> Option<Arc<str>> {
        self.weapon_ids.get(weapon).cloned()
    }

    /// "T8.3" for an item whose tier the capture saw.
    fn tier(&self, item: &ItemContribution) -> Option<Arc<str>> {
        let definition = self.sources.find_item(item.item_index?)?;
        (item.tier_observed && definition.tier.is_some()).then(|| format!("T{}.{}", definition.tier.unwrap_or_default(), definition.enchantment).into())
    }

    pub fn feed_player(&self, context: &LiveContext, player: &PlayerStats, former: bool) -> feed::Player {
        let member = self.member(context, player);
        feed::Player {
            membership: if former { feed::Membership::Left } else { membership(member.membership) },
            id: player.id.as_str().into(),
            name: player.name.as_str().into(),
            weapon_id: self.weapon_id(&member.weapon),
            weapon: member.weapon.into(),
            weapon_family: member.family.into(),
            color: player.color.as_str().into(),
            damage: player.damage,
            healing: player.healing,
            encounter_dps: player.encounter_dps,
            encounter_hps: player.encounter_hps,
            damage_share: player.damage_share,
            healing_share: player.healing_share,
        }
    }

    pub fn feed_item(&self, item: &ItemContribution) -> feed::ItemContribution {
        feed::ItemContribution {
            kind: Some(item.kind.as_str().into()),
            tier_observed: item.tier_observed,
            id: item.id.as_str().into(),
            player_id: item.player_id.as_str().into(),
            player_name: item.player_name.as_str().into(),
            item_index: item.item_index,
            slot: item.slot.as_str().into(),
            item: self.item_name(item).into(),
            tier: self.tier(item),
            damage: item.damage,
            healing: item.healing,
        }
    }
}

fn membership(text: &str) -> feed::Membership {
    match text {
        "left" => feed::Membership::Left,
        "unconfirmed" => feed::Membership::Unconfirmed,
        _ => feed::Membership::Active,
    }
}

pub(crate) fn status(text: &str) -> feed::RecordStatus {
    match text {
        "running" => feed::RecordStatus::Running,
        "paused" => feed::RecordStatus::Paused,
        "completed" => feed::RecordStatus::Completed,
        _ => feed::RecordStatus::Idle,
    }
}

pub(crate) fn feed_point(point: &TimelinePoint) -> feed::Point {
    let values = |values: Option<&meter_core::engine::Values>| values.map(|values| values.iter().map(|(key, value)| (key.clone(), *value)).collect()).unwrap_or_default();
    feed::Point {
        second: point.second.max(0) as u32,
        damage: values(Some(&point.damage)),
        healing: values(Some(&point.healing)),
        item_damage: values(point.item_damage.as_ref()),
        item_healing: values(point.item_healing.as_ref()),
    }
}

// Saved and dungeon records arrive as the stored JSON; these read it the way fixture.rs reads the C# JSON.

fn text(value: &Value, key: &str) -> Arc<str> {
    value.get(key).and_then(Value::as_str).unwrap_or_default().into()
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or_default()
}

fn values(value: Option<&Value>) -> feed::Values {
    value.and_then(Value::as_object).map(|map| map.iter().map(|(key, value)| (Arc::from(key.as_str()), value.as_f64().unwrap_or_default())).collect()).unwrap_or_default()
}

fn player_of(names: &Names, value: &Value, former: bool) -> feed::Player {
    let weapon = text(value, "weapon");
    feed::Player {
        membership: if former { feed::Membership::Left } else { membership(&text(value, "membership")) },
        id: text(value, "id"),
        name: text(value, "name"),
        weapon_id: names.weapon_id(&weapon),
        weapon,
        weapon_family: text(value, "weaponFamily"),
        color: text(value, "color"),
        damage: number(value, "damage"),
        healing: number(value, "healing"),
        encounter_dps: number(value, "encounterDps"),
        encounter_hps: number(value, "encounterHps"),
        damage_share: number(value, "damageShare"),
        healing_share: number(value, "healingShare"),
    }
}

pub(crate) fn snapshot_of(names: &Names, value: &Value) -> feed::Snapshot {
    let list = |key: &str| value.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    feed::Snapshot {
        id: text(value, "id"),
        started_at: text(value, "startedAt"),
        elapsed_seconds: number(value, "elapsedSeconds"),
        status: status(&text(value, "status")),
        mode: text(value, "mode"),
        sequence: value.get("sequence").and_then(Value::as_u64).unwrap_or_default(),
        event_count: value.get("eventCount").and_then(Value::as_u64).unwrap_or_default(),
        total_damage: number(value, "totalDamage"),
        total_healing: number(value, "totalHealing"),
        members: list("members").iter().map(|player| player_of(names, player, false)).collect(),
        former_members: list("formerMembers").iter().map(|player| player_of(names, player, true)).collect(),
        item_contributions: list("itemContributions")
            .iter()
            .map(|item| feed::ItemContribution {
                kind: item.get("kind").and_then(Value::as_str).map(Into::into),
                tier_observed: item.get("tierObserved").and_then(Value::as_bool).unwrap_or(true),
                id: text(item, "id"),
                player_id: text(item, "playerId"),
                player_name: text(item, "playerName"),
                item_index: item.get("itemIndex").and_then(Value::as_i64).map(|index| index as i32),
                slot: text(item, "slot"),
                item: text(item, "item"),
                tier: None,
                damage: number(item, "damage"),
                healing: number(item, "healing"),
            })
            .collect(),
        timeline: list("timeline")
            .iter()
            .map(|point| feed::Point {
                second: point.get("second").and_then(Value::as_u64).unwrap_or_default() as u32,
                damage: values(point.get("damage")),
                healing: values(point.get("healing")),
                item_damage: values(point.get("itemDamage")),
                item_healing: values(point.get("itemHealing")),
            })
            .collect(),
        full: true,
    }
}

impl Names {
    /// A dungeon floor's or run's totals, already resolved, as the journal shows them.
    fn feed_snapshot(&self, combat: &CombatSnapshot) -> feed::Snapshot {
        let player = |player: &PlayerStats, former: bool| feed::Player {
            membership: if former { feed::Membership::Left } else { membership(&player.membership) },
            id: player.id.as_str().into(),
            name: player.name.as_str().into(),
            weapon_id: self.weapon_id(&player.weapon),
            weapon: player.weapon.as_str().into(),
            weapon_family: player.weapon_family.as_str().into(),
            color: player.color.as_str().into(),
            damage: player.damage,
            healing: player.healing,
            encounter_dps: player.encounter_dps,
            encounter_hps: player.encounter_hps,
            damage_share: player.damage_share,
            healing_share: player.healing_share,
        };
        feed::Snapshot {
            id: combat.id.as_str().into(),
            started_at: combat.started_at.to_string().into(),
            elapsed_seconds: combat.elapsed_seconds,
            status: status(&combat.status),
            mode: combat.mode.as_str().into(),
            sequence: combat.sequence.max(0) as u64,
            event_count: combat.event_count.max(0) as u64,
            total_damage: combat.total_damage,
            total_healing: combat.total_healing,
            members: combat.members.iter().map(|member| player(member, false)).collect(),
            former_members: combat.former_members.iter().flatten().map(|member| player(member, true)).collect(),
            item_contributions: combat.item_contributions.iter().flatten().map(|item| self.feed_item(item)).collect(),
            timeline: combat.timeline.iter().map(|point| feed_point(point)).collect(),
            full: true,
        }
    }

    pub fn feed_run(&self, run: &DungeonRunSnapshot) -> feed::DungeonRun {
        feed::DungeonRun {
            summary: summary_of(&dungeon_summary(run)),
            floors: run
                .floors
                .iter()
                .map(|floor| feed::DungeonFloor {
                    id: floor.id.as_str().into(),
                    number: floor.number.max(0) as u32,
                    map_id: floor.map_id.as_str().into(),
                    map_name: floor.map_name.as_str().into(),
                    totals: Arc::new(self.feed_snapshot(&floor.totals)),
                })
                .collect(),
            totals: Arc::new(self.feed_snapshot(&run.totals)),
        }
    }
}

/// `EncounterStore.Summarize` of a live run, without going through its JSON.
pub(crate) fn dungeon_summary(run: &DungeonRunSnapshot) -> meter_store::DungeonSummary {
    meter_store::DungeonSummary {
        id: run.id.clone(),
        started_at: meter_store::DateTimeOffset::from_unix_milliseconds(run.started_at.unix_milliseconds()),
        name: run.name.clone(),
        entrance_map_id: run.entrance_map_id.clone(),
        entrance_map_name: run.entrance_map_name.clone(),
        mode: run.mode.clone(),
        status: run.status.clone(),
        floors: run.floors.len() as i32,
        duration: run.totals.elapsed_seconds,
        damage: run.totals.total_damage,
        healing: run.totals.total_healing,
    }
}

pub(crate) fn summary_of(summary: &meter_store::DungeonSummary) -> feed::DungeonSummary {
    feed::DungeonSummary {
        id: summary.id.as_str().into(),
        started_at: summary.started_at.to_json().into(),
        name: summary.name.as_str().into(),
        entrance_map_name: summary.entrance_map_name.as_str().into(),
        mode: summary.mode.as_str().into(),
        status: summary.status.as_str().into(),
        floors: summary.floors.max(0) as u32,
        duration: summary.duration,
        damage: summary.damage,
        healing: summary.healing,
    }
}

pub(crate) fn run_of(names: &Names, value: &Value) -> Option<feed::DungeonRun> {
    let summary = meter_store::DungeonSummary::of(value).ok()?;
    let floors = value.get("floors").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    Some(feed::DungeonRun {
        summary: summary_of(&summary),
        floors: floors
            .iter()
            .map(|floor| feed::DungeonFloor {
                id: text(floor, "id"),
                number: floor.get("number").and_then(Value::as_u64).unwrap_or_default() as u32,
                map_id: text(floor, "mapId"),
                map_name: text(floor, "mapName"),
                totals: Arc::new(snapshot_of(names, floor.get("totals").unwrap_or(&Value::Null))),
            })
            .collect(),
        totals: Arc::new(snapshot_of(names, value.get("totals").unwrap_or(&Value::Null))),
    })
}
