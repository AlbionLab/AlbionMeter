using System.Buffers.Binary;
using System.Text;
using AlbionMeter.Core.Capture;

internal static class ManualRecordingChecks
{
    public static void Run(Action<string, Action> test, Action<bool> check)
    {
        test("Manual recording ignores pre-Play hits, preserves Pause/Resume and resets after Stop", () =>
        {
            void Assert(bool condition, [System.Runtime.CompilerServices.CallerLineNumber] int line = 0) { if (!condition) throw new Exception($"Manual record assertion at line {line}"); }
            var analysis = new CaptureAnalysis(manualRecording: true);
            analysis.BeginCapture(1_000_000);
            var sequence = 0u;
            void Send(byte kind, Dictionary<byte, object?> values, long at) => analysis.Add(Frame(kind, values, at, ++sequence));
            var character = Guid.NewGuid();
            Send(3, new() { [253] = (byte)2, [0] = 10, [1] = character.ToByteArray(), [2] = "Alpha", [8] = "@RANDOMDUNGEON@one", [66] = "1315" }, 1_100_000);
            void Hit(float delta, long at) => Send(4, new() { [252] = (byte)6, [0] = 30, [6] = 10, [2] = delta, [7] = -1, [5] = (byte)0 }, at);
            Hit(-900, 1_200_000);
            Assert(analysis.RecordState == "idle" && analysis.Snapshot().Combat!.TotalDamage == 0 && analysis.Snapshot().Dungeons!.Length == 0);
            analysis.StartRecord(2_000_000);
            var id = analysis.Snapshot().Combat!.Id;
            Assert(analysis.Snapshot().Dungeons!.Length == 1);
            Hit(-100, 3_000_000); Hit(40, 4_000_000);
            analysis.SetPaused(true, 5_000_000);
            Hit(-900, 6_000_000);
            Assert(analysis.RecordState == "paused" && analysis.Snapshot().Combat!.TotalDamage == 100 && analysis.Snapshot().Combat!.ElapsedSeconds == 3);
            analysis.SetPaused(false, 8_000_000); Hit(-25, 9_000_000);
            Assert(analysis.Snapshot().Combat!.Id == id && analysis.Snapshot().Combat!.TotalDamage == 125 && analysis.Snapshot().Combat!.TotalHealing == 40);
            var dungeon = analysis.Snapshot().Dungeons!.Single();
            analysis.StopRecord(10_000_000); Hit(-500, 11_000_000);
            Assert(analysis.RecordState == "completed" && analysis.Snapshot().Combat!.TotalDamage == 125 && analysis.Snapshot().Combat!.ElapsedSeconds == 5);
            Assert(analysis.Snapshot().Dungeons!.Single().Totals.TotalDamage == 125);
            analysis.StartRecord(12_000_000);
            var next = analysis.Snapshot();
            Assert(next.Combat!.Id != id && next.Combat.TotalDamage == 0 && next.Combat.TotalHealing == 0 && next.Combat.EventCount == 0 && next.Combat.ElapsedSeconds == 0);
            Assert(next.Combat.Timeline.All(point => point.Damage.Values.Sum() == 0 && point.Healing.Values.Sum() == 0) && next.Combat.ItemContributions!.Length == 0 && next.Combat.WeaponContributions!.Length == 0);
            Assert(next.Dungeons!.Single().Id != dungeon.Id && next.Dungeons!.Single().Totals.TotalDamage == 0);
            Hit(-10, 13_000_000);
            Assert(analysis.Snapshot().Combat!.TotalDamage == 10 && analysis.Snapshot().Combat!.TotalHealing == 0 && analysis.Snapshot().Dungeons!.Single().Totals.TotalDamage == 10);
        });
    }
    public static void RunClock(Action<string, Action> test)
    {
        test("The record clock runs from Play without packets and stays frozen while paused", () =>
        {
            void Assert(bool condition, [System.Runtime.CompilerServices.CallerLineNumber] int line = 0) { if (!condition) throw new Exception($"Clock assertion at line {line}"); }
            var analysis = new CaptureAnalysis(manualRecording: true);
            analysis.BeginCapture(1_000_000);
            analysis.AdvanceClock(1_500_000);
            Assert(analysis.Snapshot().Combat is null || analysis.Snapshot().Combat!.ElapsedSeconds == 0);
            analysis.StartRecord(2_000_000);
            analysis.AdvanceClock(7_000_000);
            Assert(analysis.Snapshot().Combat!.ElapsedSeconds == 5);
            analysis.SetPaused(true, 8_000_000);
            Assert(analysis.Snapshot().Combat!.ElapsedSeconds == 6);
            analysis.AdvanceClock(20_000_000);
            Assert(analysis.Snapshot().Combat!.ElapsedSeconds == 6);
            analysis.SetPaused(false, 30_000_000);
            analysis.AdvanceClock(33_000_000);
            Assert(analysis.Snapshot().Combat!.ElapsedSeconds == 9);
            // A packet stamped a little before the last tick never moves the clock back.
            analysis.AdvanceClock(32_000_000);
            Assert(analysis.Snapshot().Combat!.ElapsedSeconds == 9);
        });
    }
    public static void RunCaptureDrop(Action<string, Action> test)
    {
        test("A capture that drops and comes back keeps the same record and its players", () =>
        {
            void Assert(bool condition, [System.Runtime.CompilerServices.CallerLineNumber] int line = 0) { if (!condition) throw new Exception($"Capture drop assertion at line {line}"); }
            var analysis = new CaptureAnalysis(manualRecording: true);
            analysis.BeginCapture(1_000_000);
            var sequence = 0u;
            void Send(byte kind, Dictionary<byte, object?> values, long at) => analysis.Add(Frame(kind, values, at, ++sequence));
            var character = Guid.NewGuid();
            Send(3, new() { [253] = (byte)2, [0] = 10, [1] = character.ToByteArray(), [2] = "Alpha", [8] = "@RANDOMDUNGEON@one", [66] = "1315" }, 1_100_000);
            void Hit(float delta, long at) => Send(4, new() { [252] = (byte)6, [0] = 30, [6] = 10, [2] = delta, [7] = -1, [5] = (byte)0 }, at);
            analysis.StartRecord(2_000_000);
            Hit(-100, 3_000_000);
            var before = analysis.Snapshot().Combat!;
            // The capture source ends by itself: the service pauses the record, restarts the capture keeping what it knew, and resumes.
            analysis.SetPaused(true, 4_000_000);
            analysis.BeginCapture(7_000_000, preserveRecord: true, keepIdentities: true);
            analysis.SetPaused(false, 7_000_000);
            Hit(-50, 8_000_000);
            var after = analysis.Snapshot().Combat!;
            Assert(analysis.RecordState == "running" && after.Id == before.Id);
            Assert(after.TotalDamage == 150 && after.Members.Any(member => member.Name == "Alpha" && member.Damage == 150));
        });
    }
    public static void RunReset(Action<string, Action> test)
    {
        test("Reset clears damage and healing but keeps the record, its players and the pause", () =>
        {
            void Assert(bool condition, [System.Runtime.CompilerServices.CallerLineNumber] int line = 0) { if (!condition) throw new Exception($"Reset assertion at line {line}"); }
            var analysis = new CaptureAnalysis(manualRecording: true);
            analysis.BeginCapture(1_000_000);
            var sequence = 0u;
            void Send(byte kind, Dictionary<byte, object?> values, long at) => analysis.Add(Frame(kind, values, at, ++sequence));
            var character = Guid.NewGuid();
            Send(3, new() { [253] = (byte)2, [0] = 10, [1] = character.ToByteArray(), [2] = "Alpha", [8] = "@RANDOMDUNGEON@one", [66] = "1315" }, 1_100_000);
            void Hit(float delta, long at) => Send(4, new() { [252] = (byte)6, [0] = 30, [6] = 10, [2] = delta, [7] = -1, [5] = (byte)0 }, at);
            analysis.StartRecord(2_000_000);
            Hit(-100, 3_000_000); Hit(40, 4_000_000);
            var id = analysis.Snapshot().Combat!.Id;
            analysis.ResetCounts(5_000_000);
            var reset = analysis.Snapshot().Combat!;
            Assert(analysis.RecordState == "running" && reset.Id == id && reset.TotalDamage == 0 && reset.TotalHealing == 0);
            Assert(reset.Members.Any(member => member.Name == "Alpha" && member.Damage == 0));
            Hit(-30, 6_000_000);
            var after = analysis.Snapshot().Combat!;
            Assert(after.Id == id && after.TotalDamage == 30 && after.Members.Any(member => member.Name == "Alpha" && member.Damage == 30));
            Assert(analysis.Snapshot().Dungeons!.Single().Totals.TotalDamage == 30);
            analysis.SetPaused(true, 7_000_000);
            analysis.ResetCounts(8_000_000);
            Hit(-500, 9_000_000);
            Assert(analysis.RecordState == "paused" && analysis.Snapshot().Combat!.TotalDamage == 0 && analysis.Snapshot().Combat!.Id == id);
        });
    }
    public static void RunRestart(Action<string, Action> test)
    {
        test("A meter reopened on the same map names the players it knew before closing", () =>
        {
            void Assert(bool condition, [System.Runtime.CompilerServices.CallerLineNumber] int line = 0) { if (!condition) throw new Exception($"Restart assertion at line {line}"); }
            var sequence = 0u;
            var character = Guid.NewGuid();
            var before = new CaptureAnalysis(manualRecording: true);
            before.BeginCapture(1_000_000);
            before.Add(Frame(3, new() { [253] = (byte)2, [0] = 10, [1] = character.ToByteArray(), [2] = "Alpha", [8] = "@RANDOMDUNGEON@one", [66] = "1315" }, 1_100_000, ++sequence));
            var known = before.KnownPlayers();
            Assert(known.Any(player => player.Name == "Alpha" && player.ObjectId == 10));
            // The meter closes and opens again in the middle of the same floor: no zone change announces the players.
            var after = new CaptureAnalysis(manualRecording: true);
            after.RestoreCharacter(before.Snapshot().Character!);
            after.RestorePlayers(known);
            after.BeginCapture(5_000_000);
            after.StartRecord(6_000_000);
            after.Add(Frame(4, new() { [252] = (byte)6, [0] = 30, [6] = 10, [2] = -70f, [7] = -1, [5] = (byte)0 }, 7_000_000, ++sequence));
            Assert(after.Snapshot().Combat!.Members.Any(member => member.Name == "Alpha" && member.Damage == 70));
        });
    }
    private static CaptureFrame Frame(byte kind, Dictionary<byte, object?> values, long at, uint sequence)
    {
        using var stream = new MemoryStream(); using var writer = new BinaryWriter(stream);
        writer.Write((byte)0xf3); writer.Write(kind); writer.Write((byte)1);
        if (kind == 3) { writer.Write((short)0); writer.Write((byte)8); }
        writer.Write((byte)values.Count);
        foreach (var (key, value) in values)
        {
            writer.Write(key);
            switch (value)
            {
                case byte number: writer.Write((byte)3); writer.Write(number); break;
                case int number: writer.Write((byte)9); Var(writer, (uint)((number << 1) ^ (number >> 31))); break;
                case float number: writer.Write((byte)5); writer.Write(number); break;
                case byte[] bytes: writer.Write((byte)67); Var(writer, (uint)bytes.Length); writer.Write(bytes); break;
                case string text: var encoded = Encoding.UTF8.GetBytes(text); writer.Write((byte)7); Var(writer, (uint)encoded.Length); writer.Write(encoded); break;
                default: throw new InvalidOperationException();
            }
        }
        var payload = stream.ToArray(); var packet = new byte[20 + 8 + 24 + payload.Length];
        packet[0] = 0x45; packet[9] = 17; BinaryPrimitives.WriteUInt16BigEndian(packet.AsSpan(2), (ushort)packet.Length);
        packet[12] = 192; packet[15] = 1; packet[16] = 192; packet[19] = 2;
        BinaryPrimitives.WriteUInt16BigEndian(packet.AsSpan(20), 5056); BinaryPrimitives.WriteUInt16BigEndian(packet.AsSpan(22), 12345);
        BinaryPrimitives.WriteUInt16BigEndian(packet.AsSpan(24), (ushort)(packet.Length - 20));
        packet[31] = 1; packet[39] = 1; packet[40] = 6;
        BinaryPrimitives.WriteUInt32BigEndian(packet.AsSpan(44), (uint)(12 + payload.Length)); BinaryPrimitives.WriteUInt32BigEndian(packet.AsSpan(48), sequence);
        payload.CopyTo(packet, 52); return new(at, (uint)packet.Length, 101, packet);
    }
    private static void Var(BinaryWriter writer, uint value)
    {
        while (value >= 128) { writer.Write((byte)((value & 127) | 128)); value >>= 7; }
        writer.Write((byte)value);
    }
}
