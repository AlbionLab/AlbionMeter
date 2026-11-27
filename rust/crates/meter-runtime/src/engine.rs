//! The engine thread: the only owner of the meter's state. It is `CaptureService` (live capture, replay, the drop →
//! hold → reconnect → resume cycle and the record controls), `MeterSession` (names, saves, the published record) and
//! the `MeterWorker` loop in one place, so no lock guards any of it and no command can interleave with a packet.
//!
//! Frames arrive from the capture thread over a bounded channel; SQLite, files, AlbionLab and icons get owned copies
//! through [`StoreJob`]s and never make this thread wait. The UI gets at most four snapshots a second, only when the
//! record changed, carrying only the timeline points and contribution rows that changed.

use crate::config::Config;
use crate::feed::{self, Command, FeedEvent};
use crate::resolve::{self, LiveContext, Names};
use crate::services::{Identities, Seeds, StoreJob};
use crate::{LabView, Publisher, Shared};
use crossbeam_channel::{Receiver, Sender, select};
use meter_capture::{AdapterStatistics, CaptureError, CaptureFrame, CaptureThread, ExitReason};
use meter_core::capture::pcap::PcapFile;
use meter_core::cs::DateTimeOffset;
use meter_core::dungeon::DungeonRunSnapshot;
use meter_core::maps::MapCatalog;
use meter_core::observation::local::LocalCharacterSnapshot;
use meter_core::cs::CsMap;
use meter_core::{CaptureAnalysis, CombatEngine, RecordState};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The live dungeon runs, resolved, shared by the saves, the journal and AlbionLab.
type Runs = Arc<Vec<Arc<DungeonRunSnapshot>>>;

const TICK: Duration = Duration::from_millis(250);
/// How often the full capture report (counters, roster, weapons, dungeons, identities) is rebuilt while packets arrive.
const REPORT_EVERY: Duration = Duration::from_secs(1);
const RECORD_SAVE_SECONDS: f64 = 30.0;
const DUNGEON_SAVE_EVERY: Duration = Duration::from_secs(5);
const IDENTITY_SAVE_EVERY: Duration = Duration::from_secs(15);
const PLAYERS_SAVE_EVERY: Duration = Duration::from_secs(10);
const DUNGEONS_PUBLISH_EVERY: Duration = Duration::from_secs(2);
const MAX_REPLAY_BYTES: u64 = 40_000_000;
const REPLAY_LIBRARY: &str = "Albion Meter PCAP reader";

pub(crate) enum Input {
    Command(Command),
    Opened { generation: u64, result: Result<CaptureThread, CaptureError> },
    Catalog(Arc<MapCatalog>),
    /// Save everything, stop the capture, then answer.
    Shutdown(std::sync::mpsc::Sender<()>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Source {
    None,
    Live,
    Replay,
}

impl Source {
    fn text(self) -> &'static str {
        match self {
            Source::None => "none",
            Source::Live => "live-capture",
            Source::Replay => "replay",
        }
    }
}

/// `CaptureSessionStatus`; `has_report` is whether its `Report` would be set.
struct Status {
    state: &'static str,
    source: Source,
    error: Option<String>,
    stop_reason: Option<&'static str>,
    library: Option<String>,
    adapter: Option<AdapterStatistics>,
    has_report: bool,
}

impl Status {
    fn idle() -> Self {
        Self { state: "idle", source: Source::None, error: None, stop_reason: None, library: None, adapter: None, has_report: false }
    }
    fn busy(&self) -> bool {
        matches!(self.state, "starting" | "running" | "stopping")
    }
    fn capturing(&self) -> bool {
        matches!(self.state, "running" | "starting")
    }
    fn live_running(&self) -> bool {
        self.state == "running" && self.source == Source::Live
    }
}

struct LiveCapture {
    thread: CaptureThread,
    frames: Receiver<CaptureFrame>,
    recycle: Sender<Vec<u8>>,
}

struct Replay {
    analysis: CaptureAnalysis,
    frames: Option<Receiver<CaptureFrame>>,
    cancel: Arc<AtomicBool>,
    reader: Option<JoinHandle<Result<(), String>>>,
    /// The Replay command is answered when the file is done.
    reply: bool,
}

/// What the UI was last sent for the shown record, so the next snapshot carries only what changed.
#[derive(Default)]
struct Shown {
    key: Option<(String, DateTimeOffset, Source)>,
    since: i64,
    sequence: i64,
    status: &'static str,
    items: Vec<feed::ItemContribution>,
    index: HashMap<Arc<str>, usize>,
}

pub(crate) struct Engine {
    config: Config,
    shared: Arc<Shared>,
    publish: Publisher,
    store: tokio::sync::mpsc::UnboundedSender<StoreJob>,
    inputs: Receiver<Input>,
    inputs_tx: Sender<Input>,
    names: Names,
    seeds: Seeds,
    // CaptureService
    live: Option<CaptureAnalysis>,
    device: Option<String>,
    saved_device: Option<String>,
    status: Status,
    capture: Option<LiveCapture>,
    generation: u64,
    opening: bool,
    cancelled: bool,
    stop_requested: bool,
    stop_replies: u32,
    reconnecting: bool,
    resume_after_reconnect: bool,
    restart: Option<(Instant, String)>,
    replay: Option<Replay>,
    // MeterSession
    empty: CombatEngine,
    last_saved: f64,
    last_record: Option<(String, DateTimeOffset)>,
    last_dungeon_save: Option<Instant>,
    saved_dungeon_report: u64,
    saved_dungeon_encounters: HashMap<String, i64>,
    saved_runs: HashMap<String, (String, usize, i64, i64, [u64; 3])>,
    last_identity_save: Option<Instant>,
    // What the status line and the saves read from the current analysis, as the C# status report carried it; read
    // through the analysis' accessors so it costs the same whatever the length of the record.
    report: Option<Report>,
    report_serial: u64,
    context: LiveContext,
    dirty: bool,
    last_report: Option<Instant>,
    next_players: Instant,
    last_character: Option<LocalCharacterSnapshot>,
    last_party: Option<i64>,
    shown: Shown,
    oldest_frame: Option<i64>,
    open_dungeon: Option<String>,
    open_published: u64,
    resolved: Option<((u64, bool), Runs)>,
    /// Each live run as the analysis last shared it and resolved: a run the analysis hands back unchanged (the same
    /// `Arc`) is not resolved again.
    resolved_runs: HashMap<String, (Arc<DungeonRunSnapshot>, bool, Arc<DungeonRunSnapshot>)>,
    last_dungeons: Option<Instant>,
    lab_sequence: i64,
}

/// The light part of the C# status report.
struct Report {
    frames: i64,
    bytes: i64,
    counters: CsMap<String, i64>,
    diagnostics: CsMap<String, i64>,
    character: Option<LocalCharacterSnapshot>,
}

fn wall_us() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_micros() as i64)
}

impl Engine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Config,
        shared: Arc<Shared>,
        publish: Publisher,
        store: tokio::sync::mpsc::UnboundedSender<StoreJob>,
        inputs: Receiver<Input>,
        inputs_tx: Sender<Input>,
        maps: Arc<MapCatalog>,
        seeds: Seeds,
        saved_device: Option<String>,
    ) -> Self {
        Self {
            names: Names::new(maps, shared.sources.clone()),
            config,
            shared,
            publish,
            store,
            inputs,
            inputs_tx,
            seeds,
            live: None,
            device: None,
            saved_device,
            status: Status::idle(),
            capture: None,
            generation: 0,
            opening: false,
            cancelled: false,
            stop_requested: false,
            stop_replies: 0,
            reconnecting: false,
            resume_after_reconnect: false,
            restart: None,
            replay: None,
            empty: CombatEngine::empty(None),
            last_saved: 0.0,
            last_record: None,
            last_dungeon_save: None,
            saved_dungeon_report: 0,
            saved_dungeon_encounters: HashMap::new(),
            saved_runs: HashMap::new(),
            last_identity_save: None,
            report: None,
            report_serial: 0,
            context: LiveContext::default(),
            dirty: false,
            last_report: None,
            next_players: Instant::now() + PLAYERS_SAVE_EVERY,
            last_character: None,
            last_party: None,
            shown: Shown { since: -1, ..Shown::default() },
            oldest_frame: None,
            open_dungeon: None,
            open_published: 0,
            resolved: None,
            resolved_runs: HashMap::new(),
            last_dungeons: None,
            lab_sequence: -1,
        }
    }

    fn now(&self) -> i64 {
        (self.config.clock)()
    }

    fn job(&self, job: StoreJob) {
        let _ = self.store.send(job);
    }

    pub fn run(mut self) {
        self.publish_status();
        self.publish_record(true);
        if self.config.auto_start {
            self.auto_start();
        }
        let mut next_tick = Instant::now() + TICK;
        loop {
            let frames = self.frames().unwrap_or_else(crossbeam_channel::never);
            let timeout = next_tick.saturating_duration_since(Instant::now());
            select! {
                recv(self.inputs) -> input => match input {
                    Ok(Input::Shutdown(reply)) => {
                        self.shutdown();
                        let _ = reply.send(());
                        return;
                    }
                    Ok(input) => self.input(input),
                    Err(_) => {
                        self.shutdown();
                        return;
                    }
                },
                recv(frames) -> frame => match frame {
                    Ok(frame) => {
                        self.frame(frame);
                        for _ in 0..512 {
                            match frames.try_recv() {
                                Ok(frame) => self.frame(frame),
                                Err(_) => break,
                            }
                        }
                    }
                    Err(_) => self.source_finished(),
                },
                default(timeout) => {}
            }
            let now = Instant::now();
            if now >= next_tick {
                self.tick();
                self.shared.metrics.tick.record(now.elapsed().as_micros() as u64);
                next_tick += TICK;
                if next_tick < now {
                    next_tick = now + TICK;
                }
            }
        }
    }

    fn frames(&self) -> Option<Receiver<CaptureFrame>> {
        match self.status.source {
            Source::Live => self.capture.as_ref().map(|capture| capture.frames.clone()),
            Source::Replay => self.replay.as_ref().and_then(|replay| replay.frames.clone()),
            Source::None => None,
        }
    }

    fn frame(&mut self, frame: CaptureFrame) {
        match self.status.source {
            Source::Live => {
                let Some(capture) = &self.capture else { return };
                // A requested stop ends the record at once, as the C# loop leaves at the cancellation.
                if !self.cancelled
                    && let Some(analysis) = &mut self.live
                {
                    analysis.add(&frame);
                    let latency = wall_us() - frame.timestamp_us;
                    self.shared.metrics.packet_to_engine.record(latency.max(0) as u64);
                    self.oldest_frame.get_or_insert(frame.timestamp_us);
                    self.dirty = true;
                }
                let _ = capture.recycle.try_send(frame.data);
            }
            Source::Replay => {
                if let Some(replay) = &mut self.replay {
                    replay.analysis.add(&frame);
                    self.dirty = true;
                }
            }
            Source::None => return,
        }
        self.shared.metrics.frames.fetch_add(1, Ordering::Relaxed);
    }

    fn input(&mut self, input: Input) {
        match input {
            Input::Command(command) => self.command(command),
            Input::Opened { generation, result } => self.opened(generation, result),
            Input::Catalog(maps) => {
                self.names = Names::new(maps, self.shared.sources.clone());
                self.refresh();
                self.publish_record(false);
            }
            Input::Shutdown(_) => unreachable!(),
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::StartRecord | Command::PauseRecord | Command::ResumeRecord | Command::NewRecord | Command::StopRecord => {
                let result = self.record_command(&command);
                self.publish_status();
                self.publish_record(true);
                self.publish(FeedEvent::CommandDone(result.err()));
            }
            Command::StartCapture { device, method } => {
                let method = Some(method.as_str()).filter(|method| !method.is_empty());
                let result = meter_capture::validate_start(&device, method).and_then(|()| self.start(&device, false, false));
                self.publish_status();
                self.publish_record(false);
                self.publish(FeedEvent::CaptureDone(result.err()));
            }
            Command::StopCapture => self.stop_capture(),
            Command::Replay(path) => {
                if let Err(error) = self.replay(path) {
                    self.publish(FeedEvent::CaptureDone(Some(error)));
                }
            }
            Command::OpenDungeon(id) => self.open_dungeon(&id),
            Command::ExportHistory { dungeon, to } => {
                match &dungeon {
                    None => self.save(),
                    Some(id) => {
                        if let Some(run) = self.live_dungeons().iter().find(|run| *run.id == **id) {
                            self.job(StoreJob::SaveDungeon(run.clone()));
                        }
                    }
                }
                self.job(StoreJob::Export { dungeon: dungeon.map(|id| id.to_string()), to });
            }
            _ => {}
        }
    }

    // ---- CaptureService -------------------------------------------------------------------------------------

    fn create_live_analysis(&self) -> CaptureAnalysis {
        let mut analysis = CaptureAnalysis::new(Some(self.shared.sources.clone()), true);
        analysis.seed_characters(&self.seeds.characters);
        analysis.seed_skill_weapons(&self.seeds.skills);
        if let Some(character) = &self.seeds.character {
            analysis.restore_character(character.clone());
            analysis.restore_players(self.seeds.players.clone());
        }
        if let Some(party) = &self.seeds.party {
            analysis.restore_party(party.clone());
        }
        analysis
    }

    fn live_analysis(&mut self) -> &mut CaptureAnalysis {
        if self.live.is_none() {
            self.live = Some(self.create_live_analysis());
        }
        self.live.as_mut().unwrap()
    }

    /// `RecordDevice`: the device in use, else the saved one if it is still there, else the default.
    fn record_device(&self) -> Result<String, String> {
        if let Some(device) = &self.device {
            return Ok(device.clone());
        }
        let devices = self.config.capture.devices();
        devices
            .iter()
            .find(|device| Some(&device.name) == self.saved_device.as_ref())
            .map(|device| device.name.clone())
            .or_else(|| self.config.capture.default_device(&devices))
            .ok_or_else(|| "Choose a capture interface in Data first.".to_owned())
    }

    /// The record controls of `MeterSession.Command`; every change is followed by the report it leaves (the C#
    /// status carried it), and Stop, Pause, Resume and Reset save from that report.
    fn record_command(&mut self, command: &Command) -> Result<(), String> {
        match command {
            // A Play whose capture could not start has still begun and ended a record.
            Command::StartRecord => {
                let result = self.start_record();
                self.refresh();
                return result;
            }
            // The archive of the stopped record is saved inside, from the report of the stopped record.
            Command::StopRecord => self.stop_record()?,
            Command::PauseRecord | Command::ResumeRecord => {
                self.set_record_paused(*command == Command::PauseRecord)?;
                self.refresh();
            }
            Command::NewRecord => {
                self.reset_record()?;
                self.refresh();
            }
            _ => return Ok(()),
        }
        self.save_current();
        Ok(())
    }

    fn start_record(&mut self) -> Result<(), String> {
        if matches!(self.status.state, "starting" | "stopping") {
            return Err("Wait for the capture operation to finish.".into());
        }
        let device = self.record_device()?;
        let restart = !self.status.live_running();
        let (now, reconnecting) = (self.now(), self.reconnecting);
        let analysis = self.live_analysis();
        if matches!(analysis.record_state(), RecordState::Running | RecordState::Paused) {
            return Err("Stop the current record before starting another one.".into());
        }
        if restart {
            analysis.begin_capture(Some(now), true, reconnecting);
        }
        analysis.start_record(now).map_err(|error| error.to_string())?;
        // A capture that cannot start leaves no record "running" without packets: Play is available again.
        if restart && let Err(error) = self.start(&device, true, true) {
            let now = self.now();
            let _ = self.live_analysis().stop_record(now);
            return Err(error);
        }
        self.status.has_report = true;
        Ok(())
    }

    fn stop_record(&mut self) -> Result<(), String> {
        let now = self.now();
        let analysis = self.live.as_mut().ok_or("There is no active record to stop.")?;
        analysis.stop_record(now).map_err(|error| error.to_string())?;
        self.reconnecting = false;
        self.resume_after_reconnect = false;
        self.status.has_report = true;
        // Stop ends the record, not the capture: it keeps reading the game, so the next Play already knows every player.
        // The archive is the record as it stood, saved even without events.
        self.refresh();
        if let Some(record) = self.live_record() {
            self.job(StoreJob::SaveEncounter(Box::new(record), true));
        }
        Ok(())
    }

    fn reset_record(&mut self) -> Result<(), String> {
        let now = self.now();
        let analysis = self.live.as_mut().ok_or("There is no active record to reset.")?;
        analysis.reset_counts(now).map_err(|error| error.to_string())?;
        self.status.has_report = true;
        Ok(())
    }

    fn set_record_paused(&mut self, paused: bool) -> Result<(), String> {
        // While the capture is coming back after a drop the record waits paused; Pause and Resume only change whether
        // it continues by itself once the capture is back.
        if self.reconnecting && self.live.as_ref().is_some_and(|analysis| analysis.record_state() == RecordState::Paused) {
            self.resume_after_reconnect = !paused;
            return Ok(());
        }
        if !self.status.live_running() || self.live.is_none() {
            return Err("Press Play to start a record first.".into());
        }
        let now = self.now();
        let analysis = self.live.as_mut().unwrap();
        if analysis.record_state() != if paused { RecordState::Running } else { RecordState::Paused } {
            return Err(if paused { "The record is not running." } else { "The record is not paused." }.into());
        }
        analysis.set_paused(paused, now).map_err(|error| error.to_string())?;
        self.status.has_report = true;
        Ok(())
    }

    fn reserve(&mut self, source: Source) -> Result<(), String> {
        if self.status.busy() {
            return Err("Stop the current capture before starting another operation.".into());
        }
        self.job(StoreJob::PrepareCaptures);
        self.cancelled = false;
        self.status = Status { state: "starting", source, ..Status::idle() };
        Ok(())
    }

    /// `Start` plus the part of `RunLive` before the device opens; the device itself opens on a helper thread.
    fn start(&mut self, device: &str, record_prepared: bool, resume_record: bool) -> Result<(), String> {
        if !self.config.capture.devices().iter().any(|value| value.name == device) {
            return Err("Select an available capture interface.".into());
        }
        self.reserve(Source::Live)?;
        self.device = Some(device.to_owned());
        *self.shared.selected_device.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(device.to_owned());
        self.stop_requested = false;
        self.restart = None;
        if self.saved_device.as_deref() != Some(device) {
            self.saved_device = Some(device.to_owned());
            self.job(StoreJob::SaveDevice(device.to_owned()));
        }
        let continuing = self.live.is_some();
        let (now, reconnecting, resume) = (self.now(), self.reconnecting, self.resume_after_reconnect);
        let analysis = self.live_analysis();
        if !record_prepared {
            analysis.begin_capture(Some(now), continuing || resume_record, continuing && reconnecting);
        }
        // Back after a drop: the record goes on where it was, unless it was paused before or meanwhile.
        if reconnecting && resume && analysis.record_state() == RecordState::Paused {
            let _ = analysis.set_paused(false, now);
        }
        self.reconnecting = false;
        self.resume_after_reconnect = false;
        // The status no longer carries a report until the capture runs (or Play set one).
        self.refresh();
        self.generation += 1;
        self.opening = true;
        let (generation, backend, capacity, inputs, device) = (self.generation, self.config.capture.clone(), self.config.channel_capacity, self.inputs_tx.clone(), device.to_owned());
        let spawned = std::thread::Builder::new().name("capture-open".into()).spawn(move || {
            let result = backend.open(&device).and_then(|source| CaptureThread::spawn(source, capacity).map_err(|error| CaptureError::Io(error.to_string())));
            let _ = inputs.send(Input::Opened { generation, result });
        });
        if let Err(error) = spawned {
            self.opening = false;
            self.failed(CaptureError::Io(error.to_string()));
        }
        Ok(())
    }

    fn opened(&mut self, generation: u64, result: Result<CaptureThread, CaptureError>) {
        if generation != self.generation || !self.opening {
            return;
        }
        self.opening = false;
        match result {
            Err(error) => self.failed(error),
            Ok(thread) => {
                let device = self.device.clone().unwrap_or_default();
                if self.cancelled {
                    let exit = thread.join();
                    self.ended(exit.reason, exit.statistics, self.config.capture.library(Some(&device)));
                    return;
                }
                self.status = Status {
                    state: "running",
                    source: Source::Live,
                    library: Some(self.config.capture.library(Some(&device))),
                    adapter: thread.statistics(),
                    has_report: true,
                    ..Status::idle()
                };
                self.capture = Some(LiveCapture { frames: thread.frames().clone(), recycle: thread.recycler(), thread });
                self.refresh();
                self.publish_status();
                self.publish_record(false);
            }
        }
    }

    /// The capture thread is gone: the source ended, failed, or the stop it was asked for happened.
    fn source_finished(&mut self) {
        match self.status.source {
            Source::Live => {
                let Some(capture) = self.capture.take() else { return };
                let library = self.status.library.clone().unwrap_or_default();
                let counters = capture.thread.counters();
                let exit = capture.thread.wait();
                self.shared.metrics.channel_dropped.fetch_add(counters.channel_dropped, Ordering::Relaxed);
                self.ended(exit.reason, exit.statistics, library);
            }
            Source::Replay => self.replay_finished(),
            Source::None => {}
        }
    }

    /// The end of `RunLive`: a source that ended by itself (an interface hiccup, the game reconnecting) is not the
    /// player stopping: the record is kept, paused, and continues when the capture comes back. Only a requested stop
    /// ends it.
    fn ended(&mut self, reason: ExitReason, statistics: Option<AdapterStatistics>, library: String) {
        let reason = match reason {
            ExitReason::Failed(error) => return self.failed(error),
            ExitReason::SourceEnded => "sourceEnded",
            ExitReason::Stopped => "stopped",
        };
        let dropped = !self.cancelled && reason == "sourceEnded";
        let now = self.now();
        if dropped {
            self.hold_record();
        } else if let Some(analysis) = &mut self.live
            && matches!(analysis.record_state(), RecordState::Running | RecordState::Paused)
        {
            let _ = analysis.stop_record(now);
        }
        if let Some(analysis) = &mut self.live {
            analysis.seal_record();
        }
        self.complete(Status { state: "completed", source: Source::Live, adapter: statistics, library: Some(library), stop_reason: Some(reason), has_report: true, ..Status::idle() });
        if dropped {
            self.restart_later();
        }
    }

    /// The `catch` of `RunLive`.
    fn failed(&mut self, error: CaptureError) {
        let permission = error.is_permission();
        let now = self.now();
        if !self.cancelled && !permission {
            self.hold_record();
        } else if permission
            && let Some(analysis) = &mut self.live
            && matches!(analysis.record_state(), RecordState::Running | RecordState::Paused)
        {
            // Without permission the capture does not come back by itself: the record ends instead of waiting forever.
            let _ = analysis.stop_record(now);
        }
        self.complete(Status { state: "failed", source: Source::Live, error: Some(error.message().to_owned()), stop_reason: Some("error"), has_report: true, ..Status::idle() });
        if permission {
            eprintln!("Packet capture failed: {}", error.message());
        }
        if !self.cancelled && !permission {
            self.restart_later();
        }
    }

    /// Marks a capture that dropped so its restart keeps every identity it knew, with or without a record, and
    /// pauses a running record, remembering to continue it once the capture is back.
    fn hold_record(&mut self) {
        self.reconnecting = true;
        let now = self.now();
        let Some(analysis) = &mut self.live else { return };
        self.resume_after_reconnect = analysis.record_state() == RecordState::Running;
        if self.resume_after_reconnect {
            let _ = analysis.set_paused(true, now);
        }
    }

    fn restart_later(&mut self) {
        if let Some(device) = self.device.clone() {
            self.restart = Some((Instant::now() + self.config.reconnect_delay, device));
        }
    }

    fn complete(&mut self, status: Status) {
        self.status = status;
        self.refresh();
        self.publish_status();
        self.publish_record(false);
        for _ in 0..std::mem::take(&mut self.stop_replies) {
            self.publish(FeedEvent::CaptureDone(None));
        }
    }

    fn stop_capture(&mut self) {
        if self.status.capturing() {
            self.status.state = "stopping";
        }
        self.stop_requested = true;
        self.restart = None;
        let worker = self.opening || self.capture.is_some() || self.replay.as_ref().is_some_and(|replay| replay.frames.is_some());
        self.cancelled = true;
        if let Some(capture) = &self.capture {
            capture.thread.cancel();
        }
        if let Some(replay) = &self.replay {
            replay.cancel.store(true, Ordering::Relaxed);
        }
        self.publish_status();
        if worker {
            self.stop_replies += 1;
        } else {
            self.publish(FeedEvent::CaptureDone(None));
        }
    }

    fn auto_start(&mut self) {
        let devices = self.config.capture.devices();
        let device = devices
            .iter()
            .find(|device| Some(&device.name) == self.saved_device.as_ref())
            .map(|device| device.name.clone())
            .or_else(|| self.config.capture.default_device(&devices));
        if let Some(device) = device {
            let _ = self.start(&device, false, false);
            self.publish_status();
        }
    }

    fn replay(&mut self, path: PathBuf) -> Result<(), String> {
        if std::fs::metadata(&path).map_or(true, |metadata| metadata.len() == 0 || metadata.len() > MAX_REPLAY_BYTES) {
            return Err("Choose a capture file smaller than 40 MB.".into());
        }
        self.reserve(Source::Replay)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, frames) = crossbeam_channel::bounded(1024);
        let flag = cancel.clone();
        let reader = std::thread::Builder::new()
            .name("replay".into())
            .spawn(move || {
                let mut file = PcapFile::open(&path).map_err(|error| error.to_string())?;
                while !flag.load(Ordering::Relaxed) {
                    let mut frame = CaptureFrame::default();
                    if !file.read_into(&mut frame).map_err(|error| error.to_string())? || sender.send(frame).is_err() {
                        break;
                    }
                }
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        self.replay = Some(Replay { analysis: CaptureAnalysis::new(Some(self.shared.sources.clone()), false), frames: Some(frames), cancel, reader: Some(reader), reply: true });
        self.status = Status { state: "running", source: Source::Replay, library: Some(REPLAY_LIBRARY.into()), has_report: true, ..Status::idle() };
        self.publish_status();
        Ok(())
    }

    fn replay_finished(&mut self) {
        let Some(replay) = &mut self.replay else { return };
        replay.frames = None;
        let result = replay.reader.take().map_or(Ok(()), |reader| reader.join().unwrap_or_else(|_| Err("The replay stopped unexpectedly.".into())));
        let cancelled = replay.cancel.load(Ordering::Relaxed);
        let reply = std::mem::take(&mut replay.reply);
        let status = match &result {
            Ok(()) => Status { state: "completed", source: Source::Replay, library: Some(REPLAY_LIBRARY.into()), stop_reason: Some(if cancelled { "stopped" } else { "endOfFile" }), has_report: true, ..Status::idle() },
            Err(error) => Status { state: "failed", source: Source::Replay, error: Some(error.clone()), stop_reason: Some("error"), has_report: true, ..Status::idle() },
        };
        self.complete(status);
        if reply {
            self.publish(FeedEvent::CaptureDone(result.err()));
        }
    }

    // ---- MeterSession -------------------------------------------------------------------------------------------

    fn current(&mut self) -> Option<&mut CaptureAnalysis> {
        if !self.status.has_report {
            return None;
        }
        match self.status.source {
            Source::Live => self.live.as_mut(),
            Source::Replay => self.replay.as_mut().map(|replay| &mut replay.analysis),
            Source::None => None,
        }
    }

    /// Rebuilds the capture report the C# status carried, and what follows from it: the names context, the local
    /// character files and the status line.
    fn refresh(&mut self) {
        let (replay, running) = (self.status.source == Source::Replay, self.status.capturing());
        let Some(analysis) = self.current() else {
            self.report = None;
            self.resolved = None;
            self.report_serial += 1;
            return;
        };
        let started = Instant::now();
        let report = Report {
            frames: analysis.frames(),
            bytes: analysis.bytes(),
            counters: analysis.counters(),
            diagnostics: analysis.combat_diagnostics(),
            character: analysis.character().cloned(),
        };
        let party = analysis.party().cloned();
        self.context = LiveContext::of(analysis, replay, running);
        if self.status.source == Source::Live {
            if let Some(character) = &report.character
                && self.last_character.as_ref() != Some(character)
            {
                self.last_character = Some(character.clone());
                if let Ok(value) = serde_json::to_value(character) {
                    self.job(StoreJob::Character(value));
                }
            }
            if let Some(party) = &party
                && self.last_party.is_none_or(|observed| party.observed_at_microseconds > observed)
            {
                self.last_party = Some(party.observed_at_microseconds);
                if let Ok(value) = serde_json::to_value(party) {
                    self.job(StoreJob::Party(value));
                }
            }
        }
        if let Some(capture) = &self.capture {
            self.status.adapter = capture.thread.statistics().or(self.status.adapter);
        }
        self.report = Some(report);
        self.resolved = None;
        self.shared.metrics.report.record(started.elapsed().as_micros() as u64);
        self.report_serial += 1;
        self.dirty = false;
        self.last_report = Some(Instant::now());
    }

    /// The current record, resolved (`Live()`), from the analysis rather than the last report.
    fn live_record(&mut self) -> Option<meter_core::CombatSnapshot> {
        let delta = self.current()?.combat_delta(-1)?;
        Some(self.names.resolve_live(&self.context, delta.snapshot))
    }

    /// `LiveDungeons`, from the last report; resolved once per report and shared by the saves, the journal and
    /// AlbionLab.
    fn live_dungeons(&mut self) -> Runs {
        if self.status.source != Source::Live {
            return Arc::default();
        }
        let key = (self.report_serial, self.status.capturing());
        if let Some((cached, runs)) = &self.resolved
            && *cached == key
        {
            return runs.clone();
        }
        // Only the runs that changed since they were last resolved (the running one) are copied and resolved again.
        let Some(analysis) = self.current() else { return Arc::default() };
        let shared = analysis.dungeons();
        let mut resolved = Vec::with_capacity(shared.len());
        for run in shared {
            let reuse = self.resolved_runs.get(&run.id).filter(|(raw, running, _)| Arc::ptr_eq(raw, &run) && *running == key.1).map(|entry| entry.2.clone());
            let done = match reuse {
                Some(done) => done,
                None => {
                    let done = Arc::new(self.names.resolve_dungeons(vec![(*run).clone()], key.1).remove(0));
                    self.resolved_runs.insert(run.id.clone(), (run, key.1, done.clone()));
                    done
                }
            };
            resolved.push(done);
        }
        self.resolved_runs.retain(|id, _| resolved.iter().any(|run| run.id == *id));
        let runs: Runs = Arc::new(resolved);
        self.resolved = Some((key, runs.clone()));
        runs
    }

    fn save_live_dungeons(&mut self, force: bool) {
        if !force && self.last_dungeon_save.is_some_and(|saved| saved.elapsed() < DUNGEON_SAVE_EVERY) {
            return;
        }
        self.last_dungeon_save = Some(Instant::now());
        // Nothing changed since the last save: the same rows would be written again.
        if !force && self.saved_dungeon_report == self.report_serial {
            return;
        }
        self.saved_dungeon_report = self.report_serial;
        for run in self.live_dungeons().iter() {
            for encounter in run.floors.iter().flat_map(|floor| &floor.encounters) {
                if self.saved_dungeon_encounters.get(&encounter.id) == Some(&encounter.sequence) {
                    continue;
                }
                self.saved_dungeon_encounters.insert(encounter.id.clone(), encounter.sequence);
                self.job(StoreJob::SaveEncounter(Box::new(encounter.clone()), false));
            }
            // A run that did not change since it was saved (an earlier, finished run of this capture) would be
            // written again as it is.
            let totals = &run.totals;
            let signature = (run.status.clone(), run.floors.len(), totals.sequence, totals.event_count, [totals.elapsed_seconds, totals.total_damage, totals.total_healing].map(f64::to_bits));
            if self.saved_runs.get(&run.id) != Some(&signature) {
                self.saved_runs.insert(run.id.clone(), signature);
                self.job(StoreJob::SaveDungeon(run.clone()));
            }
        }
        self.publish_dungeons(true);
    }

    fn save_current(&mut self) {
        self.save_live_dungeons(true);
        if let Some(record) = self.live_record().filter(|record| record.event_count > 0) {
            self.job(StoreJob::SaveEncounter(Box::new(record), false));
        }
    }

    fn save_identities(&mut self, force: bool) {
        if !force && self.last_identity_save.is_some_and(|saved| saved.elapsed() < IDENTITY_SAVE_EVERY) {
            return;
        }
        let debug = self.config.debug;
        if self.report.is_none() {
            return;
        }
        let Some(analysis) = self.current() else { return };
        let found = analysis.identities();
        self.last_identity_save = Some(Instant::now());
        let identities = Identities {
            characters: found.characters,
            unidentified: found.unidentified,
            sightings: found.sightings,
            skills: Some(found.skill_weapons),
            timeline: debug.then_some(found.timeline),
        };
        self.job(StoreJob::Identities(Box::new(identities)));
    }

    /// `MeterSession.Save`.
    fn save(&mut self) {
        self.refresh();
        self.save_identities(true);
        self.save_current();
    }

    fn shutdown(&mut self) {
        self.save();
        if let Some(capture) = &self.capture {
            capture.thread.cancel();
        }
        self.cancelled = true;
        if let Some(capture) = self.capture.take() {
            let exit = capture.thread.join();
            let library = self.status.library.clone().unwrap_or_default();
            self.ended(exit.reason, exit.statistics, library);
        }
        if let Some(replay) = &self.replay {
            replay.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn tick(&mut self) {
        if let Some((at, device)) = self.restart.clone()
            && Instant::now() >= at
        {
            self.restart = None;
            if !self.stop_requested && !self.status.busy() {
                let _ = self.start(&device, false, true);
                self.publish_status();
            }
        }
        if self.status.live_running() {
            let now = self.now();
            if let Some(analysis) = &mut self.live {
                analysis.advance_clock(now);
            }
            if Instant::now() >= self.next_players {
                self.next_players = Instant::now() + PLAYERS_SAVE_EVERY;
                if let Some(analysis) = &self.live
                    && let Some(character) = analysis.character()
                {
                    let players = analysis.known_players();
                    if let (Ok(character), Ok(players)) = (serde_json::to_value(character), players.iter().map(serde_json::to_value).collect::<Result<Vec<_>, _>>()) {
                        self.job(StoreJob::Players(character, players));
                    }
                }
            }
        }
        if self.dirty && self.last_report.is_none_or(|refreshed| refreshed.elapsed() >= REPORT_EVERY) {
            self.refresh();
            self.publish_status();
        }
        self.save_identities(false);
        self.save_live_dungeons(false);
        self.publish_record(false);
        self.publish_dungeons(false);
        self.update_lab();
    }

    // ---- Publishing -----------------------------------------------------------------------------------------------

    fn publish(&self, event: FeedEvent) {
        (self.publish)(event);
    }

    fn publish_status(&self) {
        let status = &self.status;
        let report = self.report.as_ref().filter(|_| status.has_report).map(|report| {
            let diagnostics = report.diagnostics.iter();
            let mut counters: Vec<(Arc<str>, f64)> = report.counters.iter().map(|(key, value)| (Arc::from(key.as_str()), *value as f64)).collect();
            for (key, value) in diagnostics.clone() {
                match counters.iter_mut().find(|(name, _)| **name == **key) {
                    Some(entry) => entry.1 = *value as f64,
                    None => counters.push((key.as_str().into(), *value as f64)),
                }
            }
            feed::CaptureReport {
                frames: report.frames.max(0) as u64,
                bytes: report.bytes.max(0) as u64,
                pending_unidentified: diagnostics.clone().find(|(key, _)| *key == "pendingUnidentified").map_or(0, |(_, value)| (*value).max(0) as u64),
                transport_messages: report.counters.get("transportMessages").map_or(0, |value| (*value).max(0) as u64),
                counters,
            }
        });
        self.publish(FeedEvent::Capture(Arc::new(feed::CaptureStatus {
            state: status.state.into(),
            source: status.source.text().into(),
            file: None,
            error: status.error.clone(),
            stop_reason: status.stop_reason.map(Into::into),
            library: status.library.clone(),
            adapter_dropped: status.adapter.map(|adapter| u64::from(adapter.dropped)),
            report,
        })));
    }

    /// `MeterSession.Update` after the saves: the record when it changed, with the 30-second save. `force` answers a
    /// command, which always shows the record as it now is.
    fn publish_record(&mut self, force: bool) {
        let (source, since) = (self.status.source, self.shown.since);
        let Some(delta) = self.current().and_then(|analysis| analysis.combat_delta(since)) else {
            return self.publish_unavailable(force);
        };
        let key = (delta.snapshot.id.clone(), delta.snapshot.started_at, source);
        let status = self.context.status;
        if self.shown.key.as_ref() != Some(&key) {
            // A new record, or a Reset (same id, new start): everything again, and the 30-second saves start over.
            if self.last_record.as_ref() != Some(&(key.0.clone(), key.1)) {
                self.last_record = Some((key.0.clone(), key.1));
                self.last_saved = 0.0;
            }
            self.shown = Shown { key: Some(key), since: -1, sequence: -1, ..Shown::default() };
            let delta = if since < 0 { Some(delta) } else { self.current().and_then(|analysis| analysis.combat_delta(-1)) };
            if let Some(full) = delta {
                self.send_record(full, status);
            }
            return;
        }
        if !force && self.shown.sequence == delta.sequence && self.shown.status == status {
            return;
        }
        self.send_record(delta, status);
    }

    fn send_record(&mut self, delta: meter_core::CombatDelta, status: &'static str) {
        let combat = &delta.snapshot;
        if delta.full {
            self.shown.items.clear();
            self.shown.index.clear();
        }
        for removed in &delta.changed.removed_contributions {
            if let Some(index) = self.shown.index.remove(removed.as_str()) {
                self.shown.items.remove(index);
                for (position, item) in self.shown.items.iter().enumerate().skip(index) {
                    self.shown.index.insert(item.id.clone(), position);
                }
            }
        }
        for item in combat.item_contributions.iter().flatten() {
            let row = self.names.feed_item(item);
            match self.shown.index.get(item.id.as_str()) {
                Some(&index) => self.shown.items[index] = row,
                None => {
                    self.shown.index.insert(row.id.clone(), self.shown.items.len());
                    self.shown.items.push(row);
                }
            }
        }
        let snapshot = feed::Snapshot {
            id: combat.id.as_str().into(),
            started_at: combat.started_at.to_string().into(),
            elapsed_seconds: combat.elapsed_seconds,
            status: resolve::status(status),
            mode: self.context.mode.into(),
            sequence: delta.sequence.max(0) as u64,
            event_count: combat.event_count.max(0) as u64,
            total_damage: combat.total_damage,
            total_healing: combat.total_healing,
            members: combat.members.iter().map(|player| self.names.feed_player(&self.context, player, false)).collect(),
            former_members: combat.former_members.iter().flatten().map(|player| self.names.feed_player(&self.context, player, true)).collect(),
            item_contributions: self.shown.items.clone(),
            timeline: combat.timeline.iter().map(|point| resolve::feed_point(point)).collect(),
            full: delta.full,
        };
        let elapsed = combat.elapsed_seconds;
        self.shown.since = delta.sequence;
        self.shown.sequence = delta.sequence;
        self.shown.status = status;
        self.publish(FeedEvent::Snapshot(Arc::new(snapshot)));
        let metrics = &self.shared.metrics;
        metrics.publishes.fetch_add(1, Ordering::Relaxed);
        if let Some(oldest) = self.oldest_frame.take() {
            metrics.packet_to_publish.record((wall_us() - oldest).max(0) as u64);
        }
        if elapsed - self.last_saved >= RECORD_SAVE_SECONDS {
            self.last_saved = elapsed;
            if let Some(record) = self.live_record() {
                self.job(StoreJob::SaveEncounter(Box::new(record), false));
            }
        }
    }

    fn publish_unavailable(&mut self, force: bool) {
        let key = (self.empty.id().to_owned(), self.empty.started_at(), Source::None);
        if !force && self.shown.key.as_ref() == Some(&key) {
            return;
        }
        let combat = self.empty.full_snapshot("idle");
        self.shown = Shown { key: Some(key), since: -1, sequence: combat.sequence, status: "idle", ..Shown::default() };
        let snapshot = feed::Snapshot {
            id: combat.id.as_str().into(),
            started_at: combat.started_at.to_string().into(),
            status: feed::RecordStatus::Idle,
            mode: "unavailable".into(),
            sequence: combat.sequence.max(0) as u64,
            full: true,
            ..feed::Snapshot::default()
        };
        self.publish(FeedEvent::Snapshot(Arc::new(snapshot)));
    }

    /// What AlbionLab reads (`session.Snapshot()`, `session.Dungeons()`, the local character), refreshed at most once a
    /// second and only while an account is paired.
    fn update_lab(&mut self) {
        let sequence = self.shown.sequence;
        if !self.shared.lab_active.load(Ordering::Relaxed) {
            return;
        }
        let updated = self.shared.lab.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).updated;
        if updated.is_some_and(|updated| sequence == self.lab_sequence || updated.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.lab_sequence = sequence;
        let record = self.live_record().unwrap_or_else(|| {
            let mut empty = self.empty.full_snapshot("idle");
            empty.mode = "unavailable".into();
            empty.location = None;
            empty.party_id = None;
            empty
        });
        let view = LabView {
            record: Arc::new(record),
            dungeons: self.live_dungeons(),
            character: self.report.as_ref().and_then(|report| report.character.as_ref()).map(|character| (character.character_id.to_string(), character.map_id.clone())),
            updated: Some(Instant::now()),
        };
        *self.shared.lab.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = view;
    }

    /// `GET /api/dungeons`: the runs of this capture first, then the saved ones; and the open run while it is live.
    fn publish_dungeons(&mut self, force: bool) {
        if !force && self.last_dungeons.is_some_and(|published| published.elapsed() < DUNGEONS_PUBLISH_EVERY) {
            return;
        }
        let live = self.live_dungeons();
        if !force && live.is_empty() && self.last_dungeons.is_some() {
            return;
        }
        self.last_dungeons = Some(Instant::now());
        let mut current: Vec<&Arc<DungeonRunSnapshot>> = live.iter().filter(|run| run.status != "idle").collect();
        current.sort_by_key(|run| std::cmp::Reverse(run.started_at));
        let summaries = current.iter().map(|run| resolve::dungeon_summary(run)).collect();
        self.job(StoreJob::Dungeons(summaries));
        // The open run again only when the report it comes from is new.
        if let Some(open) = &self.open_dungeon
            && self.open_published != self.report_serial
            && let Some(run) = live.iter().find(|run| run.id == *open)
        {
            self.open_published = self.report_serial;
            self.publish(FeedEvent::Dungeon(Arc::new(self.names.feed_run(run))));
        }
    }

    fn open_dungeon(&mut self, id: &str) {
        self.open_dungeon = Some(id.to_owned());
        let live = self.live_dungeons();
        match live.iter().find(|run| run.id == id) {
            Some(run) => {
                self.open_published = self.report_serial;
                self.publish(FeedEvent::Dungeon(Arc::new(self.names.feed_run(run))));
            }
            None => self.job(StoreJob::Dungeon(id.to_owned())),
        }
    }
}
