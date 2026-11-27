use crate::{json, paths};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// `capture-settings.json`: the capture interface chosen in Data.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSettings {
    #[serde(default)]
    pub device: Option<String>,
}

impl CaptureSettings {
    pub fn load(storage: &Path) -> Self {
        std::fs::read_to_string(storage.join(paths::CAPTURE_SETTINGS_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, storage: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(storage)?;
        std::fs::write(storage.join(paths::CAPTURE_SETTINGS_FILE), json::to_string_indented(self).map_err(std::io::Error::other)?)
    }
}

/// `CaptureHistory.LocalFiles`: the live-capture recordings (pcap with a `.json` sidecar whose `session.source` is
/// `live-capture`) of the last 24 hours, oldest first, newest kept within 512 MB.
pub fn local_capture_files(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else { return vec![] };
    let modified = |path: &Path| std::fs::metadata(path).and_then(|meta| meta.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
    let length = |path: &Path| std::fs::metadata(path).map_or(0, |meta| meta.len() as i64);
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "pcap") && path.is_file())
        .map(|path| (modified(&path), path))
        .collect();
    files.sort_by_key(|(time, _)| *time);
    let live: Vec<(SystemTime, PathBuf)> = files
        .into_iter()
        .filter(|(_, path)| {
            let mut sidecar = path.as_os_str().to_owned();
            sidecar.push(".json");
            length(path) <= 128_000_000
                && std::fs::read_to_string(&sidecar).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).is_some_and(|metadata| {
                    metadata.get("session").and_then(|session| session.get("source")).and_then(Value::as_str) == Some("live-capture")
                })
        })
        .collect();
    let cutoff = SystemTime::now() - Duration::from_secs(24 * 3600);
    let mut budget: i64 = 512_000_000;
    let mut recent: Vec<PathBuf> = live
        .into_iter()
        .rev()
        .filter(|(time, _)| *time >= cutoff)
        .take_while(|(_, path)| {
            budget -= length(path);
            budget >= 0
        })
        .map(|(_, path)| path)
        .collect();
    recent.reverse();
    recent
}
