using System.Text.Json;
using AlbionMeter.Core;

if (args.Length != 2) throw new ArgumentException("Usage: AlbionMeter.MapImporter <game-directory> <asset-directory>");
var gameDirectory = Path.GetFullPath(args[0]);
var outputDirectory = Path.GetFullPath(args[1]);
var gameData = Path.Combine(gameDirectory, "Albion-Online_Data", "StreamingAssets", "GameData");
var source = Path.Combine(gameData, "cluster", "world.bin");
var versionFile = Path.Combine(gameDirectory, "version.txt");
byte[]? Optional(string name) => File.Exists(Path.Combine(gameData, name)) ? File.ReadAllBytes(Path.Combine(gameData, name)) : null;
var data = GameAssetImporter.Import(File.ReadAllBytes(source), Optional("items.bin"), Optional("localization.bin"),
    File.Exists(versionFile) ? File.ReadAllText(versionFile).Trim() : "unknown", source);
Directory.CreateDirectory(outputDirectory);
Directory.CreateDirectory(Path.Combine(outputDirectory, "source"));
var target = Path.Combine(outputDirectory, "maps.json");
File.WriteAllText(target + ".tmp", JsonSerializer.Serialize(data, new JsonSerializerOptions(JsonSerializerDefaults.Web) { WriteIndented = true }));
File.Move(target + ".tmp", target, true);
File.Copy(source, Path.Combine(outputDirectory, "source", "world.bin"), true);
Console.WriteLine($"Imported {data.MapCount} maps and {data.WeaponCount} weapon definitions from {data.GameVersion}.");

if (Optional("items.bin") is { } combatItems && Optional("spells.bin") is { } combatSpells)
{
    var combat = GameAssetImporter.ImportCombat(combatItems, combatSpells, Optional("localization.bin"), data.GameVersion);
    File.WriteAllText(Path.Combine(outputDirectory, "combat.json"), JsonSerializer.Serialize(combat, new JsonSerializerOptions(JsonSerializerDefaults.Web)));
    Console.WriteLine($"Imported {combat.Items.Length} indexed equipment entries and {combat.Spells.Length} spell entries.");
}
