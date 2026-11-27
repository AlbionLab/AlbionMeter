# meter-runtime report

Status: the runtime is done and every test passes. Two real-time gates fail in the 10-minute soak at 5× traffic: RAM and packet → engine p99. Both are caused by work that grows with the size of the record (details below).

Commands, run from `rust/` with `CARGO_TARGET_DIR=target/rust-runtime`:
- `cargo clippy -p meter-runtime --all-targets -- -D warnings` → exit 0.
- `cargo test -p meter-runtime` → exit 0: 15 passed, 4 ignored. The ignored ones are the soak, a memory probe and one byte-exact check (see "Cross-crate requests", item 1).

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

- **Report rebuild.** The full `CaptureAnalysis::snapshot()` is rebuilt at most once a second while packets arrive, and right after every command or state change. C# rebuilt it every 200 ms. The UI snapshot itself comes from `combat_delta` on every tick.
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

### Replay latency test

On stress-30min at 60×:
- packet → engine: p50 0.10 ms, **p99 1.5 ms**, p99.9 13 ms, max 24 ms;
- packet → publish: **p99 268 ms**;
- dropped frames: 0.

### Soak

Command: `SOAK_MINUTES=12 cargo test -p meter-runtime --release -- --ignored soak --nocapture`. It ran 12 minutes in three phases: 60 s idle with the capture on, 120 s at a real 20-player fight (400 packets/s), then 540 s at 5× (2000 packets/s). It sent 1,127,998 frames and the engine took 1,127,995 (3 still in flight).

| Gate | Limit | Measured | Result |
|---|---|---|---|
| Packet → engine p99 | < 2 ms | 14.8 ms over the whole run (p50 0.05 ms, max 86 ms) | FAIL |
| Packet → publish p99 | < 300 ms | 295 ms (max 336 ms) | pass |
| Dropped game packets at 5× | 0 | 0 | pass |
| RAM idle | ≤ 60 MB | 31.7 MB | pass |
| RAM fight | ≤ 90 MB | 71.6 MB after 2 min at 1×; 330 MB peak after 9 min at 5× | FAIL |
| CPU idle | < 1 % | 0.23 % | pass |
| CPU 20-player fight | < 5 % | 3.19 % at 1×, 17.6 % at 5× (traffic generator included) | pass |

### Why the two gates fail

Both come from work that grows with the record.

1. **The full report rebuild.** `CaptureAnalysis::snapshot()` runs once a second on the engine thread. Its median grew linearly, from 0.8 ms to 21 ms over the run, with a p99 of 61 ms. Frames wait in the channel meanwhile (none are lost). The runtime needs it only for weapons, roster, character, counters, dungeons and identities.
2. **RAM grows with the record, even at 1× (≈0.27 MB/s).**
   - The `memory_probe` test feeds the same traffic straight into one `CaptureAnalysis`: 26 → 70 MB over 230k frames, about 550 bytes per counted event. That part is meter-core's record.
   - The rest is not measured separately. My reading is that it comes from the resolved copy of the dungeon runs and from the store serializing the growing live run to a `serde_json::Value` every 5 s, which leaves multi-MB transient allocations behind in the allocator.

## Cross-crate requests

1. **meter-store `json::format_double`.**
   - **Bug:** it rounds exact decimal ties up; .NET rounds them to even. Example: 161806.428955078125 is written …813 where C# writes …812. The values are equal, but the bytes differ.
   - **Fix:** take the digit count N from `{:e}`, then format with `format!("{value:.prec$e}", prec = N - 1)`. Rust's exact formatting rounds half to even, so it gives .NET's digits.
   - **Test:** `the_stress_capture_is_saved_byte_for_byte` then passes; remove its `#[ignore]`.
2. **meter-core `CaptureAnalysis`: cheap accessors** for `weapons()`, `party()`, `first_timestamp()` and the counters/diagnostics, so the per-second full `snapshot()` is no longer needed. A dungeon snapshot that is incremental, or returns `Arc` runs/encounters, would also remove the clone and resolve per report. This fixes the packet → engine gate.
3. **meter-core memory.** Bound what a record keeps per event, about 550 B today (event-id set, pending lists). It is needed for the 90 MB / 2 h gate.
4. **meter-store.** Accept typed or `Arc` snapshots and serialize straight to a `String`, without going through a `serde_json::Value`. Same `json` formatter.
5. **meter-app.** Re-export the runtime's `feed` (above), run `Runtime` as its `MeterFeed`, and keep `InstallUpdate`/`Update` on its own side.
6. **Workspace.** No Cargo.toml changes outside this crate. Dependencies added to meter-runtime: crossbeam-channel, tokio, reqwest (rustls, for avatars), uuid; dev: sha2. `rust/Cargo.lock` changed accordingly.
