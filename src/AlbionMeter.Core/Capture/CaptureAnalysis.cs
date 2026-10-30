using AlbionMeter.Core.Observation;

namespace AlbionMeter.Core.Capture;

public sealed record CaptureReport(long Frames, long Bytes, long? FirstTimestampMicroseconds, long? LastTimestampMicroseconds,
    Dictionary<string, long> Counters, Dictionary<string, long> Flows, bool CombatDecoded = false, LocalCharacterSnapshot? Character = null, PartyRosterSnapshot? Party = null, CombatSnapshot? Combat = null, Dictionary<string, long>? CombatDiagnostics = null, Dictionary<Guid, int?>? Weapons = null, long ContextFrames = 0, DungeonRunSnapshot[]? Dungeons = null, bool RecordPaused = false, long? RecordTimestampMicroseconds = null,
    [property: System.Text.Json.Serialization.JsonIgnore] Dictionary<Guid, KnownCharacter>? Characters = null,
    [property: System.Text.Json.Serialization.JsonIgnore] UnidentifiedPlayer[]? Unidentified = null,
    [property: System.Text.Json.Serialization.JsonIgnore] IdentitySighting[]? Sightings = null,
    [property: System.Text.Json.Serialization.JsonIgnore] Dictionary<int, Dictionary<int, int>>? SkillWeapons = null,
    [property: System.Text.Json.Serialization.JsonIgnore] IdentityTimelineEntry[]? Timeline = null, string? RecordState = null);

public sealed class CaptureAnalysis(CombatSourceCatalog? sources = null, bool manualRecording = false)
{
    private readonly LocalCharacterObservation _character = new();
    private readonly PartyRosterObservation _party = new();
    private readonly CapturedCombatObservation _combat = new(sources);
    private readonly DungeonCombatObservation _dungeons = new(sources);
    private long _contextFrames;
    public string RecordState { get; private set; } = manualRecording ? "idle" : "running";
    public void StartRecord(long timestamp)
    {
        if (RecordState is "running" or "paused") throw new InvalidOperationException("Stop the current record before starting another one.");
        _combat.BeginSession(timestamp);
        _dungeons.ResetRecording(timestamp, _combat.Snapshot!.Id);
        RecordState = "running";
    }
    public void StopRecord(long timestamp)
    {
        if (RecordState is not ("running" or "paused")) throw new InvalidOperationException("There is no active record to stop.");
        _combat.SetPaused(true, timestamp);
        _combat.FlushUnidentified();
        _dungeons.SetRecording(false, timestamp);
        RecordState = "completed";
    }
    /// <param name="keepIdentities">The capture came back by itself after a short drop on the same game connection: the
    /// players, their objects and the dungeon floor are still the same, so they are kept instead of learned again.</param>
    /// <summary>Reset: damage and healing start again from zero in the same record, which keeps its id, its players and
    /// its pause state (and so its AlbionLab session); the dungeon runs start again from the floor the party is on.</summary>
    public void ResetCounts(long timestamp)
    {
        if (RecordState is not ("running" or "paused")) throw new InvalidOperationException("There is no active record to reset.");
        var paused = RecordState == "paused";
        _combat.Restart(timestamp);
        _dungeons.ResetCounts(timestamp, _combat.Snapshot!.Id);
        if (paused) { _combat.SetPaused(true, timestamp); _dungeons.SetPaused(true, timestamp); }
    }
    public void BeginCapture(long? timestamp = null, bool preserveRecord = false, bool keepIdentities = false)
    {
        _contextFrames = _frames; _frames = 0; _bytes = 0; _first = null; _last = null; _counters.Clear(); _flows.Clear();
        var since = timestamp ?? DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() * 1000;
        if (!keepIdentities)
        {
            _combat.ForgetIdentities(since);
            _dungeons.ForgetIdentities(since);
            if (_restoredPlayers is { } restored) { _combat.RestorePlayers(restored); _dungeons.RestorePlayers(restored); }
        }
        _restoredPlayers = null;
        if (!preserveRecord)
        {
            var paused = _combat.IsPaused;
            var at = timestamp ?? DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() * 1000;
            _combat.BeginSession(at);
            if (paused) _combat.SetPaused(true, at);
        }
        if (manualRecording && RecordState is "idle" or "completed")
        {
            _combat.SetPaused(true, since);
            _dungeons.SetRecording(false, since);
        }
    }
    public void BeginRecord(long? timestamp = null)
    {
        var at = timestamp ?? DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() * 1000;
        _combat.BeginSession(at);
        _dungeons.BeginRecord(at);
    }
    public void SeedSkillWeapons(IEnumerable<KeyValuePair<int, Dictionary<int, int>>> skills) { var known = skills.ToArray(); _combat.SeedSkillWeapons(known); _dungeons.SeedSkillWeapons(known); }
    public void SeedCharacters(IEnumerable<KeyValuePair<Guid, KnownCharacter>> characters) { var known = characters.ToArray(); _combat.SeedCharacters(known); _dungeons.SeedCharacters(known); }
    public void SealRecord() { _combat.FlushUnidentified(); _dungeons.FlushUnidentified(); }
    public void SetPaused(bool paused, long timestamp)
    {
        if (manualRecording && RecordState is not ("running" or "paused")) throw new InvalidOperationException("Press Play to start a record first.");
        _combat.SetPaused(paused, timestamp);
        _dungeons.SetPaused(paused, timestamp);
        RecordState = paused ? "paused" : "running";
    }
    /// <summary>
    /// Moves the record's clock to the given time without a packet: live, the encounter time runs from Play even while the
    /// game sends nothing (login screen, loading, standing still), and a paused record stays frozen.
    /// </summary>
    public void AdvanceClock(long timestamp) { if (RecordState == "running") _combat.AdvanceTo(timestamp); }
    public void RestoreCharacter(LocalCharacterSnapshot character) => _character.Restore(character);
    public CapturedCombatObservation.KnownPlayer[] KnownPlayers() => _combat.KnownPlayers();
    // Players restored at startup survive the first capture start, which otherwise forgets every identity.
    private IReadOnlyList<CapturedCombatObservation.KnownPlayer>? _restoredPlayers;
    public void RestorePlayers(IReadOnlyList<CapturedCombatObservation.KnownPlayer> players)
    {
        if (players.Count == 0) return;
        _restoredPlayers = players;
        _combat.RestorePlayers(players); _dungeons.RestorePlayers(players);
    }
    public bool RestoreParty(PartyRosterSnapshot value)
    {
        if (!_party.Restore(value)) return false;
        _combat.SetParty(_party.Snapshot);
        _dungeons.RestoreParty(_party.Snapshot);
        return true;
    }
    private readonly NetworkDecoder _network = new();
    private readonly PhotonTransport _photon = new();
    private readonly Dictionary<string, long> _counters = [];
    private readonly Dictionary<string, long> _flows = [];
    private long _frames, _bytes;
    private long? _first, _last;
    public void Add(CaptureFrame frame)
    {
        _frames++; _bytes += frame.Data.Length;
        if (_last is { } last && frame.TimestampMicroseconds < last) Increment("backwardTimestamps");
        _first = Math.Min(_first ?? frame.TimestampMicroseconds, frame.TimestampMicroseconds);
        _last = Math.Max(_last ?? frame.TimestampMicroseconds, frame.TimestampMicroseconds);
        var result = _network.Decode(frame); Increment(result.Status);
        if (result.Datagram is not { } datagram) return;
        var flow = _flows.ContainsKey(datagram.Flow) || _flows.Count < 64 ? datagram.Flow : "other";
        _flows[flow] = _flows.GetValueOrDefault(flow) + 1;
        var transport = _photon.Decode(datagram); Increment(transport.Status);
        foreach (var message in transport.Messages)
        {
            Increment("transportMessages");
            if (message.Payload.Length < 2) { Increment("shortMessages"); continue; }
            Increment($"messageMagic:{message.Payload[0]:X2}");
            if ((message.Payload[1] & 0x80) != 0) Increment("encryptedMessages");
            else Increment($"messageType:{message.Payload[1] & 0x7f}");
            var decoded = Protocol18.Decode(message.Payload);
            Increment(decoded.Status);
            if (decoded.Message is { } wire)
            {
                if (NetworkDecoder.IsGamePort(datagram.SourcePort) && !NetworkDecoder.IsGamePort(datagram.DestinationPort))
                {
                    _character.Observe(wire, message.Connection ?? datagram.Flow, message.TimestampMicroseconds);
                    _party.Observe(wire, _character.Snapshot, message.TimestampMicroseconds);
                    _dungeons.Observe(wire, message.Connection ?? datagram.Flow, message.Identity, message.TimestampMicroseconds, _character.Snapshot, _party.Snapshot);
                    _combat.Observe(wire, message.Connection ?? datagram.Flow, message.Identity, message.TimestampMicroseconds, _character.Snapshot, _party.Snapshot);
                }
                else if (NetworkDecoder.IsGamePort(datagram.DestinationPort) && !NetworkDecoder.IsGamePort(datagram.SourcePort))
                {
                    _combat.ObserveOwnCast(wire, message.TimestampMicroseconds);
                    _dungeons.ObserveOwnCast(wire, message.TimestampMicroseconds);
                }
                Increment($"wireCode:{wire.Kind}:{wire.Code}");
                var key = wire.Kind == 4 ? (byte)252 : (byte)253;
                if (wire.Parameters.TryGetValue(key, out var applicationCode) && applicationCode is byte or short or int or long)
                {
                    var code = Convert.ToInt64(applicationCode);
                    if (code is >= 0 and <= 65535) Increment($"applicationCode:{wire.Kind}:{code}");
                }
            }
        }
    }
    public CaptureReport Snapshot()
    {
        var counters = new Dictionary<string, long>(_counters)
        {
            ["duplicateCommands"] = _photon.Duplicates,
            ["pendingIpFragments"] = _network.PendingFragments,
            ["expiredIpFragments"] = _network.ExpiredFragments,
            ["evictedIpFragments"] = _network.EvictedFragments,
            ["pendingPhotonAssemblies"] = _photon.PendingAssemblies,
            ["expiredPhotonAssemblies"] = _photon.ExpiredAssemblies,
            ["evictedPhotonAssemblies"] = _photon.EvictedAssemblies
        };
        return new(_frames, _bytes, _first, _last, counters, new(_flows), CombatDecoded: _combat.Snapshot is not null, Character: _character.Snapshot, Party: _party.Snapshot, Combat: _combat.Snapshot, CombatDiagnostics: _combat.Diagnostics, Weapons: _combat.Weapons, ContextFrames: _contextFrames, Dungeons: _dungeons.Snapshot(), RecordPaused: _combat.IsPaused, RecordTimestampMicroseconds: _combat.ClockTimestamp,
            Characters: _combat.Characters, Unidentified: [.. _combat.Unidentified, .. _dungeons.Unidentified], Sightings: [.. _combat.Sightings, .. _dungeons.Sightings], SkillWeapons: _combat.SkillWeapons, Timeline: [.. _combat.Timeline], RecordState: manualRecording ? RecordState : null);
    }
    private void Increment(string name)
    {
        if (!_counters.ContainsKey(name) && _counters.Count >= 512) name = "otherCounters";
        _counters[name] = _counters.GetValueOrDefault(name) + 1;
    }
}
