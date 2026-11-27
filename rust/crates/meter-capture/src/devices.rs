//! `PacketCapture` device listing and opening, and the `/api/capture/devices` answer the UI picks from.

use serde::Serialize;

use crate::{CaptureDevice, CaptureError, FrameSource};

pub const RAW_SOCKET_PREFIX: &str = "raw:";
pub const DEFAULT_ROUTE_FLAG: u32 = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Platform {
    Windows,
    Linux,
    Other,
}

const PLATFORM: Platform = if cfg!(windows) {
    Platform::Windows
} else if cfg!(target_os = "linux") {
    Platform::Linux
} else {
    Platform::Other
};

/// System sockets first (Windows: every adapter, then one per IPv4 address; Linux: `raw:any`, then each interface),
/// then the Npcap/libpcap devices when the library loads.
pub fn devices() -> Vec<CaptureDevice> {
    let mut devices = vec![];
    #[cfg(windows)]
    devices.extend(crate::windows::raw::devices_with_all());
    #[cfg(target_os = "linux")]
    devices.extend(crate::linux::devices());
    #[cfg(feature = "pcap")]
    devices.extend(crate::native_pcap::devices().unwrap_or_default());
    devices
}

/// The device a fresh install captures on.
pub fn default_device(devices: &[CaptureDevice]) -> Option<String> {
    default_device_on(PLATFORM, devices)
}

fn default_device_on(platform: Platform, devices: &[CaptureDevice]) -> Option<String> {
    let has = |name: &str| devices.iter().any(|device| device.name == name);
    match platform {
        Platform::Linux if has("raw:any") => Some("raw:any".into()),
        // Every adapter, so the game is still seen when ExitLag, NoPing or a VPN moves it to their own adapter.
        Platform::Windows if has("raw:all") => Some("raw:all".into()),
        Platform::Windows => {
            let sockets = || {
                devices
                    .iter()
                    .filter(|device| device.name.starts_with(RAW_SOCKET_PREFIX))
            };
            sockets()
                .find(|device| device.flags & DEFAULT_ROUTE_FLAG != 0)
                .or_else(|| sockets().next())
                .map(|device| device.name.clone())
        }
        _ => None,
    }
}

pub fn open(device: &str) -> Result<Box<dyn FrameSource>, CaptureError> {
    if device.starts_with(RAW_SOCKET_PREFIX) {
        #[cfg(windows)]
        return crate::windows::raw::open(device);
        #[cfg(target_os = "linux")]
        return Ok(Box::new(crate::linux::open(device)?));
        #[allow(unreachable_code)]
        return Err(CaptureError::Unsupported(
            "Socket capture is available on Windows and Linux.".into(),
        ));
    }
    #[cfg(feature = "pcap")]
    return Ok(Box::new(crate::native_pcap::open_device(device)?));
    #[cfg(not(feature = "pcap"))]
    Err(CaptureError::Unsupported(
        "This build captures through system sockets only.".into(),
    ))
}

/// What the status line shows as the capture library.
pub fn library(device: Option<&str>) -> String {
    if device.is_some_and(|device| device.starts_with(RAW_SOCKET_PREFIX)) {
        return if PLATFORM == Platform::Linux {
            "Linux packet socket"
        } else {
            "Windows raw socket"
        }
        .into();
    }
    #[cfg(feature = "pcap")]
    if let Ok(version) = crate::native_pcap::version() {
        return version;
    }
    if PLATFORM == Platform::Windows {
        "Windows raw socket"
    } else {
        "unavailable"
    }
    .into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceList {
    pub available: bool,
    pub library: Option<String>,
    /// The default device; the app puts the player's own choice here when there is one.
    pub selected_device: Option<String>,
    pub methods: Vec<MethodEntry>,
    pub devices: Vec<DeviceEntry>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MethodEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEntry {
    pub name: String,
    pub description: String,
    pub flags: u32,
    pub method: &'static str,
}

fn method(device: &str) -> &'static str {
    if device.starts_with(RAW_SOCKET_PREFIX) {
        "socket"
    } else {
        "pcap"
    }
}

/// `GET /api/capture/devices`.
pub fn device_list() -> DeviceList {
    let devices = devices();
    list_on(
        PLATFORM,
        &devices,
        library(devices.first().map(|device| device.name.as_str())),
    )
}

fn list_on(platform: Platform, devices: &[CaptureDevice], library: String) -> DeviceList {
    let any = |wanted: &str| devices.iter().any(|device| method(&device.name) == wanted);
    DeviceList {
        available: !devices.is_empty(),
        library: Some(library),
        selected_device: default_device_on(platform, devices),
        methods: vec![
            MethodEntry {
                id: "socket",
                name: if platform == Platform::Linux {
                    "System socket (Linux)"
                } else {
                    "System socket (Windows)"
                },
                available: any("socket"),
            },
            MethodEntry {
                id: "pcap",
                name: if platform == Platform::Windows {
                    "Npcap"
                } else {
                    "libpcap"
                },
                available: any("pcap"),
            },
        ],
        devices: devices
            .iter()
            .map(|device| DeviceEntry {
                name: device.name.clone(),
                description: device.description.clone(),
                flags: device.flags,
                method: method(&device.name),
            })
            .collect(),
        error: devices
            .is_empty()
            .then(|| "No capture interface is available.".into()),
    }
}

/// `POST /api/capture/start` request checks: a device, and a method (when given) that matches it.
pub fn validate_start(device: &str, method_id: Option<&str>) -> Result<(), String> {
    if device.trim().is_empty() || device.len() > 1024 {
        return Err("Select a capture interface.".into());
    }
    if let Some(id) = method_id
        && (!matches!(id, "socket" | "pcap") || id != method(device))
    {
        return Err("The selected interface does not match the capture method.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, flags: u32) -> CaptureDevice {
        CaptureDevice::new(name, "", flags)
    }

    #[test]
    fn linux_prefers_raw_any() {
        let devices = [
            device("raw:any", 1),
            device("raw:eth0", 1),
            device("eth0", 6),
        ];
        assert_eq!(
            default_device_on(Platform::Linux, &devices).as_deref(),
            Some("raw:any")
        );
        assert_eq!(default_device_on(Platform::Linux, &devices[2..]), None);
    }

    #[test]
    fn windows_prefers_all_adapters_then_the_routed_socket() {
        let all = [
            device("raw:all", 1),
            device("raw:10.0.0.2", 1),
            device(r"\Device\NPF_{X}", 6),
        ];
        assert_eq!(
            default_device_on(Platform::Windows, &all).as_deref(),
            Some("raw:all")
        );
        let sockets = [device("raw:172.16.0.1", 0), device("raw:10.0.0.2", 1)];
        assert_eq!(
            default_device_on(Platform::Windows, &sockets).as_deref(),
            Some("raw:10.0.0.2")
        );
        assert_eq!(
            default_device_on(Platform::Windows, &sockets[..1]).as_deref(),
            Some("raw:172.16.0.1")
        );
        assert_eq!(default_device_on(Platform::Windows, &all[2..]), None);
        assert_eq!(default_device_on(Platform::Other, &all), None);
    }

    #[test]
    fn device_list_matches_the_endpoint() {
        let devices = [
            CaptureDevice::new("raw:any", "All interfaces (Linux socket)", 1),
            CaptureDevice::new("wlan0", "", 0x16),
        ];
        let list = list_on(Platform::Linux, &devices, "Linux packet socket".into());
        assert_eq!(
            serde_json::to_value(&list).unwrap(),
            serde_json::json!({
                "available": true,
                "library": "Linux packet socket",
                "selectedDevice": "raw:any",
                "methods": [
                    { "id": "socket", "name": "System socket (Linux)", "available": true },
                    { "id": "pcap", "name": "libpcap", "available": true }
                ],
                "devices": [
                    { "name": "raw:any", "description": "All interfaces (Linux socket)", "flags": 1, "method": "socket" },
                    { "name": "wlan0", "description": "", "flags": 0x16, "method": "pcap" }
                ],
                "error": null
            })
        );
        let empty = list_on(Platform::Windows, &[], "Windows raw socket".into());
        assert!(!empty.available && !empty.methods[0].available && !empty.methods[1].available);
        assert_eq!(empty.methods[1].name, "Npcap");
        assert_eq!(
            empty.error.as_deref(),
            Some("No capture interface is available.")
        );
    }

    #[test]
    fn start_requests_are_checked() {
        assert_eq!(
            validate_start(" ", None),
            Err("Select a capture interface.".into())
        );
        assert_eq!(
            validate_start(&"x".repeat(1025), None),
            Err("Select a capture interface.".into())
        );
        assert_eq!(validate_start("raw:any", Some("socket")), Ok(()));
        assert_eq!(validate_start("eth0", Some("pcap")), Ok(()));
        assert!(validate_start("raw:any", Some("pcap")).is_err());
        assert!(validate_start("eth0", Some("socket")).is_err());
        assert!(validate_start("eth0", Some("npcap")).is_err());
    }

    #[test]
    fn library_names_the_socket_kind() {
        let expected = if cfg!(target_os = "linux") {
            "Linux packet socket"
        } else {
            "Windows raw socket"
        };
        assert_eq!(library(Some("raw:any")), expected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_machine_lists_raw_any_first_and_prefers_it() {
        let devices = devices();
        assert_eq!(devices[0].name, "raw:any");
        assert_eq!(default_device(&devices).as_deref(), Some("raw:any"));
        let list = device_list();
        assert!(list.available && list.methods[0].available);
    }
}
