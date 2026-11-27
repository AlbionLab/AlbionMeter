//! CaptureService's drop → hold → reconnect → resume cycle, with sources that end or fail by themselves.

mod common;

use common::*;
use meter_capture::CaptureError;
use meter_runtime::feed::RecordStatus;
use meter_runtime::Command;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

fn alpha(snapshot: &meter_runtime::feed::Snapshot) -> f64 {
    snapshot.members.iter().find(|member| &*member.name == "Alpha").map_or(-1.0, |member| member.damage)
}

/// A running record with Alpha's first hit counted.
fn recording(name: &str) -> (Meter, Arc<FakeCapture>, crossbeam_channel::Sender<Step>, ManualClock, Game) {
    let directory = temp_dir(name);
    let capture = Arc::new(FakeCapture::default());
    let source = capture.source();
    let (clock, time) = ManualClock::new(1_000_000);
    let meter = Meter::start(config(&directory, capture.clone(), time));
    let mut game = Game::default();
    meter.start_capture();
    send(&source, game.join(1_100_000));
    clock.set(2_000_000);
    meter.wait_frames(1);
    assert_eq!(meter.record(Command::StartRecord), None);
    send(&source, game.hit(-100.0, 3_000_000));
    meter.wait_frames(2);
    meter.events.wait_snapshot(0, |snapshot| snapshot.total_damage == 100.0);
    (meter, capture, source, clock, game)
}

#[test]
fn a_capture_that_drops_and_comes_back_keeps_the_same_record_and_its_players() {
    let (mut meter, capture, source, clock, mut game) = recording("drop");
    let id = meter.events.last_snapshot().id.clone();
    let next = capture.source();
    clock.set(4_000_000);
    let from = meter.events.len();
    source.send(Step::End).unwrap();
    // Held: the capture completed by itself and the record waits paused.
    let ended = meter.events.wait_capture(from, "completed");
    assert_eq!(ended.stop_reason.as_deref(), Some("sourceEnded"));
    let held = meter.events.wait_snapshot(from, |snapshot| snapshot.status == RecordStatus::Paused);
    assert_eq!(held.id, id);
    clock.set(7_000_000);
    // Back after the reconnect delay, resumed by itself, with what it knew.
    meter.events.wait_capture(from, "running");
    assert_eq!(capture.opens.load(Ordering::Relaxed), 2);
    meter.events.wait_snapshot(from, |snapshot| snapshot.status == RecordStatus::Running);
    send(&next, game.hit(-50.0, 8_000_000));
    meter.wait_frames(3);
    let after = meter.events.wait_snapshot(from, |snapshot| snapshot.total_damage == 150.0);
    assert_eq!((after.id.clone(), after.status, alpha(&after)), (id, RecordStatus::Running, 150.0));
    meter.shutdown();
}

#[test]
fn pause_and_resume_during_a_reconnect_decide_whether_the_record_continues() {
    let (mut meter, capture, source, clock, _) = recording("drop-pause");
    let _next = capture.source();
    let from = meter.events.len();
    clock.set(4_000_000);
    source.send(Step::End).unwrap();
    meter.events.wait_capture(from, "completed");
    // Pause while the capture is away: accepted, and the record stays paused once the capture is back.
    assert_eq!(meter.record(Command::PauseRecord), None);
    meter.events.wait_capture(from, "running");
    meter.settle();
    assert_eq!(meter.events.last_snapshot().status, RecordStatus::Paused);
    assert_eq!(meter.record(Command::ResumeRecord), None);
    assert_eq!(meter.events.last_snapshot().status, RecordStatus::Running);
    meter.shutdown();
}

#[test]
fn stop_during_the_reconnect_wait_keeps_the_capture_stopped() {
    let (mut meter, capture, source, _clock, _) = recording("drop-stop");
    let _next = capture.source();
    let from = meter.events.len();
    source.send(Step::End).unwrap();
    meter.events.wait_capture(from, "completed");
    let from = meter.events.len();
    meter.handle().send(Command::StopCapture);
    assert_eq!(meter.events.capture_done(from), None);
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(capture.opens.load(Ordering::Relaxed), 1, "no reconnect after Stop");
    assert_eq!(&*meter.events.last_capture().state, "completed");
    meter.shutdown();
}

#[test]
fn a_failing_source_is_held_and_reopened_but_a_permission_error_ends_the_record() {
    let (mut meter, capture, source, clock, _) = recording("fail");
    // A read error that is not about permission: like a drop, the record is held and the device reopened, which now
    // fails on permission: the record ends instead of waiting forever.
    capture.fail(CaptureError::Io("Socket capture permission denied; CAP_NET_RAW may be required: Operation not permitted".into()));
    let from = meter.events.len();
    clock.set(4_000_000);
    source.send(Step::Fail(CaptureError::Io("Socket receive failed: Network is down".into()))).unwrap();
    let failed = meter.events.wait_capture(from, "failed");
    assert_eq!((failed.error.as_deref(), failed.stop_reason.as_deref()), (Some("Socket receive failed: Network is down"), Some("error")));
    meter.events.wait_snapshot(from, |snapshot| snapshot.status == RecordStatus::Paused);
    let denied = meter.events.wait_from(from, Duration::from_secs(10), |event| match event {
        meter_runtime::FeedEvent::Capture(status) if status.error.as_deref().is_some_and(|error| error.contains("CAP_NET_RAW")) => Some(status.clone()),
        _ => None,
    });
    assert_eq!(&*denied.state, "failed");
    meter.events.wait_snapshot(from, |snapshot| snapshot.status == RecordStatus::Completed);
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(capture.opens.load(Ordering::Relaxed), 2, "no retry without permission");
    assert_eq!(meter.record(Command::PauseRecord).as_deref(), Some("Press Play to start a record first."));
    meter.shutdown();
}

#[test]
fn play_without_capture_permission_leaves_no_record_running() {
    let directory = temp_dir("denied");
    let capture = Arc::new(FakeCapture::default());
    capture.fail(CaptureError::Io("Packet capture permission denied: run Albion Meter as administrator.".into()));
    let (_clock, time) = ManualClock::new(1_000_000);
    let mut meter = Meter::start(config(&directory, capture, time));
    assert_eq!(meter.record(Command::StartRecord), None);
    meter.events.wait_capture(0, "failed");
    meter.events.wait_snapshot(0, |snapshot| snapshot.status == RecordStatus::Completed);
    // Play is available again (not "Stop the current record before starting another one.").
    assert_eq!(meter.record(Command::StartRecord), None);
    meter.shutdown();
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn stopping_the_capture_ends_the_record_and_answers_once_it_stopped() {
    let (mut meter, _capture, _source, clock, _) = recording("stop");
    clock.set(5_000_000);
    let from = meter.events.len();
    meter.handle().send(Command::StopCapture);
    meter.events.wait_capture(from, "stopping");
    assert_eq!(meter.events.capture_done(from), None);
    let status = meter.events.last_capture();
    assert_eq!((&*status.state, status.stop_reason.as_deref()), ("completed", Some("stopped")));
    let stopped = meter.events.wait_snapshot(from, |snapshot| snapshot.status == RecordStatus::Completed);
    assert_eq!((stopped.total_damage, stopped.elapsed_seconds), (100.0, 3.0));
    meter.shutdown();
}
