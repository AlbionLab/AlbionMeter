using AlbionMeter.Core.Capture;

namespace AlbionMeter.Core.Observation;

public sealed record KnownCharacter(string Name, int?[]? Equipment, int[]? Skills = null);
public sealed record UnidentifiedPlayer(string MemberId, string? RecordId, long ObjectId, int?[]? Equipment, Guid[] Candidates, long Timestamp, int[]? Skills = null, string? Visit = null);
public sealed record IdentitySighting(Guid CharacterId, long ObjectId, string Visit, long Timestamp, string Method);
public sealed record IdentityTimelineEntry(long Timestamp, string Visit, string Kind, Guid CharacterId, string? Name, long? ObjectId = null, int?[]? Equipment = null);

public sealed class CapturedCombatObservation(CombatSourceCatalog? sources = null)
{
    private sealed record Entity(Guid? Id, string Name, string Kind, int? Weapon, long Revision, int?[]? Equipment = null, int?[]? Spells = null, long WeaponSeen = 0, long GearSeen = 0);
    private readonly Dictionary<(string Connection, long ObjectId), Entity> _entities = [];
    private readonly Dictionary<Guid, Entity> _characters = [];
    private readonly Dictionary<(string Connection, long ObjectId), (int Weapon, long Timestamp, int?[] Equipment, int?[] Spells)> _pendingEquipment = [];
    private readonly Dictionary<string, long> _diagnostics = [];
    private readonly Dictionary<(string Connection, long ItemObject), (int Type, long Timestamp)> _itemTypes = [];
    private readonly Dictionary<(string Connection, long ItemObject), int?[]> _itemSpells = [];
    private readonly Dictionary<(string Connection, long ItemObject), long> _itemDurability = [];
    private readonly HashSet<(string Connection, long ItemObject)> _wornItems = [];
    private readonly Dictionary<(string Connection, long ObjectId), long> _equipmentChanges = [];
    private readonly List<(long At, string Connection, long ObjectId, int?[] Gear)> _equipmentReports = [];
    private readonly List<(long At, string Connection, long Item, Guid Container, int Slot)> _itemMoves = [];
    private readonly List<(long At, Guid From, Guid To)> _moveRequests = [];
    private Guid? _equipmentContainer;
    private readonly HashSet<(string Connection, long ItemObject)> _containerItems = [];
    private long?[] _localItems = [];
    private long _identitiesSince;
    private sealed record Pending(string Id, double Time, double Delta, int? Spell, int? Kind, string TargetKind, long At = 0);
    private readonly Dictionary<(string Connection, long ObjectId), List<Pending>> _unresolved = [];
    private int _unresolvedCount;
    private const int MaximumUnresolved = 100_000;
    private readonly Dictionary<long, long> _ownCasts = [];
    private readonly Dictionary<(string Connection, long ObjectId), (int?[] Gear, int?[] Spells, long At)> _unresolvedGear = [];
    private readonly Dictionary<int, Dictionary<int, int>> _weaponBySkill = [];
    public Dictionary<int, Dictionary<int, int>> SkillWeapons => _weaponBySkill.ToDictionary(pair => pair.Key, pair => new Dictionary<int, int>(pair.Value));
    private readonly Dictionary<Guid, List<int?[]>> _fingerprints = [];
    private readonly Dictionary<(string Connection, long ObjectId), long> _partyObjects = [];
    private readonly Dictionary<Guid, HashSet<int>> _skills = [];
    private string _visit = "v0";
    private readonly List<IdentitySighting> _sightings = [];
    private readonly HashSet<(Guid, long, string)> _sighted = [];
    public IReadOnlyList<IdentitySighting> Sightings => _sightings.ToArray();
    private void NewVisit(long timestamp) => _visit = $"v{timestamp:x}";
    private long? _leavingAt;
    private readonly Dictionary<string, long> _connectionSeen = [];
    internal void ObserveConnection(string connection, long timestamp)
    {
        if (!_connectionSeen.ContainsKey(connection) && _connectionSeen.Count < 256) _connectionSeen[connection] = timestamp;
    }
    private void ZoneVisit(long timestamp, string connection, LocalCharacterSnapshot? local)
    {
        var opened = _connectionSeen.GetValueOrDefault(connection, timestamp);
        var since = _leavingAt is { } leaving && timestamp - leaving is >= 0 and <= 60_000_000
            ? Math.Max(opened, leaving)
            : timestamp - opened is >= 0 and <= 5_000_000 ? opened : timestamp;
        _leavingAt = null;
        if (AwaitingProof) SettleAwaiting(since, final: false);
        var arrived = _entities.Where(pair => pair.Key.Connection == connection && pair.Value.Revision >= since).ToArray();
        var arrivedParty = _partyObjects.Where(pair => pair.Key.Connection == connection && pair.Value >= since).ToArray();
        var arrivedUnknownParty = arrivedParty.Where(pair => _unknownTimePartyObjects.Contains(pair.Key)).Select(pair => pair.Key).ToArray();
        var effectsSince = local?.Connection == connection && local.ObservedAtMicroseconds == timestamp ? since : timestamp;
        var arrivedEffects = _unresolved.Where(pair => pair.Key.Connection == connection)
            .Select(pair => (pair.Key, Effects: pair.Value.Where(effect => effect.At >= effectsSince).ToList()))
            .Where(pair => pair.Effects.Count > 0).ToArray();
        var arrivedGear = _unresolvedGear.Where(pair => pair.Key.Connection == connection && pair.Value.At >= effectsSince).ToArray();
        foreach (var (key, effects) in arrivedEffects)
        {
            _unresolved[key].RemoveAll(effect => effect.At >= effectsSince);
            _unresolvedCount -= effects.Count;
        }
        var previous = _visit;
        FlushUnidentified();
        NewVisit(since);
        _entities.Clear(); _partyObjects.Clear(); _unknownTimePartyObjects.Clear();
        foreach (var (key, at) in arrivedParty) _partyObjects[key] = at;
        _unknownTimePartyObjects.UnionWith(arrivedUnknownParty);
        foreach (var (key, effects) in arrivedEffects) { _unresolved[key] = effects; _unresolvedCount += effects.Count; }
        foreach (var (key, gear) in arrivedGear) _unresolvedGear[key] = gear;
        foreach (var key in _pendingEquipment.Where(pair => pair.Key.Connection != connection || pair.Value.Timestamp < since).Select(pair => pair.Key).ToArray()) _pendingEquipment.Remove(key);
        foreach (var (key, entity) in arrived)
        {
            _entities[key] = entity;
            if (entity.Id is { } character)
            {
                _sightings.RemoveAll(sighting => sighting.Visit == previous && sighting.CharacterId == character && sighting.ObjectId == key.ObjectId && sighting.Timestamp >= since);
                Sight(character, key.ObjectId, entity.Revision, "announced");
            }
        }
        _diagnostics.Remove("unresolvedSinceZoneChange"); _diagnostics.Remove("attributedSinceZoneChange");
    }
    private void Sight(Guid character, long objectId, long timestamp, string method)
    {
        if (_sighted.Count > 20000) _sighted.Clear();
        if (!_sighted.Add((character, objectId, _visit))) return;
        if (_sightings.Count >= 2000) _sightings.RemoveAt(0);
        _sightings.Add(new(character, objectId, _visit, timestamp, method));
    }
    private static int[] Signature(IEnumerable<int> skills) => skills.Distinct().Order().Take(32).ToArray();
    private readonly List<UnidentifiedPlayer> _kept = [];
    public IReadOnlyList<UnidentifiedPlayer> Unidentified => _kept.ToArray();
    public Dictionary<Guid, KnownCharacter> Characters => _characters.ToDictionary(pair => pair.Key, pair => new KnownCharacter(pair.Value.Name, pair.Value.Equipment, _skills.TryGetValue(pair.Key, out var skills) ? Signature(skills) : null));
    public void SeedSkillWeapons(IEnumerable<KeyValuePair<int, Dictionary<int, int>>> skills)
    {
        foreach (var (skill, weapons) in skills)
            if (_weaponBySkill.Count < 8192 && !_weaponBySkill.ContainsKey(skill)) _weaponBySkill[skill] = new(weapons);
    }
    private bool _rosterConfirmed;
    private readonly List<(long At, HashSet<Guid> Members, long? PartyId)> _rosterHistory = [];
    private long _partyActionAt = long.MinValue;
    private bool _firstListJoined;
    private long? _leftParty;
    private long _localLeftAt;
    private bool AwaitingProof => _awaitingRoster.Count > 0 && (!_rosterConfirmed || _leftParty is not null && !_leftPartySettled);
    private bool _leftPartySettled;
    private readonly HashSet<Guid> _joinsSeen = [];
    private readonly Dictionary<Guid, long> _memberEvidence = [];
    private readonly Dictionary<Guid, long> _leftAt = [];
    private readonly HashSet<(string Connection, long ObjectId)> _unknownTimePartyObjects = [];
    private readonly List<(Entity Actor, Pending Effect, (string Connection, long ObjectId)? Key, string Visit)> _awaitingRoster = [];
    private long _lastProof;
    private readonly List<IdentityTimelineEntry> _timeline = [];
    private readonly Dictionary<Guid, int?[]> _loggedGear = [];
    public IReadOnlyList<IdentityTimelineEntry> Timeline => _timeline.ToArray();
    private void Log(IdentityTimelineEntry entry)
    {
        if (_timeline.Count >= 4000) _timeline.RemoveAt(0);
        _timeline.Add(entry);
    }
    private static readonly int[] SkillSlots = [0, 0, 0, 3, 2, 4, -1, -1, -1, -1, -1, -1, 8, 9];
    private void LearnWeaponSkills(int?[] gear, int?[] spells)
    {
        for (var index = 0; index < Math.Min(spells.Length, SkillSlots.Length); index++)
        {
            var slot = SkillSlots[index];
            if (slot < 0 || slot >= gear.Length || gear[slot] is not > 0 || spells[index] is not > 0) continue;
            var (skill, item) = (spells[index]!.Value, gear[slot]!.Value);
            if (_weaponBySkill.Count >= 8192 && !_weaponBySkill.ContainsKey(skill)) continue;
            var items = _weaponBySkill.TryGetValue(skill, out var known) ? known : _weaponBySkill[skill] = [];
            if (items.Count < 32 || items.ContainsKey(item)) items[item] = items.GetValueOrDefault(item) + 1;
        }
    }
    private int? OwnedItemCasting(int effect)
    {
        if (sources is null) return null;
        var types = _itemSpells.Where(pair => (_wornItems.Contains(pair.Key) || _localItems.Contains(pair.Key.ItemObject)) && _itemTypes.ContainsKey(pair.Key)
                && pair.Value.Any(skill => skill is > 0 && sources.Produces(skill.Value, effect)))
            .Select(pair => _itemTypes[pair.Key].Type).Distinct().Take(2).ToArray();
        return types is [var only] ? only : null;
    }
    private ItemAttribution ResolveLearned(int effect, int?[]? worn)
    {
        Count(worn is null ? "unknownSource:noEquipment" : "unknownSource:unproven");
        return new(null, "unknown");
    }
    public void SeedCharacters(IEnumerable<KeyValuePair<Guid, KnownCharacter>> characters)
    {
        foreach (var (id, character) in characters)
        {
            if (character.Equipment is { Length: >= 7 } gear && !_fingerprints.ContainsKey(id) && _fingerprints.Count < 4096) _fingerprints[id] = [gear];
            if (character.Skills is { Length: > 0 } skills && !_skills.ContainsKey(id) && _skills.Count < 4096) _skills[id] = [.. skills];
        }
        foreach (var (id, character) in characters)
            if (!_characters.ContainsKey(id) && _characters.Count < 4096)
                _characters[id] = new(id, character.Name, "player", character.Equipment?.FirstOrDefault(), 0, character.Equipment);
    }
    private CombatEngine? _engine;
    private long _start;
    private long? _pausedAt;
    private long _pausedDuration;
    public bool IsPaused => _pausedAt is not null;
    public long? ClockTimestamp => _engine is null ? null : _pausedAt ?? _start + _pausedDuration + (long)(_engine.ElapsedSeconds * 1_000_000);
    private Guid? _localId;
    private string? _connection;
    private PartyRosterSnapshot? _party;
    private readonly Dictionary<Guid, string> _roster = [];
    public Dictionary<string, long> Diagnostics => new(_diagnostics) { ["pendingUnidentified"] = _unresolvedCount };
    public Dictionary<Guid, int?> Weapons => _characters.ToDictionary(pair => pair.Key, pair => FreshWeapon(pair.Value));
    public CombatSnapshot? Snapshot => _engine is null ? null : _engine.Snapshot(true) with { Mode = "live", Status = IsPaused ? "paused" : "running", StartedAt = DateTimeOffset.FromUnixTimeMilliseconds(_start / 1000) };
    public void ForgetIdentities(long timestamp)
    {
        FlushUnidentified();
        NewVisit(timestamp);
        _entities.Clear(); _partyObjects.Clear(); _unknownTimePartyObjects.Clear(); _pendingEquipment.Clear(); _itemTypes.Clear(); _itemSpells.Clear(); _itemDurability.Clear(); _wornItems.Clear(); _equipmentChanges.Clear(); _equipmentReports.Clear(); _itemMoves.Clear(); _moveRequests.Clear(); _containerItems.Clear(); _inspections.Clear(); _localItems = []; _unsourced.Clear(); _settled.Clear(); _gearHistory.Clear(); _loggedGear.Clear(); _rosterConfirmed = false; _rosterHistory.Clear(); _partyActionAt = long.MinValue; _firstListJoined = false; _leftParty = null; _leftPartySettled = false; _awaitingRoster.Clear(); _joinsSeen.Clear(); _memberEvidence.Clear(); _leftAt.Clear(); _roster.Clear(); _party = null;
        _identitiesSince = timestamp;
        _diagnostics.Remove("unresolvedSinceZoneChange"); _diagnostics.Remove("attributedSinceZoneChange");
    }
    public void BeginSession(long? timestamp = null, string? recordId = null) { if (_engine is not null) FlushUnidentified(); ClearUnidentified(); _awaitingRoster.Clear(); _unsourced.Clear(); _settled.Clear(); _engine = timestamp is null ? null : new([], id: recordId); _start = timestamp ?? 0; _pausedAt = null; _pausedDuration = 0; _diagnostics.Clear(); Synchronize(); }
    public void AdvanceTo(long timestamp)
    {
        if (_engine is not null) _engine.Advance(Math.Clamp(((IsPaused ? Math.Min(timestamp, _pausedAt!.Value) : timestamp) - _start - _pausedDuration) / 1_000_000d, _engine.ElapsedSeconds, CombatEngine.MaximumSeconds));
    }
    public void SetPaused(bool paused, long timestamp)
    {
        if (paused == IsPaused) return;
        if (paused)
        {
            AdvanceTo(timestamp);
            _pausedAt = Math.Max(timestamp, _start + _pausedDuration + (long)((_engine?.ElapsedSeconds ?? 0) * 1_000_000));
        }
        else
        {
            _pausedDuration += Math.Max(0, timestamp - _pausedAt!.Value);
            _pausedAt = null;
        }
    }
    private void Count(string name, long amount = 1) { if (amount > 0) _diagnostics[name] = _diagnostics.GetValueOrDefault(name) + amount; }
    private static long? Integer(object? value) => value switch { byte b => b, short s => s, int i => i, long l => l, _ => null };
    private static double? Number(object? value) => value switch { float f when float.IsFinite(f) => f, double d when double.IsFinite(d) => d, byte or short or int or long => Convert.ToDouble(value), _ => null };
    private static int? Weapon(object? value) => value is Array { Length: > 0 } array && Integer(array.GetValue(0)) is >= 0 and <= int.MaxValue ? (int)Integer(array.GetValue(0))!.Value : null;
    private static int?[] Numbers(object? value) => value is Array array ? array.Cast<object?>().Take(32).Select(item => Integer(item) is >= int.MinValue and <= int.MaxValue ? (int?)Integer(item) : null).ToArray() : [];
    public void Observe(WireMessage wire, string connection, string identity, long timestamp, LocalCharacterSnapshot? local, PartyRosterSnapshot? party)
    {
        object? Field(byte key) => wire.Parameters.GetValueOrDefault(key);
        AdvanceTo(timestamp);
        ObserveConnection(connection, timestamp);
        if (wire.Kind == 3 && wire.ReturnCode == 0 && Integer(Field(253)) == 2) ZoneVisit(timestamp, connection, local);
        if (wire.Kind == 3 && wire.ReturnCode == 0 && Integer(Field(253)) == 2 && Integer(Field(0)) is { } joinedObject
            && _equipmentChanges.TryGetValue((connection, joinedObject), out var announced) && announced >= timestamp - 2_000_000)
        {
            _equipmentChanges.Remove((connection, joinedObject));
            _equipmentReports.RemoveAll(report => report.Connection == connection && report.ObjectId == joinedObject && report.At >= timestamp - 2_000_000);
        }
        if (wire.Kind == 3 && wire.ReturnCode == 0 && Integer(Field(253)) == 2 && Field(51) is byte[] { Length: 16 } equipmentContainer) _equipmentContainer = new Guid(equipmentContainer);
        if (wire.Kind == 4 && Integer(Field(252)) is 234 or 235 && Field(1) is byte[] { Length: 16 } leaver && new Guid(leaver) == _localId
            && _leftParty is null && _rosterHistory.All(entry => entry.Members.All(member => member == _localId)) && Integer(Field(0)) is { } leftParty)
        { _leftParty = leftParty; _localLeftAt = timestamp; }
        if (_unresolved.Count > 0 && timestamp - _lastProof > 5_000_000) { _lastProof = timestamp; PossibleIdentities(); }
        if (local is not null && _localId != local.CharacterId)
        {
            if (_localId is not null) { _entities.Clear(); _characters.Clear(); _roster.Clear(); _engine = null; }
            _localId = local.CharacterId;
        }
        if (local is not null && local.Connection != _connection)
        {
            _connection = local.Connection;
            foreach (var key in _entities.Keys.Where(key => key.Connection != local.Connection).ToArray()) _entities.Remove(key);
        }
        foreach (var key in _pendingEquipment.Where(pair => timestamp - pair.Value.Timestamp > 15_000_000).Select(pair => pair.Key).ToArray()) _pendingEquipment.Remove(key);
        if (local is { ObjectId: { } objectId } && local.LocationState == "observed" && local.ObservedAtMicroseconds >= _identitiesSince)
        {
            var entity = new Entity(local.CharacterId, local.Name, "player", _characters.GetValueOrDefault(local.CharacterId)?.Weapon, timestamp, _characters.GetValueOrDefault(local.CharacterId)?.Equipment, _characters.GetValueOrDefault(local.CharacterId)?.Spells, _characters.GetValueOrDefault(local.CharacterId)?.WeaponSeen ?? 0, _characters.GetValueOrDefault(local.CharacterId)?.GearSeen ?? 0);
            if (_pendingEquipment.Remove((local.Connection, objectId), out var pending) && timestamp >= pending.Timestamp)
                entity = entity with { Weapon = pending.Weapon, Equipment = pending.Equipment, Spells = pending.Spells, WeaponSeen = pending.Timestamp, GearSeen = pending.Timestamp };
            var mapped = _entities.ContainsKey((local.Connection, objectId)) && _characters.ContainsKey(local.CharacterId);
            Sight(local.CharacterId, objectId, timestamp, "join");
            if (_unresolved.ContainsKey((local.Connection, objectId))) Identify((local.Connection, objectId), entity);
            else
            {
                _entities[(local.Connection, objectId)] = entity; _characters[local.CharacterId] = entity;
                if (!mapped) Synchronize();
                Settle(local.CharacterId, entity);
            }
        }
        SetParty(party);
        var code = Integer(Field(252));
        if (wire.Kind == 3 && wire.ReturnCode != 0 && Integer(Field(253)) is 41 or 237) _leavingAt = null;
        if (wire.Kind == 3 && wire.ReturnCode == 0 && Integer(Field(253)) == 143 && Field(1) is Array contents)
        {
            foreach (var item in contents.Cast<object?>().Select(Integer).OfType<long>().Where(item => item > 0).Take(512))
                if (_containerItems.Count < 8192) { _containerItems.Add((connection, item)); _wornItems.Remove((connection, item)); }
            if (Field(0) is byte[] { Length: 16 } inspected && contents.Length == 10 && _inspections.Count < 256)
            {
                _inspections[(connection, new Guid(inspected))] = (contents.Cast<object?>().Select(item => Integer(item) ?? 0).ToArray(), timestamp);
                CompleteInspections(connection);
            }
        }
        if (wire.Kind == 3 && wire.ReturnCode == 0 && Integer(Field(253)) == 2 && Field(52) is { } slots)
        {
            foreach (var stale in _itemTypes.Where(pair => pair.Value.Timestamp < timestamp - 5_000_000).Select(pair => pair.Key).ToArray()) { _itemTypes.Remove(stale); _itemSpells.Remove(stale); _itemDurability.Remove(stale); _wornItems.Remove(stale); _containerItems.Remove(stale); }
            _localItems = slots switch { byte[] bytes => bytes.Select(item => (long?)item).ToArray(), Array array => array.Cast<object?>().Take(16).Select(Integer).ToArray(), _ => [] };
            ApplyLocalEquipment(connection, timestamp, local);
        }
        if (wire.Kind == 4 && code == 30 && Integer(Field(0)) is { } itemObject && Integer(Field(1)) is >= 0 and <= int.MaxValue)
        {
            if (_itemTypes.Count < 4096 || _itemTypes.ContainsKey((connection, itemObject))) _itemTypes[(connection, itemObject)] = ((int)Integer(Field(1))!.Value, timestamp);
            if (Field(9) is Array itemSpells && _itemTypes.ContainsKey((connection, itemObject))) _itemSpells[(connection, itemObject)] = Numbers(itemSpells);
            if (_inspections.Count > 0) CompleteInspections(connection);
            if (Integer(Field(8)) is { } durability && _itemTypes.ContainsKey((connection, itemObject)))
            {
                if (_itemDurability.TryGetValue((connection, itemObject), out var before) && durability < before && !_containerItems.Contains((connection, itemObject))
                    && _wornItems.Add((connection, itemObject)) && _localId is { } owner && _characters.TryGetValue(owner, out var wearer)) Settle(owner, wearer);
                _itemDurability[(connection, itemObject)] = durability;
            }
            if (_localItems.Contains(itemObject)) ApplyLocalEquipment(connection, timestamp, local);
            return;
        }
        if (wire.Kind == 3 && Integer(Field(253)) == 41) { if (wire.ReturnCode == 0) _leavingAt = timestamp; return; }
        if (wire.Kind != 4) return;
        if (code == 26 && Integer(Field(0)) is { } movedItem && Field(2) is byte[] { Length: 16 } destination)
        {
            if (_itemMoves.Count < 2048) _itemMoves.Add((timestamp, connection, movedItem, new Guid(destination), (int)(Integer(Field(1)) ?? 0)));
            if (_equipmentContainer is { } worn && _localItems.Length > 0)
            {
                var slot = (int)(Integer(Field(1)) ?? 0);
                _localItems = _localItems.Select(item => item == movedItem ? 0 : item).ToArray();
                if (new Guid(destination) == worn && slot is >= 0 and < 16) { if (slot >= _localItems.Length) Array.Resize(ref _localItems, slot + 1); _localItems[slot] = movedItem; }
            }
            if (_localId is { } mover && _characters.TryGetValue(mover, out var moverEntity)) Settle(mover, moverEntity);
            return;
        }
        if (code is 29 or 123 && Integer(Field(0)) is { } observedId)
        {
            var id = code == 29 && Field(7) is byte[] { Length: 16 } bytes ? new Guid(bytes) : (Guid?)null;
            var entity = new Entity(id, code == 29 ? Field(1) as string ?? "Unknown player" : $"Mob {Integer(Field(1))}", code == 29 ? "player" : "mob", code == 29 ? Weapon(Field(40)) : null, timestamp, Numbers(Field(40)), Numbers(Field(43)), timestamp, code == 29 ? timestamp : 0);
            if (_entities.TryGetValue((connection, observedId), out var old) && old.Revision > timestamp) return;
            if (id is not null) { Identify((connection, observedId), entity, "announced"); return; }
            if (_entities.Count < 4096 || _entities.ContainsKey((connection, observedId))) _entities[(connection, observedId)] = entity;
            if (_unresolved.Remove((connection, observedId), out var discarded)) _unresolvedCount -= discarded.Count;
            Synchronize();
            return;
        }
        if (code == 244 && Field(0) is byte[] { Length: 16 } memberBytes && Field(2) is Array memberGear)
        {
            var member = new Guid(memberBytes);
            if (_memberEvidence.Count < 4096 && !_rosterConfirmed) _memberEvidence.TryAdd(member, timestamp);
            ReportedGear(connection, member, Numbers(memberGear), timestamp);
            if (AwaitingProof) SettleAwaiting(timestamp, final: false);
            return;
        }
        if (code == 18 && Integer(Field(0)) is { } caster && Integer(Field(5)) is { } echoed) ObserveCastEcho(connection, caster, echoed, timestamp, local);
        if (code == 232 && Field(1) is byte[] { Length: 16 } joined) _joinsSeen.Add(new Guid(joined));
        if (code is 234 or 235 && Field(1) is byte[] { Length: 16 } removed)
        {
            if (!_rosterConfirmed) _leftAt[new Guid(removed)] = timestamp;
        }
        if (code is 228 or 239) _partyActionAt = timestamp;
        if (code == 243 && Field(1) is byte[] { Length: 16 } stated && _memberEvidence.Count < 4096 && !_rosterConfirmed)
        {
            _memberEvidence.TryAdd(new Guid(stated), timestamp);
            if (AwaitingProof) SettleAwaiting(timestamp, final: false);
        }
        if (code == 670 && Integer(Field(0)) is { } partyObject)
        {
            if (_localId is null && Field(1) is byte[] { Length: 16 } owner) _localId = new Guid(owner);
            var partyKey = (connection, partyObject);
            if (_partyObjects.Count < 1024 || _partyObjects.ContainsKey(partyKey))
            {
                var added = !_partyObjects.ContainsKey(partyKey);
                _partyObjects[partyKey] = timestamp;
                if (added && _unresolvedGear.ContainsKey(partyKey)) Rematch(connection, timestamp);
            }
            if (!_rosterConfirmed && _unknownTimePartyObjects.Count < 1024) _unknownTimePartyObjects.Add((connection, partyObject));
            if (AwaitingProof) SettleAwaiting(timestamp, final: false);
            return;
        }
        if (code == 44 && Field(15) is not null && Integer(Field(3)) is { } looter && Field(6) is byte[] { Length: 16 } looterBytes) { NameByKill((connection, looter), new Guid(looterBytes), timestamp); return; }
        if (code == 165 && Integer(Field(4)) is { } killer && Field(5) is string killerName)
        {
            var named = _roster.Where(pair => pair.Value == killerName).Select(pair => pair.Key).Concat(_characters.Where(pair => pair.Value.Name == killerName).Select(pair => pair.Key)).Distinct().ToArray();
            if (named is [var only]) NameByKill((connection, killer), only, timestamp);
            return;
        }
        if (code == 74 && Integer(Field(4)) is { } speaker && Field(5) is byte[] { Length: 16 } speakerBytes)
        {
            var guid = new Guid(speakerBytes);
            if (_entities.TryGetValue((connection, speaker), out var current) && current.Id == guid) return;
            var previous = _characters.GetValueOrDefault(guid);
            Identify((connection, speaker), new(guid, _roster.GetValueOrDefault(guid) ?? previous?.Name ?? Field(0) as string ?? "Unknown player", "player", previous?.Weapon, timestamp, previous?.Equipment, previous?.Spells, previous?.WeaponSeen ?? 0), "chat");
            Count("identifiedByChat");
            return;
        }
        if (code == 90 && Integer(Field(0)) is { } equipmentObject && Weapon(Field(2)) is { } weapon)
        {
            if (_equipmentChanges.Count < 4096 || _equipmentChanges.ContainsKey((connection, equipmentObject))) _equipmentChanges[(connection, equipmentObject)] = timestamp;
            if (_equipmentReports.Count < 1024) _equipmentReports.Add((timestamp, connection, equipmentObject, Numbers(Field(2))));
            if (_equipmentContainer is null && _entities.TryGetValue((connection, equipmentObject), out var changer) && changer.Id == _localId) _localItems = [];
            LearnWeaponSkills(Numbers(Field(2)), Numbers(Field(7)));
            if (_entities.TryGetValue((connection, equipmentObject), out var entity) && entity.Id is { } id && timestamp >= entity.Revision)
            {
                entity = entity with { Weapon = weapon, Revision = timestamp, Equipment = Numbers(Field(2)), Spells = Field(7) is null ? entity.Spells : Numbers(Field(7)), WeaponSeen = timestamp, GearSeen = timestamp };
                GearChanged(id, timestamp);
                _entities[(connection, equipmentObject)] = entity; _characters[id] = entity; Synchronize();
                Settle(id, entity);
            }
            else if (Fingerprint((connection, equipmentObject), Numbers(Field(2))) is { } match)
            {
                Identify((connection, equipmentObject), match with { Weapon = weapon, Revision = timestamp, Equipment = Numbers(Field(2)), Spells = Field(7) is null ? match.Spells : Numbers(Field(7)), WeaponSeen = timestamp, GearSeen = timestamp }, "equipmentProof");
                Count("identifiedByEquipment");
            }
            else
            {
                if (_pendingEquipment.Count < 256) _pendingEquipment[(connection, equipmentObject)] = (weapon, timestamp, Numbers(Field(2)), Numbers(Field(7)));
                if (_unresolvedGear.Count < 1024 || _unresolvedGear.ContainsKey((connection, equipmentObject))) _unresolvedGear[(connection, equipmentObject)] = (Numbers(Field(2)), Numbers(Field(7)), timestamp);
                Count("unresolvedEquipment");
            }
            return;
        }
        if (code is not (6 or 7)) return;
        if (IsPaused) { Count("pausedHealthUpdates"); return; }
        var count = code == 7 && Field(2) is Array values ? values.Length : code == 6 ? 1 : 0;
        for (var index = 0; index < count; index++)
        {
            object? Part(byte key) => code == 6 ? Field(key) : Field(key) is Array array && index < array.Length ? array.GetValue(index) : null;
            if (Number(Part(2)) is not { } delta || delta == 0 || Integer(Part(6)) is not { } source) { Count("incompleteHealthUpdate"); continue; }
            var target = Integer(Field(0));
            if (_engine is null) { _engine = new([]); _start = timestamp; Synchronize(); }
            var time = Math.Max(0, (timestamp - _start - _pausedDuration) / 1_000_000d);
            if (time > CombatEngine.MaximumSeconds) { Count("sessionLimit"); continue; }
            var targetKind = target is { } targetObject && _entities.TryGetValue((connection, targetObject), out var targetEntity) ? targetEntity.Kind : "unknown";
            var effect = new Pending($"{identity}:{index}", time, delta, (int?)Integer(Part(7)), (int?)Integer(Part(5)), targetKind, timestamp);
            if (!_entities.TryGetValue((connection, source), out var actor) || actor.Id is null)
            {
                Count("unresolvedActor"); Count("unresolvedSinceZoneChange");
                if (_unresolvedCount < MaximumUnresolved) { (_unresolved.TryGetValue((connection, source), out var list) ? list : _unresolved[(connection, source)] = []).Add(effect); _unresolvedCount++; }
                else Count("unresolvedDropped");
                continue;
            }
            Attribute(actor, effect, (connection, source));
        }
    }
    private int?[]? FreshGear(Entity actor)
    {
        if (actor.GearSeen >= _identitiesSince) return actor.Equipment;
        var gear = WornGear(actor);
        if (actor.WeaponSeen >= _identitiesSince && actor.Weapon is > 0) { gear ??= new int?[10]; gear[0] = actor.Weapon; }
        return gear;
    }
    private int? FreshWeapon(Entity actor) => actor.WeaponSeen >= _identitiesSince ? actor.Weapon : WornGear(actor)?[0];
    private (long Before, int?[] Gear)? StartGear()
    {
        if (sources is null || _localId is null || _equipmentContainer is not { } equipment) return null;
        var own = _entities.Where(pair => pair.Value.Id == _localId).Select(pair => pair.Key).ToHashSet();
        var changes = _equipmentReports.Where(report => report.At >= _identitiesSince && own.Contains((report.Connection, report.ObjectId))).OrderBy(report => report.At).ToArray();
        if (changes.Length == 0) return null;
        var first = changes[0];
        static bool Together(long one, long other) => Math.Abs(one - other) <= 100_000;
        var moves = _itemMoves.Where(move => move.Connection == first.Connection && Together(move.At, first.At)).ToArray();
        if (moves.Length == 0) return null;
        var gear = new int?[10];
        var entered = moves.Where(move => move.Container == equipment).Select(move => move.Slot).ToHashSet();
        for (var slot = 0; slot < 8; slot++) if (first.Gear.ElementAtOrDefault(slot) is > 0 and var worn && !entered.Contains(slot)) gear[slot] = worn;
        foreach (var move in moves.Where(move => move.Container != equipment))
        {
            if (!_moveRequests.Any(request => request.From == equipment && request.To == move.Container && request.At <= move.At && move.At - request.At <= 3_000_000)) continue;
            var type = _itemTypes.TryGetValue((move.Connection, move.Item), out var known) ? known.Type : (int?)null;
            type ??= _itemMoves.Where(back => back.Connection == move.Connection && back.Item == move.Item && back.Container == equipment && back.At > move.At)
                .Select(back => changes.FirstOrDefault(report => Together(report.At, back.At)).Gear?.ElementAtOrDefault(back.Slot)).FirstOrDefault(found => found is > 0);
            if (type is not > 0 || SlotIndex(sources.FindItem(type.Value)?.Slot) is not (>= 0 and var slot) || first.Gear.ElementAtOrDefault(slot) is > 0) continue;
            gear[slot] = gear[slot] is { } other && other != type ? null : type;
        }
        return gear.Any(type => type is > 0) ? (first.At, gear) : null;
    }
    private static int SlotIndex(string? slot) => slot switch { "mainhand" => 0, "offhand" => 1, "head" => 2, "armor" => 3, "shoes" => 4, "bag" => 5, "cape" => 6, "mount" => 7, "potion" => 8, "food" => 9, _ => -1 };
    private int?[]? WornGear(Entity actor)
    {
        if (sources is null || actor.Id is null || actor.Id != _localId || _wornItems.Count == 0) return null;
        if (_entities.Any(pair => pair.Value.Id == _localId && _equipmentChanges.TryGetValue(pair.Key, out var changed) && changed >= _identitiesSince)) return null;
        var gear = new int?[10];
        var conflicts = new HashSet<int>();
        foreach (var item in _wornItems)
        {
            if (!_itemTypes.TryGetValue(item, out var type) || SlotIndex(sources.FindItem(type.Type)?.Slot) is not (>= 0 and var slot)) continue;
            if (gear[slot] is { } other && other != type.Type) conflicts.Add(slot);
            gear[slot] = type.Type;
        }
        foreach (var slot in conflicts) gear[slot] = null;
        return gear.Any(type => type is not null) ? gear : null;
    }
    private ItemAttribution SourceOf(Entity actor, Pending effect) => SourceOf(actor.Id, FreshGear(actor), actor.GearSeen >= _identitiesSince ? actor.Spells : null, effect);
    private ItemAttribution SourceOf(Guid? actorId, int?[]? gear, int?[]? spells, Pending effect)
    {
        var source = sources?.Resolve(effect.Spell, effect.Kind, effect.Delta < 0, gear ?? [], spells ?? []) ?? new(null, "unknown");
        if (source.Index is not null || effect.Spell is not > 0) return source;
        if (actorId is not null && actorId == _localId && OwnedItemCasting(effect.Spell.Value) is { } owned && sources?.FindItem(owned) is { } ownedItem) { Count("sourceFromOwnedItem"); return new(owned, ownedItem.Slot, "ability"); }
        if (sources is not null && gear is not null && actorId is { } character && !sources.CouldProduce(effect.Spell.Value, gear) && _gearHistory.TryGetValue(character, out var history))
        {
            var earlier = history.Where(previous => !previous.SequenceEqual(gear)).Select(previous => sources.Resolve(effect.Spell, effect.Kind, effect.Delta < 0, previous, []))
                .Where(found => found.Index is not null).DistinctBy(found => found.Index).Take(2).ToArray();
            if (earlier is [var only]) { Count("sourceFromEarlierEquipment"); return only; }
        }
        if (sources?.UniqueFamily(effect.Spell.Value) is { } family)
        {
            var slotIndex = SlotIndex(family.Slot);
            if (slotIndex >= 0 && gear?.ElementAtOrDefault(slotIndex) is > 0 and var worn && sources.SameFamily(worn, family.Index!.Value)) { Count("sourceFromItemFamily"); return family with { Index = worn, Exact = true }; }
            Count("sourceFromItemFamilyTierUnknown");
            return family;
        }
        return ResolveLearned(effect.Spell.Value, gear);
    }
    private readonly Dictionary<Guid, List<(CombatEvent Counted, Pending Effect)>> _unsourced = [];
    private readonly Dictionary<Guid, (long, long, int?, int?[]?, int)> _settled = [];
    private readonly Dictionary<Guid, List<int?[]>> _gearHistory = [];
    private void LogGear(Guid actorId, Entity actor, long? objectId)
    {
        if (actor.GearSeen < _identitiesSince || actor.Equipment is not { } gear || _loggedGear.TryGetValue(actorId, out var logged) && logged.SequenceEqual(gear)) return;
        objectId ??= _entities.FirstOrDefault(pair => pair.Value.Id == actorId).Key.ObjectId;
        _loggedGear[actorId] = gear; Log(new(actor.GearSeen, _visit, "gear", actorId, actor.Name, objectId is > 0 ? objectId : null, gear));
        var history = _gearHistory.TryGetValue(actorId, out var known) ? known : _gearHistory[actorId] = [];
        if (history.Count >= 16) history.RemoveAt(0);
        history.Add(gear);
    }
    private void Settle(Guid actorId, Entity actor)
    {
        LogGear(actorId, actor, null);
        if (!_unsourced.TryGetValue(actorId, out var pending) || pending.Count == 0 || _engine is null) return;
        var state = (actor.GearSeen, actor.WeaponSeen, actor.Weapon, actor.Spells, actorId == _localId ? (_wornItems.Count * 4096 + _itemSpells.Count) * 4096 + _itemMoves.Count + _equipmentReports.Count * 2048 : 0);
        if (_settled.TryGetValue(actorId, out var settled) && settled == state) return;
        _settled[actorId] = state;
        var changedAt = _entities.Where(pair => pair.Value.Id == actorId).Select(pair => _equipmentChanges.GetValueOrDefault(pair.Key)).DefaultIfEmpty(0).Max();
        var start = actorId == _localId ? StartGear() : null;
        for (var index = pending.Count - 1; index >= 0; index--)
        {
            var (counted, effect) = pending[index];
            var gear = effect.At >= changedAt ? FreshGear(actor) : start is { Before: var until, Gear: var before } && effect.At < until ? before : null;
            if (effect.At < changedAt && gear is null) continue;
            var source = effect.At >= changedAt ? SourceOf(actor, effect) : SourceOf(actorId, gear, null, effect);
            var better = source.Index is not null && (counted.SourceItemIndex is null || !counted.SourceExact && source.Exact);
            var proven = counted with { WeaponIndex = counted.WeaponIndex ?? (effect.At >= changedAt ? FreshWeapon(actor) : gear?[0] is > 0 and var held ? held : null) };
            if (better) proven = proven with { SourceItemIndex = source.Index, SourceSlot = source.Slot, SourceKind = source.Kind, SourceExact = source.Exact };
            if (proven != counted && _engine.Reattribute(counted, proven)) { Count("reattributedFromTimeline"); counted = proven; pending[index] = (proven, effect); }
            if (counted.SourceItemIndex is not null && counted.SourceExact && counted.WeaponIndex is not null) pending.RemoveAt(index);
        }
    }
    private void SettleAwaiting(long until, bool final)
    {
        if (_awaitingRoster.Count == 0 || _engine is null) return;
        var judged = _awaitingRoster.Where(entry => Moment(entry.Effect) <= until).ToArray();
        var credited = 0;
        foreach (var entry in judged)
        {
            var latest = _characters.GetValueOrDefault(entry.Actor.Id!.Value) is { } known && known.GearSeen >= entry.Actor.GearSeen ? entry.Actor with { Equipment = known.Equipment, Spells = known.Spells, Weapon = known.Weapon, WeaponSeen = known.WeaponSeen, GearSeen = known.GearSeen } : entry.Actor;
            if (Credit(latest, entry.Effect, entry.Visit == _visit ? entry.Key : null)) { credited++; _awaitingRoster.Remove(entry); }
            else if (final) { Count("outsideParty"); _awaitingRoster.Remove(entry); }
        }
        Count("creditedAfterRoster", credited);
        Synchronize();
    }
    private HashSet<Guid>? RosterAt(long timestamp)
    {
        for (var index = _rosterHistory.Count - 1; index >= 0; index--) if (_rosterHistory[index].At <= timestamp) return _rosterHistory[index].Members;
        return null;
    }
    private HashSet<Guid>? FirstList => _rosterHistory.Count > 0 && !_firstListJoined && _rosterHistory[0].Members.Any(member => member != _localId) ? _rosterHistory[0].Members : null;
    private HashSet<Guid>? LeftPartyList => _leftParty is { } left ? _rosterHistory.FirstOrDefault(entry => entry.PartyId == left).Members?.Where(member => !_joinsSeen.Contains(member)).ToHashSet() : null;
    private bool ProvenMember(Guid member, (string Connection, long ObjectId)? key, long timestamp)
    {
        if (member == _localId) return true;
        if (RosterAt(timestamp) is { } roster) return roster.Contains(member);
        if (_joinsSeen.Contains(member) || _leftAt.TryGetValue(member, out var left) && left <= timestamp) return false;
        return key is { } listed && _unknownTimePartyObjects.Contains(listed) || _memberEvidence.ContainsKey(member) || _leftAt.ContainsKey(member) || FirstList?.Contains(member) == true || LeftPartyList?.Contains(member) == true;
    }
    private static long Moment(Pending effect) => effect.At;
    private bool Credit(Entity actor, Pending effect, (string Connection, long ObjectId)? key)
    {
        if (_engine is null || !ProvenMember(actor.Id!.Value, key, Moment(effect))) return false;
        var id = actor.Id!.Value.ToString();
        if (!_engine.Knows(id))
            _engine.RegisterMember(new(id, _roster.GetValueOrDefault(actor.Id.Value) ?? actor.Name, "Weapon not observed", "unknown", Colors[(uint)actor.Id.Value.GetHashCode() % Colors.Length]), effect.Time);
        _engine.Admit(id, effect.Time);
        return Attribute(actor, effect, key, provenMember: true);
    }
    private Guid[] OpenCandidates(IEnumerable<Pending> effects)
    {
        var present = _entities.Values.Select(entity => entity.Id).ToHashSet();
        HashSet<Guid>? members = null;
        foreach (var at in effects.Select(Moment).Distinct().Take(512))
        {
            var current = RosterAt(at) ?? FirstList ?? LeftPartyList ?? [];
            if (members is null) members = [.. current]; else members.IntersectWith(current);
        }
        return (members ?? []).Where(member => member != _localId && !present.Contains(member)).ToArray();
    }
    private Dictionary<(string Connection, long ObjectId), Guid[]> PossibleIdentities()
    {
        var result = new Dictionary<(string Connection, long ObjectId), Guid[]>();
        if (_engine is null || sources is null) return result;
        var objects = _unresolved.Where(pair => pair.Value.Count > 0 && _partyObjects.ContainsKey(pair.Key)).ToDictionary(pair => pair.Key, pair => pair.Value.Where(effect => effect.Spell > 0).Select(effect => effect.Spell!.Value).Distinct().Take(64).ToArray());
        bool Fits(Guid member, int[] skills)
        {
            if (!_characters.TryGetValue(member, out var character) || character.GearSeen < _identitiesSince || character.Equipment is null) return true;
            var worn = (_gearHistory.GetValueOrDefault(member) ?? []).Append(character.Equipment).ToArray();
            return skills.All(skill => worn.Any(gear => sources.CouldProduce(skill, gear)));
        }
        foreach (var (key, skills) in objects) result[key] = OpenCandidates(_unresolved[key]).Where(member => Fits(member, skills)).ToArray();
        for (var changed = true; changed;)
        {
            changed = false;
            foreach (var (key, members) in result.Where(pair => pair.Value.Length == 1).ToArray())
            {
                if (_entities.TryGetValue(key, out var entity) && entity.Id is not null) continue;
                var member = members[0];
                var known = _characters.GetValueOrDefault(member);
                Identify(key, new(member, _roster[member], "player", known?.Weapon, 0, known?.Equipment, known?.Spells, known?.WeaponSeen ?? 0, known?.GearSeen ?? 0), "skillProof");
                Count("identifiedBySkillProof");
                result.Remove(key);
                foreach (var other in result.Keys.ToArray()) if (result[other].Contains(member)) { result[other] = result[other].Where(candidate => candidate != member).ToArray(); changed = true; }
            }
        }
        return result;
    }
    private int?[]? CommonGear(Guid[] candidates)
    {
        if (candidates.Length == 0) return null;
        var worn = candidates.Select(candidate => _characters.TryGetValue(candidate, out var character) && character.GearSeen >= _identitiesSince ? character.Equipment : null).ToArray();
        if (worn.Any(gear => gear is null)) return null;
        var common = Enumerable.Range(0, worn.Min(gear => gear!.Length)).Select(slot => worn.Select(gear => gear![slot]).Distinct().Count() == 1 ? worn[0]![slot] : null).ToArray();
        if (common.All(slot => slot is null)) return null;
        Count("gearCommonToCandidates");
        return common;
    }
    private int?[]?[] CandidateGear(Guid[] candidates) => candidates.Select(candidate => _characters.TryGetValue(candidate, out var character) && character.GearSeen >= _identitiesSince ? character.Equipment : null).ToArray();
    private ItemAttribution? SourceForAny(Guid[] candidates, Pending effect)
    {
        var gear = CandidateGear(candidates);
        if (sources is null || gear.Length == 0 || gear.Any(worn => worn is null)) return null;
        var found = gear.Select(worn => SourceOf(null, worn, null, effect)).ToArray();
        if (found.Any(source => source.Index is null || source.Slot != found[0].Slot || source.Kind != found[0].Kind || !sources.SameFamily(source.Index.Value, found[0].Index!.Value))) return null;
        Count("sourceCommonToCandidates");
        return found.All(source => source.Index == found[0].Index && source.Exact) ? found[0] : found[0] with { Index = sources.FamilyRepresentative(found[0].Index!.Value), Exact = false };
    }
    private int? FamilyForAny(Guid[] candidates, int slot)
    {
        var worn = CandidateGear(candidates).Select(gear => gear?.ElementAtOrDefault(slot)).ToArray();
        if (sources is null || worn.Length == 0 || worn.Any(item => item is not > 0) || worn.Any(item => !sources.SameFamily(item!.Value, worn[0]!.Value))) return null;
        return worn[0];
    }
    private void GearChanged(Guid actorId, long timestamp)
    {
        if (actorId == _localId) return;
        var at = (timestamp - _start - _pausedDuration) / 1_000_000d;
        if (_unsourced.TryGetValue(actorId, out var pending)) pending.RemoveAll(entry => entry.Counted.Time < at);
    }
    private bool Attribute(Entity actor, Pending effect, (string Connection, long ObjectId)? key = null, bool provenMember = false)
    {
        var actorId = actor.Id!.Value;
        if (actorId != _localId && !provenMember)
        {
            if (ProvenMember(actorId, key, Moment(effect))) return Credit(actor, effect, key);
            AwaitMembership(actor, effect, key);
            return false;
        }
        Settle(actorId, actor);
        LogGear(actorId, actor, key?.ObjectId);
        var sourceItem = SourceOf(actor, effect);
        if (actorId == _localId && sourceItem is { Kind: "ability", Slot: "mainhand", Index: { } owned } && actor.Weapon != owned && FreshWeapon(actor) is null && key is { } ownKey)
        {
            actor = actor with { Weapon = owned, WeaponSeen = effect.At };
            _entities[ownKey] = actor; _characters[actorId] = actor; Synchronize();
            Count("weaponFromOwnedItem");
            Settle(actorId, actor);
        }
        if (effect.Spell is > 0 and var skill && (_skills.Count < 4096 || _skills.ContainsKey(actorId)) && (_skills.TryGetValue(actorId, out var known) ? known : _skills[actorId] = []).Count < 64) _skills[actorId].Add(skill);
        var counted = new CombatEvent(effect.Id, actorId.ToString(), effect.Time, effect.Delta < 0 ? CombatMetric.Damage : CombatMetric.Healing, Math.Abs(effect.Delta), FreshWeapon(actor), SourceItemIndex: sourceItem.Index, SourceSlot: sourceItem.Slot, SourceKind: sourceItem.Kind, SourceExact: sourceItem.Exact);
        if (!_engine!.Add(counted)) return false;
        if ((counted.SourceItemIndex is null || !counted.SourceExact || counted.WeaponIndex is null) && (_unsourced.Count < 256 || _unsourced.ContainsKey(actorId)))
        {
            var pending = _unsourced.TryGetValue(actorId, out var list) ? list : _unsourced[actorId] = [];
            if (pending.Count < 8192) pending.Add((counted, effect));
        }
        Count(effect.Delta < 0 ? $"damageTo:{effect.TargetKind}" : "positiveHealthUpdates");
        Count("attributedSinceZoneChange");
        return true;
    }
    private void AwaitMembership(Entity actor, Pending effect, (string Connection, long ObjectId)? key)
    {
        if ((!_rosterConfirmed || _leftParty is not null && !_leftPartySettled && RosterAt(Moment(effect)) is null) && _awaitingRoster.Count < 20000)
        {
            _awaitingRoster.Add((actor, effect, key, _visit));
            Count("awaitingRoster");
        }
        else Count("outsideParty");
    }
    private void NameByKill((string Connection, long ObjectId) key, Guid character, long timestamp)
    {
        if (_entities.TryGetValue(key, out var current) && current.Id is { } existing)
        {
            if (existing != character) Count("identityConflict");
            return;
        }
        var previous = _characters.GetValueOrDefault(character);
        Identify(key, new(character, _roster.GetValueOrDefault(character) ?? previous?.Name ?? "Unknown player", "player", previous?.Weapon, timestamp, previous?.Equipment, previous?.Spells, previous?.WeaponSeen ?? 0, previous?.GearSeen ?? 0), "kill");
        Count("identifiedByKill");
    }
    private void Identify((string Connection, long ObjectId) key, Entity entity, string method = "observed")
    {
        if (entity.Id is not null && _unresolvedGear.Remove(key, out var observed))
            entity = entity with { Weapon = observed.Gear.FirstOrDefault() is > 0 and var worn ? worn : entity.Weapon, Equipment = observed.Gear, Spells = observed.Spells.Length > 0 ? observed.Spells : entity.Spells, Revision = Math.Max(entity.Revision, _identitiesSince), WeaponSeen = Math.Max(entity.WeaponSeen, _identitiesSince), GearSeen = Math.Max(entity.GearSeen, _identitiesSince) };
        if (entity.Id is not null && _pendingEquipment.Remove(key, out var pending))
            entity = entity with { Weapon = pending.Weapon, Equipment = pending.Equipment, Spells = pending.Spells, Revision = Math.Max(entity.Revision, pending.Timestamp), WeaponSeen = pending.Timestamp };
        if (entity.Id is { } sighted) Sight(sighted, key.ObjectId, entity.Revision, method);
        if (_entities.Count < 4096 || _entities.ContainsKey(key)) _entities[key] = entity;
        if (entity.Id is { } guid && (_characters.Count < 4096 || _characters.ContainsKey(guid))) _characters[guid] = entity;
        Synchronize();
        if (entity.Id is null || _engine is null || !_unresolved.Remove(key, out var effects)) return;
        _unresolvedCount -= effects.Count;
        if (effects.Count > 0 && entity.Id == _localId) _engine.ExtendMembership(entity.Id.Value.ToString(), effects.Min(effect => effect.Time));
        foreach (var effect in effects)
            if (entity.Id == _localId ? Attribute(_entities.GetValueOrDefault(key) ?? entity, effect, key) : Credit(_entities.GetValueOrDefault(key) ?? entity, effect, key)) Count("creditedAfterIdentification");
            else if (entity.Id != _localId) AwaitMembership(_entities.GetValueOrDefault(key) ?? entity, effect, key);
    }
    public void ObserveOwnCast(WireMessage wire, long timestamp)
    {
        if (wire.Kind == 2 && Integer(wire.Parameters.GetValueOrDefault((byte)253)) is 225 or 227) _partyActionAt = timestamp;
        if (wire.Kind == 2 && Integer(wire.Parameters.GetValueOrDefault((byte)253)) == 237) _leavingAt = timestamp;
        if (wire.Kind == 2 && Integer(wire.Parameters.GetValueOrDefault((byte)253)) == 30 && wire.Parameters.GetValueOrDefault((byte)1) is byte[] { Length: 16 } from
            && wire.Parameters.GetValueOrDefault((byte)4) is byte[] { Length: 16 } to && _moveRequests.Count < 1024) _moveRequests.Add((timestamp, new Guid(from), new Guid(to)));
        if (wire.Kind != 2 || Integer(wire.Parameters.GetValueOrDefault((byte)253)) != 24 || Integer(wire.Parameters.GetValueOrDefault((byte)0)) is not { } cast) return;
        foreach (var old in _ownCasts.Where(pair => timestamp - pair.Value > 5_000_000).Select(pair => pair.Key).ToArray()) _ownCasts.Remove(old);
        if (_ownCasts.Count < 256) _ownCasts[cast] = timestamp;
    }
    private void ObserveCastEcho(string connection, long caster, long cast, long timestamp, LocalCharacterSnapshot? local)
    {
        if (!_ownCasts.Remove(cast) || _localId is not { } localId) return;
        if (_entities.TryGetValue((connection, caster), out var current) && current.Id == localId) return;
        if (_entities.TryGetValue((connection, caster), out var other) && other.Id is not null) { Count("identityConflict"); return; }
        var previous = _characters.GetValueOrDefault(localId);
        Identify((connection, caster), new(localId, local?.Name ?? _roster.GetValueOrDefault(localId) ?? previous?.Name ?? "You", "player", previous?.Weapon, timestamp, previous?.Equipment, previous?.Spells, previous?.WeaponSeen ?? 0, previous?.GearSeen ?? 0), "ownCast");
        Count("identifiedByOwnCast");
    }
    private void ReportedGear(string connection, Guid member, int?[] gear, long timestamp)
    {
        if (gear.Length < 7 || _fingerprints.Count >= 4096 && !_fingerprints.ContainsKey(member)) return;
        var known = _fingerprints.TryGetValue(member, out var list) ? list : _fingerprints[member] = [];
        if (!known.Any(fingerprint => fingerprint.SequenceEqual(gear))) { known.Insert(0, gear); if (known.Count > 8) known.RemoveAt(8); }
        var previous = _characters.GetValueOrDefault(member);
        if (_characters.Count < 4096 || previous is not null)
            _characters[member] = new(member, _roster.GetValueOrDefault(member) ?? previous?.Name ?? "Unknown player", "player", gear[0] ?? previous?.Weapon, timestamp, gear, previous?.Spells, timestamp, timestamp);
        foreach (var (objectKey, entity) in _entities.Where(pair => pair.Value.Id == member && pair.Value.GearSeen < timestamp).ToArray())
            _entities[objectKey] = _characters[member] = entity with { Weapon = gear[0] ?? entity.Weapon, Equipment = gear, Spells = entity.Equipment?.SequenceEqual(gear) == true ? entity.Spells : null, WeaponSeen = timestamp, GearSeen = timestamp };
        Rematch(connection, timestamp);
    }
    private readonly Dictionary<(string Connection, Guid Character), (long[] Items, long Timestamp)> _inspections = [];
    private void CompleteInspections(string connection)
    {
        foreach (var ((owner, character), (items, at)) in _inspections.Where(pair => pair.Key.Connection == connection).ToArray())
        {
            if (items.Take(8).Any(item => item > 0 && !_itemTypes.ContainsKey((connection, item)))) continue;
            _inspections.Remove((owner, character));
            ReportedGear(connection, character, items.Select(item => item == 0 ? 0 : _itemTypes.TryGetValue((connection, item), out var type) ? type.Type : (int?)null).ToArray(), at);
            Count("gearFromInspection");
        }
    }
    private Entity? Fingerprint((string Connection, long ObjectId) key, int?[] equipment)
    {
        if (!_partyObjects.ContainsKey(key) || equipment.Take(8).Count(slot => slot is not null) < 7) return null;
        var candidates = CompleteCandidates(ClockTimestamp ?? _identitiesSince);
        if (candidates is null || candidates.Length == 0) return null;
        var worn = candidates.Select(candidate => (Candidate: candidate, Gear: _characters.TryGetValue(candidate, out var character) && character.GearSeen >= _identitiesSince ? character.Equipment : null)).ToArray();
        if (worn.Any(entry => entry.Gear is null)) return null;
        static bool Same(int?[] a, int?[] b) => Enumerable.Range(0, 8).All(slot => a.ElementAtOrDefault(slot) is not { } first || b.ElementAtOrDefault(slot) is not { } second || first == second)
            && Enumerable.Range(0, 8).Count(slot => a.ElementAtOrDefault(slot) is not null && b.ElementAtOrDefault(slot) is not null) >= 7;
        var matches = worn.Where(entry => Same(equipment, entry.Gear!)).ToArray();
        return matches is [var only] ? _characters[only.Candidate] : null;
    }
    private Guid[]? CompleteCandidates(long timestamp)
    {
        var members = RosterAt(timestamp) ?? (_rosterHistory.Count == 0 ? null : FirstList ?? LeftPartyList);
        if (members is null) return null;
        var present = _entities.Values.Select(entity => entity.Id).ToHashSet();
        return members.Where(member => member != _localId && !present.Contains(member)).ToArray();
    }
    private void Rematch(string connection, long timestamp)
    {
        foreach (var (key, (gear, _, _)) in _unresolvedGear.ToArray())
            if (!_entities.ContainsKey(key) && Fingerprint(key, gear) is { } match)
            {
                Identify(key, match with { Revision = timestamp, Equipment = gear, Weapon = gear.FirstOrDefault() ?? match.Weapon, WeaponSeen = timestamp, GearSeen = timestamp }, "equipmentProof");
                Count("identifiedByEquipment");
            }
    }
    public void FlushUnidentified()
    {
        if (AwaitingProof) SettleAwaiting(long.MaxValue, final: false);
        foreach (var actorId in _unsourced.Keys.ToArray()) if (_characters.TryGetValue(actorId, out var known)) Settle(actorId, known);
        var openObjects = _partyObjects.Keys.Where(key => !_entities.TryGetValue(key, out var entity) || entity.Id is null).ToArray();
        var openMembers = openObjects.Length == 1 && _unresolved.TryGetValue(openObjects[0], out var waiting) ? OpenCandidates(waiting) : [];
        if (_engine is not null && openObjects.Length == 1 && openMembers.Length == 1)
        {
            var previous = _characters.GetValueOrDefault(openMembers[0]);
            Identify(openObjects[0], new(openMembers[0], _roster[openMembers[0]], "player", previous?.Weapon, 0, previous?.Equipment, previous?.Spells, previous?.WeaponSeen ?? 0, previous?.GearSeen ?? 0), "elimination");
            Count("identifiedByElimination");
        }
        var possible = PossibleIdentities();
        if (_engine is not null)
            foreach (var ((connection, objectId), effects) in _unresolved)
            {
                var partyMember = _partyObjects.ContainsKey((connection, objectId));
                if (!partyMember || effects.Count == 0) { if (effects.Count > 0) Count("droppedOutsidePartyObjects"); continue; }
                var id = $"unidentified:{objectId}:{(long)(effects[0].Time * 1000)}";
                var candidates = possible.TryGetValue((connection, objectId), out var consistent) ? consistent : OpenCandidates(effects);
                var gear = _unresolvedGear.TryGetValue((connection, objectId), out var observed) ? observed.Gear : CommonGear(candidates);
                var weapon = gear?.FirstOrDefault() is > 0 and var worn ? worn : (int?)null;
                var weaponFamily = weapon is null ? FamilyForAny(candidates, 0) : null;
                _engine.RegisterMember(new(id, $"Unidentified player (obj {objectId})", weapon is > 0 ? $"Item #{weapon}" : weaponFamily is { } family ? sources!.FamilyName(family) : "Weapon not observed", "unknown", Colors[(uint)objectId % Colors.Length]), effects[0].Time);
                if (_kept.Count >= 1000) _kept.RemoveAt(0);
                _kept.Add(new(id, _engine.Id, objectId, gear, candidates, _start + (long)(effects[0].Time * 1_000_000), Signature(effects.Where(effect => effect.Spell > 0).Select(effect => effect.Spell!.Value)), _visit));
                foreach (var effect in effects)
                {
                    var source = effect.Spell == -1 && effect.Kind is 0 or 1 ? new ItemAttribution(weapon, "mainhand", "autoAttack") : SourceOf(null, gear, observed.Spells, effect);
                    if (source.Index is null && SourceForAny(candidates, effect) is { } shared) source = shared;
                    _engine.Add(new(effect.Id, id, effect.Time, effect.Delta < 0 ? CombatMetric.Damage : CombatMetric.Healing, Math.Abs(effect.Delta), weapon,
                        SourceItemIndex: source.Index, SourceSlot: source.Slot, SourceKind: source.Kind, SourceExact: source.Exact));
                }
                Count("unidentifiedPlayersKept");
            }
        ClearUnidentified();
    }
    private void ClearUnidentified() { _unresolved.Clear(); _unresolvedCount = 0; _unresolvedGear.Clear(); }
    private void ApplyLocalEquipment(string connection, long timestamp, LocalCharacterSnapshot? local)
    {
        if (local is not { ObjectId: { } objectId, LocationState: "observed" } || _localItems.Length == 0) return;
        var equipment = _localItems.Select(item => item is null or 0 ? 0 : _itemTypes.TryGetValue((connection, item.Value), out var type) ? type.Type : (int?)null).ToArray();
        var previous = _characters.GetValueOrDefault(local.CharacterId);
        var fresh = previous is not null && previous.GearSeen >= _identitiesSince ? previous.Spells : null;
        var spells = new int?[SkillSlots.Length];
        for (var index = 0; index < SkillSlots.Length; index++)
        {
            var slot = SkillSlots[index];
            var order = SkillSlots.Take(index).Count(earlier => earlier == slot);
            spells[index] = slot >= 0 && _localItems.ElementAtOrDefault(slot) is { } item && _itemSpells.TryGetValue((connection, item), out var chosen)
                ? chosen.ElementAtOrDefault(order) : fresh?.ElementAtOrDefault(index);
        }
        var entity = new Entity(local.CharacterId, local.Name, "player", equipment[0] ?? previous?.Weapon, timestamp,
            equipment.Select((type, slot) => type ?? previous?.Equipment?.ElementAtOrDefault(slot)).ToArray(), spells, equipment[0] is null ? previous?.WeaponSeen ?? 0 : timestamp, equipment.Any(type => type is not null) ? timestamp : previous?.GearSeen ?? 0);
        _entities[(local.Connection, objectId)] = entity;
        _characters[local.CharacterId] = entity;
        Synchronize();
        Settle(local.CharacterId, entity);
    }
    public void SetParty(PartyRosterSnapshot? party)
    {
        if (party == _party) return;
        _party = party;
        if (party is null || !party.IsConfirmedSince(_identitiesSince))
        {
            foreach (var member in party?.Members ?? []) if (_characters.TryGetValue(member.Id, out var named) && named.Name != member.Name) _characters[member.Id] = named with { Name = member.Name };
            return;
        }
        var at = party.ObservedAtMicroseconds;
        var first = !_rosterConfirmed;
        _rosterConfirmed = true;
        if (_rosterHistory.Count >= 1000) _rosterHistory.RemoveAt(0);
        _rosterHistory.Add((at, party.Members.Select(member => member.Id).ToHashSet(), party.PartyId));
        if (first) _firstListJoined = _partyActionAt != long.MinValue && at - _partyActionAt <= 10_000_000;
        foreach (var member in party.Members) if (!_roster.ContainsKey(member.Id)) Log(new(at, _visit, "joined", member.Id, member.Name));
        foreach (var (member, name) in _roster) if (party.Members.All(current => current.Id != member)) Log(new(at, _visit, "left", member, name));
        _roster.Clear();
        foreach (var member in party.Members)
        {
            _roster[member.Id] = member.Name;
            if (_characters.TryGetValue(member.Id, out var known) && known.Name != member.Name) _characters[member.Id] = known with { Name = member.Name };
        }
        Synchronize();
        if (first) SettleAwaiting(at, final: _leftParty is null);
        if (!_leftPartySettled && _leftParty is { } left && party.PartyId == left) { _leftPartySettled = true; SettleAwaiting(_localLeftAt, final: true); }
        if (_connection is { } connection) Rematch(connection, party.ObservedAtMicroseconds);
    }
    private void Synchronize()
    {
        if (_engine is null) return;
        var active = new Dictionary<Guid, string>(_roster);
        if (_localId is { } local && _characters.TryGetValue(local, out var self)) active[local] = _roster.GetValueOrDefault(local) ?? self.Name;
        if (_rosterConfirmed)
            foreach (var member in _engine.Snapshot(false).Members)
                if (Guid.TryParse(member.Id, out var memberId) && !active.ContainsKey(memberId)) _engine.SetMembership(member.Id, false);
        foreach (var member in active)
        {
            var weapon = _characters.GetValueOrDefault(member.Key) is { } character && character.WeaponSeen >= _identitiesSince ? character.Weapon : null;
            _engine.RegisterMember(new(member.Key.ToString(), member.Value, weapon is null ? "Weapon not observed" : weapon == 0 ? "Unarmed" : $"Item #{weapon}", "unknown", Colors[(uint)member.Key.GetHashCode() % Colors.Length]));
        }
    }
    private static readonly string[] Colors = ["#9ebbf3", "#b69aff", "#67cbd7", "#eab978", "#6dd1ad", "#eb95b7"];
}
