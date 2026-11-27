//! tests/AlbionMeter.Tests/ManualRecordingChecks.cs: Play/Pause/End/Reset through whole frames.

mod common;

use common::*;
use meter_core::testkit::message_frame;
use meter_core::{CaptureAnalysis, RecordState};

struct Session {
    analysis: CaptureAnalysis,
    sequence: u32,
}

impl Session {
    fn new() -> Self {
        let mut analysis = CaptureAnalysis::new(None, true);
        analysis.begin_capture(Some(1_000_000), false, false);
        Self { analysis, sequence: 0 }
    }
    fn send(&mut self, message: WireMessage, at: i64) {
        self.sequence += 1;
        self.analysis.add(&message_frame(&message, at, self.sequence));
    }
    fn join(&mut self, character: Guid) {
        self.send(resp(p![253 => 2u8, 0 => 10, 1 => character, 2 => "Alpha", 8 => "@RANDOMDUNGEON@one", 66 => "1315"]), 1_100_000);
    }
    fn hit(&mut self, delta: f32, at: i64) {
        self.send(ev(p![252 => 6u8, 0 => 30, 6 => 10, 2 => delta, 7 => -1, 5 => 0u8]), at);
    }
}

#[test]
fn manual_recording_ignores_pre_play_hits_preserves_pause_resume_and_resets_after_stop() {
    let mut s = Session::new();
    s.join(new_id());
    s.hit(-900.0, 1_200_000);
    let report = s.analysis.snapshot();
    assert!(s.analysis.record_state() == RecordState::Idle && report.combat.unwrap().total_damage == 0.0 && report.dungeons.unwrap().is_empty());
    s.analysis.start_record(2_000_000).unwrap();
    let id = s.analysis.snapshot().combat.unwrap().id;
    assert_eq!(s.analysis.snapshot().dungeons.unwrap().len(), 1);
    s.hit(-100.0, 3_000_000);
    s.hit(40.0, 4_000_000);
    s.analysis.set_paused(true, 5_000_000).unwrap();
    s.hit(-900.0, 6_000_000);
    let combat = s.analysis.snapshot().combat.unwrap();
    assert!(s.analysis.record_state() == RecordState::Paused && combat.total_damage == 100.0 && combat.elapsed_seconds == 3.0);
    s.analysis.set_paused(false, 8_000_000).unwrap();
    s.hit(-25.0, 9_000_000);
    let combat = s.analysis.snapshot().combat.unwrap();
    assert!(combat.id == id && combat.total_damage == 125.0 && combat.total_healing == 40.0);
    let dungeon = s.analysis.snapshot().dungeons.unwrap().remove(0);
    s.analysis.stop_record(10_000_000).unwrap();
    s.hit(-500.0, 11_000_000);
    let report = s.analysis.snapshot();
    let combat = report.combat.unwrap();
    assert!(s.analysis.record_state() == RecordState::Completed && combat.total_damage == 125.0 && combat.elapsed_seconds == 5.0);
    assert_eq!(report.dungeons.unwrap()[0].totals.total_damage, 125.0);
    s.analysis.start_record(12_000_000).unwrap();
    let next = s.analysis.snapshot();
    let combat = next.combat.unwrap();
    assert!(combat.id != id && combat.total_damage == 0.0 && combat.total_healing == 0.0 && combat.event_count == 0 && combat.elapsed_seconds == 0.0);
    assert!(combat.timeline.iter().all(|p| p.damage.values().sum::<f64>() == 0.0 && p.healing.values().sum::<f64>() == 0.0));
    assert!(combat.item_contributions.unwrap().is_empty() && combat.weapon_contributions.unwrap().is_empty());
    let dungeons = next.dungeons.unwrap();
    assert!(dungeons.len() == 1 && dungeons[0].id != dungeon.id && dungeons[0].totals.total_damage == 0.0);
    s.hit(-10.0, 13_000_000);
    let report = s.analysis.snapshot();
    let combat = report.combat.unwrap();
    assert!(combat.total_damage == 10.0 && combat.total_healing == 0.0 && report.dungeons.unwrap()[0].totals.total_damage == 10.0);
    assert!(s.analysis.start_record(14_000_000).is_err());
}

#[test]
fn the_record_clock_runs_from_play_without_packets_and_stays_frozen_while_paused() {
    let mut analysis = CaptureAnalysis::new(None, true);
    analysis.begin_capture(Some(1_000_000), false, false);
    analysis.advance_clock(1_500_000);
    assert!(analysis.snapshot().combat.is_none_or(|c| c.elapsed_seconds == 0.0));
    analysis.start_record(2_000_000).unwrap();
    analysis.advance_clock(7_000_000);
    let elapsed = |analysis: &mut CaptureAnalysis| analysis.snapshot().combat.unwrap().elapsed_seconds;
    assert_eq!(elapsed(&mut analysis), 5.0);
    analysis.set_paused(true, 8_000_000).unwrap();
    assert_eq!(elapsed(&mut analysis), 6.0);
    analysis.advance_clock(20_000_000);
    assert_eq!(elapsed(&mut analysis), 6.0);
    analysis.set_paused(false, 30_000_000).unwrap();
    analysis.advance_clock(33_000_000);
    assert_eq!(elapsed(&mut analysis), 9.0);
    // A packet stamped a little before the last tick never moves the clock back.
    analysis.advance_clock(32_000_000);
    assert_eq!(elapsed(&mut analysis), 9.0);
}

#[test]
fn a_capture_that_drops_and_comes_back_keeps_the_same_record_and_its_players() {
    let mut s = Session::new();
    s.join(new_id());
    s.analysis.start_record(2_000_000).unwrap();
    s.hit(-100.0, 3_000_000);
    let before = s.analysis.snapshot().combat.unwrap();
    // The capture source ends by itself: the service pauses the record, restarts the capture keeping what it knew, and resumes.
    s.analysis.set_paused(true, 4_000_000).unwrap();
    s.analysis.begin_capture(Some(7_000_000), true, true);
    s.analysis.set_paused(false, 7_000_000).unwrap();
    s.hit(-50.0, 8_000_000);
    let after = s.analysis.snapshot().combat.unwrap();
    assert!(s.analysis.record_state() == RecordState::Running && after.id == before.id);
    assert!(after.total_damage == 150.0 && after.members.iter().any(|m| m.name == "Alpha" && m.damage == 150.0));
}

#[test]
fn reset_clears_damage_and_healing_but_keeps_the_record_its_players_and_the_pause() {
    let mut s = Session::new();
    s.join(new_id());
    s.analysis.start_record(2_000_000).unwrap();
    s.hit(-100.0, 3_000_000);
    s.hit(40.0, 4_000_000);
    let id = s.analysis.snapshot().combat.unwrap().id;
    s.analysis.reset_counts(5_000_000).unwrap();
    let reset = s.analysis.snapshot().combat.unwrap();
    assert!(s.analysis.record_state() == RecordState::Running && reset.id == id && reset.total_damage == 0.0 && reset.total_healing == 0.0);
    assert!(reset.members.iter().any(|m| m.name == "Alpha" && m.damage == 0.0));
    s.hit(-30.0, 6_000_000);
    let report = s.analysis.snapshot();
    let after = report.combat.unwrap();
    assert!(after.id == id && after.total_damage == 30.0 && after.members.iter().any(|m| m.name == "Alpha" && m.damage == 30.0));
    assert_eq!(report.dungeons.unwrap()[0].totals.total_damage, 30.0);
    s.analysis.set_paused(true, 7_000_000).unwrap();
    s.analysis.reset_counts(8_000_000).unwrap();
    s.hit(-500.0, 9_000_000);
    let combat = s.analysis.snapshot().combat.unwrap();
    assert!(s.analysis.record_state() == RecordState::Paused && combat.total_damage == 0.0 && combat.id == id);
}

#[test]
fn a_meter_reopened_on_the_same_map_names_the_players_it_knew_before_closing() {
    let character = new_id();
    let mut before = Session::new();
    before.join(character);
    let known = before.analysis.known_players();
    assert!(known.iter().any(|p| p.name == "Alpha" && p.object_id == 10));
    // The meter closes and opens again in the middle of the same floor: no zone change announces the players.
    let mut after = CaptureAnalysis::new(None, true);
    after.restore_character(before.analysis.snapshot().character.unwrap());
    after.restore_players(known);
    after.begin_capture(Some(5_000_000), false, false);
    after.start_record(6_000_000).unwrap();
    after.add(&message_frame(&ev(p![252 => 6u8, 0 => 30, 6 => 10, 2 => -70f32, 7 => -1, 5 => 0u8]), 7_000_000, before.sequence + 1));
    assert!(after.snapshot().combat.unwrap().members.iter().any(|m| m.name == "Alpha" && m.damage == 70.0));
}

#[test]
fn the_report_serializes_like_the_csharp_capture_report() {
    let mut s = Session::new();
    s.join(new_id());
    s.analysis.start_record(2_000_000).unwrap();
    s.hit(-100.0, 3_000_000);
    let json = serde_json::to_value(s.analysis.snapshot()).unwrap();
    let keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
    let expected = [
        "frames", "bytes", "firstTimestampMicroseconds", "lastTimestampMicroseconds", "counters", "flows", "combatDecoded", "character", "party", "combat",
        "combatDiagnostics", "weapons", "contextFrames", "dungeons", "recordPaused", "recordTimestampMicroseconds", "recordState",
    ];
    let mut sorted = expected.to_vec();
    sorted.sort_unstable();
    assert_eq!(keys, sorted);
    assert_eq!(json["recordState"], "running");
    assert_eq!(json["combat"]["startedAt"], "1970-01-01T00:00:02+00:00");
    assert_eq!(json["combat"]["members"][0]["membership"], "active");
    assert!(json["party"].is_null() && json["combat"]["location"].is_null());
    assert_eq!(json["dungeons"][0]["floors"][0]["encounters"][0]["location"]["floorNumber"], 1);
}
