namespace AlbionMeter.Core;

public sealed record DungeonLocation(string RunId, string Name, string EntranceMapId, string EntranceMapName,
    string FloorId, int FloorNumber, string MapId, string MapName);
public sealed record DungeonFloor(string Id, int Number, string MapId, string MapName,
    CombatSnapshot[] Encounters, CombatSnapshot Totals);
public sealed record DungeonRunSnapshot(string Id, DateTimeOffset StartedAt, string Name, string EntranceMapId,
    string EntranceMapName, string Mode, string Status, DungeonFloor[] Floors, CombatSnapshot Totals);

public sealed class DungeonRun(string name, string entranceMapId, string entranceMapName, string mode, string? id = null, DateTimeOffset? startedAt = null)
{
    private sealed class Floor(string id, int number, string mapId, string mapName)
    {
        public string Id { get; } = id;
        public int Number { get; } = number;
        public string MapId { get; } = mapId;
        public string MapName { get; } = mapName;
        public Dictionary<string, CombatSnapshot> Encounters { get; } = [];
    }
    private readonly List<Floor> _floors = [];
    private readonly Dictionary<string, string> _encounterFloors = [];
    private Floor? _current;
    public string Id { get; } = id ?? Guid.NewGuid().ToString("N");
    public DateTimeOffset StartedAt { get; } = startedAt ?? DateTimeOffset.UtcNow;

    public bool ContainsFloor(string mapId) => _floors.Any(floor => floor.MapId == mapId);

    public DungeonLocation EnterFloor(string clusterId, string mapId, string mapName)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(clusterId);
        var existing = _floors.FirstOrDefault(floor => floor.Id == clusterId);
        if (existing is null)
        {
            if (_floors.Count >= 100) throw new InvalidOperationException("The dungeon floor limit was reached.");
            existing = new Floor(clusterId, _floors.Count + 1, mapId, mapName);
            _floors.Add(existing);
        }
        _current = existing;
        return Location;
    }

    public DungeonLocation Location => _current is { } floor
        ? new(Id, name, entranceMapId, entranceMapName, floor.Id, floor.Number, floor.MapId, floor.MapName)
        : throw new InvalidOperationException("Enter a floor before recording combat.");

    public void Record(string clusterId, CombatSnapshot encounter)
    {
        if (!encounter.Full) throw new ArgumentException("Dungeon checkpoints require a full snapshot.", nameof(encounter));
        if (encounter.Mode != mode) throw new ArgumentException("Encounter sources cannot be mixed.", nameof(encounter));
        var floor = _floors.SingleOrDefault(value => value.Id == clusterId)
            ?? throw new ArgumentException("The floor has not been observed.", nameof(clusterId));
        if (_encounterFloors.TryGetValue(encounter.Id, out var owner) && owner != clusterId)
            throw new InvalidOperationException("An encounter cannot belong to two floors.");
        if (floor.Encounters.TryGetValue(encounter.Id, out var previous) && previous.Sequence > encounter.Sequence) return;
        if (_encounterFloors.Count >= 1000 && !_encounterFloors.ContainsKey(encounter.Id))
            throw new InvalidOperationException("The dungeon encounter limit was reached.");
        _encounterFloors[encounter.Id] = clusterId;
        floor.Encounters[encounter.Id] = Clone(encounter);
    }

    public DungeonRunSnapshot Snapshot(string status)
    {
        var floors = _floors.Select(floor =>
        {
            var encounters = floor.Encounters.Values.Select(Clone).ToArray();
            return new DungeonFloor(floor.Id, floor.Number, floor.MapId, floor.MapName, encounters,
                Aggregate(encounters, $"{Id}:{floor.Id}", status));
        }).ToArray();
        return new(Id, StartedAt, name, entranceMapId, entranceMapName, mode, status, floors,
            Aggregate(floors.SelectMany(floor => floor.Encounters).ToArray(), Id, status));
    }

    private CombatSnapshot Aggregate(CombatSnapshot[] encounters, string id, string status)
    {
        var seconds = encounters.Sum(encounter => encounter.ElapsedSeconds);
        var damage = encounters.Sum(encounter => encounter.TotalDamage);
        var healing = encounters.Sum(encounter => encounter.TotalHealing);
        var members = encounters.SelectMany(encounter => encounter.Members.Concat(encounter.FormerMembers ?? [])).GroupBy(member => member.Id).Select(group =>
        {
            var member = group.Last();
            var playerDamage = group.Sum(player => player.Damage);
            var playerHealing = group.Sum(player => player.Healing);
            return member with
            {
                Damage = playerDamage, Healing = playerHealing,
                EncounterDps = seconds > 0 ? playerDamage / seconds : 0,
                EncounterHps = seconds > 0 ? playerHealing / seconds : 0,
                RollingDps = 0, RollingHps = 0,
                DamageShare = damage > 0 ? playerDamage / damage * 100 : 0,
                HealingShare = healing > 0 ? playerHealing / healing * 100 : 0
            };
        }).ToArray();
        var weapons = encounters.SelectMany(encounter => encounter.WeaponContributions ?? []).GroupBy(item => item.Id)
            .Select(group => group.Last() with { Damage = group.Sum(item => item.Damage), Healing = group.Sum(item => item.Healing) }).ToArray();
        var items = encounters.SelectMany(encounter => encounter.ItemContributions ?? []).GroupBy(item => item.Id)
            .Select(group => group.Last() with { Damage = group.Sum(item => item.Damage), Healing = group.Sum(item => item.Healing) }).ToArray();
        var timeline = new SortedDictionary<int, TimelinePoint>();
        var offset = 0d;
        foreach (var encounter in encounters.OrderBy(encounter => encounter.StartedAt))
        {
            foreach (var point in encounter.Timeline)
            {
                var second = (int)Math.Floor(offset + point.Second);
                if (!timeline.TryGetValue(second, out var target))
                {
                    target = new(second, [], [], [], [], [], []);
                    timeline[second] = target;
                }
                void Add(Dictionary<string, double> output, Dictionary<string, double>? input)
                {
                    foreach (var pair in input ?? []) output[pair.Key] = output.GetValueOrDefault(pair.Key) + pair.Value;
                }
                Add(target.Damage, point.Damage); Add(target.Healing, point.Healing);
                Add(target.WeaponDamage!, point.WeaponDamage); Add(target.WeaponHealing!, point.WeaponHealing);
                Add(target.ItemDamage!, point.ItemDamage); Add(target.ItemHealing!, point.ItemHealing);
            }
            offset += encounter.ElapsedSeconds;
        }
        return new(id, StartedAt, seconds, status, mode, encounters.Sum(encounter => encounter.Sequence),
            encounters.Sum(encounter => encounter.EventCount), damage, healing, members, timeline.Values.ToArray(), true,
            Location: encounters.FirstOrDefault()?.Location, WeaponContributions: weapons, ItemContributions: items);
    }

    private static CombatSnapshot Clone(CombatSnapshot snapshot) => snapshot with
    {
        Members = snapshot.Members.ToArray(),
        FormerMembers = snapshot.FormerMembers?.ToArray(),
        Timeline = snapshot.Timeline.Select(point => new TimelinePoint(point.Second,
            new Dictionary<string, double>(point.Damage), new Dictionary<string, double>(point.Healing),
            point.WeaponDamage is null ? null : new(point.WeaponDamage), point.WeaponHealing is null ? null : new(point.WeaponHealing),
            point.ItemDamage is null ? null : new(point.ItemDamage), point.ItemHealing is null ? null : new(point.ItemHealing))).ToArray(),
        WeaponContributions = snapshot.WeaponContributions?.ToArray(), ItemContributions = snapshot.ItemContributions?.ToArray()
    };
}
