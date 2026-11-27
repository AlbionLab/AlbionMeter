//! A fight that runs without the game: eight players trading damage and healing four times a second, a growing timeline,
//! a running capture, an AlbionLab live session, a game catalog and a dungeon run, so every screen has something to
//! show before the real engine is wired in (`--fake`).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::feed::*;

const TICK: Duration = Duration::from_millis(250);

struct Fighter {
    id: Arc<str>,
    name: &'static str,
    weapon: &'static str,
    family: &'static str,
    color: &'static str,
    /// Damage and healing per second at full swing.
    dps: f64,
    hps: f64,
    joins_at: f64,
    damage: f64,
    healing: f64,
}

const ROSTER: [(&str, &str, &str, f64, f64, f64); 9] = [
    ("Lusphu", "Great Arcane Staff · T8.3", "arcanestaff", 371.0, 0.0, 0.0),
    ("KitamuraMotoyasu", "Bloodletter · T8.2", "dagger", 355.0, 0.0, 0.0),
    ("Edsonjds", "Chillhowl · T8.1", "froststaff", 342.0, 0.0, 2.0),
    ("WaitMyPool", "Infernal Scythe · T7.3", "scythe", 327.0, 0.0, 5.0),
    ("NekoKuro", "Hallowfall · T8.1", "holystaff", 64.0, 410.0, 0.0),
    ("Tobolski01", "Incubus Mace · T8.0", "mace", 238.0, 12.0, 9.0),
    ("Jonsn0wISback", "Permafrost Prism · T7.2", "froststaff", 181.0, 0.0, 14.0),
    ("Apocalypseeee", "Blight Staff · T8.2", "naturestaff", 90.0, 320.0, 20.0),
    ("KPey", "Weapon not observed", "", 120.0, 0.0, 40.0),
];
const PALETTE: [&str; 8] = ["#9ebbf3", "#eab978", "#eb95b7", "#67cbd7", "#b69aff", "#6dd1ad", "#f28b82", "#c5d87a"];

/// A small deterministic generator, enough for a plausible fight.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

pub struct FakeFeed {
    /// Simulate an update download that becomes ready.
    pub update: bool,
    /// Press Play at once; otherwise the meter waits idle for Play.
    pub autostart: bool,
}

struct Fight {
    fighters: Vec<Fighter>,
    noise: Noise,
    status: RecordStatus,
    id: u32,
    started_at: String,
    sequence: u64,
    elapsed: f64,
    events: u64,
    timeline: Vec<Point>,
    /// The seconds changed since the last snapshot, sent as its timeline.
    dirty: Vec<u32>,
    full: bool,
}

impl Fight {
    fn new() -> Self {
        let fighters = ROSTER
            .iter()
            .enumerate()
            .map(|(index, &(name, weapon, family, dps, hps, joins_at))| Fighter {
                id: format!("p{index}").into(),
                name,
                weapon,
                family,
                color: PALETTE[index % PALETTE.len()],
                dps,
                hps,
                joins_at,
                damage: 0.0,
                healing: 0.0,
            })
            .collect();
        let mut fight =
            Self { fighters, noise: Noise(0x9e37_79b9_7f4a_7c15), status: RecordStatus::Idle, id: 0, started_at: String::new(), sequence: 0, elapsed: 0.0, events: 0, timeline: Vec::new(), dirty: Vec::new(), full: true };
        fight.restart();
        fight
    }

    fn restart(&mut self) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        // Sortable like ISO 8601, which is all mergeSnapshot needs.
        self.started_at = format!("2026-01-01T00:00:00.{:020}Z", now.as_nanos());
        self.sequence = 0;
        self.elapsed = 0.0;
        self.events = 0;
        self.timeline.clear();
        self.full = true;
        for fighter in &mut self.fighters {
            fighter.damage = 0.0;
            fighter.healing = 0.0;
        }
    }

    fn step(&mut self) {
        if self.status != RecordStatus::Running {
            return;
        }
        let seconds = TICK.as_secs_f64();
        self.elapsed += seconds;
        let second = self.elapsed.floor() as u32;
        // The fight swells and ebbs; between pulls nobody hits anything.
        let phase = (self.elapsed / 40.0).fract();
        let swing = if phase > 0.85 { 0.0 } else { 0.6 + 0.8 * (self.elapsed / 7.0).sin().abs() };
        if self.timeline.last().is_none_or(|point| point.second != second) {
            self.timeline.push(Point { second, ..Point::default() });
        }
        let point = self.timeline.last_mut().expect("pushed above");
        for fighter in &mut self.fighters {
            if self.elapsed < fighter.joins_at {
                continue;
            }
            let damage = fighter.dps * seconds * swing * (0.4 + 1.2 * self.noise.next());
            let healing = fighter.hps * seconds * swing * (0.3 + 1.4 * self.noise.next());
            fighter.damage += damage;
            fighter.healing += healing;
            if damage > 0.0 {
                *point.damage.entry(fighter.id.clone()).or_default() += damage;
                *point.item_damage.entry(item_id(&fighter.id, "mainhand")).or_default() += damage * 0.8;
                *point.item_damage.entry(item_id(&fighter.id, "head")).or_default() += damage * 0.2;
                self.events += 1;
            }
            if healing > 0.0 {
                *point.healing.entry(fighter.id.clone()).or_default() += healing;
                *point.item_healing.entry(item_id(&fighter.id, "mainhand")).or_default() += healing;
                self.events += 1;
            }
        }
        if self.dirty.last() != Some(&second) {
            self.dirty.push(second);
        }
    }

    fn players(&self, from: u32, to: u32) -> (Vec<Player>, f64, f64, f64) {
        let mut damage: HashMap<&str, f64> = HashMap::new();
        let mut healing: HashMap<&str, f64> = HashMap::new();
        let whole = from == 0 && to == u32::MAX;
        if whole {
            for fighter in &self.fighters {
                damage.insert(&fighter.id, fighter.damage);
                healing.insert(&fighter.id, fighter.healing);
            }
        } else {
            for point in self.timeline.iter().filter(|point| point.second >= from && point.second < to) {
                for (id, value) in &point.damage {
                    *damage.entry(id).or_default() += value;
                }
                for (id, value) in &point.healing {
                    *healing.entry(id).or_default() += value;
                }
            }
        }
        let elapsed = if whole { self.elapsed } else { (self.elapsed.min(to as f64) - from as f64).max(0.0) };
        let total_damage: f64 = damage.values().sum();
        let total_healing: f64 = healing.values().sum();
        let players = self
            .fighters
            .iter()
            .filter(|fighter| self.elapsed >= fighter.joins_at)
            .map(|fighter| {
                let damage = damage.get(&*fighter.id).copied().unwrap_or(0.0);
                let healing = healing.get(&*fighter.id).copied().unwrap_or(0.0);
                Player {
                    membership: Membership::Active,
                    id: fighter.id.clone(),
                    name: fighter.name.into(),
                    weapon: fighter.weapon.into(),
                    weapon_id: None,
                    weapon_family: fighter.family.into(),
                    color: fighter.color.into(),
                    damage,
                    healing,
                    encounter_dps: if elapsed > 0.0 { damage / elapsed } else { 0.0 },
                    encounter_hps: if elapsed > 0.0 { healing / elapsed } else { 0.0 },
                    damage_share: if total_damage > 0.0 { damage / total_damage * 100.0 } else { 0.0 },
                    healing_share: if total_healing > 0.0 { healing / total_healing * 100.0 } else { 0.0 },
                }
            })
            .collect();
        (players, total_damage, total_healing, elapsed)
    }

    fn items(&self, players: &[Player]) -> Vec<ItemContribution> {
        players
            .iter()
            .flat_map(|player| {
                let weapon = player.weapon.split(" · ").next().unwrap_or_default();
                let tier = player.weapon.split(" · ").nth(1).map(Arc::from);
                [
                    ItemContribution {
                        kind: Some("ability".into()),
                        tier_observed: tier.is_some(),
                        id: item_id(&player.id, "mainhand"),
                        player_id: player.id.clone(),
                        player_name: player.name.clone(),
                        item_index: Some(1),
                        slot: "mainhand".into(),
                        item: if weapon == "Weapon not observed" { "Broadsword".into() } else { weapon.into() },
                        tier,
                        damage: player.damage * 0.8,
                        healing: player.healing,
                    },
                    ItemContribution {
                        kind: Some("passive".into()),
                        tier_observed: true,
                        id: item_id(&player.id, "head"),
                        player_id: player.id.clone(),
                        player_name: player.name.clone(),
                        item_index: Some(2),
                        slot: "head".into(),
                        item: "Scholar Cowl".into(),
                        tier: Some("T8.0".into()),
                        damage: player.damage * 0.2,
                        healing: 0.0,
                    },
                ]
            })
            .collect()
    }

    fn snapshot(&mut self) -> Arc<Snapshot> {
        let (members, total_damage, total_healing, elapsed) = self.players(0, u32::MAX);
        self.sequence += 1;
        let full = std::mem::take(&mut self.full);
        let timeline = if full {
            self.timeline.clone()
        } else {
            self.dirty.iter().filter_map(|second| self.timeline.iter().rev().find(|point| point.second == *second).cloned()).collect()
        };
        self.dirty.clear();
        Arc::new(Snapshot {
            id: format!("record-{}", self.id).into(),
            started_at: self.started_at.as_str().into(),
            elapsed_seconds: elapsed,
            status: self.status,
            mode: "live".into(),
            sequence: self.sequence,
            event_count: self.events,
            total_damage,
            total_healing,
            item_contributions: self.items(&members),
            members,
            former_members: Vec::new(),
            timeline,
            full,
        })
    }

    /// The floor's own snapshot: the players' sums over its seconds, with the full timeline slice.
    fn floor(&self, from: u32, to: u32) -> Arc<Snapshot> {
        let (members, total_damage, total_healing, elapsed) = self.players(from, to);
        let timeline: Vec<Point> = self
            .timeline
            .iter()
            .filter(|point| point.second >= from && point.second < to)
            .map(|point| Point { second: point.second - from, ..point.clone() })
            .collect();
        Arc::new(Snapshot {
            id: format!("floor-{from}").into(),
            started_at: self.started_at.as_str().into(),
            elapsed_seconds: elapsed,
            status: self.status,
            mode: "live".into(),
            sequence: self.sequence,
            event_count: self.events,
            total_damage,
            total_healing,
            item_contributions: self.items(&members),
            members,
            former_members: Vec::new(),
            timeline,
            full: true,
        })
    }

    fn dungeon(&self) -> Arc<DungeonRun> {
        const FLOOR: u32 = 90;
        let floors = (self.elapsed as u32 / FLOOR + 1).min(5);
        let maps = ["Halls of the Fallen", "Sunken Crypt", "Bone Pit", "Ossuary", "Throne Room"];
        let floors: Vec<DungeonFloor> = (0..floors)
            .map(|index| DungeonFloor {
                id: format!("floor-{index}").into(),
                number: index + 1,
                map_id: format!("RANDOM_DUNGEON@{:08x}{}", 0x5eed_0000u32 + index, "aa").into(),
                map_name: maps[index as usize].into(),
                totals: self.floor(index * FLOOR, if index + 1 == floors { u32::MAX } else { (index + 1) * FLOOR }),
            })
            .collect();
        let totals = self.floor(0, u32::MAX);
        Arc::new(DungeonRun {
            summary: DungeonSummary {
                id: "run-1".into(),
                started_at: self.started_at.as_str().into(),
                name: "Solo Dungeon".into(),
                entrance_map_name: "Tumbleweed Hollow".into(),
                mode: "live".into(),
                status: if self.status == RecordStatus::Running { "running".into() } else { "completed".into() },
                floors: floors.len() as u32,
                duration: totals.elapsed_seconds,
                damage: totals.total_damage,
                healing: totals.total_healing,
            },
            floors,
            totals,
        })
    }

    fn command(&mut self, command: &Command) -> Option<&'static str> {
        use RecordStatus::*;
        match (command, self.status) {
            (Command::StartRecord, Idle | Completed) => {
                self.id += 1;
                self.restart();
                self.status = Running;
            }
            (Command::StartRecord, _) => return Some("Stop the current record before starting another one."),
            (Command::PauseRecord, Running) => self.status = Paused,
            (Command::ResumeRecord, Paused) => self.status = Running,
            (Command::NewRecord, Running | Paused) => self.restart(),
            (Command::NewRecord, _) => return Some("There is no active record to reset."),
            (Command::StopRecord, Running | Paused) => self.status = Completed,
            (Command::StopRecord, _) => return Some("There is no active record to stop."),
            _ => return Some("Press Play to start a record first."),
        }
        None
    }
}

fn item_id(player: &str, slot: &str) -> Arc<str> {
    format!("{player}:{slot}").into()
}

fn catalog() -> Catalog {
    let tiers = ["Novice's", "Journeyman's", "Adept's", "Expert's", "Master's", "Grandmaster's", "Elder's"];
    let families = [("Bow", "bow", "2H_BOW"), ("Warbow", "bow", "2H_WARBOW"), ("Longbow", "bow", "2H_LONGBOW"), ("Fire Staff", "firestaff", "MAIN_FIRESTAFF"), ("Great Arcane Staff", "arcanestaff", "2H_ARCANESTAFF"), ("Broadsword", "sword", "MAIN_SWORD")];
    let mut weapons = Vec::new();
    for (name, family, id) in families {
        for (index, tier) in tiers.iter().enumerate() {
            let level = index as u8 + 2;
            if level < 4 && name != "Bow" {
                continue;
            }
            weapons.push(CatalogEntry { id: format!("T{level}_{id}").into(), name: format!("{tier} {name}").into(), group: family.into(), tier: Some(level) });
        }
    }
    let maps = (0..140)
        .map(|index| CatalogEntry {
            id: format!("{:04}", 1000 + index).into(),
            name: format!("{} {}", ["Thetford", "Lymhurst", "Bridgewatch", "Martlock", "Fort Sterling"][index % 5], index / 5 + 1).into(),
            group: ["CITY", "SAFEAREA", "OPENPVP_YELLOW", "OPENPVP_RED", "DUNGEON_SOLO"][index % 5].into(),
            tier: if index % 5 == 0 { None } else { Some((index % 8 + 1) as u8) },
        })
        .collect();
    Catalog { game_version: Some("albiononline-linux-full-1.32.030.345319".into()), imported_at: Some("2026-11-20T18:04:00Z".into()), weapons, maps }
}

fn devices() -> DeviceList {
    let device = |name: &str, description: &str, method: &str, flags| CaptureDevice { name: name.into(), description: description.into(), method: method.into(), flags };
    DeviceList {
        library: Some("libpcap 1.10.5".into()),
        debug_enabled: false,
        selected_device: Some("raw:all".into()),
        methods: vec![
            CaptureMethod { id: "socket".into(), name: "System socket (Linux)".into(), available: true },
            CaptureMethod { id: "pcap".into(), name: "libpcap".into(), available: true },
        ],
        devices: vec![
            device("raw:all", "All interfaces (Linux socket)", "socket", 0),
            device("raw:enp5s0", "enp5s0 (Linux socket)", "socket", 1),
            device("enp5s0", "enp5s0", "pcap", 1),
            device("any", "Pseudo-device that captures on all interfaces", "pcap", 0),
        ],
        error: None,
    }
}

impl MeterFeed for FakeFeed {
    fn run(self: Box<Self>, publish: Box<dyn Fn(FeedEvent) + Send>, commands: Receiver<Command>) {
        let begun = Instant::now();
        // The loading screen shows until the "server" answers.
        std::thread::sleep(Duration::from_millis(700));
        publish(FeedEvent::Version(env!("CARGO_PKG_VERSION").into()));
        publish(FeedEvent::Connection(Connection::Connected));
        publish(FeedEvent::Devices(Arc::new(devices())));
        publish(FeedEvent::Catalog(Arc::new(catalog())));
        publish(FeedEvent::StoragePath("/home/player/.local/share/AlbionMeter/albion-meter.db".into()));
        let mut fight = Fight::new();
        if self.autostart {
            fight.command(&Command::StartRecord);
        }
        publish(FeedEvent::Snapshot(fight.snapshot()));
        let mut lab = LabStatus {
            server: "https://albionlab.app".into(),
            connected: true,
            account: Some(("KPey".into(), None)),
            visibility: "unlisted".into(),
            suggested_title: Some("Tumbleweed Hollow · Solo Dungeon".into()),
            ..LabStatus::default()
        };
        let session = |state: &str| LabSession { id: "aBc123XyZ0".into(), url: "https://albionlab.app/meter/aBc123XyZ0".into(), visibility: "unlisted".into(), state: state.into(), ..LabSession::default() };
        if self.autostart {
            lab.session = Some(session("live"));
        }
        publish(FeedEvent::Lab(Arc::new(lab.clone())));
        let mut capture = CaptureStatus { state: "running".into(), source: "live-capture".into(), library: Some("libpcap 1.10.5".into()), adapter_dropped: Some(0), ..CaptureStatus::default() };
        let mut pairing_since: Option<Instant> = None;
        let mut next = Instant::now();
        let mut tick = 0u64;
        loop {
            match commands.recv_timeout(next.saturating_duration_since(Instant::now())) {
                Ok(command) => {
                    match &command {
                        Command::StartRecord | Command::PauseRecord | Command::ResumeRecord | Command::NewRecord | Command::StopRecord => {
                            std::thread::sleep(Duration::from_millis(120));
                            let failure = fight.command(&command);
                            match (&command, failure) {
                                (Command::StartRecord, None) => lab.session = Some(session("live")),
                                (Command::StopRecord, None) => lab.session = lab.session.take().map(|session| LabSession { state: "completed".into(), ..session }),
                                _ => {}
                            }
                            publish(FeedEvent::Snapshot(fight.snapshot()));
                            publish(FeedEvent::Lab(Arc::new(lab.clone())));
                            publish(FeedEvent::CommandDone(failure.map(String::from)));
                        }
                        Command::StopCapture => {
                            capture.state = "stopped".into();
                            publish(FeedEvent::Capture(Arc::new(capture.clone())));
                            publish(FeedEvent::CaptureDone(None));
                        }
                        Command::StartCapture { .. } => {
                            capture.state = "running".into();
                            capture.stop_reason = None;
                            publish(FeedEvent::Capture(Arc::new(capture.clone())));
                            publish(FeedEvent::CaptureDone(None));
                        }
                        Command::Replay(_) => publish(FeedEvent::CaptureDone(Some("The file is not a PCAP or PCAPNG capture.".into()))),
                        Command::RefreshDevices => publish(FeedEvent::Devices(Arc::new(devices()))),
                        Command::LabConnect => {
                            lab.pairing = Some(("K7Q-4PM".into(), "https://albionlab.app/pair?code=K7Q-4PM".into()));
                            lab.pairing_error = None;
                            pairing_since = Some(Instant::now());
                            publish(FeedEvent::Lab(Arc::new(lab.clone())));
                        }
                        Command::LabDisconnect => {
                            lab.connected = false;
                            lab.account = None;
                            lab.session = None;
                            lab.pairing_error = Some("revoked".into());
                            publish(FeedEvent::Lab(Arc::new(lab.clone())));
                        }
                        Command::LabTitle(title) => {
                            if let Some(session) = &mut lab.session {
                                session.title = Some(title.clone()).filter(|title| !title.is_empty());
                            }
                            publish(FeedEvent::Lab(Arc::new(lab.clone())));
                        }
                        Command::LabVisibility(value) => {
                            lab.visibility = value.as_str().into();
                            if let Some(session) = &mut lab.session {
                                session.visibility = value.as_str().into();
                            }
                            publish(FeedEvent::Lab(Arc::new(lab.clone())));
                        }
                        Command::LabAnonymize(value) => {
                            lab.anonymize = *value;
                            if let Some(session) = &mut lab.session {
                                session.anonymize = *value;
                            }
                            publish(FeedEvent::Lab(Arc::new(lab.clone())));
                        }
                        Command::LabShareLocation(value) => {
                            lab.share_location = *value;
                            publish(FeedEvent::Lab(Arc::new(lab.clone())));
                        }
                        Command::ImportCatalog(_) => {
                            std::thread::sleep(Duration::from_millis(600));
                            let catalog = catalog();
                            publish(FeedEvent::CatalogMessage(format!("imported:{}:{}", catalog.maps.len(), catalog.weapons.len())));
                            publish(FeedEvent::Catalog(Arc::new(catalog)));
                        }
                        Command::OpenDungeon(_) => publish(FeedEvent::Dungeon(fight.dungeon())),
                        Command::ExportHistory { to, .. } => {
                            publish(FeedEvent::HistoryMessage { text: format!("Exported to {}", to.display()), error: false });
                        }
                        Command::ImportHistory(_) => {
                            publish(FeedEvent::HistoryMessage { text: "Imported 3 encounters and 1 dungeons.".into(), error: false });
                        }
                        Command::Icon(_) | Command::InstallUpdate => {}
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }
            next += TICK;
            tick += 1;
            // Like the engine, nothing is sent while the record is not counting.
            if fight.status == RecordStatus::Running {
                fight.step();
                publish(FeedEvent::Snapshot(fight.snapshot()));
            }
            if pairing_since.is_some_and(|since| since.elapsed() > Duration::from_secs(4)) {
                pairing_since = None;
                lab.pairing = None;
                lab.connected = true;
                lab.account = Some(("KPey".into(), None));
                publish(FeedEvent::Lab(Arc::new(lab.clone())));
            }
            // The capture and the dungeon are polled every one and two seconds in the web UI.
            if tick.is_multiple_of(4) {
                let frames = (begun.elapsed().as_secs_f64() * 92.0) as u64;
                capture.report = Some(CaptureReport {
                    frames,
                    bytes: frames * 121,
                    transport_messages: frames * 7 / 6,
                    pending_unidentified: 0,
                    counters: vec![
                        ("udpDatagrams".into(), frames as f64),
                        ("photonPackets".into(), frames as f64 * 0.98),
                        ("transportMessages".into(), (frames * 7 / 6) as f64),
                        ("reliableCommands".into(), frames as f64 * 0.7),
                        ("fragments".into(), frames as f64 * 0.04),
                        ("events".into(), frames as f64 * 0.9),
                        ("operationResponses".into(), frames as f64 * 0.1),
                        ("combatEvents".into(), fight.events as f64),
                        ("pendingUnidentified".into(), 0.0),
                    ],
                });
                publish(FeedEvent::Capture(Arc::new(capture.clone())));
            }
            if tick.is_multiple_of(8) {
                let run = fight.dungeon();
                publish(FeedEvent::Dungeons(Arc::new(vec![run.summary.clone()])));
                publish(FeedEvent::Dungeon(run));
            }
            if self.update {
                let seconds = begun.elapsed().as_secs_f64();
                let version = "0.2.1".to_owned();
                if (3.0..23.0).contains(&seconds) && tick.is_multiple_of(2) {
                    let percent = ((seconds - 3.0) / 20.0 * 100.0) as u8;
                    let total = 48_000_000u64;
                    publish(FeedEvent::Update(UpdateState::Downloading { version, percent, transferred: Some(total * percent as u64 / 100), total: Some(total) }));
                } else if (23.0..23.3).contains(&seconds) {
                    publish(FeedEvent::Update(UpdateState::Ready { version }));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fight_grows_and_sends_only_changed_points() {
        let mut fight = Fight::new();
        fight.command(&Command::StartRecord);
        let first = fight.snapshot();
        assert!(first.full);
        for _ in 0..40 {
            fight.step();
        }
        let next = fight.snapshot();
        assert!(!next.full);
        assert!(next.timeline.len() <= 11, "{}", next.timeline.len());
        assert_eq!(fight.timeline.len(), 11);
        assert!(next.total_damage > 0.0);
        let shares: f64 = next.members.iter().map(|player| player.damage_share).sum();
        assert!((shares - 100.0).abs() < 1e-6);
        assert_eq!(fight.command(&Command::ResumeRecord), Some("Press Play to start a record first."));
        assert_eq!(fight.command(&Command::PauseRecord), None);
        assert_eq!(fight.status, RecordStatus::Paused);
    }
}
