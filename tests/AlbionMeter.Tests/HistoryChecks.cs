using System.IO.Compression;
using System.Text.Json;
using AlbionMeter.Core;
using AlbionMeter.Server;
using Microsoft.Extensions.Configuration;

internal static class HistoryChecks
{
    public static void Run(Action<string, Action> test, Action<bool> check)
    {
        EncounterStore Store(string folder) => new(new ConfigurationBuilder().AddInMemoryCollection(new Dictionary<string, string?> { ["Storage:Directory"] = folder }).Build());
        CombatSnapshot Snapshot()
        {
            var engine = new CombatEngine([new("a", "Alpha", "Hammer", "hammer", "#ffffff"), new("b", "Beta", "Staff", "staff", "#eeeeee")]);
            engine.Advance(3);
            engine.Add(new("hit", "a", 1, CombatMetric.Damage, 320, 9022, SourceItemIndex: 4308, SourceSlot: "chest", SourceKind: "ability"));
            engine.Add(new("heal", "b", 2, CombatMetric.Healing, 64, 7108, SourceItemIndex: 7108, SourceSlot: "mainhand", SourceKind: "ability"));
            return engine.Snapshot(true);
        }
        void Reject(Action action)
        {
            try { action(); } catch (InvalidDataException) { return; }
            throw new Exception("Expected an invalid history rejection.");
        }
        test("History ZIP and JSON round trips preserve dungeon floors source items healing and timelines", () =>
        {
            var directory = Path.Combine(Path.GetTempPath(), $"albion-history-{Guid.NewGuid():N}");
            try
            {
                var source = Store(Path.Combine(directory, "source")); var target = Store(Path.Combine(directory, "target"));
                var first = Snapshot(); source.Save(first);
                var run = new DungeonRun("Dungeon", "1315", "Entrance", first.Mode);
                var location = run.EnterFloor("one", "one", "First floor"); run.Record("one", first with { Location = location });
                location = run.EnterFloor("two", "two", "Second floor"); run.Record("two", Snapshot() with { Location = location });
                source.SaveDungeon(run.Snapshot("running"));
                var archive = source.ExportHistory();
                var bytes = HistoryArchiveFile.Write(archive);
                check(bytes[0] == 80 && bytes[1] == 75);
                var result = target.ImportHistory(HistoryArchiveFile.Read(bytes));
                check(result.ImportedEncounters == 1 && result.ImportedDungeons == 1);
                var restored = target.GetDungeon(run.Id)!;
                check(restored.Status == "completed" && restored.Floors.Length == 2);
                check(restored.Totals.TotalDamage == 640 && restored.Totals.TotalHealing == 128);
                check(restored.Floors[0].Encounters[0].ItemContributions!.Single(item => item.Slot == "chest").Damage == 320);
                check(restored.Totals.Timeline.Sum(point => point.Damage.Values.Sum()) == 640);
                var original = JsonSerializer.Serialize(first with { Status = "completed" }, new JsonSerializerOptions(JsonSerializerDefaults.Web));
                check(target.Get(first.Id) == original);
                var duplicate = target.ImportHistory(archive);
                check(duplicate.ImportedEncounters == 0 && duplicate.ImportedDungeons == 0 && duplicate.SkippedEncounters == 1 && duplicate.SkippedDungeons == 1);
                check(source.ExportHistory(encounterId: first.Id).Dungeons.Length == 0);
                check(source.ExportHistory(dungeonId: run.Id).Encounters.Length == 0);
                var json = JsonSerializer.SerializeToUtf8Bytes(archive, new JsonSerializerOptions(JsonSerializerDefaults.Web));
                check(HistoryArchiveFile.Read([239, 187, 191, .. json]).Dungeons.Single().Id == run.Id);
            }
            finally { Microsoft.Data.Sqlite.SqliteConnection.ClearAllPools(); Directory.Delete(directory, true); }
        });
        test("Invalid history never partially imports or replaces existing records", () =>
        {
            var directory = Path.Combine(Path.GetTempPath(), $"albion-history-invalid-{Guid.NewGuid():N}");
            try
            {
                var store = Store(directory); var valid = Snapshot();
                var archive = new HistoryArchive("albion-meter-history", 1, [valid, Snapshot() with { TotalDamage = -1 }], []);
                Reject(() => store.ImportHistory(archive)); check(store.List().Count == 0);
                Reject(() => HistoryArchiveFile.Write(archive with { Version = 2 }));
                Reject(() => HistoryArchiveFile.Read("{}"u8.ToArray()));
                Reject(() => HistoryArchiveFile.Read("invalid"u8.ToArray()));
                store.Save(valid);
                store.ImportHistory(new("albion-meter-history", 1, [valid with { TotalDamage = 999 }], []));
                check(JsonSerializer.Deserialize<CombatSnapshot>(store.Get(valid.Id)!, new JsonSerializerOptions(JsonSerializerDefaults.Web))!.TotalDamage == 320);
            }
            finally { Microsoft.Data.Sqlite.SqliteConnection.ClearAllPools(); Directory.Delete(directory, true); }
        });
        test("History archives reject unexpected ZIP entries and invalid timeline values", () =>
        {
            using var bytes = new MemoryStream();
            using (var zip = new ZipArchive(bytes, ZipArchiveMode.Create, true))
            using (var writer = new StreamWriter(zip.CreateEntry("../history.json").Open())) writer.Write("{}");
            Reject(() => HistoryArchiveFile.Read(bytes.ToArray()));
            var value = Snapshot();
            value.Timeline[0].Damage["a"] = double.NaN;
            Reject(() => HistoryArchiveFile.Validate(new("albion-meter-history", 1, [value], [])));
        });
    }
}
