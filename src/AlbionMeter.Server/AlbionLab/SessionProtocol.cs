using System.Text.Json.Serialization;

namespace AlbionLab.Protocol;

public static class Protocol
{
    public const int Version = 1;
    public const string HubPathPublish = "/hubs/publish";
    public const string HubPathWatch = "/hubs/watch";
    public const int MaximumPlayers = 100;
    public const int MaximumKeys = 5_000;
    public const int MaximumSnaps = 500;
    public const int MaximumSeconds = 4 * 3600;
    public const int MaximumEntriesPerSecond = 1_000;
    public const int MaximumDungeons = 100;
    public const int MaximumDeltaEntries = 5_000;
    public const int MaximumMessageBytes = 4 * 1024 * 1024;
}

public sealed class ProtocolException(string message) : Exception(message);

public sealed class PlayerInfo
{
    public string Name { get; set; } = "";
    public string Weapon { get; set; } = "";
    public string? WeaponId { get; set; }
    public string Family { get; set; } = "";
    public string Color { get; set; } = "#9ebbf3";
}

public sealed class SourceKey
{
    public string T { get; set; } = "p";
    public string P { get; set; } = "";
    public int? X { get; set; }
    public string? S { get; set; }
    public string? K { get; set; }
    public string? N { get; set; }
    public string? U { get; set; }
    public bool? O { get; set; }
}

public sealed class Snap
{
    public string Id { get; set; } = "";
    public DateTimeOffset StartedAt { get; set; }
    public double Elapsed { get; set; }
    public string Status { get; set; } = "running";
    public long Events { get; set; }
    public List<string>? Members { get; set; }
    public Dictionary<string, string>? Membership { get; set; }
    public SortedDictionary<int, List<double[]>>? Sec { get; set; }
}

public sealed class FloorInfo
{
    public string Id { get; set; } = "";
    public int Number { get; set; }
    public string? Map { get; set; }
    public List<string> Snaps { get; set; } = [];
}

public sealed class DungeonInfo
{
    public string Id { get; set; } = "";
    public string Name { get; set; } = "";
    public DateTimeOffset StartedAt { get; set; }
    public string Status { get; set; } = "running";
    public string? Entrance { get; set; }
    public List<FloorInfo> Floors { get; set; } = [];
}

public sealed class SessionDelta
{
    public int V { get; set; } = Protocol.Version;
    public long Seq { get; set; }
    public bool Reset { get; set; }
    public string? Status { get; set; }
    public DateTimeOffset? StartedAt { get; set; }
    public string? Self { get; set; }
    public Dictionary<string, PlayerInfo>? Players { get; set; }
    public Dictionary<int, SourceKey>? Keys { get; set; }
    public Dictionary<string, Snap>? Snaps { get; set; }
    public string? Record { get; set; }
    public List<DungeonInfo>? Dungeons { get; set; }
}

public sealed class SessionState
{
    public int V { get; set; } = Protocol.Version;
    public string Id { get; set; } = "";
    public long Seq { get; set; }
    public string Status { get; set; } = "running";
    public DateTimeOffset StartedAt { get; set; }
    public DateTimeOffset UpdatedAt { get; set; }
    public string? Self { get; set; }
    public Dictionary<string, PlayerInfo> Players { get; set; } = [];
    public List<SourceKey> Keys { get; set; } = [];
    public Dictionary<string, Snap> Snaps { get; set; } = [];
    public string? Record { get; set; }
    public List<DungeonInfo> Dungeons { get; set; } = [];

    public void Apply(SessionDelta delta, DateTimeOffset now)
    {
        if (delta.V != Protocol.Version) throw new ProtocolException($"Protocol version {delta.V} is not supported; update Albion Meter.");
        if (delta.Reset)
        {
            Players = []; Keys = []; Snaps = []; Dungeons = []; Record = null;
        }
        if (delta.Status is { } status) Status = status is "running" or "paused" or "completed" ? status : throw new ProtocolException("Unknown status.");
        if (delta.StartedAt is { } started) StartedAt = started;
        if (delta.Self is { } self) Self = Limit(self, 64);
        if (delta.Players is { } players)
            foreach (var (id, player) in players)
            {
                Players[Limit(id, 64)] = new PlayerInfo { Name = Limit(player.Name, 40), Weapon = Limit(player.Weapon, 80), WeaponId = player.WeaponId is null ? null : Limit(player.WeaponId, 60), Family = Limit(player.Family, 40), Color = Color(player.Color) };
                if (Players.Count > Protocol.MaximumPlayers) throw new ProtocolException("Too many players.");
            }
        if (delta.Keys is { } keys)
            foreach (var (index, key) in keys.OrderBy(pair => pair.Key))
            {
                if (key.T is not ("p" or "w" or "i")) throw new ProtocolException("Unknown key type.");
                var clean = new SourceKey { T = key.T, P = Limit(key.P, 64), X = key.X, S = key.S is null ? null : Limit(key.S, 20), K = key.K is null ? null : Limit(key.K, 20), N = key.N is null ? null : Limit(key.N, 80), U = key.U is null ? null : Limit(key.U, 60), O = key.O };
                if (index == Keys.Count) Keys.Add(clean);
                else if (index >= 0 && index < Keys.Count) Keys[index] = clean;
                else throw new ProtocolException($"Key {index} arrived before key {Keys.Count}.");
                if (Keys.Count > Protocol.MaximumKeys) throw new ProtocolException("Too many sources.");
            }
        if (delta.Snaps is { } snaps)
            foreach (var (id, incoming) in snaps)
            {
                if (!Snaps.TryGetValue(id, out var snap))
                {
                    if (Snaps.Count >= Protocol.MaximumSnaps) throw new ProtocolException("Too many segments.");
                    Snaps[id] = snap = new Snap { Id = Limit(id, 64), StartedAt = incoming.StartedAt, Sec = [] };
                }
                snap.StartedAt = incoming.StartedAt == default ? snap.StartedAt : incoming.StartedAt;
                snap.Elapsed = Math.Clamp(incoming.Elapsed, 0, Protocol.MaximumSeconds);
                snap.Status = incoming.Status is "running" or "paused" or "completed" or "idle" ? incoming.Status : snap.Status;
                snap.Events = Math.Max(0, incoming.Events);
                if (incoming.Members is { } members) snap.Members = [.. members.Take(Protocol.MaximumPlayers).Select(member => Limit(member, 64))];
                if (incoming.Membership is { } membership) snap.Membership = membership.Take(Protocol.MaximumPlayers).ToDictionary(pair => Limit(pair.Key, 64), pair => pair.Value is "left" or "unconfirmed" ? pair.Value : "active");
                if (incoming.Sec is { } seconds)
                {
                    snap.Sec ??= [];
                    foreach (var (second, entries) in seconds)
                    {
                        if (second < 0 || second > Protocol.MaximumSeconds) throw new ProtocolException("Second out of range.");
                        if (entries.Count > Protocol.MaximumEntriesPerSecond) throw new ProtocolException("Too many entries in one second.");
                        foreach (var entry in entries)
                            if (entry.Length != 3 || entry[0] < 0 || entry[0] >= Keys.Count || entry[0] != Math.Floor(entry[0]) || !double.IsFinite(entry[1]) || !double.IsFinite(entry[2]) || entry[1] < 0 || entry[2] < 0)
                                throw new ProtocolException("Invalid entry.");
                        if (entries.Count == 0) snap.Sec.Remove(second); else snap.Sec[second] = entries;
                    }
                }
            }
        if (delta.Record is { } record) Record = Limit(record, 64);
        if (delta.Dungeons is { } dungeons)
        {
            if (dungeons.Count > Protocol.MaximumDungeons) throw new ProtocolException("Too many dungeons.");
            Dungeons = [.. dungeons.Select(dungeon => new DungeonInfo
            {
                Id = Limit(dungeon.Id, 64), Name = Limit(dungeon.Name, 60), StartedAt = dungeon.StartedAt, Status = Limit(dungeon.Status, 16),
                Entrance = dungeon.Entrance is null ? null : Limit(dungeon.Entrance, 60),
                Floors = [.. dungeon.Floors.Take(50).Select(floor => new FloorInfo { Id = Limit(floor.Id, 64), Number = floor.Number, Map = floor.Map is null ? null : Limit(floor.Map, 60), Snaps = [.. floor.Snaps.Take(200).Select(snap => Limit(snap, 64))] })],
            })];
        }
        Seq = Math.Max(Seq, delta.Seq);
        UpdatedAt = now;
    }

    public static int CountEntries(SessionDelta delta) =>
        delta.Snaps?.Values.Sum(snap => snap.Sec?.Values.Sum(entries => entries.Count) ?? 0) ?? 0;

    public SessionSummary Summarize()
    {
        double damage = 0, healing = 0;
        var snap = Record is { } record && Snaps.TryGetValue(record, out var found) ? found : null;
        foreach (var entries in snap?.Sec?.Values ?? Enumerable.Empty<List<double[]>>())
            foreach (var entry in entries)
                if (Keys[(int)entry[0]].T == "p") { damage += entry[1]; healing += entry[2]; }
        return new(snap?.Elapsed ?? 0, damage, healing, snap?.Members?.Count ?? Players.Count, Dungeons.Count);
    }

    public Dictionary<string, PlayerInfo> AnonymizedPlayers()
    {
        var index = 0;
        return Players.OrderBy(pair => pair.Key, StringComparer.Ordinal).ToDictionary(pair => pair.Key, pair => pair.Key == Self ? pair.Value
            : new PlayerInfo { Name = $"Player {++index}", Weapon = pair.Value.Weapon, WeaponId = pair.Value.WeaponId, Family = pair.Value.Family, Color = pair.Value.Color });
    }

    static string Limit(string? value, int length) => value is null ? "" : value.Length <= length ? value : value[..length];

    static string Color(string? value) => value is { Length: 7 } && value[0] == '#' && value[1..].All(Uri.IsHexDigit) ? value : "#9ebbf3";
}

public sealed record SessionSummary(double Duration, double Damage, double Healing, int Players, int Dungeons);

[JsonSerializable(typeof(SessionState))]
[JsonSerializable(typeof(SessionDelta))]
[JsonSourceGenerationOptions(PropertyNamingPolicy = JsonKnownNamingPolicy.CamelCase, DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull)]
public sealed partial class ProtocolJson : JsonSerializerContext;
