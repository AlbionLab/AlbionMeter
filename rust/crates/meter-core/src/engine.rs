//! Damage and healing accounting for one encounter (`CombatEngine.cs`) and the snapshot types the JSON carries.
//!
//! # Incremental snapshots
//!
//! [`CombatEngine::snapshot`] mirrors the C# API. For a UI, prefer [`CombatEngine::delta`]: timeline points are shared
//! `Arc`s that the engine copies on write, and every point, member row and contribution row carries the
//! [`CombatEngine::sequence`] at which it last changed. `delta(since)` returns the totals and member rows (their rates
//! move with the clock) plus only the contributions and points changed after `since`, and a [`CombatChanges`] set
//! naming the rows to update. A UI keeps its own models keyed by id and second, applies each delta and passes the
//! delta's `sequence` back next time: nothing in the timeline is copied, and an idle tick touches no rows at all.

use crate::cs::{CsMap, DateTimeOffset, Guid};
use crate::dungeon::DungeonLocation;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

pub const MAXIMUM_SECONDS: f64 = 7200.0;
const MAXIMUM_EVENTS: usize = 1_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum CombatMetric {
    Damage,
    Healing,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyMember {
    pub id: String,
    pub name: String,
    pub weapon: String,
    pub weapon_family: String,
    pub color: String,
}

impl PartyMember {
    pub fn new(id: &str, name: &str, weapon: &str, weapon_family: &str, color: &str) -> Self {
        Self { id: id.into(), name: name.into(), weapon: weapon.into(), weapon_family: weapon_family.into(), color: color.into() }
    }
}

/// A combat event's identity: the Photon command plus the index inside it (`{identity}:{index}` in C#), or text.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum EventId {
    Command { connection: Arc<str>, channel: u8, kind: u8, number: u32, extra: Option<u32>, index: u32 },
    Text(Arc<str>),
}

impl From<&str> for EventId {
    fn from(text: &str) -> Self {
        EventId::Text(text.into())
    }
}

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventId::Command { connection, channel, kind, number, extra, index } => {
                write!(f, "{connection}:{channel}:{kind}:{number}")?;
                if let Some(extra) = extra {
                    write!(f, ":{extra}")?;
                }
                write!(f, ":{index}")
            }
            EventId::Text(text) => f.write_str(text),
        }
    }
}

/// Slot and kind names are a small fixed vocabulary; events carry them as interned `&'static str`.
#[derive(Clone, PartialEq, Debug)]
pub struct CombatEvent {
    pub id: EventId,
    pub actor_id: Arc<str>,
    pub time: f64,
    pub metric: CombatMetric,
    pub amount: f64,
    pub weapon_index: Option<i32>,
    pub weapon_name: Option<String>,
    pub source_item_index: Option<i32>,
    pub source_slot: &'static str,
    pub source_kind: &'static str,
    pub source_exact: bool,
}

impl CombatEvent {
    pub fn new(id: &str, actor_id: &str, time: f64, metric: CombatMetric, amount: f64) -> Self {
        Self {
            id: id.into(),
            actor_id: actor_id.into(),
            time,
            metric,
            amount,
            weapon_index: None,
            weapon_name: None,
            source_item_index: None,
            source_slot: "unknown",
            source_kind: "unknown",
            source_exact: true,
        }
    }
    pub fn weapon(mut self, index: i32) -> Self {
        self.weapon_index = Some(index);
        self
    }
    pub fn source(mut self, index: i32, slot: &str, kind: &str) -> Self {
        self.source_item_index = Some(index);
        self.source_slot = crate::cs::intern(slot);
        self.source_kind = crate::cs::intern(kind);
        self
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStats {
    pub id: String,
    pub name: String,
    pub weapon: String,
    pub weapon_family: String,
    pub color: String,
    pub damage: f64,
    pub healing: f64,
    pub encounter_dps: f64,
    pub encounter_hps: f64,
    pub rolling_dps: f64,
    pub rolling_hps: f64,
    pub damage_share: f64,
    pub healing_share: f64,
    pub membership: String,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaponContribution {
    pub id: String,
    pub player_id: String,
    pub player_name: String,
    pub weapon_index: Option<i32>,
    pub weapon: String,
    pub color: String,
    pub damage: f64,
    pub healing: f64,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemContribution {
    pub id: String,
    pub player_id: String,
    pub player_name: String,
    pub item_index: Option<i32>,
    pub slot: String,
    pub item: String,
    pub color: String,
    pub damage: f64,
    pub healing: f64,
    pub kind: String,
    pub tier_observed: bool,
}

pub use crate::values::Values;

/// The ids of the events a record counted, exactly, in a few bits each: Photon numbers a channel's commands in order,
/// so each (connection, channel, type, index) keeps a bitmap over its command numbers. Anything that does not fit
/// (unreliable commands, text ids, a far jump) goes to an ordinary set.
/// (connection, channel, command type, index in the message)
type RunKey = (Arc<str>, u8, u8, u32);

#[derive(Default)]
struct EventSet {
    len: usize,
    runs: HashMap<RunKey, Bits>,
    others: HashSet<EventId>,
}

#[derive(Default)]
struct Bits {
    base: u32,
    words: Vec<u64>,
}

/// How far a bitmap may stretch to take a number: 2^24 commands, 2 MiB.
const SPAN: u64 = 1 << 24;

impl Bits {
    fn locate(&self, number: u32) -> Option<(usize, u64)> {
        let offset = u64::from(number.checked_sub(self.base)?);
        Some(((offset / 64) as usize, 1 << (offset % 64)))
    }
    fn contains(&self, number: u32) -> bool {
        self.locate(number).is_some_and(|(word, bit)| self.words.get(word).is_some_and(|w| w & bit != 0))
    }
    /// False when the number lies too far from the numbers seen so far.
    fn insert(&mut self, number: u32) -> bool {
        if self.words.is_empty() {
            self.base = number & !63;
        }
        if number < self.base {
            let gap = (self.base - (number & !63)) as u64;
            if gap + self.words.len() as u64 * 64 > SPAN {
                return false;
            }
            let words = (gap / 64) as usize;
            self.words.splice(0..0, std::iter::repeat_n(0, words));
            self.base = number & !63;
        }
        let (word, bit) = self.locate(number).unwrap();
        if word as u64 * 64 >= SPAN {
            return false;
        }
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        self.words[word] |= bit;
        true
    }
}

impl EventSet {
    fn run_key(id: &EventId) -> Option<(RunKey, u32)> {
        match id {
            EventId::Command { connection, channel, kind, number, extra: None, index } => Some(((connection.clone(), *channel, *kind, *index), *number)),
            _ => None,
        }
    }
    fn contains(&self, id: &EventId) -> bool {
        if let Some((key, number)) = Self::run_key(id)
            && self.runs.get(&key).is_some_and(|bits| bits.contains(number))
        {
            return true;
        }
        self.others.contains(id)
    }
    /// `HashSet.Add`: false when the id was counted already.
    fn insert(&mut self, id: &EventId) -> bool {
        if self.contains(id) {
            return false;
        }
        let stored = match Self::run_key(id) {
            Some((key, number)) => self.runs.entry(key).or_default().insert(number),
            None => false,
        };
        if !stored {
            self.others.insert(id.clone());
        }
        self.len += 1;
        true
    }
    fn len(&self) -> usize {
        self.len
    }
}

static SERIALS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Clone, PartialEq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelinePoint {
    pub second: i32,
    pub damage: Values,
    pub healing: Values,
    pub weapon_damage: Option<Values>,
    pub weapon_healing: Option<Values>,
    pub item_damage: Option<Values>,
    pub item_healing: Option<Values>,
}

impl TimelinePoint {
    pub fn empty(second: i32) -> Self {
        Self {
            second,
            damage: Values::new(),
            healing: Values::new(),
            weapon_damage: Some(Values::new()),
            weapon_healing: Some(Values::new()),
            item_damage: Some(Values::new()),
            item_healing: Some(Values::new()),
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CombatSnapshot {
    pub id: String,
    pub started_at: DateTimeOffset,
    pub elapsed_seconds: f64,
    pub status: String,
    pub mode: String,
    pub sequence: i64,
    pub event_count: i64,
    pub total_damage: f64,
    pub total_healing: f64,
    pub members: Vec<PlayerStats>,
    pub timeline: Vec<Arc<TimelinePoint>>,
    pub full: bool,
    pub location: Option<DungeonLocation>,
    pub former_members: Option<Vec<PlayerStats>>,
    pub party_id: Option<String>,
    pub weapon_contributions: Option<Vec<WeaponContribution>>,
    pub item_contributions: Option<Vec<ItemContribution>>,
}

/// What changed in an encounter since a given [`CombatEngine::sequence`]; see the module docs.
#[derive(Clone, Debug)]
pub struct CombatDelta {
    /// The sequence to pass to the next `delta` call.
    pub sequence: i64,
    /// True when everything is included (`since` was negative or older than the kept removal history): the UI
    /// rebuilds its model instead of merging.
    pub full: bool,
    /// Totals and every member row (their rates move with the clock), plus only the changed contribution rows and
    /// timeline points, oldest second first.
    pub snapshot: CombatSnapshot,
    /// The rows to update.
    pub changed: CombatChanges,
}

/// The model rows a delta touches; ids as in the snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CombatChanges {
    /// Members (active or former) whose name, weapon, totals or membership changed; the others only need their
    /// rate columns refreshed.
    pub members: Vec<String>,
    pub weapon_contributions: Vec<String>,
    pub item_contributions: Vec<String>,
    /// Contributions that dropped to zero and left the snapshot.
    pub removed_contributions: Vec<String>,
    pub seconds: Vec<i32>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EngineError {
    InvalidParty,
    UnknownMember,
    TransitionLimit,
    ParticipantLimit,
    InvalidTime,
    MissingIdentity,
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            EngineError::InvalidParty => "A party can contain at most 100 unique members with nonempty identities.",
            EngineError::UnknownMember => "Unknown party member.",
            EngineError::TransitionLimit => "The membership transition limit was reached.",
            EngineError::ParticipantLimit => "The encounter participant limit was reached.",
            EngineError::InvalidTime => "The time is outside the encounter.",
            EngineError::MissingIdentity => "Character identity is required.",
        })
    }
}

impl std::error::Error for EngineError {}

#[derive(Default)]
struct ActorKeys {
    weapons: HashMap<Option<i32>, Arc<str>>,
    items: HashMap<(&'static str, Option<i32>, &'static str, bool), Arc<str>>,
}

struct Recent {
    actor: Arc<str>,
    metric: CombatMetric,
    time: f64,
    amount: f64,
}

pub struct CombatEngine {
    members: Vec<PartyMember>,
    member_keys: crate::values::Keys,
    membership: CsMap<String, Vec<(f64, Option<f64>)>>,
    damage: CsMap<String, f64>,
    healing: CsMap<String, f64>,
    timeline: BTreeMap<i32, (Arc<TimelinePoint>, i64)>,
    weapons: CsMap<Arc<str>, WeaponContribution>,
    items: CsMap<Arc<str>, ItemContribution>,
    keys: HashMap<Arc<str>, ActorKeys>,
    /// The sequence at which each member and contribution row last changed, for [`CombatEngine::delta`].
    member_revisions: HashMap<String, i64>,
    contribution_revisions: HashMap<Arc<str>, i64>,
    removed: std::collections::VecDeque<(i64, Arc<str>)>,
    removed_horizon: i64,
    event_ids: EventSet,
    serial: u64,
    recent: Vec<Recent>,
    recent_floor: f64,
    dirty: BTreeSet<i32>,
    id: String,
    party_id: String,
    started_at: DateTimeOffset,
    elapsed: f64,
    sequence: i64,
}

fn shift(values: &mut Values, key: &Arc<str>, amount: f64) {
    let value = values.get(key).copied().unwrap_or(0.0) + amount;
    if value.abs() < 1e-6 {
        values.remove(key);
    } else {
        values.insert(key.clone(), value);
    }
}

fn blank(text: &str) -> bool {
    crate::cs::is_blank(text)
}

impl CombatEngine {
    pub fn new(members: Vec<PartyMember>, party_id: Option<String>, id: Option<String>) -> Result<Self, EngineError> {
        let members: Vec<PartyMember> = members.into_iter().take(101).collect();
        let distinct: HashSet<&str> = members.iter().map(|m| m.id.as_str()).collect();
        if members.len() > 100 || members.iter().any(|m| blank(&m.id)) || distinct.len() != members.len() {
            return Err(EngineError::InvalidParty);
        }
        let mut engine = Self {
            damage: members.iter().map(|m| (m.id.clone(), 0.0)).collect(),
            healing: members.iter().map(|m| (m.id.clone(), 0.0)).collect(),
            membership: members.iter().map(|m| (m.id.clone(), vec![(0.0, None)])).collect(),
            member_keys: Arc::new(members.iter().map(|m| Arc::from(m.id.as_str())).collect()),
            members,
            timeline: BTreeMap::new(),
            weapons: CsMap::new(),
            items: CsMap::new(),
            keys: HashMap::new(),
            member_revisions: HashMap::new(),
            contribution_revisions: HashMap::new(),
            removed: std::collections::VecDeque::new(),
            removed_horizon: -1,
            event_ids: EventSet::default(),
            serial: SERIALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            recent: Vec::new(),
            recent_floor: f64::INFINITY,
            dirty: BTreeSet::new(),
            id: id.unwrap_or_else(|| Guid::new_v4().simple()),
            party_id: party_id.unwrap_or_else(|| Guid::new_v4().simple()),
            started_at: DateTimeOffset::now(),
            elapsed: 0.0,
            sequence: 0,
        };
        engine.ensure_second(0);
        Ok(engine)
    }

    /// An encounter with nobody listed yet (`new CombatEngine([])`).
    pub fn empty(id: Option<String>) -> Self {
        Self::new(Vec::new(), None, id).unwrap()
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn party_id(&self) -> &str {
        &self.party_id
    }
    pub fn started_at(&self) -> DateTimeOffset {
        self.started_at
    }
    pub fn elapsed_seconds(&self) -> f64 {
        self.elapsed
    }
    pub fn sequence(&self) -> i64 {
        self.sequence
    }
    pub fn event_count(&self) -> usize {
        self.event_ids.len()
    }
    #[doc(hidden)]
    pub fn debug_sizes(&self) -> String {
        format!(
            "events={} runs={} bits={} others={} recent={} buckets={} weapons={} items={} keys={} member_rev={} contrib_rev={} removed={}",
            self.event_ids.len, self.event_ids.runs.len(), self.event_ids.runs.values().map(|b| b.words.capacity()).sum::<usize>(), self.event_ids.others.len(),
            self.recent.capacity(), self.timeline.len(), self.weapons.len(), self.items.len(), self.keys.len(), self.member_revisions.len(), self.contribution_revisions.len(), self.removed.len()
        )
    }
    /// Unique per engine instance, for callers that keep events of "this" record.
    pub fn serial(&self) -> u64 {
        self.serial
    }

    pub fn knows(&self, actor: &str) -> bool {
        self.membership.contains_key(actor)
    }

    pub fn is_member(&self, actor: &str) -> bool {
        self.membership.get(actor).is_some_and(|periods| periods.last().is_some_and(|p| p.1.is_none()))
    }

    pub fn set_membership(&mut self, actor: &str, active: bool) -> Result<(), EngineError> {
        if !self.membership.contains_key(actor) {
            return Err(EngineError::UnknownMember);
        }
        if self.is_member(actor) == active {
            return Ok(());
        }
        let elapsed = self.elapsed;
        let periods = self.membership.get_mut(actor).unwrap();
        if active {
            if periods.len() >= 1000 {
                return Err(EngineError::TransitionLimit);
            }
            periods.push((elapsed, None));
        } else {
            let last = periods.last_mut().unwrap();
            last.1 = Some(elapsed);
        }
        self.sequence += 1;
        self.touch_member(actor);
        Ok(())
    }

    pub fn extend_membership(&mut self, actor: &str, since: f64) {
        let Some(periods) = self.membership.get_mut(actor) else { return };
        if periods.len() != 1 || periods[0].0 <= since {
            return;
        }
        periods[0].0 = since.max(0.0);
        self.sequence += 1;
    }

    pub fn admit(&mut self, actor: &str, time: f64) {
        let Some(periods) = self.membership.get_mut(actor) else { return };
        let Some(index) = periods.iter().position(|p| p.1.is_none_or(|end| end > time)) else { return };
        if periods[index].0 <= time {
            return;
        }
        let floor = if index > 0 { periods[index - 1].1.unwrap_or(0.0) } else { 0.0 };
        periods[index].0 = time.max(0.0).max(floor);
        self.sequence += 1;
    }

    pub fn cover_membership(&mut self, actor: &str, since: f64) {
        let Some(periods) = self.membership.get_mut(actor) else { return };
        if periods.is_empty() {
            return;
        }
        let floor = if periods.len() > 1 { periods[periods.len() - 2].1.unwrap_or(0.0) } else { 0.0 };
        let start = since.max(0.0).max(floor);
        let last = periods.last_mut().unwrap();
        if last.0 <= start {
            return;
        }
        last.0 = start;
        self.sequence += 1;
    }

    /// `RegisterMember(member, since)`: a first registration is listed from `since`.
    pub fn register_member_since(&mut self, member: PartyMember, since: f64) -> Result<(), EngineError> {
        let known = self.membership.contains_key(&member.id);
        let id = member.id.clone();
        self.register_member(member)?;
        if !known {
            self.membership.get_mut(&id).unwrap()[0] = (since.clamp(0.0, self.elapsed), None);
        }
        Ok(())
    }

    pub fn register_member(&mut self, member: PartyMember) -> Result<(), EngineError> {
        if blank(&member.id) {
            return Err(EngineError::MissingIdentity);
        }
        if let Some(index) = self.members.iter().position(|m| m.id == member.id) {
            let id = member.id.clone();
            let renamed = self.members[index].name != member.name;
            let changed = self.members[index] != member;
            self.members[index] = member;
            self.set_membership(&id, true)?;
            self.sequence += 1;
            if changed {
                self.touch_member(&id);
            }
            if renamed {
                self.touch_contributions_of(&id);
            }
            return Ok(());
        }
        if self.members.len() >= 100 {
            return Err(EngineError::ParticipantLimit);
        }
        self.damage.insert(member.id.clone(), 0.0);
        self.healing.insert(member.id.clone(), 0.0);
        self.membership.insert(member.id.clone(), vec![(self.elapsed, None)]);
        let sequence = self.sequence + 1;
        let key: Arc<str> = member.id.as_str().into();
        Arc::make_mut(&mut self.member_keys).push(key);
        for (second, (point, revision)) in self.timeline.iter_mut() {
            let point = Arc::make_mut(point);
            point.damage.push_member(&self.member_keys, 0.0);
            point.healing.push_member(&self.member_keys, 0.0);
            *revision = sequence;
            self.dirty.insert(*second);
        }
        let id = member.id.clone();
        self.members.push(member);
        self.sequence += 1;
        self.touch_member(&id);
        Ok(())
    }

    pub fn advance(&mut self, time: f64) -> Result<(), EngineError> {
        if !time.is_finite() || time < self.elapsed || time > MAXIMUM_SECONDS {
            return Err(EngineError::InvalidTime);
        }
        let previous = self.elapsed as i32;
        self.elapsed = time;
        for second in previous..=time as i32 {
            self.ensure_second(second);
        }
        // Nothing can expire before the oldest kept event does, so most advances skip the scan.
        if self.recent_floor < time - 10.0 {
            self.recent.retain(|event| event.time >= time - 10.0);
            self.recent_floor = self.recent.iter().map(|event| event.time).fold(f64::INFINITY, f64::min);
        }
        self.sequence += 1;
        Ok(())
    }

    pub fn add(&mut self, event: &CombatEvent) -> bool {
        if matches!(&event.id, EventId::Text(text) if blank(text))
            || !self.damage.contains_key(&*event.actor_id)
            || !event.amount.is_finite()
            || event.amount <= 0.0
            || !event.time.is_finite()
            || event.time < 0.0
            || event.time > self.elapsed
            || self.event_count() >= MAXIMUM_EVENTS
        {
            return false;
        }
        if !self.membership.get(&*event.actor_id).unwrap().iter().any(|p| event.time >= p.0 && p.1.is_none_or(|end| event.time < end)) {
            return false;
        }
        let totals = match event.metric {
            CombatMetric::Damage => &self.damage,
            CombatMetric::Healing => &self.healing,
        };
        let current = *totals.get(&*event.actor_id).unwrap();
        if !(current + event.amount).is_finite() || self.event_ids.contains(&event.id) {
            return false;
        }
        self.event_ids.insert(&event.id);
        let second = event.time.floor() as i32;
        self.ensure_second(second);
        let totals = match event.metric {
            CombatMetric::Damage => &mut self.damage,
            CombatMetric::Healing => &mut self.healing,
        };
        *totals.get_mut(&*event.actor_id).unwrap() += event.amount;
        {
            let point = self.point_mut(second);
            let values = match event.metric {
                CombatMetric::Damage => &mut point.damage,
                CombatMetric::Healing => &mut point.healing,
            };
            *values.get_mut(&event.actor_id).unwrap() += event.amount;
        }
        self.credit(event, second, 1.0);
        if event.time >= self.elapsed - 10.0 {
            self.recent.push(Recent { actor: event.actor_id.clone(), metric: event.metric, time: event.time, amount: event.amount });
            self.recent_floor = self.recent_floor.min(event.time);
        }
        self.dirty.insert(second);
        self.sequence += 1;
        self.touch(second);
        self.touch_member(&event.actor_id);
        true
    }

    pub fn reattribute(&mut self, counted: &CombatEvent, proven: &CombatEvent) -> bool {
        if counted.id != proven.id || !self.event_ids.contains(&counted.id) {
            return false;
        }
        self.reattribute_counted(counted, proven)
    }

    /// `reattribute` for an event the caller knows this engine (by [`Self::serial`]) counted, so its id is not kept.
    pub fn reattribute_counted(&mut self, counted: &CombatEvent, proven: &CombatEvent) -> bool {
        if counted.actor_id != proven.actor_id
            || counted.metric != proven.metric
            || counted.amount != proven.amount
            || counted.time != proven.time
            || !self.damage.contains_key(&*counted.actor_id)
        {
            return false;
        }
        let second = counted.time.floor() as i32;
        self.ensure_second(second);
        self.credit(counted, second, -1.0);
        self.credit(proven, second, 1.0);
        self.dirty.insert(second);
        self.sequence += 1;
        self.touch(second);
        true
    }

    fn touch_member(&mut self, id: &str) {
        let sequence = self.sequence;
        match self.member_revisions.get_mut(id) {
            Some(revision) => *revision = sequence,
            None => {
                self.member_revisions.insert(id.to_owned(), sequence);
            }
        }
    }

    fn touch_contributions_of(&mut self, player: &str) {
        let sequence = self.sequence;
        for (key, _) in self.weapons.iter().filter(|(_, w)| w.player_id == player) {
            self.contribution_revisions.insert(key.clone(), sequence);
        }
        for (key, _) in self.items.iter().filter(|(_, i)| i.player_id == player) {
            self.contribution_revisions.insert(key.clone(), sequence);
        }
    }

    /// Credit runs just before the sequence moves on, so its rows are stamped with the next sequence.
    fn touch_contribution(&mut self, key: &Arc<str>, removed: bool) {
        let sequence = self.sequence + 1;
        if removed {
            self.contribution_revisions.remove(&**key);
            self.removed.push_back((sequence, key.clone()));
            if self.removed.len() > 4096 {
                self.removed_horizon = self.removed.pop_front().unwrap().0;
            }
        } else if let Some(revision) = self.contribution_revisions.get_mut(&**key) {
            *revision = sequence;
        } else {
            self.contribution_revisions.insert(key.clone(), sequence);
        }
    }

    fn point_mut(&mut self, second: i32) -> &mut TimelinePoint {
        Arc::make_mut(&mut self.timeline.get_mut(&second).unwrap().0)
    }

    fn touch(&mut self, second: i32) {
        let sequence = self.sequence;
        if let Some(entry) = self.timeline.get_mut(&second) {
            entry.1 = sequence;
        }
    }

    /// The weapon and item contribution keys of an event (`actor:weapon`, `actor:slot:item:kind[:family]`), built once
    /// per distinct key and shared by every bucket after that.
    fn contribution_keys(&mut self, event: &CombatEvent) -> (Arc<str>, Arc<str>) {
        let keys = match self.keys.get_mut(&*event.actor_id) {
            Some(keys) => keys,
            None => self.keys.entry(event.actor_id.clone()).or_default(),
        };
        let weapon = match &event.weapon_name {
            Some(name) if event.weapon_index.is_none() => format!("{}:{name}", event.actor_id).into(),
            _ => keys
                .weapons
                .entry(event.weapon_index)
                .or_insert_with(|| {
                    format!("{}:{}", event.actor_id, event.weapon_index.map_or_else(|| "unknown".to_owned(), |i| i.to_string())).into()
                })
                .clone(),
        };
        let item = keys
            .items
            .entry((event.source_slot, event.source_item_index, event.source_kind, event.source_exact))
            .or_insert_with(|| {
                format!(
                    "{}:{}:{}:{}{}",
                    event.actor_id,
                    event.source_slot,
                    event.source_item_index.map_or_else(|| "unknown".to_owned(), |i| i.to_string()),
                    event.source_kind,
                    if event.source_exact { "" } else { ":family" }
                )
                .into()
            })
            .clone();
        (weapon, item)
    }

    fn credit(&mut self, event: &CombatEvent, second: i32, sign: f64) {
        let amount = sign * event.amount;
        let damage = event.metric == CombatMetric::Damage;
        let (weapon_key, item_key) = self.contribution_keys(event);
        if !self.weapons.contains_key(&*weapon_key) {
            let member = self.members.iter().find(|m| *m.id == *event.actor_id).unwrap();
            let name = event.weapon_name.clone().unwrap_or_else(|| match event.weapon_index {
                Some(0) => "Unarmed".into(),
                Some(index) => format!("Item #{index}"),
                None => "Weapon not observed".into(),
            });
            let weapon = WeaponContribution {
                id: weapon_key.to_string(),
                player_id: member.id.clone(),
                player_name: member.name.clone(),
                weapon_index: event.weapon_index,
                weapon: name,
                color: member.color.clone(),
                damage: 0.0,
                healing: 0.0,
            };
            self.weapons.insert(weapon_key.clone(), weapon);
        }
        let weapon = self.weapons.get_mut(&*weapon_key).unwrap();
        if damage {
            weapon.damage += amount;
        } else {
            weapon.healing += amount;
        }
        // A new item contribution is named after the weapon contribution as it stood, as in C#.
        let owner = (!self.items.contains_key(&*item_key)).then(|| (weapon.player_id.clone(), weapon.player_name.clone(), weapon.color.clone()));
        let weapon_removed = weapon.damage + weapon.healing < 1e-6;
        if weapon_removed {
            self.weapons.remove(&*weapon_key);
        }
        self.touch_contribution(&weapon_key, weapon_removed);
        if let Some((player_id, player_name, color)) = owner {
            self.items.insert(
                item_key.clone(),
                ItemContribution {
                    id: item_key.to_string(),
                    player_id,
                    player_name,
                    item_index: event.source_item_index,
                    slot: event.source_slot.into(),
                    item: event.source_item_index.map(|i| format!("Item #{i}")).unwrap_or_else(|| "Unknown source".into()),
                    color,
                    damage: 0.0,
                    healing: 0.0,
                    kind: event.source_kind.into(),
                    tier_observed: event.source_exact,
                },
            );
        }
        let item = self.items.get_mut(&*item_key).unwrap();
        if damage {
            item.damage += amount;
        } else {
            item.healing += amount;
        }
        let item_removed = item.damage + item.healing < 1e-6;
        if item_removed {
            self.items.remove(&*item_key);
        }
        self.touch_contribution(&item_key, item_removed);
        let point = self.point_mut(second);
        let (weapons, items) = if damage { (&mut point.weapon_damage, &mut point.item_damage) } else { (&mut point.weapon_healing, &mut point.item_healing) };
        shift(weapons.as_mut().unwrap(), &weapon_key, amount);
        shift(items.as_mut().unwrap(), &item_key, amount);
    }

    pub fn rolling_rate(&self, actor: &str, metric: CombatMetric, window: f64) -> f64 {
        let duration = window.min(self.elapsed);
        if duration <= 0.0 {
            return 0.0;
        }
        let start = self.elapsed - window;
        let mut sum = 0.0;
        for event in &self.recent {
            if *event.actor == *actor && event.metric == metric && (event.time > start || start < 0.0) && event.time <= self.elapsed {
                sum += event.amount;
            }
        }
        sum / duration
    }

    fn name_of<'a>(&'a self, player: &str, recorded: &'a str) -> &'a str {
        self.members.iter().find(|m| m.id == player).map(|m| m.name.as_str()).unwrap_or(recorded)
    }

    /// The members the C# `Snapshot(false).Members` lists, without building a snapshot.
    pub fn active_members(&self) -> impl Iterator<Item = &PartyMember> {
        self.members.iter().filter(|m| self.is_member(&m.id))
    }

    fn header(&self, timeline: Vec<Arc<TimelinePoint>>, full: bool, status: &str) -> CombatSnapshot {
        self.rows(timeline, full, status, |_| true)
    }

    fn rows(&self, timeline: Vec<Arc<TimelinePoint>>, full: bool, status: &str, contribution: impl Fn(&str) -> bool) -> CombatSnapshot {
        let total_damage: f64 = self.damage.values().sum();
        let total_healing: f64 = self.healing.values().sum();
        let duration = self.elapsed;
        let players: Vec<(bool, PlayerStats)> = self
            .members
            .iter()
            .map(|m| {
                let (damage, healing) = (*self.damage.get(&m.id).unwrap(), *self.healing.get(&m.id).unwrap());
                (
                    self.is_member(&m.id),
                    PlayerStats {
                        id: m.id.clone(),
                        name: m.name.clone(),
                        weapon: m.weapon.clone(),
                        weapon_family: m.weapon_family.clone(),
                        color: m.color.clone(),
                        damage,
                        healing,
                        encounter_dps: if duration > 0.0 { damage / duration } else { 0.0 },
                        encounter_hps: if duration > 0.0 { healing / duration } else { 0.0 },
                        rolling_dps: self.rolling_rate(&m.id, CombatMetric::Damage, 5.0),
                        rolling_hps: self.rolling_rate(&m.id, CombatMetric::Healing, 5.0),
                        damage_share: if total_damage > 0.0 { damage / total_damage * 100.0 } else { 0.0 },
                        healing_share: if total_healing > 0.0 { healing / total_healing * 100.0 } else { 0.0 },
                        membership: "active".into(),
                    },
                )
            })
            .collect();
        let (members, former): (Vec<_>, Vec<_>) = players.into_iter().partition(|(active, _)| *active);
        CombatSnapshot {
            id: self.id.clone(),
            started_at: self.started_at,
            elapsed_seconds: duration,
            status: status.into(),
            mode: "live".into(),
            sequence: self.sequence,
            event_count: self.event_count() as i64,
            total_damage,
            total_healing,
            members: members.into_iter().map(|(_, p)| p).collect(),
            timeline,
            full,
            location: None,
            former_members: Some(former.into_iter().map(|(_, p)| PlayerStats { rolling_dps: 0.0, rolling_hps: 0.0, ..p }).collect()),
            party_id: Some(self.party_id.clone()),
            weapon_contributions: Some(
                self.weapons
                    .iter()
                    .filter(|(key, _)| contribution(key))
                    .map(|(_, w)| WeaponContribution { player_name: self.name_of(&w.player_id, &w.player_name).into(), ..w.clone() })
                    .collect(),
            ),
            item_contributions: Some(
                self.items
                    .iter()
                    .filter(|(key, _)| contribution(key))
                    .map(|(_, i)| ItemContribution { player_name: self.name_of(&i.player_id, &i.player_name).into(), ..i.clone() })
                    .collect(),
            ),
        }
    }

    /// `Snapshot(full, status)`. A non-full snapshot carries the seconds changed since the last non-full one and
    /// consumes them; points are shared, never deep-copied.
    pub fn snapshot(&mut self, full: bool, status: &str) -> CombatSnapshot {
        let seconds: Vec<i32> = if full { self.timeline.keys().copied().collect() } else { self.dirty.iter().copied().collect() };
        let points = seconds.iter().map(|s| self.timeline[s].0.clone()).collect();
        if !full {
            self.dirty.clear();
        }
        self.header(points, full, status)
    }

    /// A full snapshot without touching the pending changes `snapshot(false, …)` reports.
    pub fn full_snapshot(&self, status: &str) -> CombatSnapshot {
        self.header(self.timeline.values().map(|(point, _)| point.clone()).collect(), true, status)
    }

    /// What changed after `since` (see the module docs). A negative `since`, or one older than the removal history
    /// kept (4096 removed contributions), returns everything with `full` set.
    pub fn delta(&self, since: i64, status: &str) -> CombatDelta {
        let full = since < 0 || since < self.removed_horizon;
        let changed = |revision: i64| full || revision > since;
        let points: Vec<Arc<TimelinePoint>> = self.timeline.values().filter(|(_, revision)| changed(*revision)).map(|(point, _)| point.clone()).collect();
        let contribution = |key: &str| full || self.contribution_revisions.get(key).is_some_and(|&revision| revision > since);
        let snapshot = self.rows(points, full, status, contribution);
        let changes = CombatChanges {
            members: self.members.iter().filter(|m| full || self.member_revisions.get(&m.id).is_some_and(|&r| r > since)).map(|m| m.id.clone()).collect(),
            weapon_contributions: snapshot.weapon_contributions.iter().flatten().map(|w| w.id.clone()).collect(),
            item_contributions: snapshot.item_contributions.iter().flatten().map(|i| i.id.clone()).collect(),
            removed_contributions: if full {
                Vec::new()
            } else {
                self.removed.iter().filter(|(revision, key)| *revision > since && !self.weapons.contains_key(&**key) && !self.items.contains_key(&**key)).map(|(_, key)| key.to_string()).collect()
            },
            seconds: snapshot.timeline.iter().map(|p| p.second).collect(),
        };
        CombatDelta { sequence: self.sequence, full, snapshot, changed: changes }
    }

    fn ensure_second(&mut self, second: i32) {
        if self.timeline.contains_key(&second) {
            return;
        }
        let mut point = TimelinePoint::empty(second);
        point.damage = Values::members(&self.member_keys);
        point.healing = Values::members(&self.member_keys);
        self.timeline.insert(second, (Arc::new(point), self.sequence + 1));
        self.dirty.insert(second);
        // Two seconds back the bucket is very unlikely to grow again: freeze it. A snapshot that shares it (a dungeon
        // checkpoint always does) keeps the old copy until its next checkpoint.
        if let Some((point, _)) = self.timeline.get_mut(&(second - 2)) {
            let point = Arc::make_mut(point);
            for values in [&mut point.damage, &mut point.healing].into_iter().chain([&mut point.weapon_damage, &mut point.weapon_healing, &mut point.item_damage, &mut point.item_healing].into_iter().flatten()) {
                values.shrink();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EventId, EventSet, SPAN};
    use std::sync::Arc;

    fn command(connection: &Arc<str>, number: u32, extra: Option<u32>, index: u32) -> EventId {
        EventId::Command { connection: connection.clone(), channel: 0, kind: 6, number, extra, index }
    }

    /// The bitmap set answers exactly like a `HashSet` of the ids, out of order, backwards and across a far jump.
    #[test]
    fn the_event_set_is_exact() {
        let (a, b): (Arc<str>, Arc<str>) = ("server:0:1".into(), "server:0:2".into());
        let mut set = EventSet::default();
        let mut reference = std::collections::HashSet::new();
        let numbers = [500u32, 501, 499, 10, 70_000, 64, 63, 500, 2_000_000_000, 7, (SPAN as u32) * 3, 0, 1_000_000];
        for (n, &number) in numbers.iter().enumerate() {
            for id in [command(&a, number, None, 0), command(&a, number, None, n as u32 % 3), command(&b, number, Some(9), 0), EventId::Text(format!("t{number}").into())] {
                assert_eq!(set.insert(&id), reference.insert(id.clone()), "{id}");
            }
        }
        assert_eq!(set.len(), reference.len());
        for number in 0..600 {
            for id in [command(&a, number, None, 0), command(&b, number, None, 0), command(&a, number, None, 1)] {
                assert_eq!(set.contains(&id), reference.contains(&id), "{id}");
            }
        }
    }
}
