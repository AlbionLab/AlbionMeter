//! Writes meter-core's synthetic captures (solo, party, transport, a 30-minute raid) as pcap files, for replaying a
//! fight through the real meter: `cargo run -p meter-app --example synthetic-pcap -- <folder>`.

use meter_core::capture::pcap::write_pcap;
use meter_core::testkit::scenarios;

fn main() {
    let folder = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&folder).expect("the folder is created");
    for (name, frames) in [("solo", scenarios::solo()), ("party", scenarios::party()), ("transport", scenarios::transport()), ("stress-30min", scenarios::stress(30))] {
        let path = folder.join(format!("{name}.pcap"));
        std::fs::write(&path, write_pcap(&frames, 101)).expect("the capture is written");
        println!("{} ({} frames)", path.display(), frames.len());
    }
}
