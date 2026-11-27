//! `cargo run -p meter-core --example replay -- <pcap>`: the CaptureReport JSON of a capture, like
//! `tools/AlbionMeter.Capture replay` (and `replay-session <pcap> [context…]`). `summary <pcap>` prints only the
//! totals the live UI reads (through the incremental delta), for timing and memory measurements.

use meter_core::capture::pcap::PcapFile;
use meter_core::{CaptureAnalysis, CaptureFrame, CombatSourceCatalog};
use std::process::ExitCode;
use std::sync::Arc;

fn feed(analysis: &mut CaptureAnalysis, path: &str, mut first: impl FnMut(&mut CaptureAnalysis, &CaptureFrame)) -> std::io::Result<()> {
    let mut file = PcapFile::open(path)?;
    let mut frame = CaptureFrame { timestamp_us: 0, original_length: 0, link_type: 0, data: Vec::new() };
    while file.read_into(&mut frame)? {
        first(analysis, &frame);
        analysis.add(&frame);
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let catalog = std::env::var("ALBION_COMBAT_JSON")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../../assets/game-data/combat.json").into());
    let sources = CombatSourceCatalog::load(catalog).map(Arc::new);
    let mut analysis = CaptureAnalysis::new(sources, false);
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["summary", file] => {
            if let Err(error) = feed(&mut analysis, file, |_, _| {}) {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
            let delta = analysis.combat_delta(-1).unwrap();
            let combat = delta.snapshot;
            println!("sequence {} damage {} healing {} events {} points {}", delta.sequence, combat.total_damage, combat.total_healing, combat.event_count, combat.timeline.len());
            return ExitCode::SUCCESS;
        }
        ["replay", file] | [file] if file != "replay-session" => feed(&mut analysis, file, |_, _| {}),
        ["replay-session", session, ref context @ ..] => (|| {
            for file in context {
                feed(&mut analysis, file, |_, _| {})?;
            }
            let mut started = false;
            feed(&mut analysis, session, |analysis, frame| {
                if !started {
                    analysis.begin_capture(Some(frame.timestamp_us), false, false);
                    started = true;
                }
            })
        })(),
        _ => {
            eprintln!("Usage: replay [replay] <pcap-file> | replay-session <pcap-file> [context-files...]");
            return ExitCode::from(2);
        }
    };
    if let Err(error) = result {
        eprintln!("{}", serde_json::json!({ "error": error.to_string(), "combatDecoded": false }));
        return ExitCode::from(1);
    }
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    serde_json::to_writer_pretty(&mut out, &analysis.snapshot()).unwrap();
    use std::io::Write;
    writeln!(out).unwrap();
    ExitCode::SUCCESS
}
