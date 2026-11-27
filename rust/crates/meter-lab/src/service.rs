//! AlbionLabService.cs: pairing with an AlbionLab account, the albionlab.json settings, and the live session the
//! current record is published to every second over the publish hub.
//!
//! All state sits behind one lock that is never held across an await. Everything an await started for one session
//! checks, when it lands, that the session is still the current one, so an answer for a session that was replaced or
//! disconnected meanwhile never brings it back.

use crate::compactor::{SessionCompactor, SourceDungeon, SourceFloor, SourceItem, SourceMember, SourceSecond, SourceSnapshot, SourceWeapon};
use crate::protocol::{self, SessionDelta};
use crate::signalr::{HubConnection, HubError, HubState};
use meter_store::{json, paths, DateTimeOffset};
use serde::{Deserialize, Serialize};
use meter_core::engine::{CombatSnapshot, PlayerStats, Values};
use meter_core::dungeon::DungeonRunSnapshot;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// What the service reads from the rest of the meter: the current record and dungeon runs, the local character, and
/// the game catalogs. The record is read typed, shared, as the engine built it: no copy or JSON tree per second.
pub trait LabContext: Send + Sync + 'static {
    /// The meter's current record.
    fn record(&self) -> Arc<CombatSnapshot>;
    fn dungeons(&self) -> Vec<Arc<DungeonRunSnapshot>>;
    /// The local character's id and map, when the capture knows them.
    fn character(&self) -> Option<(String, Option<String>)>;
    /// Unique item id (T6_2H_HAMMER@1) of an item index.
    fn item_id(&self, index: i32) -> Option<String>;
    fn item_id_by_name(&self, name: &str) -> Option<String>;
    fn weapon_family(&self, index: i32) -> Option<String>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub name: String,
    pub avatar_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pub code: String,
    pub verify_url: String,
    pub expires_at: DateTimeOffset,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabSession {
    pub id: String,
    pub url: String,
    pub visibility: String,
    pub anonymize: bool,
    pub state: String,
    pub error: Option<String>,
    pub published_at: Option<DateTimeOffset>,
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabStatus {
    pub server: String,
    pub connected: bool,
    pub account: Option<Account>,
    pub pairing: Option<Pairing>,
    pub pairing_error: Option<String>,
    pub session: Option<LabSession>,
    pub share_location: bool,
    pub visibility: String,
    pub anonymize: bool,
    pub suggested_title: Option<String>,
}

/// `albionlab.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub device_token: Option<String>,
    pub account_name: Option<String>,
    pub avatar_url: Option<String>,
    pub share_location: bool,
    pub visibility: String,
    pub anonymize: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { device_token: None, account_name: None, avatar_url: None, share_location: false, visibility: "unlisted".into(), anonymize: false }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    /// Written for the user only: it holds the device token.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = json::to_string(self).map_err(std::io::Error::other)?;
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        // On disk before the rename: a power cut must never leave an empty file (and the device token lost).
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    }
}

#[derive(Debug)]
pub enum LabError {
    /// AlbionLab could not be reached or answered with an error (C# `HttpRequestException`).
    Http(String),
    Hub(HubError),
    Cancelled,
}

impl fmt::Display for LabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(message) => f.write_str(message),
            Self::Hub(error) => error.fmt(f),
            Self::Cancelled => f.write_str("The operation was canceled."),
        }
    }
}

impl std::error::Error for LabError {}

impl From<reqwest::Error> for LabError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.to_string())
    }
}

impl From<HubError> for LabError {
    fn from(error: HubError) -> Self {
        Self::Hub(error)
    }
}

type LabResult<T> = Result<T, LabError>;

struct Published {
    started_at: DateTimeOffset,
    self_id: Option<String>,
    record: SourceSnapshot,
    encounters: Vec<SourceSnapshot>,
    dungeons: Vec<SourceDungeon>,
}

struct PendingFinish {
    id: String,
    delta: SessionDelta,
    attempts: u32,
}

#[derive(Default)]
struct State {
    settings: Settings,
    pairing: Option<Pairing>,
    poll_token: Option<String>,
    pairing_error: Option<String>,
    hub: Option<HubConnection>,
    compactor: SessionCompactor,
    record_id: Option<String>,
    finished_record: Option<String>,
    session: Option<LabSession>,
    // A failed create waits before the next try, longer each time, instead of asking the site every second.
    next_create: DateTimeOffset,
    failed_creates: u32,
    // A hub that cannot connect is tried again after 5, 10, 20, 40 then every 60 seconds, not on every tick.
    next_hub: DateTimeOffset,
    failed_hubs: u32,
    // The "completed" of a session that could not be sent yet (hub reconnecting, network down); retried on each tick.
    pending_finish: Option<PendingFinish>,
    last_published: Option<Published>,
    published_start: Option<DateTimeOffset>,
}

struct Inner {
    server: String,
    settings_path: PathBuf,
    http: reqwest::Client,
    context: Arc<dyn LabContext>,
    state: Mutex<State>,
}

#[derive(Clone)]
pub struct AlbionLabService {
    inner: Arc<Inner>,
}

async fn cancellable<T>(cancel: &CancellationToken, work: impl std::future::Future<Output = LabResult<T>>) -> LabResult<T> {
    tokio::select! {
        result = work => result,
        _ = cancel.cancelled() => Err(LabError::Cancelled),
    }
}

/// The configured AlbionLab address when it is https (or http on this machine), as scheme://authority.
fn server(configured: Option<&str>) -> String {
    configured
        .and_then(|url| reqwest::Url::parse(url.trim_end_matches('/')).ok())
        .filter(|url| match url.scheme() {
            "https" => true,
            "http" => match url.host() {
                Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
                Some(url::Host::Ipv4(address)) => address.is_loopback(),
                Some(url::Host::Ipv6(address)) => address.is_loopback(),
                None => false,
            },
            _ => false,
        })
        .and_then(|url| authority(&url))
        .unwrap_or_else(|| "https://albionlab.app".into())
}

fn authority(url: &reqwest::Url) -> Option<String> {
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    })
}

/// The local player's map as a hash: the site only compares places for equality, to find two records that put one
/// player in two places at once, and keeps only its own secret-keyed hash of it, never the map.
pub fn place(map: Option<&str>) -> Option<String> {
    let map = map.filter(|map| !map.is_empty())?;
    let digest = Sha256::digest(format!("albionlab-place\n{map}").as_bytes());
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>()[..24].to_owned())
}

/// A session name from what was recorded: the dungeon or expedition runs, their entrance when the location is shared
/// (unshared runs carry none), and the party size; without runs, the recording player and weapon family.
pub fn suggest_title(self_id: Option<&str>, record: &SourceSnapshot, runs: &[SourceDungeon]) -> Option<String> {
    let count = record.members.len();
    let party = if count <= 1 { "solo".to_owned() } else { format!("{count} players") };
    let title = if !runs.is_empty() {
        let mut groups: Vec<(&str, usize)> = Vec::new();
        for run in runs {
            match groups.iter_mut().find(|(name, _)| *name == run.name) {
                Some(group) => group.1 += 1,
                None => groups.push((&run.name, 1)),
            }
        }
        let names = groups.iter().map(|(name, count)| if *count > 1 { format!("{name} ×{count}") } else { (*name).to_owned() }).collect::<Vec<_>>().join(" + ");
        let entrance = runs.iter().filter_map(|run| run.entrance.clone()).find(|name| !name.trim().is_empty());
        [Some(names), entrance, Some(party)].into_iter().flatten().filter(|part| !part.trim().is_empty()).collect::<Vec<_>>().join(" · ")
    } else if count > 1 {
        format!("Group fight · {party}")
    } else {
        let player = record.members.iter().find(|member| Some(member.id.as_str()) == self_id).or(record.members.first())?;
        if !player.family.is_empty() && player.family != "unknown" && player.family != "seen in combat" {
            format!("{} solo · {}", player.name, player.family)
        } else {
            format!("{} solo", player.name)
        }
    };
    // .NET counts UTF-16 units.
    let mut units = 0;
    Some(title.chars().take_while(|character| { units += character.len_utf16(); units <= 80 }).collect())
}

fn time(value: meter_core::cs::DateTimeOffset) -> DateTimeOffset {
    DateTimeOffset::from_unix_milliseconds(value.0)
}

fn amounts(values: &Values) -> Vec<(String, f64)> {
    values.iter().map(|(key, amount)| (key.to_string(), amount)).collect()
}

fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// A weapon seen in combat for a player whose weapon was not observed: their most used main-hand item.
struct Fallback {
    item: String,
    index: Option<i32>,
}

fn combat_weapons(record: &CombatSnapshot) -> HashMap<String, Fallback> {
    let mut sums: Vec<(String, i64, String, f64, f64)> = Vec::new();
    for item in record.item_contributions.iter().flatten() {
        let Some(index) = item.item_index.filter(|index| *index > 0) else { continue };
        if item.slot != "mainhand" || !item.tier_observed {
            continue;
        }
        let index = i64::from(index);
        match sums.iter_mut().find(|sum| sum.0 == item.player_id && sum.1 == index) {
            Some(sum) => {
                sum.3 += item.damage;
                sum.4 += item.healing;
            }
            None => sums.push((item.player_id.clone(), index, item.item.clone(), item.damage, item.healing)),
        }
    }
    let mut best: Vec<(String, i64, String, f64, f64)> = Vec::new();
    for sum in sums {
        match best.iter_mut().find(|current| current.0 == sum.0) {
            Some(current) if sum.3 > current.3 || (sum.3 == current.3 && sum.4 > current.4) => *current = sum,
            Some(_) => {}
            None => best.push(sum),
        }
    }
    best.into_iter().map(|(player, index, item, _, _)| (player, Fallback { item, index: Some(index as i32) })).collect()
}

impl AlbionLabService {
    /// `configured` is the AlbionLab address from the settings (`AlbionLab:Url`), defaulting to albionlab.app.
    pub fn new(storage: &Path, configured: Option<&str>, context: Arc<dyn LabContext>) -> Self {
        let settings_path = storage.join(paths::ALBIONLAB_FILE);
        let state = State { settings: Settings::load(&settings_path), ..State::default() };
        Self {
            inner: Arc::new(Inner {
                server: server(configured),
                settings_path,
                http: reqwest::Client::builder().timeout(Duration::from_secs(15)).build().expect("HTTP client"),
                context,
                state: Mutex::new(state),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save(&self, state: &State) {
        if let Err(error) = state.settings.save(&self.inner.settings_path) {
            eprintln!("the AlbionLab settings could not be saved: {error}");
        }
    }

    pub fn server(&self) -> &str {
        &self.inner.server
    }

    pub fn status(&self) -> LabStatus {
        let state = self.state();
        let settings = &state.settings;
        let connected = settings.device_token.is_some();
        LabStatus {
            server: self.inner.server.clone(),
            connected,
            account: connected.then(|| Account { name: settings.account_name.clone().unwrap_or_default(), avatar_url: settings.avatar_url.clone().unwrap_or_default() }),
            pairing: state.pairing.clone().filter(|pairing| pairing.expires_at.utc_ticks > DateTimeOffset::now().utc_ticks),
            pairing_error: state.pairing_error.clone(),
            session: state.session.clone(),
            share_location: settings.share_location,
            visibility: settings.visibility.clone(),
            anonymize: settings.anonymize,
            suggested_title: state.last_published.as_ref().and_then(|published| suggest_title(published.self_id.as_deref(), &published.record, &published.dungeons)),
        }
    }

    fn verify_url(&self, offered: Option<&str>, code: &str) -> String {
        match offered.and_then(|url| reqwest::Url::parse(url).ok()) {
            Some(url) if authority(&url).as_deref() == Some(self.inner.server.as_str()) => url.to_string(),
            _ => format!("{}/link?code={}", self.inner.server, crate::escape_data_string(code)),
        }
    }

    /// Asks AlbionLab for a pairing code for this machine; the account confirms it on the site.
    pub async fn connect(&self) -> LabResult<LabStatus> {
        let name = format!("Albion Meter · {}", gethostname::gethostname().to_string_lossy());
        let response = self.inner.http.post(format!("{}/api/devices/pair", self.inner.server)).json(&json!({"name": name})).send().await?.error_for_status()?;
        let body: Value = response.json().await?;
        let field = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| LabError::Http(format!("AlbionLab sent no {key}.")));
        let (poll_token, code) = (field("pollToken")?, field("code")?);
        let expires_in = body.get("expiresIn").and_then(Value::as_i64).ok_or_else(|| LabError::Http("AlbionLab sent no expiresIn.".into()))?;
        let verify_url = self.verify_url(body.get("verifyUrl").and_then(Value::as_str), &code);
        {
            let mut state = self.state();
            state.poll_token = Some(poll_token);
            let now = DateTimeOffset::now();
            state.pairing = Some(Pairing { code, verify_url, expires_at: DateTimeOffset { utc_ticks: now.utc_ticks + expires_in * 10_000_000, ..now } });
            state.pairing_error = None;
        }
        Ok(self.status())
    }

    pub async fn disconnect(&self) {
        let old = {
            let mut state = self.state();
            state.settings.device_token = None;
            state.settings.account_name = None;
            state.settings.avatar_url = None;
            self.save(&state);
            state.session = None;
            state.record_id = None;
            state.pairing = None;
            state.poll_token = None;
            state.hub.take()
        };
        if let Some(old) = old {
            old.stop().await;
        }
    }

    pub fn share_location(&self, share: bool) -> LabStatus {
        {
            let mut state = self.state();
            state.settings.share_location = share;
            self.save(&state);
        }
        self.status()
    }

    /// Saves the visibility and anonymize defaults and applies them, with the title, to the live session.
    pub async fn update_session(&self, title: Option<&str>, visibility: Option<&str>, anonymize: Option<bool>) -> LabResult<LabStatus> {
        let (token, current) = {
            let mut state = self.state();
            if let Some(visibility) = visibility {
                state.settings.visibility = visibility.into();
            }
            if let Some(anonymize) = anonymize {
                state.settings.anonymize = anonymize;
            }
            self.save(&state);
            (state.settings.device_token.clone(), state.session.clone())
        };
        let (Some(token), Some(current)) = (token, current.filter(|session| !session.id.is_empty())) else { return Ok(self.status()) };
        let response = self
            .inner
            .http
            .patch(format!("{}/api/sessions/{}", self.inner.server, current.id))
            .bearer_auth(token)
            .json(&json!({"title": title, "visibility": visibility, "anonymize": anonymize}))
            .send()
            .await?;
        let status = response.status();
        let body: Option<Value> = if status.is_success() { Some(response.json().await?) } else { None };
        let mut state = self.state();
        if let Some(session) = state.session.as_mut().filter(|session| session.id == current.id) {
            match body {
                Some(body) => {
                    session.visibility = text(&body, "visibility");
                    session.anonymize = body.get("anonymize").and_then(Value::as_bool).unwrap_or_default();
                    session.title = body.get("title").and_then(Value::as_str).map(str::to_owned);
                    session.error = None;
                }
                None => session.error = Some(format!("AlbionLab did not save the change ({}).", status.as_u16())),
            }
        }
        drop(state);
        Ok(self.status())
    }

    /// The publish loop: once a second, pairing, the waiting "completed" and the record's delta, until `cancel`.
    /// Ending it then sends the live session's "completed" (within 3 seconds) instead of leaving it to time out.
    pub async fn run(&self, cancel: CancellationToken) {
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        timer.tick().await;
        loop {
            tokio::select! {
                _ = timer.tick() => {}
                _ = cancel.cancelled() => break,
            }
            if let Err(error) = cancellable(&cancel, self.tick(&cancel)).await {
                if cancel.is_cancelled() {
                    break;
                }
                let mut state = self.state();
                state.compactor.reset();
                if let Some(session) = state.session.as_mut() {
                    session.state = "offline".into();
                    session.error = Some(error.to_string());
                }
            }
        }
        self.stop().await;
    }

    async fn tick(&self, cancel: &CancellationToken) -> LabResult<()> {
        self.poll_pairing().await?;
        self.send_pending_finish().await?;
        self.publish(cancel).await
    }

    /// Ends the live session on the site, as closing the meter does.
    pub async fn stop(&self) {
        let live = {
            let state = self.state();
            state.settings.device_token.is_some() && state.record_id.is_some()
        };
        if live {
            let _ = tokio::time::timeout(Duration::from_secs(3), self.finish_previous()).await;
        }
    }

    pub(crate) async fn poll_pairing(&self) -> LabResult<()> {
        let Some(token) = self.state().poll_token.clone() else { return Ok(()) };
        if DateTimeOffset::now().utc_ticks / 10_000_000 % 60 % 3 != 0 {
            return Ok(());
        }
        let response = self.inner.http.post(format!("{}/api/devices/poll", self.inner.server)).json(&json!({"pollToken": token})).send().await?;
        let status = response.status();
        if status == reqwest::StatusCode::ACCEPTED {
            return Ok(());
        }
        let body: Value = response.json().await?;
        let mut state = self.state();
        state.poll_token = None;
        state.pairing = None;
        match body.get("deviceToken").and_then(Value::as_str).filter(|_| status.is_success()) {
            Some(device) => {
                state.settings.device_token = Some(device.into());
                let account = body.get("account").unwrap_or(&Value::Null);
                state.settings.account_name = account.get("name").and_then(Value::as_str).map(str::to_owned);
                state.settings.avatar_url = account.get("avatarUrl").and_then(Value::as_str).map(str::to_owned);
                self.save(&state);
            }
            None => state.pairing_error = Some(if status == reqwest::StatusCode::FORBIDDEN { "denied" } else { "expired" }.into()),
        }
        Ok(())
    }

    pub(crate) async fn publish(&self, cancel: &CancellationToken) -> LabResult<()> {
        let Some(token) = self.state().settings.device_token.clone() else { return Ok(()) };
        let context = self.inner.context.clone();
        let record = context.record();
        let (id, status, started) = (record.id.clone(), record.status.clone(), time(record.started_at));
        let active = record.mode == "live" && matches!(status.as_str(), "running" | "paused" | "completed");
        let current = {
            let state = self.state();
            (state.finished_record.clone(), state.record_id.clone())
        };
        if !active || current.0.as_deref() == Some(id.as_str()) {
            return Ok(());
        }
        if current.1.as_deref() != Some(id.as_str()) {
            if current.1.is_some() {
                self.finish_previous().await?;
            }
            if status == "completed" || DateTimeOffset::now() < self.state().next_create {
                return Ok(());
            }
            self.create_session(&token, &id, started).await?;
            if self.state().record_id.as_deref() != Some(id.as_str()) {
                return Ok(());
            }
        } else {
            // Reset keeps the record but starts it again from zero (a new start time): the session is sent again in full.
            let mut state = self.state();
            if state.published_start.is_some_and(|start| start != started) {
                state.compactor.reset();
            }
        }
        self.state().published_start = Some(started);
        let connection = self.hub().await?;
        let live = self.state().session.clone();
        let (Some(connection), Some(live)) = (connection, live.filter(|session| !session.id.is_empty())) else { return Ok(()) };
        let character = context.character();
        let self_id = character.as_ref().map(|(id, _)| id.clone());
        let share = self.state().settings.share_location;
        let earliest = started.utc_ticks - 5 * 10_000_000;
        let runs: Vec<Arc<DungeonRunSnapshot>> = context.dungeons().into_iter().filter(|run| time(run.started_at).utc_ticks >= earliest).collect();
        let fallback = combat_weapons(&record);
        let encounters: Vec<SourceSnapshot> = runs.iter().flat_map(|run| run.floors.iter().flat_map(|floor| &floor.encounters)).map(|encounter| self.source(encounter, &fallback)).collect();
        let dungeons: Vec<SourceDungeon> = runs
            .iter()
            .map(|run| SourceDungeon {
                id: run.id.clone(),
                name: run.name.clone(),
                started_at: time(run.started_at),
                status: run.status.clone(),
                entrance: share.then(|| run.entrance_map_name.clone()),
                floors: run
                    .floors
                    .iter()
                    .map(|floor| SourceFloor {
                        id: format!("floor-{}", floor.number),
                        number: floor.number,
                        map: share.then(|| floor.map_name.clone()),
                        snaps: floor.encounters.iter().map(|encounter| encounter.id.clone()).collect(),
                    })
                    .collect(),
            })
            .collect();
        let source = self.source(&record, &fallback);
        let place = place(character.as_ref().and_then(|(_, map)| map.as_deref()));
        let delta = {
            let mut state = self.state();
            let delta = state.compactor.next(&status, started, self_id.as_deref(), &source, &encounters, &dungeons, place.as_deref());
            state.last_published = Some(Published { started_at: started, self_id, record: source, encounters, dungeons });
            delta
        };
        let Some(delta) = delta else { return Ok(()) };
        let result = cancellable(cancel, async { Ok(connection.invoke::<i64>("Publish", (&live.id, &delta)).await) }).await?;
        let mut state = self.state();
        match result {
            Ok(_) => {
                if let Some(session) = state.session.as_mut().filter(|session| session.id == live.id) {
                    session.state = "live".into();
                    session.error = None;
                    session.published_at = Some(DateTimeOffset::now());
                }
                if status == "completed" {
                    state.finished_record = Some(id);
                    if let Some(session) = state.session.as_mut().filter(|session| session.id == live.id) {
                        session.state = "completed".into();
                    }
                }
                Ok(())
            }
            Err(HubError::Hub(message)) => {
                // The site no longer has this session (deleted, or ended while the meter was away): a record still
                // going on gets a new session on the next tick, sent in full, instead of failing on the old one until Stop.
                let gone = message.contains("Session not found.") || message.contains("This session has ended.");
                state.compactor.reset();
                if gone && status != "completed" {
                    state.record_id = None;
                    state.session = None;
                    state.last_published = None;
                    state.finished_record = None;
                    state.published_start = None;
                } else if let Some(session) = state.session.as_mut().filter(|session| session.id == live.id) {
                    session.state = "error".into();
                    session.error = Some(message);
                }
                if gone && status == "completed" {
                    state.finished_record = Some(id);
                }
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn finish_previous(&self) -> LabResult<()> {
        {
            let mut state = self.state();
            let state = &mut *state;
            if let (Some(previous), Some(last)) = (state.session.as_ref(), state.last_published.as_ref())
                && !previous.id.is_empty()
                && previous.state != "completed"
                && state.finished_record != state.record_id
                && let Some(delta) = state.compactor.next("completed", last.started_at, last.self_id.as_deref(), &last.record, &last.encounters, &last.dungeons, None)
            {
                state.pending_finish = Some(PendingFinish { id: previous.id.clone(), delta, attempts: 0 });
            }
            state.finished_record = state.record_id.take();
            state.session = None;
            state.last_published = None;
            state.compactor = SessionCompactor::new();
        }
        self.send_pending_finish().await
    }

    /// Sends the waiting "completed", keeping it for the next tick when the hub is not there, for about two minutes.
    async fn send_pending_finish(&self) -> LabResult<()> {
        let (pending, token) = {
            let state = self.state();
            (state.pending_finish.as_ref().map(|finish| (finish.id.clone(), finish.delta.clone())), state.settings.device_token.clone())
        };
        let (Some((id, delta)), Some(_)) = (pending, token) else { return Ok(()) };
        let sent = match self.hub().await {
            Ok(Some(connection)) => connection.invoke::<i64>("Publish", (&id, &delta)).await.map(|_| ()),
            Ok(None) => Err(HubError::NotConnected),
            Err(LabError::Hub(error)) => Err(error),
            Err(LabError::Http(message)) => Err(HubError::Http(message)),
            Err(LabError::Cancelled) => return Err(LabError::Cancelled),
        };
        let mut state = self.state();
        match sent {
            Ok(()) | Err(HubError::Hub(_)) => state.pending_finish = None,
            Err(_) => {
                if let Some(finish) = state.pending_finish.as_mut().filter(|finish| finish.id == id) {
                    finish.attempts += 1;
                    if finish.attempts > 120 {
                        state.pending_finish = None;
                    }
                }
            }
        }
        Ok(())
    }

    async fn create_session(&self, token: &str, record_id: &str, started: DateTimeOffset) -> LabResult<()> {
        let (visibility, anonymize) = {
            let state = self.state();
            (state.settings.visibility.clone(), state.settings.anonymize)
        };
        let response = self
            .inner
            .http
            .post(format!("{}/api/sessions", self.inner.server))
            .bearer_auth(token)
            .json(&json!({"startedAt": started, "visibility": visibility, "anonymize": anonymize}))
            .send()
            .await?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            self.disconnect().await;
            self.state().pairing_error = Some("revoked".into());
            return Ok(());
        }
        let body: Option<Value> = response.json().await.ok();
        if !status.is_success() {
            let reason = body.as_ref().and_then(|body| body.get("error")).and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("AlbionLab answered {}.", status.as_u16()));
            let mut state = self.state();
            state.session = Some(LabSession { id: String::new(), url: String::new(), visibility: "unlisted".into(), anonymize: false, state: "error".into(), error: Some(reason), published_at: None, title: None });
            state.failed_creates += 1;
            let wait = (5 * state.failed_creates as i64).min(60);
            let now = DateTimeOffset::now();
            state.next_create = DateTimeOffset { utc_ticks: now.utc_ticks + wait * 10_000_000, ..now };
            return Ok(());
        }
        let body = body.ok_or_else(|| LabError::Http("AlbionLab answered without a session.".into()))?;
        let mut state = self.state();
        state.record_id = Some(record_id.to_owned());
        state.failed_creates = 0;
        state.next_create = DateTimeOffset::default();
        state.compactor = SessionCompactor::new();
        state.session = Some(LabSession {
            id: text(&body, "id"),
            url: text(&body, "url"),
            visibility: text(&body, "visibility"),
            anonymize: body.get("anonymize").and_then(Value::as_bool).unwrap_or_default(),
            state: "live".into(),
            error: None,
            published_at: None,
            title: None,
        });
        Ok(())
    }

    /// The publish hub, started when it is not connected; none while it reconnects or after the token was revoked.
    async fn hub(&self) -> LabResult<Option<HubConnection>> {
        let connection = {
            let mut state = self.state();
            if state.hub.is_none() {
                let tokens: Weak<Inner> = Arc::downgrade(&self.inner);
                let resets = tokens.clone();
                let hub = HubConnection::new(format!("{}{}", self.inner.server, protocol::HUB_PATH_PUBLISH), self.inner.http.clone(), move || {
                    tokens.upgrade().and_then(|inner| inner.state.lock().ok().and_then(|state| state.settings.device_token.clone()))
                });
                hub.on_reconnected(move || {
                    if let Some(inner) = resets.upgrade()
                        && let Ok(mut state) = inner.state.lock()
                    {
                        state.compactor.reset();
                    }
                });
                state.hub = Some(hub);
            }
            state.hub.clone().expect("created above")
        };
        if connection.state() == HubState::Disconnected {
            if DateTimeOffset::now() < self.state().next_hub {
                return Ok(None);
            }
            match connection.start().await {
                Ok(()) => {
                    let mut state = self.state();
                    state.compactor.reset();
                    state.failed_hubs = 0;
                }
                Err(HubError::Unauthorized) => {
                    self.disconnect().await;
                    self.state().pairing_error = Some("revoked".into());
                    return Ok(None);
                }
                Err(error) => {
                    let mut state = self.state();
                    state.failed_hubs += 1;
                    let wait = (5i64 << (state.failed_hubs - 1).min(4)).min(60);
                    let now = DateTimeOffset::now();
                    state.next_hub = DateTimeOffset { utc_ticks: now.utc_ticks + wait * 10_000_000, ..now };
                    return Err(error.into());
                }
            }
        }
        Ok((connection.state() == HubState::Connected).then_some(connection))
    }

    fn source(&self, snapshot: &CombatSnapshot, fallback: &HashMap<String, Fallback>) -> SourceSnapshot {
        let context = &self.inner.context;
        let members = snapshot
            .members
            .iter()
            .map(|player| self.member(player, &player.membership, fallback))
            .chain(snapshot.former_members.iter().flatten().map(|player| self.member(player, "left", fallback)))
            .collect();
        let weapons = snapshot
            .weapon_contributions
            .iter()
            .flatten()
            .map(|weapon| SourceWeapon {
                id: weapon.id.clone(),
                player_id: weapon.player_id.clone(),
                index: weapon.weapon_index,
                name: weapon.weapon.clone(),
                unique_id: weapon.weapon_index.and_then(|index| context.item_id(index)),
            })
            .collect();
        let items = snapshot
            .item_contributions
            .iter()
            .flatten()
            .map(|item| SourceItem {
                id: item.id.clone(),
                player_id: item.player_id.clone(),
                slot: item.slot.clone(),
                index: item.item_index,
                name: item.item.clone(),
                kind: item.kind.clone(),
                tier_observed: item.tier_observed,
                unique_id: item.item_index.and_then(|index| context.item_id(index)),
            })
            .collect();
        let timeline = snapshot
            .timeline
            .iter()
            .map(|point| SourceSecond {
                second: point.second,
                damage: amounts(&point.damage),
                healing: amounts(&point.healing),
                weapon_damage: point.weapon_damage.as_ref().map(amounts),
                weapon_healing: point.weapon_healing.as_ref().map(amounts),
                item_damage: point.item_damage.as_ref().map(amounts),
                item_healing: point.item_healing.as_ref().map(amounts),
            })
            .collect();
        SourceSnapshot {
            id: snapshot.id.clone(),
            started_at: time(snapshot.started_at),
            elapsed: snapshot.elapsed_seconds,
            status: snapshot.status.clone(),
            events: snapshot.event_count,
            members,
            weapons,
            items,
            timeline,
        }
    }

    fn member(&self, player: &PlayerStats, membership: &str, fallback: &HashMap<String, Fallback>) -> SourceMember {
        let context = &self.inner.context;
        let seen = if player.weapon == "Weapon not observed" { fallback.get(&player.id) } else { None };
        let label = seen.map_or_else(|| player.weapon.clone(), |seen| seen.item.clone());
        // A weapon identified from the abilities used still has a family; the site marks it as seen in combat by itself.
        let family = match seen {
            Some(seen) => seen.index.and_then(|index| context.weapon_family(index)).unwrap_or_else(|| player.weapon_family.clone()),
            None => player.weapon_family.clone(),
        };
        SourceMember {
            weapon_id: context.item_id_by_name(&label),
            id: player.id.clone(),
            name: player.name.clone(),
            weapon: label,
            family,
            color: player.color.clone(),
            membership: if matches!(membership, "left" | "unconfirmed") { membership.into() } else { "active".into() },
        }
    }

    #[cfg(test)]
    pub(crate) fn set_session(&self, session: Option<LabSession>) {
        self.state().session = session;
    }

    #[cfg(test)]
    pub(crate) fn settings(&self) -> Settings {
        self.state().settings.clone()
    }
}
