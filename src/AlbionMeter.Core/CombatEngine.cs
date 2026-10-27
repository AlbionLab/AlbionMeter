namespace AlbionMeter.Core;

public enum CombatMetric { Damage, Healing }
public sealed record PartyMember(string Id, string Name, string Weapon, string WeaponFamily, string Color);
public sealed record CombatEvent(string Id, string ActorId, double Time, CombatMetric Metric, double Amount, int? WeaponIndex = null, string? WeaponName = null, int? SourceItemIndex = null, string SourceSlot = "unknown", string SourceKind = "unknown", bool SourceExact = true);
public sealed record PlayerStats(string Id, string Name, string Weapon, string WeaponFamily, string Color,
    double Damage, double Healing, double EncounterDps, double EncounterHps, double RollingDps,
    double RollingHps, double DamageShare, double HealingShare, string Membership = "active");
public sealed record WeaponContribution(string Id, string PlayerId, string PlayerName, int? WeaponIndex, string Weapon, string Color, double Damage, double Healing);
public sealed record ItemContribution(string Id, string PlayerId, string PlayerName, int? ItemIndex, string Slot, string Item, string Color, double Damage, double Healing, string Kind = "unknown", bool TierObserved = true);
public sealed record TimelinePoint(int Second, Dictionary<string, double> Damage, Dictionary<string, double> Healing,
    Dictionary<string, double>? WeaponDamage = null, Dictionary<string, double>? WeaponHealing = null, Dictionary<string, double>? ItemDamage = null, Dictionary<string, double>? ItemHealing = null);
public sealed record CombatSnapshot(string Id, DateTimeOffset StartedAt, double ElapsedSeconds,
    string Status, string Mode, long Sequence, long EventCount, double TotalDamage, double TotalHealing,
    PlayerStats[] Members, TimelinePoint[] Timeline, bool Full, DungeonLocation? Location = null, PlayerStats[]? FormerMembers = null, string? PartyId = null, WeaponContribution[]? WeaponContributions = null, ItemContribution[]? ItemContributions = null);

public sealed class CombatEngine
{
    public const int MaximumSeconds = 7200;
    private const int MaximumEvents = 1_000_000;
    private readonly List<PartyMember> _members;
    private readonly Dictionary<string, List<(double Start, double? End)>> _membership = [];
    private readonly Dictionary<string, double> _damage;
    private readonly Dictionary<string, double> _healing;
    private readonly Dictionary<int, TimelinePoint> _timeline = [];
    private readonly Dictionary<string, WeaponContribution> _weapons = [];
    private readonly Dictionary<string, ItemContribution> _items = [];
    private readonly HashSet<string> _eventIds = [];
    private readonly List<CombatEvent> _recentEvents = [];
    private readonly SortedSet<int> _dirtySeconds = [];
    public string Id { get; }
    public string PartyId { get; }
    public DateTimeOffset StartedAt { get; } = DateTimeOffset.UtcNow;
    public double ElapsedSeconds { get; private set; }
    public long Sequence { get; private set; }
    public int EventCount => _eventIds.Count;

    public CombatEngine(IEnumerable<PartyMember> members, string? partyId = null, string? id = null)
    {
        Id = id ?? Guid.NewGuid().ToString("N");
        PartyId = partyId ?? Guid.NewGuid().ToString("N");
        _members = members.Take(101).ToList();
        if (_members.Count > 100 || _members.Any(x => string.IsNullOrWhiteSpace(x.Id)) || _members.Select(x => x.Id).Distinct().Count() != _members.Count)
            throw new ArgumentException("A party can contain at most 100 unique members with nonempty identities.", nameof(members));
        _damage = _members.ToDictionary(x => x.Id, _ => 0d);
        _healing = _members.ToDictionary(x => x.Id, _ => 0d);
        foreach (var member in _members) _membership[member.Id] = [(0, null)];
        EnsureSecond(0);
    }

    public bool Knows(string actorId) => _membership.ContainsKey(actorId);
    public bool IsMember(string actorId) => _membership.TryGetValue(actorId, out var periods)
        && periods.Count > 0 && periods[^1].End is null;

    public void SetMembership(string actorId, bool active)
    {
        if (!_membership.TryGetValue(actorId, out var periods)) throw new ArgumentException("Unknown party member.", nameof(actorId));
        if (IsMember(actorId) == active) return;
        if (active)
        {
            if (periods.Count >= 1000) throw new InvalidOperationException("The membership transition limit was reached.");
            periods.Add((ElapsedSeconds, null));
        }
        else periods[^1] = (periods[^1].Start, ElapsedSeconds);
        Sequence++;
    }

    public void ExtendMembership(string actorId, double since)
    {
        if (!_membership.TryGetValue(actorId, out var periods) || periods.Count != 1 || periods[0].Start <= since) return;
        periods[0] = (Math.Max(0, since), periods[0].End);
        Sequence++;
    }

    public void Admit(string actorId, double time)
    {
        if (!_membership.TryGetValue(actorId, out var periods)) return;
        var index = periods.FindIndex(period => period.End is null || period.End > time);
        if (index < 0 || periods[index].Start <= time) return;
        periods[index] = (Math.Max(Math.Max(0, time), index > 0 ? periods[index - 1].End ?? 0 : 0), periods[index].End);
        Sequence++;
    }

    public void CoverMembership(string actorId, double since)
    {
        if (!_membership.TryGetValue(actorId, out var periods) || periods.Count == 0) return;
        var start = Math.Max(Math.Max(0, since), periods.Count > 1 ? periods[^2].End ?? 0 : 0);
        if (periods[^1].Start <= start) return;
        periods[^1] = (start, periods[^1].End);
        Sequence++;
    }

    public void RegisterMember(PartyMember member, double since)
    {
        var known = _membership.ContainsKey(member.Id);
        RegisterMember(member);
        if (!known) _membership[member.Id][0] = (Math.Clamp(since, 0, ElapsedSeconds), null);
    }

    public void RegisterMember(PartyMember member)
    {
        if (string.IsNullOrWhiteSpace(member.Id)) throw new ArgumentException("Character identity is required.", nameof(member));
        var index = _members.FindIndex(value => value.Id == member.Id);
        if (index >= 0)
        {
            _members[index] = member; SetMembership(member.Id, true); Sequence++; return;
        }
        if (_members.Count >= 100) throw new InvalidOperationException("The encounter participant limit was reached.");
        _members.Add(member); _damage[member.Id] = 0; _healing[member.Id] = 0;
        _membership[member.Id] = [(ElapsedSeconds, null)];
        foreach (var point in _timeline.Values)
        {
            point.Damage[member.Id] = 0; point.Healing[member.Id] = 0; _dirtySeconds.Add(point.Second);
        }
        Sequence++;
    }

    public void Advance(double time)
    {
        if (!double.IsFinite(time) || time < ElapsedSeconds || time > MaximumSeconds)
            throw new ArgumentOutOfRangeException(nameof(time));
        var previousSecond = (int)ElapsedSeconds;
        ElapsedSeconds = time;
        for (var second = previousSecond; second <= (int)time; second++) EnsureSecond(second);
        _recentEvents.RemoveAll(x => x.Time < time - 10);
        Sequence++;
    }

    public bool Add(CombatEvent combatEvent)
    {
        if (string.IsNullOrWhiteSpace(combatEvent.Id) || !_damage.ContainsKey(combatEvent.ActorId)
            || !Enum.IsDefined(combatEvent.Metric) || !double.IsFinite(combatEvent.Amount)
            || combatEvent.Amount <= 0 || !double.IsFinite(combatEvent.Time)
            || combatEvent.Time < 0 || combatEvent.Time > ElapsedSeconds
            || EventCount >= MaximumEvents) return false;
        if (!_membership[combatEvent.ActorId].Any(period => combatEvent.Time >= period.Start
            && (period.End is null || combatEvent.Time < period.End))) return false;
        var currentTotal = combatEvent.Metric == CombatMetric.Damage ? _damage[combatEvent.ActorId] : _healing[combatEvent.ActorId];
        if (!double.IsFinite(currentTotal + combatEvent.Amount) || !_eventIds.Add(combatEvent.Id)) return false;
        var second = (int)Math.Floor(combatEvent.Time);
        var bucket = EnsureSecond(second);
        var totals = combatEvent.Metric == CombatMetric.Damage ? _damage : _healing;
        var values = combatEvent.Metric == CombatMetric.Damage ? bucket.Damage : bucket.Healing;
        totals[combatEvent.ActorId] += combatEvent.Amount;
        values[combatEvent.ActorId] += combatEvent.Amount;
        Credit(combatEvent, bucket, 1);
        if (combatEvent.Time >= ElapsedSeconds - 10) _recentEvents.Add(combatEvent);
        _dirtySeconds.Add(second);
        Sequence++;
        return true;
    }

    public bool Reattribute(CombatEvent counted, CombatEvent proven)
    {
        if (counted.Id != proven.Id || counted.ActorId != proven.ActorId || counted.Metric != proven.Metric || counted.Amount != proven.Amount
            || counted.Time != proven.Time || !_eventIds.Contains(counted.Id) || !_damage.ContainsKey(counted.ActorId)) return false;
        var second = (int)Math.Floor(counted.Time);
        var bucket = EnsureSecond(second);
        Credit(counted, bucket, -1);
        Credit(proven, bucket, 1);
        _dirtySeconds.Add(second);
        Sequence++;
        return true;
    }

    private void Credit(CombatEvent combatEvent, TimelinePoint bucket, int sign)
    {
        var amount = sign * combatEvent.Amount;
        static void Shift(Dictionary<string, double> values, string key, double amount)
        {
            var value = values.GetValueOrDefault(key) + amount;
            if (Math.Abs(value) < 1e-6) values.Remove(key); else values[key] = value;
        }
        var weaponKey = $"{combatEvent.ActorId}:{combatEvent.WeaponIndex?.ToString() ?? combatEvent.WeaponName ?? "unknown"}";
        if (!_weapons.TryGetValue(weaponKey, out var weapon))
        {
            var member = _members.First(member => member.Id == combatEvent.ActorId);
            weapon = new(weaponKey, member.Id, member.Name, combatEvent.WeaponIndex,
                combatEvent.WeaponName ?? (combatEvent.WeaponIndex == 0 ? "Unarmed" : combatEvent.WeaponIndex is { } index ? $"Item #{index}" : "Weapon not observed"), member.Color, 0, 0);
        }
        weapon = combatEvent.Metric == CombatMetric.Damage ? weapon with { Damage = weapon.Damage + amount } : weapon with { Healing = weapon.Healing + amount };
        if (weapon.Damage + weapon.Healing < 1e-6) _weapons.Remove(weaponKey); else _weapons[weaponKey] = weapon;
        Shift(combatEvent.Metric == CombatMetric.Damage ? bucket.WeaponDamage! : bucket.WeaponHealing!, weaponKey, amount);
        var itemKey = $"{combatEvent.ActorId}:{combatEvent.SourceSlot}:{combatEvent.SourceItemIndex?.ToString() ?? "unknown"}:{combatEvent.SourceKind}{(combatEvent.SourceExact ? "" : ":family")}";
        if (!_items.TryGetValue(itemKey, out var item)) item = new(itemKey, weapon.PlayerId, weapon.PlayerName, combatEvent.SourceItemIndex, combatEvent.SourceSlot, combatEvent.SourceItemIndex is { } itemIndex ? $"Item #{itemIndex}" : "Unknown source", weapon.Color, 0, 0, combatEvent.SourceKind, combatEvent.SourceExact);
        item = combatEvent.Metric == CombatMetric.Damage ? item with { Damage = item.Damage + amount } : item with { Healing = item.Healing + amount };
        if (item.Damage + item.Healing < 1e-6) _items.Remove(itemKey); else _items[itemKey] = item;
        Shift(combatEvent.Metric == CombatMetric.Damage ? bucket.ItemDamage! : bucket.ItemHealing!, itemKey, amount);
    }

    public double RollingRate(string actorId, CombatMetric metric, double window = 5)
    {
        if (window <= 0 || window > 10 || !double.IsFinite(window)) throw new ArgumentOutOfRangeException(nameof(window));
        var duration = Math.Min(window, ElapsedSeconds);
        if (duration <= 0) return 0;
        var start = ElapsedSeconds - window;
        return _recentEvents.Where(x => x.ActorId == actorId && x.Metric == metric
            && (x.Time > start || start < 0) && x.Time <= ElapsedSeconds).Sum(x => x.Amount) / duration;
    }

    private string NameOf(string playerId, string recorded) => _members.FirstOrDefault(member => member.Id == playerId)?.Name ?? recorded;

    public CombatSnapshot Snapshot(bool full, string status = "running")
    {
        var totalDamage = _damage.Values.Sum();
        var totalHealing = _healing.Values.Sum();
        var duration = ElapsedSeconds;
        var players = _members.Select(x => new PlayerStats(x.Id, x.Name, x.Weapon, x.WeaponFamily, x.Color,
            _damage[x.Id], _healing[x.Id], duration > 0 ? _damage[x.Id] / duration : 0,
            duration > 0 ? _healing[x.Id] / duration : 0, RollingRate(x.Id, CombatMetric.Damage),
            RollingRate(x.Id, CombatMetric.Healing), totalDamage > 0 ? _damage[x.Id] / totalDamage * 100 : 0,
            totalHealing > 0 ? _healing[x.Id] / totalHealing * 100 : 0)).ToArray();
        var seconds = full ? _timeline.Keys.Order().ToArray() : _dirtySeconds.ToArray();
        var points = seconds.Select(x => _timeline[x]).Select(x => new TimelinePoint(x.Second,
            new Dictionary<string, double>(x.Damage), new Dictionary<string, double>(x.Healing), new(x.WeaponDamage!), new(x.WeaponHealing!), new(x.ItemDamage!), new(x.ItemHealing!))).ToArray();
        if (!full) _dirtySeconds.Clear();
        return new CombatSnapshot(Id, StartedAt, duration, status, "live", Sequence, EventCount,
            totalDamage, totalHealing, players.Where(player => IsMember(player.Id)).ToArray(), points, full,
            FormerMembers: players.Where(player => !IsMember(player.Id)).Select(player => player with { RollingDps = 0, RollingHps = 0 }).ToArray(), PartyId: PartyId, WeaponContributions: _weapons.Values.Select(weapon => weapon with { PlayerName = NameOf(weapon.PlayerId, weapon.PlayerName) }).ToArray(),
            ItemContributions: _items.Values.Select(item => item with { PlayerName = NameOf(item.PlayerId, item.PlayerName) }).ToArray());
    }

    private TimelinePoint EnsureSecond(int second)
    {
        if (_timeline.TryGetValue(second, out var existing)) return existing;
        var point = new TimelinePoint(second, _members.ToDictionary(x => x.Id, _ => 0d),
            _members.ToDictionary(x => x.Id, _ => 0d), [], [], [], []);
        _timeline[second] = point;
        _dirtySeconds.Add(second);
        return point;
    }
}
