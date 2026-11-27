//! The real-time gate "0 allocations per packet in steady state on the decode path", measured with a counting global
//! allocator: frames are built first, then fed to a warmed-up analysis while every heap allocation is counted.

mod common;

use common::*;
use meter_core::CaptureAnalysis;
use meter_core::testkit::{message_frame, request_frame};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
    static TRACING: Cell<bool> = const { Cell::new(false) };
}

/// `ALLOC_TRACE=n` prints where the first n counted allocations come from.
fn trace() {
    TRACING.with(|t| t.set(true));
    let limit: usize = std::env::var("ALLOC_TRACE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    if COUNT.with(Cell::get) > limit {
        TRACING.with(|t| t.set(false));
        return;
    }
    let text = std::backtrace::Backtrace::force_capture().to_string();
    let frames: Vec<&str> = text.lines().filter(|l| l.contains("meter_core::") && !l.contains("allocations::")).take(3).collect();
    eprintln!("ALLOC {}", frames.join(" <- "));
    TRACING.with(|t| t.set(false));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.try_with(Cell::get).unwrap_or(false) && !TRACING.try_with(Cell::get).unwrap_or(true) {
            COUNT.with(|count| count.set(count.get() + 1));
            trace();
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNTING.try_with(Cell::get).unwrap_or(false) && !TRACING.try_with(Cell::get).unwrap_or(true) {
            COUNT.with(|count| count.set(count.get() + 1));
            trace();
        }
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn allocations(work: impl FnOnce()) -> usize {
    COUNT.with(|count| count.set(0));
    COUNTING.with(|counting| counting.set(true));
    work();
    COUNTING.with(|counting| counting.set(false));
    COUNT.with(Cell::get)
}

struct Traffic {
    sequence: u32,
    client: u32,
}

impl Traffic {
    fn server(&mut self, message: WireMessage<'static>, at: i64) -> meter_core::CaptureFrame {
        self.sequence += 1;
        message_frame(&message, at, self.sequence)
    }
    fn client(&mut self, message: WireMessage<'static>, at: i64) -> meter_core::CaptureFrame {
        self.client += 1;
        request_frame(&message, at, self.client)
    }
}

/// A party on a map: the local player, two named mates with gear, and a party list.
fn warmed_up(map: &str, sources: Option<std::sync::Arc<meter_core::CombatSourceCatalog>>) -> (CaptureAnalysis, Traffic) {
    let (me, mate, healer) = (new_id(), new_id(), new_id());
    let mut analysis = CaptureAnalysis::new(sources, false);
    let mut traffic = Traffic { sequence: 0, client: 0 };
    let setup = [
        resp(p![253 => 2u8, 0 => 10, 1 => me, 2 => "Alpha", 8 => map, 66 => "1315"]),
        ev(p![252 => 230i16, 8 => guid_bytes(&[me, mate, healer]), 9 => strings(&["Alpha", "Mate", "Healer"])]),
        ev(p![252 => 29u8, 0 => 402, 1 => "Mate", 7 => mate, 40 => [7346i16, 0, 5347, 5005, 5397, 2900, 2662, 3098]]),
        ev(p![252 => 90u8, 0 => 10, 2 => [7346i16, 0, 5347, 5005, 5397, 2900, 2662, 3098], 7 => [3338i16, 3339]]),
        ev(p![252 => 29u8, 0 => 403, 1 => "Healer", 7 => healer, 40 => [7587i16, 0, 5347, 5005, 5397, 2900, 2662, 3098]]),
    ];
    for (n, message) in setup.into_iter().enumerate() {
        let frame = traffic.server(message, 1_000_000 + n as i64 * 1000);
        analysis.add(&frame);
    }
    (analysis, traffic)
}

/// Movement, buffs, regeneration and own requests: the bulk of the game's traffic, none of it combat.
fn background(traffic: &mut Traffic, from: i64, count: i64) -> Vec<meter_core::CaptureFrame> {
    (0..count)
        .map(|n| {
            let at = from + n * 1_000;
            match n % 4 {
                0 => traffic.server(ev(p![252 => 3u8, 0 => 402, 1 => vec![7u8; 30], 2 => 1.5f32]), at),
                1 => traffic.server(ev(p![252 => 91i16, 0 => 403, 1 => 1200, 2 => "BUFF_NAME", 3 => [1i16, 2, 3]]), at),
                2 => traffic.client(req(p![253 => 21u8, 0 => 10, 1 => vec![1u8; 12]]), at),
                _ => traffic.server(ev(p![252 => 40u8, 0 => 30, 1 => 7i64, 2 => Value::Null, 3 => true]), at),
            }
        })
        .collect()
}

/// Outside combat no record is running, so this measures the whole decode path: frame, IP, Photon, Protocol18 and
/// every observation the message reaches.
#[test]
fn the_decode_path_allocates_nothing_per_packet_in_steady_state() {
    let (mut analysis, mut traffic) = warmed_up("1301", None);
    let warm = background(&mut traffic, 2_000_000, 2_000);
    for frame in &warm {
        analysis.add(frame);
    }
    let measured = background(&mut traffic, 4_100_000, 400);
    let count = allocations(|| {
        for frame in &measured {
            analysis.add(frame);
        }
    });
    assert_eq!(count, 0, "{count} allocations for {} packets", measured.len());
}

/// In a fight the record keeps what it counts: event ids for dedup, one timeline bucket per second and each
/// contribution's first entry in a bucket, and the freezing of buckets two seconds old. They follow the record's
/// seconds, not its hits: the bound is a few per elapsed second plus a few hundredths per hit.
#[test]
fn combat_packets_allocate_only_what_the_record_keeps() {
    combat_allocations(None);
}

/// The same with the game's item catalog resolving every effect to the item that cast it.
#[test]
fn combat_packets_resolve_their_sources_without_allocating() {
    let catalog = meter_core::CombatSourceCatalog::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../assets/game-data/combat.json"));
    combat_allocations(Some(std::sync::Arc::new(catalog.unwrap())));
}

fn combat_allocations(sources: Option<std::sync::Arc<meter_core::CombatSourceCatalog>>) {
    let (mut analysis, mut traffic) = warmed_up("@RANDOMDUNGEON@one", sources);
    let hits = |traffic: &mut Traffic, from: i64, count: i64| -> Vec<meter_core::CaptureFrame> {
        (0..count)
            .map(|n| {
                let source = [10, 402, 403][n as usize % 3];
                let delta = if n % 5 == 0 { 40f32 } else { -120f32 };
                let spell = [-1, 3338, 3404][n as usize % 3 % 2 + (n as usize / 7) % 2];
                traffic.server(ev(p![252 => 6u8, 0 => 30, 6 => source, 2 => delta, 7 => spell, 5 => 0u8]), from + n * 2_000)
            })
            .collect()
    };
    for frame in &hits(&mut traffic, 2_000_000, 3_000) {
        analysis.add(frame);
    }
    let measured = hits(&mut traffic, 8_002_000, 450);
    let count = allocations(|| {
        for frame in &measured {
            analysis.add(frame);
        }
    });
    let per_packet = count as f64 / measured.len() as f64;
    let seconds = (measured.last().unwrap().timestamp_us - measured[0].timestamp_us) as f64 / 1e6 + 1.0;
    println!("combat: {count} allocations for {} hits over {seconds:.1} record seconds ({per_packet:.3} per hit)", measured.len());
    assert!(count as f64 <= 0.02 * measured.len() as f64 + 40.0 * seconds, "{count} allocations");
}
