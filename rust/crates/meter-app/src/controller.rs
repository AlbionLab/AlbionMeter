//! The UI thread's side: turns feed events into the Slint globals (models updated row by row) and the user's actions
//! into commands. It is the port of the state and handlers of web/src/main.tsx, CapturePanel.tsx, CaptureNotice.tsx,
//! Library.tsx (GameLibrary), CopyDamage.tsx and DesktopChrome.tsx.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::desktop;
use crate::feed::*;
use crate::format::{compact, duration, percent, precise};
use crate::i18n::{Language, message, t, tf};
use crate::journal::Journal;
use crate::merge::Record;
use crate::ranking::{Metric, contributions, player_names, ranking_text};
use crate::ranking_image;
use crate::{AppWindow, JournalState, BarRow, CaptureState, CatalogRow, CatalogState, DiagRow, LabState, MeterState, SelectOption, Shell, T};

/// The second click of a double click on a record control is dropped (the browser's `event.detail > 1`).
const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// The browser's click count: a click within `DOUBLE_CLICK` of the previous one continues a double click, and only the
/// first click of a sequence counts (`event.detail <= 1`), whichever control it lands on.
#[derive(Default)]
pub struct ClickGuard {
    last: Option<Instant>,
}

impl ClickGuard {
    pub fn first(&mut self, now: Instant) -> bool {
        let second = self.last.is_some_and(|last| now.duration_since(last) < DOUBLE_CLICK);
        self.last = Some(now);
        !second
    }
}

/// The compositor keeps the button release of an interactive move or resize, so Slint would go on treating the bar (or
/// edge) as pressed and send every later press to it: the pointer is let go here.
fn release_grab(app: &AppWindow) {
    // After the press is handled (Slint takes the grab when the handler returns), and outside the window so nothing
    // under the pointer takes the release for a click.
    let weak = app.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(app) = weak.upgrade() {
            let position = slint::LogicalPosition::new(-1.0, -1.0);
            app.window().dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button: slint::platform::PointerEventButton::Left });
            app.window().dispatch_event(slint::platform::WindowEvent::PointerExited);
        }
    });
}

/// Rewrites `model` to `rows`, touching only the rows that changed.
pub fn sync<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: impl IntoIterator<Item = T>) {
    let mut count = 0;
    for (index, row) in rows.into_iter().enumerate() {
        if index < model.row_count() {
            if model.row_data(index).as_ref() != Some(&row) {
                model.set_row_data(index, row);
            }
        } else {
            model.push(row);
        }
        count = index + 1;
    }
    while model.row_count() > count {
        model.remove(model.row_count() - 1);
    }
}

pub fn color(hex: &str) -> slint::Color {
    let value = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0x9ebbf3);
    slint::Color::from_rgb_u8((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

pub fn model<T: Clone + 'static>(model: &Rc<VecModel<T>>) -> ModelRc<T> {
    ModelRc::from(model.clone())
}

fn megabytes(value: u64) -> String {
    format!("{:.1} MB", value as f64 / 1_048_576.0)
}

/// "2026-11-20T…" as the language writes a date (toLocaleDateString).
fn date(language: Language, iso: &str) -> String {
    let mut parts = iso.get(..10).unwrap_or_default().split('-').map(|part| part.parse::<u32>().unwrap_or(0));
    let (year, month, day) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    match language {
        Language::En => format!("{month}/{day}/{year}"),
        Language::Pt => format!("{day:02}/{month:02}/{year}"),
    }
}

thread_local! {
    static CONTROLLER: RefCell<Option<Rc<RefCell<Controller>>>> = const { RefCell::new(None) };
}

/// Makes `controller` the one feed events go to (on the UI thread).
pub fn install(controller: Rc<RefCell<Controller>>) {
    CONTROLLER.with(|slot| *slot.borrow_mut() = Some(controller));
}

/// Lets the controller go (closing): its command sender with it, which ends the feed.
pub fn uninstall() {
    CONTROLLER.with(|slot| slot.borrow_mut().take());
}

/// Hands a feed event to the controller; called on the UI thread through `slint::invoke_from_event_loop`.
pub fn dispatch(event: FeedEvent) {
    if let Some(controller) = CONTROLLER.with(|slot| slot.borrow().clone()) {
        controller.borrow_mut().handle(event);
    }
}

type Action = Box<dyn Fn(&mut Controller)>;
type TextAction = Box<dyn Fn(&mut Controller, SharedString)>;
type FlagAction = Box<dyn Fn(&mut Controller, bool)>;

#[derive(Clone, Copy)]
pub enum CopyTarget {
    Meter,
    Journal,
}

/// Runs `action` on the installed controller (UI thread).
pub fn with_controller(action: impl FnOnce(&mut Controller)) {
    if let Some(controller) = CONTROLLER.with(|slot| slot.borrow().clone()) {
        action(&mut controller.borrow_mut());
    }
}

pub struct Controller {
    app: slint::Weak<AppWindow>,
    commands: Sender<Command>,
    /// Shared with T.lookup, which runs while bindings evaluate and must not borrow the controller.
    language: Rc<Cell<Language>>,
    metric: Metric,
    record: Record,
    connection: Connection,
    busy: bool,
    error: Option<String>,
    clicks: ClickGuard,
    bar_clicks: ClickGuard,
    bars: Rc<VecModel<BarRow>>,
    icons: HashMap<Arc<str>, Option<slint::Image>>,
    /// Icons asked for and not delivered yet (each is asked for once; the cache retries failed downloads itself).
    requested: RefCell<std::collections::HashSet<Arc<str>>>,
    copy_reset: slint::Timer,
    copy_busy: bool,
    /// Owns the clipboard while an image copied there is offered (X11 and Wayland need a living owner).
    image_clipboard: Option<arboard::Clipboard>,
    icon_paths: HashMap<Arc<str>, std::path::PathBuf>,
    capture: Option<Arc<CaptureStatus>>,
    devices: Option<Arc<DeviceList>>,
    method: String,
    device: String,
    capture_busy: bool,
    capture_error: String,
    granting: bool,
    methods: Rc<VecModel<SelectOption>>,
    device_options: Rc<VecModel<SelectOption>>,
    diagnostics: Rc<VecModel<DiagRow>>,
    lab: Option<Arc<LabStatus>>,
    lab_error: String,
    lab_connecting: bool,
    lab_copied: slint::Timer,
    catalog: Option<Arc<Catalog>>,
    catalog_message: String,
    catalog_busy: bool,
    catalog_rows: Rc<VecModel<CatalogRow>>,
    storage: String,
    update: UpdateState,
    /// Whether the language and the window placement are saved (not while taking screenshots).
    pub persist: bool,
    /// Every window action asked for (drag, resize-…, minimize, …), for the UI tests.
    #[cfg(test)]
    pub window_actions: Vec<String>,
    pub journal: Journal,
}

impl Controller {
    pub fn new(app: &AppWindow, commands: Sender<Command>, language: Language) -> Rc<RefCell<Self>> {
        let journal = Journal::new(app, commands.clone(), language);
        let this = Rc::new(RefCell::new(Self {
            app: app.as_weak(),
            commands,
            language: Rc::new(Cell::new(language)),
            metric: Metric::Damage,
            record: Record::default(),
            connection: Connection::Connecting,
            busy: false,
            error: None,
            clicks: ClickGuard::default(),
            bar_clicks: ClickGuard::default(),
            bars: Rc::new(VecModel::default()),
            icons: HashMap::new(),
            requested: RefCell::default(),
            copy_reset: slint::Timer::default(),
            copy_busy: false,
            image_clipboard: None,
            icon_paths: HashMap::new(),
            capture: None,
            devices: None,
            method: "socket".into(),
            device: String::new(),
            capture_busy: false,
            capture_error: String::new(),
            granting: false,
            methods: Rc::new(VecModel::default()),
            device_options: Rc::new(VecModel::default()),
            diagnostics: Rc::new(VecModel::default()),
            lab: None,
            lab_error: String::new(),
            lab_connecting: false,
            lab_copied: slint::Timer::default(),
            catalog: None,
            catalog_message: String::new(),
            catalog_busy: false,
            catalog_rows: Rc::new(VecModel::default()),
            storage: String::new(),
            update: UpdateState::Idle,
            persist: true,
            #[cfg(test)]
            window_actions: Vec::new(),
            journal,
        }));
        Self::bind(&this, app);
        this.borrow().render_all();
        this
    }

    fn language(&self) -> Language {
        self.language.get()
    }

    fn t<'a>(&self, text: &'a str) -> &'a str {
        t(self.language(), text)
    }

    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    fn with_app(&self, apply: impl FnOnce(&AppWindow)) {
        if let Some(app) = self.app.upgrade() {
            apply(&app);
        }
    }

    fn bind(this: &Rc<RefCell<Self>>, app: &AppWindow) {
        let weak = Rc::downgrade(this);
        let on = move |action: Action| {
            let weak = weak.clone();
            move || {
                if let Some(this) = weak.upgrade() {
                    action(&mut this.borrow_mut());
                }
            }
        };
        let weak = Rc::downgrade(this);
        let with = move |action: TextAction| {
            let weak = weak.clone();
            move |value: SharedString| {
                if let Some(this) = weak.upgrade() {
                    action(&mut this.borrow_mut(), value);
                }
            }
        };
        let weak = Rc::downgrade(this);
        let flag = move |action: FlagAction| {
            let weak = weak.clone();
            move |value: bool| {
                if let Some(this) = weak.upgrade() {
                    action(&mut this.borrow_mut(), value);
                }
            }
        };

        let language = this.borrow().language.clone();
        let tr = app.global::<T>();
        tr.on_lookup(move |text| t(language.get(), &text).into());
        tr.on_fill(|text, name, value| text.replace(&format!("{{{name}}}"), &value).into());
        tr.on_capitalize(|text| crate::i18n::capitalize(&text).into());
        tr.set_pt(this.borrow().language() == Language::Pt);

        let shell = app.global::<Shell>();
        shell.on_set_language(flag(Box::new(|this, pt| this.set_language(if pt { Language::Pt } else { Language::En }))));
        shell.on_window_action(with(Box::new(|this, action| this.window_action(&action))));
        shell.on_dismiss_error(on(Box::new(|this| {
            this.error = None;
            this.render_shell();
        })));
        // "Restart and update": Velopack applies the update once the meter has closed and saved the record; the fake feed
        // (no installed meter) gets the command instead.
        shell.on_install_update(on(Box::new(|this| {
            if let UpdateState::Ready { version } = &this.update
                && crate::updater::restart_into_update()
            {
                this.update = UpdateState::Installing { version: version.clone() };
                this.render_update();
            } else {
                this.send(Command::InstallUpdate);
            }
        })));
        shell.on_open_url(|url| desktop::open_url(&url));

        let meter = app.global::<MeterState>();
        meter.on_control(with(Box::new(|this, action| this.control(&action))));
        meter.on_confirmed(with(Box::new(|this, kind| this.confirmed(&kind))));
        meter.on_metric_changed(with(Box::new(|this, metric| {
            this.metric = if metric == "healing" { Metric::Healing } else { Metric::Damage };
            this.render_meter();
        })));
        meter.on_copy(with(Box::new(|this, format| this.copy(&format))));

        let lab = app.global::<LabState>();
        // A double click would ask for two pairing codes and open the approval page twice.
        lab.on_connect(on(Box::new(|this| {
            if this.lab_connecting {
                return;
            }
            this.lab_connecting = true;
            this.lab_error.clear();
            this.send(Command::LabConnect);
        })));
        lab.on_copy_link(on(Box::new(|this| this.copy_link())));
        lab.on_save_title(with(Box::new(|this, title| this.send(Command::LabTitle(title.trim().to_owned())))));
        lab.on_set_visibility(with(Box::new(|this, value| this.send(Command::LabVisibility(value.into())))));
        lab.on_set_anonymize(flag(Box::new(|this, value| this.send(Command::LabAnonymize(value)))));
        lab.on_set_share_location(flag(Box::new(|this, value| this.send(Command::LabShareLocation(value)))));
        lab.on_disconnect(on(Box::new(|this| this.send(Command::LabDisconnect))));

        let capture = app.global::<CaptureState>();
        capture.set_methods(model(&this.borrow().methods));
        capture.set_devices(model(&this.borrow().device_options));
        capture.set_diagnostics(model(&this.borrow().diagnostics));
        capture.on_method_changed(with(Box::new(|this, method| {
            this.method = method.into();
            this.device = this.devices.as_ref().and_then(|list| list.devices.iter().find(|item| *item.method == this.method)).map(|item| item.name.to_string()).unwrap_or_default();
            this.render_capture();
        })));
        capture.on_device_changed(with(Box::new(|this, device| {
            this.device = device.into();
            this.render_capture();
        })));
        capture.on_refresh(on(Box::new(|this| this.send(Command::RefreshDevices))));
        capture.on_start(on(Box::new(|this| {
            this.capture_busy = true;
            this.capture_error.clear();
            this.send(Command::StartCapture { device: this.device.clone(), method: this.method.clone() });
            this.render_capture();
        })));
        capture.on_stop(on(Box::new(|this| {
            this.capture_busy = true;
            this.capture_error.clear();
            this.send(Command::StopCapture);
            this.render_capture();
        })));
        capture.on_replay(on(Box::new(|this| {
            let Some(file) = desktop::pick(false, "Replay capture", &["*.pcap", "*.pcapng", "*.cap"]) else { return };
            if std::fs::metadata(&file).is_ok_and(|meta| meta.len() > 40_000_000) {
                this.capture_error = this.t("Replay files are limited to 40 MB.").into();
            } else {
                this.capture_busy = true;
                this.capture_error.clear();
                this.send(Command::Replay(file));
            }
            this.render_capture();
        })));
        capture.on_grant(on(Box::new(|this| {
            this.granting = true;
            this.capture_error.clear();
            this.render_capture();
            // pkexec blocks until the password prompt is answered; on success the meter starts again with the capability.
            if !desktop::grant_capture_permission() {
                this.capture_error = this.t("Packet capture permission was not granted.").into();
            }
            this.granting = false;
            this.render_capture();
        })));
        capture.on_copy_setcap(on(Box::new(|this| {
            let command = this.setcap();
            this.clipboard(&command);
        })));

        let catalog = app.global::<CatalogState>();
        catalog.set_rows(model(&this.borrow().catalog_rows));
        catalog.on_tab_changed(with(Box::new(|this, tab| {
            this.with_app(|app| app.global::<CatalogState>().set_tab(tab));
            this.render_catalog();
        })));
        catalog.on_search_changed(with(Box::new(|this, search| {
            this.with_app(|app| app.global::<CatalogState>().set_search(search));
            this.render_catalog();
        })));
        catalog.on_import(on(Box::new(|this| {
            let Some(folder) = desktop::pick(true, "Import game folder", &[]) else { return };
            this.catalog_busy = true;
            this.catalog_message = this.t("Reading selected game files…").into();
            this.send(Command::ImportCatalog(folder));
            this.render_catalog();
        })));

        app.global::<MeterState>().set_bars(model(&this.borrow().bars));
        Journal::bind(this, app);
    }

    pub fn handle(&mut self, event: FeedEvent) {
        match event {
            FeedEvent::Connection(connection) => {
                self.connection = connection;
                if connection == Connection::Connected {
                    self.error = None;
                }
                self.render_shell();
                self.render_meter();
            }
            FeedEvent::Version(version) => self.with_app(|app| app.global::<Shell>().set_version(version.into())),
            FeedEvent::Snapshot(snapshot) => {
                if self.record.merge(snapshot) {
                    self.render_meter();
                    self.journal.live(&self.record);
                }
            }
            FeedEvent::CommandDone(error) => {
                self.busy = false;
                self.error = error.map(|error| message(self.language(), &error));
                self.render_shell();
                self.render_meter();
            }
            FeedEvent::Capture(status) => {
                self.capture = Some(status);
                self.render_capture();
                self.render_notice();
            }
            FeedEvent::CaptureDone(error) => {
                self.capture_busy = false;
                self.capture_error = error.map(|error| message(self.language(), &error)).unwrap_or_default();
                self.render_capture();
            }
            FeedEvent::Devices(list) => {
                let selected = list
                    .devices
                    .iter()
                    .find(|item| *item.name == self.device)
                    .or_else(|| list.devices.iter().find(|item| Some(&item.name) == list.selected_device.as_ref()))
                    .or(list.devices.first());
                if let Some(selected) = selected {
                    self.method = selected.method.to_string();
                }
                self.device = selected.map(|item| item.name.to_string()).unwrap_or_default();
                self.devices = Some(list);
                self.render_capture();
            }
            FeedEvent::Lab(status) => {
                // Connect answers with a pairing code: its approval page opens, as window.open did.
                if std::mem::take(&mut self.lab_connecting)
                    && let Some((_, url)) = &status.pairing
                {
                    desktop::open_url(url);
                }
                self.lab = Some(status);
                self.render_lab();
            }
            FeedEvent::LabError(error) => {
                self.lab_connecting = false;
                self.lab_error = message(self.language(), &error);
                self.render_lab();
            }
            FeedEvent::Catalog(catalog) => {
                self.catalog = Some(catalog);
                self.render_catalog();
            }
            FeedEvent::CatalogMessage(text) => {
                self.catalog_busy = false;
                self.catalog_message = match text.strip_prefix("imported:").and_then(|counts| counts.split_once(':')) {
                    Some((maps, weapons)) => tf(self.language(), "Imported {maps} maps and {weapons} weapon definitions.", &[("maps", maps), ("weapons", weapons)]),
                    None => message(self.language(), &text),
                };
                self.render_catalog();
            }
            FeedEvent::StoragePath(path) => {
                self.storage = path;
                self.render_catalog();
            }
            FeedEvent::Update(state) => {
                self.update = state;
                self.render_update();
            }
            FeedEvent::Dungeons(runs) => self.journal.runs(runs),
            FeedEvent::Dungeon(run) => self.journal.run(run),
            FeedEvent::HistoryMessage { text, error } => self.journal.history_message(text, error),
            FeedEvent::Icon(id, path) => {
                self.icon_paths.insert(id.clone(), path.clone());
                let image = slint::Image::load_from_path(&path).ok();
                if let Some(image) = &image {
                    self.journal.add_icon(id.clone(), image.clone());
                }
                self.icons.insert(id, image);
                self.render_meter();
            }
        }
    }

    pub fn render_all(&self) {
        self.render_shell();
        self.render_meter();
        self.render_notice();
        self.render_lab();
        self.render_capture();
        self.render_catalog();
        self.render_update();
    }

    fn status(&self) -> RecordStatus {
        match &self.record.snapshot {
            Some(snapshot) if &*snapshot.mode == "live" => snapshot.status,
            _ => RecordStatus::Idle,
        }
    }

    fn render_shell(&self) {
        let connection = match self.connection {
            Connection::Connecting => "connecting",
            Connection::Connected => "connected",
            Connection::Reconnecting => "reconnecting",
            Connection::Disconnected => "disconnected",
        };
        self.with_app(|app| {
            let shell = app.global::<Shell>();
            shell.set_connection(connection.into());
            shell.set_record_status(self.status().as_str().into());
            shell.set_has_snapshot(self.record.snapshot.is_some());
            shell.set_error(self.error.clone().unwrap_or_default().into());
            shell.set_maximized(app.window().is_maximized());
        });
    }

    fn render_meter(&self) {
        let language = self.language();
        let metric = self.metric;
        let snapshot = self.record.snapshot.as_deref();
        let live = snapshot.is_some_and(|snapshot| &*snapshot.mode == "live");
        let elapsed = snapshot.filter(|_| live).map_or(0.0, |snapshot| snapshot.elapsed_seconds);
        let total = snapshot.map_or(0.0, |snapshot| match metric {
            Metric::Damage => snapshot.total_damage,
            Metric::Healing => snapshot.total_healing,
        });
        let mut players: Vec<&Player> =
            snapshot.map(|snapshot| snapshot.members.iter().filter(|player| player.membership != Membership::Unconfirmed || player.damage + player.healing > 0.0).collect()).unwrap_or_default();
        players.sort_by(|a, b| metric.of(b).total_cmp(&metric.of(a)));
        let top = players.first().map_or(0.0, |player| metric.of(player));
        let rows = players.iter().enumerate().map(|(index, player)| {
            let value = metric.of(player);
            let share = match metric {
                Metric::Damage => player.damage_share,
                Metric::Healing => player.healing_share,
            };
            let icon = player.weapon_id.as_ref().and_then(|id| self.icons.get(id)).cloned().flatten();
            BarRow {
                id: player.id.as_ref().into(),
                rank: index as i32 + 1,
                name: player.name.as_ref().into(),
                weapon: player.weapon.as_ref().into(),
                has_icon: icon.is_some(),
                icon: icon.unwrap_or_default(),
                color: color(&player.color),
                fraction: if top > 0.0 { (value / top) as f32 } else { 0.0 },
                value: compact(language, value).into(),
                share: percent(language, share).into(),
                rate: format!("{} {}", precise(language, metric.rate(player)), metric.unit()).into(),
            }
        });
        sync(&self.bars, rows);
        let rate = if live && elapsed > 0.0 { total / elapsed } else { 0.0 };
        let note = tf(language, "{total} total · {rate} party {unit}", &[("total", &compact(language, total)), ("rate", &precise(language, rate)), ("unit", metric.unit())]);
        // Icons the cache has not delivered yet are asked for once.
        for id in players.iter().filter_map(|player| player.weapon_id.as_ref()) {
            if !self.icons.contains_key(id) && self.requested.borrow_mut().insert(id.clone()) {
                self.send(Command::Icon(id.clone()));
            }
        }
        self.with_app(|app| {
            let meter = app.global::<MeterState>();
            meter.set_clock(duration(elapsed).into());
            meter.set_metric(metric.name().into());
            meter.set_total_note(note.into());
            meter.set_can_control(self.connection == Connection::Connected && !self.busy);
            meter.set_copy_available(snapshot.is_some_and(|snapshot| !contributions(snapshot, metric).is_empty()));
            app.global::<Shell>().set_record_status(self.status().as_str().into());
        });
    }

    fn render_notice(&self) {
        let language = self.language();
        let (failed, reason, pending) = match self.capture.as_deref().filter(|status| &*status.source != "replay") {
            Some(status) => {
                let pending = status.report.as_ref().map_or(0, |report| report.pending_unidentified);
                let recording = matches!(&*status.state, "starting" | "running");
                let error = status.error.as_deref().unwrap_or_default().to_lowercase();
                let english = if error.contains("permission") || error.contains("cap_net_raw") || error.contains("npcap") {
                    Some("packet capture permission is missing")
                } else if let Some(reason) = &status.stop_reason {
                    match &**reason {
                        "storageLimit" => Some("the capture storage limit was reached"),
                        "sourceEnded" => Some("the network interface stopped delivering packets"),
                        "error" => Some("an error occurred"),
                        _ => None,
                    }
                } else if &*status.source == "none" {
                    Some("it was not started")
                } else {
                    None
                };
                (&*status.state == "failed", english.map(|text| t(language, text)).unwrap_or_default(), if recording && pending >= 30 { precise(language, pending as f64) } else { String::new() })
            }
            None => (false, "", String::new()),
        };
        self.with_app(|app| {
            let meter = app.global::<MeterState>();
            meter.set_notice_failed(failed);
            meter.set_notice_reason(reason.into());
            meter.set_notice_pending(pending.into());
        });
    }

    fn control(&mut self, action: &str) {
        if !self.clicks.first(Instant::now()) || self.connection != Connection::Connected || self.busy {
            return;
        }
        let command = match action {
            "play" => Command::StartRecord,
            "pause" => Command::PauseRecord,
            "resume" => Command::ResumeRecord,
            "reset" | "end" => {
                self.with_app(|app| app.global::<MeterState>().set_confirm(action.into()));
                return;
            }
            _ => return,
        };
        self.run_record(command);
    }

    fn run_record(&mut self, command: Command) {
        self.busy = true;
        self.error = None;
        self.send(command);
        self.render_shell();
        self.render_meter();
    }

    fn confirmed(&mut self, kind: &str) {
        self.with_app(|app| app.global::<MeterState>().set_confirm("".into()));
        match kind {
            "reset" => self.run_record(Command::NewRecord),
            "end" => self.run_record(Command::StopRecord),
            "disconnect" => self.send(Command::LabDisconnect),
            _ => {}
        }
    }

    pub fn clipboard(&self, text: &str) {
        self.with_app(|app| {
            app.set_clipboard(text.into());
            app.invoke_copy_clipboard();
        });
    }

    fn set_copy(&self, target: CopyTarget, done: &str, error: &str, busy: bool) {
        self.with_app(|app| match target {
            CopyTarget::Meter => {
                let meter = app.global::<MeterState>();
                meter.set_copy_done(done.into());
                meter.set_copy_error(error.into());
                meter.set_copy_busy(busy);
            }
            CopyTarget::Journal => {
                let journal = app.global::<JournalState>();
                journal.set_copy_done(done.into());
                journal.set_copy_error(error.into());
                journal.set_copy_busy(busy);
            }
        });
    }

    /// The confirmation (or the error) under the Copy button, cleared after 1.8 s (4 s for an error).
    fn flash_copy(&mut self, target: CopyTarget, done: &str, error: &str) {
        self.set_copy(target, done, error, false);
        let weak = self.app.clone();
        let wait = if error.is_empty() { Duration::from_millis(1800) } else { Duration::from_secs(4) };
        self.copy_reset.start(slint::TimerMode::SingleShot, wait, move || {
            if let Some(app) = weak.upgrade() {
                let meter = app.global::<MeterState>();
                meter.set_copy_done("".into());
                meter.set_copy_error("".into());
                let journal = app.global::<JournalState>();
                journal.set_copy_done("".into());
                journal.set_copy_error("".into());
            }
        });
    }

    fn copy(&mut self, format: &str) {
        let Some(snapshot) = self.record.snapshot.clone() else { return };
        self.copy_ranking(snapshot, self.metric, format, CopyTarget::Meter);
    }

    /// CopyDamage.tsx copy(): the ranking as text, the names, or the picture (rendered off the UI thread).
    pub fn copy_ranking(&mut self, snapshot: Arc<Snapshot>, metric: Metric, format: &str, target: CopyTarget) {
        if self.copy_busy || contributions(&snapshot, metric).is_empty() {
            return;
        }
        match format {
            "text" => {
                self.clipboard(&ranking_text(&snapshot, metric));
                self.flash_copy(target, "text", "");
            }
            "names" => {
                self.clipboard(&player_names(&snapshot, metric));
                self.flash_copy(target, "names", "");
            }
            _ => {
                let card = ranking_image::card(&snapshot, metric, self.language(), &self.icon_paths, desktop::fonts());
                self.copy_busy = true;
                self.set_copy(target, "", "", true);
                std::thread::spawn(move || {
                    let rendered = ranking_image::render_in_child(&card);
                    let _ = slint::invoke_from_event_loop(move || {
                        with_controller(|this| this.image_rendered(rendered, target));
                    });
                });
            }
        }
    }

    pub fn image_rendered(&mut self, rendered: Result<(u32, u32, Vec<u8>), String>, target: CopyTarget) {
        self.copy_busy = false;
        let result = rendered.map_err(|_| self.t("Image rendering failed.").to_owned()).and_then(|(width, height, rgba)| {
            let clipboard = match &mut self.image_clipboard {
                Some(clipboard) => clipboard,
                None => self.image_clipboard.insert(arboard::Clipboard::new().map_err(|_| self.t("Image clipboard is unavailable in this browser. Use Copy text.").to_owned())?),
            };
            clipboard
                .set_image(arboard::ImageData { width: width as usize, height: height as usize, bytes: rgba.into() })
                .map_err(|_| t(self.language(), "Could not copy. Try again.").to_owned())
        });
        match result {
            Ok(()) => self.flash_copy(target, "image", ""),
            Err(error) => self.flash_copy(target, "", &error),
        }
    }

    fn copy_link(&mut self) {
        let Some(url) = self.lab.as_ref().and_then(|lab| lab.session.as_ref()).map(|session| session.url.to_string()) else { return };
        self.clipboard(&url);
        self.with_app(|app| app.global::<LabState>().set_copied(true));
        let weak = self.app.clone();
        self.lab_copied.start(slint::TimerMode::SingleShot, Duration::from_millis(1500), move || {
            if let Some(app) = weak.upgrade() {
                app.global::<LabState>().set_copied(false);
            }
        });
    }

    fn render_lab(&self) {
        let language = self.language();
        self.with_app(|app| {
            let lab = app.global::<LabState>();
            let Some(status) = &self.lab else {
                lab.set_loaded(false);
                return;
            };
            let session = status.session.as_ref().filter(|session| !session.id.is_empty());
            lab.set_loaded(true);
            lab.set_connected(status.connected);
            lab.set_server(status.server.as_ref().into());
            lab.set_has_account(status.account.is_some());
            lab.set_account_name(status.account.as_ref().map(|account| account.0.as_str()).unwrap_or_default().into());
            lab.set_has_avatar(false);
            lab.set_pairing(status.pairing.is_some());
            lab.set_pair_code(status.pairing.as_ref().map(|pairing| pairing.0.as_str()).unwrap_or_default().into());
            lab.set_verify_url(status.pairing.as_ref().map(|pairing| pairing.1.as_str()).unwrap_or_default().into());
            lab.set_pairing_error(status.pairing_error.as_deref().unwrap_or_default().into());
            lab.set_has_session(session.is_some());
            lab.set_session_id(session.map(|session| &*session.id).unwrap_or_default().into());
            lab.set_session_state(session.map(|session| &*session.state).unwrap_or_default().into());
            lab.set_session_url(session.map(|session| &*session.url).unwrap_or_default().into());
            let link = session.map(|session| session.url.trim_start_matches("https://").trim_start_matches("http://")).unwrap_or_default();
            lab.set_session_link(link.into());
            lab.set_session_title(session.and_then(|session| session.title.as_deref()).unwrap_or_default().into());
            lab.set_session_error(status.session.as_ref().and_then(|session| session.error.as_deref()).map(|error| message(language, error)).unwrap_or_default().into());
            lab.set_visibility(session.map_or(&*status.visibility, |session| &*session.visibility).into());
            lab.set_anonymize(session.map_or(status.anonymize, |session| session.anonymize));
            lab.set_share_location(status.share_location);
            lab.set_suggested_title(status.suggested_title.as_deref().unwrap_or_default().into());
            lab.set_error(self.lab_error.as_str().into());
        });
    }

    fn setcap(&self) -> String {
        let executable = std::env::current_exe().map(|path| path.display().to_string()).unwrap_or_else(|_| "./albion-meter".into());
        format!("sudo setcap cap_net_raw=ep {executable}")
    }

    fn render_capture(&self) {
        let language = self.language();
        let status = self.capture.as_deref();
        let devices = self.devices.as_deref();
        let active = status.is_some_and(|status| matches!(&*status.state, "starting" | "running" | "stopping"));
        let status_error = status.and_then(|status| status.error.as_deref()).unwrap_or_default();
        let devices_error = devices.and_then(|devices| devices.error.as_deref()).unwrap_or_default();
        let failure = format!("{} {status_error} {devices_error}", self.capture_error).to_lowercase();
        let permission = ["cap_net_raw", "permission", "npcap", "wpcap"].iter().any(|needle| failure.contains(needle));
        let hint = match (permission, cfg!(windows), cfg!(target_os = "linux")) {
            (false, _, _) => "",
            (true, true, _) => "windows",
            (true, false, true) => "grant",
            (true, false, false) => "setcap",
        };
        let methods = devices.map(|devices| devices.methods.as_slice()).unwrap_or_default();
        sync(
            &self.methods,
            methods.iter().map(|item| SelectOption {
                value: item.id.as_ref().into(),
                label: if item.available { item.name.as_ref().into() } else { tf(language, "{name} (unavailable)", &[("name", &item.name)]).into() },
                disabled: !item.available,
                badge: "".into(),
            }),
        );
        // The card with the default route carries the game traffic; every-interface capture also follows ExitLag, NoPing and VPN adapters.
        let main = t(language, "Main");
        let mut options: Vec<SelectOption> = devices
            .map(|devices| devices.devices.as_slice())
            .unwrap_or_default()
            .iter()
            .filter(|item| *item.method == self.method)
            .map(|item| SelectOption {
                value: item.name.as_ref().into(),
                label: if item.description.is_empty() { item.name.as_ref().into() } else { item.description.as_ref().into() },
                disabled: false,
                badge: if matches!(&*item.name, "raw:all" | "raw:any") {
                    "ExitLag / VPN".into()
                } else if item.flags & 1 != 0 {
                    main.into()
                } else {
                    "".into()
                },
            })
            .collect();
        options.sort_by_key(|option| option.badge != main);
        let device_index = options.iter().position(|option| option.value == self.device.as_str()).map_or(-1, |index| index as i32);
        sync(&self.device_options, options);
        let report = status.and_then(|status| status.report.as_ref());
        sync(&self.diagnostics, report.map(|report| report.counters.as_slice()).unwrap_or_default().iter().map(|(name, value)| DiagRow { name: name.as_ref().into(), value: precise(language, *value).into() }));
        let error = if !self.capture_error.is_empty() { self.capture_error.clone() } else { message(language, status_error) };
        self.with_app(|app| {
            let capture = app.global::<CaptureState>();
            capture.set_state(status.map_or("connecting", |status| &*status.state).into());
            capture.set_source(status.map_or("none", |status| &*status.source).into());
            capture.set_method(self.method.as_str().into());
            capture.set_method_index(methods.iter().position(|item| *item.id == self.method).map_or(-1, |index| index as i32));
            capture.set_device(self.device.as_str().into());
            capture.set_device_index(device_index);
            capture.set_busy(self.capture_busy);
            capture.set_active(active);
            capture.set_stopping(status.is_some_and(|status| &*status.state == "stopping"));
            capture.set_error(error.into());
            capture.set_devices_error(if devices_error.is_empty() { String::new() } else { message(language, devices_error) }.into());
            capture.set_hint(hint.into());
            capture.set_setcap(self.setcap().into());
            capture.set_granting(self.granting);
            capture.set_frames(precise(language, report.map_or(0.0, |report| report.frames as f64)).into());
            capture.set_transport(precise(language, report.map_or(0.0, |report| report.transport_messages as f64)).into());
            capture.set_traffic(report.map_or("—".into(), |report| megabytes(report.bytes)).into());
            capture.set_drops(status.and_then(|status| status.adapter_dropped).map_or("—".into(), |dropped| precise(language, dropped as f64)).into());
            capture.set_library(status.and_then(|status| status.library.clone()).or_else(|| devices.and_then(|devices| devices.library.clone())).unwrap_or_default().into());
            capture.set_stop_reason(status.and_then(|status| status.stop_reason.as_deref()).unwrap_or_default().into());
            capture.set_file(status.and_then(|status| status.file.as_deref()).unwrap_or_default().into());
            capture.set_debug_enabled(devices.is_some_and(|devices| devices.debug_enabled));
            capture.set_capture_library(if cfg!(windows) { "Npcap" } else { "libpcap" }.into());
        });
    }

    fn render_catalog(&self) {
        let language = self.language();
        let Some(app) = self.app.upgrade() else { return };
        let state = app.global::<CatalogState>();
        let weapons = state.get_tab() == "weapons";
        let query = state.get_search().to_lowercase();
        let entries = self.catalog.as_deref().map(|catalog| if weapons { &catalog.weapons } else { &catalog.maps });
        let matches: Vec<&CatalogEntry> = entries
            .map(|entries| entries.iter().filter(|item| format!("{} {} {}", item.id, item.name, item.group).to_lowercase().contains(&query)).collect())
            .unwrap_or_default();
        sync(
            &self.catalog_rows,
            matches.iter().take(100).map(|item| CatalogRow {
                name: item.name.as_ref().into(),
                group: item.group.as_ref().into(),
                tier: item.tier.map(|tier| format!("T{tier}")).unwrap_or_default().into(),
                id: item.id.as_ref().into(),
            }),
        );
        let mut results = tf(language, "{n} results", &[("n", &precise(language, matches.len() as f64))]);
        if matches.len() > 100 {
            results.push_str(t(language, " · Showing the first 100, refine your search for more"));
        }
        let catalog = self.catalog.as_deref();
        state.set_empty(matches.is_empty());
        state.set_results(results.into());
        state.set_weapon_count(catalog.map_or(0, |catalog| catalog.weapons.len()).to_string().into());
        state.set_map_count(catalog.map_or(0, |catalog| catalog.maps.len()).to_string().into());
        state.set_maps_kept(precise(language, catalog.map_or(0, |catalog| catalog.maps.len()) as f64).into());
        state.set_weapons_kept(precise(language, catalog.map_or(0, |catalog| catalog.weapons.len()) as f64).into());
        state.set_game_version(catalog.and_then(|catalog| catalog.game_version.clone()).unwrap_or_default().into());
        state.set_imported(catalog.and_then(|catalog| catalog.imported_at.as_deref()).map_or("—".into(), |iso| date(language, iso)).into());
        state.set_message(self.catalog_message.as_str().into());
        state.set_busy(self.catalog_busy);
        state.set_storage(if self.storage.is_empty() { t(language, "Storage information unavailable.") } else { &self.storage }.into());
    }

    fn render_update(&self) {
        let language = self.language();
        let (state, version, percent, progress) = match &self.update {
            UpdateState::Idle => ("idle", "", 0, String::new()),
            UpdateState::Checking => ("checking", "", 0, String::new()),
            UpdateState::Current => ("current", "", 0, String::new()),
            UpdateState::Downloading { version, percent, transferred, total } => (
                "downloading",
                version.as_str(),
                *percent,
                match total {
                    Some(total) if *total > 0 => tf(language, "{done} of {total} · ", &[("done", &megabytes(transferred.unwrap_or(0))), ("total", &megabytes(*total))]),
                    _ => String::new(),
                },
            ),
            UpdateState::Ready { version } => ("ready", version.as_str(), 100, String::new()),
            UpdateState::Installing { version } => ("installing", version.as_str(), 100, String::new()),
            UpdateState::Error(_) => ("error", "", 0, String::new()),
        };
        self.with_app(|app| {
            let shell = app.global::<Shell>();
            shell.set_update_state(state.into());
            shell.set_update_version(version.into());
            shell.set_update_percent(percent as i32);
            shell.set_update_progress(progress.into());
        });
    }

    fn set_language(&mut self, language: Language) {
        if language == self.language() {
            return;
        }
        self.language.set(language);
        if self.persist {
            desktop::save_language(language);
        }
        self.with_app(|app| app.global::<T>().set_pt(language == Language::Pt));
        self.render_all();
        self.journal.set_language(language);
    }

    fn window_action(&mut self, action: &str) {
        #[cfg(test)]
        self.window_actions.push(action.to_owned());
        use slint::winit_030::WinitWindowAccessor;
        use slint::winit_030::winit::window::ResizeDirection;
        let Some(app) = self.app.upgrade() else { return };
        let window = app.window();
        match action {
            "minimize" => window.set_minimized(true),
            "maximize" => window.set_maximized(!window.is_maximized()),
            "close" => {
                self.save_placement();
                let _ = window.hide();
                let _ = slint::quit_event_loop();
            }
            // The compositor moves the window from the press on; a second press soon after (a double click, which the app
            // never sees as one) maximizes or restores instead.
            "drag" if !self.bar_clicks.first(Instant::now()) => window.set_maximized(!window.is_maximized()),
            "drag" => {
                window.with_winit_window(|winit| winit.drag_window());
                release_grab(&app);
            }
            _ => {
                let direction = match action.strip_prefix("resize-") {
                    Some("n") => ResizeDirection::North,
                    Some("s") => ResizeDirection::South,
                    Some("e") => ResizeDirection::East,
                    Some("w") => ResizeDirection::West,
                    Some("ne") => ResizeDirection::NorthEast,
                    Some("nw") => ResizeDirection::NorthWest,
                    Some("se") => ResizeDirection::SouthEast,
                    Some("sw") => ResizeDirection::SouthWest,
                    _ => return,
                };
                window.with_winit_window(|winit| winit.drag_resize_window(direction));
                release_grab(&app);
            }
        }
        app.global::<Shell>().set_maximized(window.is_maximized());
    }

    pub fn save_placement(&self) {
        if !self.persist {
            return;
        }
        self.with_app(|app| {
            let window = app.window();
            let size = window.size().to_logical(window.scale_factor());
            if !window.is_maximized() || desktop::placement().is_none() {
                desktop::save_placement(&desktop::Placement { width: size.width, height: size.height, maximized: window.is_maximized() });
            } else if let Some(mut saved) = desktop::placement() {
                saved.maximized = true;
                desktop::save_placement(&saved);
            }
        });
    }

    pub fn commands(&self) -> &Sender<Command> {
        &self.commands
    }

    /// A second start of the meter brings this one to the front.
    pub fn focus_window(&self) {
        use slint::winit_030::WinitWindowAccessor;
        self.with_app(|app| {
            app.window().set_minimized(false);
            // Wayland gives focus only with an activation token, which a second start does not have: the window then asks
            // for attention instead (urgent in the task bar / compositor).
            app.window().with_winit_window(|winit| {
                winit.focus_window();
                winit.request_user_attention(Some(slint::winit_030::winit::window::UserAttentionType::Informational));
            });
        });
    }

    /// The maximized state can change from the window manager (a double click, a keyboard shortcut).
    pub fn poll_window(&self) {
        self.with_app(|app| {
            let maximized = app.window().is_maximized();
            let shell = app.global::<Shell>();
            if shell.get_maximized() != maximized {
                shell.set_maximized(maximized);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_follow_the_language() {
        assert_eq!(date(Language::En, "2026-11-20T18:04:00Z"), "11/20/2026");
        assert_eq!(date(Language::Pt, "2026-11-20T18:04:00Z"), "20/11/2026");
    }

    #[test]
    fn the_second_click_of_a_double_click_is_ignored() {
        let mut guard = ClickGuard::default();
        let start = Instant::now();
        assert!(guard.first(start));
        assert!(!guard.first(start + Duration::from_millis(200)));
        // A third quick click still belongs to the same sequence.
        assert!(!guard.first(start + Duration::from_millis(400)));
        assert!(guard.first(start + Duration::from_millis(1000)));
    }

    #[test]
    fn colors_parse() {
        assert_eq!(color("#9ebbf3"), slint::Color::from_rgb_u8(0x9e, 0xbb, 0xf3));
    }
}
