//! tests/AlbionMeter.Tests/ObservationChecks.cs, part one: identities, membership, pauses and dungeon floors.

mod common;

use common::*;
use meter_core::CombatSourceCatalog;
use meter_core::catalog::CombatSourceData;
use std::sync::Arc;

fn empty_catalog() -> Option<Arc<CombatSourceCatalog>> {
    Some(Arc::new(CombatSourceCatalog::new(CombatSourceData { version: "test".into(), items: vec![], spells: vec![] })))
}

fn event_at(combat: &mut CapturedCombatObservation, fields: Vec<(u8, Value<'static>)>, at: i64, local: Option<&LocalCharacterSnapshot>, party: Option<&PartyRosterSnapshot>) {
    combat.on(ev(fields), "flow", &format!("event:{at}"), at, local, party);
}

/// Feeds the same event to the roster observation and the combat observation, as CaptureAnalysis does.
fn both(roster: &mut PartyRosterObservation, combat: &mut CapturedCombatObservation, local: &LocalCharacterSnapshot, fields: Vec<(u8, Value<'static>)>, at: i64) {
    let message = ev(fields);
    roster.observe(&message, Some(local), at);
    combat.observe(&message, &c("flow"), &format!("event:{at}").into(), at, Some(local), roster.snapshot());
}

#[test]
fn a_weapon_exclusive_frost_effect_is_credited_to_the_worn_staff_not_a_phantom_base_staff_of_another_family() {
    // Frost staves share most abilities; the game data lists each shared/signature effect under a single artifact.
    let catalog = CombatSourceCatalog::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../assets/game-data/combat.json")).unwrap();
    const CHILLHOWL: i32 = 7346;
    const ARCTIC: i32 = 7356;
    const ARCTIC_ONLY_EFFECT: i32 = 3338;
    let id = new_id();
    let local = local(id, 10, "KPey", "one", 1, "flow");
    let mut combat = CapturedCombatObservation::new(Some(Arc::new(catalog)));
    combat.begin_session(Some(1_000_000), None);
    combat.on(ev(p![252 => 90, 0 => 10, 2 => [CHILLHOWL as i16]]), "flow", "equipment", 1_000_001, Some(&local), None);
    combat.on(ev(p![252 => 6, 0 => 30, 6 => 10, 2 => -500f32, 7 => ARCTIC_ONLY_EFFECT, 5 => 0u8]), "flow", "hit", 2_000_000, Some(&local), None);
    let items = combat.snapshot().unwrap().item_contributions.unwrap();
    assert!(items.iter().any(|i| i.item_index == Some(CHILLHOWL) && i.damage == 500.0 && i.tier_observed));
    assert!(!items.iter().any(|i| i.item_index == Some(ARCTIC)));
    assert!(items.iter().all(|i| i.tier_observed));
}

#[test]
fn party_object_evidence_arriving_before_the_zone_join_survives_without_accepting_old_zone_objects() {
    let (local_id, mate_id, stranger_id) = (new_id(), new_id(), new_id());
    let mut local = local(local_id, 10, "Alpha", "old", 1, "old");
    let remembered = roster(&[(local_id, "Alpha"), (mate_id, "WaitMyPool")], 1, true);
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    combat.forget_identities(1_000_000);
    let event = |combat: &mut CapturedCombatObservation, local: &LocalCharacterSnapshot, connection: &str, fields, at: i64| {
        combat.on(ev(fields), connection, &format!("event:{at}"), at, Some(local), Some(&remembered))
    };
    event(&mut combat, &local, "old", p![252 => 670, 0 => 403], 2_000_000);
    event(&mut combat, &local, "next", p![252 => 29, 0 => 402, 1 => "WaitMyPool", 7 => mate_id], 5_000_000);
    event(&mut combat, &local, "next", p![252 => 670, 0 => 402], 5_100_000);
    local = LocalCharacterSnapshot { object_id: Some(432), connection: c("next"), observed_at_microseconds: 5_300_000, ..local };
    combat.on(resp(p![253 => 2, 0 => 432]), "next", "join", 5_300_000, Some(&local), Some(&remembered));
    event(&mut combat, &local, "next", p![252 => 29, 0 => 403, 1 => "Stranger", 7 => stranger_id], 5_400_000);
    event(&mut combat, &local, "next", p![252 => 6, 0 => 900, 6 => 402, 2 => -75f32], 6_000_000);
    event(&mut combat, &local, "next", p![252 => 6, 0 => 900, 6 => 403, 2 => -200f32], 6_100_000);
    combat.flush_unidentified();
    let snapshot = combat.snapshot().unwrap();
    assert_eq!(snapshot.total_damage, 75.0);
    assert_eq!(snapshot.members.iter().find(|m| m.id == mate_id.to_string()).unwrap().damage, 75.0);
    assert!(snapshot.members.iter().all(|m| m.id != stranger_id.to_string()));
}

#[test]
fn a_party_equipment_report_credits_damage_waiting_for_membership_immediately() {
    let (local_id, mate_id) = (new_id(), new_id());
    let local = local(local_id, 10, "Alpha", "dungeon", 1, "flow");
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    combat.forget_identities(1_000_000);
    event_at(&mut combat, p![252 => 29, 0 => 402, 1 => "WaitMyPool", 7 => mate_id], 2_000_000, Some(&local), None);
    event_at(&mut combat, p![252 => 6, 0 => 900, 6 => 402, 2 => -100f32], 3_000_000, Some(&local), None);
    assert_eq!(combat.snapshot().unwrap().total_damage, 0.0);
    event_at(&mut combat, p![252 => 244, 0 => mate_id, 2 => [6661i16, 0, 5347, 5005, 5397, 2900, 2662, 3098]], 4_000_000, Some(&local), None);
    assert_eq!(combat.snapshot().unwrap().members.iter().find(|m| m.id == mate_id.to_string()).unwrap().damage, 100.0);
    event_at(&mut combat, p![252 => 6, 0 => 900, 6 => 402, 2 => -50f32], 5_000_000, Some(&local), None);
    assert_eq!(combat.snapshot().unwrap().total_damage, 150.0);
    combat.flush_unidentified();
    assert_eq!(combat.snapshot().unwrap().total_damage, 150.0);
}

#[test]
fn a_reused_connection_keeps_refreshed_party_evidence_and_expires_evidence_from_its_previous_visit() {
    let (local_id, mate_id, stranger_id) = (new_id(), new_id(), new_id());
    let mut local = local(local_id, 10, "Alpha", "old", 1, "flow");
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    combat.forget_identities(1_000_000);
    event_at(&mut combat, p![252 => 670, 0 => 402], 2_000_000, Some(&local), None);
    event_at(&mut combat, p![252 => 670, 0 => 403], 2_100_000, Some(&local), None);
    combat.on(resp(p![253 => 41]), "flow", "leave-zone", 20_000_000, Some(&local), None);
    event_at(&mut combat, p![252 => 29, 0 => 402, 1 => "WaitMyPool", 7 => mate_id], 20_100_000, Some(&local), None);
    event_at(&mut combat, p![252 => 670, 0 => 402], 20_200_000, Some(&local), None);
    local = LocalCharacterSnapshot { object_id: Some(432), observed_at_microseconds: 20_300_000, ..local };
    combat.on(resp(p![253 => 2, 0 => 432]), "flow", "join", 20_300_000, Some(&local), None);
    event_at(&mut combat, p![252 => 29, 0 => 403, 1 => "Stranger", 7 => stranger_id], 20_400_000, Some(&local), None);
    event_at(&mut combat, p![252 => 6, 0 => 900, 6 => 402, 2 => -75f32], 21_000_000, Some(&local), None);
    event_at(&mut combat, p![252 => 6, 0 => 900, 6 => 403, 2 => -200f32], 21_100_000, Some(&local), None);
    combat.flush_unidentified();
    let snapshot = combat.snapshot().unwrap();
    assert_eq!(snapshot.total_damage, 75.0);
    assert_eq!(snapshot.members.iter().find(|m| m.id == mate_id.to_string()).unwrap().damage, 75.0);
    assert!(snapshot.members.iter().all(|m| m.id != stranger_id.to_string()));
}

#[test]
fn live_pauses_freeze_totals_and_exclude_paused_time_while_equipment_remains_observed() {
    let id = new_id();
    let local = local(id, 10, "Alpha", "one", 1, "flow");
    let mut combat = CapturedCombatObservation::new(empty_catalog());
    combat.begin_session(Some(1_000_000), None);
    let equipment = |combat: &mut CapturedCombatObservation, index: i16, at: i64| {
        combat.on(ev(p![252 => 90, 0 => 10, 2 => [index]]), "flow", &format!("equipment:{at}"), at, Some(&local), None)
    };
    let hit = |combat: &mut CapturedCombatObservation, value: f32, at: i64| {
        combat.on(ev(p![252 => 6, 0 => 30, 6 => 10, 2 => value, 7 => -1, 5 => 0u8]), "flow", &format!("hit:{at}"), at, Some(&local), None)
    };
    equipment(&mut combat, 9022, 1_000_001);
    hit(&mut combat, -100.0, 2_000_000);
    hit(&mut combat, 40.0, 3_000_000);
    combat.set_paused(true, 4_000_000);
    let paused = combat.snapshot().unwrap();
    assert!(paused.status == "paused" && paused.elapsed_seconds == 3.0);
    equipment(&mut combat, 7108, 10_000_000);
    hit(&mut combat, -900.0, 11_000_000);
    hit(&mut combat, 900.0, 11_000_001);
    let snapshot = combat.snapshot().unwrap();
    assert!(snapshot.id == paused.id && snapshot.total_damage == 100.0 && snapshot.total_healing == 40.0);
    assert!(snapshot.event_count == 2 && snapshot.elapsed_seconds == 3.0 && snapshot.timeline.len() == paused.timeline.len());
    assert_eq!(snapshot.members[0].weapon, "Item #7108");
    combat.set_paused(false, 12_000_000);
    assert_eq!(combat.clock_timestamp(), Some(12_000_000));
    hit(&mut combat, -25.0, 13_000_000);
    let resumed = combat.snapshot().unwrap();
    assert!(resumed.id == paused.id && resumed.status == "running" && resumed.total_damage == 125.0 && resumed.total_healing == 40.0);
    assert!(resumed.elapsed_seconds == 4.0 && resumed.members[0].encounter_dps == 31.25);
    let weapons = resumed.weapon_contributions.as_ref().unwrap();
    assert_eq!(weapons.iter().find(|w| w.weapon_index == Some(9022)).unwrap().damage, 100.0);
    assert_eq!(weapons.iter().find(|w| w.weapon_index == Some(7108)).unwrap().damage, 25.0);
    assert_eq!(timeline_damage(&resumed), 125.0);
    combat.set_paused(true, 14_000_000);
    combat.set_paused(false, 16_000_000);
    assert_eq!(combat.clock_timestamp(), Some(16_000_000));
    hit(&mut combat, -10.0, 17_000_000);
    let snapshot = combat.snapshot().unwrap();
    assert!(snapshot.elapsed_seconds == 6.0 && snapshot.total_damage == 135.0);
    combat.begin_session(Some(18_000_000), None);
    let snapshot = combat.snapshot().unwrap();
    assert!(!combat.is_paused() && snapshot.id != paused.id && snapshot.total_damage == 0.0);
}

#[test]
fn paused_dungeon_transitions_keep_floor_identities_without_counting_paused_damage_or_time() {
    let id = new_id();
    let mut local = local(id, 10, "Alpha", "one", 1, "flow");
    let mut dungeon = DungeonCombatObservation::new(None);
    let join = |dungeon: &mut DungeonCombatObservation, local: &mut LocalCharacterSnapshot, map: &str, at: i64| {
        local.map_id = Some(map.into());
        dungeon.on(resp(p![253 => 2, 1 => id, 8 => map, 66 => "1315"]), "flow", &format!("join:{at}"), at, Some(local), None);
    };
    let hit = |dungeon: &mut DungeonCombatObservation, local: &LocalCharacterSnapshot, value: f32, at: i64| {
        dungeon.on(ev(p![252 => 6, 0 => 30, 6 => 10, 2 => value]), "flow", &format!("hit:{at}"), at, Some(local), None)
    };
    join(&mut dungeon, &mut local, "@RANDOMDUNGEON@one", 1_000_000);
    hit(&mut dungeon, &local, 20.0, 1_500_000);
    hit(&mut dungeon, &local, -100.0, 2_000_000);
    dungeon.set_paused(true, 3_000_000);
    join(&mut dungeon, &mut local, "@RANDOMDUNGEON@two", 8_000_000);
    hit(&mut dungeon, &local, -900.0, 9_000_000);
    let paused = dungeon.snapshot().remove(0);
    assert!(paused.status == "paused" && paused.floors.len() == 2 && paused.totals.total_damage == 100.0);
    assert!(paused.floors[0].totals.elapsed_seconds == 2.0 && paused.floors[1].totals.elapsed_seconds == 0.0);
    dungeon.set_paused(false, 11_000_000);
    hit(&mut dungeon, &local, -50.0, 12_000_000);
    let resumed = dungeon.snapshot().remove(0);
    assert!(resumed.status == "running" && resumed.totals.total_damage == 150.0 && resumed.totals.total_healing == 20.0);
    assert!(resumed.totals.elapsed_seconds == 3.0 && resumed.floors[1].totals.elapsed_seconds == 1.0);
}

#[test]
fn effects_from_unnamed_party_members_are_credited_once_chat_equipment_or_own_skills_identify_them() {
    let (local_id, chatter_id, tank_id) = (new_id(), new_id(), new_id());
    let local = local(local_id, 1, "Alpha", "one", 1, "flow");
    let roster = roster(&[(local_id, "Alpha"), (chatter_id, "Beta"), (tank_id, "Gamma")], 5, true);
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    let tank_gear = [7696i16, 0, 4224, 5452, 5472, 2909, 2587, 3098];
    combat.on(ev(p![252 => 29, 0 => 900, 1 => "Gamma", 7 => tank_id, 40 => tank_gear]), "flow", "seen-before", 2, Some(&local), Some(&roster));
    combat.on(resp(p![253 => 2]), "flow", "zone", 3, Some(&local), Some(&roster));
    combat.forget_identities(4);
    let mut hits = 0;
    let mut hit = |combat: &mut CapturedCombatObservation, source: i32, value: f32, at: i64, spell: i16| {
        combat.on(ev(p![252 => 6, 0 => 30, 6 => source, 2 => value, 7 => spell]), "flow", &format!("hit:{hits}"), at, Some(&local), Some(&roster));
        hits += 1;
    };
    hit(&mut combat, 20, -100.0, 1_000_000, 1);
    hit(&mut combat, 20, -50.0, 2_000_000, 1);
    hit(&mut combat, 21, -300.0, 2_100_000, 1);
    hit(&mut combat, 22, 80.0, 2_200_000, 1);
    assert!(diagnostic(&combat, "pendingUnidentified") == Some(4) && combat.snapshot().unwrap().total_damage == 0.0);
    combat.on(ev(p![252 => 74, 0 => "beta", 1 => "hello", 4 => 20, 5 => chatter_id]), "flow", "chat", 3_000_000, Some(&local), Some(&roster));
    assert!(member_damage(&combat.snapshot().unwrap(), "Beta") == 150.0 && diagnostic(&combat, "identifiedByChat") == Some(1));
    let ten = [7696i16, 0, 4224, 5452, 5472, 2909, 2587, 3098, 0, 0];
    combat.on(ev(p![252 => 244, 0 => tank_id, 2 => ten]), "flow", "party-gear", 3_050_000, Some(&local), Some(&roster));
    combat.on(ev(p![252 => 670, 0 => 21]), "flow", "party-object", 3_060_000, Some(&local), Some(&roster));
    combat.on(ev(p![252 => 90, 0 => 21, 2 => ten]), "flow", "gear", 3_100_000, Some(&local), Some(&roster));
    assert!(member_damage(&combat.snapshot().unwrap(), "Gamma") == 300.0 && diagnostic(&combat, "identifiedByEquipment") == Some(1));
    for cast in 0..5 {
        hit(&mut combat, 22, 90.0, 4_200_000 + cast * 2_000_000, 3404);
    }
    assert!(member_healing(&combat.snapshot().unwrap(), "Alpha") == 0.0 && diagnostic(&combat, "identifiedByOwnCast").is_none());
    combat.observe_own_cast(&req(p![253 => 24, 0 => 777i64, 4 => 3404, 8 => 9]), 14_000_000);
    combat.on(ev(p![252 => 18, 0 => 22, 1 => 30, 2 => 3404, 4 => 9, 5 => 777i64]), "flow", "echo", 14_200_000, Some(&local), Some(&roster));
    assert!(diagnostic(&combat, "identifiedByOwnCast") == Some(1) && member_healing(&combat.snapshot().unwrap(), "Alpha") == 80.0 + 450.0);
    assert_eq!(diagnostic(&combat, "pendingUnidentified"), Some(0));
}

#[test]
fn after_a_capture_restart_only_a_full_party_list_confirms_membership_and_earlier_effects_of_its_members_then_count() {
    let (local_id, old_id, leaver_id, mate_id) = (new_id(), new_id(), new_id(), new_id());
    let local = local(local_id, 10, "Alpha", "dungeon", 1, "flow");
    let mut roster = PartyRosterObservation::new();
    roster.observe(&ev(p![252 => 230, 8 => guid_bytes(&[local_id, old_id, leaver_id]), 9 => strings(&["Alpha", "Yesterday", "Leaver"])]), Some(&local), 1);
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    combat.forget_identities(1_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 29, 0 => 20, 1 => "Mate", 7 => mate_id], 2_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 6, 0 => 30, 6 => 20, 2 => -100f32], 3_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 234, 0 => 1, 1 => leaver_id], 4_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 6, 0 => 30, 6 => 20, 2 => -50f32], 5_000_000);
    let snapshot = combat.snapshot().unwrap();
    assert!(snapshot.total_damage == 0.0 && diagnostic(&combat, "awaitingRoster") == Some(2) && snapshot.members.iter().all(|m| m.name != "Yesterday"));
    both(&mut roster, &mut combat, &local, p![252 => 230, 8 => guid_bytes(&[local_id, mate_id]), 9 => strings(&["Alpha", "Mate"])], 6_000_000);
    assert!(member_damage(&combat.snapshot().unwrap(), "Mate") == 150.0 && diagnostic(&combat, "creditedAfterRoster") == Some(2));
    both(&mut roster, &mut combat, &local, p![252 => 235, 0 => 1, 1 => mate_id], 7_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 6, 0 => 30, 6 => 20, 2 => -70f32], 8_000_000);
    assert!(roster.snapshot().unwrap().members.iter().all(|m| m.id != mate_id) && combat.snapshot().unwrap().total_damage == 150.0);
    both(&mut roster, &mut combat, &local, p![252 => 6, 0 => 30, 6 => 20, 2 => -10f32], 8_500_000);
    let joined = LocalCharacterSnapshot { object_id: Some(80), observed_at_microseconds: 9_000_000, ..local.clone() };
    combat.on(resp(p![253 => 2, 0 => 80]), "flow", "join", 9_000_000, Some(&joined), roster.snapshot());
    let mut visits: Vec<String> = combat.sightings().into_iter().map(|s| s.visit).collect();
    visits.dedup();
    visits.sort();
    visits.dedup();
    assert_eq!(visits.len(), 2);
}

#[test]
fn a_kill_names_the_killers_object_by_character_and_by_name_and_never_overrides_an_object_already_named() {
    let (local_id, mei_id, neer_id) = (new_id(), new_id(), new_id());
    let local = local(local_id, 10, "Alpha", "dungeon", 1, "flow");
    let roster = roster(&[(local_id, "Alpha"), (mei_id, "MistressMei"), (neer_id, "neerkpz")], 1, true);
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    event_at(&mut combat, p![252 => 6, 0 => 30, 6 => 786, 2 => -120f32], 1_000, Some(&local), Some(&roster));
    event_at(&mut combat, p![252 => 6, 0 => 30, 6 => 412, 2 => -80f32], 2_000, Some(&local), Some(&roster));
    event_at(&mut combat, p![252 => 44, 0 => 1955, 3 => 786, 6 => mei_id, 15 => 43846], 3_000, Some(&local), Some(&roster));
    event_at(&mut combat, p![252 => 165, 0 => 412, 3 => "@MOB_T6_BOSS", 4 => 412, 5 => "neerkpz"], 4_000, Some(&local), Some(&roster));
    event_at(&mut combat, p![252 => 165, 0 => 786, 3 => "@MOB_T6_BOSS", 4 => 786, 5 => "neerkpz"], 5_000, Some(&local), Some(&roster));
    let snapshot = combat.snapshot().unwrap();
    assert!(member_damage(&snapshot, "MistressMei") == 120.0 && member_damage(&snapshot, "neerkpz") == 80.0);
    assert!(diagnostic(&combat, "identifiedByKill") == Some(2) && diagnostic(&combat, "identityConflict") == Some(1));
}

#[test]
fn inspecting_a_character_reports_its_equipment_which_then_names_its_party_object_by_identical_equipment() {
    let (local_id, mate_id) = (new_id(), new_id());
    let local = local(local_id, 10, "Alpha", "dungeon", 1, "flow");
    let roster = roster(&[(local_id, "Alpha"), (mate_id, "Mate")], 1, true);
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    let send = |combat: &mut CapturedCombatObservation, message: WireMessage, at: i64| combat.on(message, "flow", &format!("event:{at}"), at, Some(&local), Some(&roster));
    send(&mut combat, ev(p![252 => 670, 0 => 55]), 1_000);
    send(&mut combat, ev(p![252 => 6, 0 => 30, 6 => 55, 2 => -90f32]), 2_000);
    let types = [9038i16, 0, 5490, 3925, 4184, 0, 2510, 3029, 0, 0];
    for (slot, &kind) in types.iter().enumerate() {
        if kind > 0 {
            send(&mut combat, ev(p![252 => 30, 0 => 30170 + slot as i32, 1 => kind]), 3_000);
        }
    }
    let contents = objects(types.iter().enumerate().map(|(slot, &kind)| Value::Int(if kind > 0 { 30170 + slot as i32 } else { 0 })).collect());
    send(&mut combat, resp(p![253 => 143, 0 => mate_id, 1 => contents]), 3_100);
    send(&mut combat, ev(p![252 => 90, 0 => 55, 2 => types]), 4_000);
    assert!(diagnostic(&combat, "gearFromInspection") == Some(1) && member_damage(&combat.snapshot().unwrap(), "Mate") == 90.0);
}

#[test]
fn a_party_formed_by_an_invite_during_the_capture_starts_then_and_the_invited_players_earlier_effects_never_count() {
    let (local_id, friend_id) = (new_id(), new_id());
    let local = local(local_id, 10, "Alpha", "city", 1, "flow");
    let mut roster = PartyRosterObservation::new();
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    combat.forget_identities(1_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 29, 0 => 20, 1 => "Friend", 7 => friend_id], 2_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 6, 0 => 30, 6 => 20, 2 => -100f32], 2_500_000);
    combat.observe_own_cast(&req(p![253 => 225, 0 => friend_id]), 3_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 230, 0 => 13735, 8 => guid_bytes(&[local_id, friend_id]), 9 => strings(&["Alpha", "Friend"])], 3_500_000);
    both(&mut roster, &mut combat, &local, p![252 => 6, 0 => 30, 6 => 20, 2 => -40f32], 4_000_000);
    assert!(member_damage(&combat.snapshot().unwrap(), "Friend") == 40.0 && diagnostic(&combat, "outsideParty") == Some(1));
}

#[test]
fn leaving_a_party_and_rejoining_the_same_one_proves_the_party_before_the_leave_never_the_time_outside_it() {
    let (local_id, mate_id) = (new_id(), new_id());
    let local = local(local_id, 10, "Alpha", "dungeon", 1, "flow");
    let mut roster = PartyRosterObservation::new();
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    combat.forget_identities(1_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 29, 0 => 20, 1 => "Mate", 7 => mate_id], 2_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 6, 0 => 30, 6 => 20, 2 => -100f32], 3_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 234, 0 => 43846, 1 => local_id], 4_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 6, 0 => 30, 6 => 20, 2 => -60f32], 5_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 228, 0 => mate_id, 1 => "Mate", 7 => 43846], 6_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 230, 0 => 43846, 8 => guid_bytes(&[mate_id, local_id]), 9 => strings(&["Mate", "Alpha"])], 6_500_000);
    both(&mut roster, &mut combat, &local, p![252 => 6, 0 => 30, 6 => 20, 2 => -30f32], 7_000_000);
    combat.flush_unidentified();
    let snapshot = combat.snapshot().unwrap();
    let everyone: Vec<_> = snapshot.members.iter().chain(snapshot.former_members.iter().flatten()).filter(|m| m.name == "Mate").collect();
    assert!(everyone.len() == 1 && everyone[0].damage == 130.0);
}

#[test]
fn characters_announced_on_a_new_zones_connection_just_before_its_join_stay_known_in_that_zone() {
    let (local_id, mei_id) = (new_id(), new_id());
    let mut local = local(local_id, 30, "Alpha", "city", 1, "old");
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    combat.on(ev(p![252 => 29, 0 => 402, 1 => "Stranger", 7 => new_id()]), "old", "old-zone", 1_000_000, Some(&local), None);
    combat.on(ev(p![252 => 29, 0 => 402, 1 => "MistressMei", 7 => mei_id]), "island", "announce", 5_000_000, Some(&local), None);
    local = LocalCharacterSnapshot { object_id: Some(432), connection: c("island"), observed_at_microseconds: 5_300_000, ..local };
    combat.on(resp(p![253 => 2, 0 => 432]), "island", "join", 5_300_000, Some(&local), None);
    let roster = PartyRosterSnapshot::build(
        vec![PartyRosterMember::new(local_id, "Alpha"), PartyRosterMember::new(mei_id, "MistressMei")].into(),
        6_000_000,
        true,
        "capture".into(),
        Some(6_000_000),
        None,
    );
    combat.on(ev(p![252 => 6, 0 => 30, 6 => 402, 2 => -75f32]), "island", "hit", 7_000_000, Some(&local), Some(&roster));
    assert_eq!(member_damage(&combat.snapshot().unwrap(), "MistressMei"), 75.0);
    let mut visits: Vec<String> = combat.sightings().into_iter().filter(|s| s.character_id == mei_id).map(|s| s.visit).collect();
    visits.sort();
    visits.dedup();
    assert_eq!(visits.len(), 1);
}

#[test]
fn evidence_about_a_party_formed_later_never_makes_its_members_part_of_the_party_before_it() {
    let (local_id, mei_id) = (new_id(), new_id());
    let local = local(local_id, 10, "Alpha", "island", 1, "flow");
    let mut roster = PartyRosterObservation::new();
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    combat.forget_identities(1_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 29, 0 => 402, 1 => "MistressMei", 7 => mei_id], 2_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 6, 0 => 30, 6 => 402, 2 => -98f32], 3_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 234, 0 => 23102, 1 => local_id], 4_000_000);
    combat.observe_own_cast(&req(p![253 => 225, 0 => mei_id]), 5_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 230, 0 => 25271, 8 => guid_bytes(&[local_id, mei_id]), 9 => strings(&["Alpha", "MistressMei"])], 6_000_000);
    both(&mut roster, &mut combat, &local, p![252 => 243, 0 => 8, 1 => mei_id], 6_100_000);
    both(&mut roster, &mut combat, &local, p![252 => 6, 0 => 30, 6 => 402, 2 => -50f32], 7_000_000);
    combat.flush_unidentified();
    assert_eq!(member_damage(&combat.snapshot().unwrap(), "MistressMei"), 50.0);
}

#[test]
fn equipment_seen_before_identification_replaces_a_stale_weapon_and_skills_alone_never_do() {
    use meter_core::observation::combat::KnownCharacter;
    let (local_id, healer_id) = (new_id(), new_id());
    let roster = roster(&[(local_id, "Alpha"), (healer_id, "Healer")], 1, true);
    let mut combat = CapturedCombatObservation::new(None);
    let known = |name: &str, values: &[i32]| KnownCharacter { name: name.into(), equipment: Some(gear(values).into()), skills: None };
    combat.seed_characters(&[(local_id, known("Alpha", &[6662, 0, 3461, 4249, 5817, 2908, 2562, 3098])), (healer_id, known("Healer", &[6662, 0, 1, 2, 3, 4, 5, 6]))]);
    combat.begin_session(Some(1), None);
    combat.forget_identities(1_000_000);
    event_at(&mut combat, p![252 => 90, 0 => 396, 2 => [7587i16, 2329, 3682, 5372, 4636, 2908, 2562, 3098, 0, 0], 7 => [3398i16, 3409, 3420, 3925]], 2_000_000, None, Some(&roster));
    event_at(&mut combat, p![252 => 74, 0 => "alpha", 1 => "hi", 4 => 396, 5 => local_id], 2_100_000, None, Some(&roster));
    assert_eq!(combat.weapons().get(&local_id).copied(), Some(Some(7587)));
    event_at(&mut combat, p![252 => 74, 0 => "healer", 1 => "hi", 4 => 500, 5 => healer_id], 2_200_000, None, Some(&roster));
    assert_eq!(combat.weapons().get(&healer_id).copied(), Some(None));
    event_at(&mut combat, p![252 => 6, 0 => 30, 6 => 500, 2 => 120f32, 7 => 3409i16], 2_300_000, None, Some(&roster));
    assert!(combat.weapons().get(&healer_id).copied() == Some(None) && diagnostic(&combat, "weaponInferredFromSkills").is_none());
}

#[test]
fn party_equipment_identical_to_a_party_objects_equipment_names_a_member_already_nearby_and_similar_equipment_does_not() {
    let (local_id, member_id, other_id) = (new_id(), new_id(), new_id());
    let roster = roster(&[(local_id, "Alpha"), (member_id, "Beta"), (other_id, "Gamma")], 1, true);
    let local = local(local_id, 452, "Alpha", "one", 1, "flow");
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    event_at(&mut combat, p![252 => 6, 0 => 30, 6 => 552, 2 => -120f32], 1_000_000, Some(&local), Some(&roster));
    event_at(&mut combat, p![252 => 90, 0 => 552, 2 => [9317i16, 2403, 4299, 5447, 4203, 2908, 2712, 3098, 0, 0]], 1_100_000, Some(&local), Some(&roster));
    event_at(&mut combat, p![252 => 670, 0 => 552], 1_200_000, Some(&local), Some(&roster));
    assert_eq!(combat.snapshot().unwrap().total_damage, 0.0);
    event_at(&mut combat, p![252 => 244, 0 => other_id, 2 => [7114i16, 0, 5342, 4997, 5397, 2900, 2837, 3098, 0, 0]], 1_300_000, Some(&local), Some(&roster));
    event_at(&mut combat, p![252 => 244, 0 => member_id, 2 => [9317i16, 2403, 4299, 5447, 4203, 2908, 2541, 3098, 0, 0]], 1_400_000, Some(&local), Some(&roster));
    assert!(combat.snapshot().unwrap().total_damage == 0.0 && diagnostic(&combat, "identifiedByEquipment").is_none());
    event_at(&mut combat, p![252 => 244, 0 => member_id, 2 => [9317i16, 2403, 4299, 5447, 4203, 2908, 2712, 3098, 0, 0]], 1_500_000, Some(&local), Some(&roster));
    assert!(member_damage(&combat.snapshot().unwrap(), "Beta") == 120.0 && diagnostic(&combat, "identifiedByEquipment") == Some(1));
}

#[test]
fn a_party_object_event_never_names_the_local_player_because_its_id_field_belongs_to_another_member() {
    let (local_id, member_id) = (new_id(), new_id());
    let roster = roster(&[(local_id, "Alpha"), (member_id, "Beta")], 1, true);
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    // As captured after a reconnect whose Join was missed: the local object 104 comes with the other member's id.
    combat.on(ev(p![252 => 670, 0 => 104i64, 1 => member_id]), "flow", "party-object", 1_000_000, None, Some(&roster));
    combat.observe_own_cast(&req(p![253 => 24, 0 => 555i64, 4 => 3925, 8 => 1]), 2_000_000);
    combat.on(ev(p![252 => 18, 0 => 104i64, 1 => 30, 2 => 3925, 4 => 1, 5 => 555i64]), "flow", "echo", 2_050_000, None, Some(&roster));
    combat.on(ev(p![252 => 6, 0 => 30, 6 => 104i64, 2 => -70f32, 7 => 3925i16]), "flow", "hit", 2_100_000, None, Some(&roster));
    assert!(combat.snapshot().unwrap().members.iter().all(|m| m.name != "Beta" || m.damage == 0.0));
}

#[test]
fn after_a_capture_restart_the_local_object_is_confirmed_only_by_the_echo_of_an_own_cast_request() {
    let local_id = new_id();
    let local = local(local_id, 452, "Alpha", "one", 1, "flow");
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    combat.forget_identities(5_000_000);
    let hit = |combat: &mut CapturedCombatObservation, at: i64, spell: i16| {
        combat.on(ev(p![252 => 6, 0 => 30, 6 => 452, 2 => -70f32, 7 => spell]), "flow", &format!("hit:{at}"), at, Some(&local), None)
    };
    hit(&mut combat, 6_000_000, -1);
    hit(&mut combat, 7_000_000, -1);
    assert!(combat.snapshot().unwrap().members.iter().all(|m| m.damage == 0.0));
    combat.observe_own_cast(&req(p![253 => 24, 0 => 555i64, 4 => 3925, 8 => 1]), 8_000_000);
    hit(&mut combat, 8_300_000, 3925);
    assert!(combat.snapshot().unwrap().members.iter().all(|m| m.damage == 0.0));
    combat.on(ev(p![252 => 18, 0 => 452, 1 => 30, 2 => 3925, 4 => 1, 5 => 555i64]), "flow", "echo", 8_250_000, Some(&local), None);
    assert!(diagnostic(&combat, "identifiedByOwnCast") == Some(1) && member_damage(&combat.snapshot().unwrap(), "Alpha") == 210.0);
}

#[test]
fn unnamed_party_objects_are_kept_as_unidentified_at_a_zone_change_while_objects_the_party_list_never_named_are_dropped() {
    let local_id = new_id();
    let local = local(local_id, 1, "Alpha", "one", 1, "flow");
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    let effect = |combat: &mut CapturedCombatObservation, source: i64, target: i64, value: f32, at: i64| {
        combat.on(ev(p![252 => 6, 0 => target, 6 => source, 2 => value]), "flow", &format!("effect:{source}:{at}"), at, Some(&local), None)
    };
    for party_object in [40i64, 42] {
        combat.on(ev(p![252 => 670, 0 => party_object]), "flow", &format!("party:{party_object}"), 500_000, Some(&local), None);
    }
    effect(&mut combat, 40, 41, -200.0, 1_000_000);
    effect(&mut combat, 42, 40, 60.0, 1_500_000);
    effect(&mut combat, 50, 40, -900.0, 2_000_000);
    effect(&mut combat, 60, 60, 500.0, 2_100_000);
    effect(&mut combat, 1, 70, -400.0, 2_200_000);
    effect(&mut combat, 70, 71, 300.0, 2_300_000);
    combat.on(resp(p![253 => 2]), "flow", "zone", 3_000_000, Some(&local), None);
    let snapshot = combat.snapshot().unwrap();
    assert_eq!(member_damage(&snapshot, "Unidentified player (obj 40)"), 200.0);
    assert_eq!(member_healing(&snapshot, "Unidentified player (obj 42)"), 60.0);
    for name in ["Unidentified player (obj 50)", "Unidentified player (obj 60)", "Unidentified player (obj 70)", "Unidentified player (obj 71)"] {
        assert!(snapshot.members.iter().all(|m| m.name != name));
    }
    assert!(diagnostic(&combat, "unidentifiedPlayersKept") == Some(2) && diagnostic(&combat, "droppedOutsidePartyObjects").unwrap() >= 2);
    assert_eq!(member_damage(&snapshot, "Alpha"), 400.0);
}

#[test]
fn the_local_weapon_follows_equipped_item_objects_listed_by_each_join() {
    let id = new_id();
    let mut local = local(id, 71, "Alpha", "one", 10, "flow");
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    let join = |combat: &mut CapturedCombatObservation, local: &mut LocalCharacterSnapshot, slots: Vec<u8>, at: i64| {
        local.observed_at_microseconds = at;
        combat.on(WireMessage::new(3, 2, Some(0), p![253 => 2, 0 => 71, 52 => slots]), "flow", &format!("join:{at}"), at, Some(local), None);
    };
    let item = |combat: &mut CapturedCombatObservation, local: &LocalCharacterSnapshot, item: i64, kind: i32, at: i64| {
        combat.on(ev(p![252 => 30, 0 => item, 1 => kind]), "flow", &format!("item:{item}:{at}"), at, Some(local), None)
    };
    item(&mut combat, &local, 74, 6662, 9_000_000);
    join(&mut combat, &mut local, vec![74, 0, 72], 10_000_000);
    assert_eq!(combat.weapons().get(&id).copied(), Some(Some(6662)));
    join(&mut combat, &mut local, vec![74, 0, 72], 30_000_000);
    assert_eq!(combat.weapons().get(&id).copied(), Some(Some(6662)));
    item(&mut combat, &local, 74, 5697, 31_000_000);
    assert_eq!(combat.weapons().get(&id).copied(), Some(Some(5697)));
    combat.forget_identities(40_000_000);
    combat.on(ev(p![252 => 6, 0 => 30, 6 => 71, 2 => -10f32]), "flow", "stale-hit", 41_000_000, Some(&local), None);
    assert_eq!(diagnostic(&combat, "unresolvedActor"), Some(1));
}

#[test]
fn unidentified_nearby_actors_are_tracked_until_a_zone_change_announces_them_again() {
    let (id, other_id) = (new_id(), new_id());
    let local = local(id, 10, "Alpha", "one", 1, "flow");
    let roster = roster(&[(id, "Alpha"), (other_id, "Beta")], 1, true);
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    let hit = |combat: &mut CapturedCombatObservation, source: i32, at: i64| {
        combat.on(ev(p![252 => 6, 0 => 30, 6 => source, 2 => -50f32]), "flow", &format!("hit:{source}:{at}"), at, Some(&local), Some(&roster))
    };
    hit(&mut combat, 20, 2);
    hit(&mut combat, 20, 3);
    assert_eq!(diagnostic(&combat, "unresolvedSinceZoneChange"), Some(2));
    combat.on(resp(p![253 => 2]), "flow", "zone", 4, Some(&local), Some(&roster));
    assert!(diagnostic(&combat, "unresolvedSinceZoneChange").is_none() && diagnostic(&combat, "unresolvedActor") == Some(2));
    combat.on(ev(p![252 => 29, 0 => 20, 1 => "Beta", 7 => other_id]), "flow", "announce", 5, Some(&local), Some(&roster));
    hit(&mut combat, 20, 6);
    assert!(diagnostic(&combat, "attributedSinceZoneChange") == Some(1) && diagnostic(&combat, "unresolvedSinceZoneChange").is_none());
    assert_eq!(member_damage(&combat.snapshot().unwrap(), "Beta"), 50.0);
}

#[test]
fn record_controls_after_a_capture_gap_do_not_extend_the_active_dungeon_floor() {
    let id = new_id();
    let mut local = local(id, 10, "Alpha", "one", 1, "flow");
    let mut dungeon = DungeonCombatObservation::new(None);
    local.map_id = Some("@RANDOMDUNGEON@one".into());
    dungeon.on(resp(p![253 => 2, 1 => id, 8 => "@RANDOMDUNGEON@one", 66 => "1315"]), "flow", "join", 1_000_000, Some(&local), None);
    dungeon.on(ev(p![252 => 6, 0 => 30, 6 => 10, 2 => -100f32]), "flow", "hit", 3_000_000, Some(&local), None);
    dungeon.begin_record(600_000_000);
    assert_eq!(dungeon.snapshot()[0].floors[0].totals.elapsed_seconds, 2.0);
    dungeon.set_paused(true, 900_000_000);
    dungeon.set_paused(false, 4_000_000_000);
    assert_eq!(dungeon.snapshot()[0].floors[0].totals.elapsed_seconds, 2.0);
}

#[test]
fn observed_dungeon_joins_group_floors_revisits_and_new_instances_without_double_counting() {
    let id = Guid::parse("11111111-1111-1111-1111-111111111111").unwrap();
    let mut dungeon = DungeonCombatObservation::new(None);
    let mut local = local(id, 10, "Alpha", "1315", 0, "flow");
    let join = |dungeon: &mut DungeonCombatObservation, local: &mut LocalCharacterSnapshot, map: &str, entrance: &str, at: i64| {
        local.map_id = Some(map.into());
        local.observed_at_microseconds = at;
        dungeon.on(resp(p![253 => 2i16, 0 => 10, 1 => id, 2 => "Alpha", 8 => map, 66 => entrance]), "flow", &format!("join:{at}"), at, Some(local), None);
    };
    let hit = |dungeon: &mut DungeonCombatObservation, local: &LocalCharacterSnapshot, value: f32, at: i64| {
        dungeon.on(ev(p![252 => 6, 0 => 30, 2 => value, 6 => 10]), "flow", &format!("hit:{at}"), at, Some(local), None)
    };
    join(&mut dungeon, &mut local, "1315", "1301", 0);
    join(&mut dungeon, &mut local, "@RANDOMDUNGEON@first", "1315", 1_000_000);
    hit(&mut dungeon, &local, -100.0, 2_000_000);
    hit(&mut dungeon, &local, 20.0, 3_000_000);
    join(&mut dungeon, &mut local, "@RANDOMDUNGEON@second", "1315", 5_000_000);
    hit(&mut dungeon, &local, -300.0, 6_000_000);
    let initial = dungeon.snapshot().remove(0);
    assert!(initial.floors.len() == 2 && initial.entrance_map_id == "1315");
    assert!(initial.floors[0].totals.total_damage == 100.0 && initial.floors[0].totals.total_healing == 20.0);
    assert!(initial.floors[0].totals.elapsed_seconds == 4.0 && initial.floors[0].totals.members[0].encounter_dps == 25.0);
    assert!(initial.floors[1].totals.total_damage == 300.0 && initial.totals.total_damage == 400.0);
    assert_eq!(timeline_damage(&initial.totals), 400.0);
    assert_eq!(timeline_healing(&initial.totals), 20.0);
    assert_eq!(dungeon.snapshot()[0].totals.total_damage, 400.0);
    join(&mut dungeon, &mut local, "@RANDOMDUNGEON@second", "1315", 7_000_000);
    assert_eq!(dungeon.snapshot()[0].floors[1].encounters.len(), 1);
    join(&mut dungeon, &mut local, "@RANDOMDUNGEON@first", "1315", 8_000_000);
    hit(&mut dungeon, &local, -50.0, 9_000_000);
    let revisited = dungeon.snapshot().remove(0);
    assert!(revisited.floors.len() == 2 && revisited.floors[0].encounters.len() == 2);
    assert!(revisited.floors[0].totals.total_damage == 150.0 && revisited.totals.total_damage == 450.0);
    dungeon.begin_record(10_000_000);
    hit(&mut dungeon, &local, -25.0, 11_000_000);
    assert_eq!(dungeon.snapshot()[0].totals.total_damage, 475.0);
    join(&mut dungeon, &mut local, "1315", "1301", 12_000_000);
    hit(&mut dungeon, &local, -900.0, 13_000_000);
    let runs = dungeon.snapshot();
    assert!(runs.len() == 1 && runs[0].status == "completed" && runs[0].totals.total_damage == 475.0);
    join(&mut dungeon, &mut local, "@RANDOMDUNGEON@third", "1315", 14_000_000);
    hit(&mut dungeon, &local, -70.0, 15_000_000);
    let runs = dungeon.snapshot();
    assert!(runs.len() == 2 && runs[1].totals.total_damage == 70.0);
    join(&mut dungeon, &mut local, "@RANDOMDUNGEON@first", "1315", 16_000_000);
    let runs = dungeon.snapshot();
    assert!(runs.len() == 2 && runs[0].status == "running");
    dungeon.forget_identities(100_000_000);
    hit(&mut dungeon, &local, -1000.0, 101_000_000);
    let runs = dungeon.snapshot();
    assert!(runs.iter().map(|r| r.totals.total_damage).sum::<f64>() == 545.0 && runs.iter().all(|r| r.status == "completed"));
    join(&mut dungeon, &mut local, "@RANDOMDUNGEON@third", "1315", 102_000_000);
    hit(&mut dungeon, &local, -5.0, 103_000_000);
    let runs = dungeon.snapshot();
    assert!(runs.len() == 2 && runs[1].totals.total_damage == 75.0);
    let mut replayed = DungeonCombatObservation::new(None);
    local.map_id = Some("@RANDOMDUNGEON@first".into());
    replayed.on(resp(p![253 => 2, 0 => 10, 1 => id, 2 => "Alpha", 8 => "@RANDOMDUNGEON@first", 66 => "1315"]), "flow", "replay", 1_000_000, Some(&local), None);
    let replay = replayed.snapshot().remove(0);
    assert_eq!(replay.id, initial.id);
    assert_eq!(replay.floors[0].encounters[0].id, initial.floors[0].encounters[0].id);
}

#[test]
fn a_new_record_clears_damage_healing_timeline_and_former_members_while_retaining_current_equipment() {
    let (local_id, other_id) = (new_id(), new_id());
    let local = local(local_id, 10, "Alpha", "one", 1, "flow");
    let mut roster = PartyRosterObservation::new();
    roster.observe(&ev(p![252 => 230, 8 => guid_bytes(&[local_id, other_id]), 9 => strings(&["Alpha", "Beta"])]), Some(&local), 1);
    let mut combat = CapturedCombatObservation::new(None);
    combat.on(ev(p![252 => 29, 0 => 20, 1 => "Beta", 7 => other_id]), "flow", "entity", 2, Some(&local), roster.snapshot());
    combat.on(ev(p![252 => 90, 0 => 10, 2 => [9022i16]]), "flow", "equipment", 3, Some(&local), roster.snapshot());
    combat.on(ev(p![252 => 7, 0 => 30, 2 => floats(&[-100.0, 40.0, -200.0]), 6 => [10i16, 10, 20]]), "flow", "hits", 4, Some(&local), roster.snapshot());
    roster.observe(&ev(p![252 => 234, 1 => other_id]), Some(&local), 5);
    combat.set_party(roster.snapshot());
    let previous = combat.snapshot().unwrap();
    assert!(previous.total_damage == 300.0 && previous.total_healing == 40.0 && previous.former_members.as_ref().unwrap().len() == 1);
    combat.begin_session(Some(6_000_000), None);
    let current = combat.snapshot().unwrap();
    assert!(current.id != previous.id && current.party_id != previous.party_id);
    assert!(current.total_damage == 0.0 && current.total_healing == 0.0 && current.event_count == 0 && current.elapsed_seconds == 0.0);
    assert!(current.members.len() == 1 && current.members[0].name == "Alpha");
    assert!(current.members[0].weapon == "Item #9022" && current.former_members.as_ref().unwrap().is_empty());
    assert!(current.weapon_contributions.as_ref().unwrap().is_empty() && current.item_contributions.as_ref().unwrap().is_empty());
    assert!(timeline_damage(&current) == 0.0 && timeline_healing(&current) == 0.0);
    assert!(current.members.iter().all(|m| m.encounter_dps == 0.0 && m.rolling_dps == 0.0 && m.encounter_hps == 0.0 && m.rolling_hps == 0.0));
    combat.on(ev(p![252 => 6, 0 => 30, 6 => 10, 2 => -25f32]), "flow", "new-hit", 7_000_000, Some(&local), roster.snapshot());
    assert!(combat.snapshot().unwrap().total_damage == 25.0 && previous.total_damage == 300.0 && previous.total_healing == 40.0);
}

#[test]
fn a_newer_solo_confirmation_replaces_recovered_membership_and_rejoining_restores_prior_damage() {
    let (local_id, other_id) = (new_id(), new_id());
    let local = local(local_id, 10, "Alpha", "one", 1, "flow");
    let mut roster = PartyRosterObservation::new();
    let joined = ev(p![252 => 230, 8 => guid_bytes(&[local_id, other_id]), 9 => strings(&["Alpha", "Beta"])]);
    roster.observe(&joined, Some(&local), 1);
    let mut combat = CapturedCombatObservation::new(None);
    combat.on(ev(p![252 => 29, 0 => 20, 1 => "Beta", 7 => other_id]), "flow", "entity", 2, Some(&local), roster.snapshot());
    let hit = |combat: &mut CapturedCombatObservation, roster: &PartyRosterObservation, identity: &str, at: i64, amount: f32| {
        combat.on(ev(p![252 => 6, 0 => 30, 6 => 20, 2 => -amount]), "flow", identity, at, Some(&local), roster.snapshot())
    };
    hit(&mut combat, &roster, "before", 3, 100.0);
    assert!(roster.restore(PartyRosterSnapshot::build(Arc::from([]), 4, true, "user-confirmed".into(), None, None)));
    combat.set_party(roster.snapshot());
    assert!(combat.snapshot().unwrap().members.iter().all(|m| m.name != "Beta"));
    assert_eq!(combat.snapshot().unwrap().former_members.unwrap()[0].damage, 100.0);
    assert!(!roster.restore(common::roster(&[(local_id, "Alpha"), (other_id, "Beta")], 1, true)));
    hit(&mut combat, &roster, "outside", 5, 200.0);
    assert_eq!(combat.snapshot().unwrap().total_damage, 100.0);
    roster.observe(&joined, Some(&local), 6);
    assert_eq!(roster.snapshot().unwrap().source, "capture");
    hit(&mut combat, &roster, "returned", 7, 50.0);
    assert_eq!(member_damage(&combat.snapshot().unwrap(), "Beta"), 150.0);
    assert!(combat.snapshot().unwrap().former_members.unwrap().is_empty());
    let previous = roster.snapshot().unwrap().clone();
    assert!(!roster.restore(common::roster(&[(other_id, "Beta")], 8, true)));
    assert!(!roster.restore(common::roster(&[(local_id, "Alpha"), (local_id, "Duplicate")], 8, true)));
    assert!(roster.snapshot().unwrap().same(&previous));
}

#[test]
fn party_damage_survives_a_capture_boundary_and_zoning_without_another_party_join() {
    let (local_id, other_id) = (new_id(), new_id());
    let mut local = local(local_id, 10, "Alpha", "one", 1, "first");
    let mut party = PartyRosterObservation::new();
    party.observe(&ev(p![252 => 230, 8 => guid_bytes(&[local_id, other_id]), 9 => strings(&["Alpha", "Beta"])]), Some(&local), 1);
    let mut combat = CapturedCombatObservation::new(None);
    combat.on(ev(p![252 => 29, 0 => 20, 1 => "Beta", 7 => other_id]), "first", "initial", 2, Some(&local), party.snapshot());
    combat.begin_session(Some(3), None);
    local = LocalCharacterSnapshot { object_id: Some(99), map_id: Some("two".into()), connection: c("second"), ..local };
    combat.on(ev(p![252 => 29, 0 => 80, 1 => "Beta", 7 => other_id]), "second", "new-object", 4, Some(&local), party.snapshot());
    combat.on(ev(p![252 => 6, 0 => 99, 6 => 80, 2 => -100f32]), "second", "hit", 5, Some(&local), party.snapshot());
    assert_eq!(member_damage(&combat.snapshot().unwrap(), "Beta"), 100.0);
    party.observe(&ev(p![252 => 234, 1 => other_id]), Some(&local), 6);
    combat.on(ev(p![252 => 6, 0 => 99, 6 => 80, 2 => -50f32]), "second", "outside", 6, Some(&local), party.snapshot());
    assert_eq!(combat.snapshot().unwrap().former_members.unwrap().iter().find(|m| m.name == "Beta").unwrap().damage, 100.0);
    party.observe(&ev(p![252 => 232, 1 => other_id, 2 => "Beta"]), Some(&local), 7);
    combat.on(ev(p![252 => 6, 0 => 99, 6 => 80, 2 => -25f32]), "second", "returned", 7, Some(&local), party.snapshot());
    assert_eq!(member_damage(&combat.snapshot().unwrap(), "Beta"), 125.0);
}

#[test]
fn entities_observed_before_join_survive_confirmation_on_the_same_connection() {
    let id = new_id();
    let local = local(id, 10, "Alpha", "map", 1, "next");
    let mut observation = CapturedCombatObservation::new(None);
    observation.on(ev(p![252 => 123, 0 => 30, 1 => 424]), "next", "spawn", 1, None, None);
    observation.on(ev(p![252 => 90, 0 => 10, 2 => [9022i16]]), "next", "equipment", 2, None, None);
    observation.on(ev(p![252 => 6, 0 => 30, 2 => -310f32, 6 => 10]), "next", "hit", 3, Some(&local), None);
    assert!(diagnostic(&observation, "damageTo:mob") == Some(1) && observation.weapons().get(&id).copied() == Some(Some(9022)));
}

#[test]
fn captured_combat_counts_batches_once_and_isolates_unresolved_actors() {
    let (id, other) = (new_id(), new_id());
    let local = local(id, 10, "Alpha", "map", 1, "flow");
    let party = roster(&[(id, "Alpha"), (other, "Beta")], 1, true);
    let mut observation = CapturedCombatObservation::new(None);
    observation.on(ev(p![252 => 29, 0 => 20, 1 => "Beta", 7 => other, 40 => [7108i16]]), "flow", "spawn", 1, Some(&local), Some(&party));
    let hit = ev(p![252 => 7, 0 => 10, 2 => objects(vec![(-98f32).into(), (-77f32).into()]), 6 => longs(&[20, 20])]);
    observation.observe(&hit, &c("flow"), &"hit".into(), 2, Some(&local), Some(&party));
    observation.observe(&hit, &c("flow"), &"hit".into(), 2, Some(&local), Some(&party));
    let snapshot = observation.snapshot().unwrap();
    assert!(snapshot.event_count == 2 && snapshot.total_damage == 175.0);
    assert_eq!(observation.weapons().get(&other).copied(), Some(Some(7108)));
    observation.on(ev(p![252 => 90, 0 => 20i64, 2 => ints(&[9022])]), "flow", "equipment", 3, Some(&local), Some(&party));
    assert_eq!(observation.weapons().get(&other).copied(), Some(Some(9022)));
    observation.observe(&hit, &c("foreign"), &"wrong-flow".into(), 4, Some(&local), Some(&party));
    assert_eq!(observation.snapshot().unwrap().total_damage, 175.0);
    observation.begin_session(None, None);
    assert!(observation.snapshot().is_none());
    observation.observe(&hit, &c("flow"), &"next".into(), 5, Some(&local), Some(&party));
    assert_eq!(observation.snapshot().unwrap().total_damage, 175.0);
}

#[test]
fn captured_damage_keeps_raw_overkill_and_recognizes_mob_targets() {
    let id = new_id();
    let local = local(id, 10, "Alpha", "map", 1, "flow");
    let mut observation = CapturedCombatObservation::new(None);
    observation.on(ev(p![252 => 123, 0 => 30, 1 => 424]), "flow", "mob", 1, Some(&local), None);
    observation.on(ev(p![252 => 6, 0 => 30, 2 => -310f32, 6 => 10i64]), "flow", "hit", 2, Some(&local), None);
    assert!(observation.snapshot().unwrap().total_damage == 310.0 && diagnostic(&observation, "damageTo:mob") == Some(1));
    observation.on(ev(p![252 => 6, 0 => 30, 6 => 10i64]), "flow", "missing", 3, Some(&local), None);
    assert_eq!(observation.snapshot().unwrap().total_damage, 310.0);
    let next = LocalCharacterSnapshot { connection: c("next"), object_id: Some(99), ..local.clone() };
    observation.on(ev(p![252 => 6, 0 => 30, 2 => -50f32, 6 => 10i64]), "next", "stale-object", 4, Some(&next), None);
    assert_eq!(observation.snapshot().unwrap().total_damage, 310.0);
}
