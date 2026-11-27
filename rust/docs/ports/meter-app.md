# meter-app (Slint UI) — report

## Results

- `CARGO_TARGET_DIR=target/rust-ui cargo clippy -p meter-app --all-targets -- -D warnings` → exit 0 (only the workspace's own "unused workspace dependency meter-core/meter-lab" manifest warnings)
- `CARGO_TARGET_DIR=target/rust-ui cargo test -p meter-app` → `test result: ok. 20 passed; 0 failed`
- Checklist `rust/docs/ui-checklist.md`: **44 / 71 verified**, 6 done with a known difference (`[~]`), 21 open
- Parity: `node rust/scripts/ui-parity.mjs` (with `CARGO_TARGET_DIR=target/rust-ui`) → 22 screen pairs in `rust/target/ui-parity/<screen>-{react,slint}.png`. I looked at all 22 and fixed what differed. Screens: loading, meter-damage, meter-healing, meter-pt, meter-pt-paused, meter-idle, meter-paused, meter-completed, confirm-reset, confirm-end, error-banner, lab-loading, lab-disconnected, lab-pairing, lab-nosession, capture-failed, capture, capture-permission, game-data, game-data-pt, update-downloading, update-ready.

## Measurements (release build, Linux/Wayland, NVIDIA, 10 minutes each, sampled every minute)

| Run | CPU (one core) | RSS | Anonymous RSS (own heap) |
|---|---|---|---|
| femtovg, idle (`--fake-idle`) | 0.02–0.08 % | 125.9 MB, flat | 31.2 MB, flat |
| femtovg, fake fight (`--fake`, 9 players, 4 Hz) | 1.5–2.3 % | 128.0 → 134.2 MB | 33.1 → 39.4 MB |
| software renderer, fake fight (`--fake --software`) | 0.7–1.3 % | 41.0 → 47.7 MB | 5.5 → 12.2 MB |

- About 75 MB of the femtovg RSS is the OpenGL driver mapped from files (libnvidia-gpucomp, libLLVM, libgallium, eglcore). The ≤ 60 MB idle gate cannot be met with femtovg on this machine. The software renderer meets it and also uses less CPU. **Recommendation (coordinator's call): make the software renderer the default and femtovg an opt-in.**
- The roughly 0.65 MB/min growth during the fight is the fake timeline (one point per second, kept in the feed and in the UI's `Record`) plus the dungeon run the fake feed republishes every 2 s.
- Release binary: 20.3 MB, just over the 20 MB budget. It grew from 19.9 MB when serde derive and `software-renderer-path` were added. The workspace profile is not mine to change; `opt-level = "s"` would likely bring it back under.

## Differences that remain (Slint vs React)

1. The React AlbionLab card is clipped to about 90 px. This is a bug in the reference, not in the port: `.desktop .mini-main` is a fixed-height flex column and the card has `overflow:hidden`, so it shrinks and hides the session name, the toggles, Connect account and the pairing code. Slint keeps the card's full height, so on those screens the bars start lower than in React. Fix for the web app: `.mini-main > * { flex-shrink: 0 }`.
2. The confirm modal has no `backdrop-filter: blur(2px)` behind it. Background pixels match exactly; the text behind is less dimmed.
3. Animations are left out on purpose (event-driven redraw): the pulsing live dot, the 0.3 s bar-width transition, and the fade/pop of the modal, toast and menu. The spinner turns only on the loading screen.
4. The AlbionLab avatar is an empty circle; meter-lab does not deliver the image yet.
5. "Ranking as image" is not implemented and shows "Image rendering is unavailable.". Text and player names are identical to CopyDamage.tsx.
6. No tooltips (`title` attributes) and no `:focus-visible` rings, except on selects and text fields.
7. Text in the toasts and banners cannot flow inline, so "What's new" sits on its own line and the bold banner prefix is a separate run.
8. The dungeon journal (DungeonJournal/Timeline/Inspector/Loadout/HistoryTransfer) is not reachable in the current React app, so there is no parity pair for it. It is ported, behind `--journal`.

## Port table

| Reference | Rust / Slint |
|---|---|
| main.tsx (top bar, controls, ConfirmModal, AlbionLabCard, SessionName, LanguageSwitch, WeaponIcon, bars) | ui/app.slint, ui/chrome.slint, ui/meter.slint, src/controller.rs |
| useMeter.ts, state.ts | src/merge.rs, src/controller.rs (busy, errors, ClickGuard) |
| types.ts | src/feed.rs, src/format.rs |
| i18n.ts | src/i18n.rs + src/i18n_table.rs (generated verbatim, count checked by a test), ui/i18n.slint |
| CaptureNotice.tsx | meter.slint CaptureNotice, controller::render_notice |
| CapturePanel.tsx, ui.tsx | ui/capture.slint, ui/widgets.slint, controller::render_capture |
| CopyDamage.tsx | src/ranking.rs, meter.slint CopyButton |
| DesktopChrome.tsx | ui/chrome.slint |
| Library.tsx GameLibrary | ui/library.slint, controller::render_catalog |
| Library.tsx DungeonJournal, Timeline.tsx, timelineBuckets.ts, timelineNavigation.ts, TimelineInspector.tsx, sourceItems.ts, DungeonLoadout.tsx, HistoryTransfer.tsx | ui/journal.slint, src/journal.rs |
| Select.tsx, Icon.tsx (same paths), ItemIcon.tsx | ui/widgets.slint, ui/icons.slint |
| styles.css | ui/theme.slint + component styles |
| desktop/main.js | src/main.rs, src/desktop.rs (settings.json/window.json shared with Electron, fonts, links, pkexec setcap grant), controller::window_action |
| (new) | src/fake.rs (`--fake`, `--fake-idle`, `--fake-update`), src/fixture.rs + fixtures/parity.json + fixtures/icons (real cached weapon icons), src/headless.rs (software renderer → PNG), src/screenshots.rs |

## Traits and structs for the integration phase (cross-crate requests)

- `feed::MeterFeed::run(self, publish, commands)`, with `FeedEvent` and `Command` in src/feed.rs. The UI types mirror types.ts.
- **meter-core:** a `Snapshot` as in feed.rs, published at ≤ 4 Hz as an `Arc` with `full` and only the changed timeline points (mergeSnapshot semantics). `Player.weapon_id` should be set by the engine, so the UI no longer looks it up by name. Record commands should answer with the C# English error text.
- **meter-capture:** `DeviceList` (methods, devices, flags bit 0 = default route, error) and `CaptureStatus` (state, source, stop reason keys, adapter drops, counters in server order including `pendingUnidentified` and `transportMessages`).
- **meter-lab:** `LabStatus` / `LabSession`, avatar bytes, `LabError(String)`, and `Icon(id, PathBuf)` from the icon cache. The cache owns retries; the UI asks once per id.
- **meter-store:** dungeon list and detail (`DungeonSummary`, `DungeonRun`), `ExportHistory { dungeon, to }`, `ImportHistory(path)` (answered with `HistoryMessage`), `ImportCatalog(folder)`, the storage path.
- **Not done yet:** native file pickers on Windows/macOS (Linux uses zenity/kdialog), Velopack wiring behind `InstallUpdate`, and the startup splash steps of desktop/main.js (no server to start any more).

## Side effect to know about

My first `--screenshots` run (before persistence was turned off for screenshot modes) wrote `"language": "pt"` into `~/.config/Albion Meter/settings.json`. It also wrote width/height 1256×1374 into `window.json`, the files the Electron build shares. I do not know the previous values; please check them. Screenshot and fixture modes no longer save anything.

## Integration: the real meter (meter-runtime)

- **Feed.** `src/feed.rs` is now `pub use meter_runtime::feed::*`. By default `main` runs `meter_runtime::Runtime` as the `MeterFeed`, configured by `Config::from_environment`. It reads the C# settings: `--Storage:Directory=…`, `--Capture:AutoStart=true`, `--AlbionLab:Url=…`, or the `Storage__Directory` style variables.
- **Modes that still use the fake feed:** `--fake`, `--fake-idle`, `--screenshots`, and the fixture modes (`--screen`, `--screenshot`).
- **Update.** `Update` and `InstallUpdate` stay on the app side; the runtime ignores `InstallUpdate`.
- **Item icons.** The runtime's `ItemContribution` has no `item_id`. The journal looks the id up from the item index:
  - in the real meter, in the shipped `combat.json` (`CombatSourceCatalog::find_item`);
  - in the fixture, in its item table.
  - Icons that are missing are requested with `Command::Icon`.
- **Startup, as desktop/main.js does it:**
  - **Storage.** The storage folder, the portable `data` folder and `assets/game-data` come from meter-runtime's `Config`.
  - **Capture autostart.** Off, as in the Electron build. `--Capture:AutoStart=true` turns it on.
  - **Linux permission** (`desktop::capture_permission_at_start`):
    - It runs only in an installed meter (`ALBION_METER_PACKAGED`, else an executable outside a Cargo `target` folder).
    - If the meter lacks CAP_NET_RAW, it asks once with `pkexec setcap`.
    - An AppImage first copies itself and its `assets` to `<settings>/server/<version>`, as Electron did with the server.
    - Granted, it starts the permitted executable. Refused, it opens anyway, and the Capture screen's "Allow packet capture" offers the grant again. That grant closes the meter cleanly, then starts it again.
  - **Windows.** An unelevated installed meter restarts itself through UAC (`meter_capture::elevation`). Npcap availability is shown by the capture method list. This code is not compiled here: the Windows cross-check needs a MinGW C compiler for ring.
  - **Single instance.** One meter per storage folder (`src/instance.rs`): a lock file plus a loopback port. A second start asks the first window to come to the front and quits. On Wayland, without an activation token, the window asks for attention (niri marks it urgent).
  - **Clean shutdown.** After the window closes, the command channel closes, the runtime saves the record, ends the AlbionLab session and stops the capture, and `main` joins its thread.
- **`merge.rs` fix.** A replayed capture starts at the capture's own times, which are earlier than now. The "older snapshot" rule of state.ts no longer compares across modes or against the "unavailable" placeholder. Before this, a replay of any capture recorded before the app started was dropped.

### Verification
- `cargo test -p meter-app` → 35 passed. This includes `instance::tests` and the merge test for replays.
- `--screenshot <png> --replay <pcap> --Storage:Directory=<temp>` runs the real Runtime offscreen. Screenshots taken:
  - the replayed 30-minute raid from `cargo run -p meter-app --example synthetic-pcap -- <folder>`, showing the bars with real weapon icons;
  - the dungeon history from a copy of a real `encounters.db`: 18 runs, opened only in a temporary folder.
- `python3 rust/scripts/runtime-check.py <release binary> <pcap>` with a real window on niri → 5/5:
  - the window opens;
  - a second start quits and the first asks for attention;
  - closing the window ends the meter (exit 0);
  - the replayed record is in the history afterwards.

### Measurements (release, software renderer, real window, temporary storage)
| | CPU | RSS |
|---|---|---|
| Idle, `--Capture:AutoStart=true` (capture refused: no CAP_NET_RAW on this binary) | 0.0–0.1 % | 40.3 MB, flat (4.8 MB anonymous) |
| Replaying the 30-minute raid (the whole file in a few seconds) | 9 % for the first 10 s | 146 MB peak |
| After that replay, record shown | 0.1 % | 82.4 MB (42.7 MB anonymous) |

- **Binary: 27.7 MB with the runtime** (tokio, reqwest/rustls, rusqlite, the capture). The 20 MB budget needs `opt-level = "s"` or trimming in the runtime's dependencies.
- **Idle with a live capture running was not measured.** It needs `setcap cap_net_raw=ep` on the binary, which takes the user's password.
