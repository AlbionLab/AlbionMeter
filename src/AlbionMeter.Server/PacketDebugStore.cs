using AlbionMeter.Core.Capture;
using Microsoft.Data.Sqlite;

namespace AlbionMeter.Server;

public sealed class PacketDebugStore : IDisposable
{
    public const long MaximumBytes = 32_000_000;
    public const int MaximumPackets = 10_000;
    private readonly SqliteConnection? _connection;
    private readonly List<CaptureFrame> _pending = [];
    private readonly string _session = Guid.NewGuid().ToString("N");
    public bool Enabled { get; }
    public PacketDebugStore(IConfiguration configuration)
    {
        Enabled = configuration.GetValue<bool>("Capture:Debug");
        if (!Enabled) return;
        var directory = configuration["Storage:Directory"] ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "AlbionMeter");
        Directory.CreateDirectory(directory);
        _connection = new(new SqliteConnectionStringBuilder { DataSource = Path.Combine(directory, "encounters.db") }.ToString());
        _connection.Open();
        using var command = _connection.CreateCommand();
        command.CommandText = "CREATE TABLE IF NOT EXISTS packets (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, timestamp_us INTEGER NOT NULL, link_type INTEGER NOT NULL, original_length INTEGER NOT NULL, data BLOB NOT NULL)";
        command.ExecuteNonQuery();
    }
    public void Add(CaptureFrame frame)
    {
        if (!Enabled) return;
        _pending.Add(frame);
        if (_pending.Count >= 128) Flush();
    }
    public void Flush()
    {
        if (_connection is null || _pending.Count == 0) return;
        using var transaction = _connection.BeginTransaction();
        using var command = _connection.CreateCommand(); command.Transaction = transaction;
        command.CommandText = "INSERT INTO packets (session_id, timestamp_us, link_type, original_length, data) VALUES ($session, $time, $link, $length, $data)";
        foreach (var frame in _pending)
        {
            command.Parameters.Clear();
            command.Parameters.AddWithValue("$session", _session); command.Parameters.AddWithValue("$time", frame.TimestampMicroseconds);
            command.Parameters.AddWithValue("$link", frame.LinkType); command.Parameters.AddWithValue("$length", frame.OriginalLength); command.Parameters.AddWithValue("$data", frame.Data);
            command.ExecuteNonQuery();
        }
        command.Parameters.Clear();
        command.CommandText = $"DELETE FROM packets WHERE id NOT IN (SELECT id FROM packets ORDER BY id DESC LIMIT {MaximumPackets})";
        command.ExecuteNonQuery();
        command.CommandText = "SELECT COALESCE(SUM(LENGTH(data)), 0) FROM packets";
        var bytes = Convert.ToInt64(command.ExecuteScalar());
        if (bytes > MaximumBytes)
        {
            command.CommandText = "DELETE FROM packets WHERE id IN (SELECT id FROM (SELECT id, SUM(LENGTH(data)) OVER (ORDER BY id) AS removed FROM packets) WHERE removed <= $excess)";
            command.Parameters.AddWithValue("$excess", bytes - MaximumBytes + 262144);
            command.ExecuteNonQuery();
        }
        transaction.Commit(); _pending.Clear();
    }
    public void Dispose()
    {
        try { Flush(); } finally { _connection?.Dispose(); }
    }
}
