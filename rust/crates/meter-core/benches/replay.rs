//! `cargo bench -p meter-core`: decode throughput (packets/s) and per-packet latency of `CaptureAnalysis::add` over
//! captures. Real captures come from `METER_BENCH_PCAPS` (paths separated by `:`) or `.dev/data/captures/*.pcap`;
//! the synthetic scenarios always run.
//!
//! Before criterion runs, each capture is replayed once with a timer around every `add`, printing p50/p99/p99.9/max
//! and how many times faster than real time the capture decodes (the "≥ 50× real raid traffic" gate).

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use meter_core::capture::pcap::PcapFile;
use meter_core::{CaptureAnalysis, CaptureFrame, CombatSourceCatalog};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

fn captures() -> Vec<(String, Vec<CaptureFrame>)> {
    let mut paths: Vec<PathBuf> = std::env::var("METER_BENCH_PCAPS").map(|v| v.split(':').map(PathBuf::from).collect()).unwrap_or_default();
    let recorded = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../.dev/data/captures");
    if paths.is_empty()
        && let Ok(entries) = std::fs::read_dir(recorded)
    {
        paths.extend(entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "pcap")));
    }
    let mut result: Vec<(String, Vec<CaptureFrame>)> = paths
        .iter()
        .filter_map(|path| {
            let mut file = PcapFile::open(path).ok()?;
            let mut frames = Vec::new();
            while let Ok(Some(frame)) = file.read() {
                frames.push(frame);
            }
            Some((path.file_name()?.to_string_lossy().into_owned(), frames))
        })
        .collect();
    result.extend(meter_core::testkit::scenarios::all().into_iter().filter(|(_, frames)| frames.len() > 1000).map(|(name, frames)| (name.to_owned(), frames)));
    result
}

fn catalog() -> Option<Arc<CombatSourceCatalog>> {
    CombatSourceCatalog::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../assets/game-data/combat.json")).map(Arc::new)
}

fn latency(name: &str, frames: &[CaptureFrame], sources: Option<Arc<CombatSourceCatalog>>) {
    let mut analysis = CaptureAnalysis::new(sources, false);
    let mut times: Vec<u64> = Vec::with_capacity(frames.len());
    let started = Instant::now();
    for frame in frames {
        let before = Instant::now();
        analysis.add(frame);
        times.push(before.elapsed().as_nanos() as u64);
    }
    let total = started.elapsed();
    times.sort_unstable();
    let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize] as f64 / 1000.0;
    let span = (frames.last().unwrap().timestamp_us - frames[0].timestamp_us).max(1) as f64 / 1_000_000.0;
    let rate = frames.len() as f64 / total.as_secs_f64();
    println!(
        "{name}: {} packets, {:.0} packets/s, {:.0}x real time, per packet p50 {:.2} µs p99 {:.2} µs p99.9 {:.2} µs max {:.2} µs",
        frames.len(),
        rate,
        rate / (frames.len() as f64 / span),
        at(0.5),
        at(0.99),
        at(0.999),
        at(1.0)
    );
}

fn replay(c: &mut Criterion) {
    let sources = catalog();
    let captures = captures();
    for (name, frames) in &captures {
        latency(name, frames, sources.clone());
    }
    let mut group = c.benchmark_group("replay");
    for (name, frames) in &captures {
        group.throughput(Throughput::Elements(frames.len() as u64));
        group.bench_function(name.as_str(), |b| {
            b.iter(|| {
                let mut analysis = CaptureAnalysis::new(sources.clone(), false);
                for frame in frames {
                    analysis.add(black_box(frame));
                }
                black_box(analysis.combat_delta(-1).map(|d| d.sequence))
            })
        });
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20);
    targets = replay
}
criterion_main!(benches);
