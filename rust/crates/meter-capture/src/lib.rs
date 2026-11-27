//! Raw link-layer frames from the network: live sources (Npcap/libpcap, Linux AF_PACKET) and pcap files.

/// One captured link-layer frame, as the C# `CaptureFrame` record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureFrame {
    pub timestamp_us: i64,
    pub original_length: u32,
    pub link_type: i32,
    pub data: Vec<u8>,
}
