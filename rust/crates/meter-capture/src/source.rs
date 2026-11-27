use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::CaptureFrame;

/// LINKTYPE_RAW: the frame starts at the IP header (system sockets).
pub const LINK_TYPE_RAW: i32 = 101;

/// `AdapterStatistics`: packets the source took and the ones the OS buffer or the interface dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterStatistics {
    pub received: u32,
    pub dropped: u32,
    pub interface_dropped: u32,
}

/// What one `read` produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    /// `frame` holds a packet.
    Frame,
    /// Nothing arrived within the source's poll timeout (≤ 100 ms), so the caller can check for cancellation.
    Idle,
    /// The source ended by itself (a file, or an interface that went away).
    Ended,
}

/// A live or file capture source, as the C# `ICaptureSource`.
pub trait FrameSource: Send {
    fn link_type(&self) -> i32;
    /// Fills `frame`, reusing its `data` buffer. Never blocks longer than the source's poll timeout.
    fn read(&mut self, frame: &mut CaptureFrame) -> Result<Read, CaptureError>;
    fn statistics(&mut self) -> Option<AdapterStatistics>;
    /// Whether the kernel/driver filters to game packets; `false` means every packet is checked in user space.
    fn kernel_filtered(&self) -> bool {
        true
    }
}

/// Capture errors with the C# messages, which the UI shows as they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    /// `ArgumentException`: the device is not one of `devices()`.
    InvalidDevice(String),
    /// `IOException` / `InvalidDataException`.
    Io(String),
    /// `PlatformNotSupportedException`.
    Unsupported(String),
}

impl CaptureError {
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidDevice(message) | Self::Io(message) | Self::Unsupported(message) => {
                message
            }
        }
    }

    /// `CaptureService` stops retrying on these: the capture does not come back by itself without permission.
    pub fn is_permission(&self) -> bool {
        let message = self.message();
        message.to_ascii_lowercase().contains("permission") || message.contains("CAP_NET_RAW")
    }

    /// `{message}: {OS error text}`, the C# `Error(message)` helpers.
    pub(crate) fn os(message: &str, error: &std::io::Error) -> Self {
        Self::Io(format!("{message}: {}", os_text(error)))
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for CaptureError {}

/// The OS error text without Rust's " (os error N)" suffix, as `Win32Exception.Message` gives it.
pub(crate) fn os_text(error: &std::io::Error) -> String {
    let text = error.to_string();
    match text.find(" (os error ") {
        Some(end) => text[..end].to_owned(),
        None => text,
    }
}

pub(crate) fn now_us() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_micros() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_errors_are_recognised() {
        assert!(
            CaptureError::Io(
                "Socket capture permission denied; CAP_NET_RAW may be required: x".into()
            )
            .is_permission()
        );
        assert!(
            CaptureError::Io(
                "Packet capture permission denied: run Albion Meter as administrator.".into()
            )
            .is_permission()
        );
        assert!(!CaptureError::Io("Socket receive failed: x".into()).is_permission());
    }

    #[cfg(unix)]
    #[test]
    fn os_text_drops_the_code() {
        let error = std::io::Error::from_raw_os_error(libc::EPERM);
        assert_eq!(os_text(&error), "Operation not permitted");
    }
}
