using AlbionMeter.Core.Capture;
using AlbionMeter.Server;
using Microsoft.Data.Sqlite;
using Microsoft.Extensions.Configuration;

internal static class PacketStorageChecks
{
    public static void Run(Action<string, Action> test, Action<bool> check)
    {
        IConfiguration Settings(string folder, bool debug) => new ConfigurationBuilder().AddInMemoryCollection(new Dictionary<string, string?> { ["Storage:Directory"] = folder, ["Capture:Debug"] = debug.ToString() }).Build();
        test("Normal mode never creates a packet database or raw capture files", () =>
        {
            var folder = Path.Combine(Path.GetTempPath(), $"albion-no-packets-{Guid.NewGuid():N}");
            using (var store = new PacketDebugStore(Settings(folder, false)))
            {
                check(!store.Enabled);
                for (var i = 0; i < 256; i++) store.Add(new(i, 4, 101, [1, 2, 3, 4]));
                store.Flush();
            }
            check(!Directory.Exists(folder));
        });
        test("Debug mode stores exact packet bytes in a bounded packets table", () =>
        {
            var folder = Path.Combine(Path.GetTempPath(), $"albion-debug-packets-{Guid.NewGuid():N}");
            try
            {
                using (var store = new PacketDebugStore(Settings(folder, true)))
                {
                    check(store.Enabled);
                    for (var i = 0; i < PacketDebugStore.MaximumPackets + 300; i++) store.Add(new(i, 4, 101, [1, 2, 3, 4]));
                }
                using var db = new SqliteConnection($"Data Source={Path.Combine(folder, "encounters.db")}"); db.Open();
                using var command = db.CreateCommand(); command.CommandText = "SELECT COUNT(*) FROM packets";
                check(Convert.ToInt64(command.ExecuteScalar()) == PacketDebugStore.MaximumPackets);
                command.CommandText = "SELECT data FROM packets ORDER BY id DESC LIMIT 1";
                check(((byte[])command.ExecuteScalar()!).SequenceEqual(new byte[] { 1, 2, 3, 4 }));
                command.CommandText = "SELECT MIN(timestamp_us) FROM packets"; check(Convert.ToInt64(command.ExecuteScalar()) == 300);
                check(!Directory.EnumerateFiles(folder, "*.pcap*", SearchOption.AllDirectories).Any());
            }
            finally { SqliteConnection.ClearAllPools(); Directory.Delete(folder, true); }
        });
        test("Debug payload retention is limited by bytes as well as packet count", () =>
        {
            var folder = Path.Combine(Path.GetTempPath(), $"albion-debug-byte-limit-{Guid.NewGuid():N}");
            try
            {
                using (var store = new PacketDebugStore(Settings(folder, true)))
                    for (var i = 0; i < 600; i++) store.Add(new(i, 65536, 101, new byte[65536]));
                using var db = new SqliteConnection($"Data Source={Path.Combine(folder, "encounters.db")}"); db.Open();
                using var command = db.CreateCommand(); command.CommandText = "SELECT SUM(LENGTH(data)) FROM packets";
                check(Convert.ToInt64(command.ExecuteScalar()) <= PacketDebugStore.MaximumBytes);
            }
            finally { SqliteConnection.ClearAllPools(); Directory.Delete(folder, true); }
        });
        test("Normal mode removes legacy recordings without removing player context or user files", () =>
        {
            var folder = Path.Combine(Path.GetTempPath(), $"albion-capture-cleanup-{Guid.NewGuid():N}"); Directory.CreateDirectory(folder);
            try
            {
                var generated = Path.Combine(folder, $"{Guid.NewGuid():N}.pcap");
                File.WriteAllBytes(generated, [1, 2, 3]); File.WriteAllText(generated + ".json", "{}");
                File.WriteAllText(Path.Combine(folder, "local-party.json"), "{}"); File.WriteAllText(Path.Combine(folder, "local-character.json"), "{}");
                File.WriteAllBytes(Path.Combine(folder, "user-recording.pcap"), [1]);
                check(CaptureStorage.RemoveLegacyRecordings(folder) == 5);
                check(!File.Exists(generated) && !File.Exists(generated + ".json"));
                check(File.Exists(Path.Combine(folder, "local-character.json")) && File.Exists(Path.Combine(folder, "local-party.json")) && File.Exists(Path.Combine(folder, "user-recording.pcap")));
            }
            finally { Directory.Delete(folder, true); }
        });
        test("Disabling debug clears diagnostic packets while preserving combat history", () =>
        {
            var folder = Path.Combine(Path.GetTempPath(), $"albion-debug-disabled-{Guid.NewGuid():N}");
            try
            {
                using (var debug = new PacketDebugStore(Settings(folder, true))) debug.Add(new(1, 1, 101, [1]));
                var store = new EncounterStore(Settings(folder, false));
                using var db = new SqliteConnection($"Data Source={store.DatabasePath}"); db.Open();
                using var command = db.CreateCommand(); command.CommandText = "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'packets'";
                check(Convert.ToInt64(command.ExecuteScalar()) == 0);
                command.CommandText = "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'encounters'";
                check(Convert.ToInt64(command.ExecuteScalar()) == 1);
            }
            finally { SqliteConnection.ClearAllPools(); Directory.Delete(folder, true); }
        });
        test("Linux socket interfaces are available independently of libpcap", () =>
        {
            if (!OperatingSystem.IsLinux()) return;
            var devices = LinuxSocketCapture.Devices();
            check(devices.Any(device => device.Name == "raw:any"));
            check(PacketCapture.DefaultDevice(devices) == "raw:any");
            check(PacketCapture.Library("raw:any") == "Linux packet socket");
        });
        test("Restored character context cannot reuse an old connection or object identifier", () =>
        {
            var character = new AlbionMeter.Core.Observation.LocalCharacterObservation();
            character.Restore(new(Guid.NewGuid(), 42, "Alpha", "Guild", "Alliance", "1315", null, "observed", 1000, "old-connection"));
            check(character.Snapshot is { Name: "Alpha", Guild: "Guild", ObjectId: null, Connection: "", LocationState: "last-observed" });
        });
    }
}
