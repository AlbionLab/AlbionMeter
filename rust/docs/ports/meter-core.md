# meter-core: Rust port report

Status: done. Parity with the C# holds on every synthetic capture. All gate commands pass. Nothing is committed or staged.

## Gate commands (from `rust/`, `CARGO_TARGET_DIR=target/rust-core`, `$HOME/.cargo/bin` on PATH)

| Command | Result |
|---|---|
| `cargo clippy -p meter-core --all-targets -- -D warnings` | exit 0, no warnings |
| `cargo test -p meter-core` | exit 0, 125 passed, 0 failed (11 test binaries) |
| `cargo bench -p meter-core` | see the bench numbers below |
| `rust/scripts/diff-replay.sh --synthetic` (from the repo root) | MATCH ×4: solo-dungeon, party-dungeon, transport, stress-30min |

## C# → Rust module map

| C# | Rust (`rust/crates/meter-core/src`) |
|---|---|
| PacketCapture.cs `PcapFile` (pcap/pcapng reader), PcapWriter.cs (format only) | `capture/pcap.rs` (`PcapFile::read_into` reuses the frame buffer; `write_pcap` is used by fixtures) |
| Capture/NetworkDecoder.cs | `capture/network.rs` (payloads borrowed; fragment buffers pooled) |
| Capture/PhotonTransport.cs | `capture/photon.rs` (interned connections, structured command keys, reused payload arena, 32k dedup window preallocated) |
| Capture/Protocol18.cs | `capture/protocol18.rs` (zero-copy `Value<'a>`/`WireMessage<'a>` over a reset-per-message bump arena; `decode()` gives an owned copy) |
| Capture/CaptureAnalysis.cs (+ CaptureReport) | `capture/analysis.rs` |
| CombatEngine.cs | `engine.rs` (+ the delta/changes API) |
| CombatSourceCatalog.cs | `catalog.rs` |
| DungeonRun.cs | `dungeon.rs` |
| Observation/CapturedCombatObservation.cs | `observation/combat.rs` |
| Observation/DungeonCombatObservation.cs | `observation/dungeon.rs` |
| Observation/LocalCharacterObservation.cs | `observation/local.rs` |
| Observation/PartyRosterObservation.cs | `observation/roster.rs` |
| Observation/PartyObservation.cs | `observation/party.rs` |
| GameAssetImporter.cs | `importer.rs` (DES-CBC via RustCrypto, gzip via flate2, XML via roxmltree) |
| Server/MapCatalog.cs + importer data types | `maps.rs` |
| Server/RecordControls.cs | `record_controls.rs` |
| .NET semantics (Dictionary order, Guid, DateTimeOffset JSON) | `cs.rs` |
| Test fixtures and synthetic scenarios | `testkit/` |

Examples:
- `examples/replay.rs`: `replay <pcap>`, `replay-session <pcap> [context…]` and `summary <pcap>`.
- `examples/synth.rs`: writes the synthetic pcaps.

Harness: `rust/scripts/diff-replay.sh`. It accepts real pcaps as arguments, or `--synthetic`; `KEEP=1` keeps the JSON. It sorts keys and compares numbers as doubles, because the C# writes `100` where serde writes `100.0`. Random 32-hex record and party ids are renamed by order of first appearance.

How parity is kept: map and array order follows .NET `Dictionary` enumeration, including slot reuse after removals (`CsMap`), so float sums and member and contribution order match. `Guid` layout, format and hash match .NET (colors depend on the hash). The `record ==` reference semantics of the C# are kept where they matter (roster arrays, the `_settled` spells array).

## Tests: C# checks ported

| C# suite | Rust file | Ported |
|---|---|---|
| CaptureChecks | `tests/capture.rs` | 15 core checks + 3 extra |
| ManualRecordingChecks | `tests/manual_recording.rs` | 5/5 + a JSON shape check |
| MembershipGapChecks | `tests/membership_gap.rs` | 7/7 |
| ObservationChecks | `tests/observation.rs`, `tests/sources.rs` | 55/55 (the "Unknown weapon contributions never relabeled" check is in `tests/engine.rs`) |
| Program.cs, core checks | `tests/engine.rs` | 24 engine, dungeon, record-controls, Kind and expedition checks + the delta test |
| GameAssetImporter | `tests/importer.rs` | the C# check + 2 new ones |
| — | `tests/allocations.rs` | the real-time gate (new) |

The repeated "Distinct identical hits" and "Unknown weapon contributions" C# checks are covered once each.

### Not ported, and why

- **Program.cs: belongs to other crates.** EncounterStore (history persistence, empty records, ObserveIdentities ×5, retention, parameterized ids) and HistoryChecks (ZIP/JSON export and import) belong to meter-store. The KnownCharacter, UnidentifiedPlayer and sighting types they need are exported from `meter_core::observation::combat`. WeaponIconCache and AlbionLabService.SuggestTitle belong to meter-lab. MeterSession belongs to meter-app.
- **CaptureChecks: belongs to meter-capture or the app.**
  - Capture recovery (CaptureHistory) belongs to meter-store/meter-capture.
  - The Linux BPF filter and `IsAlbionPacket` checks belong to meter-capture.
  - The native libpcap checks and the libpcap-vs-built-in reader comparison belong to meter-capture.
  - CaptureService replay belongs to app wiring.
- **"(CombatMetric)99 is rejected".** Rust's closed enum cannot express it.
- **Not my crate:** PacketStorageChecks and RaceChecks (server).

## Real-time gates (coordinator message)

### Allocations (`tests/allocations.rs`, counting global allocator)

- **Decode path: 0 allocations over 400 steady-state packets** (moves, buffs, requests, other events), with every observation run.
- **Combat: 0.044 allocations per hit** (20 for 450 hits), with and without the real catalog. All of it is what the record keeps:
  - one timeline bucket per elapsed second;
  - each contribution's first entry in a bucket;
  - growth of the kept event-id and pending lists.

To get there, five things changed:
- Protocol18 is zero-copy over a bump arena.
- Names and ids are shared `Arc<str>`.
- Slot and kind names are interned `&'static str`.
- Contribution keys are cached.
- Source resolution uses bit masks plus a per-effect table of the items that produce each effect.

### Bench (`cargo bench -p meter-core`)

Hardware: this machine, with the real `combat.json` loaded. There are no real captures yet, so the numbers come from the synthetic 30-minute raid (14,778 packets, 5 players + 35 unidentified, 102 floors).
- **Criterion:** 195 ms per replay, **75.7k packets/s**.
- **Latency pass:**

  | p50 | p99 | p99.9 | max | Speed |
  |---|---|---|---|---|
  | 9.3 µs | **87 µs** | 545 µs | 8.2 ms | 7,855× real time |

  The 8.2 ms max is a one-time scan of the 8k items the first time an effect is seen.
- **Decode alone** (IP + Photon + Protocol18): ~2.6M packets/s.

Real pcaps are picked up automatically from `METER_BENCH_PCAPS` or `.dev/data/captures`.

### Memory and speed against the C# tool (same capture)

| | Time | Peak RSS |
|---|---|---|
| Rust `replay summary` (engine state only) | 0.49 s | 32 MB |
| Rust full report | 0.53 s | 41 MB |
| C# `tools/AlbionMeter.Capture replay` | 3.9 s | 226 MB |

### Snapshot/delta API (rustdoc in `engine.rs`)

`CaptureAnalysis::combat_delta(since)` and `CombatEngine::delta(since)` return a `CombatDelta`:
- `sequence`
- `full`
- a snapshot with the totals and all member rows (their rates move with the clock), plus only the changed contribution rows and timeline points
- `changed: CombatChanges`, listing the changed members, the changed and removed weapon/item contributions, and the changed seconds

Timeline points are `Arc`s, copied on write, so nothing is deep-copied per tick and an idle tick touches no rows.

## Divergences from the C# (deliberate)

- **C# crashes, Rust degrades.** Where the C# throws out of `Add` (more than 100 participants, floor or encounter limits, `_roster[member]` after the member left the roster), Rust returns or ignores an `Err`, or falls back to the known name.
- **DateTimeOffset** is kept at ms precision, as the C# derives it. `MapCatalogData.importedAt` is kept as a string.

## Cross-crate requests

1. **Workspace (coordinator).** I added these to meter-core's own Cargo.toml: `bumpalo`, `cbc`, `des`, `flate2`, `roxmltree`, `sha2`, and the `criterion` dev-dependency. Hoist them to `[workspace.dependencies]` if you prefer. `rust/Cargo.lock` changed accordingly.
2. **meter-app.**
   - Poll `combat_delta(last_sequence)` at ≤ 4 Hz and update the rows named in `changed`.
   - `CaptureAnalysis::snapshot()` builds the full C# report, including dungeon aggregates; use it for saving and export, not per tick.
   - The engine thread should own the `CaptureAnalysis` and share the catalog as an `Arc<CombatSourceCatalog>`, which is Send + Sync.
3. **meter-store.** The identity types (`KnownCharacter`, `UnidentifiedPlayer`, `IdentitySighting`, `IdentityTimelineEntry`, `KnownPlayer`) are serde with C# field names. `CaptureReport` carries them in its skipped fields (characters, unidentified, sightings, skill weapons, timeline), as the C# `[JsonIgnore]` ones do.
4. **meter-capture.** Nothing required. The pcap file reader lives in core, per the README.

## Left to do

- **Run the harness on real captures** as soon as some exist: `rust/scripts/diff-replay.sh <pcap>...` and `METER_BENCH_PCAPS=… cargo bench -p meter-core`.
- **Optional: remove the first-effect spike** by precomputing the per-effect producer tables on a background thread when the catalog loads.
