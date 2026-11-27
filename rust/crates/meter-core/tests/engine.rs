//! Core checks of tests/AlbionMeter.Tests/Program.cs: the combat engine, dungeon runs, record controls.

mod common;

use common::*;
use meter_core::catalog::{CombatItemDefinition, CombatSourceData};
use meter_core::dungeon::{DungeonError, DungeonRun};
use meter_core::engine::{CombatEvent, CombatMetric::*, EngineError, PartyMember};
use meter_core::observation::dungeon::kind;
use meter_core::record_controls::{RecordControl, RecordControls};
use meter_core::{CaptureAnalysis, CombatEngine, CombatSourceCatalog};

fn party() -> Vec<PartyMember> {
    vec![PartyMember::new("a", "Alpha", "Sword", "sword", "#ffffff"), PartyMember::new("b", "Beta", "Staff", "holystaff", "#eeeeee")]
}

fn engine() -> CombatEngine {
    CombatEngine::new(party(), None, None).unwrap()
}

fn hit(id: &str, actor: &str, time: f64, amount: f64) -> CombatEvent {
    CombatEvent::new(id, actor, time, Damage, amount)
}

fn equal(expected: f64, actual: f64) {
    assert!((expected - actual).abs() <= 0.00001, "expected {expected}, received {actual}");
}

#[test]
fn dungeon_aggregates_preserve_source_items_equipment_histories_and_immutable_timelines() {
    let mut run = DungeonRun::new("Dungeon", "1315", "Entrance", "live", None, None);
    let first = run.enter_floor("one", "one", "Floor one").unwrap();
    let mut e = engine();
    e.advance(2.0).unwrap();
    e.add(&hit("one", "a", 1.0, 100.0).weapon(9022).source(9022, "mainhand", "autoAttack"));
    e.add(&CombatEvent::new("two", "b", 1.5, Healing, 40.0).weapon(7108).source(7108, "mainhand", "ability"));
    let mut snapshot = e.snapshot(true, "running");
    snapshot.location = Some(first);
    run.record("one", snapshot).unwrap();
    let second = run.enter_floor("two", "two", "Floor two").unwrap();
    let mut next = engine();
    next.advance(3.0).unwrap();
    next.add(&hit("three", "a", 1.0, 300.0).weapon(9022).source(4308, "chest", "ability"));
    let mut snapshot = next.snapshot(true, "running");
    snapshot.location = Some(second);
    run.record("two", snapshot).unwrap();
    let snapshot = run.snapshot("running");
    equal(400.0, snapshot.totals.total_damage);
    equal(40.0, snapshot.totals.total_healing);
    equal(5.0, snapshot.totals.elapsed_seconds);
    equal(400.0, timeline_damage(&snapshot.totals));
    let items = |f: fn(&meter_core::engine::TimelinePoint) -> f64| snapshot.totals.timeline.iter().map(|p| f(p)).sum::<f64>();
    equal(400.0, items(|p| p.item_damage.as_ref().unwrap().values().sum()));
    equal(40.0, items(|p| p.item_healing.as_ref().unwrap().values().sum()));
    let weapons = snapshot.totals.weapon_contributions.as_ref().unwrap();
    equal(400.0, weapons.iter().find(|w| w.player_id == "a").unwrap().damage);
    let contributions = snapshot.totals.item_contributions.as_ref().unwrap();
    equal(100.0, contributions.iter().find(|i| i.slot == "mainhand" && i.player_id == "a").unwrap().damage);
    equal(300.0, contributions.iter().find(|i| i.slot == "chest").unwrap().damage);
    equal(80.0, snapshot.totals.members.iter().find(|m| m.id == "a").unwrap().encounter_dps);
    // Snapshots are immutable: changing one never reaches the run.
    let mut copy = snapshot.floors[0].encounters[0].timeline[1].clone();
    std::sync::Arc::make_mut(&mut copy).item_damage.as_mut().unwrap().clear();
    let again = run.snapshot("running");
    equal(400.0, again.totals.timeline.iter().map(|p| p.item_damage.as_ref().unwrap().values().sum::<f64>()).sum());
}

#[test]
fn catalog_finds_exact_cape_tier_and_rejects_unknown_identifiers() {
    let source = CombatSourceCatalog::new(CombatSourceData {
        version: "test".into(),
        items: vec![CombatItemDefinition::new(2637, "T4_CAPEITEM_FW_THETFORD@3", "Thetford Cape", "cape", Some(4), 3)],
        spells: vec![],
    });
    let cape = source.find_item(2637).unwrap();
    assert!(cape.tier == Some(4) && cape.enchantment == 3 && cape.slot == "cape");
    assert!(source.find_item(99999).is_none() && !source.contains_item("../unknown"));
}

#[test]
fn record_controls_persist_pause_boundaries_and_recover_them_in_chronological_order() {
    let directory = std::env::temp_dir().join(format!("albion-record-controls-{}", Guid::new_v4().simple()));
    let controls = RecordControls::new(&directory);
    controls.append(&RecordControl::new(3_000_000, "resume", None)).unwrap();
    controls.append(&RecordControl::new(1_000_000, "pause", None)).unwrap();
    use std::io::Write;
    std::fs::OpenOptions::new().append(true).open(directory.join("record-controls.jsonl")).unwrap().write_all(b"invalid\n").unwrap();
    let recovered = RecordControls::new(&directory).read().unwrap();
    assert!(recovered.len() == 2 && recovered[0].action == "pause" && recovered[1].action == "resume");
    let mut analysis = CaptureAnalysis::new(None, false);
    RecordControls::apply(&mut analysis, &recovered[0]);
    assert!(analysis.snapshot().record_paused);
    RecordControls::apply(&mut analysis, &recovered[1]);
    assert!(!analysis.snapshot().record_paused);
    RecordControls::apply(&mut analysis, &RecordControl::new(4_000_000, "pause", Some(Guid::new_v4())));
    assert!(!analysis.snapshot().record_paused);
    RecordControls::apply(&mut analysis, &RecordControl::new(4_000_000, "pause", None));
    analysis.begin_capture(Some(5_000_000), false, false);
    assert!(analysis.snapshot().record_paused);
    RecordControls::apply(&mut analysis, &RecordControl::new(6_000_000, "new-record", None));
    assert!(!analysis.snapshot().record_paused);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn an_empty_encounter_has_finite_zero_rates() {
    let snapshot = engine().snapshot(true, "running");
    equal(0.0, snapshot.members[0].encounter_dps);
    equal(0.0, snapshot.members[0].rolling_dps);
}

#[test]
fn encounter_and_rolling_rates_use_distinct_durations() {
    let mut e = engine();
    e.advance(30.0).unwrap();
    e.add(&hit("old", "a", 10.0, 42000.0));
    e.add(&hit("new", "a", 28.0, 18000.0));
    let member = e.snapshot(true, "running").members[0].clone();
    equal(2000.0, member.encounter_dps);
    equal(3600.0, member.rolling_dps);
}

#[test]
fn distinct_identical_hits_are_both_counted() {
    let mut e = engine();
    e.advance(1.0).unwrap();
    e.add(&hit("one", "a", 0.5, 50.0));
    e.add(&hit("two", "a", 0.5, 50.0));
    equal(100.0, e.snapshot(true, "running").total_damage);
}

#[test]
fn a_repeated_transport_identity_is_counted_once() {
    let mut e = engine();
    e.advance(1.0).unwrap();
    assert!(e.add(&hit("one", "a", 0.5, 50.0)));
    assert!(!e.add(&hit("one", "a", 0.5, 50.0)));
    equal(50.0, e.snapshot(true, "running").total_damage);
}

#[test]
fn buckets_use_half_open_second_boundaries() {
    let mut e = engine();
    e.advance(2.0).unwrap();
    e.add(&hit("one", "a", 0.999, 70.0));
    e.add(&hit("two", "a", 1.0, 30.0));
    let timeline = e.snapshot(true, "running").timeline;
    equal(70.0, timeline[0].damage.get("a").copied().unwrap());
    equal(30.0, timeline[1].damage.get("a").copied().unwrap());
}

#[test]
fn valid_idle_observation_produces_zero_buckets() {
    let mut e = engine();
    e.advance(5.0).unwrap();
    assert_eq!(e.snapshot(true, "running").timeline.len(), 6);
    equal(0.0, *e.snapshot(true, "running").timeline[3].damage.get("a").unwrap());
}

#[test]
fn healing_and_damage_stay_separate() {
    let mut e = engine();
    e.advance(2.0).unwrap();
    e.add(&hit("one", "a", 1.0, 100.0));
    e.add(&CombatEvent::new("two", "b", 1.0, Healing, 80.0));
    let snapshot = e.snapshot(true, "running");
    equal(100.0, snapshot.total_damage);
    equal(80.0, snapshot.total_healing);
    equal(40.0, snapshot.members[1].encounter_hps);
    equal(0.0, snapshot.members[1].damage);
}

#[test]
fn every_players_damage_reconciles_with_the_timeline() {
    let mut e = engine();
    e.advance(60.0).unwrap();
    for i in 0..600 {
        e.add(&hit(&i.to_string(), if i % 2 == 0 { "a" } else { "b" }, f64::from(i) / 10.0, f64::from(i + 1)));
    }
    let snapshot = e.snapshot(true, "running");
    for member in &snapshot.members {
        equal(member.damage, snapshot.timeline.iter().map(|p| *p.damage.get(member.id.as_str()).unwrap()).sum());
    }
    equal(snapshot.total_damage, snapshot.members.iter().map(|m| m.damage).sum());
}

#[test]
fn rolling_rates_decay_without_new_combat_events() {
    let mut e = engine();
    e.advance(1.0).unwrap();
    e.add(&hit("one", "a", 0.5, 100.0));
    e.advance(6.0).unwrap();
    equal(0.0, e.rolling_rate("a", Damage, 5.0));
}

#[test]
fn late_events_revise_their_original_bucket() {
    let mut e = engine();
    e.advance(10.0).unwrap();
    e.snapshot(false, "running");
    e.add(&hit("late", "a", 2.5, 100.0));
    let delta = e.snapshot(false, "running");
    assert_eq!(delta.timeline.len(), 1);
    assert_eq!(delta.timeline[0].second, 2);
    equal(100.0, *delta.timeline[0].damage.get("a").unwrap());
}

#[test]
fn a_full_snapshot_does_not_consume_pending_changes() {
    let mut e = engine();
    e.snapshot(false, "running");
    e.advance(1.0).unwrap();
    e.add(&hit("one", "a", 1.0, 10.0));
    e.snapshot(true, "running");
    assert!(!e.snapshot(false, "running").timeline.is_empty());
    assert!(e.snapshot(false, "running").timeline.is_empty());
}

#[test]
fn snapshot_dictionaries_cannot_mutate_engine_state() {
    let mut e = engine();
    let snapshot = e.snapshot(true, "running");
    let mut point = snapshot.timeline[0].clone();
    std::sync::Arc::make_mut(&mut point).damage.insert("a".into(), 999.0);
    equal(0.0, *e.snapshot(true, "running").timeline[0].damage.get("a").unwrap());
}

#[test]
fn unknown_players_and_invalid_events_are_rejected() {
    let mut e = engine();
    e.advance(1.0).unwrap();
    assert!(!e.add(&hit("unknown", "x", 0.0, 10.0)));
    assert!(!e.add(&hit("future", "a", 2.0, 10.0)));
    assert!(!e.add(&hit("negative", "a", -1.0, 10.0)));
    assert!(!e.add(&hit("nan", "a", 0.0, f64::NAN)));
    assert!(!e.add(&hit("infinity", "a", 0.0, f64::INFINITY)));
    assert!(!e.add(&hit("zero", "a", 0.0, 0.0)));
    // `(CombatMetric)99` cannot be built in Rust: the enum is closed.
    assert_eq!(e.event_count(), 0);
}

#[test]
fn rates_are_not_artificially_capped() {
    let mut e = engine();
    e.advance(1.0).unwrap();
    e.add(&hit("burst", "a", 0.5, 100000.0));
    equal(100000.0, e.snapshot(true, "running").members[0].encounter_dps);
}

#[test]
fn shares_use_the_same_party_denominator() {
    let mut e = engine();
    e.advance(1.0).unwrap();
    e.add(&hit("one", "a", 0.5, 30.0));
    e.add(&hit("two", "b", 0.5, 70.0));
    equal(30.0, e.snapshot(true, "running").members[0].damage_share);
    equal(70.0, e.snapshot(true, "running").members[1].damage_share);
}

#[test]
fn early_rolling_rates_use_observed_duration() {
    let mut e = engine();
    e.advance(2.0).unwrap();
    e.add(&hit("one", "a", 1.0, 100.0));
    equal(50.0, e.rolling_rate("a", Damage, 5.0));
}

#[test]
fn backward_and_excessive_clock_advances_are_rejected() {
    let mut e = engine();
    e.advance(2.0).unwrap();
    for time in [1.0, 7201.0, f64::NAN] {
        assert_eq!(e.advance(time), Err(EngineError::InvalidTime));
    }
}

#[test]
fn separate_encounters_never_share_totals() {
    let mut first = engine();
    first.advance(1.0).unwrap();
    first.add(&hit("one", "a", 0.0, 100.0));
    let mut second = engine();
    equal(0.0, second.snapshot(true, "running").total_damage);
    assert_ne!(first.id(), second.id());
}

#[test]
fn random_dungeons_and_expeditions_hardcore_included_are_recorded_as_dungeon_runs() {
    assert_eq!(kind(Some("@RANDOMDUNGEON@6d8f0b2a-1c3e-4f5a-9b7d-2e4c6a8f0b1d")), Some("Random dungeon"));
    assert_eq!(kind(Some("@EXPEDITION@88714a12-4a45-4081-80ec-4f48e5b76565")), Some("Expedition"));
    assert_eq!(kind(Some("1316")), None);
    assert_eq!(kind(Some("@ISLAND@1f2e3d4c")), None);
}

#[test]
fn every_expedition_entry_is_its_own_run_named_by_its_template() {
    let character = new_id();
    let mut observation = DungeonCombatObservation::new(None);
    let mut timestamp = 1_000_000i64;
    let mut enter = |observation: &mut DungeonCombatObservation, map: &str, template: &str| {
        let outside = LocalCharacterSnapshot::new(character, Some(1), "KPey", Some("1301"), "observed", timestamp, "zone");
        timestamp += 1_000_000;
        observation.on(resp(p![0 => map, 1 => template, 253 => 41]), "zone", "zone:1", timestamp, Some(&outside), None);
        timestamp += 1_000_000;
        let inside = LocalCharacterSnapshot { map_id: Some(map.into()), observed_at_microseconds: timestamp, ..outside };
        observation.on(resp(p![1 => character, 8 => map, 66 => "1301", 253 => 2]), "zone", "zone:2", timestamp, Some(&inside), None);
    };
    enter(&mut observation, "@EXPEDITION@84fb7dda-9fab-4aa3-a98d-05252b177b0c", "Mine-SOLO");
    enter(&mut observation, "@EXPEDITION@1a2b3c4d-9fab-4aa3-a98d-05252b177b0c", "Mine-SOLO");
    let runs = observation.snapshot();
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|run| run.name == "Expedition" && run.floors.len() == 1 && run.floors[0].map_name == "Mine-SOLO"));
}

#[test]
fn dungeon_checkpoints_replace_totals_and_revisits_preserve_floor_identity() {
    let mut run = DungeonRun::new("Test dungeon", "1207", "Birchcopse", "live", None, None);
    let first = run.enter_floor("cluster-a", "random-a", "Random dungeon").unwrap();
    let mut e = engine();
    e.advance(10.0).unwrap();
    e.add(&hit("hit", "a", 1.0, 100.0));
    run.record(&first.floor_id, e.snapshot(true, "running")).unwrap();
    run.record(&first.floor_id, e.snapshot(true, "running")).unwrap();
    let second = run.enter_floor("cluster-b", "random-b", "Random dungeon").unwrap();
    let mut other = engine();
    other.advance(30.0).unwrap();
    other.add(&hit("hit", "a", 2.0, 300.0));
    run.record(&second.floor_id, other.snapshot(true, "running")).unwrap();
    assert_eq!(run.enter_floor("cluster-a", "random-a", "Random dungeon").unwrap().floor_number, 1);
    let snapshot = run.snapshot("completed");
    assert_eq!(snapshot.floors.len(), 2);
    equal(400.0, snapshot.totals.total_damage);
    equal(10.0, snapshot.totals.members[0].encounter_dps);
    equal(100.0, snapshot.floors[0].totals.total_damage);
    assert_ne!(DungeonRun::new("Test dungeon", "1207", "Birchcopse", "live", None, None).id(), run.id());
    assert_eq!(run.record(&second.floor_id, e.snapshot(true, "running")), Err(DungeonError::TwoFloors));
    assert_eq!(run.record(&first.floor_id, e.snapshot(false, "running")), Err(DungeonError::NotFull));
}

#[test]
fn leaving_removes_active_membership_while_preserving_party_contributions() {
    let mut e = CombatEngine::new(party(), Some("party-one".into()), None).unwrap();
    e.advance(2.0).unwrap();
    e.add(&hit("damage", "a", 1.0, 100.0));
    e.add(&CombatEvent::new("healing", "a", 1.5, Healing, 40.0));
    e.set_membership("a", false).unwrap();
    let departed = e.snapshot(true, "running");
    assert!(departed.members.iter().all(|m| m.id != "a"));
    equal(100.0, departed.former_members.as_ref().unwrap()[0].damage);
    equal(40.0, departed.former_members.as_ref().unwrap()[0].healing);
    equal(100.0, departed.total_damage);
    e.advance(4.0).unwrap();
    assert!(!e.add(&hit("outside", "a", 3.0, 900.0)));
    e.set_membership("a", true).unwrap();
    let returned = e.snapshot(true, "running");
    assert_eq!(returned.party_id.as_deref(), Some("party-one"));
    equal(100.0, returned.members.iter().find(|m| m.id == "a").unwrap().damage);
    assert!(returned.former_members.as_ref().unwrap().is_empty());
    e.advance(5.0).unwrap();
    e.add(&hit("after", "a", 4.5, 25.0));
    equal(125.0, e.snapshot(true, "running").total_damage);
    let mut another = CombatEngine::new(party(), Some("party-two".into()), None).unwrap();
    equal(0.0, another.snapshot(true, "running").total_damage);
    assert_ne!(another.party_id(), e.party_id());
}

#[test]
fn late_contributions_use_membership_at_event_time_rather_than_arrival_time() {
    let mut e = engine();
    e.advance(2.0).unwrap();
    e.set_membership("a", false).unwrap();
    e.advance(4.0).unwrap();
    assert!(e.add(&hit("late-before-leave", "a", 1.0, 75.0)));
    assert!(!e.add(&hit("at-leave", "a", 2.0, 100.0)));
    e.set_membership("a", true).unwrap();
    e.advance(5.0).unwrap();
    assert!(!e.add(&hit("late-during-absence", "a", 3.0, 100.0)));
    assert!(e.add(&hit("at-return", "a", 4.0, 25.0)));
    equal(100.0, e.snapshot(true, "running").total_damage);
}

#[test]
fn dungeon_totals_retain_former_participants() {
    let mut e = engine();
    e.advance(2.0).unwrap();
    e.add(&hit("hit", "a", 1.0, 150.0));
    e.set_membership("a", false).unwrap();
    let snapshot = e.snapshot(true, "running");
    let mut run = DungeonRun::new("Dungeon", "1207", "Birchcopse", "live", None, None);
    run.enter_floor("floor-one", "random", "Random dungeon").unwrap();
    run.record("floor-one", snapshot.clone()).unwrap();
    equal(150.0, run.snapshot("completed").totals.members.iter().find(|m| m.id == "a").unwrap().damage);
    let json = serde_json::to_value(&snapshot).unwrap();
    equal(150.0, json["formerMembers"][0]["damage"].as_f64().unwrap());
}

#[test]
fn a_newly_observed_member_cannot_claim_contributions_from_before_joining() {
    let mut e = CombatEngine::empty(None);
    e.advance(5.0).unwrap();
    e.register_member(party()[0].clone()).unwrap();
    assert!(!e.add(&hit("before", "a", 4.0, 100.0)));
    assert!(e.add(&hit("now", "a", 5.0, 25.0)));
    equal(0.0, *e.snapshot(true, "running").timeline[0].damage.get("a").unwrap());
    equal(25.0, e.snapshot(true, "running").total_damage);
}

#[test]
fn unknown_weapon_contributions_are_never_relabeled_after_equipment_is_discovered() {
    let mut e = CombatEngine::new(vec![PartyMember::new("a", "Alpha", "Unknown", "unknown", "#ffffff")], None, None).unwrap();
    e.advance(1.0).unwrap();
    e.add(&hit("before", "a", 1.0, 10.0));
    e.register_member(PartyMember::new("a", "Alpha", "Hammer", "hammer", "#ffffff")).unwrap();
    e.advance(2.0).unwrap();
    e.add(&hit("after", "a", 2.0, 20.0).weapon(9022));
    let snapshot = e.snapshot(true, "running");
    let weapons = snapshot.weapon_contributions.unwrap();
    let unknown = weapons.iter().find(|w| w.weapon_index.is_none()).unwrap();
    assert!(unknown.damage == 10.0 && unknown.weapon == "Weapon not observed");
    equal(20.0, weapons.iter().find(|w| w.weapon_index == Some(9022)).unwrap().damage);
}

/// The incremental snapshot: an idle tick touches no rows, a late hit only its second, its member and its sources.
#[test]
fn deltas_carry_only_the_rows_changed_since_the_last_sequence() {
    let mut e = engine();
    e.advance(30.0).unwrap();
    let all = e.delta(-1, "running");
    assert!(all.full && all.snapshot.timeline.len() == 31 && all.changed.members == ["a", "b"]);
    let idle = e.delta(all.sequence, "running");
    assert!(!idle.full && idle.snapshot.timeline.is_empty() && idle.sequence == all.sequence);
    assert_eq!(idle.changed, meter_core::CombatChanges::default());
    assert_eq!(idle.snapshot.members.len(), 2);
    e.add(&hit("late", "a", 2.5, 100.0).weapon(9022));
    let late = e.delta(idle.sequence, "running");
    assert_eq!(late.changed.seconds, [2]);
    assert_eq!(late.changed.members, ["a"]);
    assert_eq!(late.changed.weapon_contributions, ["a:9022"]);
    assert_eq!(late.changed.item_contributions, ["a:unknown:unknown:unknown"]);
    assert_eq!(late.snapshot.weapon_contributions.as_ref().unwrap().len(), 1);
    equal(100.0, late.snapshot.total_damage);
    e.advance(31.5).unwrap();
    let tick = e.delta(late.sequence, "running");
    assert!(tick.changed.seconds == [31] && tick.changed.members.is_empty() && tick.snapshot.weapon_contributions.unwrap().is_empty());
    // Moving the hit to its proven source drops the old item row and adds the new one.
    let counted = hit("late", "a", 2.5, 100.0).weapon(9022);
    assert!(e.reattribute(&counted, &counted.clone().source(9022, "mainhand", "autoAttack")));
    let moved = e.delta(tick.sequence, "running");
    assert_eq!(moved.changed.removed_contributions, ["a:unknown:unknown:unknown"]);
    assert_eq!(moved.changed.item_contributions, ["a:mainhand:9022:autoAttack"]);
    assert!(moved.changed.members.is_empty() && moved.changed.seconds == [2]);
    // Points are shared with the engine until it writes them again.
    let shared = e.full_snapshot("running");
    assert!(std::sync::Arc::ptr_eq(&shared.timeline[5], &e.full_snapshot("running").timeline[5]));
}
