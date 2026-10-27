using System.IO.Compression;
using System.Security.Cryptography;
using System.Text.RegularExpressions;
using System.Xml;
using System.Xml.Linq;

namespace AlbionMeter.Core;

public sealed record MapDefinition(string Id, string Name, string Type, int? Tier, string File, bool Enabled, string[] Exits);
public sealed record WeaponDefinition(string Id, string Name, string Family, int? Tier, string Slot);
public sealed record IndexedWeaponDefinition(int Index, string Id, string Name, string Family, int? Tier, int Enchantment);
public sealed record MapCatalogData(int SchemaVersion, string Source, string GameVersion, DateTimeOffset ImportedAt,
    string SourceFile, string Sha256, int MapCount, MapDefinition[] Maps, int WeaponCount = 0, WeaponDefinition[]? Weapons = null, IndexedWeaponDefinition[]? IndexedWeapons = null);

public static class GameAssetImporter
{
    public static MapCatalogData Import(byte[] world, byte[]? items, byte[]? localization, string version, string sourceFile)
    {
        using var worldReader = Open(world);
        var document = XDocument.Load(worldReader);
        var clusters = document.Root?.Element("clusters")?.Elements("cluster")
            ?? throw new InvalidDataException("World clusters were not found.");
        var maps = clusters.Select(cluster =>
        {
            string Get(string key) => (string?)cluster.Attribute(key) ?? string.Empty;
            var tier = Regex.Match(Get("file"), "_T([1-8])_");
            return new MapDefinition(Get("id"), Get("displayname") is { Length: > 0 } name ? name : Get("id"),
                Get("type"), tier.Success ? int.Parse(tier.Groups[1].Value) : null, Get("file"), Get("enabled") != "false",
                cluster.Element("exits")?.Elements("exit").Select(exit => (string?)exit.Attribute("targetid"))
                    .Where(target => !string.IsNullOrWhiteSpace(target)).Select(target => target!.Split('@')[^1]).Distinct().ToArray() ?? []);
        }).ToArray();
        if (maps.Length == 0 || maps.Any(map => string.IsNullOrWhiteSpace(map.Id)) || maps.Select(map => map.Id).Distinct().Count() != maps.Length)
            throw new InvalidDataException("World map identifiers are missing or duplicated.");
        var weapons = items is null ? [] : ReadWeapons(items, localization);
        return new(1, "local-game-installation", version, DateTimeOffset.UtcNow, sourceFile,
            Convert.ToHexStringLower(SHA256.HashData(world)), maps.Length, maps, weapons.Length, weapons, items is null ? [] : ReadIndexedWeapons(items, weapons));
    }

    private static readonly HashSet<string> TriggerAttributes = ["spell", "effect", "endeffect", "collisioneffect", "landscapecollisioneffect", "spellonhit", "spellonend", "spellonexpire",
        "spellontrigger", "applyspellwhentargetsinarea", "applyspellwhennotargetsinarea"];

    public static CombatSourceData ImportCombat(byte[] items, byte[] spells, byte[]? localization, string version, byte[]? transformations = null)
    {
        using var itemReader = Open(items);
        using var spellReader = Open(spells);
        var itemElements = XDocument.Load(itemReader).Root!.Elements().ToArray();
        var ids = itemElements.Where(item => item.Attribute("slottype") is not null).Select(item => (string?)item.Attribute("uniquename") ?? "").ToHashSet();
        var names = localization is null ? new Dictionary<string, string>() : ReadNames(localization, ids);
        var spellDocument = XDocument.Load(spellReader);
        var spellNames = spellDocument.Root!.Elements().Select(spell => (string?)spell.Attribute("uniquename")).OfType<string>().ToHashSet();
        var passives = spellDocument.Root!.Elements("passivespell").Select(spell => (string?)spell.Attribute("uniquename")).OfType<string>().ToHashSet();
        var forms = new Dictionary<string, string[]>();
        if (transformations is not null)
        {
            using var transformationReader = Open(transformations);
            foreach (var form in XDocument.Load(transformationReader).Root!.Elements("transformation"))
                if ((string?)form.Attribute("uniquename") is { } formId)
                    forms[formId] = form.Descendants().Where(element => element.Name.LocalName is "spell" or "passivespell").Select(element => (string?)element.Attribute("uniquename")).OfType<string>().Distinct().ToArray();
        }
        var byId = itemElements.Where(item => item.Attribute("uniquename") is not null).GroupBy(item => (string)item.Attribute("uniquename")!).ToDictionary(group => group.Key, group => group.First());
        string[] Pool(XElement item, int depth = 0)
        {
            var list = item.Element("craftingspelllist");
            if (list is null) return [];
            var shared = (string?)list.Attribute("reference") is { } reference && depth < 8 && byId.TryGetValue(reference, out var source) ? Pool(source, depth + 1) : [];
            return shared.Concat(list.Elements("craftspell").Select(spell => (string?)spell.Attribute("uniquename")).OfType<string>()).Distinct().ToArray();
        }
        var definitions = new List<CombatItemDefinition>();
        var index = 1;
        foreach (var item in itemElements)
        {
            var id = (string?)item.Attribute("uniquename");
            if (string.IsNullOrEmpty(id)) continue;
            var crafted = item.Attribute("slottype") is null ? [] : Pool(item);
            void Add(int level, string? consumed)
            {
                var pool = consumed is not null && spellNames.Contains(consumed) ? crafted.Append(consumed).Distinct().ToArray() : crafted;
                if (item.Attribute("slottype") is { } slot) definitions.Add(new(index, level == 0 ? id : $"{id}@{level}", names.GetValueOrDefault(id, id), slot.Value, int.TryParse((string?)item.Attribute("tier"), out var tier) ? tier : null, level,
                    slot.Value == "cape" ? item.Descendants("craftspell").Select(spell => (string?)spell.Attribute("uniquename")).OfType<string>().Distinct().ToArray() : pool.Where(passives.Contains).ToArray(),
                    pool.Length == 0 ? null : pool));
                index++;
            }
            Add(int.TryParse((string?)item.Attribute("enchantmentlevel"), out var level) ? level : 0, (string?)item.Attribute("consumespell"));
            foreach (var enchantment in item.Element("enchantments")?.Elements() ?? []) Add(int.Parse(enchantment.Attribute("enchantmentlevel")!.Value), (string?)enchantment.Attribute("consumespell"));
        }
        var spellDefinitions = new List<CombatSpellDefinition>();
        index = 0;
        foreach (var spell in spellDocument.Root!.Elements())
        {
            if (spell.Name == "colortag") continue;
            if (spell.Name.LocalName is not ("activespell" or "passivespell" or "togglespell")) throw new InvalidDataException("Unsupported spell definition.");
            var id = spell.Attribute("uniquename")?.Value ?? throw new InvalidDataException("Missing spell identifier.");
            var effects = spell.Descendants().Where(element => !element.Name.LocalName.StartsWith("If", StringComparison.Ordinal) && !element.Name.LocalName.StartsWith("remove", StringComparison.Ordinal))
                .Attributes().Where(attribute => TriggerAttributes.Contains(attribute.Name.LocalName) && spellNames.Contains(attribute.Value) || attribute.Parent!.Name.LocalName == "triggerspell" && attribute.Name.LocalName == "name").Select(attribute => attribute.Value)
                .Concat(spell.Descendants("transformation").Select(form => (string?)form.Attribute("type")).OfType<string>().SelectMany(form => forms.GetValueOrDefault(form) ?? []))
                .Distinct().ToArray();
            spellDefinitions.Add(new(index++, id, effects));
            if (spell.Name == "activespell" && spell.Element("channelingspell") is not null) spellDefinitions.Add(new(index++, id, effects));
        }
        return new(version, definitions.ToArray(), spellDefinitions.ToArray());
    }

    private static IndexedWeaponDefinition[] ReadIndexedWeapons(byte[] items, WeaponDefinition[] weapons)
    {
        using var reader = Open(items);
        var document = XDocument.Load(reader);
        var lookup = weapons.ToDictionary(weapon => weapon.Id);
        var result = new List<IndexedWeaponDefinition>();
        var index = 1;
        foreach (var item in document.Root!.Elements())
        {
            var id = (string?)item.Attribute("uniquename");
            if (string.IsNullOrEmpty(id)) continue;
            void Add(int enchantment)
            {
                if (lookup.TryGetValue(id, out var weapon)) result.Add(new(index, enchantment == 0 ? id : $"{id}@{enchantment}", weapon.Name, weapon.Family, weapon.Tier, enchantment));
                index++;
            }
            Add(int.TryParse((string?)item.Attribute("enchantmentlevel"), out var level) ? level : 0);
            foreach (var enchantment in item.Element("enchantments")?.Elements() ?? [])
                Add(int.Parse(enchantment.Attribute("enchantmentlevel")!.Value));
        }
        return result.ToArray();
    }

    private static WeaponDefinition[] ReadWeapons(byte[] items, byte[]? localization)
    {
        using var reader = Open(items);
        var document = XDocument.Load(reader);
        if (document.Root?.Name != "items") throw new InvalidDataException("The selected items file is invalid.");
        var definitions = document.Root.Elements().Where(item => item.Name.LocalName is "weapon" or "transformationweapon"
            && (string?)item.Attribute("shopcategory") == "weapons").ToArray();
        var ids = definitions.Select(item => (string?)item.Attribute("uniquename") ?? string.Empty).ToHashSet();
        var names = localization is null ? new Dictionary<string, string>() : ReadNames(localization, ids);
        return definitions.Select(item =>
        {
            var id = (string?)item.Attribute("uniquename") ?? throw new InvalidDataException("Missing weapon identifier.");
            var family = (string?)item.Attribute("shopsubcategory1") ?? "unknown";
            return new WeaponDefinition(id, names.GetValueOrDefault(id, id), family,
                int.TryParse((string?)item.Attribute("tier"), out var tier) ? tier : null,
                (string?)item.Attribute("slottype") ?? "unknown");
        }).ToArray();
    }

    private static Dictionary<string, string> ReadNames(byte[] localization, HashSet<string> ids)
    {
        using var reader = Open(localization);
        Dictionary<string, string> names = [];
        while (reader.Read())
        {
            if (reader.NodeType != XmlNodeType.Element || reader.Name != "tu") continue;
            var key = reader.GetAttribute("tuid");
            if (key is null || !key.StartsWith("@ITEMS_", StringComparison.Ordinal) || !ids.Contains(key[7..])) continue;
            using var subtree = reader.ReadSubtree();
            var unit = XElement.Load(subtree);
            var value = unit.Elements("tuv").FirstOrDefault(element =>
                string.Equals((string?)element.Attribute(XNamespace.Xml + "lang"), "EN-US", StringComparison.OrdinalIgnoreCase))?.Element("seg")?.Value;
            if (!string.IsNullOrWhiteSpace(value)) names[key[7..]] = value;
        }
        return names;
    }

    private static XmlReader Open(byte[] encrypted)
    {
        if (encrypted.Length is 0 or > 20_000_000) throw new InvalidDataException("Game files must be between 1 byte and 20 MB.");
        using var cipher = DES.Create();
        cipher.Key = Convert.FromHexString("30EF724742F20432");
        cipher.IV = Convert.FromHexString("0EA6DC89DBEDDC4F");
        var compressed = new MemoryStream(cipher.DecryptCbc(encrypted, cipher.IV, PaddingMode.PKCS7));
        var gzip = new GZipStream(compressed, CompressionMode.Decompress);
        return XmlReader.Create(gzip, new XmlReaderSettings
        {
            CloseInput = true, DtdProcessing = DtdProcessing.Prohibit, XmlResolver = null, MaxCharactersInDocument = 120_000_000
        });
    }
}
