//! `cargo run -p meter-core --example synth -- <directory>`: writes the synthetic captures of
//! `meter_core::testkit::scenarios`, for `rust/scripts/diff-replay.sh` until real captures are available.

use meter_core::capture::pcap::write_pcap;
use std::path::Path;

fn main() {
    let directory = std::env::args().nth(1).unwrap_or_else(|| "synthetic".into());
    let directory = Path::new(&directory);
    std::fs::create_dir_all(directory).unwrap();
    for (name, frames) in meter_core::testkit::scenarios::all() {
        std::fs::write(directory.join(format!("{name}.pcap")), write_pcap(&frames, 101)).unwrap();
    }
}
