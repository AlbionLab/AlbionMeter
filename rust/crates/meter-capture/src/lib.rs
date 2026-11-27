//! Raw link-layer frames from the network: live sources (Npcap/libpcap, Linux AF_PACKET, Windows raw sockets),
//! the capture thread that hands them to the engine, and pcap recording.

mod devices;
pub mod elevation;
mod filter;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(feature = "pcap")]
pub mod native_pcap;
mod source;
pub mod storage;
mod thread;
#[cfg(windows)]
pub mod windows;
mod writer;

pub use devices::{
    DEFAULT_ROUTE_FLAG, DeviceEntry, DeviceList, MethodEntry, RAW_SOCKET_PREFIX, default_device,
    device_list, devices, library, open, validate_start,
};
pub use filter::{DEFAULT_FILTER, LINUX_FILTER, is_albion_packet};
pub use source::{AdapterStatistics, CaptureError, FrameSource, LINK_TYPE_RAW, Read};
pub use thread::{CaptureCounters, CaptureExit, CaptureThread, ExitReason};
pub use writer::PcapWriter;

/// One captured link-layer frame, as the C# `CaptureFrame` record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureFrame {
    pub timestamp_us: i64,
    pub original_length: u32,
    pub link_type: i32,
    pub data: Vec<u8>,
}

/// A capture interface, as the C# `CaptureDevice` record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureDevice {
    pub name: String,
    pub description: String,
    pub flags: u32,
}

impl CaptureDevice {
    fn new(name: impl Into<String>, description: impl Into<String>, flags: u32) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            flags,
        }
    }
}
