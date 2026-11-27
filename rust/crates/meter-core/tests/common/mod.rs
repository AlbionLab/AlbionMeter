//! Builders that read like the C# checks: `ev(p![252 => 6, 0 => 30, 2 => -100f32])`.
#![allow(dead_code, unused_imports)]

pub use meter_core::capture::photon::Connection;
pub use meter_core::capture::protocol18::{Value, WireMessage};
pub use meter_core::cs::Guid;
pub use meter_core::observation::combat::CapturedCombatObservation;
pub use meter_core::observation::dungeon::DungeonCombatObservation;
pub use meter_core::observation::local::LocalCharacterSnapshot;
pub use meter_core::observation::roster::{PartyRosterMember, PartyRosterObservation, PartyRosterSnapshot};
pub use meter_core::testkit::{guid_bytes, strings};

/// `new() { [k] = v, ... }`
#[macro_export]
macro_rules! p {
    ($($key:expr => $value:expr),* $(,)?) => { vec![$(($key as u8, $crate::common::Value::from($value))),*] };
}

pub fn ev(parameters: Vec<(u8, Value<'static>)>) -> WireMessage<'static> {
    WireMessage::new(4, 1, None, parameters)
}

pub fn resp(parameters: Vec<(u8, Value<'static>)>) -> WireMessage<'static> {
    WireMessage::new(3, 1, Some(0), parameters)
}

pub fn req(parameters: Vec<(u8, Value<'static>)>) -> WireMessage<'static> {
    WireMessage::new(2, 1, None, parameters)
}

pub fn c(name: &str) -> Connection {
    name.into()
}

pub fn new_id() -> Guid {
    Guid::new_v4()
}

/// `new LocalCharacterSnapshot(id, objectId, name, null, null, map, null, "observed", at, connection)`
pub fn local(id: Guid, object: i64, name: &str, map: &str, at: i64, connection: &str) -> LocalCharacterSnapshot {
    LocalCharacterSnapshot::new(id, Some(object), name, Some(map), "observed", at, connection)
}

pub fn roster(members: &[(Guid, &str)], at: i64, complete: bool) -> PartyRosterSnapshot {
    PartyRosterSnapshot::new(members.iter().map(|(id, name)| PartyRosterMember::new(*id, name)).collect(), at, complete)
}

pub fn floats(values: &[f32]) -> Value<'static> {
    Value::from(values.iter().map(|&v| Value::Float(v)).collect::<Vec<_>>())
}

pub fn ints(values: &[i32]) -> Value<'static> {
    Value::from(values.iter().map(|&v| Value::Int(v)).collect::<Vec<_>>())
}

pub fn longs(values: &[i64]) -> Value<'static> {
    Value::from(values.iter().map(|&v| Value::Long(v)).collect::<Vec<_>>())
}

pub fn objects(values: Vec<Value<'static>>) -> Value<'static> {
    Value::from(values)
}

pub fn gear(values: &[i32]) -> Vec<Option<i32>> {
    values.iter().map(|&v| Some(v)).collect()
}

/// The combat observation as the C# checks call it.
pub trait Observe {
    fn on(&mut self, message: WireMessage<'_>, connection: &str, identity: &str, at: i64, local: Option<&LocalCharacterSnapshot>, party: Option<&PartyRosterSnapshot>);
}

impl Observe for CapturedCombatObservation {
    fn on(&mut self, message: WireMessage<'_>, connection: &str, identity: &str, at: i64, local: Option<&LocalCharacterSnapshot>, party: Option<&PartyRosterSnapshot>) {
        self.observe(&message, &c(connection), &identity.into(), at, local, party);
    }
}

impl Observe for DungeonCombatObservation {
    fn on(&mut self, message: WireMessage<'_>, connection: &str, identity: &str, at: i64, local: Option<&LocalCharacterSnapshot>, party: Option<&PartyRosterSnapshot>) {
        self.observe(&message, &c(connection), &identity.into(), at, local, party);
    }
}

pub fn diagnostic(combat: &CapturedCombatObservation, name: &str) -> Option<i64> {
    combat.diagnostics().get(name).copied()
}

pub fn member_damage(snapshot: &meter_core::CombatSnapshot, name: &str) -> f64 {
    let matches: Vec<_> = snapshot.members.iter().filter(|m| m.name == name).collect();
    assert_eq!(matches.len(), 1, "exactly one member named {name}");
    matches[0].damage
}

pub fn member_healing(snapshot: &meter_core::CombatSnapshot, name: &str) -> f64 {
    let matches: Vec<_> = snapshot.members.iter().filter(|m| m.name == name).collect();
    assert_eq!(matches.len(), 1, "exactly one member named {name}");
    matches[0].healing
}

pub fn timeline_damage(snapshot: &meter_core::CombatSnapshot) -> f64 {
    snapshot.timeline.iter().map(|p| p.damage.values().sum::<f64>()).sum()
}

pub fn timeline_healing(snapshot: &meter_core::CombatSnapshot) -> f64 {
    snapshot.timeline.iter().map(|p| p.healing.values().sum::<f64>()).sum()
}
