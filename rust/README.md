# Albion Meter — Rust port

The whole meter (packet capture, Photon decoding, combat engine, storage, AlbionLab live session, UI, updater) rewritten as **one native Rust process with a Slint UI**: no Electron, no webview, no .NET server, no localhost HTTP.

The C# + React code in this repo is the **reference implementation**. Behaviour, numbers and look must match it; when in doubt, read the C# / TSX and do what it does.

## Rules for everyone working here

- Never add AI attribution to commits or PRs (no `Co-Authored-By`, no "Generated with"). Agents do not commit; the coordinator does.
- Stay inside the crate(s) your role owns. Need something from another crate? Write the request in your report instead of editing it.
- Build with your own target dir so parallel agents don't block each other: `CARGO_TARGET_DIR=target/<role> cargo …` (run from `rust/`).
- `cargo clippy -p <your crate> --all-targets -- -D warnings` and `cargo test -p <your crate>` must pass before you report.
- Keep code idiomatic and terse; comments only where the *why* is not obvious (same density as the C#).

## Performance budget (the reason for the port)

| | Electron + .NET today | Target |
|---|---|---|
| Processes | 4+ | 1 |
| RAM idle / in a 30-min fight | 300–500 MB | ≤ 60 MB / ≤ 90 MB |
| CPU idle with capture on | — | < 1 % of one core |
| UI updates | 10 full snapshots/s over JSON | ≤ 4 Hz, incremental model updates, no JSON |
| Release binary | ~150 MB install | ≤ 20 MB |

Hot paths never allocate per packet when they can reuse buffers; the timeline is never copied whole per tick.

## Architecture

```
capture thread ──(bounded channel: CaptureFrame)──▶ engine thread ──(Arc<Snapshot>, ≤4 Hz)──▶ Slint UI thread
 meter-capture                                        meter-core: owns ALL meter state        slint::invoke_from_event_loop
                                                         │ commands (play/pause/end/reset) come back over a channel
                                                         ▼
                                       tokio runtime: meter-store (SQLite), meter-lab (AlbionLab, icons), updater
```

State has exactly one owner (the engine thread); everything else talks to it through channels. No `Mutex` around meter state.

## Crates and owners

| Crate | Owner role | Ports (C#/TS reference) |
|---|---|---|
| `meter-capture` | rust-capture | `NativePcap.cs`, `LinuxSocketCapture.cs`, `PacketCapture.cs` (devices, live sources, Albion port filter, BPF), `PcapWriter.cs`, `CaptureStorage.cs`, `WindowsElevation.cs`, `WindowsFirewall.cs` |
| `meter-core` | rust-core | `NetworkDecoder.cs`, pcap file reader, `PhotonTransport.cs`, `Protocol18.cs`, `CaptureAnalysis.cs`, `CombatEngine.cs`, `CombatSourceCatalog.cs`, `DungeonRun.cs`, `Observation/*`, `GameAssetImporter.cs`, `MapCatalog.cs`, `RecordControls.cs` |
| `meter-store` | rust-services | `EncounterStore*.cs`, `HistoryArchive.cs`, `CaptureHistory.cs`, `CharacterContextStore.cs`, `HistoryEndpoints.cs` (export/import format) |
| `meter-lab` | rust-services | `AlbionLab/*` (pairing, settings, SignalR live session, compactor), `WeaponIconCache.cs` |
| `meter-app` | rust-ui | `web/src/*` (every screen, styles, i18n), `desktop/main.js` (window, update flow, permission grant) — wiring of all crates comes last |

Contracts between crates:
- `meter_capture::CaptureFrame { timestamp_us, original_length, link_type, data }` — already defined; live sources yield these.
- `meter-core` exposes `Analysis::add(&CaptureFrame)` and serde types whose JSON is **field-for-field identical** to the C# output (camelCase, same nulls), so `tools/AlbionMeter.Capture replay <pcap>` output can be diffed against the Rust replay.
- `meter-store` keeps the **same storage directory, SQLite schema and JSON files** as today (`%LOCALAPPDATA%\AlbionMeter` on Windows, `~/.local/share/AlbionMeter` on Linux, plus the portable `data` folder next to the exe), so an existing user's history opens unchanged.
- `meter-app` talks to the engine through a `MeterFeed` trait so the UI can run on fake data before the engine exists.

## Phases

1. **Parallel ports** — capture, core (with differential tests against C#), services (store + lab), UI on fake data.
2. **Integration** — wire the threads, record controls, settings, AlbionLab, icons, updater (Velopack), Windows elevation/Npcap check.
3. **QA** — differential replay vs C#, existing-history compatibility, RAM/CPU measurements against the Electron build, Windows + Linux packaging in CI.
4. **Beta** next to the Electron build, then switch.

## Libraries

`slint` (UI; winit backend, femtovg renderer with software fallback for weak GPUs), `pcap` (Npcap/libpcap), `libc` (AF_PACKET + BPF), `rusqlite` with `bundled`, `serde`/`serde_json`, `tokio`, `reqwest` (rustls), `tokio-tungstenite` (minimal SignalR JSON hub client), `velopack` (updates + installers), `windows` (elevation, firewall), `crossbeam-channel`.

## Real-time guarantees (acceptance gates, measured in CI and on a weak PC)

| Gate | Limit | How it is measured |
|---|---|---|
| Packet → engine | p99 < 2 ms | timestamp at `CaptureFrame` vs. after `Analysis::add`, histogram in a replay bench |
| Packet → pixels | p99 < 300 ms (≤ 4 Hz UI) | engine publish time vs. Slint frame rendered |
| Decode throughput | ≥ 50× real raid traffic on one core | `cargo bench` (criterion) on recorded pcaps |
| Dropped game packets | 0 at 5× real traffic | capture thread drop counter in a soak test |
| RAM | idle ≤ 60 MB, 2 h fight ≤ 90 MB, no growth after the 2 h cap | RSS sampled every 10 s in a 2 h soak |
| CPU | idle < 1 %, 20-player fight < 5 % of one core | soak test, `/proc/<pid>/stat` / Windows perf counters |
| Allocations per packet | 0 in steady state on the decode path | counting global allocator in a test |

Design rules behind them: the capture thread only copies bytes into recycled buffers; the engine thread owns all state and never waits on I/O (SQLite, HTTP and icons run on tokio and receive owned copies); the UI gets an `Arc` snapshot plus a "what changed" set and updates only those model rows; nothing in the UI animates or polls while idle (event-driven redraw only); bounded channels everywhere, so overload drops and counts instead of growing memory or latency.

## UI parity gate

Every screen and state of the React app is rendered twice from the **same fake data** — the React build via Playwright with mocked `/api` routes (as `web/tests/race-guards.mjs` does) and the Slint app with `--fake --screen <name>` — and compared side by side (`rust/scripts/ui-parity.mjs`, images in `rust/target/ui-parity/`). A screen is done only when the two match in layout, spacing, colours, fonts, icons and text, and every interaction listed in `rust/docs/ui-checklist.md` works.
