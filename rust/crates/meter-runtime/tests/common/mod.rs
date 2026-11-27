//! What the runtime tests share: a fake capture backend whose devices are scripted frame sources, a manual clock,
//! and a collector for the events the runtime publishes.
#![allow(dead_code)]

use crossbeam_channel::{Receiver, Sender};
use meter_capture::{AdapterStatistics, CaptureDevice, CaptureError, CaptureFrame, DeviceList, DeviceEntry, FrameSource, MethodEntry, Read};
use meter_core::capture::protocol18::{Value, WireMessage};
use meter_core::cs::Guid;
use meter_core::testkit::{guid_bytes, message_frame, msg, strings};
use meter_runtime::feed::{CaptureStatus, FeedEvent, Snapshot};
use meter_runtime::{CaptureBackend, Clock, Config, Handle, Runtime};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub const DEVICE: &str = "raw:test";

/// Panics of the runtime's own threads (meter, engine, meter-io), so a wait reports the panic instead of timing out.
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn capture_panics() {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current().name().unwrap_or("unnamed").to_owned();
            if !thread.starts_with("meter") && thread != "engine" {
                return default(info);
            }
            let payload = info.payload().downcast_ref::<&str>().map(|text| text.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
            let location = info.location().map(|at| at.to_string()).unwrap_or_default();
            PANICS.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(format!("thread '{thread}' panicked at {location}: {payload}"));
            default(info);
        }));
    });
}

fn runtime_panics() -> Vec<String> {
    PANICS.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

pub fn temp_dir(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!("meter-runtime-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

pub fn repo_assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../assets/game-data")
}

pub enum Step {
    Frame(CaptureFrame),
    End,
    Fail(CaptureError),
}

/// A live source the test feeds frame by frame; it ends or fails when told to.
pub struct ScriptedSource(Receiver<Step>);

impl FrameSource for ScriptedSource {
    fn link_type(&self) -> i32 {
        101
    }
    fn read(&mut self, frame: &mut CaptureFrame) -> Result<Read, CaptureError> {
        match self.0.recv_timeout(Duration::from_millis(20)) {
            Ok(Step::Frame(next)) => {
                *frame = next;
                Ok(Read::Frame)
            }
            Ok(Step::End) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Ok(Read::Ended),
            Ok(Step::Fail(error)) => Err(error),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Ok(Read::Idle),
        }
    }
    fn statistics(&mut self) -> Option<AdapterStatistics> {
        Some(AdapterStatistics::default())
    }
}

pub type Opener = Box<dyn FnOnce() -> Result<Box<dyn FrameSource>, CaptureError> + Send>;

/// One device, `raw:test`; each open takes the next scripted source (or error).
#[derive(Default)]
pub struct FakeCapture {
    queue: Mutex<VecDeque<Opener>>,
    pub opens: AtomicUsize,
}

impl FakeCapture {
    /// Queues a source and returns the sender that drives it.
    pub fn source(&self) -> Sender<Step> {
        let (sender, receiver) = crossbeam_channel::unbounded();
        self.queue.lock().unwrap().push_back(Box::new(move || Ok(Box::new(ScriptedSource(receiver)) as Box<dyn FrameSource>)));
        sender
    }
    pub fn fail(&self, error: CaptureError) {
        self.queue.lock().unwrap().push_back(Box::new(move || Err(error)));
    }
    pub fn custom(&self, open: Opener) {
        self.queue.lock().unwrap().push_back(open);
    }
}

impl CaptureBackend for FakeCapture {
    fn devices(&self) -> Vec<CaptureDevice> {
        vec![CaptureDevice { name: DEVICE.into(), description: "Test socket".into(), flags: 1 }]
    }
    fn default_device(&self, _: &[CaptureDevice]) -> Option<String> {
        Some(DEVICE.into())
    }
    fn device_list(&self) -> DeviceList {
        DeviceList {
            available: true,
            library: Some("Test socket".into()),
            selected_device: Some(DEVICE.into()),
            methods: vec![MethodEntry { id: "socket", name: "System socket (Linux)", available: true }, MethodEntry { id: "pcap", name: "libpcap", available: false }],
            devices: vec![DeviceEntry { name: DEVICE.into(), description: "Test socket".into(), flags: 1, method: "socket" }],
            error: None,
        }
    }
    fn library(&self, _: Option<&str>) -> String {
        "Test socket".into()
    }
    fn open(&self, _: &str) -> Result<Box<dyn FrameSource>, CaptureError> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        let open = self.queue.lock().unwrap().pop_front();
        match open {
            Some(open) => open(),
            None => Err(CaptureError::Io("No scripted source left.".into())),
        }
    }
}

pub struct ManualClock(Arc<AtomicI64>);

impl ManualClock {
    pub fn new(at: i64) -> (Self, Clock) {
        let time = Arc::new(AtomicI64::new(at));
        let read = time.clone();
        (Self(time), Arc::new(move || read.load(Ordering::Relaxed)))
    }
    pub fn set(&self, at: i64) {
        self.0.store(at, Ordering::Relaxed);
    }
}

/// The events the runtime published, in order; positions count from the first event even when `keep` drops the
/// oldest ones (the soak keeps only a tail, so hours of snapshots do not count as the runtime's memory).
#[derive(Clone, Default)]
pub struct Events(Arc<(Mutex<Log>, Condvar)>);

#[derive(Default)]
pub struct Log {
    events: Vec<FeedEvent>,
    dropped: usize,
    keep: Option<usize>,
}

impl Events {
    pub fn publisher(&self) -> impl Fn(FeedEvent) + Send + Sync + 'static {
        let events = self.0.clone();
        move |event| {
            let mut log = events.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            log.events.push(event);
            if let Some(keep) = log.keep
                && log.events.len() > 2 * keep
            {
                let excess = log.events.len() - keep;
                log.events.drain(..excess);
                log.dropped += excess;
            }
            events.1.notify_all();
        }
    }

    pub fn keep(&self, events: usize) {
        self.0.0.lock().unwrap().keep = Some(events);
    }

    /// The events still kept.
    pub fn all(&self) -> Vec<FeedEvent> {
        self.0.0.lock().unwrap().events.clone()
    }

    pub fn len(&self) -> usize {
        let log = self.0.0.lock().unwrap();
        log.dropped + log.events.len()
    }

    /// Waits for an event after position `from` that `matches` accepts.
    pub fn wait_from<T>(&self, from: usize, timeout: Duration, mut matches: impl FnMut(&FeedEvent) -> Option<T>) -> T {
        let deadline = Instant::now() + timeout;
        let mut seen = from;
        let mut log = self.0.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            let start = seen.saturating_sub(log.dropped).min(log.events.len());
            for event in &log.events[start..] {
                if let Some(found) = matches(event) {
                    return found;
                }
            }
            seen = log.dropped + log.events.len();
            let now = Instant::now();
            let panics = runtime_panics();
            if now >= deadline || !panics.is_empty() {
                // Fail without holding the log: the runtime's threads keep publishing into it.
                let last = format!("{:#?}", &log.events[log.events.len().saturating_sub(5)..]);
                drop(log);
                if !panics.is_empty() {
                    panic!("the runtime panicked: {}\nlast events: {last}", panics.join("\n"));
                }
                panic!("timed out waiting for an event; last events: {last}");
            }
            let wait = (deadline - now).min(Duration::from_millis(100));
            log = self.0.1.wait_timeout(log, wait).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
    }

    /// The answer to the command sent after position `from`.
    pub fn command_done(&self, from: usize) -> Option<String> {
        self.wait_from(from, Duration::from_secs(10), |event| match event {
            FeedEvent::CommandDone(result) => Some(result.clone()),
            _ => None,
        })
    }

    pub fn capture_done(&self, from: usize) -> Option<String> {
        self.wait_from(from, Duration::from_secs(30), |event| match event {
            FeedEvent::CaptureDone(result) => Some(result.clone()),
            _ => None,
        })
    }

    pub fn last_snapshot(&self) -> Arc<Snapshot> {
        self.all().iter().rev().find_map(|event| if let FeedEvent::Snapshot(snapshot) = event { Some(snapshot.clone()) } else { None }).expect("a snapshot")
    }

    pub fn last_capture(&self) -> Arc<CaptureStatus> {
        self.all().iter().rev().find_map(|event| if let FeedEvent::Capture(status) = event { Some(status.clone()) } else { None }).expect("a capture status")
    }

    pub fn wait_capture(&self, from: usize, state: &str) -> Arc<CaptureStatus> {
        self.wait_from(from, Duration::from_secs(10), |event| match event {
            FeedEvent::Capture(status) if &*status.state == state => Some(status.clone()),
            _ => None,
        })
    }

    pub fn wait_snapshot(&self, from: usize, mut matches: impl FnMut(&Snapshot) -> bool) -> Arc<Snapshot> {
        self.wait_from(from, Duration::from_secs(10), |event| match event {
            FeedEvent::Snapshot(snapshot) if matches(snapshot) => Some(snapshot.clone()),
            _ => None,
        })
    }
}

pub struct Meter {
    pub events: Events,
    pub handle: Option<Handle>,
    pub storage: PathBuf,
}

impl Meter {
    pub fn start(config: Config) -> Self {
        capture_panics();
        let events = Events::default();
        let storage = config.storage.clone();
        let started = Instant::now();
        let handle = Runtime::new(config).start(events.publisher());
        events.wait_from(0, Duration::from_secs(60), |event| matches!(event, FeedEvent::Connection(_)).then_some(()));
        if started.elapsed() > Duration::from_secs(10) {
            eprintln!("the runtime took {:?} to start", started.elapsed());
        }
        Self { events, handle: Some(handle), storage }
    }

    pub fn handle(&self) -> &Handle {
        self.handle.as_ref().unwrap()
    }

    /// Sends a record command and returns its answer.
    pub fn record(&self, command: meter_runtime::Command) -> Option<String> {
        let from = self.events.len();
        self.handle().send(command);
        self.events.command_done(from)
    }

    pub fn start_capture(&self) {
        let from = self.events.len();
        self.handle().send(meter_runtime::Command::StartCapture { device: DEVICE.into(), method: "socket".into() });
        assert_eq!(self.events.capture_done(from), None);
        self.events.wait_capture(from, "running");
    }

    /// Waits until the engine has taken `frames` frames in all.
    pub fn wait_frames(&self, frames: u64) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.handle().metrics().frames < frames {
            assert!(Instant::now() < deadline, "the engine took {} of {frames} frames", self.handle().metrics().frames);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Lets the engine tick (publish, save) at least once more.
    pub fn settle(&self) {
        std::thread::sleep(Duration::from_millis(600));
    }

    pub fn shutdown(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown();
        }
    }
}

pub fn config(storage: &Path, capture: Arc<FakeCapture>, clock: Clock) -> Config {
    let mut config = Config::new(storage, capture);
    config.clock = clock;
    config.reconnect_delay = Duration::from_millis(300);
    config
}

/// The ManualRecordingChecks frames: Alpha joins as object 10 on a random dungeon floor, and hits from object 10.
pub struct Game {
    pub character: Guid,
    sequence: u32,
}

impl Default for Game {
    fn default() -> Self {
        Self { character: Guid([1, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 1]), sequence: 0 }
    }
}

impl Game {
    pub fn frame(&mut self, message: &WireMessage, at: i64) -> CaptureFrame {
        self.sequence += 1;
        message_frame(message, at, self.sequence)
    }
    pub fn join(&mut self, at: i64) -> CaptureFrame {
        let message = msg(3, vec![(253, Value::Byte(2)), (0, 10.into()), (1, self.character.into()), (2, "Alpha".into()), (8, "@RANDOMDUNGEON@one".into()), (66, "1315".into())]);
        self.frame(&message, at)
    }
    /// A hit (negative) or heal (positive) by object 10 on object 30.
    pub fn hit(&mut self, delta: f32, at: i64) -> CaptureFrame {
        let message = msg(4, vec![(252, Value::Byte(6)), (0, 30.into()), (6, 10.into()), (2, delta.into()), (7, (-1).into()), (5, Value::Byte(0))]);
        self.frame(&message, at)
    }
}

pub fn party_list(ids: &[Guid], names: &[&str], party: i32) -> WireMessage<'static> {
    msg(4, vec![(252, Value::Short(230)), (0, party.into()), (8, guid_bytes(ids)), (9, strings(names))])
}

pub fn send(source: &Sender<Step>, frame: CaptureFrame) {
    source.send(Step::Frame(frame)).unwrap();
}
