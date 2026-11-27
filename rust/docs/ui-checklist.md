# UI parity checklist (meter-app)

The definition of done for the Slint UI. Each line is one visual state or one interaction of the React app (`web/src`)
and the Electron shell (`desktop/main.js`). A line is ticked only after it was verified, and the evidence is named
after it:

- `pair:<screen>`: `rust/target/ui-parity/<screen>-{react,slint}.png`, compared by eye (`node rust/scripts/ui-parity.mjs`,
  both apps rendered from `rust/crates/meter-app/fixtures/parity.json`);
- `test:<name>`: a unit or UI test (`cargo test -p meter-app`; the `ui_tests` drive the real UI through Slint's testing
  backend: clicks at the controls' positions, keys, text, mock time);
- `window:<check>`: `python3 rust/scripts/window-check.py <release binary>` on a real niri session (a virtual pointer
  clicks the real window; niri and the Wayland protocol log report what happened).

Every run uses a temporary settings folder (`ALBION_METER_CONFIG_DIR`); none touches `~/.config/Albion Meter`.

Legend: `[x]` verified · `[~]` verified, with a known and deliberate difference (written after it).

## Shell — main.tsx top bar, DesktopChrome.tsx, desktop/main.js

- [x] Frameless window, 1280×820 by default, 820×560 minimum, background `#0a0d13` — pair:meter-damage, window:minimum size
- [x] Size and maximized state saved in `window.json` (shared with the Electron build) and asked for at the next start — window:size saved, window:saved size asked for (niri, a tiling compositor, then sizes a new window itself, as it does Electron's)
- [x] Brand: mark 26 px, "ALBION METER" 14 px / 800 / .08em, version pill — pair:meter-damage
- [x] Views Meter / Capture / Game data with their icons; active one orange — pair:meter-damage, pair:capture, pair:game-data, test:header_views_language_and_window_actions
- [~] Below 1000 px the views show icons only, so the window controls stay inside the bar down to 820 px — pair:narrow (the web bar overflows at 820 px and cuts off maximize and close; styles.css drops the labels only at ≤780 px)
- [x] Language switch EN / PT; saved in `settings.json`, used at the next start; system locale on the first run — pair:meter-pt, pair:game-data-pt, test:header_views_language_and_window_actions
- [x] Status pill: Ready, Recording, Paused, Stopped, Reconnecting — pair:meter-idle, pair:meter-damage, pair:meter-paused, pair:meter-completed, pair:loading
- [x] Window controls minimize / maximize↔restore / close (close hover red) — window:minimize button, window:maximize button toggles back, window:close button
- [x] Drag the window by the bar — window:drag by the top bar (moved exactly 160,90)
- [x] Double click on the bar maximizes / restores — window:double click on the bar (niri maximized it to 2560×1406)
- [x] Resize from the edges and corners — window:resize from the right edge, window:resize from the top-left corner
- [x] After a move or resize the next press goes where it lands (the compositor keeps the release; the grab is let go) — window:resize after drag
- [x] Loading screen until the first snapshot while not connected — pair:loading
- [x] Error banner with Dismiss; server messages translated — pair:error-banner, test:messages_from_the_services_are_translated
- [x] Update toast: downloading (version, %, bar, "x MB of y MB · ", What's new) — pair:update-downloading
- [x] Update toast: ready (Later hides it for that version, Restart and update → installing) — pair:update-ready
- [x] Software renderer by default; `femtovg` via `--renderer`, `ALBION_METER_RENDERER` or `"renderer"` in settings.json; a renderer that fails to start makes the meter start again with the other — run with `ALBION_METER_RENDERER=femtovg __EGL_VENDOR_LIBRARY_FILENAMES=/dev/null`: "the femtovg renderer did not start …; starting again with software"
- [x] No animation or timer runs while idle — idle CPU 0.0x % (Performance below)

## Meter — main.tsx

- [x] Current session card: eyebrow, "Encounter" + clock — pair:meter-damage, pair:meter-idle
- [x] Idle / completed: Play; running: Pause; paused: Resume — pair:meter-idle, pair:meter-damage, pair:meter-paused, pair:meter-completed
- [x] Running / paused: Reset and End session after a separator, and the session note — pair:meter-damage, pair:meter-pt-paused
- [x] Controls disabled while a command runs or the meter is not connected — test:record_controls_send_commands_ignore_double_clicks_and_disable_while_busy
- [x] Double-click guard on Play/Pause/Resume/Reset/End — test:record_controls_send_commands_ignore_double_clicks_and_disable_while_busy, test:the_second_click_of_a_double_click_is_ignored
- [~] Reset asks first (warn icon, texts, Cancel / Reset) and sends new-record — pair:confirm-reset, test:reset_and_end_ask_first_and_the_modal_answers_keys_and_the_backdrop (no backdrop blur: Slint has no backdrop filter)
- [~] End session asks first (bad icon, texts, Cancel / End session) and sends stop-record — pair:confirm-end, same test (no backdrop blur)
- [x] Modal: Escape and the backdrop cancel; Enter confirms — test:reset_and_end_ask_first_and_the_modal_answers_keys_and_the_backdrop
- [x] Heading "{total} total · {rate} party {unit}" with compact / precise per language — pair:meter-damage, pair:meter-pt, test:compact_matches_intl
- [x] Metric switch Damage / Healing re-sorts the bars — pair:meter-healing, test:copy_menu_copies_the_ranking_and_the_names_and_the_toast_clears
- [x] Empty states — pair:meter-idle
- [x] Bars: rank, weapon icon or "?", name, value + share, rate, colour fill and edge — pair:meter-damage, pair:meter-healing
- [x] Unconfirmed members with no damage/healing hidden — pair:meter-damage (the fixture's unconfirmed "Ghost")
- [x] Weapon icons by item id — pair:meter-damage (real cached icons in the fixture)
- [x] Copy button and its menu; disabled with no players — pair:copy-menu, test:copy_menu_copies_the_ranking_and_the_names_and_the_toast_clears
- [x] Ranking as text identical to `damageText` — test:ranks_like_the_web_copy, test:copy_menu_…
- [x] Player names identical to `playerNames` — test:ranks_like_the_web_copy, test:copy_menu_…
- [x] Ranking as image: the same picture as `damageImage` (720 px card at 2×), on the clipboard as an image — pair:ranking-image, pair:ranking-image-healing-pt (React's read back from the browser clipboard), test:rows_rank_players_like_the_text
- [x] Copy confirmation under the button, clears after 1.8 s (errors after 4 s); "Preparing image…" while rendering — test:copy_menu_…, test:copy_errors_stay_four_seconds

## AlbionLab card — main.tsx

- [x] Loading: "Connecting to AlbionLab…" — pair:lab-loading
- [x] Not connected: text, pairing errors, Connect account — pair:lab-disconnected (the React card is clipped by a bug in styles.css, see below; the Slint card shows all of it)
- [x] Connect is ignored while a connect is pending; the approval page opens once — test:albionlab_connect_once_name_editing_and_disconnect
- [x] Pairing: approval text, code, Open AlbionLab again, "Waiting for approval…" — pair:lab-pairing (same React clipping)
- [x] Connected, no session — pair:lab-nosession, pair:meter-idle
- [x] Connected with session: state pill, link without scheme, copy (check for 1.5 s), Open full analysis, orange border and glow — pair:meter-damage, pair:meter-completed, test:albionlab_connect_once_…
- [~] Account name and avatar — pair:meter-damage (the name; the avatar image arrives from meter-lab in the integration phase, an empty circle until then; React shows a broken image there today)
- [x] Session name field: placeholder, 80 chars, Enter/blur saves, Escape reverts; "Suggestion: <name>" applies it — test:albionlab_connect_once_name_editing_and_disconnect, pair:meter-damage
- [x] Visibility select — pair:meter-damage, pair:lab-nosession
- [x] Hide other players' names, Share dungeon location — test:albionlab_connect_once_…
- [x] Disconnect account asks first — test:albionlab_connect_once_…
- [x] Session / card errors translated — test:messages_from_the_services_are_translated

## Capture notice — CaptureNotice.tsx

- [x] Failed capture banner with Open Capture — pair:capture-failed
- [x] ≥ 30 pending unidentified updates while recording: amber banner — pair:capture-pending, test:capture_notice_pending_players_and_interface_errors
- [x] Hidden on the Capture view and for replays — pair:capture (a running capture), pair:capture-permission (failed, on the Capture view)

## Capture — CapturePanel.tsx, ui.tsx

- [x] Head: eyebrow, title, text, status pill, source — pair:capture
- [x] Method select (unavailable methods disabled) — pair:capture
- [x] Interface select (filtered by method; Main / ExitLag / VPN badges) — pair:capture
- [x] Refresh, Start capture, Stop, Replay capture enabled/disabled like React; Start sends the chosen interface — pair:capture, test:capture_notice_pending_players_and_interface_errors
- [x] Interface access error note; command / status errors banner — pair:capture-devices-error, pair:capture-permission, test:messages_from_the_services_are_translated
- [x] Permission hint: Linux "Allow packet capture" (pkexec setcap, restart) — pair:capture-permission, pair:capture-devices-error
- [x] Four stat cards — pair:capture
- [x] Diagnostics grid with count badge and empty state — pair:capture, pair:capture-permission
- [x] Session side panel — pair:capture

## Game data — Library.tsx GameLibrary

- [x] Heading: title, game version, Weapons / Maps with counts, search — pair:game-data, pair:game-maps
- [x] Table: NAME, WEAPON FAMILY / MAP TYPE, TIER tag, ID; at most the window height − 290 px, scrolling inside — pair:game-data, pair:game-maps
- [x] Search by name, family/type or id; "{n} results" + "Showing the first 100…"; empty text — pair:game-search, pair:game-search-empty, pair:game-maps, test:game_data_search_and_folder_import
- [x] Import game folder: folder picker, Importing…, result message — test:game_data_search_and_folder_import
- [x] Catalog and Local records side panels — pair:game-data, pair:game-data-pt

## Dungeon journal — Library.tsx DungeonJournal, Timeline.tsx, TimelineInspector.tsx, DungeonLoadout.tsx, HistoryTransfer.tsx

main.tsx does not mount these; the React side of the pairs is `rust/scripts/parity-journal.tsx`, which mounts the real
components inside main.tsx's top bar. The Slint journal is behind `--journal`.

- [x] Runs list, run hero, floor tabs, five stat cards, empty state — pair:journal, pair:journal-floor, pair:journal-empty, test:dungeon_journal_interactions
- [x] Timeline: grid, axes, a line per player or item, fill under ≤ 4 series, legend — pair:journal-chart, pair:journal-items, pair:journal-healing, pair:journal-floor
- [x] Cursor: click/drag, arrow keys (Shift ×10), Home/End/Enter/Escape — pair:journal-pinned, test:dungeon_journal_interactions
- [x] Scrubber and Latest — pair:journal-chart, test:dungeon_journal_interactions
- [x] Inspector: scope, order, metric, search, expandable players with item sources and icons — pair:journal-chart, pair:journal-pinned, test:dungeon_journal_interactions
- [x] Players & equipment (expand / collapse all), floors table, progression by floor — pair:journal-loadout, pair:journal-floors, pair:journal-progression, test:dungeon_journal_interactions
- [x] Copy (ranking of the shown floor) — test:dungeon_journal_interactions, pair:journal-chart
- [x] Export dungeon / history, import history (40 MB limit), result messages — pair:history-imported, test:dungeon_journal_interactions

## Performance

- [x] Snapshots ≤ 4 Hz, models updated row by row (`controller::sync`) — test:the_fight_grows_and_sends_only_changed_points
- [x] Idle CPU < 1 % of a core (release, software renderer) — measured, see the report
- [x] RSS reported idle and after a 10-minute fake fight — measured, see the report

## Differences kept on purpose

- No backdrop blur behind the confirm modal (Slint has no backdrop filter); the background pixels match.
- No animations (live-dot pulse, bar-width transition, modal/menu/toast fades): the UI redraws only when something
  changes. The spinner turns only on the loading screen.
- No focus rings (`:focus-visible`) except on selects and text fields; no `title` tooltips.
- Text in toasts and banners cannot flow inline: "What's new" sits on its own line, the bold banner prefix is a separate run.
- Dates are formatted from the UTC time (the web app uses the browser's time zone).
- Below 1000 px the views show icons only (see Shell).

## Bugs found in the reference while comparing

- `.desktop .mini-main` is a fixed-height flex column with `overflow-y: auto`, so the AlbionLab card (`overflow: hidden`)
  shrinks to ~90 px and hides its body (session name, toggles, Connect account, the pairing code). Fix:
  `.mini-main > * { flex-shrink: 0; }`.
- At the 820 px minimum width the Electron top bar overflows and cuts off the maximize and close buttons.
