//! Npcap installs wpcap.dll under System32\Npcap, outside the DLL search path, and the meter must run without it.
//! The app delay-loads wpcap.dll; `load` puts it in the process from its real place before the first pcap call, so
//! the delay-load helper finds the module already loaded.

use std::sync::OnceLock;

use windows::Win32::System::LibraryLoader::{LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::core::HSTRING;

unsafe extern "C" {
    fn pcap_init(options: u32, error: *mut std::ffi::c_char) -> i32;
}

const PCAP_CHAR_ENC_UTF_8: u32 = 1;

/// Whether Npcap is installed and initialised (UTF-8 names and errors); computed once.
pub fn load() -> bool {
    static LOADED: OnceLock<bool> = OnceLock::new();
    *LOADED.get_or_init(|| {
        let mut system = [0u16; 260];
        // SAFETY: the buffer length is passed with the slice.
        let length = unsafe { GetSystemDirectoryW(Some(&mut system)) } as usize;
        if length == 0 || length >= system.len() {
            return false;
        }
        let path = format!(
            "{}\\Npcap\\wpcap.dll",
            String::from_utf16_lossy(&system[..length])
        );
        // Altered search path: Packet.dll, next to wpcap.dll, loads from the same folder.
        // SAFETY: a valid wide path; the module stays loaded for the life of the process.
        if unsafe { LoadLibraryExW(&HSTRING::from(path), None, LOAD_WITH_ALTERED_SEARCH_PATH) }
            .is_err()
        {
            return false;
        }
        let mut error = [0 as std::ffi::c_char; 256];
        // SAFETY: wpcap.dll is loaded; `error` is PCAP_ERRBUF_SIZE bytes.
        unsafe { pcap_init(PCAP_CHAR_ENC_UTF_8, error.as_mut_ptr()) == 0 }
    })
}
