using System.Text.Json;
using AlbionMeter.Core;
using Microsoft.Data.Sqlite;

namespace AlbionMeter.Server;

public sealed record EncounterSummary(string Id, DateTimeOffset StartedAt, double Duration,
    double Damage, double Healing, int Members, string Mode, DungeonLocation? Location = null);
public sealed record DungeonSummary(string Id, DateTimeOffset StartedAt, string Name, string EntranceMapId, string EntranceMapName, string Mode, string Status, int Floors, double Duration, double Damage, double Healing);

public sealed partial class EncounterStore
{
    public const int RetainedEncounters = 1000;
    public const int ListedEncounters = 500;
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly string _connectionString;
    public string DatabasePath { get; }
    public EncounterStore(IConfiguration configuration)
    {
        var directory = configuration["Storage:Directory"] ?? Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "AlbionMeter");
        Directory.CreateDirectory(directory);
        DatabasePath = Path.GetFullPath(Path.Combine(directory, "encounters.db"));
        _connectionString = new SqliteConnectionStringBuilder { DataSource = DatabasePath }.ToString();
        using var connection = Open();
        using var command = connection.CreateCommand();
        command.CommandText = "CREATE TABLE IF NOT EXISTS encounters (id TEXT PRIMARY KEY, started_at TEXT NOT NULL, summary TEXT NOT NULL, snapshot TEXT NOT NULL)";
        command.ExecuteNonQuery();
        command.CommandText = "CREATE TABLE IF NOT EXISTS dungeon_runs (id TEXT PRIMARY KEY, started_at TEXT NOT NULL, summary TEXT NOT NULL, snapshot TEXT NOT NULL)";
        command.ExecuteNonQuery();
        EnsureIdentitySchema(connection);
        if (!configuration.GetValue<bool>("Capture:Debug"))
        {
            command.CommandText = "DROP TABLE IF EXISTS packets; DELETE FROM identity_timeline";
            command.ExecuteNonQuery();
        }
    }

    public void Save(CombatSnapshot snapshot, bool allowEmpty = false)
    {
        if (snapshot.EventCount == 0 && !allowEmpty) return;
        using var connection = Open();
        using var transaction = connection.BeginTransaction();
        using var command = connection.CreateCommand();
        command.Transaction = transaction;
        command.CommandText = "INSERT OR REPLACE INTO encounters VALUES ($id, $date, $summary, $snapshot)";
        command.Parameters.AddWithValue("$id", snapshot.Id);
        command.Parameters.AddWithValue("$date", snapshot.StartedAt.ToString("O"));
        command.Parameters.AddWithValue("$summary", JsonSerializer.Serialize(new EncounterSummary(snapshot.Id,
            snapshot.StartedAt, snapshot.ElapsedSeconds, snapshot.TotalDamage, snapshot.TotalHealing, snapshot.Members.Length + (snapshot.FormerMembers?.Length ?? 0), snapshot.Mode, snapshot.Location), JsonOptions));
        command.Parameters.AddWithValue("$snapshot", JsonSerializer.Serialize(snapshot with { Status = "completed", Full = true }, JsonOptions));
        command.ExecuteNonQuery();
        ReapplyIdentities(connection, transaction, snapshot.Id);
        command.CommandText = $"DELETE FROM encounters WHERE id IN (SELECT id FROM encounters ORDER BY (COALESCE(json_extract(summary, '$.damage'), 0) + COALESCE(json_extract(summary, '$.healing'), 0)) > 0, started_at LIMIT MAX(0, (SELECT COUNT(*) FROM encounters) - {RetainedEncounters}))";
        command.Parameters.Clear();
        command.ExecuteNonQuery();
        transaction.Commit();
    }

    public List<EncounterSummary> List()
    {
        using var connection = Open();
        using var command = connection.CreateCommand();
        command.CommandText = $"SELECT summary FROM encounters ORDER BY started_at DESC LIMIT {ListedEncounters}";
        using var reader = command.ExecuteReader();
        List<EncounterSummary> result = [];
        while (reader.Read()) result.Add(JsonSerializer.Deserialize<EncounterSummary>(reader.GetString(0), JsonOptions)!);
        return result;
    }

    public string? Get(string id)
    {
        using var connection = Open();
        using var command = connection.CreateCommand();
        command.CommandText = "SELECT snapshot FROM encounters WHERE id = $id";
        command.Parameters.AddWithValue("$id", id);
        return command.ExecuteScalar() as string;
    }

    public static DungeonSummary Summarize(DungeonRunSnapshot run) => new(run.Id, run.StartedAt, run.Name, run.EntranceMapId,
        run.EntranceMapName, run.Mode, run.Status, run.Floors.Length, run.Totals.ElapsedSeconds, run.Totals.TotalDamage, run.Totals.TotalHealing);

    public void SaveDungeon(DungeonRunSnapshot run, bool allowEmpty = false)
    {
        if (run.Totals.EventCount == 0 && !allowEmpty) return;
        using var connection = Open();
        using var transaction = connection.BeginTransaction();
        using var command = connection.CreateCommand();
        command.Transaction = transaction;
        command.CommandText = "INSERT OR REPLACE INTO dungeon_runs VALUES ($id, $date, $summary, $snapshot)";
        command.Parameters.AddWithValue("$id", run.Id);
        command.Parameters.AddWithValue("$date", run.StartedAt.ToString("O"));
        command.Parameters.AddWithValue("$summary", JsonSerializer.Serialize(Summarize(run), JsonOptions));
        command.Parameters.AddWithValue("$snapshot", JsonSerializer.Serialize(run, JsonOptions));
        command.ExecuteNonQuery();
        command.Parameters.Clear();
        command.CommandText = "DELETE FROM dungeon_runs WHERE id NOT IN (SELECT id FROM dungeon_runs ORDER BY started_at DESC LIMIT 100)";
        command.ExecuteNonQuery();
        transaction.Commit();
    }

    public List<DungeonSummary> ListDungeons()
    {
        using var connection = Open();
        using var command = connection.CreateCommand();
        command.CommandText = "SELECT summary FROM dungeon_runs ORDER BY started_at DESC LIMIT 100";
        using var reader = command.ExecuteReader();
        List<DungeonSummary> runs = [];
        while (reader.Read()) runs.Add(JsonSerializer.Deserialize<DungeonSummary>(reader.GetString(0), JsonOptions)!);
        return runs;
    }

    public DungeonRunSnapshot? GetDungeon(string id)
    {
        using var connection = Open();
        using var command = connection.CreateCommand();
        command.CommandText = "SELECT snapshot FROM dungeon_runs WHERE id = $id";
        command.Parameters.AddWithValue("$id", id);
        return command.ExecuteScalar() is string json ? JsonSerializer.Deserialize<DungeonRunSnapshot>(json, JsonOptions) : null;
    }

    private SqliteConnection Open()
    {
        var connection = new SqliteConnection(_connectionString);
        connection.Open();
        return connection;
    }
}
