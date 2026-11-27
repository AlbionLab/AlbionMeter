//! What the UI shows and what it asks for, independent of where it comes from. The engine thread (meter-core), the store
//! (meter-store) and AlbionLab (meter-lab) will publish these in the integration phase; today `fake::FakeFeed` does.
//! Field names follow web/src/types.ts and the JSON the C# server sends, so the mapping stays one to one.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

pub type Values = HashMap<Arc<str>, f64>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RecordStatus {
    #[default]
    Idle,
    Running,
    Paused,
    Completed,
}

impl RecordStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Membership {
    #[default]
    Active,
    Left,
    Unconfirmed,
}

#[derive(Clone, Debug, Default)]
pub struct Player {
    pub membership: Membership,
    pub id: Arc<str>,
    pub name: Arc<str>,
    /// "Name · T8.3", "Unarmed" or "Weapon not observed".
    pub weapon: Arc<str>,
    /// The weapon's item id once known (the icon cache key, `/api/weapon-icon/{id}` today).
    pub weapon_id: Option<Arc<str>>,
    pub weapon_family: Arc<str>,
    /// "#rrggbb"
    pub color: Arc<str>,
    pub damage: f64,
    pub healing: f64,
    pub encounter_dps: f64,
    pub encounter_hps: f64,
    pub damage_share: f64,
    pub healing_share: f64,
}

#[derive(Clone, Debug, Default)]
pub struct ItemContribution {
    /// autoAttack | passive | ability
    pub kind: Option<Arc<str>>,
    pub tier_observed: bool,
    pub id: Arc<str>,
    pub player_id: Arc<str>,
    pub player_name: Arc<str>,
    pub item_index: Option<i32>,
    /// The item's id once known (the icon cache key, `/api/items/{index}` today).
    pub item_id: Option<Arc<str>>,
    pub slot: Arc<str>,
    pub item: Arc<str>,
    /// "T8.3" when the item's tier is known.
    pub tier: Option<Arc<str>>,
    pub damage: f64,
    pub healing: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Point {
    pub second: u32,
    pub damage: Values,
    pub healing: Values,
    pub item_damage: Values,
    pub item_healing: Values,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub id: Arc<str>,
    /// ISO 8601, compared as text like state.ts does.
    pub started_at: Arc<str>,
    pub elapsed_seconds: f64,
    pub status: RecordStatus,
    /// live | replay | history
    pub mode: Arc<str>,
    pub sequence: u64,
    pub event_count: u64,
    pub total_damage: f64,
    pub total_healing: f64,
    pub members: Vec<Player>,
    pub former_members: Vec<Player>,
    pub item_contributions: Vec<ItemContribution>,
    /// Only the points that changed since the last snapshot of this record unless `full`.
    pub timeline: Vec<Point>,
    pub full: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Connection {
    #[default]
    Connecting,
    Connected,
    Reconnecting,
    Disconnected,
}

#[derive(Clone, Debug, Default)]
pub struct CaptureStatus {
    /// idle | starting | running | stopping | stopped | failed
    pub state: Arc<str>,
    /// none | live-capture | replay | socket | pcap
    pub source: Arc<str>,
    pub file: Option<String>,
    pub error: Option<String>,
    /// storageLimit | sourceEnded | error
    pub stop_reason: Option<Arc<str>>,
    pub library: Option<String>,
    pub adapter_dropped: Option<u64>,
    pub report: Option<CaptureReport>,
}

#[derive(Clone, Debug, Default)]
pub struct CaptureReport {
    pub frames: u64,
    pub bytes: u64,
    /// The decoder counters then the combat diagnostics, in the server's order.
    pub counters: Vec<(Arc<str>, f64)>,
    pub pending_unidentified: u64,
    pub transport_messages: u64,
}

#[derive(Clone, Debug)]
pub struct CaptureMethod {
    pub id: Arc<str>,
    pub name: Arc<str>,
    pub available: bool,
}

#[derive(Clone, Debug)]
pub struct CaptureDevice {
    pub name: Arc<str>,
    pub description: Arc<str>,
    pub method: Arc<str>,
    /// Bit 0: the card that carries the default route.
    pub flags: u32,
}

#[derive(Clone, Debug, Default)]
pub struct DeviceList {
    pub library: Option<String>,
    pub debug_enabled: bool,
    pub selected_device: Option<Arc<str>>,
    pub methods: Vec<CaptureMethod>,
    pub devices: Vec<CaptureDevice>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct LabSession {
    pub id: Arc<str>,
    pub url: Arc<str>,
    pub visibility: Arc<str>,
    pub anonymize: bool,
    /// live | completed | offline | …
    pub state: Arc<str>,
    pub error: Option<String>,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct LabStatus {
    pub server: Arc<str>,
    pub connected: bool,
    pub account: Option<(String, Option<Vec<u8>>)>,
    /// (code, verify url)
    pub pairing: Option<(String, String)>,
    /// revoked | denied | expired
    pub pairing_error: Option<Arc<str>>,
    pub session: Option<LabSession>,
    pub share_location: bool,
    pub visibility: Arc<str>,
    pub anonymize: bool,
    pub suggested_title: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CatalogEntry {
    pub id: Arc<str>,
    pub name: Arc<str>,
    /// The weapon family or the map type.
    pub group: Arc<str>,
    pub tier: Option<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub game_version: Option<String>,
    /// ISO 8601
    pub imported_at: Option<String>,
    pub weapons: Vec<CatalogEntry>,
    pub maps: Vec<CatalogEntry>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    Current,
    Downloading { version: String, percent: u8, transferred: Option<u64>, total: Option<u64> },
    Ready { version: String },
    Installing { version: String },
    Error(String),
}

#[derive(Clone, Debug)]
pub struct DungeonSummary {
    pub id: Arc<str>,
    pub started_at: Arc<str>,
    pub name: Arc<str>,
    pub entrance_map_name: Arc<str>,
    pub mode: Arc<str>,
    pub status: Arc<str>,
    pub floors: u32,
    pub duration: f64,
    pub damage: f64,
    pub healing: f64,
}

#[derive(Clone, Debug)]
pub struct DungeonFloor {
    pub id: Arc<str>,
    pub number: u32,
    pub map_id: Arc<str>,
    pub map_name: Arc<str>,
    pub totals: Arc<Snapshot>,
}

#[derive(Clone, Debug)]
pub struct DungeonRun {
    pub summary: DungeonSummary,
    pub floors: Vec<DungeonFloor>,
    pub totals: Arc<Snapshot>,
}

/// One thing that changed, sent from the feed's thread. Snapshots come at most 4 times a second.
#[derive(Clone, Debug)]
pub enum FeedEvent {
    Connection(Connection),
    Version(String),
    Snapshot(Arc<Snapshot>),
    Capture(Arc<CaptureStatus>),
    Devices(Arc<DeviceList>),
    Lab(Arc<LabStatus>),
    /// An AlbionLab action failed; the message is shown on the card.
    LabError(String),
    Catalog(Arc<Catalog>),
    CatalogMessage(String),
    StoragePath(String),
    Update(UpdateState),
    Dungeons(Arc<Vec<DungeonSummary>>),
    Dungeon(Arc<DungeonRun>),
    /// A record command answered: `None` on success, or the error to show in the banner.
    CommandDone(Option<String>),
    /// A capture command answered.
    CaptureDone(Option<String>),
    /// A weapon or item icon in the icon cache, by item id.
    Icon(Arc<str>, PathBuf),
    HistoryMessage { text: String, error: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    StartRecord,
    PauseRecord,
    ResumeRecord,
    /// Reset: clear the count of the current record (the C# "new-record").
    NewRecord,
    /// End session (the C# "stop-record").
    StopRecord,
    RefreshDevices,
    StartCapture { device: String, method: String },
    StopCapture,
    Replay(PathBuf),
    LabConnect,
    LabTitle(String),
    LabVisibility(String),
    LabAnonymize(bool),
    LabShareLocation(bool),
    LabDisconnect,
    ImportCatalog(PathBuf),
    OpenDungeon(Arc<str>),
    ExportHistory { dungeon: Option<Arc<str>>, to: PathBuf },
    ImportHistory(PathBuf),
    Icon(Arc<str>),
    InstallUpdate,
}

/// Where the meter's data comes from. `run` is called once on a thread of its own and returns when `commands` closes.
pub trait MeterFeed: Send + 'static {
    fn run(self: Box<Self>, publish: Box<dyn Fn(FeedEvent) + Send>, commands: Receiver<Command>);
}
