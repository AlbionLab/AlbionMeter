# meter-runtime report

Status: the runtime is done and every test passes. Every real-time gate passes in the 12-minute soak, including RAM (63 MB peak at 5×, flat) and packet → engine p99 (0.54 ms), and in the accelerated 2-hour fight (see "Gate numbers").

Commands, run from `rust/` with `CARGO_TARGET_DIR=target/rust-core`:
- `cargo clippy -p meter-runtime --all-targets -- -D warnings` → exit 0.
- `cargo test -p meter-runtime` → exit 0: 16 passed, 3 ignored. The ignored ones are the soak, the memory probe and the accelerated 2-hour fight.

## C# → Rust

| C# | Rust (`src/`) |
|---|---|
| Program.cs: storage dir (explicit > portable `data` > LocalApplicationData/AlbionMeter), `assets/combat.json`, bundled or imported `maps.json`, `Storage:Directory` / `Capture:Debug` / `Capture:AutoStart` / `AlbionLab:Url` | `config.rs` (`Config::from_environment`, `load_sources`, `load_maps`) |
| CaptureService.cs: Start/Reserve/RunLive/StopAsync/ReplayAsync, StartRecord/StopRecord/ResetRecord/SetRecordPaused, HoldRecord, RestartLater (3 s), permission errors, SaveDevice/RecordDevice, RemoveLegacyRecordings, AutoStart, Publish → CharacterContextStore | `engine.rs` |
| MeterSession.cs: ResolveLive, LiveDungeons, the Command switch with Archive, SaveCurrent, SaveLiveDungeons (5 s), SaveIdentities (15 s), Update (30 s record save, publish only on a change of sequence or status), Save | `engine.rs` + `resolve.rs` |
| MeterHub/MeterWorker loop | the engine tick (250 ms, ≤ 4 Hz) and `Runtime::serve` shutdown |
| CaptureEndpoints / CatalogEndpoints / HistoryEndpoints / `/api/dungeons*` / `/api/storage` | `Command`s handled in `engine.rs` and `services.rs` |
| CharacterContextStore wiring, EncounterStore calls | `services.rs` (`StoreJob`, `store_loop`, `open_store` seeds) |
| AlbionLabService wiring (`LabContext`) | `services.rs` (`Context`, `lab_status_loop`, `Services::lab`) + `LabView` in `lib.rs` |
| WeaponIconCache use | `Services::icon` |
| PacketCapture device/open, WindowsFirewall.AllowCapture | `backend.rs` (`CaptureBackend`, `SystemCapture`) |

### Threads

- **Capture thread.** `meter_capture::CaptureThread` (bounded channel, 4096 frames, buffers recycled).
- **Engine thread.** It alone owns both `CaptureAnalysis` instances (live and replay), the record controls, the status and the published model. There is no Mutex around meter state.
- **tokio (2 workers).** It runs:
  - the store jobs, one at a time in send order, on the blocking pool;
  - AlbionLab: the `run` loop plus a status poller;
  - icons, catalog import and the device list.

Devices open on a short-lived helper thread (the firewall rule may take seconds), and the result comes back as `Input::Opened`. Commands are serialized with frames, so the race fixes of 9d612a7 hold by construction. Every record command publishes its snapshot before `CommandDone`.

### Deliberate differences

- **Report rebuild.** The status report (counters, diagnostics, character, party, weapons) is read from `CaptureAnalysis`'s accessors at most once a second while packets arrive, and right after every command or state change; none of them grows with the record. C# rebuilt the whole report every 200 ms. The UI snapshot itself comes from `combat_delta` on every tick, the dungeon runs from `CaptureAnalysis::dungeons()` (each run an `Arc`, resolved again only when it changed) and the identities from `identities()` at their 15 s save.
- **Saves.** Encounters and runs go to the store's typed `save_encounter` / `save_dungeon_run`, straight to their JSON text.
- **Heap (Linux/glibc).** `Runtime::start` limits glibc to two heap arenas, and the store worker calls `malloc_trim(0)` after each encounter or run save. Without this, freed multi-MB save strings stayed in per-thread arenas: 154 MB RSS at 5× where the live data is about 40 MB.
- **Dungeon saves.** The 5 s save skips runs that did not change. C# rewrote every run of the capture; the resulting DB is the same.
- **Seeds.** Known characters, skill weapons and context files are read once at startup. C# read them when it created the live analysis, which happens once.
- **Not ported.** `PacketDebugStore` (debug-only packet table) and the unused `AppendControl`.
- **InstallUpdate.** It is ignored here; the updater belongs to meter-app.

## Command / FeedEvent mapping

`src/feed.rs` is meter-app's `feed.rs`, type for type. Proposal: replace meter-app's copy with `pub use meter_runtime::feed::*;`. `Runtime` implements `MeterFeed` directly, so no adapter is needed: `Box::new(meter_runtime::Runtime::new(Config::from_environment(version, &args)))`.

### Commands

| Command | Runtime |
|---|---|
| StartRecord / PauseRecord / ResumeRecord / NewRecord (Reset) / StopRecord | `MeterSession.Command` `start-record` / `pause-record` / `resume-record` / `new-record` / `stop-record` → Snapshot, Capture, then `CommandDone(err)` with the C# messages |
| StartCapture{device, method} | endpoint validation + `Start` → `CaptureDone`, Capture |
| StopCapture | `StopAsync` → Capture "stopping", then `CaptureDone(None)` once the worker ended |
| Replay(path) | 40 MB check, `ReplayAsync` → `CaptureDone` at the end of the file, with the error if it failed |
| RefreshDevices | `/api/capture/devices` → Devices (selected = the device in use, else the default) |
| LabConnect / LabTitle / LabVisibility / LabAnonymize / LabShareLocation / LabDisconnect | AlbionLabService with the endpoint checks → `LabError(..)` on failure ("AlbionLab is unreachable: …", "The name is limited to 80 characters.", "Unknown visibility."); Lab is published whenever the status changes |
| ImportCatalog(folder) | finds `GameData/cluster/world.bin`, `items.bin`, `localization.bin` and `version.txt` as Library.tsx does → Catalog + `CatalogMessage("imported:M:W")` or the error text |
| OpenDungeon(id) | the live run, else `store.GetDungeon` → Dungeon |
| ExportHistory{dungeon, to} | `session.Save()` (or saves the live run) then writes the ZIP → `HistoryMessage` |
| ImportHistory(path) | → `HistoryMessage` with the web UI's text, then Dungeons |
| Icon(id) | `WeaponIconCache.get` → `Icon(id, assets/weapon-icons/id.png)` |
| InstallUpdate | ignored (meter-app) |

### Events

| Event | When it is sent |
|---|---|
| Version, StoragePath (encounters.db path), Catalog, Devices, Dungeons, Lab, Capture, Snapshot (unavailable), Connection(Connected) | at start |
| Snapshot | ≤ 4 Hz and only when sequence or status changed. Timeline points are only the changed ones unless `full`; members and item contributions come complete, from cached rows updated by the delta |
| Capture | on every state change and at ≤ 1 Hz with the report counters |
| Dungeons | when the list changed |
| Dungeon | the open live run, when its report changed |

Not produced: `Update` (meter-app owns it). The account avatar bytes are downloaded here.

## Tests

- **`replay.rs`: the end-to-end test.**
  - The four synthetic pcaps (solo, party, transport, stress-30min) are replayed through the Runtime with `Command::Replay`. The saved encounter is compared with what the real C# `CaptureService.ReplayAsync` + `MeterSession.Save` wrote to an EncounterStore (fixtures from a throwaway dotnet harness outside the repo, with the same combat.json and maps.json).
  - solo, party and transport match **byte for byte**.
  - stress-30min matches as exact doubles (canonical SHA-256). Byte for byte it differs in 518 numbers; see "Cross-crate requests", item 1.
  - Also checked: the Capture/Snapshot events, and that the timeline rebuilt from deltas equals the saved one.
- **`session.rs`: ManualRecordingChecks through the runtime.**
  - pre-Play hits, Pause/Resume, Stop with its archive, the new record and both dungeon runs in the store;
  - the clock running and frozen;
  - Reset;
  - the C# error texts and capture-settings.json;
  - restart naming the players from known-players.json.
- **MembershipGapChecks are not ported here.** They exercise only `DungeonCombatObservation`, with no service or session logic; meter-core ports them 7/7.
- **`reconnect.rs`: hold and resume with scripted sources.**
  - a drop, then reconnect after the delay and resume with the same record and players;
  - Pause/Resume during the reconnect;
  - Stop during the wait (no reopen);
  - a read failure → hold → reopen fails on permission → the record ends and there is no retry;
  - Play without permission;
  - StopCapture ends the record.
- **`gates.rs`: latency through the runtime.** The stress-30min raid is played live at 60× with wall-clock stamps.

## Gate numbers (release, this machine, 16 cores, load average 3–4 from the other agents)

"Before" is 76a5033. "After" is this change: meter-core's accessors, bounded record and shared dungeon runs, plus the runtime switched to them, the typed store saves and the heap settings.

### Soak

Command: `SOAK_MINUTES=12 cargo test -p meter-runtime --release -- --ignored soak --nocapture`. Phases: 60 s idle, 120 s at 400 packets/s (a real 20-player fight), 540 s at 2000 packets/s (5×). 1,127,996 frames were sent and all were taken.

| Gate | Limit | Before | After |
|---|---|---|---|
| Packet → engine p99 | < 2 ms | 14.8 ms (max 86 ms) — FAIL | **0.54 ms** (p50 0.06, p99.9 4.0, max 16.7 ms) — pass |
| Packet → publish p99 | < 300 ms | 295 ms (max 336) — pass | 262 ms (max 263) — pass |
| Dropped game packets at 5× | 0 | 0 — pass | 0 — pass |
| RAM idle | ≤ 60 MB | 31.7 MB — pass | 30.9 MB — pass |
| RAM fight (peak) | ≤ 90 MB | 71.6 MB at 1×; 330 MB at 5× — FAIL | **41.9 MB at 1×; 63.1 MB at 5×**, flat (+0.8 MB over the second half) — pass |
| CPU idle | < 1 % | 0.23 % — pass | 0.27 % — pass |
| CPU 20-player fight | < 5 % | 3.19 % at 1× — pass | 4.10 % at 1× (17.9 % at 5×, traffic generator included) — pass |

Status-report rebuild on the engine thread: before, p50 grew from 0.8 to 21 ms (p99 61 ms); after, **p50 0.047 ms, p99 0.10 ms, max 0.14 ms**, flat.

How the RAM fix was found:
- With the meter-core changes alone, RSS at 5× was 154 MB.
- With `MALLOC_ARENA_MAX=2` set outside the process, RSS stayed at 30–60 MB.
- Setting the arena limit in-process (the test harness has already made arenas) gave 111 MB; adding `malloc_trim` after saves gave 63 MB.

### Replay latency test

On stress-30min at 60×:

| | Before | After |
|---|---|---|
| Packet → engine p99 | 1.5 ms (max 24 ms) | **0.43 ms** (p99.9 5.6 ms, max 18 ms) |
| Packet → publish p99 | 268 ms | 253 ms |
| Dropped frames | 0 | 0 |

### Two hours at 400 packets/s, accelerated

New test: `two_hours_of_fight_stay_within_the_ram_and_tick_gates`, run with `cargo test -p meter-runtime --release --test gates -- --ignored two_hours --nocapture`.

It plays 2,880,000 frames, 7,200 s of game time, into one `CaptureAnalysis` in about 30 s. That reaches the 1,000,000-event and 7,200-bucket caps. On game time it does what the engine does:
- every 250 ms: the UI delta;
- every second: the status;
- every 5 s: the dungeon runs;
- every 15 s: the identities;
- every 30 s: the full record.

| | Limit | First version of this change | Final |
|---|---|---|---|
| Peak RSS | ≤ 90 MB | 87.4 MB | **66.5 MB** (first hour 54.2, second hour 66.5; it stops growing at the event cap) |
| Engine tick p99 | < 2 ms | 3.3 ms (the 5 s dungeon snapshot) | **1.58 ms** (p50 0.35 ms; minutes 1–21 p99 1.08 ms, minutes 100–120 p99 1.04 ms) |

`memory_probe` (230k frames at 5×): 26 → 70.9 MB before, 26 → 29.6 MB after.

### What the record kept, and what it keeps now (meter-core)

The C# rules are unchanged, and the output is byte-identical: `diff-replay.sh --synthetic` matches, and so does `the_stress_capture_is_saved_byte_for_byte`, which is no longer ignored.

- **Pending unsourced events** (`_unsourced`, up to 8,192 per player in each of the two engines).
  - Before: the counted event plus its effect, about 200 B.
  - Now: 48 B. The id is dropped; C#'s "is the id in the engine's set" check becomes "was it counted by this engine" (engine serial). Slot and kind are stored as 1-byte codes.
- **Event ids** (C#'s all-time `HashSet<string>`).
  - Before: about 50 B per event.
  - Now: an exact set. Each Photon (connection, channel, type, index) keeps a bitmap over command numbers, 2.88M commands in 360 KB. Anything else goes to an ordinary set.
- **Timeline buckets.**
  - Damage and healing are dense per-member values sharing one key list per record.
  - Buckets two seconds old are frozen: weapon and item maps become exact `(key id, value)` slices with ids from a process-wide key table. They are frozen by copy-on-write, because the dungeon checkpoint always shares them.
  - The live map is boxed.
- **Dungeon runs.**
  - `snapshot_shared` returns one `Arc` per run, rebuilt only when the run's record moved.
  - A total over one encounter is that encounter's points.
  - Several encounters are merged in one ordered pass, with each encounter's shifted points cached.

## Cross-crate requests

1. ~~meter-store `json::format_double`~~ (done in 5a54692).
2. ~~meter-core cheap accessors and `Arc` dungeon runs~~ (done).
3. ~~meter-core record memory~~ (done).
4. ~~meter-store typed saves~~ (done in 5a54692; the runtime now uses them).
5. **meter-app.** Re-export the runtime's `feed` (above), run `Runtime` as its `MeterFeed`, and keep `InstallUpdate`/`Update` on its own side.
6. **Workspace.** No Cargo.toml changes outside this crate. Dependencies added to meter-runtime: crossbeam-channel, tokio, reqwest (rustls, for avatars), uuid, libc (Linux/glibc only, for the heap settings); dev: sha2. `rust/Cargo.lock` changed accordingly.
