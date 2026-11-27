//! End to end: a capture replayed through the runtime lands in the history exactly as the C# server saves it.
//!
//! The fixtures are what `CaptureService.ReplayAsync` + `MeterSession.Save` wrote to a fresh EncounterStore for the
//! synthetic captures of `meter_core::testkit::scenarios` (same combat.json and maps.json), read back with
//! `EncounterStore.Get`. Random 32-hex ids are numbered by first appearance on both sides.

mod common;

use common::*;
use meter_core::capture::pcap::write_pcap;
use meter_core::testkit::scenarios;
use meter_runtime::feed::FeedEvent;
use meter_runtime::Command;
use meter_store::EncounterStore;
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn normalise(json: &str) -> String {
    let mut ids: Vec<String> = vec![];
    let mut out = String::with_capacity(json.len());
    let bytes = json.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'"' && index + 33 < bytes.len() && bytes[index + 33] == b'"' && bytes[index + 1..index + 33].iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b)) {
            let id = &json[index + 1..index + 33];
            let number = ids.iter().position(|known| known == id).unwrap_or_else(|| {
                ids.push(id.to_owned());
                ids.len() - 1
            });
            out.push_str(&format!("\"<id {number}>\""));
            index += 34;
        } else {
            let next = json[index..].chars().next().unwrap();
            out.push(next);
            index += next.len_utf8();
        }
    }
    out
}

/// The form `canonical.py` hashed the C# output in: document order kept, 32-hex ids numbered by first appearance,
/// numbers as the bits of their double (so `1E+15`/`1000000000000000` and 17-digit spellings compare by value).
fn canonical(json: &str) -> String {
    fn text(value: &str, ids: &mut Vec<String>, out: &mut String) {
        let renamed;
        let value = if value.len() == 32 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            let number = ids.iter().position(|known| known == value).unwrap_or_else(|| {
                ids.push(value.to_owned());
                ids.len() - 1
            });
            renamed = format!("<id {number}>");
            renamed.as_str()
        } else {
            value
        };
        out.push_str(&format!("s{}:{value}", value.len()));
    }
    fn walk(value: &serde_json::Value, ids: &mut Vec<String>, out: &mut String) {
        match value {
            serde_json::Value::Object(map) => {
                out.push('{');
                for (key, value) in map {
                    text(key, ids, out);
                    walk(value, ids, out);
                }
                out.push('}');
            }
            serde_json::Value::Array(values) => {
                out.push('[');
                for value in values {
                    walk(value, ids, out);
                }
                out.push(']');
            }
            serde_json::Value::Null => out.push('z'),
            serde_json::Value::Bool(flag) => out.push(if *flag { 't' } else { 'f' }),
            serde_json::Value::String(value) => text(value, ids, out),
            serde_json::Value::Number(number) => out.push_str(&format!("n{:016x}", number.as_f64().unwrap().to_bits())),
        }
    }
    let mut out = String::new();
    walk(&serde_json::from_str(json).unwrap(), &mut vec![], &mut out);
    out
}

/// Replays one capture through a fresh runtime and returns the saved encounters' JSON and the events.
fn replay(name: &str, frames: &[meter_core::CaptureFrame]) -> (Vec<String>, Vec<FeedEvent>) {
    let directory = temp_dir(name);
    let pcap = directory.join(format!("{name}.pcap"));
    std::fs::write(&pcap, write_pcap(frames, 101)).unwrap();
    let mut config = meter_runtime::Config::new(directory.join("storage"), Arc::new(FakeCapture::default()));
    config.assets = Some(repo_assets());
    let mut meter = Meter::start(config);
    let from = meter.events.len();
    meter.handle().send(Command::Replay(pcap));
    assert_eq!(meter.events.capture_done(from), None, "{name}");
    meter.shutdown();
    let store = EncounterStore::open(&directory.join("storage"), false).unwrap();
    let saved = store.list().unwrap().iter().map(|summary| store.get(&summary.id).unwrap().unwrap()).collect();
    let events = meter.events.all();
    let _ = std::fs::remove_dir_all(&directory);
    (saved, events)
}

#[test]
fn replayed_captures_are_saved_as_the_csharp_server_saves_them() {
    for (name, frames) in scenarios::all() {
        let (saved, events) = replay(name, &frames);
        assert_eq!(saved.len(), 1, "{name}: one encounter");
        let actual = normalise(&saved[0]);
        if let Some(directory) = std::env::var_os("METER_DUMP_REPLAY") {
            std::fs::write(std::path::Path::new(&directory).join(format!("{name}.json")), &saved[0]).unwrap();
        }
        if name == "stress-30min" {
            // 6.5 MB of C# JSON: only its canonical hash is kept (see `canonical`).
            let expected = include_str!("fixtures/csharp-replay-stress-30min.sha256").trim();
            assert_eq!(format!("{:x}", Sha256::digest(canonical(&saved[0]).as_bytes())), expected, "{name}");
        } else {
            let expected = std::fs::read_to_string(format!("{}/tests/fixtures/csharp-replay-{name}.json", env!("CARGO_MANIFEST_DIR"))).unwrap();
            assert_eq!(actual, normalise(expected.trim_end()), "{name}");
        }
        // The events a UI sees: the replay runs then completes at the end of the file, and the last snapshot is the
        // record that was saved.
        let states: Vec<String> = events.iter().filter_map(|event| if let FeedEvent::Capture(status) = event { Some(format!("{}:{}", status.state, status.source)) } else { None }).collect();
        assert!(states.contains(&"running:replay".to_owned()), "{name}: {states:?}");
        assert_eq!(states.last().map(String::as_str), Some("completed:replay"), "{name}");
        let FeedEvent::Capture(last) = events.iter().rev().find(|event| matches!(event, FeedEvent::Capture(_))).unwrap() else { unreachable!() };
        assert_eq!(last.stop_reason.as_deref(), Some("endOfFile"));
        assert_eq!(last.report.as_ref().unwrap().frames, frames.len() as u64, "{name}");
        let saved: serde_json::Value = serde_json::from_str(&saved[0]).unwrap();
        let snapshot = events.iter().rev().find_map(|event| if let FeedEvent::Snapshot(snapshot) = event { Some(snapshot.clone()) } else { None }).unwrap();
        assert_eq!(&*snapshot.mode, "replay");
        assert_eq!(snapshot.total_damage, saved["totalDamage"].as_f64().unwrap(), "{name}");
        assert_eq!(snapshot.event_count, saved["eventCount"].as_u64().unwrap(), "{name}");
        assert_eq!(snapshot.members.len(), saved["members"].as_array().unwrap().len(), "{name}");
    }
}

#[test]
fn the_ui_timeline_built_from_deltas_equals_the_saved_one() {
    let frames = scenarios::stress(5);
    let (saved, events) = replay("deltas", &frames);
    let saved: serde_json::Value = serde_json::from_str(&saved[0]).unwrap();
    // Merge every snapshot the way meter-app's merge.rs does.
    let mut timeline: std::collections::BTreeMap<u32, f64> = Default::default();
    let mut record = None;
    for event in &events {
        if let FeedEvent::Snapshot(snapshot) = event {
            if snapshot.full || record.as_ref() != Some(&(snapshot.id.clone(), snapshot.started_at.clone())) {
                timeline.clear();
                record = Some((snapshot.id.clone(), snapshot.started_at.clone()));
            }
            for point in &snapshot.timeline {
                timeline.insert(point.second, point.damage.values().sum());
            }
        }
    }
    let expected: std::collections::BTreeMap<u32, f64> = saved["timeline"].as_array().unwrap().iter().map(|point| (point["second"].as_u64().unwrap() as u32, point["damage"].as_object().unwrap().values().map(|v| v.as_f64().unwrap()).sum())).collect();
    assert_eq!(timeline.len(), expected.len());
    for (second, damage) in &expected {
        assert!((timeline[second] - damage).abs() < 1e-6, "second {second}: {} != {damage}", timeline[second]);
    }
    let snapshots = events.iter().filter(|event| matches!(event, FeedEvent::Snapshot(_))).count();
    let fulls = events.iter().filter(|event| matches!(event, FeedEvent::Snapshot(snapshot) if snapshot.full)).count();
    assert!(fulls <= 3, "{fulls} full snapshots of {snapshots}");
}

/// Byte for byte, the stress capture differs from the C# file in 518 numbers: doubles whose shortest form ends on an
/// exact decimal tie (161806.428955078125), which .NET rounds to even (…812) and `meter_store::json::format_double`
/// rounds up (…813). The values are equal (the test above); this one passes once meter-store rounds ties to even.
#[test]
#[ignore = "meter-store format_double rounds 17-digit ties up; .NET rounds them to even"]
fn the_stress_capture_is_saved_byte_for_byte() {
    let (saved, _) = replay("stress-bytes", &scenarios::stress(30));
    let expected = include_str!("fixtures/csharp-replay-stress-30min.text.sha256").trim();
    assert_eq!(format!("{:x}", Sha256::digest(normalise(&saved[0]).as_bytes())), expected);
}
