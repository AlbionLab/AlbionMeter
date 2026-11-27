//! The real-time gates of rust/README.md, measured through the whole runtime: a live source stamps each frame with
//! the wall clock as the kernel would, and the runtime's own histograms time it to `CaptureAnalysis::add` and to the
//! snapshot that carries it.
//!
//! `cargo test -p meter-runtime --release --test gates` for the latency numbers; the soak is
//! `cargo test -p meter-runtime --release -- --ignored soak --nocapture` (SOAK_MINUTES, default 10; SOAK_PPS,
//! default 2000 = 5 × a 400 packets/s raid).

mod common;

use common::*;
use meter_capture::{AdapterStatistics, CaptureError, CaptureFrame, FrameSource, Read};
use meter_core::capture::protocol18::Value;
use meter_core::cs::Guid;
use meter_core::testkit::{message_frame, msg, put32, scenarios};
use meter_runtime::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn wall_us() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64
}

/// Frames played at a fixed rate, each stamped with the wall clock when it is handed over; a frame's photon command
/// number is rewritten so that cycling through templates never repeats one.
struct Paced {
    setup: Vec<CaptureFrame>,
    templates: Vec<CaptureFrame>,
    /// Frames per second, as `f64` bits; the soak changes it between phases.
    rate: Arc<AtomicU64>,
    current: f64,
    started: Option<Instant>,
    emitted: Arc<AtomicU64>,
    sent: u64,
    sequence: u32,
    random: u64,
    /// Play the templates once, in order, then end; otherwise pick them at random forever.
    once: bool,
    next: usize,
}

impl Paced {
    fn pick(&mut self) -> Option<usize> {
        if self.once {
            self.next += 1;
            return (self.next <= self.templates.len()).then(|| self.next - 1);
        }
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        Some((self.random % self.templates.len() as u64) as usize)
    }
}

impl FrameSource for Paced {
    fn link_type(&self) -> i32 {
        101
    }
    fn read(&mut self, frame: &mut CaptureFrame) -> Result<Read, CaptureError> {
        let rate = f64::from_bits(self.rate.load(Ordering::Relaxed));
        if rate != self.current {
            (self.current, self.started, self.sent) = (rate, None, 0);
        }
        let started = *self.started.get_or_insert_with(Instant::now);
        let template = if let Some(setup) = (!self.setup.is_empty()).then(|| self.setup.remove(0)) {
            setup
        } else {
            let due = (started.elapsed().as_secs_f64() * rate) as u64;
            if self.sent >= due {
                let wait = Duration::from_secs_f64((self.sent + 1) as f64 / rate).saturating_sub(started.elapsed());
                std::thread::sleep(wait.min(Duration::from_millis(50)));
                if self.sent >= (started.elapsed().as_secs_f64() * rate) as u64 {
                    return Ok(Read::Idle);
                }
            }
            match self.pick() {
                Some(index) => self.templates[index].clone(),
                None => return Ok(Read::Ended),
            }
        };
        self.sent += 1;
        self.sequence += 1;
        frame.data.clear();
        frame.data.extend_from_slice(&template.data);
        put32(&mut frame.data, 48, self.sequence);
        frame.original_length = template.original_length;
        frame.link_type = 101;
        frame.timestamp_us = wall_us();
        self.emitted.fetch_add(1, Ordering::Relaxed);
        Ok(Read::Frame)
    }
    fn statistics(&mut self) -> Option<AdapterStatistics> {
        Some(AdapterStatistics::default())
    }
}

fn id(n: u8) -> Guid {
    Guid([n, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, n])
}

/// A 20-player party fighting: the local player joins a dungeon floor, the roster and the others are announced, then
/// hits on mobs, heals on players and other game traffic (moves) in a 35/10/55 mix.
fn raid() -> (Vec<CaptureFrame>, Vec<CaptureFrame>) {
    let ids: Vec<Guid> = (1..=20).map(id).collect();
    let names: Vec<String> = (1..=20).map(|n| format!("Player{n:02}")).collect();
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let objects: Vec<i32> = (0..20).map(|n| if n == 0 { 10 } else { 400 + n }).collect();
    let weapons = [7346i16, 6661, 9022, 7108, 7587, 4308, 2637];
    let mut setup = vec![];
    setup.push(message_frame(&msg(3, vec![(253, Value::Byte(2)), (0, 10.into()), (1, ids[0].into()), (2, "Player01".into()), (8, "@RANDOMDUNGEON@raid".into()), (66, "1315".into())]), 0, 0));
    setup.push(message_frame(&party_list(&ids, &name_refs, 77), 0, 0));
    for n in 1..20 {
        let gear = [weapons[n % weapons.len()], 0, 5347, 5005, 5397, 2900, 2662, 3098];
        setup.push(message_frame(&msg(4, vec![(252, Value::Byte(29)), (0, objects[n].into()), (1, names[n].as_str().into()), (7, ids[n].into()), (40, gear.as_slice().into())]), 0, 0));
    }
    let spells = [-1, 3338, 3404, 3409, 3413, 3342];
    let mut templates = vec![];
    for (n, object) in objects.iter().enumerate() {
        for k in 0..7 {
            let hit = msg(4, vec![(252, Value::Byte(6)), (0, (5000 + k).into()), (6, (*object).into()), (2, (-(100.0 + 37.0 * k as f32 + n as f32)).into()), (7, spells[k as usize % spells.len()].into()), (5, Value::Byte(0))]);
            templates.push(message_frame(&hit, 0, 0));
        }
        for k in 0..2 {
            let target = objects[(n + 1 + k) % objects.len()];
            let heal = msg(4, vec![(252, Value::Byte(6)), (0, target.into()), (6, (*object).into()), (2, (55.0 + n as f32).into()), (7, 3404.into()), (5, Value::Byte(0))]);
            templates.push(message_frame(&heal, 0, 0));
        }
        for k in 0..11 {
            let other = msg(4, vec![(252, Value::Byte(3)), (0, (*object).into()), (1, Value::Long(k)), (4, Value::Float(k as f32))]);
            templates.push(message_frame(&other, 0, 0));
        }
    }
    (setup, templates)
}

fn paced(setup: Vec<CaptureFrame>, templates: Vec<CaptureFrame>, rate: Arc<AtomicU64>, once: bool, emitted: Arc<AtomicU64>) -> Box<dyn FrameSource> {
    Box::new(Paced { setup, templates, rate, current: 0.0, started: None, emitted, sent: 0, sequence: 0, random: 0x9e37_79b9_7f4a_7c15, once, next: 0 })
}

fn table(rows: &[(&str, &str, String, bool)]) {
    println!("| Gate | Limit | Measured | |");
    println!("|---|---|---|---|");
    for (gate, limit, measured, pass) in rows {
        println!("| {gate} | {limit} | {measured} | {} |", if *pass { "pass" } else { "FAIL" });
    }
}

fn ms(microseconds: u64) -> String {
    format!("{:.3} ms", microseconds as f64 / 1000.0)
}

/// The limits hold for the release build the app ships; an unoptimised build only has to take every frame.
const TIMED: bool = !cfg!(debug_assertions);

#[test]
fn packet_latency_through_the_runtime_on_a_replayed_raid() {
    // The 30-minute synthetic raid (with floors, swaps and late names), replayed live at 60× its pace.
    let frames = scenarios::stress(30);
    let rate = frames.len() as f64 / (30.0 * 60.0) * 60.0;
    let directory = temp_dir("latency");
    let capture = Arc::new(FakeCapture::default());
    let emitted = Arc::new(AtomicU64::new(0));
    let total = frames.len() as u64;
    capture.custom(Box::new({
        let emitted = emitted.clone();
        move || Ok(paced(vec![], frames, Arc::new(AtomicU64::new(rate.to_bits())), true, emitted))
    }));
    let mut config = meter_runtime::Config::new(&directory, capture);
    config.assets = Some(repo_assets());
    let mut meter = Meter::start(config);
    assert_eq!(meter.record(Command::StartRecord), None);
    let deadline = Instant::now() + Duration::from_secs(120);
    while meter.handle().metrics().frames < total {
        assert!(Instant::now() < deadline, "{:?}", meter.handle().metrics());
        std::thread::sleep(Duration::from_millis(50));
    }
    meter.settle();
    let metrics = meter.handle().metrics();
    meter.shutdown();
    println!("replayed {total} frames at {rate:.0}/s: {metrics:?}");
    let engine = metrics.packet_to_engine;
    let publish = metrics.packet_to_publish;
    table(&[
        ("Packet → engine p99", "< 2 ms", format!("{} (p50 {}, p99.9 {}, max {}, n {})", ms(engine.p99_us), ms(engine.p50_us), ms(engine.p999_us), ms(engine.max_us), engine.count), engine.p99_us < 2_000),
        ("Packet → publish p99", "< 300 ms", format!("{} (p50 {}, max {}, n {})", ms(publish.p99_us), ms(publish.p50_us), ms(publish.max_us), publish.count), publish.p99_us < 300_000),
        ("Dropped game packets", "0", metrics.channel_dropped.to_string(), metrics.channel_dropped == 0),
    ]);
    assert_eq!(engine.count, total);
    if TIMED {
        assert!(engine.p99_us < 2_000, "{engine:?}");
        assert!(publish.p99_us < 300_000, "{publish:?}");
    }
    assert_eq!(metrics.channel_dropped, 0);
    let _ = std::fs::remove_dir_all(&directory);
}

#[cfg(target_os = "linux")]
mod process {
    pub fn rss_mb() -> f64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let line = status.lines().find(|line| line.starts_with("VmRSS:")).unwrap();
        line.split_whitespace().nth(1).unwrap().parse::<f64>().unwrap() / 1024.0
    }
    /// User + system CPU seconds of this process.
    pub fn cpu_seconds() -> f64 {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        let fields: Vec<&str> = stat[stat.rfind(')').unwrap() + 2..].split_whitespace().collect();
        let ticks: f64 = fields[11].parse::<f64>().unwrap() + fields[12].parse::<f64>().unwrap();
        ticks / 100.0
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "soak: minutes of synthetic raid traffic"]
fn soak() {
    let minutes: f64 = std::env::var("SOAK_MINUTES").ok().and_then(|value| value.parse().ok()).unwrap_or(10.0);
    let fast: f64 = std::env::var("SOAK_PPS").ok().and_then(|value| value.parse().ok()).unwrap_or(2000.0);
    let real = fast / 5.0;
    let total = minutes * 60.0;
    let (idle_seconds, real_seconds) = (60.0f64.min(total / 5.0), 120.0f64.min(total / 5.0));
    let directory = temp_dir("soak");
    let capture = Arc::new(FakeCapture::default());
    let idle = capture.source();
    let emitted = Arc::new(AtomicU64::new(0));
    let rate = Arc::new(AtomicU64::new(real.to_bits()));
    let (setup, templates) = raid();
    capture.custom(Box::new({
        let (emitted, rate) = (emitted.clone(), rate.clone());
        move || Ok(paced(setup, templates, rate, false, emitted))
    }));
    let mut config = meter_runtime::Config::new(&directory, capture);
    config.assets = Some(repo_assets());
    let mut meter = Meter::start(config);
    meter.events.keep(200);
    let mut samples: Vec<(&str, f64, f64)> = vec![];
    // Runs a phase, sampling RSS every 10 s; returns its CPU % of one core.
    let mut phase = |label: &'static str, seconds: f64, meter: &Meter| {
        let begun = (Instant::now(), process::cpu_seconds());
        while begun.0.elapsed().as_secs_f64() < seconds {
            std::thread::sleep(Duration::from_secs_f64(10.0f64.min(seconds - begun.0.elapsed().as_secs_f64()).max(0.01)));
            let elapsed = begun.0.elapsed().as_secs_f64();
            let (rss, cpu) = (process::rss_mb(), (process::cpu_seconds() - begun.1) / elapsed * 100.0);
            let metrics = meter.handle().metrics();
            samples.push((label, elapsed, rss));
            println!("{label:<5} t={elapsed:>5.0}s rss={rss:>5.1} MB cpu={cpu:>5.2}% frames={} dropped={} report p50={}us", metrics.frames, metrics.channel_dropped, metrics.report.p50_us);
        }
        (process::cpu_seconds() - begun.1) / begun.0.elapsed().as_secs_f64() * 100.0
    };

    // Idle: capture on, no game traffic, no record.
    meter.start_capture();
    std::thread::sleep(Duration::from_secs(2));
    let idle_cpu = phase("idle", idle_seconds, &meter);
    // The idle source ends, the capture reconnects to the raid source and Play starts the record: first at the pace of
    // a real 20-player fight, then at five times that.
    let from = meter.events.len();
    drop(idle);
    meter.events.wait_capture(from, "completed");
    meter.events.wait_capture(from, "running");
    assert_eq!(meter.record(Command::StartRecord), None);
    let real_cpu = phase("1x", real_seconds, &meter);
    rate.store(fast.to_bits(), Ordering::Relaxed);
    let fast_cpu = phase("5x", total - idle_seconds - real_seconds, &meter);
    let metrics = meter.handle().metrics();
    let sent = emitted.load(Ordering::Relaxed);
    meter.shutdown();
    let rss = |label: &str| samples.iter().filter(|sample| sample.0 == label).map(|sample| sample.2).fold(0.0, f64::max);
    let fast_samples: Vec<f64> = samples.iter().filter(|sample| sample.0 == "5x").map(|sample| sample.2).collect();
    let half = fast_samples.len() / 2;
    let growth = fast_samples[half..].iter().copied().fold(0.0, f64::max) - fast_samples[..half].iter().copied().fold(0.0, f64::max);
    let (engine, publish, report, tick) = (metrics.packet_to_engine, metrics.packet_to_publish, metrics.report, metrics.tick);
    println!();
    println!("{minutes} min: {idle_seconds} s idle, {real_seconds} s at {real} packets/s, the rest at {fast} packets/s; {sent} frames sent, {} taken, {} snapshots", metrics.frames, metrics.publishes);
    println!("engine thread: report rebuild p50 {} p99 {} max {} (n {}); tick p50 {} p99 {} max {}", ms(report.p50_us), ms(report.p99_us), ms(report.max_us), report.count, ms(tick.p50_us), ms(tick.p99_us), ms(tick.max_us));
    table(&[
        ("Packet → engine p99", "< 2 ms", format!("{} (p50 {}, p99.9 {}, max {})", ms(engine.p99_us), ms(engine.p50_us), ms(engine.p999_us), ms(engine.max_us)), engine.p99_us < 2_000),
        ("Packet → publish p99", "< 300 ms", format!("{} (p50 {}, max {})", ms(publish.p99_us), ms(publish.p50_us), ms(publish.max_us)), publish.p99_us < 300_000),
        ("Dropped game packets at 5×", "0", metrics.channel_dropped.to_string(), metrics.channel_dropped == 0),
        ("RAM idle", "≤ 60 MB", format!("{:.1} MB", rss("idle")), rss("idle") <= 60.0),
        ("RAM in the fight (peak)", "≤ 90 MB", format!("{:.1} MB at 1×, {:.1} MB at 5× (+{growth:.1} MB over its second half)", rss("1x"), rss("5x")), rss("5x") <= 90.0),
        ("CPU idle", "< 1 %", format!("{idle_cpu:.2} %"), idle_cpu < 1.0),
        ("CPU 20-player fight", "< 5 %", format!("{real_cpu:.2} % at 1× ({fast_cpu:.2} % at 5×), traffic generator included"), real_cpu < 5.0),
    ]);
    assert_eq!(metrics.channel_dropped, 0);
    // A few frames are still in the channel when the counters are read.
    assert!(metrics.frames + 64 >= sent, "{} of {sent} frames taken", metrics.frames);
    let _ = std::fs::remove_dir_all(&directory);
}

/// Where the fight's memory goes: the same traffic straight into one `CaptureAnalysis`, without the runtime.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "probe"]
fn memory_probe_analysis_alone() {
    let (setup, templates) = raid();
    let sources = meter_core::CombatSourceCatalog::load(repo_assets().join("combat.json")).map(Arc::new);
    let mut analysis = meter_core::CaptureAnalysis::new(sources, true);
    let start = wall_us();
    analysis.begin_capture(Some(start), false, false);
    let mut source = Paced { setup, templates, rate: Arc::new(AtomicU64::new(1e12f64.to_bits())), current: 0.0, started: None, emitted: Arc::default(), sent: 0, sequence: 0, random: 0x9e37_79b9_7f4a_7c15, once: false, next: 0 };
    let mut frame = CaptureFrame::default();
    let mut started = false;
    let frames: u64 = std::env::var("PROBE_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(230_000);
    let report_each = std::env::var("PROBE_REPORT").map_or(true, |v| v != "0");
    for n in 0..frames {
        source.read(&mut frame).unwrap();
        frame.timestamp_us = start + n as i64 * 500;
        analysis.add(&frame);
        if n == 30 && !started {
            analysis.start_record(frame.timestamp_us).unwrap();
            started = true;
        }
        if report_each && n % 2000 == 0 {
            let _ = analysis.snapshot();
        }
        if n % 23_000 == 0 {
            println!("frames={n} rss={:.1} MB", process::rss_mb());
        }
    }
    println!("frames={frames} rss={:.1} MB", process::rss_mb());
}
