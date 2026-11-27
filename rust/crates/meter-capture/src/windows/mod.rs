//! Windows-only capture: raw sockets (`RawSocketCapture`, `AllAdaptersCapture`), loading Npcap, and the firewall rule.

pub mod firewall;
#[cfg(feature = "pcap")]
pub mod npcap;
pub mod raw;
