//! What Program.cs decides at startup: the storage directory, the shipped assets and the settings the C# server read
//! from its configuration (`Storage:Directory`, `Capture:Debug`, `Capture:AutoStart`, `AlbionLab:Url`).

use crate::backend::{CaptureBackend, SystemCapture};
use meter_core::catalog::CombatSourceData;
use meter_core::maps::MapCatalog;
use meter_core::CombatSourceCatalog;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Unix microseconds; the wall clock outside tests.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_micros() as i64))
}

#[derive(Clone)]
pub struct Config {
    pub storage: PathBuf,
    /// The shipped `combat.json` and `maps.json` (`assets` next to the executable).
    pub assets: Option<PathBuf>,
    pub debug: bool,
    pub auto_start: bool,
    /// The AlbionLab address; `None` is albionlab.app.
    pub lab_server: Option<String>,
    /// Run the AlbionLab publish loop (off in tests that must not touch the network).
    pub lab: bool,
    /// The weapon icon render service; `None` is render.albiononline.com.
    pub icon_service: Option<String>,
    pub version: String,
    pub capture: Arc<dyn CaptureBackend>,
    pub clock: Clock,
    /// How long a dropped capture waits before it is opened again (`RestartLater`).
    pub reconnect_delay: Duration,
    /// Frames the capture thread may queue ahead of the engine before it drops and counts.
    pub channel_capacity: usize,
}

impl Config {
    /// The configuration the C# server would read from this environment and command line: `Storage__Directory`,
    /// `Capture__Debug`, `Capture__AutoStart` and `AlbionLab__Url` environment variables, or `--Storage:Directory=…`
    /// style arguments, which win.
    pub fn from_environment(version: &str, args: &[String]) -> Self {
        let setting = |key: &str| {
            let argument = format!("--{key}");
            let mut value = std::env::var(key.replace(':', "__")).ok();
            for (index, arg) in args.iter().enumerate() {
                if let Some(rest) = arg.strip_prefix(&argument) {
                    if let Some(inline) = rest.strip_prefix('=') {
                        value = Some(inline.to_owned());
                    } else if rest.is_empty() {
                        value = args.get(index + 1).cloned();
                    }
                }
            }
            value
        };
        let flag = |key: &str| setting(key).is_some_and(|value| value.eq_ignore_ascii_case("true"));
        let storage = setting("Storage:Directory").map(PathBuf::from);
        Self {
            storage: meter_store::paths::storage_directory(storage.as_deref()),
            assets: shipped_assets(),
            debug: flag("Capture:Debug"),
            auto_start: flag("Capture:AutoStart"),
            lab_server: setting("AlbionLab:Url"),
            lab: true,
            icon_service: None,
            version: version.to_owned(),
            capture: Arc::new(SystemCapture),
            clock: system_clock(),
            reconnect_delay: Duration::from_secs(3),
            channel_capacity: 4096,
        }
    }

    /// A configuration for one storage directory and no shipped assets; for tools and tests.
    pub fn new(storage: impl Into<PathBuf>, capture: Arc<dyn CaptureBackend>) -> Self {
        Self {
            storage: storage.into(),
            assets: None,
            debug: false,
            auto_start: false,
            lab_server: None,
            lab: false,
            icon_service: None,
            version: "0.0.0".into(),
            capture,
            clock: system_clock(),
            reconnect_delay: Duration::from_secs(3),
            channel_capacity: 4096,
        }
    }

    pub(crate) fn captures_directory(&self) -> PathBuf {
        meter_store::paths::captures_directory(&self.storage)
    }

    /// The imported catalog, written over the shipped one.
    pub(crate) fn maps_path(&self) -> PathBuf {
        self.storage.join("assets").join("maps.json")
    }

    /// `CombatSourceCatalog.Load(assets/combat.json) ?? new(...)`.
    pub(crate) fn load_sources(&self) -> CombatSourceCatalog {
        self.assets
            .as_ref()
            .and_then(|assets| CombatSourceCatalog::load(assets.join("combat.json")))
            .unwrap_or_else(|| CombatSourceCatalog::new(CombatSourceData { version: "unknown".into(), items: vec![], spells: vec![] }))
    }

    /// `MapCatalog`: the imported `maps.json` in the storage directory, else the shipped one.
    pub(crate) fn load_maps(&self) -> MapCatalog {
        let imported = self.maps_path();
        let path = if imported.exists() { Some(imported) } else { self.assets.as_ref().map(|assets| assets.join("maps.json")) };
        path.filter(|path| path.exists()).map(MapCatalog::load).unwrap_or_default()
    }
}

/// `assets` next to the executable (the installer copies assets/game-data there); a development build also finds
/// the repository's `assets/game-data`.
fn shipped_assets() -> Option<PathBuf> {
    let has = |directory: &Path| directory.join("combat.json").exists() || directory.join("maps.json").exists();
    let exe = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    let mut candidates: Vec<PathBuf> = exe.iter().flat_map(|directory| [directory.join("assets"), directory.join("assets").join("game-data")]).collect();
    if cfg!(debug_assertions) {
        candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../assets/game-data"));
    }
    candidates.into_iter().find(|directory| has(directory))
}
