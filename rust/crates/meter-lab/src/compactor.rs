//! SessionCompactor.cs: turns the meter's snapshots into the smallest delta the site needs, remembering what it sent.

use crate::protocol::{DungeonInfo, Entry, FloorInfo, PlayerInfo, SessionDelta, Snap, SourceKey};
use indexmap::IndexMap;
use meter_store::DateTimeOffset;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, PartialEq)]
pub struct SourceMember {
    pub id: String,
    pub name: String,
    pub weapon: String,
    pub family: String,
    pub color: String,
    pub membership: String,
    pub weapon_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceWeapon {
    pub id: String,
    pub player_id: String,
    pub index: Option<i32>,
    pub name: String,
    pub unique_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceItem {
    pub id: String,
    pub player_id: String,
    pub slot: String,
    pub index: Option<i32>,
    pub name: String,
    pub kind: String,
    pub tier_observed: bool,
    pub unique_id: Option<String>,
}

/// Source id → amount, in the snapshot's order (it decides the order new keys are numbered in).
pub type Amounts = Vec<(String, f64)>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceSecond {
    pub second: i32,
    pub damage: Amounts,
    pub healing: Amounts,
    pub weapon_damage: Option<Amounts>,
    pub weapon_healing: Option<Amounts>,
    pub item_damage: Option<Amounts>,
    pub item_healing: Option<Amounts>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceSnapshot {
    pub id: String,
    pub started_at: DateTimeOffset,
    pub elapsed: f64,
    pub status: String,
    pub events: i64,
    pub members: Vec<SourceMember>,
    pub weapons: Vec<SourceWeapon>,
    pub items: Vec<SourceItem>,
    pub timeline: Vec<SourceSecond>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceFloor {
    pub id: String,
    pub number: i32,
    pub map: Option<String>,
    pub snaps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceDungeon {
    pub id: String,
    pub name: String,
    pub started_at: DateTimeOffset,
    pub status: String,
    pub entrance: Option<String>,
    pub floors: Vec<SourceFloor>,
}

pub struct SessionCompactor {
    keys: HashMap<String, usize>,
    table: Vec<SourceKey>,
    keys_sent: usize,
    players_sent: HashMap<String, PlayerInfo>,
    seconds_sent: HashMap<String, HashMap<i32, u64>>,
    snaps_sent: HashMap<String, String>,
    dungeons_sent: Option<String>,
    status_sent: Option<String>,
    record_sent: Option<String>,
    // Every place of the record so far, kept across resets so a full resend carries them all.
    places: BTreeMap<i32, String>,
    places_sent: usize,
    seq: i64,
    reset: bool,
}

impl Default for SessionCompactor {
    fn default() -> Self {
        Self {
            keys: HashMap::new(),
            table: Vec::new(),
            keys_sent: 0,
            players_sent: HashMap::new(),
            seconds_sent: HashMap::new(),
            snaps_sent: HashMap::new(),
            dungeons_sent: None,
            status_sent: None,
            record_sent: None,
            places: BTreeMap::new(),
            places_sent: 0,
            seq: 0,
            reset: true,
        }
    }
}

impl SessionCompactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// The next delta sends everything again (after a reconnect or a failed publish).
    pub fn reset(&mut self) {
        self.reset = true;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn next(
        &mut self,
        status: &str,
        started_at: DateTimeOffset,
        self_id: Option<&str>,
        record: &SourceSnapshot,
        encounters: &[SourceSnapshot],
        dungeons: &[SourceDungeon],
        place: Option<&str>,
    ) -> Option<SessionDelta> {
        let full = self.reset;
        if full {
            self.players_sent.clear();
            self.seconds_sent.clear();
            self.snaps_sent.clear();
            self.dungeons_sent = None;
            self.status_sent = None;
            self.record_sent = None;
            self.keys_sent = 0;
            self.places_sent = 0;
        }
        // A record started again from zero has none of its old places.
        let second = record.elapsed as i32;
        if self.places.last_key_value().is_some_and(|(last, _)| *last > second) {
            self.places.clear();
            self.places_sent = 0;
        }
        if let Some(place) = place.filter(|place| !place.is_empty())
            && self.places.last_key_value().is_none_or(|(_, last)| last != place)
        {
            self.places.insert(second, place.into());
        }
        let mut delta = SessionDelta { reset: full, ..SessionDelta::default() };
        let mut changed = full;
        if self.status_sent.as_deref() != Some(status) {
            self.status_sent = Some(status.into());
            delta.status = Some(status.into());
            delta.started_at = Some(started_at);
            delta.self_id = self_id.map(Into::into);
            changed = true;
        }
        for snap in std::iter::once(record).chain(encounters.iter().filter(|snap| snap.id != record.id)) {
            for member in &snap.members {
                let player = PlayerInfo { name: member.name.clone(), weapon: member.weapon.clone(), weapon_id: member.weapon_id.clone(), family: member.family.clone(), color: member.color.clone() };
                if self.players_sent.get(&member.id) == Some(&player) {
                    continue;
                }
                self.players_sent.insert(member.id.clone(), player.clone());
                delta.players.get_or_insert_with(IndexMap::new).insert(member.id.clone(), player);
            }
            if let Some(compact) = self.compact(snap, full) {
                delta.snaps.get_or_insert_with(IndexMap::new).insert(snap.id.clone(), compact);
                changed = true;
            }
        }
        if self.record_sent.as_deref() != Some(&record.id) {
            self.record_sent = Some(record.id.clone());
            delta.record = Some(record.id.clone());
            changed = true;
        }
        if self.places.len() > self.places_sent {
            delta.places = Some(self.places.iter().skip(self.places_sent).map(|(second, place)| (*second, place.clone())).collect());
            self.places_sent = self.places.len();
            changed = true;
        }
        let dungeon_text = dungeons
            .iter()
            .map(|dungeon| {
                let floors: Vec<String> = dungeon.floors.iter().map(|floor| format!("{}:{}:{}:{}", floor.id, floor.number, floor.map.as_deref().unwrap_or_default(), floor.snaps.join("+"))).collect();
                format!("{};{};{};{};{}", dungeon.id, dungeon.name, dungeon.status, dungeon.entrance.as_deref().unwrap_or_default(), floors.join(","))
            })
            .collect::<Vec<_>>()
            .join("|");
        if self.dungeons_sent.as_deref() != Some(&dungeon_text) {
            self.dungeons_sent = Some(dungeon_text);
            delta.dungeons = Some(
                dungeons
                    .iter()
                    .map(|dungeon| DungeonInfo {
                        id: dungeon.id.clone(),
                        name: dungeon.name.clone(),
                        started_at: dungeon.started_at,
                        status: dungeon.status.clone(),
                        entrance: dungeon.entrance.clone(),
                        floors: dungeon.floors.iter().map(|floor| FloorInfo { id: floor.id.clone(), number: floor.number, map: floor.map.clone(), snaps: floor.snaps.clone() }).collect(),
                    })
                    .collect(),
            );
            changed = true;
        }
        if delta.players.is_some() {
            changed = true;
        }
        if self.keys_sent < self.table.len() {
            delta.keys = Some((self.keys_sent..self.table.len()).map(|index| (index, self.table[index].clone())).collect());
            self.keys_sent = self.table.len();
        }
        if !changed {
            return None;
        }
        self.reset = false;
        self.seq += 1;
        delta.seq = self.seq;
        Some(delta)
    }

    fn key(&mut self, key: String, describe: impl FnOnce() -> SourceKey) -> usize {
        if let Some(index) = self.keys.get(&key) {
            return *index;
        }
        let index = self.table.len();
        self.table.push(describe());
        self.keys.insert(key, index);
        index
    }

    fn compact(&mut self, snap: &SourceSnapshot, full: bool) -> Option<Snap> {
        let weapons: HashMap<&str, &SourceWeapon> = snap.weapons.iter().map(|weapon| (weapon.id.as_str(), weapon)).collect();
        let items: HashMap<&str, &SourceItem> = snap.items.iter().map(|item| (item.id.as_str(), item)).collect();
        let mut sent = self.seconds_sent.remove(&snap.id).unwrap_or_default();
        let mut seconds: Option<BTreeMap<i32, Vec<Entry>>> = None;
        let owner = |id: &str| id.split(':').next().unwrap_or_default().to_owned();
        for point in &snap.timeline {
            let mut entries: Vec<Entry> = Vec::new();
            let mut add = |compactor: &mut Self, key: String, describe: &dyn Fn() -> SourceKey, damage: f64, healing: f64| {
                if damage <= 0.0 && healing <= 0.0 {
                    return;
                }
                let index = compactor.key(key, describe);
                let entry = match entries.iter_mut().position(|entry| entry[0] == index as f64) {
                    Some(position) => &mut entries[position],
                    None => {
                        entries.push([index as f64, 0.0, 0.0]);
                        entries.last_mut().expect("just pushed")
                    }
                };
                entry[1] += damage.round_ties_even();
                entry[2] += healing.round_ties_even();
            };
            for (player, value) in &point.damage {
                add(self, format!("p|{player}"), &|| SourceKey { t: "p".into(), p: player.clone(), ..Default::default() }, *value, 0.0);
            }
            for (player, value) in &point.healing {
                add(self, format!("p|{player}"), &|| SourceKey { t: "p".into(), p: player.clone(), ..Default::default() }, 0.0, *value);
            }
            for (values, healing) in [(&point.weapon_damage, false), (&point.weapon_healing, true)] {
                for (id, value) in values.iter().flatten() {
                    let describe = || match weapons.get(id.as_str()) {
                        Some(weapon) => SourceKey { t: "w".into(), p: weapon.player_id.clone(), x: weapon.index, n: Some(weapon.name.clone()), u: weapon.unique_id.clone(), ..Default::default() },
                        None => SourceKey { t: "w".into(), p: owner(id), n: Some("Unknown weapon".into()), ..Default::default() },
                    };
                    let (damage, heal) = if healing { (0.0, *value) } else { (*value, 0.0) };
                    add(self, format!("w|{id}"), &describe, damage, heal);
                }
            }
            for (values, healing) in [(&point.item_damage, false), (&point.item_healing, true)] {
                for (id, value) in values.iter().flatten() {
                    let describe = || match items.get(id.as_str()) {
                        Some(item) => SourceKey {
                            t: "i".into(),
                            p: item.player_id.clone(),
                            x: item.index,
                            s: Some(item.slot.clone()),
                            k: Some(item.kind.clone()),
                            n: Some(item.name.clone()),
                            u: item.unique_id.clone(),
                            o: Some(item.tier_observed),
                        },
                        None => SourceKey { t: "i".into(), p: owner(id), n: Some("Unknown source".into()), ..Default::default() },
                    };
                    let (damage, heal) = if healing { (0.0, *value) } else { (*value, 0.0) };
                    add(self, format!("i|{id}"), &describe, damage, heal);
                }
            }
            let mut list: Vec<Entry> = entries.into_iter().filter(|entry| entry[1] > 0.0 || entry[2] > 0.0).collect();
            list.sort_by(|a, b| a[0].total_cmp(&b[0]));
            let mut hasher = DefaultHasher::new();
            for entry in &list {
                entry.map(f64::to_bits).hash(&mut hasher);
            }
            let hash = hasher.finish();
            let previous = sent.get(&point.second);
            if (list.is_empty() && previous.is_none()) || previous == Some(&hash) {
                continue;
            }
            sent.insert(point.second, hash);
            seconds.get_or_insert_with(BTreeMap::new).insert(point.second, list);
        }
        self.seconds_sent.insert(snap.id.clone(), sent);
        let members: Vec<String> = snap.members.iter().map(|member| member.id.clone()).collect();
        let membership: IndexMap<String, String> = snap.members.iter().filter(|member| member.membership != "active").map(|member| (member.id.clone(), member.membership.clone())).collect();
        let header = format!(
            "{:.1};{};{};{};{}",
            snap.elapsed,
            snap.status,
            snap.events,
            members.join(","),
            membership.iter().map(|(id, state)| format!("{id}={state}")).collect::<Vec<_>>().join(",")
        );
        let header_changed = self.snaps_sent.get(&snap.id) != Some(&header);
        if !full && seconds.is_none() && !header_changed {
            return None;
        }
        self.snaps_sent.insert(snap.id.clone(), header);
        let whole = full || header_changed;
        Some(Snap {
            id: snap.id.clone(),
            started_at: snap.started_at,
            elapsed: snap.elapsed,
            status: snap.status.clone(),
            events: snap.events,
            members: whole.then_some(members),
            membership: whole.then_some(membership),
            sec: seconds,
        })
    }
}
