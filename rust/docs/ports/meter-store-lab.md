# rust-services report: meter-store + meter-lab

## C# → Rust

| C# | Rust |
|---|---|
| EncounterStore.cs (save/list/get/dungeons, retention, schema) | `meter-store/src/encounters.rs` (`EncounterStore`, `EncounterSummary`, `DungeonSummary`) |
| EncounterStore.History.cs | `meter-store/src/history.rs` (`export_history`, `import_history`) |
| EncounterStore.Identity.cs (schema + ALTER migrations, characters, skill_weapons, timeline, reconcile/merge/rekey/label) | `meter-store/src/identity.rs` |
| HistoryArchive.cs + HistoryEndpoints.cs (export/import file format) | `meter-store/src/archive.rs` (`HistoryArchive::write/read/validate`, ZIP with one `history.json`, BOM, limits) |
| CaptureHistory.cs | `meter-store/src/capture_history.rs` (`local_capture_files`); also `CaptureSettings` for capture-settings.json (from CaptureService.cs) |
| CharacterContextStore.cs | `meter-store/src/character_context.rs` (local-character.json, local-party.json, known-players.json) |
| Program.cs storage rules | `meter-store/src/paths.rs` (`storage_directory`: explicit > portable `data` next to the exe > LocalApplicationData/AlbionMeter) |
| System.Text.Json output / DateTimeOffset forms | `meter-store/src/json.rs` (C#-identical escaping and double format), `meter-store/src/time.rs` |
| AlbionLab/SessionProtocol.cs | `meter-lab/src/protocol.rs` (wire types + constants) |
| AlbionLab/SessionCompactor.cs | `meter-lab/src/compactor.rs` |
| AlbionLab/AlbionLabService.cs | `meter-lab/src/service.rs` (`AlbionLabService`, `LabContext` trait, `Settings`, `suggest_title`, `place`) |
| Microsoft.AspNetCore.SignalR.Client (what was used) | `meter-lab/src/signalr.rs` (`HubConnection`) |
| WeaponIconCache.cs | `meter-lab/src/icons.rs` |

## Done
- Same file names, the exact schema and migration statements (sqlite_master identical to a C#-created DB), same JSON files.
- JSON is byte-identical to the C# output: the `\uXXXX` escaping, .NET shortest doubles (`5`, `1E+15`), dates unescaped when typed-serialized and `+` after a JsonNode rewrite (identity merges), as C# does.
- Snapshots are `serde_json::Value`; writes take `impl Serialize` and typed reads `DeserializeOwned`, so moving to meter-core types only changes type parameters. Note: serde_json `preserve_order` + `float_roundtrip` are enabled.
- Race fixes from 9d612a7: one lock never held across an await; PATCH/publish answers apply only if `session.id` is still the one they were sent for; settings load under the lock (at construction); an icon request removes only its own pending entry (`Arc::ptr_eq`).
- SignalR: negotiate (bearer, Azure redirect, 401 → `Unauthorized`), WebSocket with an Authorization header (the AlbionLab DeviceAuthentication accepts it), handshake, invocation/completion, ping every 15 s, 30 s server timeout, the close message (`allowReconnect`), reconnect 0/2/10/30 s, an `on_reconnected` callback that resets the compactor.
- Icons: one download per id at a time (OnceCell per id), the 128/default/64 size fallback, 256 KB limit, PNG signature check, atomic write to `assets/weapon-icons/<id>.png`.

## Tests (no calls to albionlab.app; fake HTTP + WebSocket site on 127.0.0.1)
- meter-store: the fresh schema equals the C# one (`tests/fixtures/schema.json` from the C# EncounterStore); a C# history.zip imports to byte-identical rows; C# imports a Rust export (checked by hand with the harness: 1/1 imported); invalid archives (negative number, version 2, `{}`, junk, `../history.json`, duplicate member, control chars) are rejected with nothing written; 4 identity scenarios (elimination + label, same-zone merge into an empty roster entry, group label + last-candidate elimination, re-save keeps names) give byte-identical snapshots and the same resolutions and known characters as C#; retention; empty saves; skill weapons; context files; capture settings; recordings filter.
- The real `~/.local/share/AlbionMeter/encounters.db` (65 encounters, 18 runs, 3806 characters, 720 skill weapons) is copied to a temp dir and never opened in place (its mtime is still 2026-11-18). list/get/dungeons/get_dungeon/known_characters/known_skill_weapons read correctly, every summary and snapshot re-serializes to the same bytes, and the whole history exports, validates and re-imports. The test is skipped when no DB exists (`METER_REAL_DB` overrides the path).
- meter-lab: the compactor matches the C# deltas over 8 steps (new keys, unchanged → null, membership left/unconfirmed, dungeons + floor encounter, status change, reset, a restart clearing places, completed; banker's rounding as Math.Round); SuggestTitle matches the C# output in 7 cases, including the 80-unit cut; SignalR framing (handshake, invocation JSON, completion result/error, pings and split frames, reconnect after a close that allows it and after the socket drops, a final close, 401); settings round-trip byte-identical to the C# file, mode 0600; the settings race; the PATCH-after-replace race; publish while disconnected; a session gone on the site; create backoff; a revoked token; pairing; the publish loop sending "completed" on stop; icons downloaded once under 96 concurrent requests; a failing icon never downloaded twice at once; the size fallback and disk cache.
- C# reference outputs came from a throwaway harness outside the repo (`dotnet` console referencing src/AlbionMeter.Server); its outputs are in `crates/*/tests/fixtures`. Building it wrote only to the ignored `src/*/bin` folders.

## Commands (from rust/, CARGO_TARGET_DIR=target/rust-services)
- `cargo clippy -p meter-store --all-targets -- -D warnings` → exit 0
- `cargo clippy -p meter-lab --all-targets -- -D warnings` → exit 0
- `cargo test -p meter-store` → exit 0: 3 unit + 4 csharp_parity + 1 existing_data (~33 s, debug) + 5 store tests pass
- `cargo test -p meter-lab` → exit 0: 15 unit + 2 compactor parity tests pass (run 3 times, no flakes)

## Missing / differences
- `SessionState.Apply/Summarize/AnonymizedPlayers` (the site-side half of SessionProtocol.cs) is not ported: the meter never uses it, and the AlbionLab repo's copy has already moved on (clocks, places).
- History import keeps the archive's JSON as it is; C# re-serializes it through typed records, which drops unknown fields. Once meter-core's types exist, importing through them restores that. Validation runs on the JSON (missing numbers count as 0, as the C# defaults).
- When the connection closes under a pending "completed", it stays pending and is retried. In C#, that exception escaped to the loop's catch-all.
- No keep-alive timing test (the 15 s/30 s timers are not shortened for tests).
- The UI-facing HTTP endpoints (/api/history/export etc.) are replaced by direct calls. The meter-app wiring should save the current session/dungeon before an export, as HistoryEndpoints did.

## Cross-crate requests
- meter-core: types for `CombatSnapshot`, `DungeonRunSnapshot`, `LocalCharacterSnapshot`, `PartyRosterSnapshot`, `KnownPlayer`, `KnownCharacter`, `UnidentifiedPlayer`, `IdentitySighting` and `IdentityTimelineEntry`, with C#-identical JSON. Until then meter-store defines the identity inputs itself (`meter_store::{KnownCharacter, UnidentifiedPlayer, ...}`) and they can be re-exported or swapped later.
- meter-app (integration): implement `meter_lab::LabContext` (current record, dungeon runs, character id + map, `CombatSourceCatalog.FindItem(index).Id` / `ItemIdByName`, `MapCatalog.WeaponFamily`); run `AlbionLabService::run(cancel)` on the tokio runtime and cancel it on exit (it then ends the session within 3 s); build `WeaponIconCache::new(storage, known)` with the catalog check; open `EncounterStore::open(storage_directory(None), debug)`.
- Workspace: `rust/Cargo.lock` changed (new deps: rusqlite bundled, zip, uuid, reqwest rustls, tokio, tokio-tungstenite, tokio-util, sha2, indexmap, url, gethostname). serde_json gets `preserve_order` + `float_roundtrip` workspace-wide through feature unification.
