using System.Text.Json;

namespace AlbionMeter.Core;

public sealed record CombatItemDefinition(int Index, string Id, string Name, string Slot, int? Tier, int Enchantment, string[]? PassiveSpells = null, string[]? Spells = null);
public sealed record CombatSpellDefinition(int Index, string Id, string[] Effects);
public sealed record CombatSourceData(string Version, CombatItemDefinition[] Items, CombatSpellDefinition[] Spells);
public sealed record ItemAttribution(int? Index, string Slot, string Kind = "unknown", bool Exact = true);

public sealed class CombatSourceCatalog
{
    private readonly Dictionary<int, CombatItemDefinition> _items;
    private readonly Dictionary<int, CombatSpellDefinition> _spells;
    private readonly HashSet<string> _itemIds;
    private readonly Dictionary<string, HashSet<string>> _descendants = [];
    public CombatSourceCatalog(CombatSourceData data)
    {
        _items = data.Items.ToDictionary(item => item.Index);
        _itemIds = data.Items.Select(item => item.Id).ToHashSet(StringComparer.Ordinal);
        _spells = data.Spells.ToDictionary(spell => spell.Index);
        var byName = data.Spells.GroupBy(spell => spell.Id).ToDictionary(group => group.Key, group => group.First());
        foreach (var spell in byName.Values)
        {
            var seen = new HashSet<string>();
            var pending = new Stack<string>(); pending.Push(spell.Id);
            while (pending.TryPop(out var id) && seen.Count < 512)
            {
                if (!seen.Add(id) || !byName.TryGetValue(id, out var current)) continue;
                foreach (var effect in current.Effects) pending.Push(effect);
            }
            _descendants[spell.Id] = seen;
        }
    }
    public static CombatSourceCatalog? Load(string path) => File.Exists(path)
        ? new(JsonSerializer.Deserialize<CombatSourceData>(File.ReadAllText(path), new JsonSerializerOptions(JsonSerializerDefaults.Web))!) : null;
    public CombatItemDefinition? FindItem(int index) => _items.GetValueOrDefault(index);
    public bool Produces(int activeSpell, int effectSpell) => activeSpell == effectSpell
        || _spells.TryGetValue(activeSpell, out var root) && _spells.TryGetValue(effectSpell, out var effect) && _descendants.TryGetValue(root.Id, out var descendants) && descendants.Contains(effect.Id);
    public bool ContainsItem(string id) => _itemIds.Contains(id);
    private static readonly string[] TierWords = ["Beginner's ", "Novice's ", "Journeyman's ", "Adept's ", "Expert's ", "Master's ", "Grandmaster's ", "Elder's "];
    public string FamilyName(int? index) => index is { } known && _items.TryGetValue(known, out var item)
        ? $"{TierWords.Aggregate(item.Name, (name, word) => name.StartsWith(word, StringComparison.Ordinal) ? name[word.Length..] : name)} · tier not observed" : ItemName(index);
    private static string FamilyOf(string id) => System.Text.RegularExpressions.Regex.Replace(id.Split('@')[0], "^T[0-9]_", "");
    private readonly Dictionary<int, ItemAttribution?> _familyByEffect = [];
    public ItemAttribution? UniqueFamily(int effectIndex)
    {
        if (_familyByEffect.TryGetValue(effectIndex, out var cached)) return cached;
        ItemAttribution? result = null;
        if (_spells.TryGetValue(effectIndex, out var effect))
        {
            var families = new Dictionary<string, ItemAttribution>();
            var producers = new List<int>();
            foreach (var item in _items.Values)
            {
                if (item.Spells is null || item.Slot is "bag" or "mount") continue;
                var producing = item.Spells.Where(skill => _descendants.TryGetValue(skill, out var effects) && effects.Contains(effect.Id)).ToArray();
                if (producing.Length == 0) continue;
                producers.Add(item.Index);
                families.TryAdd(FamilyOf(item.Id), new(FamilyRepresentative(item.Index), item.Slot, producing.All(skill => item.PassiveSpells?.Contains(skill) == true) ? "passive" : "ability", false));
                if (families.Count > 1) break;
            }
            if (families.Count == 1) result = producers is [var only] ? families.Values.Single() with { Index = only, Exact = true } : families.Values.Single();
        }
        if (_familyByEffect.Count < 16384) _familyByEffect[effectIndex] = result;
        return result;
    }
    private Dictionary<string, int>? _representatives;
    public int FamilyRepresentative(int item)
    {
        _representatives ??= _items.Values.GroupBy(definition => FamilyOf(definition.Id)).ToDictionary(group => group.Key, group => group.Min(definition => definition.Index));
        return _items.TryGetValue(item, out var definition) ? _representatives[FamilyOf(definition.Id)] : item;
    }
    public bool SameFamily(int item, int other) => _items.TryGetValue(item, out var first) && _items.TryGetValue(other, out var second) && FamilyOf(first.Id) == FamilyOf(second.Id);
    public bool CouldProduce(int effectIndex, int?[] equipment)
    {
        if (!_spells.TryGetValue(effectIndex, out var effect)) return true;
        for (var slot = 0; slot < Math.Min(equipment.Length, 10); slot++)
        {
            if (slot is 5 or 7 || equipment[slot] is not > 0) continue;
            if (!_items.TryGetValue(equipment[slot]!.Value, out var worn)) return true;
            if ((worn.Spells ?? worn.PassiveSpells ?? []).Any(skill => _descendants.TryGetValue(skill, out var effects) && effects.Contains(effect.Id))) return true;
        }
        return false;
    }
    private Dictionary<string, string>? _idsByName;
    public string? ItemIdByName(string name)
    {
        _idsByName ??= _items.Values.GroupBy(item => $"{item.Name} · T{item.Tier}.{item.Enchantment}").ToDictionary(group => group.Key, group => group.First().Id, StringComparer.Ordinal);
        return _idsByName.GetValueOrDefault(name);
    }
    public string ItemName(int? index) => index is null ? "Unknown source" : index == 0 ? "Unarmed" : _items.TryGetValue(index.Value, out var item)
        ? $"{item.Name} · T{item.Tier}.{item.Enchantment}" : $"Item #{index}";
    public ItemAttribution Resolve(int? spellIndex, int? origin, bool damage, int?[] equipment, int?[] activeSpells)
    {
        if (damage && spellIndex == -1 && origin is 0 or 1) return new(equipment.Length > 0 ? equipment[0] : null, "mainhand", "autoAttack");
        if (spellIndex is null || !_spells.TryGetValue(spellIndex.Value, out var effect)) return new(null, "unknown");
        var matches = new HashSet<int>();
        var slots = new[] { 0, 0, 0, 3, 2, 4, -1, -1, -1, -1, -1, -1, 8, 9 };
        for (var index = 0; index < Math.Min(slots.Length, activeSpells.Length); index++)
        {
            var slot = slots[index];
            if (slot < 0 || slot >= equipment.Length || equipment[slot] is not > 0 || activeSpells[index] is not { } active) continue;
            if (_spells.TryGetValue(active, out var root) && _descendants.TryGetValue(root.Id, out var descendants) && descendants.Contains(effect.Id)) matches.Add(slot);
        }
        if (equipment.Length > 6 && equipment[6] is { } capeIndex && _items.TryGetValue(capeIndex, out var cape) && cape.Slot == "cape")
        {
            foreach (var passive in cape.PassiveSpells ?? [])
                if (_descendants.TryGetValue(passive, out var effects) && effects.Contains(effect.Id)) matches.Add(6);
        }
        var allowed = new HashSet<int>();
        var onlyPassive = true;
        var unprovable = false;
        for (var slot = 0; slot < Math.Min(equipment.Length, 10); slot++)
        {
            if (slot is 5 or 7 || equipment[slot] is not > 0 || matches.Count > 1 && !matches.Contains(slot)) continue;
            if (!_items.TryGetValue(equipment[slot]!.Value, out var worn)) { unprovable = true; continue; }
            var producing = (worn.Spells ?? worn.PassiveSpells ?? []).Where(skill => _descendants.TryGetValue(skill, out var effects) && effects.Contains(effect.Id)).ToArray();
            if (producing.Length == 0) continue;
            allowed.Add(slot);
            onlyPassive &= producing.All(skill => worn.PassiveSpells?.Contains(skill) == true);
        }
        if (matches.Count != 1 && allowed.Count == 1 && !unprovable) return Attribution(equipment, allowed.Single(), onlyPassive);
        if (matches.Count != 1) return new(null, "unknown");
        var match = matches.Single();
        return Attribution(equipment, match, match == 6);
    }
    private static ItemAttribution Attribution(int?[] equipment, int slot, bool passive) =>
        new(equipment[slot], slot switch { 0 => "mainhand", 1 => "offhand", 2 => "head", 3 => "chest", 4 => "shoes", 6 => "cape", 8 => "potion", 9 => "food", _ => "unknown" }, passive ? "passive" : "ability");
}
