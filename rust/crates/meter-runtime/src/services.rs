//! Everything that waits on a disk or a network, on tokio: the history store (one job at a time, in the order the
//! engine sent them), the local character files, AlbionLab, weapon icons, the game catalog import and history files.

use crate::config::Config;
use crate::engine::Input;
use crate::feed::{self, FeedEvent};
use crate::resolve;
use crate::{Publisher, Shared};
use meter_core::cs::{CsMap, Guid};
use meter_core::dungeon::DungeonRunSnapshot;
use meter_core::maps::{MapCatalog, MapCatalogData};
use meter_core::observation::combat::{self as core_identity, KnownPlayer};
use meter_core::observation::local::LocalCharacterSnapshot;
use meter_core::observation::roster::PartyRosterSnapshot;
use meter_core::CombatSnapshot;
use meter_lab::{AlbionLabService, LabContext, WeaponIconCache};
use meter_store::{CaptureSettings, CharacterContextStore, EncounterStore, HistoryArchive};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// What a new live analysis starts from: the identities the history knows and the local character files
/// (`CreateLiveAnalysis`). Read once at startup, before the first capture needs them.
#[derive(Default)]
pub(crate) struct Seeds {
    pub characters: Vec<(Guid, core_identity::KnownCharacter)>,
    pub skills: Vec<(i32, CsMap<i32, i32>)>,
    pub character: Option<LocalCharacterSnapshot>,
    pub party: Option<PartyRosterSnapshot>,
    pub players: Vec<KnownPlayer>,
}

pub(crate) struct Identities {
    pub characters: CsMap<Guid, core_identity::KnownCharacter>,
    pub unidentified: Vec<core_identity::UnidentifiedPlayer>,
    pub sightings: Vec<core_identity::IdentitySighting>,
    pub skills: Option<CsMap<i32, CsMap<i32, i32>>>,
    pub timeline: Option<Vec<core_identity::IdentityTimelineEntry>>,
}

pub(crate) enum StoreJob {
    SaveEncounter(Box<CombatSnapshot>, bool),
    SaveDungeon(Arc<DungeonRunSnapshot>),
    Identities(Box<Identities>),
    Character(Value),
    Party(Value),
    Players(Value, Vec<Value>),
    SaveDevice(String),
    /// `Reserve`: the captures folder exists, and without debug the legacy recordings are gone.
    PrepareCaptures,
    /// The runs of the live capture, to list before the saved ones.
    Dungeons(Vec<meter_store::DungeonSummary>),
    Dungeon(String),
    Export { dungeon: Option<String>, to: PathBuf },
    Import(PathBuf),
    /// Answered once every job sent before it is done.
    Barrier(tokio::sync::oneshot::Sender<()>),
}

fn uuid(id: &Guid) -> Uuid {
    Uuid::parse_str(&id.to_string()).unwrap_or_default()
}

fn guid(id: &Uuid) -> Guid {
    Guid::parse(&id.to_string()).unwrap_or_default()
}

/// Opens the history and reads what the engine starts from.
pub(crate) fn open_store(config: &Config) -> (Arc<EncounterStore>, Arc<CharacterContextStore>, Seeds) {
    let store = EncounterStore::open(&config.storage, config.debug).unwrap_or_else(|error| panic!("The history at {} could not be opened: {error}", config.storage.display()));
    let context = CharacterContextStore::open(&config.storage);
    let mut characters: Vec<(Guid, core_identity::KnownCharacter)> = store
        .known_characters()
        .unwrap_or_default()
        .iter()
        .map(|(id, character)| {
            (guid(id), core_identity::KnownCharacter { name: character.name.clone(), equipment: character.equipment.as_ref().map(|equipment| equipment.as_slice().into()), skills: character.skills.clone() })
        })
        .collect();
    characters.sort_by_key(|(id, _)| id.to_string());
    let mut skills: Vec<(i32, CsMap<i32, i32>)> = store.known_skill_weapons().unwrap_or_default().into_iter().map(|(skill, weapons)| (skill, weapons.into_iter().collect())).collect();
    skills.sort_by_key(|(skill, _)| *skill);
    let character_value = context.snapshot();
    let character: Option<LocalCharacterSnapshot> = character_value.clone().and_then(|value| serde_json::from_value(value).ok());
    let players = character_value.map(|value| context.recent_players(&value)).unwrap_or_default().into_iter().filter_map(|value| serde_json::from_value(value).ok()).collect();
    let party = context.party().and_then(|value| serde_json::from_value(value).ok());
    (Arc::new(store), Arc::new(context), Seeds { characters, skills, character, party, players })
}

struct StoreWorker {
    config: Config,
    shared: Arc<Shared>,
    publish: Publisher,
    store: Arc<EncounterStore>,
    context: Arc<CharacterContextStore>,
    listed: Option<Vec<meter_store::DungeonSummary>>,
}

impl StoreWorker {
    fn run(&mut self, job: StoreJob) {
        let report = |what: &str, result: meter_store::Result<()>| {
            if let Err(error) = result {
                eprintln!("{what}: {error}");
            }
        };
        match job {
            StoreJob::SaveEncounter(snapshot, allow_empty) => report("Failed to save the encounter", self.store.save(&*snapshot, allow_empty)),
            StoreJob::SaveDungeon(run) => report("Failed to save the dungeon run", self.store.save_dungeon(&*run, true)),
            StoreJob::Identities(identities) => {
                // Identity evidence that cannot be written is learned again from the next save.
                let _ = self.identities(&identities);
            }
            StoreJob::Character(value) => self.context.observe(&value),
            StoreJob::Party(value) => self.context.observe_party(&value),
            StoreJob::Players(character, players) => self.context.observe_players(&character, &players),
            StoreJob::SaveDevice(device) => {
                let _ = CaptureSettings { device: Some(device) }.save(&self.config.storage);
            }
            StoreJob::PrepareCaptures => {
                let directory = self.config.captures_directory();
                let _ = std::fs::create_dir_all(&directory);
                if !self.config.debug {
                    meter_capture::storage::remove_legacy_recordings(&directory);
                }
            }
            StoreJob::Dungeons(current) => self.dungeons(current, false),
            StoreJob::Dungeon(id) => {
                let names = self.shared.names();
                if let Ok(Some(value)) = self.store.get_dungeon::<Value>(&id)
                    && let Some(run) = resolve::run_of(&names, &value)
                {
                    (self.publish)(FeedEvent::Dungeon(Arc::new(run)));
                }
            }
            StoreJob::Export { dungeon, to } => {
                let result = self.store.export_history(None, dungeon.as_deref()).and_then(|archive| archive.write()).and_then(|bytes| Ok(std::fs::write(&to, bytes)?));
                (self.publish)(match result {
                    Ok(()) => FeedEvent::HistoryMessage { text: format!("Exported to {}", to.display()), error: false },
                    Err(error) => FeedEvent::HistoryMessage { text: error.to_string(), error: true },
                });
            }
            StoreJob::Import(path) => {
                let result = std::fs::read(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| if bytes.is_empty() || bytes.len() > meter_store::MAX_UPLOAD_BYTES { Err("Choose a history file smaller than 40 MB.".to_owned()) } else { Ok(bytes) })
                    .and_then(|bytes| HistoryArchive::read(&bytes).and_then(|archive| self.store.import_history(&archive)).map_err(|error| error.to_string()));
                (self.publish)(match result {
                    Ok(result) => {
                        let skipped = result.skipped_encounters + result.skipped_dungeons;
                        let mut text = format!("Imported {} encounters and {} dungeons.", result.imported_encounters, result.imported_dungeons);
                        if skipped > 0 {
                            text.push_str(&format!(" {skipped} existing records or records over the storage limit were skipped."));
                        }
                        FeedEvent::HistoryMessage { text, error: false }
                    }
                    Err(text) => FeedEvent::HistoryMessage { text, error: true },
                });
                self.dungeons(vec![], true);
            }
            StoreJob::Barrier(done) => {
                let _ = done.send(());
            }
        }
    }

    fn identities(&self, identities: &Identities) -> meter_store::Result<()> {
        let equipment = |gear: &Option<core_identity::Gear>| gear.as_ref().map(|gear| gear.to_vec());
        let characters: Vec<(Uuid, meter_store::KnownCharacter)> = identities
            .characters
            .iter()
            .map(|(id, character)| (uuid(id), meter_store::KnownCharacter { name: character.name.clone(), equipment: equipment(&character.equipment), skills: character.skills.clone() }))
            .collect();
        let unidentified: Vec<meter_store::UnidentifiedPlayer> = identities
            .unidentified
            .iter()
            .map(|player| meter_store::UnidentifiedPlayer {
                member_id: player.member_id.clone(),
                record_id: player.record_id.clone(),
                object_id: player.object_id,
                equipment: equipment(&player.equipment),
                candidates: player.candidates.iter().map(uuid).collect(),
                timestamp: player.timestamp,
                skills: player.skills.clone(),
                visit: player.visit.clone(),
            })
            .collect();
        let sightings: Vec<meter_store::IdentitySighting> = identities
            .sightings
            .iter()
            .map(|sighting| meter_store::IdentitySighting { character_id: uuid(&sighting.character_id), object_id: sighting.object_id, visit: sighting.visit.clone(), timestamp: sighting.timestamp, method: sighting.method.clone() })
            .collect();
        self.store.observe_identities(characters.iter().map(|(id, character)| (id, character)), &unidentified, &sightings)?;
        if let Some(skills) = &identities.skills {
            let skills: Vec<(i32, BTreeMap<i32, i32>)> = skills.iter().map(|(skill, weapons)| (*skill, weapons.iter().map(|(weapon, count)| (*weapon, *count)).collect())).collect();
            self.store.observe_skill_weapons(skills.iter().map(|(skill, weapons)| (skill, weapons)))?;
        }
        if let Some(timeline) = &identities.timeline {
            let timeline: Vec<meter_store::IdentityTimelineEntry> = timeline
                .iter()
                .map(|entry| meter_store::IdentityTimelineEntry {
                    timestamp: entry.timestamp,
                    visit: entry.visit.clone(),
                    kind: entry.kind.clone(),
                    character_id: uuid(&entry.character_id),
                    name: entry.name.clone(),
                    object_id: entry.object_id,
                    equipment: equipment(&entry.equipment),
                })
                .collect();
            self.store.observe_timeline(&timeline)?;
        }
        Ok(())
    }

    /// `GET /api/dungeons`, published only when the list changed.
    fn dungeons(&mut self, current: Vec<meter_store::DungeonSummary>, force: bool) {
        let stored = self.store.list_dungeons().unwrap_or_default();
        let stored: Vec<_> = stored.into_iter().filter(|run| !current.iter().any(|active| active.id == run.id)).collect();
        let mut list = current;
        list.extend(stored);
        list.truncate(100);
        if !force && self.listed.as_ref() == Some(&list) {
            return;
        }
        (self.publish)(FeedEvent::Dungeons(Arc::new(list.iter().map(resolve::summary_of).collect())));
        self.listed = Some(list);
    }
}

/// Runs the store jobs one after another on tokio's blocking pool.
pub(crate) async fn store_loop(config: Config, shared: Arc<Shared>, publish: Publisher, store: Arc<EncounterStore>, context: Arc<CharacterContextStore>, mut jobs: tokio::sync::mpsc::UnboundedReceiver<StoreJob>) {
    let mut worker = Some(StoreWorker { config, shared, publish, store, context, listed: None });
    while let Some(job) = jobs.recv().await {
        let mut taken = worker.take().unwrap();
        match tokio::task::spawn_blocking(move || {
            taken.run(job);
            taken
        })
        .await
        {
            Ok(back) => worker = Some(back),
            Err(_) => return,
        }
    }
}

/// What the AlbionLab publish loop reads from the engine.
pub(crate) struct Context(pub Arc<Shared>);

impl LabContext for Context {
    fn record(&self) -> Value {
        let record = self.0.lab_view().record;
        serde_json::to_value(&*record).unwrap_or_default()
    }
    fn dungeons(&self) -> Vec<Value> {
        self.0.lab_view().dungeons.iter().filter_map(|run| serde_json::to_value(&**run).ok()).collect()
    }
    fn character(&self) -> Option<(String, Option<String>)> {
        self.0.lab_view().character
    }
    fn item_id(&self, index: i32) -> Option<String> {
        self.0.sources.find_item(index).map(|item| item.id.clone())
    }
    fn item_id_by_name(&self, name: &str) -> Option<String> {
        self.0.sources.item_id_by_name(name)
    }
    fn weapon_family(&self, index: i32) -> Option<String> {
        self.0.maps().weapon_family(Some(index))
    }
}

pub(crate) fn lab_status(status: &meter_lab::LabStatus, avatar: Option<Vec<u8>>) -> feed::LabStatus {
    feed::LabStatus {
        server: status.server.as_str().into(),
        connected: status.connected,
        account: status.account.as_ref().map(|account| (account.name.clone(), avatar)),
        pairing: status.pairing.as_ref().map(|pairing| (pairing.code.clone(), pairing.verify_url.clone())),
        pairing_error: status.pairing_error.as_deref().map(Into::into),
        session: status.session.as_ref().map(|session| feed::LabSession {
            id: session.id.as_str().into(),
            url: session.url.as_str().into(),
            visibility: session.visibility.as_str().into(),
            anonymize: session.anonymize,
            state: session.state.as_str().into(),
            error: session.error.clone(),
            title: session.title.clone(),
        }),
        share_location: status.share_location,
        visibility: status.visibility.as_str().into(),
        anonymize: status.anonymize,
        suggested_title: status.suggested_title.clone(),
    }
}

/// The account picture for the AlbionLab card, at most 256 KB.
async fn avatar(http: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let mut response = http.get(url).send().await.ok()?.error_for_status().ok()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > 256_000 {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes)
}

/// Publishes the AlbionLab card whenever it changes and tells the engine whether anyone reads the live record.
pub(crate) async fn lab_status_loop(lab: AlbionLabService, shared: Arc<Shared>, publish: Publisher, cancel: meter_lab::CancellationToken) {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(8)).build().expect("HTTP client");
    let mut last: Option<meter_lab::LabStatus> = None;
    let mut avatars: (String, Option<Vec<u8>>) = (String::new(), None);
    let mut timer = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = timer.tick() => {}
            _ = shared.lab_changed.notified() => {}
            _ = cancel.cancelled() => return,
        }
        let status = lab.status();
        shared.lab_active.store(status.connected, Ordering::Relaxed);
        if last.as_ref() == Some(&status) {
            continue;
        }
        if let Some(account) = &status.account
            && !account.avatar_url.is_empty()
            && avatars.0 != account.avatar_url
        {
            avatars = (account.avatar_url.clone(), avatar(&http, &account.avatar_url).await);
        }
        publish(FeedEvent::Lab(Arc::new(lab_status(&status, status.account.as_ref().and(avatars.1.clone())))));
        last = Some(status);
    }
}

pub(crate) fn device_list(config: &Config, shared: &Shared) -> feed::DeviceList {
    let list = config.capture.device_list();
    let selected = shared.selected_device.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    feed::DeviceList {
        library: list.library,
        debug_enabled: config.debug,
        selected_device: selected.or(list.selected_device).map(Into::into),
        methods: list.methods.iter().map(|method| feed::CaptureMethod { id: method.id.into(), name: method.name.into(), available: method.available }).collect(),
        devices: list.devices.iter().map(|device| feed::CaptureDevice { name: device.name.as_str().into(), description: device.description.as_str().into(), method: device.method.into(), flags: device.flags }).collect(),
        error: list.error,
    }
}

pub(crate) fn catalog(maps: &MapCatalog) -> Option<feed::Catalog> {
    let data = maps.data()?;
    let tier = |tier: Option<i32>| tier.and_then(|tier| u8::try_from(tier).ok());
    Some(feed::Catalog {
        game_version: Some(data.game_version.clone()),
        imported_at: Some(data.imported_at.clone()),
        weapons: data.weapons.iter().flatten().map(|weapon| feed::CatalogEntry { id: weapon.id.as_str().into(), name: weapon.name.as_str().into(), group: weapon.family.as_str().into(), tier: tier(weapon.tier) }).collect(),
        maps: data.maps.iter().map(|map| feed::CatalogEntry { id: map.id.as_str().into(), name: map.name.as_str().into(), group: map.kind.as_str().into(), tier: tier(map.tier) }).collect(),
    })
}

const IMPORT_FAILED: &str = "The game files could not be imported. Select an installed game folder with valid world.bin, items.bin and localization.bin files.";

/// The browser's folder walk of Library.tsx: `…/GameData/cluster/world.bin`, `items.bin` and `localization.bin` next
/// to `cluster`, and `version.txt` at the game root above `Albion-Online_Data`.
fn game_files(folder: &Path) -> Result<(PathBuf, PathBuf), String> {
    fn find(directory: &Path, depth: usize, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && depth > 0 {
                find(&path, depth - 1, found);
            } else if path.ends_with("GameData/cluster/world.bin") {
                found.push(path);
            }
        }
    }
    let mut found = vec![];
    find(folder, 8, &mut found);
    let [world] = found.as_slice() else { return Err("Select one installed game folder containing GameData/cluster/world.bin.".into()) };
    let game_data = world.parent().and_then(Path::parent).ok_or(IMPORT_FAILED)?.to_path_buf();
    Ok((world.clone(), game_data))
}

fn read_limited(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::metadata(path) {
        Err(_) => Ok(None),
        Ok(metadata) if metadata.len() == 0 || metadata.len() > 20_000_000 => Err(IMPORT_FAILED.into()),
        Ok(_) => std::fs::read(path).map(Some).map_err(|_| IMPORT_FAILED.to_owned()),
    }
}

pub(crate) fn import_catalog(config: &Config, folder: &Path) -> Result<MapCatalogData, String> {
    let (world, game_data) = game_files(folder)?;
    let world = read_limited(&world)?.ok_or(IMPORT_FAILED)?;
    let items = read_limited(&game_data.join("items.bin"))?;
    let localization = read_limited(&game_data.join("localization.bin"))?;
    let root = game_data.ancestors().find(|path| path.file_name().is_some_and(|name| name == "Albion-Online_Data")).and_then(Path::parent).unwrap_or(&game_data);
    let version = std::fs::metadata(root.join("version.txt"))
        .ok()
        .filter(|metadata| metadata.len() < 1000)
        .and_then(|_| std::fs::read_to_string(root.join("version.txt")).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "unknown".into());
    if version.chars().count() > 200 {
        return Err(IMPORT_FAILED.into());
    }
    let data = meter_core::importer::import_maps(&world, items.as_deref(), localization.as_deref(), &version, "browser-folder-selection/world.bin").map_err(|_| IMPORT_FAILED.to_owned())?;
    let mut catalog = MapCatalog::default();
    catalog.save(config.maps_path(), data.clone()).map_err(|error| error.to_string())?;
    Ok(data)
}

/// The services the router hands commands to.
#[derive(Clone)]
pub(crate) struct Services {
    pub config: Config,
    pub shared: Arc<Shared>,
    pub publish: Publisher,
    pub engine: crossbeam_channel::Sender<Input>,
    pub lab: AlbionLabService,
    pub icons: Arc<WeaponIconCache>,
    pub importing: Arc<AtomicBool>,
}

impl Services {
    pub fn devices(&self, runtime: &tokio::runtime::Handle) {
        let this = self.clone();
        runtime.spawn_blocking(move || (this.publish)(FeedEvent::Devices(Arc::new(device_list(&this.config, &this.shared)))));
    }

    pub fn import_catalog(&self, runtime: &tokio::runtime::Handle, folder: PathBuf) {
        if self.importing.swap(true, Ordering::AcqRel) {
            return (self.publish)(FeedEvent::CatalogMessage("Another import is running.".into()));
        }
        let this = self.clone();
        runtime.spawn_blocking(move || {
            match import_catalog(&this.config, &folder) {
                Ok(data) => {
                    let maps = Arc::new(MapCatalog::new(Some(data)));
                    this.shared.set_maps(maps.clone());
                    let _ = this.engine.send(Input::Catalog(maps.clone()));
                    let counts = maps.data().map_or((0, 0), |data| (data.map_count, data.weapon_count));
                    if let Some(catalog) = catalog(&maps) {
                        (this.publish)(FeedEvent::Catalog(Arc::new(catalog)));
                    }
                    (this.publish)(FeedEvent::CatalogMessage(format!("imported:{}:{}", counts.0, counts.1)));
                }
                Err(message) => (this.publish)(FeedEvent::CatalogMessage(message)),
            }
            this.importing.store(false, Ordering::Release);
        });
    }

    pub fn icon(&self, runtime: &tokio::runtime::Handle, id: Arc<str>) {
        let this = self.clone();
        runtime.spawn(async move {
            if this.icons.get(&id).await.is_some() {
                let path = meter_store::paths::weapon_icons_directory(&this.config.storage).join(format!("{id}.png"));
                (this.publish)(FeedEvent::Icon(id, path));
            }
        });
    }

    /// The AlbionLab commands, with the C# endpoint checks and error texts.
    pub fn lab(&self, runtime: &tokio::runtime::Handle, command: feed::Command) {
        let this = self.clone();
        runtime.spawn(async move {
            let unreachable = |error: meter_lab::LabError| FeedEvent::LabError(format!("AlbionLab is unreachable: {error}"));
            let event = match command {
                feed::Command::LabConnect => this.lab.connect().await.err().map(unreachable),
                feed::Command::LabDisconnect => {
                    this.lab.disconnect().await;
                    None
                }
                feed::Command::LabShareLocation(share) => {
                    this.lab.share_location(share);
                    None
                }
                feed::Command::LabTitle(title) if title.encode_utf16().count() > 80 => Some(FeedEvent::LabError("The name is limited to 80 characters.".into())),
                feed::Command::LabTitle(title) => this.lab.update_session(Some(&title), None, None).await.err().map(unreachable),
                feed::Command::LabVisibility(visibility) if !matches!(visibility.as_str(), "private" | "unlisted" | "public") => Some(FeedEvent::LabError("Unknown visibility.".into())),
                feed::Command::LabVisibility(visibility) => this.lab.update_session(None, Some(&visibility), None).await.err().map(unreachable),
                feed::Command::LabAnonymize(anonymize) => this.lab.update_session(None, None, Some(anonymize)).await.err().map(unreachable),
                _ => None,
            };
            if let Some(event) = event {
                (this.publish)(event);
            }
            this.shared.lab_changed.notify_one();
        });
    }
}

