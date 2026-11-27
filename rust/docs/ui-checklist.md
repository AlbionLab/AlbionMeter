# UI parity checklist (meter-app)

The definition of done for the Slint UI. Each line is one visual state or one interaction of the React app (`web/src`)
and the Electron shell (`desktop/main.js`). Tick a line only after verifying it: visual states against the pair in
`rust/target/ui-parity/<screen>-{react,slint}.png` (`node rust/scripts/ui-parity.mjs`), interactions by running
`cargo run -p meter-app -- --fake` (or a unit test, named in brackets).

Legend: `[x]` verified · `[ ]` not done or not verified · `[~]` done with a known difference (written after it).

## Shell — main.tsx top bar, DesktopChrome.tsx, desktop/main.js

- [x] Frameless window, 1280×820 by default (`min(1280, area−120)` × `min(820, area−100)`), 820×560 minimum, background `#0a0d13`
- [ ] Size and maximized state saved in `window.json` and restored (shared with the Electron build)
- [x] Brand: mark 26 px, "ALBION METER" 14 px / 800 / .08em, version pill `v<version>` (from the server version)
- [x] Views Meter / Capture / Game data with their icons (activity, database, sword); active one orange
- [x] Language switch EN / PT (mono 11 px); choice saved in `settings.json` and used at next start; system locale on first run
- [x] Status pill: Ready (grey), Recording (green), Paused (amber), Stopped (grey, completed), Reconnecting (amber, not connected)
- [x] Window controls minimize / maximize↔restore icon / close (close hover red `#e81123`)
- [ ] Drag the window by the empty bar; double click on the bar maximizes/restores
- [ ] Resize from the window edges (frameless)
- [x] Loading screen (mark 56 px, spinner, "Loading the meter…") until the first snapshot, while not connected
- [x] Error banner with Dismiss (command failures, translated server messages)
- [x] Update toast: downloading (version, percent, progress bar, "x MB of y MB · ", What's new link)
- [x] Update toast: ready (Later hides it for that version, Restart and update → installing, buttons disabled, "Restarting…")
- [x] No animation or timer runs while idle (spinner only on the loading screen)

## Meter — main.tsx

- [x] Current session card: eyebrow, "Encounter" + mm:ss / h:mm:ss clock (0 when not live)
- [x] Idle / completed: Play (green, 116 px min); running: Pause (amber); paused: Resume (green)
- [x] Running / paused: Reset (ghost) and End session (danger) after a separator, and the session note with the info icon
- [ ] Controls disabled while a command runs or the meter is not connected
- [x] Double-click guard: the second click of a double click on Play/Pause/Resume/Reset/End is ignored [controller::control]
- [~] Reset opens the confirm modal (warn icon, texts, Cancel / Reset); confirm sends new-record — no backdrop blur (Slint has no backdrop filter)
- [~] End session opens the confirm modal (bad icon, texts, Cancel / End session); confirm sends stop-record — no backdrop blur
- [ ] Modal: Escape and the backdrop cancel; Enter confirms
- [x] Damage / Healing panel heading: title, "{total} total · {rate} party {unit}" (compact / precise per language)
- [x] Metric switch Damage / Healing re-sorts the bars
- [x] Empty states: "Press Play, then fight…" (idle) / "Waiting for combat…" (otherwise)
- [x] Bars: rank, weapon icon (or "?"), name, value compact + (share %), rate "N DPS/HPS"; fill = share of the leader in the player colour (28 %) with a 2 px edge
- [x] Unconfirmed members with no damage/healing are hidden
- [x] Weapon icons by item id from the icon cache; "?" while missing
- [ ] Copy button: menu (Ranking as image / as text / Player names), disabled with no players
- [x] Ranking as text identical to CopyDamage.tsx `damageText` [ranking::tests]
- [x] Player names identical to `playerNames` [ranking::tests]
- [~] Ranking as image — not implemented: shows "Image rendering is unavailable."
- [ ] Copy confirmation toast under the button, clears after 1.8 s (errors 4 s)

## AlbionLab card — main.tsx

- [x] Loading: "Connecting to AlbionLab…"
- [~] Not connected: text, pairing errors (revoked / denied / expired), Connect account (link icon) — verified in Slint; the React card is clipped by a reference bug (see below)
- [ ] Connect is ignored while a connect is pending (double click)
- [~] Pairing: approval text, code (34 px mono, brand colour), Open AlbionLab again, "Waiting for approval…" — same clipping in React
- [x] Connected, no session: "No live session" / "Creating the session…" with its text and My sessions
- [x] Connected with session: Live / Finished / Offline — resending / Not published pill, link without scheme, copy (check for 1.5 s), Open full analysis, orange border and glow
- [~] Account name and avatar — name only; the avatar image (meter-lab download) shows an empty circle
- [ ] Session name field (placeholder = suggestion or "Untitled session", 80 chars, Enter/blur saves, Escape reverts) and "Suggestion: <name>" button
- [x] Visibility select (unlisted / public / private)
- [x] Hide other players' names, Share dungeon location checkboxes
- [ ] Disconnect account (asks first)
- [ ] Session / card errors translated

## Capture notice — CaptureNotice.tsx

- [x] Failed capture: sticky red banner "Combat is not being recorded because …" + Open Capture
- [ ] ≥ 30 pending unidentified updates while recording: amber banner
- [x] Hidden on the Capture view and for replays

## Capture — CapturePanel.tsx, ui.tsx

- [x] Head: eyebrow, title, text, status pill (tone by state), source
- [x] Method select (unavailable methods disabled, "(unavailable)")
- [x] Interface select (filtered by method; Main / ExitLag / VPN badges; Main first)
- [x] Refresh, Start capture, Stop, Replay capture enabled/disabled like React
- [ ] Interface access error note; command / status errors banner
- [x] Permission hint: Windows text / Linux "Allow packet capture" (pkexec setcap, restart) / setcap command + Copy
- [x] Four stat cards (frames, transport messages, MB, adapter drops)
- [x] Diagnostics grid with count badge and empty state
- [x] Session side panel: library, combat decoding, stop reason, file, packet persistence, notes

## Game data — Library.tsx GameLibrary

- [x] Heading: title, game version note, Weapons / Maps segmented with counts, search field
- [x] Table: NAME, WEAPON FAMILY / MAP TYPE, TIER tag, ID (mono); first 100 rows
- [ ] Search by name, family/type or id; "{n} results" + "Showing the first 100…"; empty text
- [ ] Import game data: text, Import game folder (folder picker), Importing…, result message
- [x] Catalog and Local records side panels

## Dungeon journal — Library.tsx DungeonJournal, Timeline.tsx, TimelineInspector.tsx, DungeonLoadout.tsx, HistoryTransfer.tsx

Not reachable in the current React app (main.tsx renders only Meter / Capture / Game data), so there is no React
screenshot to compare; the Slint port is behind `--journal`.

- [ ] Runs list, run hero, floor tabs, five stat cards
- [ ] Timeline: grid, axes, one line per player or item, fill under ≤ 4 series, cursor, click/drag/arrow keys/Home/End/Escape
- [ ] Scrubber and Latest
- [ ] Inspector: scope, order, metric, search, expandable players with item sources
- [ ] Players & equipment table (expand / collapse all), floors table, progression by floor
- [ ] Export dungeon / history, import history (40 MB limit), result messages

## Performance

- [x] Snapshots ≤ 4 Hz, models updated row by row (`controller::sync`)
- [x] Idle CPU < 1 % of a core (release build)
- [x] RSS reported idle and after a 10-minute fake fight

## Verification record

Parity pairs verified by eye in `rust/target/ui-parity/` (22 screens): loading, meter-damage, meter-healing, meter-pt,
meter-pt-paused, meter-idle, meter-paused, meter-completed, confirm-reset, confirm-end, error-banner, lab-loading,
lab-disconnected, lab-pairing, lab-nosession, capture-failed, capture, capture-permission, game-data, game-data-pt,
update-downloading, update-ready.

Reference bug found while comparing: in the Electron layout (`.desktop .mini-main` is a fixed-height flex column with
`overflow-y: auto`) the AlbionLab card (`overflow: hidden`) shrinks to ~90 px and clips its body (session name, toggles,
Connect account, the pairing code). The Slint card keeps its natural height; fix in `web/src/styles.css` with
`.mini-main > * { flex-shrink: 0; }` if the React build stays in use.
