//! `NativePcap`: live capture through Npcap (Windows) or libpcap (Linux), filtered in the driver.

use std::ffi::{CStr, c_char};

use crate::{
    AdapterStatistics, CaptureDevice, CaptureError, CaptureFrame, DEFAULT_FILTER, FrameSource, Read,
};

const SNAPSHOT_LENGTH: i32 = 262_144;
const TIMEOUT_MS: i32 = 100;

unsafe extern "C" {
    fn pcap_lib_version() -> *const c_char;
}

pub struct NativePcap {
    capture: pcap::Capture<pcap::Active>,
    link_type: i32,
}

/// On Windows the library is Npcap's wpcap.dll, which may be missing; nothing may call into it before this says yes.
pub(crate) fn available() -> Result<(), CaptureError> {
    #[cfg(windows)]
    if !crate::windows::npcap::load() {
        return Err(CaptureError::Io("Npcap is not installed.".into()));
    }
    Ok(())
}

fn error(error: pcap::Error) -> CaptureError {
    match error {
        pcap::Error::PcapError(message) => CaptureError::Io(message),
        other => CaptureError::Io(other.to_string()),
    }
}

pub fn version() -> Result<String, CaptureError> {
    available()?;
    // SAFETY: libpcap returns a static NUL-terminated string.
    Ok(unsafe { CStr::from_ptr(pcap_lib_version()) }
        .to_string_lossy()
        .into_owned())
}

pub fn devices() -> Result<Vec<CaptureDevice>, CaptureError> {
    available()?;
    let devices = pcap::Device::list().map_err(error)?;
    Ok(devices
        .into_iter()
        .map(|device| {
            let status = match device.flags.connection_status {
                pcap::ConnectionStatus::Unknown => 0,
                pcap::ConnectionStatus::Connected => 0x10,
                pcap::ConnectionStatus::Disconnected => 0x20,
                pcap::ConnectionStatus::NotApplicable => 0x30,
            };
            CaptureDevice::new(
                device.name,
                device.desc.unwrap_or_default(),
                device.flags.if_flags.bits() | status,
            )
        })
        .collect())
}

pub fn open_device(name: &str) -> Result<NativePcap, CaptureError> {
    if !devices()?.iter().any(|device| device.name == name) {
        return Err(CaptureError::InvalidDevice(
            "Select an available capture interface.".into(),
        ));
    }
    // A blocking read with a short timeout, delivering packets as they come: idle costs nothing and a stop is
    // noticed within TIMEOUT_MS.
    let mut capture = pcap::Capture::from_device(name)
        .map_err(error)?
        .snaplen(SNAPSHOT_LENGTH)
        .promisc(false)
        .timeout(TIMEOUT_MS)
        .immediate_mode(true)
        .open()
        .map_err(error)?;
    capture.filter(DEFAULT_FILTER, true).map_err(error)?;
    let link_type = capture.get_datalink().0;
    Ok(NativePcap { capture, link_type })
}

impl FrameSource for NativePcap {
    fn link_type(&self) -> i32 {
        self.link_type
    }

    fn read(&mut self, frame: &mut CaptureFrame) -> Result<Read, CaptureError> {
        let packet = match self.capture.next_packet() {
            Ok(packet) => packet,
            Err(pcap::Error::TimeoutExpired) => return Ok(Read::Idle),
            Err(pcap::Error::NoMorePackets) => return Ok(Read::Ended),
            Err(other) => return Err(error(other)),
        };
        let header = packet.header;
        // timeval fields are 32-bit on Windows.
        #[allow(clippy::useless_conversion)]
        let (seconds, micros) = (i64::from(header.ts.tv_sec), i64::from(header.ts.tv_usec));
        if header.caplen as usize > SNAPSHOT_LENGTH as usize
            || header.caplen > header.len
            || !(0..1_000_000).contains(&micros)
            || packet.data.len() != header.caplen as usize
        {
            return Err(CaptureError::Io(
                "The capture contains an invalid packet header.".into(),
            ));
        }
        frame.timestamp_us = seconds
            .checked_mul(1_000_000)
            .and_then(|stamp| stamp.checked_add(micros))
            .ok_or_else(|| {
                CaptureError::Io("The capture contains an invalid packet header.".into())
            })?;
        frame.original_length = header.len;
        frame.link_type = self.link_type;
        frame.data.clear();
        frame.data.extend_from_slice(packet.data);
        Ok(Read::Frame)
    }

    fn statistics(&mut self) -> Option<AdapterStatistics> {
        self.capture.stats().ok().map(|stat| AdapterStatistics {
            received: stat.received,
            dropped: stat.dropped,
            interface_dropped: stat.if_dropped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_version_is_reported() {
        // A Windows machine without Npcap (a CI runner) has no library to ask.
        if available().is_err() {
            return;
        }
        assert!(version().unwrap().to_ascii_lowercase().contains("pcap"));
    }

    #[test]
    fn unknown_devices_are_refused() {
        assert!(open_device("not-an-interface").is_err());
    }
}
