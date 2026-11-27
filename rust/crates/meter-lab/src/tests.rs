use crate::signalr::{HubConnection, HubError, HubState};
use crate::testing::{negotiate_then, FakeSite, Http, Invoke, Request};
use crate::{AlbionLabService, LabContext, LabSession, Settings, WeaponIconCache};
use meter_core::dungeon::DungeonRunSnapshot;
use meter_core::engine::{CombatSnapshot, PlayerStats, TimelinePoint, Values, WeaponContribution};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!("meter-lab-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn no_hub() -> Invoke {
    Arc::new(|_| Box::pin(async { None }))
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not reached in 5 s");
}

// ---- SignalR ----

fn hub_answers() -> Invoke {
    Arc::new(|message: Value| {
        Box::pin(async move {
            match message["target"].as_str() {
                Some("Publish") => Some(Ok(json!(message["arguments"].as_array().map_or(0, Vec::len) + 5))),
                Some("Fail") => Some(Err("Session not found.".into())),
                _ => None,
            }
        })
    })
}

#[tokio::test]
async fn signalr_negotiates_handshakes_invokes_and_completes() {
    let site = FakeSite::start(negotiate_then(|_| (404, "{}".into())), hub_answers()).await;
    let hub = HubConnection::new(format!("{}/hubs/publish", site.address), reqwest::Client::new(), || Some("device".into()));
    assert_eq!(hub.state(), HubState::Disconnected);
    assert_eq!(hub.invoke::<i64>("Publish", ("s",)).await, Err(HubError::NotConnected));
    hub.start().await.unwrap();
    assert_eq!(hub.state(), HubState::Connected);
    {
        let seen = site.seen.lock().unwrap();
        assert_eq!(seen.requests[0].method, "POST");
        assert_eq!(seen.requests[0].path, "/hubs/publish/negotiate?negotiateVersion=1");
        assert_eq!(seen.requests[0].authorization.as_deref(), Some("Bearer device"));
        assert_eq!(seen.sockets[0], (Some("Bearer device".into()), "id=token%20one".into()));
    }
    assert_eq!(hub.invoke::<i64>("Publish", ("session", json!({"v": 1}))).await, Ok(7));
    assert_eq!(hub.invoke::<i64>("Fail", ("session",)).await, Err(HubError::Hub("Session not found.".into())));
    let frames = site.seen.lock().unwrap().frames.clone();
    let invocation: Value = serde_json::from_str(frames[1].trim_end_matches('\u{1e}')).unwrap();
    assert_eq!(invocation, json!({"type": 1, "invocationId": "0", "target": "Publish", "arguments": ["session", {"v": 1}]}));

    // Pings, client-method invocations, unknown completions and a message split over two frames are all fine.
    site.send("{\"type\":6}\u{1e}{\"type\":1,\"target\":\"Notify\",\"arguments\":[]}\u{1e}{\"type\":3,\"invocationId\":\"99\",\"result\":1}\u{1e}{\"type\":");
    site.send("6}\u{1e}");
    assert_eq!(hub.invoke::<i64>("Publish", (1, 2, 3)).await, Ok(8));
    hub.stop().await;
    assert_eq!(hub.state(), HubState::Disconnected);
    assert_eq!(hub.invoke::<i64>("Publish", ("s",)).await, Err(HubError::NotConnected));
}

#[tokio::test]
async fn signalr_reconnects_when_the_server_allows_it_and_fails_waiting_invocations() {
    let site = FakeSite::start(negotiate_then(|_| (404, "{}".into())), hub_answers()).await;
    let hub = HubConnection::new(format!("{}/hubs/publish", site.address), reqwest::Client::new(), || Some("device".into()));
    let reconnected = Arc::new(AtomicUsize::new(0));
    let count = reconnected.clone();
    hub.on_reconnected(move || {
        count.fetch_add(1, Ordering::SeqCst);
    });
    hub.start().await.unwrap();
    let waiting = tokio::spawn({
        let hub = hub.clone();
        async move { hub.invoke::<i64>("Hold", ()).await }
    });
    wait_until(|| site.seen.lock().unwrap().frames.len() >= 2).await;
    site.send("{\"type\":7,\"error\":\"restarting\",\"allowReconnect\":true}\u{1e}");
    assert!(matches!(waiting.await.unwrap(), Err(HubError::Closed(reason)) if reason.contains("restarting")));
    wait_until(|| reconnected.load(Ordering::SeqCst) == 1).await;
    assert_eq!(hub.state(), HubState::Connected);
    assert_eq!(site.seen.lock().unwrap().sockets.len(), 2);
    assert_eq!(hub.invoke::<i64>("Publish", ("again",)).await, Ok(6));

    // A WebSocket that drops is reconnected too; a close that does not allow it is final.
    site.send("<close>");
    wait_until(|| reconnected.load(Ordering::SeqCst) == 2).await;
    site.send("{\"type\":7,\"error\":\"bye\"}\u{1e}");
    wait_until(|| hub.state() == HubState::Disconnected).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(site.seen.lock().unwrap().sockets.len(), 3);
    // It can be started again.
    hub.start().await.unwrap();
    assert_eq!(hub.invoke::<i64>("Publish", ()).await, Ok(5));
}

#[tokio::test]
async fn signalr_reports_a_rejected_token() {
    let http: Http = Arc::new(|_| Box::pin(async { (401, Vec::new()) }));
    let site = FakeSite::start(http, no_hub()).await;
    let hub = HubConnection::new(format!("{}/hubs/publish", site.address), reqwest::Client::new(), || Some("old".into()));
    assert_eq!(hub.start().await, Err(HubError::Unauthorized));
    assert_eq!(hub.state(), HubState::Disconnected);
}

// ---- AlbionLab service ----

struct Meter {
    record: Mutex<Arc<CombatSnapshot>>,
}

impl LabContext for Meter {
    fn record(&self) -> Arc<CombatSnapshot> {
        self.record.lock().unwrap().clone()
    }
    fn dungeons(&self) -> Vec<Arc<DungeonRunSnapshot>> {
        vec![]
    }
    fn character(&self) -> Option<(String, Option<String>)> {
        Some(("a".into(), Some("1301".into())))
    }
    fn item_id(&self, index: i32) -> Option<String> {
        (index == 9022).then(|| "T6_2H_HAMMER".into())
    }
    fn item_id_by_name(&self, name: &str) -> Option<String> {
        (name == "Great Hammer").then(|| "T6_2H_HAMMER".into())
    }
    fn weapon_family(&self, _: i32) -> Option<String> {
        None
    }
}

fn values(entries: &[(&str, f64)]) -> Values {
    let mut values = Values::new();
    for (key, value) in entries {
        values.insert((*key).into(), *value);
    }
    values
}

/// A live record of one player hitting once with a Great Hammer (weapon 9022).
fn live_record(status: &str) -> CombatSnapshot {
    let point = TimelinePoint {
        second: 0,
        damage: values(&[("a", 10.0)]),
        healing: Values::new(),
        weapon_damage: Some(values(&[("a:9022", 10.0)])),
        weapon_healing: None,
        item_damage: None,
        item_healing: None,
    };
    CombatSnapshot {
        id: "rec-1".into(),
        started_at: meter_core::cs::DateTimeOffset::from_unix_milliseconds(meter_store::DateTimeOffset::parse("2026-11-20T10:00:00+00:00").unwrap().unix_microseconds() / 1000),
        elapsed_seconds: 2.0,
        status: status.into(),
        mode: "live".into(),
        sequence: 1,
        event_count: 1,
        total_damage: 10.0,
        total_healing: 0.0,
        members: vec![PlayerStats {
            id: "a".into(),
            name: "Alpha".into(),
            weapon: "Great Hammer".into(),
            weapon_family: "hammer".into(),
            color: "#ffffff".into(),
            damage: 10.0,
            healing: 0.0,
            encounter_dps: 5.0,
            encounter_hps: 0.0,
            rolling_dps: 5.0,
            rolling_hps: 0.0,
            damage_share: 100.0,
            healing_share: 0.0,
            membership: "active".into(),
        }],
        timeline: vec![Arc::new(point)],
        full: true,
        location: None,
        former_members: None,
        party_id: None,
        weapon_contributions: Some(vec![WeaponContribution {
            id: "a:9022".into(),
            player_id: "a".into(),
            player_name: "Alpha".into(),
            weapon_index: Some(9022),
            weapon: "Great Hammer".into(),
            color: "#ffffff".into(),
            damage: 10.0,
            healing: 0.0,
        }]),
        item_contributions: None,
    }
}

fn service(directory: &Path, site: &FakeSite, record: CombatSnapshot) -> AlbionLabService {
    AlbionLabService::new(directory, Some(&site.address), Arc::new(Meter { record: Mutex::new(Arc::new(record)) }))
}

fn session(id: &str) -> LabSession {
    LabSession { id: id.into(), url: format!("https://albionlab.app/s/{id}"), visibility: "unlisted".into(), anonymize: false, state: "live".into(), error: None, published_at: None, title: None }
}

#[test]
fn settings_written_by_csharp_round_trip_unchanged() {
    let directory = TempDir::new("settings");
    let path = directory.path().join("albionlab.json");
    let csharp = r#"{"deviceToken":"device","accountName":"Thalys M\u00E1rcio","avatarUrl":"https://cdn.discordapp.com/avatars/1/2.png?size=128","shareLocation":false,"visibility":"public","anonymize":false}"#;
    std::fs::write(&path, csharp).unwrap();
    let settings = Settings::load(&path);
    assert_eq!(settings.account_name.as_deref(), Some("Thalys Márcio"));
    settings.save(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), csharp);
    #[cfg(unix)]
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions()) & 0o777, 0o600);
    std::fs::write(&path, r#"{"deviceToken":"device"}"#).unwrap();
    assert_eq!(Settings::load(&path), Settings { device_token: Some("device".into()), ..Settings::default() });
    std::fs::write(&path, "not json").unwrap();
    assert_eq!(Settings::load(&path), Settings::default());
}

#[tokio::test]
async fn settings_changed_while_the_loop_reads_them_are_kept() {
    let site = FakeSite::start(negotiate_then(|_| (404, "{}".into())), no_hub()).await;
    for _ in 0..50 {
        let directory = TempDir::new("settings-race");
        std::fs::write(directory.path().join("albionlab.json"), r#"{"deviceToken":"device","shareLocation":false,"visibility":"unlisted","anonymize":false}"#).unwrap();
        let lab = service(directory.path(), &site, live_record("idle"));
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let threads: Vec<_> = (0..3)
            .map(|index| {
                let (lab, barrier) = (lab.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    match index {
                        0 => assert!(lab.share_location(true).share_location),
                        _ => assert!(lab.status().connected),
                    }
                })
            })
            .collect();
        threads.into_iter().for_each(|thread| thread.join().unwrap());
        assert!(lab.status().share_location && lab.status().connected);
        assert!(std::fs::read_to_string(directory.path().join("albionlab.json")).unwrap().contains("\"shareLocation\":true"));
    }
}

#[tokio::test]
async fn a_visibility_change_answered_after_the_session_was_replaced_does_not_bring_it_back() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (entered_site, release_site) = (entered.clone(), release.clone());
    let http: Http = Arc::new(move |_| {
        let (entered, release) = (entered_site.clone(), release_site.clone());
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
            (200, json!({"visibility": "public", "anonymize": true, "title": "Renamed"}).to_string().into_bytes())
        })
    });
    let site = FakeSite::start(http, no_hub()).await;
    let directory = TempDir::new("patch");
    std::fs::write(directory.path().join("albionlab.json"), r#"{"deviceToken":"device"}"#).unwrap();
    let lab = service(directory.path(), &site, live_record("idle"));
    lab.set_session(Some(session("first")));
    let update = tokio::spawn({
        let lab = lab.clone();
        async move { lab.update_session(Some("Renamed"), Some("public"), Some(true)).await.unwrap() }
    });
    entered.notified().await;
    // Meanwhile the publish loop finished the first session and created one for the next record.
    lab.set_session(Some(session("second")));
    release.notify_one();
    let result = update.await.unwrap();
    assert_eq!(site.seen.lock().unwrap().requests.iter().map(|request| format!("{} {}", request.method, request.path)).collect::<Vec<_>>(), ["PATCH /api/sessions/first"]);
    assert_eq!(result.session, Some(session("second")));
    assert_eq!(lab.settings().visibility, "public");

    // Without a replacement the answer is applied to the session it was sent for.
    lab.set_session(Some(session("first")));
    let update = tokio::spawn({
        let lab = lab.clone();
        async move { lab.update_session(Some("Renamed"), Some("public"), Some(true)).await.unwrap() }
    });
    entered.notified().await;
    release.notify_one();
    let applied = update.await.unwrap().session.unwrap();
    assert_eq!((applied.id.as_str(), applied.visibility.as_str(), applied.anonymize, applied.title.as_deref()), ("first", "public", true, Some("Renamed")));
}

fn sessions_site(rest: impl Fn(Request) -> (u16, String) + Send + Sync + 'static) -> Http {
    negotiate_then(move |request| {
        if request.method == "POST" && request.path == "/api/sessions" {
            (200, json!({"id": "live", "url": "https://albionlab.app/s/live", "visibility": "unlisted", "anonymize": false}).to_string())
        } else {
            rest(request)
        }
    })
}

#[tokio::test]
async fn a_record_published_while_its_session_is_disconnected_does_not_bring_the_session_back() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let published = Arc::new(Mutex::new(Vec::new()));
    let (entered_hub, release_hub, seen) = (entered.clone(), release.clone(), published.clone());
    let invoke: Invoke = Arc::new(move |message| {
        let (entered, release, seen) = (entered_hub.clone(), release_hub.clone(), seen.clone());
        Box::pin(async move {
            seen.lock().unwrap().push(message["arguments"].clone());
            entered.notify_one();
            release.notified().await;
            Some(Ok(json!(1)))
        })
    });
    let site = FakeSite::start(sessions_site(|_| (404, "{}".into())), invoke).await;
    let directory = TempDir::new("publish");
    std::fs::write(directory.path().join("albionlab.json"), r#"{"deviceToken":"device"}"#).unwrap();
    let lab = service(directory.path(), &site, live_record("running"));
    let cancel = crate::CancellationToken::new();
    let publish = tokio::spawn({
        let (lab, cancel) = (lab.clone(), cancel.clone());
        async move { lab.publish(&cancel).await }
    });
    entered.notified().await;
    assert_eq!(lab.status().session.map(|session| session.id), Some("live".into()));
    // Disconnect clears the session under the lock; the hub answers the publish after that.
    lab.set_session(None);
    release.notify_one();
    publish.await.unwrap().unwrap();
    assert!(lab.status().session.is_none());

    let arguments = published.lock().unwrap()[0].clone();
    assert_eq!(arguments[0], "live");
    let delta = &arguments[1];
    assert_eq!((delta["reset"].as_bool(), delta["status"].as_str(), delta["self"].as_str(), delta["record"].as_str()), (Some(true), Some("running"), Some("a"), Some("rec-1")));
    assert_eq!(delta["players"]["a"]["weaponId"], "T6_2H_HAMMER");
    assert_eq!(delta["keys"]["1"], json!({"t": "w", "p": "a", "x": 9022, "n": "Great Hammer", "u": "T6_2H_HAMMER"}));
    assert_eq!(delta["places"]["2"], crate::place(Some("1301")).unwrap());
    assert_eq!(crate::place(Some("1301")).unwrap().len(), 24);
}

#[tokio::test]
async fn a_session_the_site_no_longer_has_is_replaced_and_a_failed_create_waits() {
    let invoke: Invoke = Arc::new(|_| Box::pin(async { Some(Err("Session not found.".into())) }));
    let site = FakeSite::start(sessions_site(|_| (404, "{}".into())), invoke).await;
    let directory = TempDir::new("gone");
    std::fs::write(directory.path().join("albionlab.json"), r#"{"deviceToken":"device"}"#).unwrap();
    let lab = service(directory.path(), &site, live_record("running"));
    let cancel = crate::CancellationToken::new();
    lab.publish(&cancel).await.unwrap();
    assert!(lab.status().session.is_none());
    lab.publish(&cancel).await.unwrap();
    let creates = site.seen.lock().unwrap().requests.iter().filter(|request| request.path == "/api/sessions").count();
    assert_eq!(creates, 2);

    let failing: Http = negotiate_then(|_| (500, json!({"error": "The site is down."}).to_string()));
    let down = FakeSite::start(failing, no_hub()).await;
    let lab = service(directory.path(), &down, live_record("running"));
    lab.publish(&cancel).await.unwrap();
    let session = lab.status().session.unwrap();
    assert_eq!((session.state.as_str(), session.error.as_deref()), ("error", Some("The site is down.")));
    lab.publish(&cancel).await.unwrap();
    assert_eq!(down.seen.lock().unwrap().requests.len(), 1, "a failed create waits before the next try");
}

#[tokio::test]
async fn a_revoked_device_token_disconnects() {
    let http: Http = Arc::new(|request: Request| {
        Box::pin(async move {
            if request.path == "/api/sessions" {
                (200, json!({"id": "live", "url": "u", "visibility": "unlisted", "anonymize": false}).to_string().into_bytes())
            } else {
                (401, Vec::new())
            }
        })
    });
    let site = FakeSite::start(http, no_hub()).await;
    let directory = TempDir::new("revoked");
    std::fs::write(directory.path().join("albionlab.json"), r#"{"deviceToken":"device","accountName":"A"}"#).unwrap();
    let lab = service(directory.path(), &site, live_record("running"));
    lab.publish(&crate::CancellationToken::new()).await.unwrap();
    let status = lab.status();
    assert!(!status.connected && status.session.is_none());
    assert_eq!(status.pairing_error.as_deref(), Some("revoked"));
    assert_eq!(Settings::load(&directory.path().join("albionlab.json")).device_token, None);
}

#[tokio::test]
async fn pairing_gets_a_code_and_then_the_device_token() {
    let http: Http = Arc::new(|request: Request| {
        Box::pin(async move {
            match request.path.as_str() {
                "/api/devices/pair" => (200, json!({"pollToken": "poll", "code": "AB 12", "verifyUrl": "https://evil.example/link", "expiresIn": 300}).to_string().into_bytes()),
                "/api/devices/poll" => (200, json!({"deviceToken": "device", "account": {"name": "KPey", "avatarUrl": "https://cdn.discordapp.com/a.png"}}).to_string().into_bytes()),
                _ => (404, b"{}".to_vec()),
            }
        })
    });
    let site = FakeSite::start(http, no_hub()).await;
    let directory = TempDir::new("pairing");
    let lab = service(directory.path(), &site, live_record("idle"));
    let status = lab.connect().await.unwrap();
    let pairing = status.pairing.unwrap();
    assert_eq!(pairing.verify_url, format!("{}/link?code=AB%2012", site.address));
    assert!(site.seen.lock().unwrap().requests[0].body.contains("Albion Meter \u{b7} "));
    for _ in 0..40 {
        lab.poll_pairing().await.unwrap();
        if lab.status().connected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let status = lab.status();
    assert!(status.connected && status.pairing.is_none());
    assert_eq!(status.account.unwrap().name, "KPey");
    assert_eq!(Settings::load(&directory.path().join("albionlab.json")).device_token.as_deref(), Some("device"));
}

#[test]
fn the_server_must_be_https_or_this_machine() {
    let lab = |url: &str| AlbionLabService::new(Path::new("/nonexistent"), Some(url), Arc::new(Meter { record: Mutex::new(Arc::new(live_record("idle"))) })).server().to_owned();
    assert_eq!(lab("http://127.0.0.1:5000/"), "http://127.0.0.1:5000");
    assert_eq!(lab("https://staging.albionlab.app/path"), "https://staging.albionlab.app");
    assert_eq!(lab("http://albionlab.app"), "https://albionlab.app");
    assert_eq!(lab("ftp://x"), "https://albionlab.app");
}

// ---- Icons ----

const PNG: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82];

fn png_site(downloads: Arc<Mutex<HashMap<String, usize>>>, fail: bool, overlap: Arc<(AtomicUsize, AtomicUsize)>) -> Http {
    Arc::new(move |request: Request| {
        let (downloads, overlap) = (downloads.clone(), overlap.clone());
        Box::pin(async move {
            let file = request.path.rsplit('/').next().unwrap_or_default();
            let id = file.split(".png").next().unwrap_or_default().to_owned();
            *downloads.lock().unwrap().entry(id).or_default() += 1;
            if overlap.0.fetch_add(1, Ordering::SeqCst) > 0 {
                overlap.1.fetch_add(1, Ordering::SeqCst);
            }
            tokio::time::sleep(Duration::from_millis(if fail { 1 } else { 30 })).await;
            overlap.0.fetch_sub(1, Ordering::SeqCst);
            if fail { (502, Vec::new()) } else { (200, PNG.to_vec()) }
        })
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_for_one_icon_download_it_once() {
    let downloads = Arc::new(Mutex::new(HashMap::new()));
    let site = FakeSite::start(png_site(downloads.clone(), false, Arc::default()), no_hub()).await;
    let directory = TempDir::new("icons");
    let ids = ["T4_MAIN_SWORD", "T5_2H_HOLYSTAFF", "T6_MAIN_FROSTSTAFF"];
    let cache = Arc::new(WeaponIconCache::with_service(directory.path(), &site.address, move |id| ids.contains(&id)));
    let tasks: Vec<_> = (0..96)
        .map(|index| {
            let cache = cache.clone();
            tokio::spawn(async move { cache.get(ids[index % ids.len()]).await })
        })
        .collect();
    for task in tasks {
        assert_eq!(task.await.unwrap().expect("icon").as_slice(), PNG);
    }
    let downloads = downloads.lock().unwrap().clone();
    assert!(ids.iter().all(|id| downloads.get(*id) == Some(&1)), "{downloads:?}");
    assert!(ids.iter().all(|id| directory.path().join("assets/weapon-icons").join(format!("{id}.png")).is_file()));
    assert!(cache.get("../etc/passwd").await.is_none() && cache.get("T8_UNKNOWN").await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_icon_that_keeps_failing_is_never_downloaded_twice_at_once() {
    let downloads = Arc::new(Mutex::new(HashMap::new()));
    let overlap: Arc<(AtomicUsize, AtomicUsize)> = Arc::default();
    let site = FakeSite::start(png_site(downloads.clone(), true, overlap.clone()), no_hub()).await;
    let directory = TempDir::new("icons-failing");
    let cache = Arc::new(WeaponIconCache::with_service(directory.path(), &site.address, |id| id == "T4_MAIN_SWORD"));
    let until = tokio::time::Instant::now() + Duration::from_millis(600);
    let tasks: Vec<_> = (0..32)
        .map(|_| {
            let cache = cache.clone();
            tokio::spawn(async move {
                while tokio::time::Instant::now() < until {
                    assert!(cache.get("T4_MAIN_SWORD").await.is_none());
                }
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap();
    }
    let total = downloads.lock().unwrap().get("T4_MAIN_SWORD").copied().unwrap_or_default();
    assert_eq!(total, 3, "each size is tried once, then the failure is remembered: {total}");
    assert_eq!(overlap.1.load(Ordering::SeqCst), 0, "{} of {total} downloads overlapped another", overlap.1.load(Ordering::SeqCst));
    assert!(!directory.path().join("assets/weapon-icons/T4_MAIN_SWORD.png").exists());
}

#[tokio::test]
async fn an_icon_is_tried_at_each_size_and_then_read_from_disk() {
    let http: Http = Arc::new(|request: Request| Box::pin(async move { if request.path.ends_with("size=128") { (502, Vec::new()) } else { (200, PNG.to_vec()) } }));
    let site = FakeSite::start(http, no_hub()).await;
    let directory = TempDir::new("icons-sizes");
    let cache = WeaponIconCache::with_service(directory.path(), &site.address, |_| true);
    assert_eq!(cache.get("T4_MAIN_SWORD@1").await.unwrap().as_slice(), PNG);
    assert_eq!(site.seen.lock().unwrap().requests.iter().map(|request| request.path.clone()).collect::<Vec<_>>(), ["/v1/item/T4_MAIN_SWORD%401.png?size=128", "/v1/item/T4_MAIN_SWORD%401.png"]);
    assert_eq!(cache.get("T4_MAIN_SWORD@1").await.unwrap().as_slice(), PNG);
    assert_eq!(site.seen.lock().unwrap().requests.len(), 2);
}

#[tokio::test]
async fn the_publish_loop_publishes_every_second_and_ends_the_session_when_stopped() {
    let published = Arc::new(Mutex::new(Vec::new()));
    let seen = published.clone();
    let invoke: Invoke = Arc::new(move |message| {
        let seen = seen.clone();
        Box::pin(async move {
            seen.lock().unwrap().push(message["arguments"].clone());
            Some(Ok(json!(1)))
        })
    });
    let site = FakeSite::start(sessions_site(|_| (404, "{}".into())), invoke).await;
    let directory = TempDir::new("loop");
    std::fs::write(directory.path().join("albionlab.json"), r#"{"deviceToken":"device"}"#).unwrap();
    let lab = service(directory.path(), &site, live_record("running"));
    let cancel = crate::CancellationToken::new();
    let run = tokio::spawn({
        let (lab, cancel) = (lab.clone(), cancel.clone());
        async move { lab.run(cancel).await }
    });
    wait_until(|| lab.status().session.is_some_and(|session| session.published_at.is_some())).await;
    assert_eq!(lab.status().suggested_title.as_deref(), Some("Alpha solo · hammer"));
    cancel.cancel();
    run.await.unwrap();
    let published = published.lock().unwrap();
    let last = published.last().unwrap();
    assert_eq!((last[0].as_str(), last[1]["status"].as_str(), last[1]["reset"].as_bool()), (Some("live"), Some("completed"), Some(false)));
    assert!(lab.status().session.is_none());
}
