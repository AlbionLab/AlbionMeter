# Capture and replay

## Current implementation

The application can record packets through native libpcap on Linux or Npcap on Windows and replay savefiles through the same analysis pipeline. This is a diagnostic capture implementation, not a completed live combat meter. The combat dashboard remains a separately labeled demonstration.

The pipeline reads Ethernet, tagged Ethernet, raw IP, loopback, Linux cooked v1 and Linux cooked v2 frames. It validates UDP lengths, reconstructs bounded IPv4 fragment sets, reads unencrypted Photon command envelopes, reconstructs bounded Photon fragment sets and deduplicates commands by directional endpoints, peer, connection challenge, channel and sequence. Unreliable commands include their additional sequence in the identity. Duplicate tracking has a 32,768-command window.

Protocol18 decoding supports scalar values, common arrays, parameter tables, opaque custom values and basic dictionaries. Unsupported nested dictionary schemas and unsupported type codes are explicit diagnostics. Event and operation code histograms help establish the protocol mapping from a real capture. They do not establish combat attribution.

The separately tested PartyObservation component preserves roster identity across zone generations, invalidates transient object bindings, rejects stale updates, handles object-ID reuse and distinguishes unknown membership from known nonmembership. It is not yet connected to a verified Albion event mapping.

## Requirements

Use a 64-bit Windows or Linux process. Install Npcap on Windows or libpcap 1.9+ on Linux. The application does not install drivers or alter capture privileges. The running process must have permission to read the selected interface. Offline replay does not require packet capture privileges. Npcap is resolved from the Windows system Npcap directory; Linux loads libpcap.so.1. UTF-8 library initialization supports non-ASCII filenames.

## Web interface

Run `node scripts/dev.mjs` from the project directory and open `http://127.0.0.1:5187`. In Data source:

1. Select the interface carrying the game's traffic.
2. Click Start recording.
3. Play through the scenario being investigated.
4. Click Stop. The panel shows the local capture path, diagnostics and adapter drops when available.
5. Replay a saved PCAP or PCAPNG with Replay capture.

Recordings stop at 10 minutes or 128 MB. Web uploads are limited to 40 MB, so use shorter recordings for browser replay or the CLI for larger recordings. Files are stored in the application's local data directory under captures. A JSON report is saved next to each capture. New recordings are refused after 20 matching capture/report files; archive or remove generated files manually to make room. The UI does not delete recordings automatically.

Interface selection is explicit. A port match alone does not prove that traffic belongs to Albion. The default filter includes UDP 5055, 5056 and 5058 and IPv4 fragments. Packet recording is local and passive; the application does not send game packets.

## Command-line interface

List interfaces:

```bash
dotnet run --project tools/AlbionMeter.Capture -- devices
```

Record a new file for 60 seconds, replacing the interface name with a value from the device list:

```bash
dotnet run --project tools/AlbionMeter.Capture -- record enp3s0 /tmp/albion-session.pcap 60
```

On Windows, pass the exact Npcap device identifier and a new output path. Existing capture files are never overwritten. Ctrl+C requests a graceful stop. The CLI writes a JSON report next to the recording.

Replay without starting a server:

```bash
dotnet run --project tools/AlbionMeter.Capture -- replay /tmp/albion-session.pcap
```

## Validation and limits

The native offline tests create explicit synthetic packet fixtures and read them through the installed libpcap. They verify actual native savefile I/O, not a real Albion session. The service test also exercises replay and report persistence without opening an HTTP socket.

```bash
dotnet run --project tests/AlbionMeter.Tests -- --native-capture
```

The default test run skips native-library tests so ordinary Windows CI does not silently require Npcap. Linux CI includes an explicit native test step. Windows native execution is still unvalidated here.

Earlier sandbox restrictions blocked interface enumeration and server sockets. After access was enabled, the browser integration suite passed. Linux live capture was subsequently verified on wlan0 after desktop authentication granted cap_net_raw=ep to the specific AlbionMeter.Server apphost executable. The global dotnet runtime was not modified. Run the apphost directly to use this capability; rebuilding or replacing it may remove the capability. Windows capture remains unvalidated.

Remaining limitations include IPv6 fragment reassembly, CRC-enabled Photon envelopes, encrypted packets, some complex Protocol18 values, reliable-channel reordering, capture-interface recovery and validated game-event mapping. Unsupported inputs remain visible in diagnostics. Fragment expiry uses capture timestamp progress; incomplete sets at end-of-file remain reported as pending. There is no claim that a gap proves packet loss or that all party members' combat is observable.

Before connecting combat totals, validate a real session containing character discovery, equipment, damage, healing, party join/leave, zone transitions and dungeon revisits. Weapon observations and membership must be attached at contribution time. Full party snapshots must be distinguished from incomplete/invalid snapshots so a decoding failure does not erase the roster.

## Format references

- [Photon binary protocol layers](https://doc.photonengine.com/realtime/v5/reference/binary-protocol)
- [libpcap nonblocking reads](https://www.tcpdump.org/manpages/pcap_setnonblock.3pcap.html)
- [Npcap UTF-8 initialization](https://npcap.com/guide/wpcap/pcap_init.html)
- [Npcap savefile support and timestamp precision](https://npcap.com/guide/wpcap/pcap_open_offline.html)
- [Reference protocol type definitions](https://github.com/Triky313/AlbionOnline-StatisticsAnalysis/blob/main/src/StatisticsAnalysisTool.Protocol18/Protocol18Type.cs)
- [Reference event identifiers](https://github.com/Triky313/AlbionOnline-StatisticsAnalysis/blob/main/src/StatisticsAnalysisTool/Network/EventCodes.cs)

Protocol constants and field representations inform the independent implementation. Historical comments in the reference enum may contain older event numbers; they must not be treated as the current mapping. No reference implementation files were incorporated into this project.

## Party contribution behavior

CombatEngine now retains membership intervals and former participants separately from its active roster. Leaving removes the character from active snapshots immediately; rejoining restores their retained totals. Contributions during an absence are rejected even when they arrive after rejoining. Contributions before leaving remain eligible if they arrive late. Different engine instances isolate different party measurement sessions, and explicit PartyId metadata identifies the party separately from the encounter. Dungeon aggregation and persistence include former participants.

A new record resets its measurement period rather than carrying the prior record total into the new one.

A synthetic, non-game PCAP fixture is available at `artifacts/fixtures/protocol18-synthetic.pcap`. Its CLI replay output is `artifacts/packet-replay-check.json`. These artifacts verify native packet reading and Protocol18 scalar decoding without claiming live combat validation.

## Offline schema inspection

Run `dotnet run --project tools/AlbionMeter.Capture -- inspect <pcap-file>` to summarize incoming event parameter shapes and bounded numeric samples for event codes 6 and 7. This command does not modify the capture or feed the combat dashboard. Event names and meanings still require version-specific validation. See `party-zone-investigation.md` for the current real-capture findings.

## Starting a new combat record

The live combat header includes New record. The action archives the current encounter as completed, creates a new encounter identity, clears damage, observed healing, encounter and rolling rates, timeline, weapon totals and source-item totals, and retains only the currently observed party and local character. Former participants remain in the archived encounter and are absent from the new one. Explicitly archived empty records are saved; automatic history saves still omit empty encounters.

While capture is running, only combat aggregation is reset. Packet capture, transport deduplication, character bindings, current equipment and observed membership remain available. The capture worker and record reset serialize access to aggregation so an arriving event belongs to one record. The archive is saved before the reset; a storage failure leaves the current record intact. If live capture has ended, New record restarts the previously selected adapter and creates a clean record after context recovery. Imported replays do not offer this live action.

Validation passed 75 backend checks, frontend checks and a native-capture browser integration run against isolated temporary storage on the loopback adapter. The browser run verified the button, a new SQLite history entry, removal of former members, zero totals and empty item contributions, and restarting capture after a stop. Nonzero damage and healing reset, equipment retention and immutable prior snapshots are covered by the observation regression. See artifacts/new-record.png.

## Observed random dungeon floors

Successful local Join responses identify random-dungeon instances through parameter 8. The user's recorded transition changed from @RANDOMDUNGEON@1ee50435-0726-4641-8b5f-434b56850631 to @RANDOMDUNGEON@1065f2ae-56a5-4581-ac8e-7865160d92ef. Both joins carry entrance map 1315 in parameter 66, resolved by the local dictionary to Watchwood Grove. A dedicated combat observer tracks these floor records alongside the existing session. The session's aggregate is not reused as a per-floor checkpoint.

Consecutive random-dungeon joins with the same entrance belong to the observed run. Returning to an already observed instance reuses that floor and appends a distinct segment. Going outside closes the active segment; a subsequent unseen instance creates another run. Floor numbers describe discovery order, not a guaranteed ordinal when observation starts midway through a dungeon. This integration currently targets the recorded @RANDOMDUNGEON@ map family; other instance families need their own validated mapping.

Run and floor-segment identities derive from character, instance and observed join time. Replaying retained captures reconstructs the same records rather than creating duplicate copies after restart. Capture rollover retains the dungeon observer's context. Explicit New record closes the current floor segment and starts another while dungeon totals retain both segments. Snapshot checkpoints replace earlier versions of the same segment.

Floor and dungeon aggregates preserve player damage/healing, rates, source-item and weapon contributions, and immutable timelines. Rates divide contribution by observed floor time, including idle time. Aggregate timelines concatenate observed segments; boundaries within an original one-second bucket cannot recover sub-second timing. Whole-dungeon totals use the summed observed segment durations.

Dungeon journal and Encounter history > Dungeons offer floor tabs, floor-to-dungeon damage/healing percentages, party DPS/HPS, player shares, per-floor member progression and combined timelines. Open combat details reuses the meter's item/weapon charts, exports and clipboard controls for the selected floor or entire dungeon. Observed runs are checkpointed to SQLite every five seconds; floor encounters also appear in history with their floor location.

The retained two-floor replay reconciles to 30,952 damage and 1,220 observed healing: floor 1 contributes 26,433 damage (85.4%) and 716 healing; floor 2 contributes 4,519 damage (14.6%) and 504 healing. The actual API and SQLite records contain both floors and the same totals. Browser validation covered floor selection, percentages, healing progression, item graphs and grouped history. Regression validation passed 77 backend checks, including revisits, same-floor reconnects, new instances, explicit record resets, deterministic replay identities, preserved source breakdowns and immutable aggregated buckets. See artifacts/dungeon-floor-comparison.png.

## Pausing and resuming a live record

The combat header exposes Pause record and Resume record alongside New record. Pausing freezes combat time, damage, observed healing, rates and timeline accumulation without stopping raw packet capture. Character, equipment, party and zone observations continue. Health events received while paused do not contribute. Resume retains the record identity and prior totals, and subtracts paused intervals from the aggregation clock. A paused dungeon transition retains both observed floor identities while excluding paused damage and time.

Resume also restarts an ended capture on its previous adapter while retaining the in-memory record. A control-time anchor excludes inactive capture gaps even when no network message follows an earlier resume. New record archives the prior record and starts unpaused. Raw PCAP files intentionally still contain traffic observed during pauses; raw replay without the local control journal can therefore include those events.

Control boundaries are recorded in captures/record-controls.jsonl with microsecond timestamps and character identity. Local capture recovery applies them chronologically so paused health changes are not restored as dungeon damage during warmup. Capture restart preserves the paused state. Explicit controls are applied only to their observed character. Recording storage now permits up to 200 PCAP files within the existing 2.56 GB total byte budget, allowing small files created by recording restarts without deleting retained evidence.

## Equipment contribution images

Dungeon equipment and floor-progression rows show the item's icon and an explicit tier/enchantment badge, including capes, armor and shoes. Item contributions resolve metadata through their historical item index, rather than the character's current weapon. Player rows resolve their observed weapon label. The same icon component appears in the combat source-item table. Unknown equipment retains a question-mark placeholder.

GET /api/items/{index} exposes validated equipment metadata. GET /api/item-icon/{id} uses the local icon cache and accepts only identifiers present in the imported equipment or weapon catalog. Existing weapon-icon URLs remain compatible. Images come from the existing Albion render service and are cached locally; no game-file images are generated or fabricated.

Validation passed 81 backend checks and the frontend checks. Browser integration used isolated storage and loopback UDP traffic to confirm frozen metrics while packet counts continued, correct buttons, the same record after pause/resume and capture stop/restart, and exclusion of paused time. It also verified Thetford Cape T4.3 and Stalker Jacket T4.0 metadata and PNG responses, loaded dungeon weapon/shoe images, visible tier badges and a cape contribution row. See artifacts/paused-record.png and artifacts/dungeon-equipment-icons.png.

History transfer uses **Export history** and **Import history** in Encounters and Dungeons. A record can also be exported individually from its row; a selected dungeon has an **Export dungeon** button. The versioned ZIP contains `history.json` with full snapshots, player names, equipment contributions, timelines, damage, healing and dungeon floors. It does not bundle packet captures or game assets. The importer also accepts the uncompressed JSON.

Imports validate the complete file before writing and commit all new records in a single SQLite transaction. Existing identifiers are skipped without replacement. Imports do not evict local records when the encounter or dungeon retention limit has been reached; the result reports skipped records. The upload limit is 40 MB and decompressed JSON is limited to 128 MB. Imported records are completed snapshots and never replace the active capture.

The timeline reserves a separate analysis column beside the chart. On smaller screens the analysis appears below the chart. No floating contributor tooltip covers the plot. The chart fills the available height beside the analysis. Its vertical cursor starts at the latest visible interval and can be dragged, or moved with Left/Right arrows, Shift+arrows (ten intervals), Home and End. Arrow navigation also works from the analysis area while leaving text fields, selects and buttons to their normal keyboard behavior. Escape or the follow-latest button returns the cursor to the latest interval.

The analysis lists the whole party including zero contributions, with compact item icons, names, damage, healing, DPS/HPS and shares. Expand a player to inspect equipment contributions and attack types. Search, scrolling, sorting and the entire-record scope remain available. Timeline visibility filters remain independent of the analysis list. These controls are shared by the live dashboard, archived encounters, dungeon totals and individual floors.

### Current storage policy

The server no longer records live PCAP segments, creates sidecar capture reports or replays old packet files on startup. Normal capture processes frames in memory and persists only processed combat/player/equipment data. Retained identity fingerprints, sightings and skill mappings support actual player/source attribution. The unused diagnostic identity timeline is saved only when `Capture__Debug=true`. Generated legacy PCAP files/reports are removed when normal capture starts; stored encounters, dungeons and character/party context are preserved. Returning to normal mode drops the debug-only `packets` table. Imported PCAP files are temporary and are removed after analysis.

Debug raw frames use the SQLite `packets` table, with batched inserts and retention limits of 10,000 frames / 32 MB of payloads. Packet storage is independent of capture backend. The Data page lists socket and Npcap/libpcap methods separately, validates that an interface belongs to its method, and persists the selected interface. Linux socket capture uses AF_PACKET/SOCK_DGRAM and emits raw IPv4 frames to the existing decoder; Windows retains its receive-all IPv4 socket. Both filter to Albion UDP ports and IPv4 fragments, without packet injection.

### Manual encounter lifecycle

Live `CaptureAnalysis` uses manual recording mode. Capture initialization freezes the aggregation clock and health processing in idle state, while character, party and equipment observations remain available. No dungeon run is created before Play. Play starts a fresh combat identity, clears every metric/source contribution and initializes a new dungeon record from the currently observed location. Pause/Resume retain the identity and exclude paused health updates and elapsed time. Stop freezes aggregation, finalizes dungeon segments, stops the capture worker and archives the encounter. The next Play resets the current aggregation without deleting prior history.

The server exposes `start-record`, `pause-record`, `resume-record` and `stop-record` session commands. Resume before Play or after Stop is rejected. Stop ends the record but never the capture: the adapter keeps reading, so the next Play keeps every player already identified. Live snapshot status follows the recording lifecycle rather than the adapter state. Automatic interface startup is disabled by default; explicitly starting an adapter or enabling automatic interface observation still leaves recording idle. Verification includes packet-based tests for pre-Play and paused damage/healing exclusion and dungeon resets, plus native socket/Playwright checks for UI controls, network shutdown, distinct encounter identities and saved history.
