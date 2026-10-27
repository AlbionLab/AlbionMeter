using AlbionMeter.Core.Capture;
using AlbionMeter.Core.Observation;

internal static class MembershipGapChecks
{
    public static void Run(Action<string, Action> test, Action<bool> assert)
    {
        test("A rejected dungeon entrance restores deferred effects to the current floor", () =>
        {
            var localId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "@RANDOMDUNGEON@one", null, "observed", 1_000_000, "flow");
            var dungeon = new DungeonCombatObservation();
            dungeon.Observe(new(3, 1, 0, new() { [253] = 2, [0] = 10, [1] = localId.ToByteArray(), [8] = local.MapId, [66] = "1315" }), "flow", "join", 1_000_000, local, null);
            dungeon.ObserveOwnCast(new(2, 1, null, new() { [253] = 237 }), 2_000_000);
            dungeon.Observe(new(4, 1, null, new() { [252] = 6, [0] = 900, [6] = 10, [2] = -100f }), "flow", "deferred", 2_100_000, local, null);
            dungeon.Observe(new(3, 1, 1, new() { [253] = 237 }), "flow", "rejected", 2_200_000, local, null);
            dungeon.Observe(new(4, 1, null, new() { [252] = 6, [0] = 900, [6] = 10, [2] = -50f }), "flow", "continued", 3_000_000, local, null);
            dungeon.FlushUnidentified();
            var run = dungeon.Snapshot().Single();
            assert(run.Floors.Length == 1 && run.Totals.TotalDamage == 150);
        });
        test("Paused destination effects stay excluded after the floor join and resume", () =>
        {
            var localId = Guid.NewGuid(); var memberId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "@RANDOMDUNGEON@one", null, "observed", 1_000_000, "old");
            var party = new PartyRosterSnapshot([new(localId, "Alpha"), new(memberId, "Mate")], 1, true);
            var dungeon = new DungeonCombatObservation();
            void Event(Dictionary<byte, object?> fields, long at) => dungeon.Observe(new(4, 1, null, fields), "next", $"event:{at}", at, local, party);
            dungeon.Observe(new(3, 1, 0, new() { [253] = 2, [0] = 10, [1] = localId.ToByteArray(), [8] = local.MapId, [66] = "1315" }), "old", "first", 1_000_000, local, party);
            dungeon.SetPaused(true, 2_000_000);
            Event(new() { [252] = 29, [0] = 402, [1] = "Mate", [7] = memberId.ToByteArray() }, 5_000_000);
            Event(new() { [252] = 6, [0] = 900, [6] = 402, [2] = -100f }, 5_100_000);
            local = local with { Connection = "next", MapId = "@RANDOMDUNGEON@two", ObservedAtMicroseconds = 5_300_000 };
            dungeon.Observe(new(3, 1, 0, new() { [253] = 2, [0] = 10, [1] = localId.ToByteArray(), [8] = local.MapId, [66] = "1315" }), "next", "second", 5_300_000, local, party);
            var floor = dungeon.Snapshot().Single().Floors[1];
            assert(floor.Totals.TotalDamage == 0 && floor.Totals.ElapsedSeconds == 0);
            dungeon.SetPaused(false, 6_000_000);
            Event(new() { [252] = 6, [0] = 900, [6] = 402, [2] = -50f }, 7_000_000);
            assert(dungeon.Snapshot().Single().Floors[1].Totals.TotalDamage == 50);
        });
        test("Destination traffic before join belongs to the new floor for both named and late-identified players", () =>
        {
            foreach (var reusedConnection in new[] { false, true })
            foreach (var namedBeforeHit in new[] { false, true })
            {
                var destination = reusedConnection ? "old" : "next";
                var localId = Guid.NewGuid(); var memberId = Guid.NewGuid(); var previousActorId = Guid.NewGuid();
                var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "@RANDOMDUNGEON@first", null, "observed", 1_000_000, "old");
                var party = new PartyRosterSnapshot([new(localId, "Alpha"), new(memberId, "Mate"), new(previousActorId, "Previous actor")], 1, true);
                var dungeon = new DungeonCombatObservation();
                void Event(string connection, Dictionary<byte, object?> fields, long at) => dungeon.Observe(new(4, 1, null, fields), connection, $"event:{at}", at, local, party);
                void Join(string connection, string map, long at)
                {
                    local = local with { Connection = connection, MapId = map, ObservedAtMicroseconds = at };
                    dungeon.Observe(new(3, 1, 0, new() { [253] = 2, [0] = 10, [1] = localId.ToByteArray(), [8] = map, [66] = "1315" }), connection, $"join:{at}", at, local, party);
                }
                Join("old", "@RANDOMDUNGEON@first", 1_000_000);
                Event("old", new() { [252] = 29, [0] = 402, [1] = "Previous actor", [7] = previousActorId.ToByteArray() }, 1_100_000);
                Event("old", new() { [252] = 6, [0] = 900, [6] = 402, [2] = -40f }, 2_000_000);
                if (reusedConnection) dungeon.ObserveOwnCast(new(2, 1, null, new() { [253] = 237 }), 4_900_000);
                Event(destination, new() { [252] = 670, [0] = 402 }, 5_000_000);
                if (namedBeforeHit) Event(destination, new() { [252] = 29, [0] = 402, [1] = "Mate", [7] = memberId.ToByteArray() }, 5_010_000);
                Event(destination, new() { [252] = 90, [0] = 402, [2] = new short[] { 6661 } }, 5_050_000);
                Event(destination, new() { [252] = 6, [0] = 900, [6] = 402, [2] = -100f }, 5_100_000);
                Event(destination, new() { [252] = 6, [0] = 10, [6] = 402, [2] = 25f }, 5_150_000);
                assert(dungeon.Snapshot().Single().Floors.Single().Totals.TotalDamage == 40);
                Join(destination, "@RANDOMDUNGEON@second", 5_300_000);
                if (!namedBeforeHit) Event(destination, new() { [252] = 29, [0] = 402, [1] = "Mate", [7] = memberId.ToByteArray() }, 6_000_000);
                dungeon.FlushUnidentified();
                var run = dungeon.Snapshot().Single();
                assert(run.Floors[0].Totals.TotalDamage == 40 && run.Floors[0].Totals.TotalHealing == 0);
                assert(run.Floors[0].Totals.ElapsedSeconds == 4);
                assert(run.Floors[1].Totals.TotalDamage == 100 && run.Floors[1].Totals.TotalHealing == 25);
                assert(run.Floors[1].Encounters.Single().Members.Single(member => member.Id == memberId.ToString()).Damage == 100);
                assert(run.Floors[1].Encounters.Single().Members.Single(member => member.Id == previousActorId.ToString()).Damage == 0);
                assert(run.Totals.TotalDamage == 140 && run.Totals.TotalHealing == 25);
                dungeon.FlushUnidentified();
                assert(dungeon.Snapshot().Single().Totals.TotalDamage == 140);
            }
        });
        test("Editing a remembered roster does not confirm the remaining members in a new capture", () =>
        {
            var localId = Guid.NewGuid(); var memberId = Guid.NewGuid(); var leavingId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "dungeon", null, "observed", 1, "flow");
            var party = new PartyRosterObservation();
            assert(party.Restore(new([new(localId, "Alpha"), new(memberId, "Mate"), new(leavingId, "Leaving")], 1, true)));
            party.Observe(new(4, 1, null, new() { [252] = 234, [1] = leavingId.ToByteArray() }), local, 5_000_000);
            assert(party.Snapshot!.ObservedAtMicroseconds == 5_000_000 && party.Snapshot.Members.Length == 2);
            assert(!party.Snapshot.IsConfirmedSince(1_000_000));
            assert(!party.Snapshot.IsConfirmedSince(null));
            party.Observe(new(4, 1, null, new() { [252] = 230, [8] = localId.ToByteArray().Concat(memberId.ToByteArray()).ToArray(), [9] = new object[] { "Alpha", "Mate" } }), local, 6_000_000);
            assert(party.Snapshot.IsConfirmedSince(1_000_000));
            assert((party.Snapshot with { Source = "user-confirmed" }).IsConfirmedSince(null));
        });
        test("Identifying a pending actor before membership proof preserves damage and healing for either proof type", () =>
        {
            foreach (var fullList in new[] { false, true })
            {
                var localId = Guid.NewGuid(); var memberId = Guid.NewGuid();
                var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "dungeon", null, "observed", 1, "flow");
                var combat = new CapturedCombatObservation();
                combat.BeginSession(1); combat.ForgetIdentities(1_000_000);
                PartyRosterSnapshot? roster = null;
                void Event(Dictionary<byte, object?> fields, long at) => combat.Observe(new(4, 1, null, fields), "flow", $"event:{at}", at, local, roster);
                Event(new() { [252] = 6, [0] = 900, [6] = 402, [2] = -100f }, 2_000_000);
                Event(new() { [252] = 6, [0] = 10, [6] = 402, [2] = 25f }, 2_100_000);
                Event(new() { [252] = 29, [0] = 402, [1] = "Mate", [7] = memberId.ToByteArray() }, 3_000_000);
                assert(combat.Snapshot!.TotalDamage == 0 && combat.Snapshot.TotalHealing == 0);
                if (fullList)
                {
                    roster = new([new(localId, "Alpha"), new(memberId, "Mate")], 4_000_000, true);
                    combat.SetParty(roster);
                }
                else Event(new() { [252] = 244, [0] = memberId.ToByteArray(), [2] = new short[] { 6661, 0, 5347, 5005, 5397, 2900, 2662, 3098 } }, 4_000_000);
                assert(combat.Snapshot.TotalDamage == 100 && combat.Snapshot.TotalHealing == 25);
                assert(combat.Snapshot.Members.Single(member => member.Id == memberId.ToString()).Damage == 100);
                combat.FlushUnidentified();
                assert(combat.Snapshot.TotalDamage == 100 && combat.Snapshot.TotalHealing == 25 && combat.Snapshot.EventCount == 2);
            }
        });
        test("Identifying an outsider never carries its damage into a party joined later", () =>
        {
            var localId = Guid.NewGuid(); var memberId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "dungeon", null, "observed", 1, "flow");
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1);
            var roster = new PartyRosterSnapshot([new(localId, "Alpha")], 1, true);
            void Event(Dictionary<byte, object?> fields, long at) => combat.Observe(new(4, 1, null, fields), "flow", $"event:{at}", at, local, roster);
            Event(new() { [252] = 6, [0] = 900, [6] = 402, [2] = -100f }, 2_000_000);
            Event(new() { [252] = 29, [0] = 402, [1] = "Mate", [7] = memberId.ToByteArray() }, 3_000_000);
            roster = new([new(localId, "Alpha"), new(memberId, "Mate")], 4_000_000, true);
            combat.SetParty(roster);
            Event(new() { [252] = 6, [0] = 900, [6] = 402, [2] = -200f }, 3_500_000);
            Event(new() { [252] = 6, [0] = 900, [6] = 402, [2] = -50f }, 5_000_000);
            combat.FlushUnidentified();
            assert(combat.Snapshot!.TotalDamage == 50 && combat.Snapshot.EventCount == 1);
        });
        test("Unidentified destination effects and equipment before join remain attributable after join", () =>
        {
            var localId = Guid.NewGuid(); var memberId = Guid.NewGuid();
            var local = new LocalCharacterSnapshot(localId, 10, "Alpha", null, null, "old", null, "observed", 1, "old");
            var combat = new CapturedCombatObservation();
            combat.BeginSession(1);
            var roster = new PartyRosterSnapshot([new(localId, "Alpha"), new(memberId, "Mate")], 1, true);
            void Event(string connection, Dictionary<byte, object?> fields, long at) => combat.Observe(new(4, 1, null, fields), connection, $"event:{at}", at, local, roster);
            Event("old", new() { [252] = 6, [0] = 900, [6] = 402, [2] = -500f }, 1_000_000);
            Event("next", new() { [252] = 670, [0] = 402 }, 5_000_000);
            Event("next", new() { [252] = 90, [0] = 402, [2] = new short[] { 6661 } }, 5_050_000);
            Event("next", new() { [252] = 6, [0] = 900, [6] = 402, [2] = -100f }, 5_100_000);
            Event("next", new() { [252] = 6, [0] = 10, [6] = 402, [2] = 25f }, 5_150_000);
            local = local with { ObjectId = 432, Connection = "next", ObservedAtMicroseconds = 5_300_000 };
            combat.Observe(new(3, 1, 0, new() { [253] = 2, [0] = 432 }), "next", "join", 5_300_000, local, roster);
            assert(combat.Diagnostics["pendingUnidentified"] == 2);
            Event("next", new() { [252] = 29, [0] = 402, [1] = "Mate", [7] = memberId.ToByteArray() }, 6_000_000);
            combat.FlushUnidentified();
            assert(combat.Snapshot!.TotalDamage == 100 && combat.Snapshot.TotalHealing == 25);
            assert(combat.Snapshot.Members.Single(member => member.Id == memberId.ToString()).Damage == 100);
            assert(combat.Snapshot.Members.All(member => !member.Id.StartsWith("unidentified:")));
            assert(combat.Snapshot.WeaponContributions!.Single(weapon => weapon.WeaponIndex == 6661).Damage == 100);
            assert(combat.Diagnostics["pendingUnidentified"] == 0);
        });
    }
}
