using System.Security.Cryptography;
using System.Text;
using AlbionMeter.Core.Capture;

namespace AlbionMeter.Core.Observation;

public sealed class DungeonCombatObservation(CombatSourceCatalog? sources = null)
{
    private readonly CapturedCombatObservation _combat = new(sources);
    private readonly List<(Guid Character, string Entrance, DungeonRun Run)> _runs = [];
    private DungeonRun? _active;
    private Guid? _character;
    private string? _map;
    private string? _outsideMap;
    private long _latestTimestamp;
    private bool _paused;
    private bool _recording = true;
    private string? _recordId;
    private (Guid Character, string Map, string Entrance)? _lastLocation;
    private sealed record Arrival(WireMessage Message, string Connection, string Identity, long Timestamp, bool Paused);
    private readonly List<Arrival> _arrivals = [];
    private string? _zoneConnection;
    private long? _transitionAt;
    private const int MaximumArrivals = 8192;
    private const long ArrivalWindow = 5_000_000;
    public void SetRecording(bool enabled, long timestamp)
    {
        if (!enabled)
        {
            _arrivals.Clear();
            _combat.SetPaused(true, Bounded(timestamp));
            Checkpoint(Bounded(timestamp), "completed");
            _active = null; _map = null;
        }
        _recording = enabled;
    }
    public void ResetRecording(long timestamp, string recordId)
    {
        _arrivals.Clear();
        _runs.Clear(); _active = null; _map = null; _paused = false; _recording = true; _recordId = recordId;
        _combat.BeginSession(timestamp);
        if (_lastLocation is not { } location || !IsDungeon(location.Map)) return;
        _character = location.Character; _map = location.Map;
        _active = new(Kind(location.Map)!, location.Entrance, location.Entrance, "live", Identity($"{recordId}:{location.Character}:{location.Map}"), DateTimeOffset.FromUnixTimeMilliseconds(timestamp / 1000));
        _runs.Add((location.Character, location.Entrance, _active));
        _active.EnterFloor(location.Map, location.Map, Kind(location.Map)!);
        _combat.BeginSession(timestamp, Identity($"{_active.Id}:{timestamp}"));
    }
    private long _lastFrame;
    private const long CaptureGapMicroseconds = 30_000_000;
    private static bool IsDungeon(string? map) => Kind(map) is not null;
    public static string? Kind(string? map) => map is null ? null
        : map.StartsWith("@RANDOMDUNGEON@", StringComparison.OrdinalIgnoreCase) ? "Random dungeon"
        : map.StartsWith("@EXPEDITION@", StringComparison.OrdinalIgnoreCase) ? "Expedition"
        : null;
    private static string Identity(string value) => Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(value)))[..32];
    private long Bounded(long timestamp) => _lastFrame > 0 && timestamp - _lastFrame > CaptureGapMicroseconds ? _lastFrame : timestamp;

    public void Observe(WireMessage wire, string connection, string identity, long timestamp, LocalCharacterSnapshot? local, PartyRosterSnapshot? party)
    {
        var join = wire.Kind == 3 && wire.ReturnCode == 0 && wire.Parameters.GetValueOrDefault((byte)253) is byte or short or int or long
            && Convert.ToInt64(wire.Parameters[(byte)253]) == 2
            && local is { LocationState: "observed", MapId: not null }
            && wire.Parameters.GetValueOrDefault((byte)8) as string == local.MapId
            && wire.Parameters.GetValueOrDefault((byte)1) is byte[] { Length: 16 } bytes && new Guid(bytes) == local.CharacterId;
        if (join)
        {
            var entranceMap = wire.Parameters.GetValueOrDefault((byte)66) as string;
            if (!IsDungeon(local!.MapId)) _outsideMap = local.MapId;
            if (string.IsNullOrWhiteSpace(entranceMap) || IsDungeon(entranceMap)) entranceMap = _outsideMap ?? "unknown";
            _lastLocation = (local.CharacterId, local.MapId!, entranceMap);
        }
        if (!_recording)
        {
            _latestTimestamp = Math.Max(_latestTimestamp, timestamp);
            _lastFrame = Math.Max(_lastFrame, timestamp);
            _combat.Observe(wire, connection, identity, timestamp, local, party);
            return;
        }
        if (_transitionAt is not null && wire.Kind == 3 && wire.ReturnCode != 0
            && wire.Parameters.GetValueOrDefault((byte)253) is byte or short or int or long
            && Convert.ToInt64(wire.Parameters[(byte)253]) is 41 or 237)
        {
            foreach (var arrival in _arrivals.Where(arrival => arrival.Connection == _zoneConnection)) ReplayArrival(arrival, local, party);
            _arrivals.Clear(); _transitionAt = null;
            _combat.SetPaused(_paused, timestamp);
        }
        if (!join && (connection != (_zoneConnection ?? local?.Connection) || _transitionAt is { } leaving && timestamp >= leaving))
        {
            _combat.ObserveConnection(connection, timestamp);
            _arrivals.RemoveAll(arrival => timestamp - arrival.Timestamp > ArrivalWindow);
            if (_arrivals.Count == MaximumArrivals) _arrivals.RemoveAt(0);
            _arrivals.Add(new(wire, connection, identity, timestamp, _paused));
            return;
        }
        _latestTimestamp = Math.Max(_latestTimestamp, timestamp);
        _lastFrame = Math.Max(_lastFrame, timestamp);
        var arrivals = join ? _arrivals.Where(arrival => arrival.Connection == connection && arrival.Timestamp <= timestamp
            && timestamp - arrival.Timestamp <= ArrivalWindow && (_transitionAt is null || arrival.Timestamp >= _transitionAt)).ToArray() : [];
        if (join) { _arrivals.Clear(); _zoneConnection = connection; _transitionAt = null; }
        var changed = join && (_character != local!.CharacterId || _map != local.MapId);
        if (changed) { _combat.FlushUnidentified(); Checkpoint(arrivals.Length > 0 ? arrivals.Min(arrival => arrival.Timestamp) : timestamp, "completed"); }
        _combat.Observe(wire, connection, identity, timestamp, local, party);
        if (!changed)
        {
            foreach (var arrival in arrivals) ReplayArrival(arrival, local, party);
            if (arrivals.Length > 0) _combat.SetPaused(_paused, timestamp);
            if (wire.Kind == 3 && wire.ReturnCode == 0 && wire.Parameters.GetValueOrDefault((byte)253) is byte or short or int or long
                && Convert.ToInt64(wire.Parameters[(byte)253]) == 41) _transitionAt = timestamp;
            return;
        }
        if (_character != local!.CharacterId) { _outsideMap = null; _active = null; }
        _character = local.CharacterId;
        var previousMap = _map;
        _map = local.MapId;
        if (!IsDungeon(_map))
        {
            _active = null;
            _outsideMap = _map;
            return;
        }
        var entrance = wire.Parameters.GetValueOrDefault((byte)66) as string;
        if (string.IsNullOrWhiteSpace(entrance) || IsDungeon(entrance)) entrance = _outsideMap ?? "unknown";
        var known = _runs.FirstOrDefault(entry => entry.Character == local.CharacterId && entry.Run.ContainsFloor(_map!));
        if (known.Run is not null) _active = known.Run;
        else if (_active is null || !IsDungeon(previousMap) || _runs.First(entry => entry.Run == _active).Entrance != entrance)
        {
            if (_runs.Count >= 100) _runs.RemoveAt(0);
            _active = new(Kind(_map)!, entrance, entrance, "live", Identity(_recordId is null ? $"{local.CharacterId}:{_map}" : $"{_recordId}:{local.CharacterId}:{_map}"), DateTimeOffset.FromUnixTimeMilliseconds(timestamp / 1000));
            _runs.Add((local.CharacterId, entrance, _active));
        }
        _active.EnterFloor(_map!, _map!, Kind(_map)!);
        _combat.BeginSession(arrivals.Length > 0 ? arrivals.Min(arrival => arrival.Timestamp) : timestamp, Identity($"{_active.Id}:{_map}:{timestamp}"));
        foreach (var arrival in arrivals) ReplayArrival(arrival, local, party);
        _combat.SetPaused(_paused, timestamp);
        Checkpoint(timestamp, _paused ? "paused" : "running");
    }

    private void ReplayArrival(Arrival arrival, LocalCharacterSnapshot? local, PartyRosterSnapshot? party)
    {
        _combat.SetPaused(arrival.Paused, arrival.Timestamp);
        if (arrival.Paused && arrival.Message.Kind == 4 && arrival.Message.Parameters.GetValueOrDefault((byte)252) is byte or short or int or long
            && Convert.ToInt64(arrival.Message.Parameters[(byte)252]) is 6 or 7) return;
        _combat.Observe(arrival.Message, arrival.Connection, arrival.Identity, arrival.Timestamp, local, party);
    }

    private void Checkpoint(long timestamp, string status)
    {
        if (_active is null) return;
        _combat.AdvanceTo(timestamp);
        if (_combat.Snapshot is { } snapshot)
            _active.Record(_active.Location.FloorId, snapshot with { Location = _active.Location, Status = status });
    }

    public void ObserveOwnCast(WireMessage wire, long timestamp)
    {
        _combat.ObserveOwnCast(wire, timestamp);
        if (wire.Kind == 2 && wire.Parameters.GetValueOrDefault((byte)253) is byte or short or int or long
            && Convert.ToInt64(wire.Parameters[(byte)253]) == 237) _transitionAt = timestamp;
    }
    public void ForgetIdentities(long timestamp)
    {
        _arrivals.Clear(); _transitionAt = null;
        if (_active is not null) { _combat.FlushUnidentified(); Checkpoint(Bounded(timestamp), "completed"); }
        _active = null; _map = null;
        _combat.ForgetIdentities(timestamp);
    }
    public IReadOnlyList<UnidentifiedPlayer> Unidentified => _combat.Unidentified;
    public IReadOnlyList<IdentitySighting> Sightings => _combat.Sightings;
    public void SeedSkillWeapons(IEnumerable<KeyValuePair<int, Dictionary<int, int>>> skills) => _combat.SeedSkillWeapons(skills);
    public IReadOnlyList<IdentityTimelineEntry> Timeline => _combat.Timeline;
    public void SeedCharacters(IEnumerable<KeyValuePair<Guid, KnownCharacter>> characters) => _combat.SeedCharacters(characters);
    public void FlushUnidentified() { _combat.FlushUnidentified(); Checkpoint(Bounded(_latestTimestamp), _paused ? "paused" : "running"); }
    public void BeginRecord(long timestamp)
    {
        _arrivals.Clear();
        _combat.FlushUnidentified();
        Checkpoint(Bounded(timestamp), "completed");
        _paused = false;
        if (_active is not null) _combat.BeginSession(timestamp, Identity($"{_active.Id}:{_map}:{timestamp}"));
    }

    public void SetPaused(bool paused, long timestamp)
    {
        var at = paused ? Bounded(timestamp) : timestamp;
        _paused = paused;
        _combat.SetPaused(paused, at);
        _latestTimestamp = Math.Max(_latestTimestamp, at);
        Checkpoint(at, paused ? "paused" : "running");
    }

    public void RestoreParty(PartyRosterSnapshot? party) => _combat.SetParty(party);

    public DungeonRunSnapshot[] Snapshot()
    {
        Checkpoint(Bounded(_latestTimestamp), _paused ? "paused" : "running");
        return _runs.Select(entry => entry.Run.Snapshot(entry.Run == _active ? _paused ? "paused" : "running" : "completed")).ToArray();
    }
}
