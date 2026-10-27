# Party and zone investigation

## Floor attribution during zone arrival, 2026-10-25

Dungeon observation now defers traffic from a destination connection until its successful local Join identifies the floor. An observed dungeon-entry request or successful zone-leave response also starts deferral when the connection is reused. The queue holds at most 8,192 messages from the preceding five seconds; only messages from the confirmed destination connection are replayed. Stop, new-record and capture-gap boundaries discard the queue.

The previous floor is checkpointed before the destination messages are replayed into the new floor's encounter. Both already-named and subsequently identified actors retain damage, healing and equipment context, without relabeling contributions on the previous floor. Pause state is retained with each deferred observation. Messages from a remembered connection after a capture gap are not treated as destination evidence without a transition. An explicitly rejected entrance restores deferred messages to the current floor and cancels the pending transition. An observed transition also takes precedence over a recently opened connection when expiring old object identities.

Combat attribution now checks membership at the effect's timestamp even when its actor is currently listed in the party, so replayed or delayed effects from before a later join remain excluded. Regressions cover different and reused connections, named and late-identified actors, paused arrivals, delayed outside-party damage, unchanged totals after repeated checkpointing, and the existing manual-recording and capture-gap checks.

These changes are validated with deterministic observations, not a reproduction of the original live packet stream. The running server was not restarted or updated in memory. Existing saved encounters are not reconstructed.

## Additional membership gaps, 2026-10-25

Further regressions reproduced two more ways to lose pending contributions. When health effects arrived before an actor was identified, identification discarded them if party membership was not yet proven. Identified effects now use the same bounded membership wait as effects from actors already named; a later full list or party equipment report can credit both damage and healing. Effects already proven outside the party are still excluded when the actor joins later.

Zone confirmation also flushed unidentified effects and equipment already arriving on the destination connection. Those observations are now partitioned by connection and visit boundary before the old visit is flushed, then restored for identification in the destination visit. Preservation requires the local character's observed join at that timestamp; a join without that confirmation does not establish which earlier effects belong to the destination. Floor attribution is handled separately by the destination-message deferral described above.

The character header and performance table used the last incremental roster edit as proof of current membership, while the decoder used the original full-list timestamp. They now share the decoder's confirmation predicate. A departure from a remembered roster no longer confirms every remaining member after a capture gap. Explicit user confirmation remains supported.

These changes do not update the running process, interrupt capture, or rewrite saved encounters. Live packet validation remains unavailable in this environment.

## WaitMyPool membership investigation, 2026-10-25

The persisted party includes WaitMyPool, and identity sightings name object 268 in the latest dungeon visit. The previous floor records 776,788 damage for this character, while the latest saved floor omits it. These records establish a mismatch between remembered membership and credited combat, but normal capture mode retains no packets to establish the exact live event sequence.

Deterministic regressions exposed two membership defects. Zone confirmation preserved character announcements arriving on the destination connection before the Join response, but erased event 670 party-object evidence arriving in that same interval. A named actor could therefore remain waiting for membership despite having already been proven a party member. Party-object evidence now retains its observation timestamp and survives only when it belongs to the destination connection and the new visit. Repeated evidence refreshes its timestamp; old connections and old visits do not establish membership for reused object ids.

Event 244 also recorded membership evidence without settling effects already waiting for that evidence. It now settles those effects immediately after applying the reported equipment. Regression coverage verifies immediate recovery, unchanged totals after flushing, and exclusion of stale party-object evidence on both different and reused connections.

This is a source correction. The running server has no code reload or live membership correction endpoint; changing its assembly files or persisted roster does not update its in-memory decoder. No server restart, capture interruption, or historical data rewrite was performed. The original live packet sequence remains unverified.

## Status and sequencing

Requested after completion and validation of the meter. This document records preliminary source inspection and the investigation acceptance criteria. It does not claim packet capture, protocol reverse engineering, or reproduction of the reported defect has been completed.

Validation on 2026-10-23: C# build passed with zero warnings and errors; 26 C# checks and 13 frontend checks passed; TypeScript and frontend bundle passed. The browser integration test could not start Kestrel because socket creation returned Permission denied. Windows execution, real traffic, packet decoding and automatic zone transitions remain unvalidated. Existing data is synthetic.

## Reported symptom

The reference meter sometimes recovers after leaving or rejoining the party. The exact application version, activity, capture adapter and event sequence are not yet established. Recovery after a party change is evidence to investigate state reconstruction; it does not identify the failing component.

## Preliminary source observations

Inspected the reference main branch through public source URLs on 2026-10-23; this is not a pinned checkout or necessarily the version used when the bug occurred.

[EntityController.cs](https://github.com/Triky313/AlbionOnline-StatisticsAnalysis/blob/main/src/StatisticsAnalysisTool/Network/Manager/EntityController.cs) maintains character GUIDs and object-ID mappings separately. AddEntity preserves an existing object ID when an update omits it. SetPartyAsync resets membership before rebuilding it. Removing the local member resets party membership and adds the local entity back. RemoveEntitiesByLastUpdate also clears object-ID mappings for all remaining nonlocal entities in a second loop. Its call sites and ordering must be investigated before calling this incorrect. These mechanisms make stale mappings and incomplete membership reconstruction plausible hypotheses, not confirmed causes.

[PartyController.cs](https://github.com/Triky313/AlbionOnline-StatisticsAnalysis/blob/main/src/StatisticsAnalysisTool/Party/PartyController.cs) derives the displayed party from entities marked in-party and reads equipment from those entities. Displayed membership therefore depends on upstream entity state.

## Required investigation

Use passive observation of the local client's traffic and deterministic offline replay. Preserve the application version, game version, capture provider, interface, region, event ordering and source mapping revision with each reproduction. Do not use a synthetic trace to claim a real protocol defect has been reproduced.

| Scenario | Evidence to collect | Required meter behavior |
| --- | --- | --- |
| Start capture while already grouped | First observed membership and entity events | Show incomplete roster until there is sufficient evidence |
| Change zone with party unchanged | Transition boundary, character identity and new object bindings | Preserve roster, invalidate old-zone bindings |
| Reuse an object ID after zoning | Old/new identity, session and zone generation | Never attribute a new character's hits to the old character |
| Receive combat before identity | Arrival order and late identity resolution | Bounded pending buffer with explicit unresolved counters |
| Lose party membership events | Capture drops, decode errors and subsequent full roster | Reconcile from a verified snapshot without inventing members |
| Leave and rejoin party | Full sequence before and after reported recovery | Explain which state was rebuilt and retain historical attribution |
| HCE or repeated party reconstruction | Snapshot/delta ordering and missing object IDs | Membership changes must not accidentally erase valid current-zone bindings |
| Revisit a dungeon floor | Instance identity and transition sequence | Reuse the recorded floor without duplicating contributions |
| Reconnect or change character | New connection identity and local character GUID | Isolate sessions and prevent old events entering the new state |
| Party member outside observation | Membership versus locally observed events | Distinguish unobserved activity from measured zero |
| Retransmission or out-of-order fragments | Transport identity and reassembly diagnostics | Deduplicate transport copies while preserving distinct identical hits |

## Intended design

Keep persistent character identity, party membership, transient entity bindings, weapon observations and dungeon navigation in separate state components. Scope transient bindings to a connection and zone generation. Process normalized state changes serially; publish immutable snapshots to the UI. A concurrent dictionary alone does not make a multistep state transition atomic.

Store the observed weapon at contribution time when available. Later weapon changes must not relabel earlier damage. Membership must likewise be evaluated at the event's time rather than using only the current party filter.

Maintain bounded diagnostics for packet counts, capture drops reported by the adapter, parser failures, unknown event codes, unresolved entities, rejected stale events and pending-buffer expiry. A sequence gap alone is not proof of capture loss. Preserve uncertainty instead of silently manufacturing missing damage or identities.

Provide recovery from subsequent observed authoritative state. A passive meter cannot force the server to resend information or reconstruct packets it never received. A local reset can clear bad bindings, but it must label the roster as incomplete until new observations establish it.

## Completion gate

Before claiming the reported bug is fixed: obtain a reproducible trace from the affected version, demonstrate the failure in replay, add a regression test, implement the correction, and show that replay no longer requires leaving the party. Validate Windows and Linux capture separately, then exercise zones, floors, reconnection and packet loss against the same diagnostics. Keep unsupported event versions visible rather than falling back silently to demo data.

## Implementation progress

A native capture/replay pipeline and a separately tested party observation model now exist. The model has regression tests for zone invalidation, party reconstruction without erasing object bindings, stale identity updates, object-ID reuse and atomic roster replacement. These are synthetic reproductions of failure classes, not confirmation that the reference application's reported bug has been reproduced or fixed. See [capture-and-replay.md](capture-and-replay.md) for current capabilities and limits.

## Local installation inspection on 2026-10-23

The installation at `/opt/albion-online-launcher-bin/game_x64` reports `albiononline-linux-full-1.32.030.345319`. Its `GameAssembly.so` is a stripped ELF x86-64 shared object. `Albion-Online_Data/il2cpp_data/Metadata/global-metadata.dat` has the IL2CPP metadata magic and version 39. This establishes a native Linux IL2CPP installation, not the location of a Windows client running under Proton. No running Albion client or separate Proton installation was identified in the inspected process and installation paths.

The metadata contains null-terminated names for HealthUpdate, HealthUpdates, PartyJoined, PartyPlayerJoined, PartyPlayerLeft, ChangeCluster, EventCodes and OperationCodes. ScriptingAssemblies.json lists Albion.Common.dll, Albion.PhotonClient.dll and Photon3Unity3D.dll. These are useful investigation leads; string presence does not establish enum values, field layouts or executable method behavior. No complete decompilation has been performed. The local inventory and metadata fingerprint are in `artifacts/investigation/local-client.json`.

GameData contains items.bin, spells.bin, localization.bin, randomdungeons.bin, staticdungeons.bin and dynamicclusters.bin. The existing importer already reads local map and weapon definitions. Further work should correlate spell identifiers and dungeon templates with observed network identifiers; a template alone cannot identify a particular dungeon run or its visited floors.

Reference source inspection is now pinned to revision `e0d2fb862805c039458a83c83f4a91b3d6fcd0b6`. Its HealthUpdate model maps target, time, health delta, resulting health, effect type, effect origin, source and spell to parameters 0 through 7. HealthUpdates represents several updates with parallel indexed collections. PartyJoined reads player identifiers and names from parameters 8 and 9. These mappings must be validated against the installed client and actual captures before live attribution is enabled. In particular, incomplete parallel collections must not silently manufacture a source identifier or zero-valued contribution.

The earlier validation paragraph is historical: real capture and browser validation now work. The captured session contains 20,183 frames and 55,672 successfully decoded wire messages; it includes the local character join that established the displayed identity and map. Decoding a wire message is not equivalent to correctly interpreting its combat semantics. Live party and combat integration and reproduction of the reported reference-meter bug remain incomplete.

## Capture schema inspection

The capture CLI now supports `inspect <pcap-file>`. It examines server-to-client messages, aggregates parameter types and collection lengths by event code, and emits a bounded set of numeric samples for candidate health events. Membership samples for event codes 230–234 include player names and identifiers; keep these reports local. Other arbitrary string payloads are not included. This is a schema investigation tool, not a combat meter or a source of party totals.

Replaying the recorded session produced byte-identical inspection reports on two consecutive runs. Event shape counts reconcile with their message totals. The report is stored locally at `artifacts/investigation/packet-shapes.json`.

| Observation | Count | Implication |
| --- | ---: | --- |
| Recorded frames | 20,183 | Complete stored input for this investigation |
| Incoming UDP datagrams on game ports | 13,250 | Direction-filtered input |
| Successfully decoded incoming wire messages | 55,643 | Structural parsing, not semantic combat validation |
| Incoming encrypted messages | 69 | Not decoded |
| Incoming unsupported message kinds | 2 | Not interpreted |
| Duplicate incoming transport commands rejected | 94 | Avoid counting retransmitted commands twice |
| Candidate HealthUpdate messages, event 6 | 1,175 | 1,143 include all fields 0–7; 22 omit field 3; 10 omit field 2 |
| Candidate HealthUpdates messages, event 7 | 141 | 126 batches of 2, 14 of 3, 1 of 4: 298 indexed observations |
| NewCharacter messages, event 29 | 683 | Observation count, not distinct characters or party size |

The individual samples include three consecutive deltas of -56 with resulting health 127, 71 and 15 for the same target, followed by -56 with no resulting-health field. The missing field must not automatically be interpreted as zero or used as proof of a kill. Batch samples include distinct spell indices sharing a timestamp, and explicit zero deltas. Deduplication by timestamp, source and amount could therefore discard legitimate observations. Preserve the transport identity and batch position instead.

No event codes named Party in the pinned reference enum appear in this capture. This does not prove that the local player was ungrouped: capture timing, missing observations and version mismatches must still be considered. A controlled trace with join, damage, healing, zone change, leave and rejoin is required to validate party reconstruction. Health changes from nearby entities cannot be treated as party contributions until identity and membership are established.

## Observed two-member party

A subsequent live capture contains event 230 with 32 bytes in parameter 8 and two names in parameter 9. The identities match the local character and the party member shown in the user's screenshot. The roster decoder now reads this observed representation, validates array alignment and distinct identities, and applies join/leave/disband updates. Malformed snapshots do not replace known membership. Zone transitions retain the roster; character changes clear it. Nearby NewCharacter observations do not add party members.

The dashboard displays this roster separately from combat totals. Cached membership is explicitly marked last observed after restarting capture. Player island identifiers now display as Player island; ownership is not inferred from the local character name. The two-member trace validates initial roster decoding. Leave/rejoin and zone behavior currently have synthetic regression coverage and still require controlled real traces. Tests passed: 61 backend checks plus the browser integration suite, and a browser check of the real observed roster and island label.

## Initial captured combat integration

The capture pipeline now feeds identified local-character and observed-party health changes into CombatEngine. Event 6 is decoded as one observation and event 7 as indexed observations. Deduplication uses the Photon message identity plus the batch position. Raw negative deltas are recorded as damage; positive deltas remain observed health changes, not validated effective healing. Missing deltas are rejected. Unresolved actors and nonparty actors are excluded and counted in diagnostics. Known NewCharacter and NewMob bindings distinguish target categories in diagnostics.

The real duel and subsequent mob hits reconcile to 3,184 damage by MistressMei and 685 by KPey (291 against the other player, plus 84 and 310 against mobs). The 49 observations sum to 3,869 across the timeline. A finishing hit of 77 with 24 remaining health demonstrates why reported damage must not be labeled effective health loss. Target 375 was previously observed through NewMob with type index 424; its health event reports -310 from local object 393, spell index 3343.

A confirmed ordering issue was corrected: entities can arrive on the destination connection before the local Join response. Confirmation now preserves bindings already observed on that same connection while removing bindings from other connections. Equipment events arriving shortly before identity can be held for up to 15 seconds in a bounded buffer. Event 90 updates main-hand equipment through the scoped object binding and character GUID. Local item indexing includes enchantment entries in XML order. In this installation indices 7108 and 9022 resolve to Blazing Staff T4.3 and Hammer T6.0.

Live start can recover context by processing up to three recent local live-capture files; imported replay files are excluded. Historical contributions are cleared before new capture begins. Context frame counts are reported separately. This reconstruction cannot recover missed events between captures. The capture publishes snapshots up to five times per second, while graph buckets use packet arrival time at one-second resolution. It does not yet reconstruct server-clock timing, automatically split encounters/floors, or implement bounded delayed combat attribution for unresolved actors. Live records are saved without inferred dungeon locations. The ten-minute recording limit remains in effect.

## Restart and zone regression reproduced

The previous three-file recovery window could omit the original PartyJoined event. Imported replay files also consumed positions in that window. The character header fell back to its persisted roster while the combat decoder had no roster, so known party actors were classified outsideParty. The latest-file warmup was a defect in this meter; it does not establish the cause of a reference-project defect.

Recovery now processes all retained local live recordings in chronological file order, excludes imported replays, and accepts recordings up to the recorder's 128 MB file limit. A regression check ensures an older local file remains selected with more than three newer files. Another verifies damage attribution across capture reset and zone rebinding without another PartyJoined, plus preservation of prior contribution when a member leaves and returns.

A real-capture replay stopped before the user's leave/rejoin sequence reproduces the failure with the incomplete context: KPey 184, MistressMei missing, 49 outsideParty events. With complete context the same interval yields KPey 184 and MistressMei 2,780, with 72 attributed player damage observations. No new party join is included in the tested interval. Missing or deleted recording history still cannot establish an unobserved roster; the recovery is bounded by retained evidence, not a guarantee across capture gaps.

The graph now enables newly discovered series within the same encounter while preserving explicit toggles for existing series. Clipboard exports retain zero-contribution members and handle an all-zero total without NaN percentages or division by zero.

## Weapon-at-hit attribution

Combat events now carry the observed main-hand item index at processing time. CombatEngine stores separate totals and sparse per-second buckets keyed by character and weapon. Returning to a previously used weapon adds to its existing contribution without relabeling prior hits. Unknown equipment and explicitly unarmed equipment remain distinct. Positive health changes use the same attribution. Distinct swaps in one second remain separate bucket entries.

Snapshots and saved encounters preserve weapon contributions. The interface supports By player and By player + weapon, per-second weapon details in the tooltip, and an encounter-wide comparison table. Historical records without this attribution are not reconstructed from their final equipment. This measures the weapon observed when the event arrived; attribution to a spell originally cast before a swap is not established, and unobserved or reordered equipment events remain a limitation. Comparison totals alone do not establish causal weapon effectiveness or normalize for boss mechanics, time equipped or buffs.

Validation: 68 backend checks include weapon swapping, returning to a previous weapon, same-second damage/healing, duplicate rejection, immutable bucket snapshots and unknown-weapon preservation. Replaying the captured duel and mob hits reconciles both weapon totals and every weapon bucket with player totals (3,869 damage).

## Spell-linked source items

The local equipment and spell catalogs now retain 8,151 equipment entries and 9,326 indexed spell entries. Spell indexing skips color tags and reserves the generated channeling entry. Equipment event 90 supplies equipment in parameter 2 and active spells in parameter 7. Source resolution follows linked spell effects from the observed active slot to the damage spell; ambiguous and unsupported effects remain unknown.

The captured Stalker Jacket test links STORMSHIELD (3952) to STORMSHIELD_EFFECT (3953), with chest item 4308. Its 22 observed pulses of 32 produce 704 chest damage. Hammer skills and basic attacks produce 646 main-hand damage. The total remains 1,350. Equipped-weapon context intentionally still includes both sources; source-item totals are a separate decomposition, never extra player damage.

The interface supports source-item lines, per-player tooltip breakdowns, encounter item totals and one-second or one-minute buckets. Minute buckets sum damage rather than report DPS. Validation passed 69 backend checks, 15 frontend checks, the browser integration suite, and browser rendering of the recorded 704/646 split. The screenshot is artifacts/item-source-breakdown.png.

Coverage is not complete: unsupported passive/cape effects, missing equipment observations, and effects remaining after equipment swaps need further validation. Equipment is observed at event arrival, not reconstructed at the original cast. Old stored encounters are not retroactively attributed. Individual encounters retain item contributions; dungeon-wide item aggregation has not been validated. Positive health changes remain observed healing rather than verified effective healing.

## Thetford Cape correction

Cape item definitions now retain their craft-spell roots. The effect graph follows applyeffectoneventpassive and chainspell references. Source attribution checks the observed cape in equipment slot 6, preserving unknown results when no equipped item matches or multiple slots match. Ordinary auto-attacks remain main-hand damage.

Replaying capture 740728a1539d4c4e96c7143a22300a19 with retained context identified 455 MistressMei damage as Adept's Thetford Cape T4.3 (item 2637), alongside 124 main-hand damage and 880 still unknown. KPey contributed 41. The combined total remains 1,500. Previously saved summaries are not rewritten. New regression coverage includes missing equipment, wrong cape, ambiguous effects, ordinary attacks and bucket reconciliation; 70 checks passed.

The validation build surfaced an existing NU1903 advisory for SQLitePCLRaw.lib.e_sqlite3 2.1.11. Validation used WarningsNotAsErrors=NU1903 for this invocation; no project audit policy or dependency version was changed. Dependency remediation remains outstanding.

## Ranged auto attacks

The retained 740728a1539d4c4e96c7143a22300a19 capture contains three MistressMei damage events of 32 with causing spell -1 and origin 1. The previous predicate accepted only origin 0 and left these ranged attacks unknown. Both observed origin values now classify negative health changes with spell -1 as autoAttack. Missing origin, different origins and positive health updates do not acquire this classification. Unknown equipment can preserve the attack type without inventing an item.

Source contributions now retain kind (autoAttack, ability, passive or unknown), with distinct totals and timeline keys for attacks and abilities of the same weapon. The replay reconciles to 1,500: KPey auto attacks 41; MistressMei auto attacks 96, Thetford Cape passive 455, identified weapon ability 124, unresolved effects 784. Regression validation passed 71 backend checks and 15 frontend checks. Browser validation confirmed the 96 attack subtotal, source grouping, copied player-only text, PNG clipboard output and unchanged control dimensions after both copy actions. See artifacts/auto-attack-breakdown.png.

Copy text omits its title and total footer. Copy success temporarily replaces the button content with a checkmark, reserves its width and announces success through a visually hidden status. Clipboard errors use an anchored message. The recording count limit now counts PCAP recordings rather than including their diagnostic JSON companions; all retained evidence remains available.

## Ironbreaker and item-level grouping

The KPey capture dc78092d7bd3459b920c54eabbec4fec contains a 171 damage event with spell 3343 (IRONBREAKER_EFFECT), origin 2. IRONBREAKER (3342), observed in the weapon's active slots, refers to it through spelleffectarea's effect attribute. The importer now retains this edge. Replaying the same interval yields 1,401 damage: 320 main-hand auto attacks, 337 main-hand abilities and 744 chest abilities, with no unknown source in this interval. The importer-to-observation regression verifies encrypted fixture import, slot resolution and reconciliation; 72 backend checks passed.

Source-item presentation groups attack, ability and passive details under the same character, item index and equipment slot. The table and item graph show one total per source item. Attack and ability amounts remain available within the row and tooltip. Items and different players remain separate, and minute aggregation preserves their sums. The browser replay shows a single Hammer row and series totaling 657, a Jacket row and series totaling 744, and the ability detail in the chart tooltip. The frontend grouping regression verifies slot/player separation, sparse bucket sums, missing buckets and input immutability. See artifacts/grouped-source-items.png.

## Active, former and recovered membership

The user's solo report at Lymhurst Portal exposed a stale recovered roster. The available b0528557a7fd4a59a159d46ad1f7265c interval has no PartyJoined, PartyDisbanded or PartyPlayerLeft event; retained recordings have a gap between 17:44 and 17:58. This is not evidence that a departure packet was decoded incorrectly. The previously recovered roster was nevertheless presented as currently confirmed membership.

Live player statistics now distinguish active and unconfirmed membership using whether the roster was observed within the current recording. The local character remains active. Recovered unconfirmed members are dimmed and labeled Unconfirmed, and are excluded from the confirmed-member count. This presentation flag does not erase the recovered roster used by combat decoding; an unobserved departure can still require reconciliation. Source-item totals are never used to infer membership.

Former members remain in the performance table, dimmed and labeled Left party. Their past contributions stay intact; rejoining removes the former state and resumes their totals. Header roster information also distinguishes recovered membership from current evidence. The user's explicit solo report was persisted as an empty user-confirmed roster, with its prior JSON preserved at /tmp/albion-party-before-user-confirmation.json. This is a state correction based on the user report, not a fabricated packet observation.

Capture recovery now accepts a newer persisted roster after replay warmup, only when its character matches the recovered local identity. Older or invalid snapshots cannot replace newer membership. Subsequent real party events replace the user-confirmed state. A regression verifies that a newer solo confirmation removes the other member, preserves 100 prior damage, rejects 200 outside-party damage, and restores 150 after rejoining and another 50 damage. Validation passed 73 backend checks, frontend checks and browser membership transitions. See artifacts/left-party-membership.png.
