//! AlbionLab: pairing, the albionlab.json settings, the live session published over SignalR, and the weapon icon cache.
//! Everything here runs on tokio, never on the engine or UI thread; every request has a timeout and can be cancelled.

mod compactor;
mod icons;
pub mod protocol;
mod service;
pub mod signalr;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

pub use compactor::{Amounts, SessionCompactor, SourceDungeon, SourceFloor, SourceItem, SourceMember, SourceSecond, SourceSnapshot, SourceWeapon};
pub use icons::{Icon, WeaponIconCache, RENDER_SERVICE};
pub use service::{place, suggest_title, Account, AlbionLabService, LabContext, LabError, LabSession, LabStatus, Pairing, Settings};
pub use tokio_util::sync::CancellationToken;

/// .NET `Uri.EscapeDataString`: everything but the RFC 3986 unreserved characters percent-encoded.
pub(crate) fn escape_data_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            escaped.push(byte as char);
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}
