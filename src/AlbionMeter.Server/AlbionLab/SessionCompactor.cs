namespace AlbionLab.Protocol;

public sealed record SourceMember(string Id, string Name, string Weapon, string Family, string Color, string Membership = "active", string? WeaponId = null);
public sealed record SourceWeapon(string Id, string PlayerId, int? Index, string Name, string? UniqueId = null);
public sealed record SourceItem(string Id, string PlayerId, string Slot, int? Index, string Name, string Kind, bool TierObserved, string? UniqueId = null);
public sealed record SourceSecond(int Second, IReadOnlyDictionary<string, double> Damage, IReadOnlyDictionary<string, double> Healing,
    IReadOnlyDictionary<string, double>? WeaponDamage = null, IReadOnlyDictionary<string, double>? WeaponHealing = null,
    IReadOnlyDictionary<string, double>? ItemDamage = null, IReadOnlyDictionary<string, double>? ItemHealing = null);
public sealed record SourceSnapshot(string Id, DateTimeOffset StartedAt, double Elapsed, string Status, long Events, IReadOnlyList<SourceMember> Members,
    IReadOnlyList<SourceWeapon> Weapons, IReadOnlyList<SourceItem> Items, IReadOnlyList<SourceSecond> Timeline);
public sealed record SourceFloor(string Id, int Number, string? Map, IReadOnlyList<string> Snaps);
public sealed record SourceDungeon(string Id, string Name, DateTimeOffset StartedAt, string Status, string? Entrance, IReadOnlyList<SourceFloor> Floors);

public sealed class SessionCompactor
{
    readonly Dictionary<string, int> keys = [];
    readonly List<SourceKey> table = [];
    int keysSent;
    readonly Dictionary<string, PlayerInfo> playersSent = [];
    readonly Dictionary<string, Dictionary<int, long>> secondsSent = [];
    readonly Dictionary<string, string> snapsSent = [];
    string? dungeonsSent;
    string? statusSent;
    string? recordSent;
    // Every place of the record so far, kept across resets so a full resend carries them all.
    readonly SortedDictionary<int, string> places = [];
    int placesSent;
    long seq;
    bool reset = true;

    public void Reset() => reset = true;

    public SessionDelta? Next(string status, DateTimeOffset startedAt, string? self, SourceSnapshot record, IEnumerable<SourceSnapshot> encounters, IReadOnlyList<SourceDungeon> dungeons, string? place = null)
    {
        var full = reset;
        if (full)
        {
            playersSent.Clear(); secondsSent.Clear(); snapsSent.Clear(); dungeonsSent = null; statusSent = null; recordSent = null; keysSent = 0; placesSent = 0;
        }
        // A record started again from zero has none of its old places.
        var second = (int)record.Elapsed;
        if (places.Count > 0 && places.Keys.Last() > second) { places.Clear(); placesSent = 0; }
        if (place is { Length: > 0 } && (places.Count == 0 || places.Values.Last() != place)) places[second] = place;
        var delta = new SessionDelta { Reset = full };
        var changed = full;
        if (statusSent != status) { delta.Status = statusSent = status; delta.StartedAt = startedAt; delta.Self = self; changed = true; }

        var snaps = new List<SourceSnapshot> { record };
        snaps.AddRange(encounters.Where(snap => snap.Id != record.Id));
        foreach (var snap in snaps)
        {
            foreach (var member in snap.Members)
            {
                var player = new PlayerInfo { Name = member.Name, Weapon = member.Weapon, WeaponId = member.WeaponId, Family = member.Family, Color = member.Color };
                if (playersSent.TryGetValue(member.Id, out var sent) && Same(sent, player)) continue;
                (delta.Players ??= [])[member.Id] = playersSent[member.Id] = player;
            }
            var compact = Compact(snap, full);
            if (compact is not null) { (delta.Snaps ??= [])[snap.Id] = compact; changed = true; }
        }
        if (recordSent != record.Id) { delta.Record = recordSent = record.Id; changed = true; }
        if (places.Count > placesSent)
        {
            delta.Places = places.Skip(placesSent).ToDictionary(pair => pair.Key, pair => pair.Value);
            placesSent = places.Count;
            changed = true;
        }
        var dungeonText = string.Join("|", dungeons.Select(dungeon => $"{dungeon.Id};{dungeon.Name};{dungeon.Status};{dungeon.Entrance};" +
            string.Join(",", dungeon.Floors.Select(floor => $"{floor.Id}:{floor.Number}:{floor.Map}:{string.Join("+", floor.Snaps)}"))));
        if (dungeonsSent != dungeonText)
        {
            dungeonsSent = dungeonText;
            delta.Dungeons = [.. dungeons.Select(dungeon => new DungeonInfo
            {
                Id = dungeon.Id, Name = dungeon.Name, StartedAt = dungeon.StartedAt, Status = dungeon.Status, Entrance = dungeon.Entrance,
                Floors = [.. dungeon.Floors.Select(floor => new FloorInfo { Id = floor.Id, Number = floor.Number, Map = floor.Map, Snaps = [.. floor.Snaps] })],
            })];
            changed = true;
        }
        if (delta.Players is not null) changed = true;
        if (keysSent < table.Count)
        {
            delta.Keys = [];
            for (var index = keysSent; index < table.Count; index++) delta.Keys[index] = table[index];
            keysSent = table.Count;
        }
        if (!changed) return null;
        reset = false;
        delta.Seq = ++seq;
        return delta;
    }

    Snap? Compact(SourceSnapshot snap, bool full)
    {
        var weapons = snap.Weapons.ToDictionary(weapon => weapon.Id);
        var items = snap.Items.ToDictionary(item => item.Id);
        if (!secondsSent.TryGetValue(snap.Id, out var sent)) secondsSent[snap.Id] = sent = [];
        SortedDictionary<int, List<double[]>>? seconds = null;
        foreach (var point in snap.Timeline)
        {
            var entries = new Dictionary<int, double[]>();
            void Add(string key, Func<SourceKey> describe, double damage, double healing)
            {
                if (damage <= 0 && healing <= 0) return;
                if (!keys.TryGetValue(key, out var index)) { index = keys[key] = table.Count; table.Add(describe()); }
                if (!entries.TryGetValue(index, out var entry)) entries[index] = entry = [index, 0, 0];
                entry[1] += Math.Round(damage);
                entry[2] += Math.Round(healing);
            }
            foreach (var (player, value) in point.Damage) Add($"p|{player}", () => new SourceKey { T = "p", P = player }, value, 0);
            foreach (var (player, value) in point.Healing) Add($"p|{player}", () => new SourceKey { T = "p", P = player }, 0, value);
            void Weapon(IReadOnlyDictionary<string, double>? values, bool healing)
            {
                foreach (var (id, value) in values ?? new Dictionary<string, double>())
                    Add($"w|{id}", () => weapons.TryGetValue(id, out var weapon)
                        ? new SourceKey { T = "w", P = weapon.PlayerId, X = weapon.Index, N = weapon.Name, U = weapon.UniqueId }
                        : new SourceKey { T = "w", P = id.Split(':')[0], N = "Unknown weapon" }, healing ? 0 : value, healing ? value : 0);
            }
            void Item(IReadOnlyDictionary<string, double>? values, bool healing)
            {
                foreach (var (id, value) in values ?? new Dictionary<string, double>())
                    Add($"i|{id}", () => items.TryGetValue(id, out var item)
                        ? new SourceKey { T = "i", P = item.PlayerId, X = item.Index, S = item.Slot, K = item.Kind, N = item.Name, U = item.UniqueId, O = item.TierObserved }
                        : new SourceKey { T = "i", P = id.Split(':')[0], N = "Unknown source" }, healing ? 0 : value, healing ? value : 0);
            }
            Weapon(point.WeaponDamage, false); Weapon(point.WeaponHealing, true);
            Item(point.ItemDamage, false); Item(point.ItemHealing, true);
            var list = entries.Values.Where(entry => entry[1] > 0 || entry[2] > 0).OrderBy(entry => entry[0]).ToList();
            var hash = list.Aggregate(17L, (current, entry) => HashCode.Combine(current, entry[0], entry[1], entry[2]));
            var known = sent.TryGetValue(point.Second, out var previous);
            if (list.Count == 0 && !known) continue;
            if (known && previous == hash) continue;
            sent[point.Second] = hash;
            (seconds ??= [])[point.Second] = list;
        }
        var members = snap.Members.Select(member => member.Id).ToList();
        var membership = snap.Members.Where(member => member.Membership != "active").ToDictionary(member => member.Id, member => member.Membership);
        var header = $"{snap.Elapsed:0.0};{snap.Status};{snap.Events};{string.Join(",", members)};{string.Join(",", membership.Select(pair => pair.Key + "=" + pair.Value))}";
        var headerChanged = !snapsSent.TryGetValue(snap.Id, out var previousHeader) || previousHeader != header;
        if (!full && seconds is null && !headerChanged) return null;
        snapsSent[snap.Id] = header;
        return new Snap
        {
            Id = snap.Id, StartedAt = snap.StartedAt, Elapsed = snap.Elapsed, Status = snap.Status, Events = snap.Events,
            Members = full || headerChanged ? members : null, Membership = full || headerChanged ? membership : null, Sec = seconds,
        };
    }

    static bool Same(PlayerInfo a, PlayerInfo b) => a.Name == b.Name && a.Weapon == b.Weapon && a.WeaponId == b.WeaponId && a.Family == b.Family && a.Color == b.Color;
}
