using AlbionMeter.Core.Capture;
using AlbionMeter.Core.Observation;

internal static class ObservationChecks
{
    public static void Run(Action<string, Action> test, Action<bool> assert)
    {
        test("Party object evidence arriving before the zone join survives without accepting old zone objects", () =>
        {
            var localId = Guid.NewGuid(); var mateId = Guid.NewGuid(); var strangerId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "old", null, "observed", 1, "old");
            var remembered = new PartyRosterSnapshot([new(localId, "Alpha"), new(mateId, "WaitMyPool")], 1, true);
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1); combat.ForgetIdentities(1_000_000);
            void Event(string connection, Dictionary<byte, object?> fields, long at) => combat.Observe(new(4, 1, null, fields), connection, $"event:{at}", at, local, remembered);
            Event("old", new() { [252] = 670, [0] = 403 }, 2_000_000);
            Event("next", new() { [252] = 29, [0] = 402, [1] = "WaitMyPool", [7] = mateId.ToByteArray() }, 5_000_000);
            Event("next", new() { [252] = 670, [0] = 402 }, 5_100_000);
            local = local with { ObjectId = 432, Connection = "next", ObservedAtMicroseconds = 5_300_000 };
            combat.Observe(new(3, 1, 0, new() { [253] = 2, [0] = 432 }), "next", "join", 5_300_000, local, remembered);
            Event("next", new() { [252] = 29, [0] = 403, [1] = "Stranger", [7] = strangerId.ToByteArray() }, 5_400_000);
            Event("next", new() { [252] = 6, [0] = 900, [6] = 402, [2] = -75f }, 6_000_000);
            Event("next", new() { [252] = 6, [0] = 900, [6] = 403, [2] = -200f }, 6_100_000);
            combat.FlushUnidentified();
            assert(combat.Snapshot!.TotalDamage == 75);
            assert(combat.Snapshot.Members.Single(member => member.Id == mateId.ToString()).Damage == 75);
            assert(combat.Snapshot.Members.All(member => member.Id != strangerId.ToString()));
        });
        test("A party equipment report credits damage waiting for membership immediately", () =>
        {
            var localId = Guid.NewGuid(); var mateId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "dungeon", null, "observed", 1, "flow");
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1); combat.ForgetIdentities(1_000_000);
            void Event(Dictionary<byte, object?> fields, long at) => combat.Observe(new(4, 1, null, fields), "flow", $"event:{at}", at, local, null);
            Event(new() { [252] = 29, [0] = 402, [1] = "WaitMyPool", [7] = mateId.ToByteArray() }, 2_000_000);
            Event(new() { [252] = 6, [0] = 900, [6] = 402, [2] = -100f }, 3_000_000);
            assert(combat.Snapshot!.TotalDamage == 0);
            Event(new() { [252] = 244, [0] = mateId.ToByteArray(), [2] = new short[] { 6661, 0, 5347, 5005, 5397, 2900, 2662, 3098 } }, 4_000_000);
            assert(combat.Snapshot!.Members.Single(member => member.Id == mateId.ToString()).Damage == 100);
            Event(new() { [252] = 6, [0] = 900, [6] = 402, [2] = -50f }, 5_000_000);
            assert(combat.Snapshot.TotalDamage == 150);
            combat.FlushUnidentified();
            assert(combat.Snapshot.TotalDamage == 150);
        });
        test("A reused connection keeps refreshed party evidence and expires evidence from its previous visit", () =>
        {
            var localId = Guid.NewGuid(); var mateId = Guid.NewGuid(); var strangerId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "old", null, "observed", 1, "flow");
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1); combat.ForgetIdentities(1_000_000);
            void Event(Dictionary<byte, object?> fields, long at) => combat.Observe(new(4, 1, null, fields), "flow", $"event:{at}", at, local, null);
            Event(new() { [252] = 670, [0] = 402 }, 2_000_000);
            Event(new() { [252] = 670, [0] = 403 }, 2_100_000);
            combat.Observe(new(3, 1, 0, new() { [253] = 41 }), "flow", "leave-zone", 20_000_000, local, null);
            Event(new() { [252] = 29, [0] = 402, [1] = "WaitMyPool", [7] = mateId.ToByteArray() }, 20_100_000);
            Event(new() { [252] = 670, [0] = 402 }, 20_200_000);
            local = local with { ObjectId = 432, ObservedAtMicroseconds = 20_300_000 };
            combat.Observe(new(3, 1, 0, new() { [253] = 2, [0] = 432 }), "flow", "join", 20_300_000, local, null);
            Event(new() { [252] = 29, [0] = 403, [1] = "Stranger", [7] = strangerId.ToByteArray() }, 20_400_000);
            Event(new() { [252] = 6, [0] = 900, [6] = 402, [2] = -75f }, 21_000_000);
            Event(new() { [252] = 6, [0] = 900, [6] = 403, [2] = -200f }, 21_100_000);
            combat.FlushUnidentified();
            assert(combat.Snapshot!.TotalDamage == 75);
            assert(combat.Snapshot.Members.Single(member => member.Id == mateId.ToString()).Damage == 75);
            assert(combat.Snapshot.Members.All(member => member.Id != strangerId.ToString()));
        });
        test("Live pauses freeze totals and exclude paused time while equipment remains observed", () =>
        {
            var id = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 10, "Alpha", null, null, "one", null, "observed", 1, "flow");
            var combat = new CapturedCombatObservation(new AlbionMeter.Core.CombatSourceCatalog(new("test", [], [])));
            combat.BeginSession(1_000_000);
            void Equipment(short index, long at) => combat.Observe(new(4, 1, null, new() { [252] = 90, [0] = 10, [2] = new short[] { index } }), "flow", $"equipment:{at}", at, local, null);
            void Hit(float value, long at) => combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = 10, [2] = value, [7] = -1, [5] = (byte)0 }), "flow", $"hit:{at}", at, local, null);
            Equipment(9022, 1_000_001); Hit(-100, 2_000_000); Hit(40, 3_000_000);
            combat.SetPaused(true, 4_000_000);
            var paused = combat.Snapshot!;
            assert(paused.Status == "paused" && paused.ElapsedSeconds == 3);
            Equipment(7108, 10_000_000); Hit(-900, 11_000_000); Hit(900, 11_000_001);
            assert(combat.Snapshot!.Id == paused.Id && combat.Snapshot!.TotalDamage == 100 && combat.Snapshot!.TotalHealing == 40);
            assert(combat.Snapshot!.EventCount == 2 && combat.Snapshot!.ElapsedSeconds == 3 && combat.Snapshot!.Timeline.Length == paused.Timeline.Length);
            assert(combat.Snapshot!.Members.Single().Weapon == "Item #7108");
            combat.SetPaused(false, 12_000_000); assert(combat.ClockTimestamp == 12_000_000); Hit(-25, 13_000_000);
            var resumed = combat.Snapshot!;
            assert(resumed.Id == paused.Id && resumed.Status == "running" && resumed.TotalDamage == 125 && resumed.TotalHealing == 40);
            assert(resumed.ElapsedSeconds == 4 && resumed.Members.Single().EncounterDps == 31.25);
            assert(resumed.WeaponContributions!.Single(item => item.WeaponIndex == 9022).Damage == 100);
            assert(resumed.WeaponContributions!.Single(item => item.WeaponIndex == 7108).Damage == 25);
            assert(resumed.Timeline.Sum(point => point.Damage.Values.Sum()) == 125);
            combat.SetPaused(true, 14_000_000); combat.SetPaused(false, 16_000_000); assert(combat.ClockTimestamp == 16_000_000); Hit(-10, 17_000_000);
            assert(combat.Snapshot!.ElapsedSeconds == 6 && combat.Snapshot!.TotalDamage == 135);
            combat.BeginSession(18_000_000);
            assert(!combat.IsPaused && combat.Snapshot!.Id != paused.Id && combat.Snapshot!.TotalDamage == 0);
        });
        test("Paused dungeon transitions keep floor identities without counting paused damage or time", () =>
        {
            var id = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 10, "Alpha", null, null, "one", null, "observed", 1, "flow");
            var dungeon = new DungeonCombatObservation();
            void Join(string map, long at)
            {
                local = local with { MapId = map };
                dungeon.Observe(new(3, 1, 0, new() { [253] = 2, [1] = id.ToByteArray(), [8] = map, [66] = "1315" }), "flow", $"join:{at}", at, local, null);
            }
            void Hit(float value, long at) => dungeon.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = 10, [2] = value }), "flow", $"hit:{at}", at, local, null);
            Join("@RANDOMDUNGEON@one", 1_000_000); Hit(20, 1_500_000); Hit(-100, 2_000_000);
            dungeon.SetPaused(true, 3_000_000);
            Join("@RANDOMDUNGEON@two", 8_000_000); Hit(-900, 9_000_000);
            var paused = dungeon.Snapshot().Single();
            assert(paused.Status == "paused" && paused.Floors.Length == 2 && paused.Totals.TotalDamage == 100);
            assert(paused.Floors[0].Totals.ElapsedSeconds == 2 && paused.Floors[1].Totals.ElapsedSeconds == 0);
            dungeon.SetPaused(false, 11_000_000); Hit(-50, 12_000_000);
            var resumed = dungeon.Snapshot().Single();
            assert(resumed.Status == "running" && resumed.Totals.TotalDamage == 150 && resumed.Totals.TotalHealing == 20);
            assert(resumed.Totals.ElapsedSeconds == 3 && resumed.Floors[1].Totals.ElapsedSeconds == 1);
        });
        test("After a server change, a party object is named by its weapon only when one waiting member carries it", () =>
        {
            CapturedCombatObservation Run(bool betaSameWeapon)
            {
                var localId = Guid.NewGuid(); var betaId = Guid.NewGuid(); var gammaId = Guid.NewGuid();
                var local = new LocalCharacterSnapshot(localId, 1, "Alpha", null, null, "one", null, "observed", 1, "flow");
                var roster = new PartyRosterSnapshot([new(localId, "Alpha"), new(betaId, "Beta"), new(gammaId, "Gamma")], 5, true);
                var combat = new CapturedCombatObservation();
                combat.BeginSession(1);
                combat.Observe(new(4, 1, null, new() { [252] = 29, [0] = 900, [1] = "Gamma", [7] = gammaId.ToByteArray(), [40] = new short[] { 7696, 0, 4224, 5452, 5472, 2909, 2587, 3098 } }), "flow", "gamma-seen", 2, local, roster);
                if (betaSameWeapon) combat.Observe(new(4, 1, null, new() { [252] = 29, [0] = 901, [1] = "Beta", [7] = betaId.ToByteArray(), [40] = new short[] { 7696, 0, 1111, 2222, 3333, 2909, 2587, 3098 } }), "flow", "beta-seen", 3, local, roster);
                // A new route to the game server: every object id changes and nobody in sight is announced again.
                combat.Observe(new(3, 1, 0, new() { [253] = 2 }), "flow", "zone", 4, local, roster);
                combat.ForgetIdentities(5);
                var hits = 0;
                void Hit(long source, float value, long at) => combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = source, [2] = value, [7] = 1 }), "flow", $"hit:{hits++}", at, local, roster);
                Hit(21, -250, 1_000_000); Hit(21, -50, 2_000_000);
                combat.Observe(new(4, 1, null, new() { [252] = 670, [0] = 21 }), "flow", "party-object", 2_100_000, local, roster);
                combat.Observe(new(4, 1, null, new() { [252] = 90, [0] = 21, [2] = new short[] { 7696, 0, 0, 0, 0, 0, 0, 0, 0, 0 } }), "flow", "gear", 2_200_000, local, roster);
                combat.FlushUnidentified();
                return combat;
            }
            var unique = Run(betaSameWeapon: false);
            assert(unique.Snapshot!.Members.Single(member => member.Name == "Gamma").Damage == 300 && unique.Diagnostics["identifiedByWeapon"] == 1);
            assert(!unique.Snapshot!.Members.Any(member => member.Name.StartsWith("Unidentified", StringComparison.Ordinal)));
            var shared = Run(betaSameWeapon: true);
            assert(!shared.Diagnostics.ContainsKey("identifiedByWeapon") && shared.Snapshot!.Members.All(member => member.Name is not ("Gamma" or "Beta") || member.Damage == 0));
        });
        test("Effects from unnamed party members are credited once chat, equipment or own skills identify them", () =>
        {
            var localId = Guid.NewGuid(); var chatterId = Guid.NewGuid(); var tankId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 1, "Alpha", null, null, "one", null, "observed", 1, "flow");
            var roster = new PartyRosterSnapshot([new(localId, "Alpha"), new(chatterId, "Beta"), new(tankId, "Gamma")], 5, true);
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1);
            combat.Observe(new(4, 1, null, new() { [252] = 29, [0] = 900, [1] = "Gamma", [7] = tankId.ToByteArray(), [40] = new short[] { 7696, 0, 4224, 5452, 5472, 2909, 2587, 3098 } }), "flow", "seen-before", 2, local, roster);
            combat.Observe(new(3, 1, 0, new() { [253] = 2 }), "flow", "zone", 3, local, roster);
            combat.ForgetIdentities(4);
            var hits = 0;
            void Hit(long source, float value, long at, short spell = 1) => combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = source, [2] = value, [7] = spell }), "flow", $"hit:{hits++}", at, local, roster);
            Hit(20, -100, 1_000_000); Hit(20, -50, 2_000_000); Hit(21, -300, 2_100_000); Hit(22, 80, 2_200_000);
            assert(combat.Diagnostics["pendingUnidentified"] == 4 && combat.Snapshot!.TotalDamage == 0);
            combat.Observe(new(4, 1, null, new() { [252] = 74, [0] = "beta", [1] = "hello", [4] = 20, [5] = chatterId.ToByteArray() }), "flow", "chat", 3_000_000, local, roster);
            assert(combat.Snapshot!.Members.Single(member => member.Name == "Beta").Damage == 150 && combat.Diagnostics["identifiedByChat"] == 1);
            combat.Observe(new(4, 1, null, new() { [252] = 244, [0] = tankId.ToByteArray(), [2] = new short[] { 7696, 0, 4224, 5452, 5472, 2909, 2587, 3098, 0, 0 } }), "flow", "party-gear", 3_050_000, local, roster);
            combat.Observe(new(4, 1, null, new() { [252] = 670, [0] = 21 }), "flow", "party-object", 3_060_000, local, roster);
            combat.Observe(new(4, 1, null, new() { [252] = 90, [0] = 21, [2] = new short[] { 7696, 0, 4224, 5452, 5472, 2909, 2587, 3098, 0, 0 } }), "flow", "gear", 3_100_000, local, roster);
            assert(combat.Snapshot!.Members.Single(member => member.Name == "Gamma").Damage == 300 && combat.Diagnostics["identifiedByEquipment"] == 1);
            for (var cast = 0; cast < 5; cast++) Hit(22, 90, 4_200_000 + cast * 2_000_000, 3404);
            assert(combat.Snapshot!.Members.Single(member => member.Name == "Alpha").Healing == 0 && !combat.Diagnostics.ContainsKey("identifiedByOwnCast"));
            combat.ObserveOwnCast(new(2, 1, null, new() { [253] = 24, [0] = 777L, [4] = 3404, [8] = 9 }), 14_000_000);
            combat.Observe(new(4, 1, null, new() { [252] = 18, [0] = 22, [1] = 30, [2] = 3404, [4] = 9, [5] = 777L }), "flow", "echo", 14_200_000, local, roster);
            assert(combat.Diagnostics["identifiedByOwnCast"] == 1 && combat.Snapshot!.Members.Single(member => member.Name == "Alpha").Healing == 80 + 450);
            assert(combat.Diagnostics["pendingUnidentified"] == 0);
        });
        test("After a capture restart only a full party list confirms membership; earlier effects of its members then count", () =>
        {
            var localId = Guid.NewGuid(); var oldId = Guid.NewGuid(); var leaverId = Guid.NewGuid(); var mateId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "dungeon", null, "observed", 1, "flow");
            var roster = new PartyRosterObservation();
            roster.Observe(new(4, 1, null, new() { [252] = 230, [8] = localId.ToByteArray().Concat(oldId.ToByteArray()).Concat(leaverId.ToByteArray()).ToArray(), [9] = new object[] { "Alpha", "Yesterday", "Leaver" } }), local, 1);
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1); combat.ForgetIdentities(1_000_000);
            void Event(Dictionary<byte, object?> parameters, long at)
            {
                var message = new WireMessage(4, 1, null, parameters);
                roster.Observe(message, local, at);
                combat.Observe(message, "flow", $"event:{at}", at, local, roster.Snapshot);
            }
            Event(new() { [252] = 29, [0] = 20, [1] = "Mate", [7] = mateId.ToByteArray() }, 2_000_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 20, [2] = -100f }, 3_000_000);
            Event(new() { [252] = 234, [0] = 1, [1] = leaverId.ToByteArray() }, 4_000_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 20, [2] = -50f }, 5_000_000);
            assert(combat.Snapshot!.TotalDamage == 0 && combat.Diagnostics["awaitingRoster"] == 2 && combat.Snapshot.Members.All(member => member.Name != "Yesterday"));
            Event(new() { [252] = 230, [8] = localId.ToByteArray().Concat(mateId.ToByteArray()).ToArray(), [9] = new object[] { "Alpha", "Mate" } }, 6_000_000);
            assert(combat.Snapshot!.Members.Single(member => member.Name == "Mate").Damage == 150 && combat.Diagnostics["creditedAfterRoster"] == 2);
            Event(new() { [252] = 235, [0] = 1, [1] = mateId.ToByteArray() }, 7_000_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 20, [2] = -70f }, 8_000_000);
            assert(roster.Snapshot!.Members.All(member => member.Id != mateId) && combat.Snapshot!.TotalDamage == 150);
            Event(new() { [252] = 6, [0] = 30, [6] = 20, [2] = -10f }, 8_500_000);
            combat.Observe(new(3, 1, 0, new() { [253] = 2, [0] = 80 }), "flow", "join", 9_000_000, local with { ObjectId = 80, ObservedAtMicroseconds = 9_000_000 }, roster.Snapshot);
            assert(combat.Sightings.Select(sighting => sighting.Visit).Distinct().Count() == 2);
        });
        test("A kill names the killer's object by character and by name, and never overrides an object already named", () =>
        {
            var localId = Guid.NewGuid(); var meiId = Guid.NewGuid(); var neerId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "dungeon", null, "observed", 1, "flow");
            var roster = new PartyRosterSnapshot([new(localId, "Alpha"), new(meiId, "MistressMei"), new(neerId, "neerkpz")], 1, true);
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1);
            void Event(Dictionary<byte, object?> parameters, long at) => combat.Observe(new(4, 1, null, parameters), "flow", $"event:{at}", at, local, roster);
            Event(new() { [252] = 6, [0] = 30, [6] = 786, [2] = -120f }, 1_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 412, [2] = -80f }, 2_000);
            Event(new() { [252] = 44, [0] = 1955, [3] = 786, [6] = meiId.ToByteArray(), [15] = 43846 }, 3_000);
            Event(new() { [252] = 165, [0] = 412, [3] = "@MOB_T6_BOSS", [4] = 412, [5] = "neerkpz" }, 4_000);
            Event(new() { [252] = 165, [0] = 786, [3] = "@MOB_T6_BOSS", [4] = 786, [5] = "neerkpz" }, 5_000);
            var members = combat.Snapshot!.Members;
            assert(members.Single(member => member.Name == "MistressMei").Damage == 120 && members.Single(member => member.Name == "neerkpz").Damage == 80);
            assert(combat.Diagnostics["identifiedByKill"] == 2 && combat.Diagnostics["identityConflict"] == 1);
        });
        test("Inspecting a character reports its equipment, which then names its party object by identical equipment", () =>
        {
            var localId = Guid.NewGuid(); var mateId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "dungeon", null, "observed", 1, "flow");
            var roster = new PartyRosterSnapshot([new(localId, "Alpha"), new(mateId, "Mate")], 1, true);
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1);
            void Event(WireMessage message, long at) => combat.Observe(message, "flow", $"event:{at}", at, local, roster);
            Event(new(4, 1, null, new() { [252] = 670, [0] = 55 }), 1_000);
            Event(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = 55, [2] = -90f }), 2_000);
            short[] types = [9038, 0, 5490, 3925, 4184, 0, 2510, 3029, 0, 0];
            for (var slot = 0; slot < types.Length; slot++) if (types[slot] > 0) Event(new(4, 1, null, new() { [252] = 30, [0] = 30170 + slot, [1] = types[slot] }), 3_000);
            Event(new(3, 1, 0, new() { [253] = 143, [0] = mateId.ToByteArray(), [1] = types.Select((type, slot) => type > 0 ? (object)(30170 + slot) : 0).ToArray() }), 3_100);
            Event(new(4, 1, null, new() { [252] = 90, [0] = 55, [2] = types }), 4_000);
            assert(combat.Diagnostics["gearFromInspection"] == 1 && combat.Snapshot!.Members.Single(member => member.Name == "Mate").Damage == 90);
        });
        test("A party formed by an invite during the capture starts then; the invited player's earlier effects never count", () =>
        {
            var localId = Guid.NewGuid(); var friendId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "city", null, "observed", 1, "flow");
            var roster = new PartyRosterObservation();
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1); combat.ForgetIdentities(1_000_000);
            void Event(Dictionary<byte, object?> parameters, long at)
            {
                var message = new WireMessage(4, 1, null, parameters);
                roster.Observe(message, local, at);
                combat.Observe(message, "flow", $"event:{at}", at, local, roster.Snapshot);
            }
            Event(new() { [252] = 29, [0] = 20, [1] = "Friend", [7] = friendId.ToByteArray() }, 2_000_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 20, [2] = -100f }, 2_500_000);
            combat.ObserveOwnCast(new(2, 1, null, new() { [253] = 225, [0] = friendId.ToByteArray() }), 3_000_000);
            Event(new() { [252] = 230, [0] = 13735, [8] = localId.ToByteArray().Concat(friendId.ToByteArray()).ToArray(), [9] = new object[] { "Alpha", "Friend" } }, 3_500_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 20, [2] = -40f }, 4_000_000);
            assert(combat.Snapshot!.Members.Single(member => member.Name == "Friend").Damage == 40 && combat.Diagnostics["outsideParty"] == 1);
        });
        test("Leaving a party and rejoining the same one proves the party before the leave, never the time outside it", () =>
        {
            var localId = Guid.NewGuid(); var mateId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "dungeon", null, "observed", 1, "flow");
            var roster = new PartyRosterObservation();
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1); combat.ForgetIdentities(1_000_000);
            void Event(Dictionary<byte, object?> parameters, long at)
            {
                var message = new WireMessage(4, 1, null, parameters);
                roster.Observe(message, local, at);
                combat.Observe(message, "flow", $"event:{at}", at, local, roster.Snapshot);
            }
            Event(new() { [252] = 29, [0] = 20, [1] = "Mate", [7] = mateId.ToByteArray() }, 2_000_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 20, [2] = -100f }, 3_000_000);
            Event(new() { [252] = 234, [0] = 43846, [1] = localId.ToByteArray() }, 4_000_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 20, [2] = -60f }, 5_000_000);
            Event(new() { [252] = 228, [0] = mateId.ToByteArray(), [1] = "Mate", [7] = 43846 }, 6_000_000);
            Event(new() { [252] = 230, [0] = 43846, [8] = mateId.ToByteArray().Concat(localId.ToByteArray()).ToArray(), [9] = new object[] { "Mate", "Alpha" } }, 6_500_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 20, [2] = -30f }, 7_000_000);
            combat.FlushUnidentified();
            assert(combat.Snapshot!.Members.Concat(combat.Snapshot.FormerMembers ?? []).Single(member => member.Name == "Mate").Damage == 130);
        });
        test("Characters announced on a new zone's connection just before its join stay known in that zone", () =>
        {
            var localId = Guid.NewGuid(); var meiId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 30, "Alpha", null, null, "city", null, "observed", 1, "old");
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1);
            combat.Observe(new(4, 1, null, new() { [252] = 29, [0] = 402, [1] = "Stranger", [7] = Guid.NewGuid().ToByteArray() }), "old", "old-zone", 1_000_000, local, null);
            combat.Observe(new(4, 1, null, new() { [252] = 29, [0] = 402, [1] = "MistressMei", [7] = meiId.ToByteArray() }), "island", "announce", 5_000_000, local, null);
            local = local with { ObjectId = 432, Connection = "island", ObservedAtMicroseconds = 5_300_000 };
            combat.Observe(new(3, 1, 0, new() { [253] = 2, [0] = 432 }), "island", "join", 5_300_000, local, null);
            var roster = new PartyRosterSnapshot([new(localId, "Alpha"), new(meiId, "MistressMei")], 6_000_000, true, ListedAtMicroseconds: 6_000_000);
            combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = 402, [2] = -75f }), "island", "hit", 7_000_000, local, roster);
            assert(combat.Snapshot!.Members.Single(member => member.Name == "MistressMei").Damage == 75);
            assert(combat.Sightings.Where(sighting => sighting.CharacterId == meiId).Select(sighting => sighting.Visit).Distinct().Count() == 1);
        });
        test("Evidence about a party formed later never makes its members part of the party before it", () =>
        {
            var localId = Guid.NewGuid(); var meiId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "island", null, "observed", 1, "flow");
            var roster = new PartyRosterObservation();
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1); combat.ForgetIdentities(1_000_000);
            void Event(Dictionary<byte, object?> parameters, long at)
            {
                var message = new WireMessage(4, 1, null, parameters);
                roster.Observe(message, local, at);
                combat.Observe(message, "flow", $"event:{at}", at, local, roster.Snapshot);
            }
            Event(new() { [252] = 29, [0] = 402, [1] = "MistressMei", [7] = meiId.ToByteArray() }, 2_000_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 402, [2] = -98f }, 3_000_000);
            Event(new() { [252] = 234, [0] = 23102, [1] = localId.ToByteArray() }, 4_000_000);
            combat.ObserveOwnCast(new(2, 1, null, new() { [253] = 225, [0] = meiId.ToByteArray() }), 5_000_000);
            Event(new() { [252] = 230, [0] = 25271, [8] = localId.ToByteArray().Concat(meiId.ToByteArray()).ToArray(), [9] = new object[] { "Alpha", "MistressMei" } }, 6_000_000);
            Event(new() { [252] = 243, [0] = 8, [1] = meiId.ToByteArray() }, 6_100_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 402, [2] = -50f }, 7_000_000);
            combat.FlushUnidentified();
            assert(combat.Snapshot!.Members.Single(member => member.Name == "MistressMei").Damage == 50);
        });
        test("A skill several item families share is never assigned to one of them without the worn item", () =>
        {
            var catalog = new AlbionMeter.Core.CombatSourceCatalog(new("test",
                [new(1, "T4_MAIN_HOLYSTAFF", "Holy Staff", "mainhand", 4, 0, [], ["HOLYFLASH"]), new(2, "T4_2H_DIVINESTAFF", "Divine Staff", "mainhand", 4, 0, [], ["HOLYFLASH"])],
                [new(0, "HOLYFLASH", [])]));
            assert(catalog.UniqueFamily(0) is null && catalog.Resolve(0, 3, false, [1], []) is { Index: 1, Slot: "mainhand" } && catalog.Resolve(0, 3, false, [], []).Index is null);
            assert(catalog.FamilyName(1) == "Holy Staff · tier not observed");
        });
        test("Worn items' game skill lists place effects, and equipment reported later corrects earlier effects of the same visit", () =>
        {
            var catalog = new AlbionMeter.Core.CombatSourceCatalog(new("test",
                [new(1, "T4_MAIN_NATURESTAFF_AVALON", "Ironroot Staff", "mainhand", 4, 0, [], ["SOUL_LINK"]), new(2, "T5_MAIN_NATURESTAFF_AVALON", "Ironroot Staff", "mainhand", 5, 0, [], ["SOUL_LINK"]), new(3, "T6_ARMOR_LEATHER_ROYAL", "Royal Jacket", "armor", 6, 0, [], ["FLAMESHIELD"]),
                 new(4, "T4_MAIN_FIRESTAFF", "Fire Staff", "mainhand", 4, 0, [], ["FIREBALL"])],
                [new(0, "SOUL_LINK", ["SOUL_LINK_MULTISPELL2"]), new(1, "SOUL_LINK_MULTISPELL2", []), new(2, "FLAMESHIELD", []), new(3, "FIREBALL", [])]));
            var one = Guid.NewGuid(); var two = Guid.NewGuid(); var three = Guid.NewGuid();
            var roster = new PartyRosterSnapshot([new(one, "One"), new(two, "Two"), new(three, "Three")], 1, true);
            var combat = new CapturedCombatObservation(catalog);
            combat.BeginSession(1);
            long at = 1_000;
            void Event(Dictionary<byte, object?> parameters) => combat.Observe(new(4, 1, null, parameters), "flow", $"event:{at}", at += 1_000, null, roster);
            void Heal(long source) => Event(new() { [252] = 6, [0] = 99, [6] = source, [2] = 10f, [7] = (short)1 });
            Event(new() { [252] = 29, [0] = 10, [1] = "One", [7] = one.ToByteArray(), [40] = new short[] { 1, 0, 0, 3, 0, 0, 0, 0, 0, 0 } });
            Heal(10);
            Event(new() { [252] = 74, [0] = "two", [1] = "hi", [4] = 20, [5] = two.ToByteArray() });
            Heal(20); Heal(20); Heal(20);
            Event(new() { [252] = 244, [0] = two.ToByteArray(), [2] = new short[] { 1, 0, 0, 3, 0, 0, 0, 0, 0, 0 } });
            Event(new() { [252] = 74, [0] = "three", [1] = "hi", [4] = 30, [5] = three.ToByteArray() });
            Heal(30);
            Event(new() { [252] = 90, [0] = 30, [2] = new short[] { 1, 0, 0, 3, 0, 0, 0, 0, 0, 0 } });
            Heal(30);
            combat.FlushUnidentified();
            var items = combat.Snapshot!.ItemContributions!;
            assert(items.Single(item => item.PlayerName == "One") is { ItemIndex: 1, Slot: "mainhand", Kind: "ability", Healing: 10 });
            assert(items.Single(item => item.PlayerName == "Two") is { ItemIndex: 1, Healing: 30 } && combat.Diagnostics["reattributedFromTimeline"] == 3);
            assert(items.Single(item => item.PlayerName == "Three" && !item.TierObserved) is { ItemIndex: 1, Healing: 10 } && items.Single(item => item.PlayerName == "Three" && item.TierObserved).Healing == 10);
            assert(items.Sum(item => item.Healing) == combat.Snapshot.TotalHealing && combat.Timeline.Count(entry => entry.Kind == "gear") == 3);
        });
        test("Skills chosen on the local character's items explain its effects, and nothing else guesses a source", () =>
        {
            var catalog = new AlbionMeter.Core.CombatSourceCatalog(new("test",
                [new(7587, "T4_MAIN_HOLYSTAFF@4", "Adept's Holy Staff", "mainhand", 4, 4), new(7594, "T6_MAIN_HOLYSTAFF@1", "Master's Holy Staff", "mainhand", 6, 1)],
                [new(3404, "HOLYFLASH", []), new(3409, "HEALINGBEAM", ["HEALINGBEAM_EFFECT"]), new(3413, "HEALINGBEAM_EFFECT", [])]));
            var localId = Guid.NewGuid(); var healerId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 71, "Alpha", null, null, "one", null, "observed", 10, "flow");
            var roster = new PartyRosterSnapshot([new(localId, "Alpha"), new(healerId, "Healer")], 1, true);
            var combat = new CapturedCombatObservation(catalog);
            combat.BeginSession(1);
            void Event(Dictionary<byte, object?> parameters, long at) => combat.Observe(new(4, 1, null, parameters), "flow", $"event:{at}", at, local, roster);
            Event(new() { [252] = 30, [0] = 302, [1] = 7587, [9] = new short[] { 3404, 3409, 3420 } }, 9_000_000);
            local = local with { ObservedAtMicroseconds = 10_000_000 };
            combat.Observe(new(3, 2, 0, new() { [253] = 2, [0] = 71, [52] = new short[] { 302 } }), "flow", "join", 10_000_000, local, roster);
            Event(new() { [252] = 6, [0] = 30, [6] = 71, [2] = 200f, [7] = (short)3413 }, 11_000_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 71, [2] = 100f, [7] = (short)3404 }, 11_100_000);
            var own = combat.Snapshot!.ItemContributions!.Single(contribution => contribution.PlayerName == "Alpha");
            assert(own is { ItemIndex: 7587, Slot: "mainhand", Kind: "ability", Healing: 300 });
            Event(new() { [252] = 74, [0] = "healer", [1] = "hi", [4] = 20, [5] = healerId.ToByteArray() }, 12_000_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 20, [2] = 150f, [7] = (short)3413 }, 12_100_000);
            assert(combat.Snapshot!.ItemContributions!.Single(contribution => contribution.PlayerName == "Healer").ItemIndex is null);
        });
        test("Equipment seen before identification replaces a stale weapon, and skills alone never do", () =>
        {
            var localId = Guid.NewGuid(); var healerId = Guid.NewGuid();
            var roster = new PartyRosterSnapshot([new(localId, "Alpha"), new(healerId, "Healer")], 1, true);
            var combat = new CapturedCombatObservation();
            combat.SeedCharacters([new(localId, new("Alpha", [6662, 0, 3461, 4249, 5817, 2908, 2562, 3098])), new(healerId, new("Healer", [6662, 0, 1, 2, 3, 4, 5, 6]))]);
            combat.BeginSession(1); combat.ForgetIdentities(1_000_000);
            void Event(Dictionary<byte, object?> parameters, long at) => combat.Observe(new(4, 1, null, parameters), "flow", $"event:{at}", at, null, roster);
            Event(new() { [252] = 90, [0] = 396, [2] = new short[] { 7587, 2329, 3682, 5372, 4636, 2908, 2562, 3098, 0, 0 }, [7] = new short[] { 3398, 3409, 3420, 3925 } }, 2_000_000);
            Event(new() { [252] = 74, [0] = "alpha", [1] = "hi", [4] = 396, [5] = localId.ToByteArray() }, 2_100_000);
            assert(combat.Weapons[localId] == 7587);
            Event(new() { [252] = 74, [0] = "healer", [1] = "hi", [4] = 500, [5] = healerId.ToByteArray() }, 2_200_000);
            assert(combat.Weapons[healerId] is null);
            Event(new() { [252] = 6, [0] = 30, [6] = 500, [2] = 120f, [7] = (short)3409 }, 2_300_000);
            assert(combat.Weapons[healerId] is null && !combat.Diagnostics.ContainsKey("weaponInferredFromSkills"));
        });
        test("Party equipment identical to a party object's equipment names a member already nearby; similar equipment does not", () =>
        {
            var localId = Guid.NewGuid(); var memberId = Guid.NewGuid(); var otherId = Guid.NewGuid();
            var roster = new PartyRosterSnapshot([new(localId, "Alpha"), new(memberId, "Beta"), new(otherId, "Gamma")], 1, true);
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1);
            void Event(Dictionary<byte, object?> parameters, long at) => combat.Observe(new(4, 1, null, parameters), "flow", $"event:{at}", at, null, roster);
            Event(new() { [252] = 6, [0] = 30, [6] = 552, [2] = -120f }, 1_000_000);
            Event(new() { [252] = 90, [0] = 552, [2] = new short[] { 9317, 2403, 4299, 5447, 4203, 2908, 2712, 3098, 0, 0 } }, 1_100_000);
            Event(new() { [252] = 670, [0] = 552, [1] = localId.ToByteArray() }, 1_200_000);
            assert(combat.Snapshot!.TotalDamage == 0);
            Event(new() { [252] = 244, [0] = otherId.ToByteArray(), [2] = new short[] { 7114, 0, 5342, 4997, 5397, 2900, 2837, 3098, 0, 0 } }, 1_300_000);
            Event(new() { [252] = 244, [0] = memberId.ToByteArray(), [2] = new short[] { 9317, 2403, 4299, 5447, 4203, 2908, 2541, 3098, 0, 0 } }, 1_400_000);
            assert(combat.Snapshot!.TotalDamage == 0 && !combat.Diagnostics.ContainsKey("identifiedByEquipment"));
            Event(new() { [252] = 244, [0] = memberId.ToByteArray(), [2] = new short[] { 9317, 2403, 4299, 5447, 4203, 2908, 2712, 3098, 0, 0 } }, 1_500_000);
            assert(combat.Snapshot!.Members.Single(member => member.Name == "Beta").Damage == 120 && combat.Diagnostics["identifiedByEquipment"] == 1);
        });
        test("After a capture restart the local object is confirmed only by the echo of an own cast request", () =>
        {
            var localId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 452, "Alpha", null, null, "one", null, "observed", 1, "flow");
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1);
            combat.ForgetIdentities(5_000_000);
            void Hit(long at, short spell) => combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = 452, [2] = -70f, [7] = spell }), "flow", $"hit:{at}", at, local, null);
            Hit(6_000_000, -1); Hit(7_000_000, -1);
            assert(combat.Snapshot!.Members.All(member => member.Damage == 0));
            combat.ObserveOwnCast(new(2, 1, null, new() { [253] = 24, [0] = 555L, [4] = 3925, [8] = 1 }), 8_000_000);
            Hit(8_300_000, 3925);
            assert(combat.Snapshot!.Members.All(member => member.Damage == 0));
            combat.Observe(new(4, 1, null, new() { [252] = 18, [0] = 452, [1] = 30, [2] = 3925, [4] = 1, [5] = 555L }), "flow", "echo", 8_250_000, local, null);
            assert(combat.Diagnostics["identifiedByOwnCast"] == 1 && combat.Snapshot!.Members.Single(member => member.Name == "Alpha").Damage == 210);
        });
        test("After a capture restart the local weapon is proven by its durability dropping in use, back to the capture's start", () =>
        {
            var catalog = new AlbionMeter.Core.CombatSourceCatalog(new("test",
                [new(9022, "T6_MAIN_HAMMER", "Master's Hammer", "mainhand", 6, 0, [], ["IRONBREAKER"]), new(9030, "T6_2H_HAMMER", "Master's Great Hammer", "mainhand", 6, 0, [], ["IRONBREAKER"])],
                [new(3342, "IRONBREAKER", [])]));
            var localId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 393, "Alpha", null, null, "one", null, "observed", 1, "flow");
            var combat = new CapturedCombatObservation(catalog);
            combat.BeginSession(1);
            combat.ForgetIdentities(5_000_000);
            void Event(Dictionary<byte, object?> parameters, long at) => combat.Observe(new(4, 1, null, parameters), "flow", $"event:{at}", at, local, null);
            combat.ObserveOwnCast(new(2, 1, null, new() { [253] = 24, [0] = 555L, [4] = 3342, [8] = 1 }), 6_000_000);
            Event(new() { [252] = 18, [0] = 393, [1] = 30, [2] = 3342, [4] = 1, [5] = 555L }, 6_050_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 393, [2] = -50f, [5] = (byte)0, [7] = (short)-1 }, 6_100_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 393, [2] = -200f, [7] = (short)3342 }, 6_200_000);
            assert(combat.Snapshot!.ItemContributions!.All(contribution => contribution.ItemIndex is null));
            Event(new() { [252] = 30, [0] = 395, [1] = 9022, [8] = 303_190_000L, [9] = new short[] { 3342 } }, 7_000_000);
            assert(combat.Snapshot!.ItemContributions!.All(contribution => contribution.ItemIndex is null));
            Event(new() { [252] = 30, [0] = 395, [1] = 9022, [8] = 303_180_000L, [9] = new short[] { 3342 } }, 8_000_000);
            var own = combat.Snapshot!.ItemContributions!.Where(contribution => contribution.PlayerName == "Alpha").ToArray();
            assert(own.Length == 2 && own.All(contribution => contribution.ItemIndex == 9022) && own.Sum(contribution => contribution.Damage) == 250);
            assert(combat.Weapons[localId] == 9022);
        });
        test("A join after a capture restart proves the equipment since the restart, unless the equipment changed in between", () =>
        {
            var catalog = new AlbionMeter.Core.CombatSourceCatalog(new("test",
                [new(4308, "T4_ARMOR_LEATHER_MORGANA", "Adept's Stalker Jacket", "armor", 4, 0, [], ["STORMSHIELD"]), new(4310, "T5_ARMOR_LEATHER_MORGANA", "Expert's Stalker Jacket", "armor", 5, 0, [], ["STORMSHIELD"])],
                [new(3952, "STORMSHIELD", [])]));
            foreach (var changed in new[] { false, true })
            {
                var localId = Guid.NewGuid();
                var local = new LocalCharacterSnapshot(localId, 393, "Alpha", null, null, "one", null, "observed", 1, "flow");
                var combat = new CapturedCombatObservation(catalog);
                combat.BeginSession(1);
                combat.ForgetIdentities(5_000_000);
                void Event(Dictionary<byte, object?> parameters, long at) => combat.Observe(new(4, 1, null, parameters), "flow", $"event:{at}", at, local, null);
                combat.ObserveOwnCast(new(2, 1, null, new() { [253] = 24, [0] = 555L, [4] = 3952, [8] = 1 }), 6_000_000);
                Event(new() { [252] = 18, [0] = 393, [1] = 30, [2] = 3952, [4] = 1, [5] = 555L }, 6_050_000);
                Event(new() { [252] = 6, [0] = 30, [6] = 393, [2] = -300f, [7] = (short)3952 }, 6_100_000);
                if (changed) Event(new() { [252] = 90, [0] = 393, [2] = new short[] { 0, 0, 0, 4308, 0, 0, 0, 0, 0, 0 } }, 7_000_000);
                Event(new() { [252] = 90, [0] = 900, [2] = new short[] { 0, 0, 0, 4308, 0, 0, 0, 0, 0, 0 } }, 9_998_000);
                Event(new() { [252] = 30, [0] = 903, [1] = 4308, [8] = 161_700_000L }, 9_999_000);
                local = local with { ObjectId = 900, ObservedAtMicroseconds = 10_000_000 };
                combat.Observe(new(3, 2, 0, new() { [253] = 2, [0] = 900L, [52] = new long[] { 0, 0, 0, 903, 0, 0, 0, 0, 0, 0 } }), "flow", "join", 10_000_000, local, null);
                var chest = combat.Snapshot!.ItemContributions!.Single(contribution => contribution.PlayerName == "Alpha");
                assert(changed ? !chest.TierObserved : chest is { ItemIndex: 4308, Damage: 300, TierObserved: true });
            }
        });
        test("The item moves of the first equipment change prove the equipment worn since a capture restart", () =>
        {
            var catalog = new AlbionMeter.Core.CombatSourceCatalog(new("test",
                [new(9022, "T6_MAIN_HAMMER", "Master's Hammer", "mainhand", 6, 0, [], ["IRONBREAKER"]), new(9030, "T7_MAIN_HAMMER", "Grandmaster's Hammer", "mainhand", 7, 0, [], ["IRONBREAKER"]),
                 new(4308, "T4_ARMOR_LEATHER_MORGANA", "Adept's Stalker Jacket", "armor", 4, 0, [], ["STORMSHIELD"]), new(4310, "T5_ARMOR_LEATHER_MORGANA", "Expert's Stalker Jacket", "armor", 5, 0, [], ["STORMSHIELD"])],
                [new(3342, "IRONBREAKER", []), new(3952, "STORMSHIELD", [])]));
            var equipment = Guid.NewGuid(); var bag = Guid.NewGuid();
            foreach (var requested in new[] { true, false })
            {
                var localId = Guid.NewGuid();
                var local = new LocalCharacterSnapshot(localId, 393, "Alpha", null, null, "one", null, "observed", 1, "flow");
                var combat = new CapturedCombatObservation(catalog);
                combat.BeginSession(1);
                void Event(Dictionary<byte, object?> parameters, long at) => combat.Observe(new(4, 1, null, parameters), "flow", $"event:{at}", at, local, null);
                combat.Observe(new(3, 2, 0, new() { [253] = 2, [0] = 393L, [51] = equipment.ToByteArray(), [54] = bag.ToByteArray() }), "flow", "join", 500_000, local, null);
                combat.ForgetIdentities(5_000_000);
                combat.ObserveOwnCast(new(2, 1, null, new() { [253] = 24, [0] = 555L, [4] = 3342, [8] = 1 }), 6_000_000);
                Event(new() { [252] = 18, [0] = 393, [1] = 30, [2] = 3342, [4] = 1, [5] = 555L }, 6_050_000);
                Event(new() { [252] = 6, [0] = 30, [6] = 393, [2] = -100f, [5] = (byte)0, [7] = (short)-1 }, 6_100_000);
                Event(new() { [252] = 6, [0] = 30, [6] = 393, [2] = -300f, [7] = (short)3952 }, 6_200_000);
                if (requested) combat.ObserveOwnCast(new(2, 1, null, new() { [253] = 30, [1] = equipment.ToByteArray(), [2] = 2, [4] = bag.ToByteArray(), [5] = 2 }), 7_000_000);
                Event(new() { [252] = 26, [0] = 425, [2] = bag.ToByteArray(), [3] = 2 }, 7_200_000);
                Event(new() { [252] = 90, [0] = 393, [2] = new short[] { 0, 0, 0, 4308, 0, 0, 0, 0, 0, 0 } }, 7_200_000);
                combat.ObserveOwnCast(new(2, 1, null, new() { [253] = 30, [1] = bag.ToByteArray(), [2] = 3, [4] = equipment.ToByteArray(), [5] = 3 }), 7_400_000);
                Event(new() { [252] = 26, [0] = 425, [2] = equipment.ToByteArray(), [3] = 3 }, 7_600_000);
                Event(new() { [252] = 90, [0] = 393, [2] = new short[] { 9022, 0, 0, 4308, 0, 0, 0, 0, 0, 0 } }, 7_600_000);
                var own = combat.Snapshot!.ItemContributions!.Where(contribution => contribution.PlayerName == "Alpha").ToArray();
                assert(own.Single(contribution => contribution.Slot is "chest" or "armor") is { ItemIndex: 4308, TierObserved: true, Damage: 300 });
                var hammer = own.Single(contribution => contribution.Kind == "autoAttack");
                assert(requested ? hammer is { ItemIndex: 9022, Damage: 100 } : hammer.ItemIndex is null);
            }
        });
        test("A swap after a join is kept when a later item report arrives for an item the join listed", () =>
        {
            var catalog = new AlbionMeter.Core.CombatSourceCatalog(new("test",
                [new(9022, "T6_MAIN_HAMMER", "Master's Hammer", "mainhand", 6, 0, [], ["IRONBREAKER"]), new(4308, "T4_ARMOR_LEATHER_MORGANA", "Adept's Stalker Jacket", "armor", 4, 0, [], ["STORMSHIELD"]),
                 new(4309, "T4_ARMOR_LEATHER_MORGANA@1", "Adept's Stalker Jacket", "armor", 4, 1, [], ["STORMSHIELD"])],
                [new(3342, "IRONBREAKER", []), new(3952, "STORMSHIELD", [])]));
            var equipment = Guid.NewGuid(); var bag = Guid.NewGuid(); var localId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 400, "Alpha", null, null, "one", null, "observed", 1_000_000, "flow");
            var combat = new CapturedCombatObservation(catalog);
            combat.BeginSession(1);
            void Event(Dictionary<byte, object?> parameters, long at) => combat.Observe(new(4, 1, null, parameters), "flow", $"event:{at}", at, local, null);
            Event(new() { [252] = 90, [0] = 400, [2] = new short[] { 9022, 0, 0, 4308, 0, 0, 0, 0, 0, 0 } }, 999_000);
            Event(new() { [252] = 30, [0] = 403, [1] = 9022, [8] = 1000L }, 999_500);
            Event(new() { [252] = 30, [0] = 405, [1] = 4308, [8] = 1000L }, 999_600);
            combat.Observe(new(3, 2, 0, new() { [253] = 2, [0] = 400L, [51] = equipment.ToByteArray(), [52] = new long[] { 403, 0, 0, 405, 0, 0, 0, 0, 0, 0 }, [54] = bag.ToByteArray() }), "flow", "join", 1_000_000, local, null);
            Event(new() { [252] = 6, [0] = 30, [6] = 400, [2] = -100f, [7] = (short)3952 }, 2_000_000);
            Event(new() { [252] = 26, [0] = 407, [1] = 3, [2] = equipment.ToByteArray(), [3] = 3 }, 3_000_000);
            Event(new() { [252] = 90, [0] = 400, [2] = new short[] { 9022, 0, 0, 4309, 0, 0, 0, 0, 0, 0 } }, 3_000_000);
            Event(new() { [252] = 26, [0] = 405, [2] = bag.ToByteArray(), [3] = 3 }, 3_000_000);
            Event(new() { [252] = 30, [0] = 403, [1] = 9022, [8] = 990L }, 3_700_000);
            Event(new() { [252] = 6, [0] = 30, [6] = 400, [2] = -200f, [7] = (short)3952 }, 4_000_000);
            var chest = combat.Snapshot!.ItemContributions!.Where(contribution => contribution.PlayerName == "Alpha").ToDictionary(contribution => contribution.ItemIndex ?? 0, contribution => contribution.Damage);
            assert(chest.GetValueOrDefault(4308) == 100 && chest.GetValueOrDefault(4309) == 200);
        });
        test("Each potion enchantment's own spell names the exact potion", () =>
        {
            var catalog = new AlbionMeter.Core.CombatSourceCatalog(new("test",
                [new(598, "T4_POTION_COOLDOWN", "Minor Poison Potion", "potion", 4, 0, [], ["POTION_POISON_P1"]), new(599, "T4_POTION_COOLDOWN@1", "Minor Poison Potion", "potion", 4, 1, [], ["POTION_POISON_P2"])],
                [new(4395, "POTION_POISON_P1", []), new(4400, "POTION_POISON_P2", [])]));
            assert(catalog.UniqueFamily(4395) is { Index: 598, Slot: "potion", Exact: true } && catalog.UniqueFamily(4400) is { Index: 599, Exact: true });
        });
        test("Unnamed party objects are kept as unidentified at a zone change while objects the party list never named are dropped", () =>
        {
            var localId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 1, "Alpha", null, null, "one", null, "observed", 1, "flow");
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1);
            void Effect(long source, long target, float value, long at) => combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = target, [6] = source, [2] = value }), "flow", $"effect:{source}:{at}", at, local, null);
            foreach (var partyObject in new long[] { 40, 42 }) combat.Observe(new(4, 1, null, new() { [252] = 670, [0] = partyObject }), "flow", $"party:{partyObject}", 500_000, local, null);
            Effect(40, 41, -200, 1_000_000); Effect(42, 40, 60, 1_500_000); Effect(50, 40, -900, 2_000_000);
            Effect(60, 60, 500, 2_100_000); Effect(1, 70, -400, 2_200_000); Effect(70, 71, 300, 2_300_000);
            combat.Observe(new(3, 1, 0, new() { [253] = 2 }), "flow", "zone", 3_000_000, local, null);
            var members = combat.Snapshot!.Members;
            assert(members.Single(member => member.Name == "Unidentified player (obj 40)").Damage == 200);
            assert(members.Single(member => member.Name == "Unidentified player (obj 42)").Healing == 60);
            assert(members.All(member => member.Name is not ("Unidentified player (obj 50)" or "Unidentified player (obj 60)" or "Unidentified player (obj 70)" or "Unidentified player (obj 71)")));
            assert(combat.Diagnostics["unidentifiedPlayersKept"] == 2 && combat.Diagnostics["droppedOutsidePartyObjects"] >= 2 && members.Single(member => member.Name == "Alpha").Damage == 400);
        });
        test("The local weapon follows equipped item objects listed by each join", () =>
        {
            var id = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 71, "Alpha", null, null, "one", null, "observed", 10, "flow");
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1);
            void Join(byte[] slots, long at) { local = local with { ObservedAtMicroseconds = at }; combat.Observe(new(3, 2, 0, new() { [253] = 2, [0] = 71, [52] = slots }), "flow", $"join:{at}", at, local, null); }
            void Item(long item, int type, long at) => combat.Observe(new(4, 1, null, new() { [252] = 30, [0] = item, [1] = type }), "flow", $"item:{item}:{at}", at, local, null);
            Item(74, 6662, 9_000_000); Join([74, 0, 72], 10_000_000);
            assert(combat.Weapons[id] == 6662);
            Join([74, 0, 72], 30_000_000);
            assert(combat.Weapons[id] == 6662);
            Item(74, 5697, 31_000_000);
            assert(combat.Weapons[id] == 5697);
            combat.ForgetIdentities(40_000_000);
            combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = 71, [2] = -10f }), "flow", "stale-hit", 41_000_000, local, null);
            assert(combat.Diagnostics["unresolvedActor"] == 1);
        });
        test("Unidentified nearby actors are tracked until a zone change announces them again", () =>
        {
            var id = Guid.NewGuid(); var otherId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 10, "Alpha", null, null, "one", null, "observed", 1, "flow");
            var roster = new PartyRosterSnapshot([new(id, "Alpha"), new(otherId, "Beta")], 1, true);
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1);
            void Hit(long source, long at) => combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = source, [2] = -50f }), "flow", $"hit:{source}:{at}", at, local, roster);
            Hit(20, 2); Hit(20, 3);
            assert(combat.Diagnostics["unresolvedSinceZoneChange"] == 2);
            combat.Observe(new(3, 1, 0, new() { [253] = 2 }), "flow", "zone", 4, local, roster);
            assert(!combat.Diagnostics.ContainsKey("unresolvedSinceZoneChange") && combat.Diagnostics["unresolvedActor"] == 2);
            combat.Observe(new(4, 1, null, new() { [252] = 29, [0] = 20, [1] = "Beta", [7] = otherId.ToByteArray() }), "flow", "announce", 5, local, roster);
            Hit(20, 6);
            assert(combat.Diagnostics["attributedSinceZoneChange"] == 1 && !combat.Diagnostics.ContainsKey("unresolvedSinceZoneChange"));
            assert(combat.Snapshot!.Members.Single(member => member.Name == "Beta").Damage == 50);
        });
        test("Record controls after a capture gap do not extend the active dungeon floor", () =>
        {
            var id = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 10, "Alpha", null, null, "one", null, "observed", 1, "flow");
            var dungeon = new DungeonCombatObservation();
            local = local with { MapId = "@RANDOMDUNGEON@one" };
            dungeon.Observe(new(3, 1, 0, new() { [253] = 2, [1] = id.ToByteArray(), [8] = local.MapId, [66] = "1315" }), "flow", "join", 1_000_000, local, null);
            dungeon.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = 10, [2] = -100f }), "flow", "hit", 3_000_000, local, null);
            dungeon.BeginRecord(600_000_000);
            assert(dungeon.Snapshot().Single().Floors[0].Totals.ElapsedSeconds == 2);
            dungeon.SetPaused(true, 900_000_000); dungeon.SetPaused(false, 4_000_000_000);
            assert(dungeon.Snapshot().Single().Floors[0].Totals.ElapsedSeconds == 2);
        });
        test("Observed dungeon joins group floors revisits and new instances without double counting", () =>
        {
            var id = Guid.Parse("11111111-1111-1111-1111-111111111111");
            var dungeon = new DungeonCombatObservation();
            var local = new LocalCharacterSnapshot(id, 10, "Alpha", null, null, "1315", null, "observed", 0, "flow");
            void Join(string map, string entrance, long timestamp)
            {
                local = local with { MapId = map, ObservedAtMicroseconds = timestamp };
                dungeon.Observe(new(3, 1, 0, new() { [253] = (short)2, [0] = 10, [1] = id.ToByteArray(), [2] = "Alpha", [8] = map, [66] = entrance }), "flow", $"join:{timestamp}", timestamp, local, null);
            }
            void Hit(float value, long timestamp) => dungeon.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [2] = value, [6] = 10 }), "flow", $"hit:{timestamp}", timestamp, local, null);
            Join("1315", "1301", 0);
            Join("@RANDOMDUNGEON@first", "1315", 1_000_000); Hit(-100, 2_000_000); Hit(20, 3_000_000);
            Join("@RANDOMDUNGEON@second", "1315", 5_000_000); Hit(-300, 6_000_000);
            var initial = dungeon.Snapshot().Single();
            assert(initial.Floors.Length == 2 && initial.EntranceMapId == "1315");
            assert(initial.Floors[0].Totals.TotalDamage == 100 && initial.Floors[0].Totals.TotalHealing == 20);
            assert(initial.Floors[0].Totals.ElapsedSeconds == 4 && initial.Floors[0].Totals.Members.Single().EncounterDps == 25);
            assert(initial.Floors[1].Totals.TotalDamage == 300 && initial.Totals.TotalDamage == 400);
            assert(initial.Totals.Timeline.Sum(point => point.Damage.Values.Sum()) == 400);
            assert(initial.Totals.Timeline.Sum(point => point.Healing.Values.Sum()) == 20);
            assert(dungeon.Snapshot().Single().Totals.TotalDamage == 400);
            Join("@RANDOMDUNGEON@second", "1315", 7_000_000);
            assert(dungeon.Snapshot().Single().Floors[1].Encounters.Length == 1);
            Join("@RANDOMDUNGEON@first", "1315", 8_000_000); Hit(-50, 9_000_000);
            var revisited = dungeon.Snapshot().Single();
            assert(revisited.Floors.Length == 2 && revisited.Floors[0].Encounters.Length == 2);
            assert(revisited.Floors[0].Totals.TotalDamage == 150 && revisited.Totals.TotalDamage == 450);
            dungeon.BeginRecord(10_000_000); Hit(-25, 11_000_000);
            assert(dungeon.Snapshot().Single().Totals.TotalDamage == 475);
            Join("1315", "1301", 12_000_000); Hit(-900, 13_000_000);
            assert(dungeon.Snapshot().Single().Status == "completed" && dungeon.Snapshot().Single().Totals.TotalDamage == 475);
            Join("@RANDOMDUNGEON@third", "1315", 14_000_000); Hit(-70, 15_000_000);
            assert(dungeon.Snapshot().Length == 2 && dungeon.Snapshot().Last().Totals.TotalDamage == 70);
            Join("@RANDOMDUNGEON@first", "1315", 16_000_000);
            assert(dungeon.Snapshot().Length == 2 && dungeon.Snapshot().First().Status == "running");
            dungeon.ForgetIdentities(100_000_000); Hit(-1_000, 101_000_000);
            assert(dungeon.Snapshot().Sum(run => run.Totals.TotalDamage) == 545 && dungeon.Snapshot().All(run => run.Status == "completed"));
            Join("@RANDOMDUNGEON@third", "1315", 102_000_000); Hit(-5, 103_000_000);
            assert(dungeon.Snapshot().Length == 2 && dungeon.Snapshot().Last().Totals.TotalDamage == 75);
            var replayed = new DungeonCombatObservation();
            local = local with { MapId = "@RANDOMDUNGEON@first" };
            replayed.Observe(new(3, 1, 0, new() { [253] = 2, [0] = 10, [1] = id.ToByteArray(), [2] = "Alpha", [8] = local.MapId, [66] = "1315" }), "flow", "replay", 1_000_000, local, null);
            assert(replayed.Snapshot().Single().Id == initial.Id);
            assert(replayed.Snapshot().Single().Floors[0].Encounters[0].Id == initial.Floors[0].Encounters[0].Id);
        });
        test("A new record clears damage healing timeline and former members while retaining current equipment", () =>
        {
            var localId = Guid.NewGuid(); var otherId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "one", null, "observed", 1, "flow");
            var roster = new PartyRosterObservation();
            roster.Observe(new(4, 1, null, new() { [252] = 230, [8] = localId.ToByteArray().Concat(otherId.ToByteArray()).ToArray(), [9] = new object[] { "Alpha", "Beta" } }), local, 1);
            var combat = new CapturedCombatObservation();
            combat.Observe(new(4, 1, null, new() { [252] = 29, [0] = 20, [1] = "Beta", [7] = otherId.ToByteArray() }), "flow", "entity", 2, local, roster.Snapshot);
            combat.Observe(new(4, 1, null, new() { [252] = 90, [0] = 10, [2] = new short[] { 9022 } }), "flow", "equipment", 3, local, roster.Snapshot);
            combat.Observe(new(4, 1, null, new() { [252] = 7, [0] = 30, [2] = new float[] { -100, 40, -200 }, [6] = new short[] { 10, 10, 20 } }), "flow", "hits", 4, local, roster.Snapshot);
            roster.Observe(new(4, 1, null, new() { [252] = 234, [1] = otherId.ToByteArray() }), local, 5);
            combat.SetParty(roster.Snapshot);
            var previous = combat.Snapshot!;
            assert(previous.TotalDamage == 300 && previous.TotalHealing == 40 && previous.FormerMembers!.Length == 1);
            combat.BeginSession(6_000_000);
            var current = combat.Snapshot!;
            assert(current.Id != previous.Id && current.PartyId != previous.PartyId);
            assert(current.TotalDamage == 0 && current.TotalHealing == 0 && current.EventCount == 0 && current.ElapsedSeconds == 0);
            assert(current.Members.Length == 1 && current.Members[0].Name == "Alpha");
            assert(current.Members[0].Weapon == "Item #9022" && current.FormerMembers!.Length == 0);
            assert(current.WeaponContributions!.Length == 0 && current.ItemContributions!.Length == 0);
            assert(current.Timeline.All(point => point.Damage.Values.Sum() == 0 && point.Healing.Values.Sum() == 0));
            assert(current.Members.All(member => member.EncounterDps == 0 && member.RollingDps == 0 && member.EncounterHps == 0 && member.RollingHps == 0));
            combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = 10, [2] = -25f }), "flow", "new-hit", 7_000_000, local, roster.Snapshot);
            assert(combat.Snapshot!.TotalDamage == 25 && previous.TotalDamage == 300 && previous.TotalHealing == 40);
        });
        test("A newer solo confirmation replaces recovered membership and rejoining restores prior damage", () =>
        {
            var localId = Guid.NewGuid(); var otherId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "one", null, "observed", 1, "flow");
            var roster = new PartyRosterObservation();
            var joined = new WireMessage(4, 1, null, new() { [252] = 230, [8] = localId.ToByteArray().Concat(otherId.ToByteArray()).ToArray(), [9] = new object[] { "Alpha", "Beta" } });
            roster.Observe(joined, local, 1);
            var combat = new CapturedCombatObservation();
            combat.Observe(new(4, 1, null, new() { [252] = 29, [0] = 20, [1] = "Beta", [7] = otherId.ToByteArray() }), "flow", "entity", 2, local, roster.Snapshot);
            void Hit(string identity, long timestamp, float amount) => combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = 20, [2] = -amount }), "flow", identity, timestamp, local, roster.Snapshot);
            Hit("before", 3, 100);
            assert(roster.Restore(new([], 4, true, "user-confirmed")));
            combat.SetParty(roster.Snapshot);
            assert(combat.Snapshot!.Members.All(member => member.Name != "Beta"));
            assert(combat.Snapshot!.FormerMembers!.Single().Damage == 100);
            assert(!roster.Restore(new([new(localId, "Alpha"), new(otherId, "Beta")], 1, true)));
            Hit("outside", 5, 200);
            assert(combat.Snapshot!.TotalDamage == 100);
            roster.Observe(joined, local, 6);
            assert(roster.Snapshot!.Source == "capture");
            Hit("returned", 7, 50);
            assert(combat.Snapshot!.Members.Single(member => member.Name == "Beta").Damage == 150);
            assert(combat.Snapshot!.FormerMembers!.Length == 0);
            var previous = roster.Snapshot;
            assert(!roster.Restore(new([new(otherId, "Beta")], 8, true)));
            assert(!roster.Restore(new([new(localId, "Alpha"), new(localId, "Duplicate")], 8, true)));
            assert(roster.Snapshot == previous);
        });
        test("Imported area effects retain the weapon source of Ironbreaker damage", () =>
        {
            byte[] Encrypt(string xml)
            {
                using var compressed = new MemoryStream();
                using (var gzip = new System.IO.Compression.GZipStream(compressed, System.IO.Compression.CompressionMode.Compress, true))
                    gzip.Write(System.Text.Encoding.UTF8.GetBytes(xml));
                using var cipher = System.Security.Cryptography.DES.Create();
                cipher.Key = Convert.FromHexString("30EF724742F20432");
                var iv = Convert.FromHexString("0EA6DC89DBEDDC4F");
                return cipher.EncryptCbc(compressed.ToArray(), iv, System.Security.Cryptography.PaddingMode.PKCS7);
            }
            var data = AlbionMeter.Core.GameAssetImporter.ImportCombat(
                Encrypt("<items><equipmentitem uniquename='T6_MAIN_HAMMER' slottype='mainhand' tier='6'/></items>"),
                Encrypt("<spells><colortag/><activespell uniquename='IRONBREAKER'><spelleffectarea effect='IRONBREAKER_EFFECT'/></activespell><activespell uniquename='IRONBREAKER_EFFECT'/></spells>"), null, "test");
            var sources = new AlbionMeter.Core.CombatSourceCatalog(data);
            assert(sources.Resolve(1, 2, true, [1], [0]) is { Index: 1, Slot: "mainhand", Kind: "ability" });
            assert(sources.Resolve(1, 2, true, [1], []).Index is null);
            var id = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 10, "Alpha", null, null, "map", null, "observed", 1, "flow");
            var combat = new CapturedCombatObservation(sources);
            combat.Observe(new(4, 1, null, new() { [252] = 90, [0] = 10, [2] = new short[] { 1 }, [7] = new short[] { 0 } }), "flow", "equipment", 1, local, null);
            combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [2] = -171f, [6] = 10, [7] = 1, [5] = (byte)2 }), "flow", "hit", 2, local, null);
            var snapshot = combat.Snapshot!;
            assert(snapshot.TotalDamage == 171);
            assert(snapshot.ItemContributions!.Single() is { Damage: 171, ItemIndex: 1, Slot: "mainhand", Kind: "ability" });
            assert(snapshot.Timeline.Single().ItemDamage!.Values.Sum() == 171);
        });
        test("Ranged and melee auto attacks retain their type apart from abilities and cape procs", () =>
        {
            var sources = new AlbionMeter.Core.CombatSourceCatalog(new("test",
                [new(2637, "T4_CAPEITEM_FW_THETFORD@3", "Thetford Cape", "cape", 4, 3, ["CAPE"])],
                [new(214, "CAPE", []), new(3215, "FIRE", [])]));
            int?[] equipment = [7108, 0, 0, 0, 0, 0, 2637];
            assert(sources.Resolve(-1, 1, true, equipment, []).Kind == "autoAttack");
            assert(sources.Resolve(-1, 0, true, equipment, []).Kind == "autoAttack");
            assert(sources.Resolve(-1, 1, false, equipment, []).Kind == "unknown");
            assert(sources.Resolve(-1, 5, true, equipment, []).Kind == "unknown");
            assert(sources.Resolve(null, 1, true, equipment, []).Kind == "unknown");
            assert(sources.Resolve(-1, 1, true, [], []) is { Kind: "autoAttack", Index: null });
            var id = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 10, "Alpha", null, null, "map", null, "observed", 1, "flow");
            var combat = new CapturedCombatObservation(sources);
            combat.Observe(new(4, 1, null, new() { [252] = 90, [0] = 10, [2] = new short[] { 7108, 0, 0, 0, 0, 0, 2637 }, [7] = new short[] { 3215 } }), "flow", "equipment", 1, local, null);
            var batch = new WireMessage(4, 1, null, new() { [252] = 7, [0] = 30, [2] = new float[] { -32, -32, -32, -32, -106 }, [6] = new short[] { 10, 10, 10, 10, 10 }, [7] = new short[] { -1, -1, -1, 3215, 214 }, [5] = new byte[] { 1, 1, 1, 3, 2 } });
            combat.Observe(batch, "flow", "hits", 2, local, null);
            combat.Observe(batch, "flow", "hits", 2, local, null);
            var snapshot = combat.Snapshot!;
            assert(snapshot.TotalDamage == 234);
            assert(snapshot.ItemContributions!.Single(item => item.Kind == "autoAttack") is { Damage: 96, ItemIndex: 7108, Slot: "mainhand" });
            assert(snapshot.ItemContributions!.Single(item => item.Kind == "ability").Damage == 32);
            assert(snapshot.ItemContributions!.Single(item => item.Kind == "passive").Damage == 106);
            assert(snapshot.ItemContributions!.Sum(item => item.Damage) == snapshot.TotalDamage);
            assert(snapshot.Timeline.Single().ItemDamage!.Values.Sum() == snapshot.TotalDamage);
        });
        test("Cape passive chains resolve only for the observed equipped cape", () =>
        {
            var sources = new AlbionMeter.Core.CombatSourceCatalog(new("test",
                [new(2500, "T4_CAPEITEM_FW_THETFORD", "Thetford Cape", "cape", 4, 0, ["PASSIVE_CAPE_THETFORD"])],
                [new(100, "PASSIVE_CAPE_THETFORD", ["CHAIN"]), new(101, "CHAIN", ["EFFECT"]), new(102, "EFFECT", [])]));
            int?[] equipment = [7108, 0, 0, 0, 0, 0, 2500];
            assert(sources.Resolve(102, 3, true, equipment, []) is { Index: 2500, Slot: "cape" });
            assert(sources.Resolve(999, 3, true, equipment, []).Index is null);
            assert(sources.Resolve(102, 3, true, [7108], []).Index is null);
            assert(sources.Resolve(102, 3, true, [7108, 0, 0, 0, 0, 0, 9999], []).Index is null);
            assert(sources.Resolve(-1, 0, true, equipment, []) is { Index: 7108, Slot: "mainhand" });
            assert(sources.Resolve(102, 3, true, equipment, [100]).Index is null);
            var id = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 10, "Alpha", null, null, "map", null, "observed", 1, "flow");
            var combat = new CapturedCombatObservation(sources);
            combat.Observe(new(4, 1, null, new() { [252] = 90, [0] = 10, [2] = new short[] { 7108, 0, 0, 0, 0, 0, 2500 } }), "flow", "equipment", 1, local, null);
            combat.Observe(new(4, 1, null, new() { [252] = 7, [0] = 30, [2] = new object[] { -32f, -138f }, [6] = new object[] { 10, 10 }, [7] = new object[] { -1, 102 }, [5] = new object[] { 0, 3 } }), "flow", "hits", 2, local, null);
            var snapshot = combat.Snapshot!;
            assert(snapshot.TotalDamage == 170);
            assert(snapshot.ItemContributions!.Single(item => item.Slot == "cape").Damage == 138);
            assert(snapshot.ItemContributions!.Single(item => item.Slot == "mainhand").Damage == 32);
            assert(snapshot.Timeline.Single().ItemDamage!.Values.Sum() == 170);
        });
        test("Item attribution follows chest effects without counting them as weapon damage", () =>
        {
            var sources = new AlbionMeter.Core.CombatSourceCatalog(new("test", [], [new(3952, "STORMSHIELD", ["STORMSHIELD_EFFECT"]), new(3953, "STORMSHIELD_EFFECT", []), new(3359, "HAMMERWHIRLWIND2", [])]));
            int?[] equipment = [9022, 0, 0, 4308, 0]; int?[] spells = [-1, -1, 3359, 3952];
            assert(sources.Resolve(3953, 3, true, equipment, spells) is { Index: 4308, Slot: "chest" });
            assert(sources.Resolve(3359, 3, true, equipment, spells) is { Index: 9022, Slot: "mainhand" });
            assert(sources.Resolve(-1, 0, true, equipment, spells) is { Index: 9022, Slot: "mainhand" });
            assert(sources.Resolve(-1, 8, false, equipment, spells).Index is null);
            assert(sources.Resolve(9999, 3, true, equipment, spells).Index is null);
            assert(sources.Resolve(3953, 3, true, equipment, [3952, -1, -1, 3952]).Index is null);
            var id = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 10, "Alpha", null, null, "map", null, "observed", 1, "flow");
            var combat = new CapturedCombatObservation(sources);
            combat.Observe(new(4, 1, null, new() { [252] = 90, [0] = 10, [2] = new short[] { 9022, 0, 0, 4308, 0 }, [7] = new short[] { -1, -1, 3359, 3952 } }), "flow", "equipment", 1, local, null);
            combat.Observe(new(4, 1, null, new() { [252] = 7, [0] = 30, [2] = new object[] { -40f, -32f }, [6] = new object[] { 10, 10 }, [7] = new object[] { -1, 3953 }, [5] = new object[] { 0, 3 } }), "flow", "hits", 2, local, null);
            var snapshot = combat.Snapshot!;
            assert(snapshot.TotalDamage == 72);
            assert(snapshot.ItemContributions!.Single(item => item.Slot == "chest").Damage == 32);
            assert(snapshot.ItemContributions!.Single(item => item.Slot == "mainhand").Damage == 40);
            assert(snapshot.Timeline.Single().ItemDamage!.Values.Sum() == 72);
            assert(snapshot.WeaponContributions!.Single().Damage == 72);
        });
        test("Weapon contributions retain the weapon at each hit across swaps and same-second events", () =>
        {
            var id = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 10, "Alpha", null, null, "map", null, "observed", 1, "flow");
            var combat = new CapturedCombatObservation();
            void Equipment(int index, long timestamp) => combat.Observe(new(4, 1, null, new() { [252] = 90, [0] = 10, [2] = new short[] { (short)index } }), "flow", $"equipment:{timestamp}", timestamp, local, null);
            void Hit(float amount, string identity, long timestamp) => combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = 10, [2] = amount }), "flow", identity, timestamp, local, null);
            Equipment(9022, 1_000_000); Hit(-100, "one", 1_100_000);
            Equipment(7108, 1_200_000); Hit(-200, "two", 1_300_000); Hit(40, "heal", 1_400_000);
            Equipment(9022, 1_500_000); Hit(-50, "three", 1_600_000); Hit(-50, "three", 1_600_000);
            var snapshot = combat.Snapshot!;
            assert(snapshot.TotalDamage == 350 && snapshot.TotalHealing == 40);
            assert(snapshot.WeaponContributions!.Single(weapon => weapon.WeaponIndex == 9022).Damage == 150);
            assert(snapshot.WeaponContributions!.Single(weapon => weapon.WeaponIndex == 7108) is { Damage: 200, Healing: 40 });
            assert(snapshot.Timeline.Single().WeaponDamage!.Values.Sum() == 350);
            assert(snapshot.Timeline.Single().WeaponHealing!.Values.Sum() == 40);
            snapshot.Timeline.Single().WeaponDamage!.Clear();
            assert(combat.Snapshot!.Timeline.Single().WeaponDamage!.Values.Sum() == 350);
        });
        test("Unknown weapon contributions are never relabeled after equipment is discovered", () =>
        {
            var engine = new AlbionMeter.Core.CombatEngine([new("a", "Alpha", "Unknown", "unknown", "#ffffff")]);
            engine.Advance(1);
            engine.Add(new("before", "a", 1, AlbionMeter.Core.CombatMetric.Damage, 10));
            engine.RegisterMember(new("a", "Alpha", "Hammer", "hammer", "#ffffff"));
            engine.Advance(2);
            engine.Add(new("after", "a", 2, AlbionMeter.Core.CombatMetric.Damage, 20, 9022));
            var snapshot = engine.Snapshot(true);
            assert(snapshot.WeaponContributions!.Single(weapon => weapon.WeaponIndex is null) is { Damage: 10, Weapon: "Weapon not observed" });
            assert(snapshot.WeaponContributions!.Single(weapon => weapon.WeaponIndex == 9022).Damage == 20);
        });
        test("Party damage survives a capture boundary and zoning without another party join", () =>
        {
            var localId = Guid.NewGuid(); var otherId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "one", null, "observed", 1, "first");
            var party = new PartyRosterObservation();
            party.Observe(new(4, 1, null, new() { [252] = 230, [8] = localId.ToByteArray().Concat(otherId.ToByteArray()).ToArray(), [9] = new object[] { "Alpha", "Beta" } }), local, 1);
            var combat = new CapturedCombatObservation();
            combat.Observe(new(4, 1, null, new() { [252] = 29, [0] = 20, [1] = "Beta", [7] = otherId.ToByteArray() }), "first", "initial", 2, local, party.Snapshot);
            combat.BeginSession(3);
            local = local with { ObjectId = 99, MapId = "two", Connection = "second" };
            combat.Observe(new(4, 1, null, new() { [252] = 29, [0] = 80, [1] = "Beta", [7] = otherId.ToByteArray() }), "second", "new-object", 4, local, party.Snapshot);
            combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 99, [6] = 80, [2] = -100f }), "second", "hit", 5, local, party.Snapshot);
            assert(combat.Snapshot!.Members.Single(member => member.Name == "Beta").Damage == 100);
            party.Observe(new(4, 1, null, new() { [252] = 234, [1] = otherId.ToByteArray() }), local, 6);
            combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 99, [6] = 80, [2] = -50f }), "second", "outside", 6, local, party.Snapshot);
            assert(combat.Snapshot!.FormerMembers!.Single(member => member.Name == "Beta").Damage == 100);
            party.Observe(new(4, 1, null, new() { [252] = 232, [1] = otherId.ToByteArray(), [2] = "Beta" }), local, 7);
            combat.Observe(new(4, 1, null, new() { [252] = 6, [0] = 99, [6] = 80, [2] = -25f }), "second", "returned", 7, local, party.Snapshot);
            assert(combat.Snapshot!.Members.Single(member => member.Name == "Beta").Damage == 125);
        });
        test("Entities observed before join survive confirmation on the same connection", () =>
        {
            var id = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 10, "Alpha", null, null, "map", null, "observed", 1, "next");
            var observation = new CapturedCombatObservation();
            observation.Observe(new(4, 1, null, new() { [252] = 123, [0] = 30, [1] = 424 }), "next", "spawn", 1, null, null);
            observation.Observe(new(4, 1, null, new() { [252] = 90, [0] = 10, [2] = new short[] { 9022 } }), "next", "equipment", 2, null, null);
            observation.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [2] = -310f, [6] = 10 }), "next", "hit", 3, local, null);
            assert(observation.Diagnostics["damageTo:mob"] == 1 && observation.Weapons[id] == 9022);
        });
        test("Captured combat counts batches once and isolates unresolved actors", () =>
        {
            var id = Guid.NewGuid(); var other = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 10, "Alpha", null, null, "map", null, "observed", 1, "flow");
            var party = new PartyRosterSnapshot([new(id, "Alpha"), new(other, "Beta")], 1, true);
            var observation = new CapturedCombatObservation();
            observation.Observe(new(4, 1, null, new() { [252] = 29, [0] = 20, [1] = "Beta", [7] = other.ToByteArray(), [40] = new short[] { 7108 } }), "flow", "spawn", 1, local, party);
            var hit = new WireMessage(4, 1, null, new() { [252] = 7, [0] = 10, [2] = new object[] { -98f, -77f }, [6] = new object[] { 20L, 20L } });
            observation.Observe(hit, "flow", "hit", 2, local, party);
            observation.Observe(hit, "flow", "hit", 2, local, party);
            assert(observation.Snapshot is { EventCount: 2, TotalDamage: 175 });
            assert(observation.Weapons[other] == 7108);
            observation.Observe(new(4, 1, null, new() { [252] = 90, [0] = 20L, [2] = new object[] { 9022 } }), "flow", "equipment", 3, local, party);
            assert(observation.Weapons[other] == 9022);
            observation.Observe(hit, "foreign", "wrong-flow", 4, local, party);
            assert(observation.Snapshot!.TotalDamage == 175);
            observation.BeginSession();
            assert(observation.Snapshot is null);
            observation.Observe(hit, "flow", "next", 5, local, party);
            assert(observation.Snapshot!.TotalDamage == 175);
        });
        test("Captured damage keeps raw overkill and recognizes mob targets", () =>
        {
            var id = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 10, "Alpha", null, null, "map", null, "observed", 1, "flow");
            var observation = new CapturedCombatObservation();
            observation.Observe(new(4, 1, null, new() { [252] = 123, [0] = 30, [1] = 424 }), "flow", "mob", 1, local, null);
            observation.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [2] = -310f, [6] = 10L }), "flow", "hit", 2, local, null);
            assert(observation.Snapshot!.TotalDamage == 310 && observation.Diagnostics["damageTo:mob"] == 1);
            observation.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [6] = 10L }), "flow", "missing", 3, local, null);
            assert(observation.Snapshot!.TotalDamage == 310);
            var next = local with { Connection = "next", ObjectId = 99 };
            observation.Observe(new(4, 1, null, new() { [252] = 6, [0] = 30, [2] = -50f, [6] = 10L }), "next", "stale-object", 4, next, null);
            assert(observation.Snapshot!.TotalDamage == 310);
        });
        test("Captured party membership survives zoning and applies leave and rejoin", () =>
        {
            var localId = Guid.NewGuid(); var otherId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 1, "Alpha", null, null, "1301", null, "observed", 1, "one");
            var roster = new PartyRosterObservation();
            var join = new WireMessage(4, 1, null, new() { [252] = (short)230, [8] = localId.ToByteArray().Concat(otherId.ToByteArray()).ToArray(), [9] = new object[] { "Alpha", "Beta" } });
            roster.Observe(join, local, 2);
            assert(roster.Snapshot is { Complete: true, Members.Length: 2 });
            roster.Observe(new(3, 1, 0, new() { [253] = 2 }), local with { MapId = "1302", Connection = "two" }, 3);
            assert(roster.Snapshot!.Members.Length == 2);
            roster.Observe(new(4, 1, null, new() { [252] = 234, [1] = otherId.ToByteArray() }), local, 4);
            assert(roster.Snapshot!.Members.Length == 1);
            roster.Observe(join, local, 2);
            assert(roster.Snapshot!.Members.Length == 1);
            roster.Observe(new(4, 1, null, new() { [252] = 232, [1] = otherId.ToByteArray(), [2] = "Beta" }), local, 5);
            assert(roster.Snapshot!.Members.Length == 2);
            roster.Observe(new(4, 1, null, new() { [252] = 234, [1] = localId.ToByteArray() }), local, 6);
            assert(roster.Snapshot!.Members.Length == 0);
        });
        test("Malformed and unrelated party rosters cannot replace known membership", () =>
        {
            var id = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(id, 1, "Alpha", null, null, "1301", null, "observed", 1, "one");
            var roster = new PartyRosterObservation();
            roster.Observe(new(4, 1, null, new() { [252] = 230, [8] = id.ToByteArray(), [9] = new object[] { "Alpha" } }), local, 1);
            roster.Observe(new(4, 1, null, new() { [252] = 230, [8] = new byte[17], [9] = new object[] { "Invalid" } }), local, 2);
            roster.Observe(new(4, 1, null, new() { [252] = 230, [8] = Guid.NewGuid().ToByteArray(), [9] = new object[] { "Other" } }), local, 3);
            assert(roster.Snapshot!.Members.Single().Id == id);
            roster.Observe(new(3, 1, 0, new()), local with { CharacterId = Guid.NewGuid() }, 4);
            assert(roster.Snapshot is null);
        });
        test("Local identity requires a successful join and ignores nearby players", () =>
        {
            var state = new LocalCharacterObservation();
            var id = Guid.NewGuid();
            var join = new WireMessage(3, 1, 0, new() { [253] = 2, [1] = id.ToByteArray(), [2] = "Alpha", [8] = "1301", [58] = "Guild", [79] = "Alliance" });
            state.Observe(join with { ReturnCode = 1 }, "server", 1);
            assert(state.Snapshot is null);
            state.Observe(join, "server", 2);
            assert(state.Snapshot is { Name: "Alpha", Guild: "Guild", Alliance: "Alliance", MapId: "1301" });
            state.Observe(new(4, 1, null, new() { [252] = 29, [7] = Guid.NewGuid().ToByteArray(), [8] = "Other" }), "server", 3);
            assert(state.Snapshot!.Guild == "Guild");
            state.Observe(new(4, 1, null, new() { [252] = 29, [7] = id.ToByteArray(), [8] = "", [51] = "" }), "server", 4);
            assert(state.Snapshot!.Guild == "" && state.Snapshot.Alliance == "");
            state.Observe(join, "server", 1);
            assert(state.Snapshot!.Guild == "");
        });
        test("Zone transitions invalidate the old map until a new join", () =>
        {
            var state = new LocalCharacterObservation();
            var join = new WireMessage(3, 1, 0, new() { [253] = 2, [1] = Guid.NewGuid().ToByteArray(), [2] = "Alpha", [8] = "1301" });
            state.Observe(join, "server", 1);
            var change = new WireMessage(3, 1, 0, new() { [253] = 41, [0] = "1302" });
            state.Observe(change, "other", 2);
            assert(state.Snapshot!.MapId == "1301");
            state.Observe(change, "server", 3);
            assert(state.Snapshot is { MapId: null, DestinationMapId: "1302", LocationState: "changing" });
            state.Observe(join with { Parameters = new() { [253] = 2, [1] = Guid.NewGuid().ToByteArray(), [2] = "Beta", [8] = "1302" } }, "next", 4);
            assert(state.Snapshot is { Name: "Beta", MapId: "1302", Guild: null, Alliance: null, DestinationMapId: null });
        });
        test("A zone change preserves roster but invalidates object bindings", () =>
        {
            var state = new PartyObservation(); var context = state.Context;
            var member = new ObservedCharacter(Guid.NewGuid(), "Alpha", "T4_MAIN_SWORD");
            state.ReplaceParty(context, 1, [member]); state.Observe(context, 2, 42, member);
            assert(state.Resolve(context, 42)!.IsPartyMember == true);
            var next = state.EnterZone();
            assert(state.Snapshot().Members.Length == 1 && state.Resolve(next, 42) is null);
            assert(!state.Observe(context, 3, 42, member));
            assert(state.Observe(next, 4, 93, member));
            assert(state.Resolve(next, 93)!.IsPartyMember == true);
        });
        test("Party reconstruction does not erase the current weapon or binding", () =>
        {
            var state = new PartyObservation(); var context = state.Context;
            var member = new ObservedCharacter(Guid.NewGuid(), "Alpha", "T4_MAIN_SWORD");
            state.Observe(context, 1, 42, member);
            assert(state.Resolve(context, 42)!.IsPartyMember is null);
            state.ReplaceParty(context, 2, [member with { WeaponId = null }]);
            assert(state.Resolve(context, 42)!.Character.WeaponId == "T4_MAIN_SWORD");
            state.ReplaceParty(context, 3, []);
            assert(state.Resolve(context, 42)!.IsPartyMember == false);
            state.ReplaceParty(context, 4, [member]);
            assert(state.Resolve(context, 42)!.IsPartyMember == true);
        });
        test("Object reuse and stale observations cannot recover an old identity", () =>
        {
            var state = new PartyObservation(); var context = state.Context;
            var first = new ObservedCharacter(Guid.NewGuid(), "Alpha", "Sword");
            var second = new ObservedCharacter(Guid.NewGuid(), "Beta", "Staff");
            state.Observe(context, 5, 42, first); state.Observe(context, 6, 42, second);
            assert(!state.Observe(context, 4, 42, first));
            assert(!state.Observe(context, 4, 43, first));
            assert(state.Resolve(context, 42)!.Character.Id == second.Id);
            assert(state.Snapshot().BindingConflicts == 1);
            var next = state.BeginConnection();
            assert(state.Resolve(next, 42) is null && !state.Snapshot().RosterComplete);
            assert(!state.Observe(context, 99, 42, first));
        });
        test("Invalid roster replacement is atomic and older snapshots are rejected", () =>
        {
            var state = new PartyObservation(); var context = state.Context;
            var member = new ObservedCharacter(Guid.NewGuid(), "Alpha", null);
            state.ReplaceParty(context, 5, [member]);
            assert(!state.ReplaceParty(context, 4, []));
            var rejected = false;
            try { state.ReplaceParty(context, 6, [member, member]); } catch (ArgumentException) { rejected = true; }
            assert(rejected && state.Snapshot().Members.Length == 1);
        });
        test("Protocol18 reads scalar event values and compressed integers", () =>
        {
            var decoded = Protocol18.Decode([0xf3, 4, 1, 4, 252, 11, 6, 0, 9, 84, 2, 12, 50, 6, 10, 86]);
            assert(decoded.Status == "decodedMessage");
            var values = decoded.Message!.Parameters;
            assert((int)values[252]! == 6 && (int)values[0]! == 42 && (int)values[2]! == -50 && (long)values[6]! == 43);
        });
        test("Protocol18 decodes party names arrays and GUID byte arrays", () =>
        {
            var result = Protocol18.Decode([0xf3, 4, 1, 2, 8, 67, 2, 1, 2, 9, 71, 2, 1, 65, 1, 66]);
            assert(result.Status == "decodedMessage");
            assert(((byte[])result.Message!.Parameters[8]!).SequenceEqual(new byte[] { 1, 2 }));
            assert(((object?[])result.Message.Parameters[9]!).SequenceEqual(new object?[] { "A", "B" }));
        });
        test("Protocol18 distinguishes unsupported encrypted and malformed messages", () =>
        {
            assert(Protocol18.Decode([0xf3, 0x84]).Status == "encryptedMessage");
            assert(Protocol18.Decode([0xf3, 4, 1, 1, 0, 255]).Status == "unsupportedValueType");
            assert(Protocol18.Decode([0xf3, 4, 1, 1, 0, 9, 255, 255, 255, 255, 127]).Status == "malformedMessage");
            assert(Protocol18.Decode([0xf3, 4, 1, 2, 0, 30, 0, 30]).Status == "malformedMessage");
            assert(Protocol18.Decode([0xf3, 4, 1, 0, 0]).Status == "malformedMessage");
        });
        test("Protocol18 random messages cannot bypass limits or crash parsing", () =>
        {
            var random = new Random(21);
            for (var i = 0; i < 10000; i++)
            {
                var bytes = new byte[random.Next(2, 300)]; random.NextBytes(bytes); bytes[0] = 0xf3; bytes[1] = 4;
                Protocol18.Decode(bytes);
            }
        });
    }
}
