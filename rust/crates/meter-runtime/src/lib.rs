//! The whole meter without a UI: what the .NET server did minus HTTP (Program.cs, CaptureService, MeterSession, the
//! MeterHub worker, the capture, catalog and history endpoints, CharacterContextStore, AlbionLabService and
//! WeaponIconCache), behind the [`feed`] contract the Slint app consumes.
//!
//! Threads (see rust/README.md): the capture thread hands frames over a bounded channel to the engine thread, which
//! owns every piece of meter state ([`engine`]); a small tokio runtime runs the history store, the character files,
//! AlbionLab, icons and imports on owned copies ([`services`]). The thread that calls [`MeterFeed::run`] (or
//! [`Runtime::start`]) only routes commands.

mod backend;
mod config;
mod engine;
pub mod feed;
mod metrics;
mod resolve;
mod services;

pub use backend::{CaptureBackend, SystemCapture};
pub use config::{Clock, Config, system_clock};
pub use feed::{Command, FeedEvent, MeterFeed};
pub use metrics::{Latency, MetricsSnapshot};

use engine::{Engine, Input};
use meter_core::dungeon::DungeonRunSnapshot;
use meter_core::maps::MapCatalog;
use meter_core::{CombatEngine, CombatSnapshot, CombatSourceCatalog};
use services::{Services, StoreJob};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

pub(crate) type Publisher = Arc<dyn Fn(FeedEvent) + Send + Sync>;

/// What AlbionLab reads of the live meter, replaced whole by the engine at most once a second.
#[derive(Clone)]
pub(crate) struct LabView {
    pub record: Arc<CombatSnapshot>,
    pub dungeons: Arc<Vec<Arc<DungeonRunSnapshot>>>,
    pub character: Option<(String, Option<String>)>,
    pub updated: Option<Instant>,
}

/// The few things more than one thread reads. None of it is meter state: the engine publishes copies here.
pub(crate) struct Shared {
    pub sources: Arc<CombatSourceCatalog>,
    maps: RwLock<Arc<MapCatalog>>,
    icon_ids: RwLock<HashSet<String>>,
    pub lab: Mutex<LabView>,
    pub lab_active: AtomicBool,
    pub lab_changed: tokio::sync::Notify,
    pub selected_device: Mutex<Option<String>>,
    pub metrics: metrics::Metrics,
}

fn weapon_ids(maps: &MapCatalog) -> HashSet<String> {
    maps.data().and_then(|data| data.indexed_weapons.as_ref()).into_iter().flatten().map(|weapon| weapon.id.clone()).collect()
}

impl Shared {
    fn new(sources: Arc<CombatSourceCatalog>, maps: Arc<MapCatalog>) -> Self {
        let mut empty = CombatEngine::empty(None).full_snapshot("idle");
        empty.mode = "unavailable".into();
        empty.party_id = None;
        Self {
            sources,
            icon_ids: RwLock::new(weapon_ids(&maps)),
            maps: RwLock::new(maps),
            lab: Mutex::new(LabView { record: Arc::new(empty), dungeons: Arc::default(), character: None, updated: None }),
            lab_active: AtomicBool::new(false),
            lab_changed: tokio::sync::Notify::new(),
            selected_device: Mutex::new(None),
            metrics: metrics::Metrics::default(),
        }
    }

    pub fn maps(&self) -> Arc<MapCatalog> {
        self.maps.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    pub fn set_maps(&self, maps: Arc<MapCatalog>) {
        *self.icon_ids.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = weapon_ids(&maps);
        *self.maps.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = maps;
    }

    pub fn names(&self) -> resolve::Names {
        resolve::Names::new(self.maps(), self.sources.clone())
    }

    pub fn lab_view(&self) -> LabView {
        self.lab.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// `WeaponIconCache`'s check: only items and weapons of the game data are ever requested.
    fn known_icon(&self, id: &str) -> bool {
        self.sources.contains_item(id) || self.icon_ids.read().unwrap_or_else(|poisoned| poisoned.into_inner()).contains(id)
    }
}

/// The meter, ready to run. [`MeterFeed::run`] is the UI's entry point; [`Runtime::start`] runs it on a thread of its
/// own and returns a [`Handle`] (tools and tests).
pub struct Runtime {
    config: Config,
}

pub struct Handle {
    commands: Option<std::sync::mpsc::Sender<Command>>,
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Handle {
    pub fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }

    pub fn metrics(&self) -> MetricsSnapshot {
        self.shared.metrics.snapshot()
    }

    /// Saves the current record, ends the AlbionLab session and stops the capture, as closing the meter does.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.commands = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Runtime {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn start(self, publish: impl Fn(FeedEvent) + Send + Sync + 'static) -> Handle {
        let (commands, receiver) = std::sync::mpsc::channel();
        let shared = self.shared();
        let thread = std::thread::Builder::new()
            .name("meter".into())
            .spawn({
                let shared = shared.clone();
                move || self.serve(Arc::new(publish), receiver, shared)
            })
            .expect("meter thread");
        Handle { commands: Some(commands), shared, thread: Some(thread) }
    }

    fn shared(&self) -> Arc<Shared> {
        Arc::new(Shared::new(Arc::new(self.config.load_sources()), Arc::new(self.config.load_maps())))
    }

    fn serve(self, publish: Publisher, commands: std::sync::mpsc::Receiver<Command>, shared: Arc<Shared>) {
        let config = self.config;
        publish(FeedEvent::Version(config.version.clone()));
        let tokio = tokio::runtime::Builder::new_multi_thread().worker_threads(2).max_blocking_threads(4).thread_name("meter-io").enable_all().build().expect("tokio runtime");
        let (store, context, seeds) = services::open_store(&config);
        publish(FeedEvent::StoragePath(store.database_path().display().to_string()));
        if let Some(catalog) = services::catalog(&shared.maps()) {
            publish(FeedEvent::Catalog(Arc::new(catalog)));
        }
        let (store_jobs, store_receiver) = tokio::sync::mpsc::unbounded_channel();
        let store_task = tokio.spawn(services::store_loop(config.clone(), shared.clone(), publish.clone(), store, context, store_receiver));
        let lab = meter_lab::AlbionLabService::new(&config.storage, config.lab_server.as_deref(), Arc::new(services::Context(shared.clone())));
        let icons = {
            let shared = shared.clone();
            let known = move |id: &str| shared.known_icon(id);
            Arc::new(match &config.icon_service {
                Some(service) => meter_lab::WeaponIconCache::with_service(&config.storage, service, known),
                None => meter_lab::WeaponIconCache::new(&config.storage, known),
            })
        };
        let (engine_inputs, engine_receiver) = crossbeam_channel::unbounded();
        let engine = Engine::new(
            config.clone(),
            shared.clone(),
            publish.clone(),
            store_jobs.clone(),
            engine_receiver,
            engine_inputs.clone(),
            shared.maps(),
            seeds,
            meter_store::CaptureSettings::load(&config.storage).device,
        );
        let engine_thread = std::thread::Builder::new().name("engine".into()).spawn(move || engine.run()).expect("engine thread");
        let services = Services { config: config.clone(), shared: shared.clone(), publish: publish.clone(), engine: engine_inputs.clone(), lab: lab.clone(), icons, importing: Arc::default() };
        let io = tokio.handle().clone();
        services.devices(&io);
        let _ = store_jobs.send(StoreJob::Dungeons(vec![]));
        let cancel = meter_lab::CancellationToken::new();
        let status_task = tokio.spawn(services::lab_status_loop(lab.clone(), shared.clone(), publish.clone(), cancel.clone()));
        let lab_task = config.lab.then(|| {
            let (lab, cancel) = (lab.clone(), cancel.clone());
            tokio.spawn(async move { lab.run(cancel).await })
        });
        publish(FeedEvent::Connection(feed::Connection::Connected));
        for command in commands.iter() {
            match command {
                Command::RefreshDevices => services.devices(&io),
                Command::LabConnect | Command::LabTitle(_) | Command::LabVisibility(_) | Command::LabAnonymize(_) | Command::LabShareLocation(_) | Command::LabDisconnect => services.lab(&io, command),
                Command::ImportCatalog(folder) => services.import_catalog(&io, folder),
                Command::ImportHistory(path) => {
                    let _ = store_jobs.send(StoreJob::Import(path));
                }
                Command::Icon(id) => services.icon(&io, id),
                // The updater belongs to the app (Velopack); nothing here installs anything.
                Command::InstallUpdate => {}
                command => {
                    let _ = engine_inputs.send(Input::Command(command));
                }
            }
        }
        // Closing: AlbionLab ends its session first (within 3 s), then the record is saved and the capture stops,
        // as the C# hosted services stop in reverse order.
        cancel.cancel();
        if let Some(task) = lab_task {
            let _ = tokio.block_on(async { tokio::time::timeout(Duration::from_secs(5), task).await });
        }
        let (done, saved) = std::sync::mpsc::channel();
        if engine_inputs.send(Input::Shutdown(done)).is_ok() {
            let _ = saved.recv();
        }
        let _ = engine_thread.join();
        let (barrier, flushed) = tokio::sync::oneshot::channel();
        if store_jobs.send(StoreJob::Barrier(barrier)).is_ok() {
            let _ = tokio.block_on(flushed);
        }
        drop(store_jobs);
        drop(services);
        let _ = tokio.block_on(store_task);
        let _ = tokio.block_on(status_task);
        shared.lab_active.store(false, Ordering::Relaxed);
        tokio.shutdown_timeout(Duration::from_secs(1));
    }
}

impl MeterFeed for Runtime {
    fn run(self: Box<Self>, publish: Box<dyn Fn(FeedEvent) + Send>, commands: std::sync::mpsc::Receiver<Command>) {
        let publish = Mutex::new(publish);
        let shared = self.shared();
        self.serve(Arc::new(move |event| (publish.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))(event)), commands, shared);
    }
}
