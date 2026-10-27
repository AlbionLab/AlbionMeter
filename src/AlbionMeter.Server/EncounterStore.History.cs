using System.Text.Json;
using AlbionMeter.Core;
using Microsoft.Data.Sqlite;

namespace AlbionMeter.Server;

public sealed partial class EncounterStore
{
    public HistoryArchive ExportHistory(string? encounterId = null, string? dungeonId = null)
    {
        if (encounterId is not null && dungeonId is not null) throw new ArgumentException("Choose one record to export.");
        using var connection = Open();
        using var transaction = connection.BeginTransaction();
        T[] Read<T>(string table, string? id)
        {
            using var command = connection.CreateCommand();
            command.Transaction = transaction;
            command.CommandText = $"SELECT snapshot FROM {table}" + (id is null ? " ORDER BY started_at DESC" : " WHERE id = $id");
            if (id is not null) command.Parameters.AddWithValue("$id", id);
            using var reader = command.ExecuteReader();
            List<T> values = [];
            while (reader.Read()) values.Add(JsonSerializer.Deserialize<T>(reader.GetString(0), JsonOptions)!);
            if (id is not null && values.Count == 0) throw new KeyNotFoundException("The record was not found.");
            return values.ToArray();
        }
        var encounters = dungeonId is null ? Read<CombatSnapshot>("encounters", encounterId) : [];
        var dungeons = encounterId is null ? Read<DungeonRunSnapshot>("dungeon_runs", dungeonId) : [];
        transaction.Commit();
        return new("albion-meter-history", 1, encounters, dungeons);
    }

    public HistoryImportResult ImportHistory(HistoryArchive archive)
    {
        HistoryArchiveFile.Validate(archive);
        using var connection = Open();
        using var transaction = connection.BeginTransaction();
        int Count(string table)
        {
            using var count = connection.CreateCommand(); count.Transaction = transaction;
            count.CommandText = $"SELECT COUNT(*) FROM {table}";
            return Convert.ToInt32(count.ExecuteScalar());
        }
        var encounterCount = Count("encounters");
        var dungeonCount = Count("dungeon_runs");
        var addedEncounters = 0; var addedDungeons = 0;
        bool Insert(string table, string id, DateTimeOffset startedAt, object summary, object snapshot, int count, int limit)
        {
            if (count >= limit) return false;
            using var write = connection.CreateCommand(); write.Transaction = transaction;
            write.CommandText = $"INSERT OR IGNORE INTO {table} (id, started_at, summary, snapshot) VALUES ($id, $date, $summary, $snapshot)";
            write.Parameters.AddWithValue("$id", id); write.Parameters.AddWithValue("$date", startedAt.ToString("O"));
            write.Parameters.AddWithValue("$summary", JsonSerializer.Serialize(summary, JsonOptions));
            write.Parameters.AddWithValue("$snapshot", JsonSerializer.Serialize(snapshot, JsonOptions));
            return write.ExecuteNonQuery() > 0;
        }
        foreach (var source in archive.Encounters)
        {
            var snapshot = source with { Status = "completed" };
            var summary = new EncounterSummary(snapshot.Id, snapshot.StartedAt, snapshot.ElapsedSeconds, snapshot.TotalDamage, snapshot.TotalHealing, snapshot.Members.Length + (snapshot.FormerMembers?.Length ?? 0), snapshot.Mode, snapshot.Location);
            if (Insert("encounters", snapshot.Id, snapshot.StartedAt, summary, snapshot, encounterCount, RetainedEncounters)) { encounterCount++; addedEncounters++; }
        }
        foreach (var source in archive.Dungeons)
        {
            var run = source with { Status = "completed", Totals = source.Totals with { Status = "completed" }, Floors = source.Floors.Select(floor => floor with { Totals = floor.Totals with { Status = "completed" }, Encounters = floor.Encounters.Select(encounter => encounter with { Status = "completed" }).ToArray() }).ToArray() };
            if (Insert("dungeon_runs", run.Id, run.StartedAt, Summarize(run), run, dungeonCount, 100)) { dungeonCount++; addedDungeons++; }
        }
        transaction.Commit();
        return new(addedEncounters, addedDungeons, archive.Encounters.Length - addedEncounters, archive.Dungeons.Length - addedDungeons);
    }
}
