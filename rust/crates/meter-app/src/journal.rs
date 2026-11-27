//! The dungeon journal: web/src/Library.tsx DungeonJournal, Timeline.tsx (+ timelineBuckets.ts, timelineNavigation.ts),
//! TimelineInspector.tsx, DungeonLoadout.tsx, sourceItems.ts and HistoryTransfer.tsx. English only, like the web UI.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::controller::{Controller, color, model, sync};
use crate::desktop;
use crate::feed::*;
use crate::format::{compact, duration, precise};
use crate::i18n::Language;
use crate::merge::Record;
use crate::ranking::Metric;
use crate::{AppWindow, AxisLabel, Cell, FloorRow, FloorTab, InspectorItem, InspectorPlayer, JournalState, LegendRow, LoadoutRow, ProgressionRow, RunRow, Series};

pub type ItemIds = Rc<dyn Fn(i32) -> Option<Arc<str>>>;

const PALETTE: [&str; 8] = ["#9ebbf3", "#eab978", "#eb95b7", "#67cbd7", "#b69aff", "#6dd1ad", "#f28b82", "#c5d87a"];
const MAX_FILLED_SERIES: usize = 4;
const RESOLUTIONS: [u32; 5] = [1, 5, 10, 30, 60];
const RANGES: [u32; 4] = [30, 120, 600, 0];
const SLOTS: [(&str, &str); 11] =
    [("mainhand", "Main hand"), ("offhand", "Off hand"), ("head", "Head"), ("armor", "Chest"), ("chest", "Chest"), ("shoes", "Shoes"), ("cape", "Cape"), ("bag", "Bag"), ("mount", "Mount"), ("food", "Food"), ("potion", "Potion")];

pub fn slot_name(slot: &str) -> String {
    let lower = slot.to_lowercase();
    SLOTS.iter().find(|(key, _)| *key == lower).map_or_else(|| slot.to_owned(), |(_, name)| (*name).to_owned())
}

fn slot_order(slot: &str) -> usize {
    SLOTS.iter().position(|(key, _)| *key == slot).unwrap_or(usize::MAX)
}

fn source_kind(kind: Option<&str>) -> &'static str {
    match kind {
        Some("autoAttack") => "Auto attack",
        Some("passive") => "Passive",
        Some("ability") => "Ability",
        _ => "Unknown",
    }
}

fn percent(value: f64, total: f64) -> String {
    format!("{:.1}%", if total > 0.0 { value / total * 100.0 } else { 0.0 })
}

fn rate(value: f64, seconds: f64) -> f64 {
    if seconds > 0.0 { value / seconds } else { 0.0 }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// new Date(iso).toLocaleString(): "11/26/2026, 8:00:00 PM" in English, "26/11/2026, 20:00:00" in Portuguese (UTC).
fn when(language: Language, iso: &str) -> String {
    let number = |range: std::ops::Range<usize>| iso.get(range).and_then(|part| part.parse::<u32>().ok());
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (number(0..4), number(5..7), number(8..10), number(11..13), number(14..16), number(17..19)) else {
        return iso.to_owned();
    };
    match language {
        Language::En => format!("{month}/{day}/{year}, {}:{minute:02}:{second:02} {}", if hour % 12 == 0 { 12 } else { hour % 12 }, if hour < 12 { "AM" } else { "PM" }),
        Language::Pt => format!("{day:02}/{month:02}/{year}, {hour:02}:{minute:02}:{second:02}"),
    }
}

/// sourceItems.ts: the contributions of one item (per player, slot and item, or family when the tier was not observed).
#[derive(Clone, Debug)]
pub struct SourceItem {
    pub id: String,
    pub first: ItemContribution,
    pub damage: f64,
    pub healing: f64,
    pub contributions: Vec<ItemContribution>,
}

impl SourceItem {
    fn of(&self, metric: Metric) -> f64 {
        match metric {
            Metric::Damage => self.damage,
            Metric::Healing => self.healing,
        }
    }
}

pub fn source_items(contributions: &[ItemContribution]) -> Vec<SourceItem> {
    let mut items: Vec<SourceItem> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for contribution in contributions {
        let id = format!(
            "{}:{}:{}{}",
            contribution.player_id,
            contribution.slot,
            contribution.item_index.map_or_else(|| "unknown".to_owned(), |index| index.to_string()),
            if contribution.tier_observed { "" } else { ":family" }
        );
        let position = *index.entry(id.clone()).or_insert_with(|| {
            items.push(SourceItem { id, first: contribution.clone(), damage: 0.0, healing: 0.0, contributions: Vec::new() });
            items.len() - 1
        });
        let item = &mut items[position];
        item.damage += contribution.damage;
        item.healing += contribution.healing;
        item.contributions.push(contribution.clone());
    }
    items
}

fn contribution(item: &ItemContribution, metric: Metric) -> f64 {
    match metric {
        Metric::Damage => item.damage,
        Metric::Healing => item.healing,
    }
}

/// timelineBuckets.ts for one bucket: the points of [second, second + resolution) summed.
fn bucket(timeline: &[Point], second: u32, resolution: u32) -> Point {
    let from = timeline.partition_point(|point| point.second < second);
    let to = timeline.partition_point(|point| point.second < second + resolution);
    let mut sum = Point { second, ..Point::default() };
    for point in &timeline[from..to] {
        for (target, source) in [(&mut sum.damage, &point.damage), (&mut sum.healing, &point.healing), (&mut sum.item_damage, &point.item_damage), (&mut sum.item_healing, &point.item_healing)] {
            for (id, value) in source {
                *target.entry(id.clone()).or_default() += value;
            }
        }
    }
    sum
}

fn values(point: &Point, metric: Metric) -> &Values {
    match metric {
        Metric::Damage => &point.damage,
        Metric::Healing => &point.healing,
    }
}

fn item_values(point: &Point, metric: Metric) -> &Values {
    match metric {
        Metric::Damage => &point.item_damage,
        Metric::Healing => &point.item_healing,
    }
}

/// timelineNavigation.ts: the interval to pin and the scroll offset that shows it.
pub fn reveal(second: i64, resolution: u32, max_second: u32, window: u32, start: u32, end: u32) -> (u32, u32) {
    let resolution = resolution as i64;
    let second = (second.clamp(0, max_second as i64) / resolution * resolution) as u32;
    let limit = max_second.saturating_sub(window);
    let mut next = start as i64;
    if second < start {
        next = second as i64;
    } else if second as i64 + resolution > end as i64 {
        next = (max_second as i64).min(second as i64 + resolution) - window as i64;
    }
    let next = next.clamp(0, limit as i64) as u32;
    (second, limit - next)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Order {
    Damage,
    Healing,
    Name,
    Party,
}

struct Plot {
    start: u32,
    end: u32,
    window: u32,
    max_second: u32,
    latest: u32,
    left: f32,
    right: f32,
}

pub struct Journal {
    app: slint::Weak<AppWindow>,
    commands: Sender<Command>,
    language: Language,
    runs: Arc<Vec<DungeonSummary>>,
    run: Option<Arc<DungeonRun>>,
    selected: Option<Arc<str>>,
    floor: u32,
    metric: Metric,
    resolution: u32,
    range: u32,
    by_item: bool,
    size: (f32, f32),
    offset: u32,
    pinned: Option<u32>,
    record_scope: bool,
    order: Order,
    inspector_metric: Metric,
    search: String,
    open_players: HashSet<String>,
    loadout_open: Option<HashSet<String>>,
    progression_open: Option<HashSet<String>>,
    exporting: bool,
    link_reset: slint::Timer,
    icons: HashMap<Arc<str>, slint::Image>,
    visible: bool,
    stale: bool,
    requested: RefCell<HashSet<Arc<str>>>,
    /// The item id of an item index, for its icon (`ItemContribution` carries only the index).
    pub item_ids: ItemIds,
    run_rows: Rc<VecModel<RunRow>>,
    tabs: Rc<VecModel<FloorTab>>,
    y_labels: Rc<VecModel<AxisLabel>>,
    x_labels: Rc<VecModel<AxisLabel>>,
    grid: Rc<VecModel<f32>>,
    series: Rc<VecModel<Series>>,
    legend: Rc<VecModel<LegendRow>>,
    inspector: Rc<VecModel<InspectorPlayer>>,
    loadout: Rc<VecModel<LoadoutRow>>,
    floor_rows: Rc<VecModel<FloorRow>>,
    heads: Rc<VecModel<SharedString>>,
    progression: Rc<VecModel<ProgressionRow>>,
}

impl Journal {
    pub fn new(app: &AppWindow, commands: Sender<Command>, language: Language) -> Self {
        Self {
            app: app.as_weak(),
            commands,
            language,
            runs: Arc::default(),
            run: None,
            selected: None,
            floor: 0,
            metric: Metric::Damage,
            resolution: 1,
            range: 120,
            by_item: false,
            size: (800.0, 210.0),
            offset: 0,
            pinned: None,
            record_scope: false,
            order: Order::Damage,
            inspector_metric: Metric::Damage,
            search: String::new(),
            open_players: HashSet::new(),
            loadout_open: None,
            progression_open: None,
            exporting: false,
            link_reset: slint::Timer::default(),
            icons: HashMap::new(),
            visible: false,
            stale: false,
            requested: RefCell::default(),
            item_ids: Rc::new(|_| None),
            run_rows: Rc::default(),
            tabs: Rc::default(),
            y_labels: Rc::default(),
            x_labels: Rc::default(),
            grid: Rc::default(),
            series: Rc::default(),
            legend: Rc::default(),
            inspector: Rc::default(),
            loadout: Rc::default(),
            floor_rows: Rc::default(),
            heads: Rc::default(),
            progression: Rc::default(),
        }
    }

    pub fn bind(this: &Rc<RefCell<Controller>>, app: &AppWindow) {
        let state = app.global::<JournalState>();
        {
            let controller = this.borrow();
            let journal = &controller.journal;
            state.set_runs(model(&journal.run_rows));
            state.set_floors(model(&journal.tabs));
            state.set_y_labels(model(&journal.y_labels));
            state.set_x_labels(model(&journal.x_labels));
            state.set_grid(model(&journal.grid));
            state.set_series(model(&journal.series));
            state.set_legend(model(&journal.legend));
            state.set_inspector(model(&journal.inspector));
            state.set_loadout(model(&journal.loadout));
            state.set_floor_rows(model(&journal.floor_rows));
            state.set_progression_heads(ModelRc::from(journal.heads.clone()));
            state.set_progression(model(&journal.progression));
            state.set_list_note("No recorded dungeons yet. Entering a dungeon during capture starts a run.".into());
        }
        let weak = Rc::downgrade(this);
        let with = move |action: fn(&mut Journal, SharedString)| {
            let weak = weak.clone();
            move |value: SharedString| {
                if let Some(this) = weak.upgrade() {
                    action(&mut this.borrow_mut().journal, value);
                }
            }
        };
        let weak = Rc::downgrade(this);
        let on = move |action: fn(&mut Journal)| {
            let weak = weak.clone();
            move || {
                if let Some(this) = weak.upgrade() {
                    action(&mut this.borrow_mut().journal);
                }
            }
        };
        let weak = Rc::downgrade(this);
        state.on_select_floor(move |floor| {
            if let Some(this) = weak.upgrade() {
                let journal = &mut this.borrow_mut().journal;
                journal.floor = floor.max(0) as u32;
                journal.offset = 0;
                journal.pinned = None;
                journal.render();
            }
        });
        let weak = Rc::downgrade(this);
        state.on_plot_resized(move |width, height| {
            if let Some(this) = weak.upgrade() {
                let journal = &mut this.borrow_mut().journal;
                let size = (width, height.max(100.0));
                if size != journal.size {
                    journal.size = size;
                    journal.render_chart();
                }
            }
        });
        let weak = Rc::downgrade(this);
        state.on_pick(move |x| {
            if let Some(this) = weak.upgrade() {
                this.borrow_mut().journal.pick(x);
            }
        });
        let weak = Rc::downgrade(this);
        state.on_key(move |key, shift| {
            if let Some(this) = weak.upgrade() {
                this.borrow_mut().journal.key(&key, shift);
            }
        });
        let weak = Rc::downgrade(this);
        state.on_scrub(move |value| {
            if let Some(this) = weak.upgrade() {
                let journal = &mut this.borrow_mut().journal;
                if let Some(plot) = journal.plot() {
                    let limit = plot.max_second.saturating_sub(plot.window);
                    journal.offset = limit.saturating_sub(value.round().max(0.0) as u32);
                    journal.render_chart();
                }
            }
        });
        state.on_select_run(with(|journal, id| {
            journal.selected = Some(id.as_str().into());
            journal.floor = 0;
            journal.offset = 0;
            journal.pinned = None;
            let _ = journal.commands.send(Command::OpenDungeon(id.as_str().into()));
            journal.render_list();
        }));
        state.on_metric_changed(with(|journal, metric| {
            journal.metric = if metric == "healing" { Metric::Healing } else { Metric::Damage };
            journal.inspector_metric = journal.metric;
            journal.render();
        }));
        state.on_grouping_changed(with(|journal, grouping| {
            journal.by_item = grouping == "item";
            journal.render_chart();
        }));
        state.on_resolution_changed(with(|journal, value| {
            journal.resolution = value.parse().unwrap_or(1);
            journal.render_chart();
        }));
        state.on_range_changed(with(|journal, value| {
            journal.range = value.parse().unwrap_or(0);
            journal.offset = 0;
            journal.render_chart();
        }));
        state.on_follow_latest(on(|journal| {
            journal.offset = 0;
            journal.render_chart();
        }));
        state.on_scope_changed(with(|journal, scope| {
            journal.record_scope = scope == "record";
            journal.render_chart();
        }));
        state.on_order_changed(with(|journal, order| {
            journal.order = match order.as_str() {
                "healing" => Order::Healing,
                "name" => Order::Name,
                "party" => Order::Party,
                _ => Order::Damage,
            };
            journal.render_chart();
        }));
        state.on_inspector_metric_changed(with(|journal, metric| {
            journal.inspector_metric = if metric == "healing" { Metric::Healing } else { Metric::Damage };
            journal.render_chart();
        }));
        state.on_inspector_search_changed(with(|journal, search| {
            journal.search = search.to_lowercase();
            journal.render_chart();
        }));
        state.on_toggle_player(with(|journal, id| {
            if !journal.open_players.remove(id.as_str()) {
                journal.open_players.insert(id.into());
            }
            journal.render_chart();
        }));
        state.on_toggle_loadout(with(|journal, id| {
            let ids = journal.member_ids();
            toggle(&mut journal.loadout_open, &ids, &id);
            journal.render_tables();
        }));
        state.on_toggle_loadout_all(on(|journal| {
            let ids = journal.member_ids();
            toggle_all(&mut journal.loadout_open, &ids);
            journal.render_tables();
        }));
        state.on_toggle_progression(with(|journal, id| {
            let ids = journal.run_member_ids();
            toggle(&mut journal.progression_open, &ids, &id);
            journal.render_tables();
        }));
        state.on_toggle_progression_all(on(|journal| {
            let ids = journal.run_member_ids();
            toggle_all(&mut journal.progression_open, &ids);
            journal.render_tables();
        }));
        state.on_export_run(on(|journal| {
            let Some(run) = journal.run.clone() else { return };
            journal.export(Some(run.summary.id.clone()));
        }));
        state.on_export_history(on(|journal| journal.export(None)));
        state.on_import_history(on(|journal| {
            let Some(file) = desktop::pick(false, "Import history", &["*.zip", "*.json"]) else { return };
            if std::fs::metadata(&file).is_ok_and(|meta| meta.len() > 40_000_000) {
                journal.history_message("Choose a history file smaller than 40 MB.".into(), true);
                return;
            }
            journal.with(|state| state.set_importing(true));
            let _ = journal.commands.send(Command::ImportHistory(file));
        }));
        state.on_dismiss_transfer(on(|journal| journal.with(|state| state.set_transfer_message("".into()))));
        let weak = Rc::downgrade(this);
        state.on_copy(move |format| {
            let Some(this) = weak.upgrade() else { return };
            let mut controller = this.borrow_mut();
            let copied = controller.journal.totals().cloned().map(|totals| (totals, controller.journal.metric));
            if let Some((totals, metric)) = copied {
                controller.copy_ranking(totals, metric, &format, crate::controller::CopyTarget::Journal);
            }
        });
        let weak = Rc::downgrade(this);
        state.on_copy_link(move || {
            let Some(this) = weak.upgrade() else { return };
            let controller = this.borrow();
            let journal = &controller.journal;
            let Some(run) = &journal.run else { return };
            let floor = if journal.floor > 0 { format!("/floor/{}", journal.floor) } else { String::new() };
            if let Some(app) = journal.app.upgrade() {
                app.set_clipboard(format!("#/dungeons/{}{floor}", run.summary.id).into());
                app.invoke_copy_clipboard();
                app.global::<JournalState>().set_link_copied(true);
            }
            let weak = journal.app.clone();
            journal.link_reset.start(slint::TimerMode::SingleShot, Duration::from_millis(1500), move || {
                if let Some(app) = weak.upgrade() {
                    app.global::<JournalState>().set_link_copied(false);
                }
            });
        });
    }

    /// An icon from the icon cache, or "?" (`false`) while it is missing.
    fn icon(&self, id: Option<&Arc<str>>) -> (slint::Image, bool) {
        id.and_then(|id| self.icons.get(id)).map_or_else(|| (slint::Image::default(), false), |image| (image.clone(), true))
    }

    /// An item's icon: its id from the item index (the shipped combat.json, or the fixture's item table), then the cache.
    fn item_icon(&self, item: &SourceItem) -> (slint::Image, bool) {
        let id = item.first.item_index.and_then(|index| (self.item_ids)(index));
        if let Some(id) = &id
            && !self.icons.contains_key(id)
            && self.requested.borrow_mut().insert(id.clone())
        {
            let _ = self.commands.send(Command::Icon(id.clone()));
        }
        self.icon(id.as_ref())
    }

    /// A player's weapon by name (weapons.ts), with its tier when the catalog knows it.
    fn weapon(&self, player: &Player) -> (slint::Image, bool, String) {
        let (icon, has_icon) = self.icon(player.weapon_id.as_ref());
        let tier = player.weapon_id.as_ref().and_then(|_| player.weapon.rsplit(" · ").next()).filter(|tier| tier.starts_with('T')).unwrap_or_default();
        (icon, has_icon, tier.to_owned())
    }

    /// DungeonLoadout.tsx: the main-hand item seen in combat when the weapon was never observed, else the weapon.
    fn identity(&self, player: &Player, seen: Option<&(SourceItem, String)>) -> (slint::Image, bool, String) {
        match seen {
            Some((item, _)) => {
                let (icon, has_icon) = self.item_icon(item);
                (icon, has_icon, item_tier(item))
            }
            None => self.weapon(player),
        }
    }

    pub fn add_icon(&mut self, id: Arc<str>, image: slint::Image) {
        self.icons.insert(id, image);
        self.refresh();
    }

    /// Redraws the journal when it is on screen; otherwise it is redrawn when it is shown (dungeon updates arrive every
    /// second or two during a run and must not cost anything while the meter is shown).
    fn refresh(&mut self) {
        if self.visible {
            self.render_list();
            self.render();
        } else {
            self.stale = true;
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if visible && std::mem::take(&mut self.stale) {
            self.render_list();
            self.render();
        }
    }

    fn with(&self, apply: impl FnOnce(&JournalState)) {
        if let Some(app) = self.app.upgrade() {
            apply(&app.global::<JournalState>());
        }
    }

    fn export(&mut self, dungeon: Option<Arc<str>>) {
        let folder = std::env::var_os("XDG_DOWNLOAD_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|home| PathBuf::from(home).join("Downloads")))
            .unwrap_or_else(|| PathBuf::from("."));
        let name = dungeon.as_ref().map_or_else(|| "albion-history.zip".to_owned(), |id| format!("albion-dungeon-{id}.zip"));
        self.exporting = true;
        self.with(|state| state.set_exporting(true));
        let _ = self.commands.send(Command::ExportHistory { dungeon, to: folder.join(name) });
    }

    pub fn history_message(&mut self, text: String, error: bool) {
        self.exporting = false;
        self.with(|state| {
            state.set_exporting(false);
            state.set_importing(false);
            state.set_transfer_message(text.into());
            state.set_transfer_error(error);
        });
    }

    pub fn set_language(&mut self, language: Language) {
        self.language = language;
        self.refresh();
    }

    /// The live record is not a dungeon run by itself; the run arrives through `FeedEvent::Dungeon`.
    pub fn live(&mut self, _record: &Record) {}

    pub fn runs(&mut self, runs: Arc<Vec<DungeonSummary>>) {
        self.runs = runs;
        if self.selected.is_none()
            && let Some(first) = self.runs.first()
        {
            let _ = self.commands.send(Command::OpenDungeon(first.id.clone()));
        }
        self.refresh();
    }

    pub fn run(&mut self, run: Arc<DungeonRun>) {
        if self.run.as_ref().is_none_or(|previous| previous.summary.id != run.summary.id) {
            self.offset = 0;
            self.pinned = None;
            self.loadout_open = None;
            self.progression_open = None;
        }
        if self.selected.as_ref().is_none_or(|selected| *selected == run.summary.id) {
            self.run = Some(run);
            self.refresh();
        }
    }

    fn totals(&self) -> Option<&Arc<Snapshot>> {
        let run = self.run.as_ref()?;
        if self.floor == 0 { Some(&run.totals) } else { run.floors.iter().find(|floor| floor.number == self.floor).map(|floor| &floor.totals) }
    }

    fn member_ids(&self) -> Vec<String> {
        self.totals().map(|totals| totals.members.iter().map(|member| member.id.to_string()).collect()).unwrap_or_default()
    }

    fn run_member_ids(&self) -> Vec<String> {
        self.run.as_ref().map(|run| run.totals.members.iter().map(|member| member.id.to_string()).collect()).unwrap_or_default()
    }

    fn render_list(&self) {
        let language = self.language;
        let active = self.run.as_ref().map(|run| run.summary.id.clone());
        sync(
            &self.run_rows,
            self.runs.iter().map(|run| RunRow {
                id: run.id.as_ref().into(),
                entrance: run.entrance_map_name.as_ref().into(),
                status: run.status.as_ref().into(),
                line: format!("{} · {} · {}", run.name, plural(run.floors as usize, "floor", "floors"), duration(run.duration)).into(),
                damage: compact(language, run.damage).into(),
                healing: compact(language, run.healing).into(),
                started: when(language, &run.started_at).into(),
                active: active.as_ref() == Some(&run.id),
            }),
        );
    }

    pub fn render(&mut self) {
        let language = self.language;
        let Some(run) = self.run.clone() else {
            self.with(|state| state.set_has_run(false));
            return;
        };
        let totals = self.totals().cloned();
        let current = run.floors.iter().find(|floor| floor.number == self.floor);
        sync(
            &self.tabs,
            std::iter::once(FloorTab { number: 0, label: "All floors".into(), time: duration(run.totals.elapsed_seconds).into(), active: self.floor == 0 }).chain(run.floors.iter().map(|floor| FloorTab {
                number: floor.number as i32,
                label: format!("Floor {}", floor.number).into(),
                time: duration(floor.totals.elapsed_seconds).into(),
                active: self.floor == floor.number,
            })),
        );
        let metric = self.metric;
        let metric_total = |snapshot: &Snapshot| match metric {
            Metric::Damage => snapshot.total_damage,
            Metric::Healing => snapshot.total_healing,
        };
        sync(
            &self.floor_rows,
            run.floors.iter().map(|floor| FloorRow {
                number: floor.number as i32,
                label: format!("Floor {}", floor.number).into(),
                map: floor.map_name.as_ref().into(),
                map_id: floor.map_id.rsplit('@').next().unwrap_or_default().chars().take(8).collect::<String>().into(),
                time: duration(floor.totals.elapsed_seconds).into(),
                damage: precise(language, floor.totals.total_damage).into(),
                dps: precise(language, rate(floor.totals.total_damage, floor.totals.elapsed_seconds)).into(),
                healing: precise(language, floor.totals.total_healing).into(),
                hps: precise(language, rate(floor.totals.total_healing, floor.totals.elapsed_seconds)).into(),
                share: if metric_total(&run.totals) > 0.0 { (metric_total(&floor.totals) / metric_total(&run.totals) * 100.0) as f32 } else { 0.0 },
                selected: self.floor == floor.number,
            }),
        );
        let place = if self.floor > 0 { format!("Floor {}", self.floor) } else { "Entire dungeon".into() };
        let share_note = |value: f64, total: f64| if self.floor > 0 { format!("{} of dungeon", percent(value, total)) } else { String::new() };
        self.with(|state| {
            state.set_has_run(true);
            state.set_run_name(run.summary.name.as_ref().into());
            state.set_run_title(run.summary.entrance_map_name.as_ref().into());
            state.set_run_status(run.summary.status.as_ref().into());
            state.set_run_meta(format!("Started {} · {} · {}", when(language, &run.summary.started_at), plural(run.floors.len(), "floor", "floors"), run.summary.mode).into());
            state.set_has_totals(totals.is_some());
            state.set_copy_available(totals.as_ref().is_some_and(|totals| !crate::ranking::contributions(totals, metric).is_empty()));
            if let Some(totals) = &totals {
                state.set_damage(compact(language, totals.total_damage).into());
                state.set_damage_note(share_note(totals.total_damage, run.totals.total_damage).into());
                state.set_dps(precise(language, rate(totals.total_damage, totals.elapsed_seconds)).into());
                state.set_healing(compact(language, totals.total_healing).into());
                state.set_healing_note(share_note(totals.total_healing, run.totals.total_healing).into());
                state.set_hps(precise(language, rate(totals.total_healing, totals.elapsed_seconds)).into());
                state.set_time(duration(totals.elapsed_seconds).into());
                state.set_loadout_count(totals.members.len().to_string().into());
                state.set_item_grouping(!totals.item_contributions.is_empty());
            }
            state.set_chart_title(place.as_str().into());
            state.set_chart_note(current.map_or("All observed floors combined", |floor| &*floor.map_name).into());
            state.set_metric(metric.name().into());
            state.set_loadout_note(format!("{place} · Ranked by {}", metric.name()).into());
            state.set_floors_note(format!("Share of dungeon {} · Click a floor to inspect it", metric.name()).into());
            state.set_progression_note(format!("{} per player and item on each floor", if metric == Metric::Damage { "Damage" } else { "Healing" }).into());
        });
        self.render_tables();
        self.render_chart();
    }

    fn plot(&self) -> Option<Plot> {
        let totals = self.totals()?;
        let max_second = totals.elapsed_seconds.max(0.0).floor() as u32;
        let window = if self.range > 0 { self.range.min(max_second.max(30)) } else { max_second.max(30) };
        let end = window.max(max_second - self.offset.min(max_second.saturating_sub(window)));
        let start = end.saturating_sub(window);
        let latest = end.min(max_second) / self.resolution * self.resolution;
        Some(Plot { start, end, window, max_second, latest, left: 48.0, right: self.size.0 - 16.0 })
    }

    fn can_inspect(&self) -> bool {
        self.totals().is_some_and(|totals| {
            let total = match self.metric {
                Metric::Damage => totals.total_damage,
                Metric::Healing => totals.total_healing,
            };
            total > 0.0 && !totals.timeline.is_empty() && !totals.members.is_empty()
        })
    }

    fn pin(&mut self, second: i64, show: bool) {
        let Some(plot) = self.plot() else { return };
        if !self.can_inspect() {
            return;
        }
        let second = if show {
            let (second, offset) = reveal(second, self.resolution, plot.max_second, plot.window, plot.start, plot.end);
            self.offset = offset;
            second
        } else {
            (second.min(plot.end as i64).min(plot.max_second as i64).max(plot.start as i64).max(0) as u32) / self.resolution * self.resolution
        };
        self.pinned = Some(second);
        self.render_chart();
    }

    fn pick(&mut self, x: f32) {
        let Some(plot) = self.plot() else { return };
        let second = plot.start as f64 + ((x - plot.left) / (plot.right - plot.left).max(1.0)) as f64 * plot.window as f64;
        self.pin(second.floor() as i64, false);
    }

    fn key(&mut self, key: &str, shift: bool) {
        let Some(plot) = self.plot() else { return };
        if !self.can_inspect() {
            return;
        }
        let current = self.pinned.unwrap_or(plot.latest) as i64;
        let step = self.resolution as i64 * if shift { 10 } else { 1 };
        match key {
            "Escape" => {
                self.pinned = None;
                self.render_chart();
            }
            "ArrowLeft" => self.pin(current - step, true),
            "ArrowRight" => self.pin(current + step, true),
            "Home" => self.pin(0, true),
            "End" => self.pin(plot.max_second as i64, true),
            "Enter" => self.pin(current, true),
            _ => {}
        }
    }

    pub fn render_chart(&mut self) {
        let language = self.language;
        let (Some(totals), Some(plot)) = (self.totals().cloned(), self.plot()) else { return };
        let metric = self.metric;
        let resolution = self.resolution;
        let items = source_items(&totals.item_contributions);
        let by_item = self.by_item && !totals.item_contributions.is_empty();
        // (id, name, color): the players, or each player's items.
        let keys: Vec<(String, String, slint::Color)> = if by_item {
            items.iter().enumerate().map(|(index, item)| (item.id.clone(), format!("{} · {}", item.first.player_name, item.first.item), color(PALETTE[index % PALETTE.len()]))).collect()
        } else {
            totals.members.iter().chain(totals.former_members.iter().filter(|former| !totals.members.iter().any(|member| member.id == former.id))).map(|player| (player.id.to_string(), player.name.to_string(), color(&player.color))).collect()
        };
        let total = match metric {
            Metric::Damage => totals.total_damage,
            Metric::Healing => totals.total_healing,
        };
        let has_data = total > 0.0 && !totals.timeline.is_empty();
        let can_inspect = has_data && !keys.is_empty();
        // The buckets of the visible window, one column per key.
        let timeline = &totals.timeline;
        let mut seconds: Vec<u32> = Vec::new();
        let mut columns: Vec<Vec<f64>> = vec![Vec::new(); keys.len()];
        let mut peak = 0f64;
        let key_index: HashMap<&str, usize> = keys.iter().enumerate().map(|(index, key)| (key.0.as_str(), index)).collect();
        let keys_by_id = &key_index;
        let item_of: HashMap<&str, usize> = if by_item { items.iter().flat_map(|item| item.contributions.iter().map(move |detail| (&*detail.id, keys_by_id[item.id.as_str()]))).collect() } else { HashMap::new() };
        for point in &timeline[timeline.partition_point(|point| point.second < plot.start / resolution * resolution)..] {
            let second = point.second / resolution * resolution;
            if second > plot.end {
                break;
            }
            if seconds.last() != Some(&second) {
                seconds.push(second);
                columns.iter_mut().for_each(|column| column.push(0.0));
            }
            let last = seconds.len() - 1;
            if by_item {
                for (id, value) in item_values(point, metric) {
                    if let Some(&column) = item_of.get(&**id) {
                        columns[column][last] += value;
                    }
                }
            } else {
                for (id, value) in values(point, metric) {
                    if let Some(&column) = key_index.get(&**id) {
                        columns[column][last] += value;
                    }
                }
            }
        }
        for column in &columns {
            peak = column.iter().copied().fold(peak, f64::max);
        }
        let (width, height) = self.size;
        let (left, right, top, bottom) = (plot.left, plot.right, 18f32, height - 30.0);
        let maximum = (peak.max(1000.0) / 500.0).ceil() * 500.0;
        let x = |second: f64| left + ((second - plot.start as f64) / plot.window as f64) as f32 * (right - left);
        let y = |value: f64| bottom - (value / maximum) as f32 * (bottom - top);
        sync(&self.grid, (0..=4).rev().map(|step| y(maximum * step as f64 / 4.0)));
        sync(&self.y_labels, (0..=4).rev().map(|step| AxisLabel { text: compact(language, maximum * step as f64 / 4.0).into(), at: y(maximum * step as f64 / 4.0) }));
        sync(
            &self.x_labels,
            (0..=5).map(|step| {
                let second = plot.start as f64 + plot.window as f64 * step as f64 / 5.0;
                AxisLabel { text: duration(second).into(), at: x(second) }
            }),
        );
        let step = seconds.len().div_ceil(((right - left) / 2.0).max(1.0) as usize).max(1);
        let fill = keys.len() <= MAX_FILLED_SERIES;
        let series: Vec<Series> = if can_inspect && !seconds.is_empty() {
            keys.iter()
                .zip(&columns)
                .map(|(key, values)| {
                    let mut line = String::new();
                    let mut visit = |index: usize, first: bool| {
                        let _ = write!(line, "{}{:.1} {:.1}", if first { "M" } else { "L" }, x(seconds[index] as f64), y(values[index]));
                    };
                    let mut index = 0;
                    while index < seconds.len() {
                        if step == 1 {
                            visit(index, index == 0);
                        } else {
                            let (mut low, mut high) = (index, index);
                            for next in index + 1..seconds.len().min(index + step) {
                                if values[next] < values[low] {
                                    low = next;
                                }
                                if values[next] > values[high] {
                                    high = next;
                                }
                            }
                            visit(low.min(high), index == 0);
                            visit(low.max(high), false);
                        }
                        index += step;
                    }
                    let area = if fill { format!("{line}L{:.1} {bottom:.1}L{:.1} {bottom:.1}Z", x(*seconds.last().unwrap_or(&0) as f64), x(seconds[0] as f64)) } else { String::new() };
                    Series { color: key.2, line: line.into(), area: area.into(), fill }
                })
                .collect()
        } else {
            Vec::new()
        };
        sync(&self.series, series);
        let legend: Vec<LegendRow> = keys.iter().map(|key| LegendRow { name: key.1.as_str().into(), color: key.2 }).collect();
        sync(&self.legend, legend);
        let second = self.pinned.unwrap_or(plot.latest);
        let point = bucket(timeline, second, resolution);
        let limit = plot.max_second.saturating_sub(plot.window);
        let live = matches!(&*totals.mode, "live" | "replay");
        self.with(|state| {
            state.set_plot_top(top);
            state.set_plot_bottom(bottom);
            state.set_can_inspect(can_inspect);
            state.set_has_data(has_data);
            state.set_no_players(has_data && keys.is_empty());
            state.set_series_legend(can_inspect && by_item);
            state.set_grouping_index(if by_item { 1 } else { 0 });
            state.set_resolution_index(RESOLUTIONS.iter().position(|value| *value == resolution).unwrap_or(0) as i32);
            state.set_range_index(RANGES.iter().position(|value| *value == self.range).unwrap_or(1) as i32);
            state.set_empty_title(if totals.event_count == 0 { "Waiting for combat".into() } else { format!("No {} recorded", metric.name()) }.into());
            state.set_empty_text(
                if live {
                    if metric == Metric::Damage { "No attributed damage in this record yet." } else { "No attributed healing in this record yet." }
                } else {
                    "Start a packet capture in Data source to record combat."
                }
                .into(),
            );
            state.set_cursor_shown(can_inspect && second >= plot.start && second <= plot.end && second <= plot.max_second);
            state.set_cursor_x(x(second as f64));
            state.set_cursor_label(duration(second as f64).into());
            state.set_scrubber(has_data && plot.max_second > plot.window);
            state.set_scrubber_max(limit as f32);
            state.set_scrubber_value(limit.saturating_sub(self.offset) as f32);
            state.set_latest(self.offset == 0);
            let _ = width;
        });
        self.render_inspector(&totals, &point, &items);
    }

    fn render_inspector(&self, totals: &Snapshot, point: &Point, items: &[SourceItem]) {
        let language = self.language;
        let metric = self.inspector_metric;
        let record = self.record_scope;
        let resolution = self.resolution;
        let mut players: Vec<&Player> = totals.members.iter().collect();
        players.extend(totals.former_members.iter().filter(|former| !totals.members.iter().any(|member| member.id == former.id)));
        let count = players.len();
        let value = |player: &Player, kind: Metric| if record { kind.of(player) } else { values(point, kind).get(&player.id).copied().unwrap_or(0.0) };
        let total: f64 = players.iter().map(|player| value(player, metric)).sum();
        let seconds = if record { totals.elapsed_seconds } else { (totals.elapsed_seconds - point.second as f64).clamp(0.0, resolution as f64) };
        players.retain(|player| player.name.to_lowercase().contains(&self.search));
        match self.order {
            Order::Name => players.sort_by(|a, b| crate::ranking::name_order(&a.name, &b.name)),
            Order::Damage | Order::Healing => {
                let kind = if self.order == Order::Damage { Metric::Damage } else { Metric::Healing };
                players.sort_by(|a, b| value(b, kind).total_cmp(&value(a, kind)).then_with(|| crate::ranking::name_order(&a.name, &b.name)));
            }
            Order::Party => {}
        }
        let unit = metric.unit();
        let rows = players.iter().map(|player| {
            let amount = value(player, metric);
            let mut contributions: Vec<(&SourceItem, f64)> = items
                .iter()
                .filter(|item| *item.first.player_id == *player.id)
                .map(|item| (item, if record { item.of(metric) } else { item.contributions.iter().map(|detail| item_values(point, metric).get(&detail.id).copied().unwrap_or(0.0)).sum() }))
                .filter(|(_, value)| *value > 0.0)
                .collect();
            contributions.sort_by(|a, b| b.1.total_cmp(&a.1));
            let item_rows: Vec<InspectorItem> = contributions
                .iter()
                .map(|(item, item_value)| {
                    let sources = item
                        .contributions
                        .iter()
                        .map(|detail| (detail, if record { contribution(detail, metric) } else { item_values(point, metric).get(&detail.id).copied().unwrap_or(0.0) }))
                        .filter(|(_, amount)| *amount > 0.0)
                        .map(|(detail, amount)| format!("{} {}", source_kind(detail.kind.as_deref()), precise(language, amount)))
                        .collect::<Vec<_>>()
                        .join(" · ");
                    InspectorItem {
                        name: item.first.item.as_ref().into(),
                        sources: sources.into(),
                        value: precise(language, *item_value).into(),
                        icon: self.item_icon(item).0,
                        has_icon: self.item_icon(item).1,
                        tier: item_tier(item).into(),
                    }
                })
                .collect();
            let (icon, has_icon, tier) = match contributions.first() {
                Some((primary, _)) => {
                    let (icon, has_icon) = self.item_icon(primary);
                    (icon, has_icon, item_tier(primary))
                }
                None => self.weapon(player),
            };
            InspectorPlayer {
                id: player.id.as_ref().into(),
                name: player.name.as_ref().into(),
                color: color(&player.color),
                icon,
                has_icon,
                tier: tier.into(),
                damage: precise(language, value(player, Metric::Damage)).into(),
                healing: precise(language, value(player, Metric::Healing)).into(),
                rate: format!("{} {unit}", precise(language, rate(amount, seconds))).into(),
                value: precise(language, amount).into(),
                share: percent(amount, total).into(),
                open: self.open_players.contains(&*player.id),
                items: ModelRc::new(VecModel::from(item_rows)),
            }
        });
        sync(&self.inspector, rows);
        self.with(|state| {
            state.set_interval(format!("{} – {}", duration(point.second as f64), duration((point.second + resolution) as f64)).into());
            state.set_scope_index(if record { 1 } else { 0 });
            state.set_order_index(match self.order {
                Order::Damage => 0,
                Order::Healing => 1,
                Order::Name => 2,
                Order::Party => 3,
            });
            state.set_inspector_metric(metric.name().into());
            state.set_summary_total(format!("{} {}", precise(language, total), metric.name()).into());
            state.set_summary_rate(format!("{} {unit} · {count} players", precise(language, rate(total, seconds))).into());
        });
    }

    fn render_tables(&self) {
        let language = self.language;
        let Some(run) = self.run.clone() else { return };
        let metric = self.metric;
        let unit = metric.unit();
        if let Some(totals) = self.totals().cloned() {
            let items = items_by_player(&totals);
            let seconds = totals.elapsed_seconds;
            let ids = self.member_ids();
            let open = expanded(&self.loadout_open, &ids);
            let mut members: Vec<&Player> = totals.members.iter().collect();
            members.sort_by(|a, b| metric.of(b).total_cmp(&metric.of(a)));
            let mut rows = Vec::new();
            for member in members {
                let own = sorted(items.get(&*member.id).cloned().unwrap_or_default(), metric);
                let seen = combat_weapon(&member.weapon, &own);
                let is_open = open.contains(&*member.id);
                let identity = self.identity(member, seen.as_ref());
                rows.push(LoadoutRow {
                    id: member.id.as_ref().into(),
                    kind: "player".into(),
                    open: is_open,
                    name: member.name.as_ref().into(),
                    sub: format!("{} · {}", seen.as_ref().map_or(&*member.weapon, |seen| seen.1.as_str()), plural(own.len(), "source", "sources")).into(),
                    color: color(&member.color),
                    slot: "".into(),
                    damage: precise(language, member.damage).into(),
                    damage_share: member.damage_share as f32,
                    damage_bar: true,
                    dps: precise(language, member.encounter_dps).into(),
                    healing: precise(language, member.healing).into(),
                    healing_share: member.healing_share as f32,
                    healing_bar: true,
                    hps: precise(language, member.encounter_hps).into(),
                    icon: identity.0,
                    has_icon: identity.1,
                    tier: identity.2.into(),
                    bar_color: color(&member.color),
                });
                if !is_open {
                    continue;
                }
                let faded = color(&member.color).with_alpha(0x88 as f32 / 255.0);
                for item in &own {
                    rows.push(LoadoutRow {
                        id: item.id.as_str().into(),
                        kind: "item".into(),
                        open: false,
                        name: item.first.item.as_ref().into(),
                        sub: sources(item, metric, language).into(),
                        color: color(&member.color),
                        slot: slot_name(&item.first.slot).into(),
                        damage: if item.damage != 0.0 { precise(language, item.damage) } else { "—".into() }.into(),
                        damage_share: if member.damage > 0.0 { (item.damage / member.damage * 100.0) as f32 } else { 0.0 },
                        damage_bar: item.damage > 0.0,
                        dps: if item.damage != 0.0 { precise(language, rate(item.damage, seconds)) } else { String::new() }.into(),
                        healing: if item.healing != 0.0 { precise(language, item.healing) } else { "—".into() }.into(),
                        healing_share: if member.healing > 0.0 { (item.healing / member.healing * 100.0) as f32 } else { 0.0 },
                        healing_bar: item.healing > 0.0,
                        hps: if item.healing != 0.0 { precise(language, rate(item.healing, seconds)) } else { String::new() }.into(),
                        icon: self.item_icon(item).0,
                        has_icon: self.item_icon(item).1,
                        tier: item_tier(item).into(),
                        bar_color: faded,
                    });
                }
                if own.is_empty() {
                    rows.push(LoadoutRow { id: format!("{}:empty", member.id).into(), kind: "empty".into(), name: "No item sources observed".into(), ..LoadoutRow::default() });
                }
            }
            sync(&self.loadout, rows);
            let all = !ids.is_empty() && ids.iter().all(|id| open.contains(id));
            self.with(|state| {
                state.set_loadout_toggle(ids.len() > 1);
                state.set_loadout_all(all);
            });
        }
        // LoadoutProgression
        let floors: Vec<(&DungeonFloor, HashMap<String, SourceItem>)> =
            run.floors.iter().map(|floor| (floor, source_items(&floor.totals.item_contributions).into_iter().map(|item| (item.id.clone(), item)).collect())).collect();
        let items = items_by_player(&run.totals);
        let total = match metric {
            Metric::Damage => run.totals.total_damage,
            Metric::Healing => run.totals.total_healing,
        };
        let ids = self.run_member_ids();
        let open = expanded(&self.progression_open, &ids);
        let mut members: Vec<&Player> = run.totals.members.iter().collect();
        members.sort_by(|a, b| metric.of(b).total_cmp(&metric.of(a)));
        let mut rows = Vec::new();
        for member in members {
            let own = items.get(&*member.id).cloned().unwrap_or_default();
            let seen = combat_weapon(&member.weapon, &own);
            let is_open = open.contains(&*member.id);
            let identity = self.identity(member, seen.as_ref());
            let mut cells: Vec<Cell> = floors
                .iter()
                .map(|(floor, _)| {
                    let player = floor.totals.members.iter().find(|player| player.id == member.id);
                    let value = player.map_or(0.0, |player| metric.of(player));
                    let floor_total = match metric {
                        Metric::Damage => floor.totals.total_damage,
                        Metric::Healing => floor.totals.total_healing,
                    };
                    Cell { value: precise(language, value).into(), sub: format!("{} {unit} · {} PT", precise(language, player.map_or(0.0, |player| metric.rate(player))), percent(value, floor_total)).into() }
                })
                .collect();
            cells.push(Cell { value: precise(language, metric.of(member)).into(), sub: format!("{} PT", percent(metric.of(member), total)).into() });
            rows.push(ProgressionRow {
                id: member.id.as_ref().into(),
                kind: "player".into(),
                open: is_open,
                name: member.name.as_ref().into(),
                sub: seen.as_ref().map_or(&*member.weapon, |seen| seen.1.as_str()).into(),
                cells: ModelRc::new(VecModel::from(cells)),
                icon: identity.0,
                has_icon: identity.1,
                tier: identity.2.into(),
            });
            if !is_open {
                continue;
            }
            for item in sorted(own, metric).into_iter().filter(|item| item.of(metric) > 0.0) {
                let mut cells: Vec<Cell> = floors
                    .iter()
                    .map(|(floor, floor_items)| {
                        let value = floor_items.get(&item.id).map_or(0.0, |item| item.of(metric));
                        let player = floor.totals.members.iter().find(|player| player.id == member.id).map_or(0.0, |player| metric.of(player));
                        Cell {
                            value: if value != 0.0 { precise(language, value) } else { "—".into() }.into(),
                            sub: if value > 0.0 { format!("{} {unit} · {}", precise(language, rate(value, floor.totals.elapsed_seconds)), percent(value, player)) } else { String::new() }.into(),
                        }
                    })
                    .collect();
                cells.push(Cell { value: precise(language, item.of(metric)).into(), sub: format!("{} of player", percent(item.of(metric), metric.of(member))).into() });
                rows.push(ProgressionRow {
                    id: item.id.as_str().into(),
                    kind: "item".into(),
                    open: false,
                    name: item.first.item.as_ref().into(),
                    sub: slot_name(&item.first.slot).into(),
                    cells: ModelRc::new(VecModel::from(cells)),
                    icon: self.item_icon(&item).0,
                    has_icon: self.item_icon(&item).1,
                    tier: item_tier(&item).into(),
                });
            }
        }
        sync(&self.progression, rows);
        sync(&self.heads, run.floors.iter().map(|floor| SharedString::from(format!("FLOOR {}", floor.number))).chain(std::iter::once("DUNGEON TOTAL".into())));
        let all = !ids.is_empty() && ids.iter().all(|id| open.contains(id));
        self.with(|state| {
            state.set_progression_toggle(ids.len() > 1);
            state.set_progression_all(all);
        });
    }
}

/// ItemIcon.tsx: the tier under an item's picture, when the item's tier was observed.
fn item_tier(item: &SourceItem) -> String {
    item.first.tier.as_deref().filter(|_| item.first.tier_observed).unwrap_or_default().to_owned()
}

fn items_by_player(snapshot: &Snapshot) -> HashMap<String, Vec<SourceItem>> {
    let mut result: HashMap<String, Vec<SourceItem>> = HashMap::new();
    for item in source_items(&snapshot.item_contributions) {
        result.entry(item.first.player_id.to_string()).or_default().push(item);
    }
    result
}

fn sorted(mut items: Vec<SourceItem>, metric: Metric) -> Vec<SourceItem> {
    items.sort_by(|a, b| b.of(metric).total_cmp(&a.of(metric)).then_with(|| slot_order(&a.first.slot).cmp(&slot_order(&b.first.slot))));
    items
}

fn sources(item: &SourceItem, metric: Metric, language: Language) -> String {
    item.contributions
        .iter()
        .filter(|detail| contribution(detail, metric) > 0.0)
        .map(|detail| format!("{} {}", source_kind(detail.kind.as_deref()), precise(language, contribution(detail, metric))))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A player whose weapon was never seen in their equipment is shown with the main-hand item seen in combat.
fn combat_weapon(weapon: &str, items: &[SourceItem]) -> Option<(SourceItem, String)> {
    if weapon != "Weapon not observed" {
        return None;
    }
    let weapons: Vec<&SourceItem> = items.iter().filter(|item| item.first.slot.to_lowercase() == "mainhand" && item.first.item_index.is_some_and(|index| index != 0)).collect();
    let main = weapons.iter().max_by(|a, b| a.damage.total_cmp(&b.damage).then_with(|| a.healing.total_cmp(&b.healing)))?;
    let more = if weapons.len() > 1 { format!(" +{}", weapons.len() - 1) } else { String::new() };
    Some(((*main).clone(), format!("{}{more} · seen in combat", main.first.item)))
}

/// useExpanded: up to five players start open, more start closed.
fn expanded(open: &Option<HashSet<String>>, ids: &[String]) -> HashSet<String> {
    open.clone().unwrap_or_else(|| if ids.len() <= 5 { ids.iter().cloned().collect() } else { HashSet::new() })
}

fn toggle(open: &mut Option<HashSet<String>>, ids: &[String], id: &str) {
    let mut next = expanded(open, ids);
    if !next.remove(id) {
        next.insert(id.to_owned());
    }
    *open = Some(next);
}

fn toggle_all(open: &mut Option<HashSet<String>>, ids: &[String]) {
    let current = expanded(open, ids);
    let all = !ids.is_empty() && ids.iter().all(|id| current.contains(id));
    *open = Some(if all { HashSet::new() } else { ids.iter().cloned().collect() });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_follow_to_locale_string() {
        assert_eq!(when(Language::En, "2026-11-26T20:00:00.000Z"), "11/26/2026, 8:00:00 PM");
        assert_eq!(when(Language::En, "2026-11-26T00:05:09Z"), "11/26/2026, 12:05:09 AM");
        assert_eq!(when(Language::Pt, "2026-11-26T20:00:00.000Z"), "26/11/2026, 20:00:00");
    }

    #[test]
    fn reveal_matches_timeline_navigation() {
        // Inside the window: no scroll.
        assert_eq!(reveal(50, 1, 300, 120, 180, 300), (50, 130));
        assert_eq!(reveal(200, 5, 300, 120, 180, 300), (200, 0));
        // Past the end clamps to the last second.
        assert_eq!(reveal(999, 1, 300, 120, 180, 300), (300, 0));
        assert_eq!(reveal(-4, 1, 300, 120, 180, 300), (0, 180));
    }

    #[test]
    fn source_items_group_by_player_slot_and_item() {
        let contribution = |id: &str, slot: &str, index: i32, observed: bool, damage: f64| ItemContribution {
            id: id.into(),
            player_id: "p".into(),
            slot: slot.into(),
            item_index: Some(index),
            tier_observed: observed,
            damage,
            ..ItemContribution::default()
        };
        let items = source_items(&[contribution("a", "mainhand", 1, true, 10.0), contribution("b", "mainhand", 1, true, 5.0), contribution("c", "mainhand", 1, false, 1.0), contribution("d", "head", 2, true, 2.0)]);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, "p:mainhand:1");
        assert_eq!(items[0].damage, 15.0);
        assert_eq!(items[1].id, "p:mainhand:1:family");
    }

    #[test]
    fn buckets_sum_the_interval() {
        let point = |second: u32, value: f64| Point { second, damage: Values::from_iter([(Arc::from("p"), value)]), ..Point::default() };
        let timeline = [point(0, 1.0), point(4, 2.0), point(5, 4.0), point(9, 8.0), point(10, 16.0)];
        assert_eq!(bucket(&timeline, 5, 5).damage[&Arc::from("p")], 12.0);
        assert_eq!(bucket(&timeline, 0, 5).damage[&Arc::from("p")], 3.0);
    }

    #[test]
    fn expansion_defaults_to_small_parties() {
        let ids: Vec<String> = (0..3).map(|index| index.to_string()).collect();
        let mut open = None;
        assert_eq!(expanded(&open, &ids).len(), 3);
        toggle(&mut open, &ids, "1");
        assert_eq!(expanded(&open, &ids).len(), 2);
        toggle_all(&mut open, &ids);
        assert_eq!(expanded(&open, &ids).len(), 3);
        toggle_all(&mut open, &ids);
        assert!(expanded(&open, &ids).is_empty());
    }
}
