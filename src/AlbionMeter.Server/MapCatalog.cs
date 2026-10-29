using System.Text.Json;
using AlbionMeter.Core;

namespace AlbionMeter.Server;

public sealed class MapCatalog
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly object _gate = new();
    private MapCatalogData? _data;
    private readonly string _path;
    public MapCatalog(IConfiguration configuration)
    {
        var directory = configuration["Storage:Directory"] ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "AlbionMeter");
        _path = Path.Combine(directory, "assets", "maps.json");
        var bundled = Path.Combine(AppContext.BaseDirectory, "assets", "maps.json");
        var path = File.Exists(_path) ? _path : bundled;
        if (File.Exists(path)) _data = JsonSerializer.Deserialize<MapCatalogData>(File.ReadAllText(path), JsonOptions);
    }
    public MapCatalogData? Data { get { lock (_gate) return _data; } }
    public MapDefinition? Find(string id) => Data?.Maps.FirstOrDefault(map => map.Id == id);
    public string WeaponName(int? index)
    {
        if (index is null) return "Weapon not observed";
        if (index == 0) return "Unarmed";
        var weapon = Data?.IndexedWeapons?.FirstOrDefault(value => value.Index == index);
        return weapon is null ? $"Item #{index}" : $"{weapon.Name} · T{weapon.Tier}.{weapon.Enchantment}";
    }
    private static readonly Dictionary<string, string> FamilyNames = new()
    {
        ["arcanestaff"] = "Arcane Staff", ["axe"] = "Axe", ["bow"] = "Bow", ["crossbow"] = "Crossbow", ["cursestaff"] = "Cursed Staff",
        ["dagger"] = "Dagger", ["firestaff"] = "Fire Staff", ["froststaff"] = "Frost Staff", ["hammer"] = "Hammer", ["holystaff"] = "Holy Staff",
        ["knuckles"] = "War Gloves", ["mace"] = "Mace", ["naturestaff"] = "Nature Staff", ["quarterstaff"] = "Quarterstaff",
        ["shapeshifterstaff"] = "Shapeshifter Staff", ["spear"] = "Spear", ["sword"] = "Sword",
    };
    public string? WeaponFamily(int? index)
    {
        var family = index is > 0 ? Data?.IndexedWeapons?.FirstOrDefault(value => value.Index == index)?.Family : null;
        return family is null or "unknown" ? null : FamilyNames.GetValueOrDefault(family, family);
    }
    public string ResolveName(string id) => Find(id)?.Name ?? id;
    public void Save(MapCatalogData data)
    {
        lock (_gate)
        {
            Directory.CreateDirectory(Path.GetDirectoryName(_path)!);
            File.WriteAllText(_path + ".tmp", JsonSerializer.Serialize(data, JsonOptions));
            File.Move(_path + ".tmp", _path, true);
            _data = data;
        }
    }
}
