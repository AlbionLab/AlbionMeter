//! tests/AlbionMeter.Tests/ManualRecordingChecks.cs, driven the way the app drives it: frames from a live source and
//! the UI's record commands, through CaptureService/MeterSession semantics, checked on what the UI and the history get.

mod common;

use common::*;
use meter_runtime::feed::{FeedEvent, RecordStatus};
use meter_runtime::Command;
use meter_store::EncounterStore;
use std::sync::Arc;

fn alpha(snapshot: &meter_runtime::feed::Snapshot) -> f64 {
    snapshot.members.iter().find(|member| &*member.name == "Alpha").map_or(-1.0, |member| member.damage)
}

#[test]
fn manual_recording_ignores_pre_play_hits_preserves_pause_resume_and_resets_after_stop() {
    let directory = temp_dir("manual");
    let capture = Arc::new(FakeCapture::default());
    let source = capture.source();
    let (clock, time) = ManualClock::new(1_000_000);
    let mut meter = Meter::start(config(&directory, capture, time));
    let mut game = Game::default();
    meter.start_capture();
    send(&source, game.join(1_100_000));
    send(&source, game.hit(-900.0, 1_200_000));
    meter.wait_frames(2);
    meter.settle();
    let idle = meter.events.last_snapshot();
    assert_eq!(idle.status, RecordStatus::Idle);
    assert_eq!(idle.total_damage, 0.0);

    clock.set(2_000_000);
    assert_eq!(meter.record(Command::StartRecord), None);
    let id = meter.events.last_snapshot().id.clone();
    assert_eq!(meter.events.last_snapshot().status, RecordStatus::Running);
    send(&source, game.hit(-100.0, 3_000_000));
    send(&source, game.hit(40.0, 4_000_000));
    meter.wait_frames(4);
    clock.set(5_000_000);
    assert_eq!(meter.record(Command::PauseRecord), None);
    send(&source, game.hit(-900.0, 6_000_000));
    meter.wait_frames(5);
    meter.settle();
    let paused = meter.events.last_snapshot();
    assert_eq!((paused.status, paused.total_damage, paused.elapsed_seconds), (RecordStatus::Paused, 100.0, 3.0));

    clock.set(8_000_000);
    assert_eq!(meter.record(Command::ResumeRecord), None);
    send(&source, game.hit(-25.0, 9_000_000));
    meter.wait_frames(6);
    let resumed = meter.events.wait_snapshot(0, |snapshot| snapshot.total_damage == 125.0);
    assert_eq!((resumed.id.clone(), resumed.total_healing), (id.clone(), 40.0));

    clock.set(10_000_000);
    assert_eq!(meter.record(Command::StopRecord), None);
    send(&source, game.hit(-500.0, 11_000_000));
    meter.wait_frames(7);
    meter.settle();
    let stopped = meter.events.last_snapshot();
    assert_eq!((stopped.status, stopped.total_damage, stopped.elapsed_seconds), (RecordStatus::Completed, 125.0, 5.0));
    // Stop ends the record, not the capture.
    assert_eq!(&*meter.events.last_capture().state, "running");

    clock.set(12_000_000);
    assert_eq!(meter.record(Command::StartRecord), None);
    let next = meter.events.last_snapshot();
    assert_ne!(next.id, id);
    assert_eq!((next.total_damage, next.total_healing, next.event_count, next.elapsed_seconds), (0.0, 0.0, 0, 0.0));
    assert!(next.item_contributions.is_empty());
    assert!(next.timeline.iter().all(|point| point.damage.values().sum::<f64>() == 0.0 && point.healing.values().sum::<f64>() == 0.0));
    send(&source, game.hit(-10.0, 13_000_000));
    meter.wait_frames(8);
    let counted = meter.events.wait_snapshot(0, |snapshot| snapshot.id == next.id && snapshot.total_damage == 10.0);
    assert_eq!(counted.total_healing, 0.0);

    // The archive of the stopped record, the second record and both dungeon runs are in the history.
    meter.shutdown();
    let store = EncounterStore::open(&directory, false).unwrap();
    let encounters = store.list().unwrap();
    let first = encounters.iter().find(|encounter| *encounter.id == *id).expect("the stopped record");
    assert_eq!((first.damage, first.healing, first.duration), (125.0, 40.0, 5.0));
    assert!(encounters.iter().any(|encounter| *encounter.id == *next.id && encounter.damage == 10.0));
    let runs = store.list_dungeons().unwrap();
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(runs.iter().any(|run| run.damage == 125.0) && runs.iter().any(|run| run.damage == 10.0), "{runs:?}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_record_clock_runs_from_play_without_packets_and_stays_frozen_while_paused() {
    let directory = temp_dir("clock");
    let capture = Arc::new(FakeCapture::default());
    let _source = capture.source();
    let (clock, time) = ManualClock::new(1_000_000);
    let mut meter = Meter::start(config(&directory, capture, time));
    meter.start_capture();
    clock.set(1_500_000);
    meter.settle();
    assert_eq!(meter.events.last_snapshot().elapsed_seconds, 0.0);
    clock.set(2_000_000);
    assert_eq!(meter.record(Command::StartRecord), None);
    clock.set(7_000_000);
    let from = meter.events.len();
    meter.events.wait_snapshot(from, |snapshot| snapshot.elapsed_seconds == 5.0);
    clock.set(8_000_000);
    assert_eq!(meter.record(Command::PauseRecord), None);
    assert_eq!(meter.events.last_snapshot().elapsed_seconds, 6.0);
    clock.set(20_000_000);
    meter.settle();
    assert_eq!(meter.events.last_snapshot().elapsed_seconds, 6.0);
    // A paused record sends nothing: no snapshot while it stays frozen.
    let quiet = meter.events.len();
    meter.settle();
    assert!(!meter.events.all()[quiet..].iter().any(|event| matches!(event, FeedEvent::Snapshot(_))));
    clock.set(30_000_000);
    assert_eq!(meter.record(Command::ResumeRecord), None);
    clock.set(33_000_000);
    let from = meter.events.len();
    meter.events.wait_snapshot(from, |snapshot| snapshot.elapsed_seconds == 9.0);
    // A tick stamped a little before the last one never moves the clock back.
    clock.set(32_000_000);
    meter.settle();
    assert_eq!(meter.events.last_snapshot().elapsed_seconds, 9.0);
    meter.shutdown();
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn reset_clears_damage_and_healing_but_keeps_the_record_its_players_and_the_pause() {
    let directory = temp_dir("reset");
    let capture = Arc::new(FakeCapture::default());
    let source = capture.source();
    let (clock, time) = ManualClock::new(1_000_000);
    let mut meter = Meter::start(config(&directory, capture, time));
    let mut game = Game::default();
    meter.start_capture();
    send(&source, game.join(1_100_000));
    meter.wait_frames(1);
    clock.set(2_000_000);
    assert_eq!(meter.record(Command::StartRecord), None);
    send(&source, game.hit(-100.0, 3_000_000));
    send(&source, game.hit(40.0, 4_000_000));
    meter.wait_frames(3);
    let id = meter.events.wait_snapshot(0, |snapshot| snapshot.total_damage == 100.0).id.clone();
    clock.set(5_000_000);
    assert_eq!(meter.record(Command::NewRecord), None);
    let reset = meter.events.last_snapshot();
    assert_eq!((reset.id.clone(), reset.status, reset.total_damage, reset.total_healing, alpha(&reset)), (id.clone(), RecordStatus::Running, 0.0, 0.0, 0.0));
    send(&source, game.hit(-30.0, 6_000_000));
    meter.wait_frames(4);
    let after = meter.events.wait_snapshot(0, |snapshot| snapshot.id == id && snapshot.total_damage == 30.0);
    assert_eq!(alpha(&after), 30.0);
    clock.set(7_000_000);
    assert_eq!(meter.record(Command::PauseRecord), None);
    clock.set(8_000_000);
    assert_eq!(meter.record(Command::NewRecord), None);
    send(&source, game.hit(-500.0, 9_000_000));
    meter.wait_frames(5);
    meter.settle();
    let paused = meter.events.last_snapshot();
    assert_eq!((paused.id.clone(), paused.status, paused.total_damage), (id, RecordStatus::Paused, 0.0));
    meter.shutdown();
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn record_commands_answer_with_the_csharp_errors() {
    let directory = temp_dir("errors");
    let capture = Arc::new(FakeCapture::default());
    let _source = capture.source();
    let (_clock, time) = ManualClock::new(1_000_000);
    let mut meter = Meter::start(config(&directory, capture, time));
    assert_eq!(meter.record(Command::StopRecord).as_deref(), Some("There is no active record to stop."));
    assert_eq!(meter.record(Command::NewRecord).as_deref(), Some("There is no active record to reset."));
    assert_eq!(meter.record(Command::PauseRecord).as_deref(), Some("Press Play to start a record first."));
    // Play opens the saved (here the default) device by itself.
    assert_eq!(meter.record(Command::StartRecord), None);
    meter.events.wait_capture(0, "running");
    assert_eq!(meter.record(Command::StartRecord).as_deref(), Some("Stop the current record before starting another one."));
    assert_eq!(meter.record(Command::ResumeRecord).as_deref(), Some("The record is not paused."));
    assert_eq!(meter.record(Command::PauseRecord), None);
    assert_eq!(meter.record(Command::PauseRecord).as_deref(), Some("The record is not running."));
    let from = meter.events.len();
    meter.handle().send(Command::StartCapture { device: DEVICE.into(), method: "pcap".into() });
    assert_eq!(meter.events.capture_done(from).as_deref(), Some("The selected interface does not match the capture method."));
    let from = meter.events.len();
    meter.handle().send(Command::StartCapture { device: DEVICE.into(), method: "socket".into() });
    assert_eq!(meter.events.capture_done(from).as_deref(), Some("Stop the current capture before starting another operation."));
    let from = meter.events.len();
    meter.handle().send(Command::StartCapture { device: "raw:gone".into(), method: "socket".into() });
    assert_eq!(meter.events.capture_done(from).as_deref(), Some("Select an available capture interface."));
    // The chosen device is remembered in capture-settings.json, as CaptureService.SaveDevice writes it.
    meter.shutdown();
    let settings = std::fs::read_to_string(directory.join("capture-settings.json")).unwrap();
    assert_eq!(settings, "{\n  \"device\": \"raw:test\"\n}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_meter_reopened_on_the_same_map_names_the_players_it_knew_before_closing() {
    let directory = temp_dir("restart");
    let (clock, time) = ManualClock::new(1_000_000);
    let mut game = Game::default();
    {
        let capture = Arc::new(FakeCapture::default());
        let source = capture.source();
        let mut meter = Meter::start(config(&directory, capture, time.clone()));
        meter.start_capture();
        send(&source, game.join(1_100_000));
        meter.wait_frames(1);
        // known-players.json is written every 10 seconds of capture, as the C# loop does.
        std::thread::sleep(std::time::Duration::from_millis(10_800));
        meter.shutdown();
    }
    assert!(std::fs::read_to_string(directory.join("captures/known-players.json")).unwrap().contains("\"Alpha\""));
    assert!(directory.join("captures/local-character.json").exists());
    // The meter closes and opens again in the middle of the same floor: no zone change announces the players.
    let capture = Arc::new(FakeCapture::default());
    let source = capture.source();
    let mut meter = Meter::start(config(&directory, capture, time));
    clock.set(5_000_000);
    meter.start_capture();
    clock.set(6_000_000);
    assert_eq!(meter.record(Command::StartRecord), None);
    send(&source, game.hit(-70.0, 7_000_000));
    meter.wait_frames(1);
    let named = meter.events.wait_snapshot(0, |snapshot| snapshot.total_damage == 70.0);
    assert_eq!(alpha(&named), 70.0, "{:?}", named.members);
    meter.shutdown();
    let _ = std::fs::remove_dir_all(&directory);
}
