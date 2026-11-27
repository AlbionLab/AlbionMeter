//! Where live frames come from: the system's capture devices (`PacketCapture` in C#), or a fake one in tests.

use meter_capture::{CaptureDevice, CaptureError, DeviceList, FrameSource};

pub trait CaptureBackend: Send + Sync + 'static {
    fn devices(&self) -> Vec<CaptureDevice>;
    fn default_device(&self, devices: &[CaptureDevice]) -> Option<String>;
    /// `GET /api/capture/devices` before the selected device is filled in.
    fn device_list(&self) -> DeviceList;
    fn library(&self, device: Option<&str>) -> String;
    /// Opens a device; called on a helper thread, never on the engine thread (it may run the firewall rule first).
    fn open(&self, device: &str) -> Result<Box<dyn FrameSource>, CaptureError>;
}

pub struct SystemCapture;

impl CaptureBackend for SystemCapture {
    fn devices(&self) -> Vec<CaptureDevice> {
        meter_capture::devices()
    }

    fn default_device(&self, devices: &[CaptureDevice]) -> Option<String> {
        meter_capture::default_device(devices)
    }

    fn device_list(&self) -> DeviceList {
        meter_capture::device_list()
    }

    fn library(&self, device: Option<&str>) -> String {
        meter_capture::library(device)
    }

    fn open(&self, device: &str) -> Result<Box<dyn FrameSource>, CaptureError> {
        #[cfg(windows)]
        if device.starts_with(meter_capture::RAW_SOCKET_PREFIX)
            && let Some(message) = meter_capture::windows::firewall::allow_capture().and_then(|rule| rule.message())
        {
            println!("{message}");
        }
        meter_capture::open(device)
    }
}
