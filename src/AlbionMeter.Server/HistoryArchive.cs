using System.IO.Compression;
using System.Text.Json;
using AlbionMeter.Core;

namespace AlbionMeter.Server;

public sealed record HistoryArchive(string Format, int Version, CombatSnapshot[] Encounters, DungeonRunSnapshot[] Dungeons);
public sealed record HistoryImportResult(int ImportedEncounters, int ImportedDungeons, int SkippedEncounters, int SkippedDungeons);

public static class HistoryArchiveFile
{
    public const int MaxUploadBytes = 40_000_000;
    private const int MaxJsonBytes = 128_000_000;
    private static readonly JsonSerializerOptions Options = new(JsonSerializerDefaults.Web) { MaxDepth = 64 };

    public static byte[] Write(HistoryArchive archive)
    {
        Validate(archive);
        var json = JsonSerializer.SerializeToUtf8Bytes(archive, Options);
        if (json.Length > MaxJsonBytes) throw new InvalidDataException("This history is too large. Export individual records instead.");
        using var output = new MemoryStream();
        using (var zip = new ZipArchive(output, ZipArchiveMode.Create, true))
        using (var stream = zip.CreateEntry("history.json", CompressionLevel.Optimal).Open()) stream.Write(json);
        if (output.Length > MaxUploadBytes) throw new InvalidDataException("This history is too large. Export individual records instead.");
        return output.ToArray();
    }

    public static HistoryArchive Read(byte[] bytes)
    {
        if (bytes.Length is 0 or > MaxUploadBytes) throw new InvalidDataException("Choose a history file smaller than 40 MB.");
        try
        {
            if (bytes.Length >= 2 && bytes[0] == 80 && bytes[1] == 75)
            {
                using var input = new MemoryStream(bytes);
                using var zip = new ZipArchive(input, ZipArchiveMode.Read);
                if (zip.Entries.Count != 1 || zip.Entries[0].FullName != "history.json" || zip.Entries[0].Length > MaxJsonBytes)
                    throw new InvalidDataException("Invalid history archive.");
                using var stream = zip.Entries[0].Open();
                using var json = new MemoryStream();
                var buffer = new byte[81920];
                int read;
                while ((read = stream.Read(buffer)) > 0)
                {
                    if (json.Length + read > MaxJsonBytes) throw new InvalidDataException("History exceeds the import limit.");
                    json.Write(buffer, 0, read);
                }
                bytes = json.ToArray();
            }
            var content = bytes.AsSpan();
            if (content.StartsWith(new byte[] { 239, 187, 191 })) content = content[3..];
            var archive = JsonSerializer.Deserialize<HistoryArchive>(content, Options) ?? throw new InvalidDataException("Empty history file.");
            Validate(archive);
            return archive;
        }
        catch (JsonException) { throw new InvalidDataException("The file is not a supported Albion Meter history."); }
    }

    public static void Validate(HistoryArchive archive)
    {
        void Require([System.Diagnostics.CodeAnalysis.DoesNotReturnIf(false)] bool valid) { if (!valid) throw new InvalidDataException("The history contains invalid or unsupported records."); }
        void Text(string? value) => Require(!string.IsNullOrWhiteSpace(value) && value.Length <= 4096 && !value.Any(char.IsControl));
        void Number(double value) => Require(double.IsFinite(value) && value >= 0 && value <= 1e15);
        long cells = 0;
        void Snapshot(CombatSnapshot? value)
        {
            Require(value is not null);
            var snapshot = value!;
            Text(snapshot.Id); Text(snapshot.Mode);
            Require(snapshot.Full && snapshot.StartedAt != default && snapshot.EventCount >= 0 && snapshot.Sequence >= 0);
            Require(snapshot.Status is "idle" or "running" or "paused" or "completed");
            Number(snapshot.ElapsedSeconds); Number(snapshot.TotalDamage); Number(snapshot.TotalHealing);
            Require(snapshot.Members is not null && snapshot.Timeline is not null);
            var members = snapshot.Members!.Concat(snapshot.FormerMembers ?? []).ToArray();
            Require(members.Length <= 100 && members.Select(member => member?.Id).Distinct().Count() == members.Length);
            foreach (var member in members)
            {
                Require(member is not null);
                Text(member!.Id); Text(member.Name); Text(member.Weapon); Text(member.Color);
                Number(member.Damage); Number(member.Healing); Number(member.EncounterDps); Number(member.EncounterHps);
                Number(member.RollingDps); Number(member.RollingHps); Number(member.DamageShare); Number(member.HealingShare);
            }
            var seconds = new HashSet<int>();
            foreach (var point in snapshot.Timeline!)
            {
                Require(point is not null);
                Require(point!.Second >= 0 && point.Second <= Math.Ceiling(snapshot.ElapsedSeconds) + 1 && seconds.Add(point.Second));
                Require(point.Damage is not null && point.Healing is not null);
                foreach (var map in new[] { point.Damage, point.Healing, point.WeaponDamage, point.WeaponHealing, point.ItemDamage, point.ItemHealing })
                {
                    if (map is null) continue;
                    cells += map.Count;
                    Require(cells <= 10_000_000);
                    foreach (var (key, amount) in map) { Text(key); Number(amount); }
                }
            }
            foreach (var item in snapshot.ItemContributions ?? [])
            {
                Require(item is not null); Text(item!.Id); Text(item.PlayerId); Text(item.PlayerName); Text(item.Item); Text(item.Slot); Number(item.Damage); Number(item.Healing);
            }
            foreach (var weapon in snapshot.WeaponContributions ?? [])
            {
                Require(weapon is not null); Text(weapon!.Id); Text(weapon.PlayerId); Text(weapon.PlayerName); Text(weapon.Weapon); Number(weapon.Damage); Number(weapon.Healing);
            }
        }
        Require(archive.Format == "albion-meter-history" && archive.Version == 1);
        Require(archive.Encounters is not null && archive.Dungeons is not null);
        Require(archive.Encounters!.Length <= EncounterStore.RetainedEncounters && archive.Dungeons!.Length <= 100);
        Require(archive.Encounters.Select(item => item?.Id).Distinct().Count() == archive.Encounters.Length);
        Require(archive.Dungeons.Select(item => item?.Id).Distinct().Count() == archive.Dungeons.Length);
        foreach (var encounter in archive.Encounters) Snapshot(encounter);
        foreach (var run in archive.Dungeons)
        {
            Require(run is not null); Text(run!.Id); Text(run.Name); Text(run.EntranceMapId); Text(run.EntranceMapName); Text(run.Mode);
            Require(run.StartedAt != default && run.Floors is not null && run.Floors.Length <= 100);
            Snapshot(run.Totals);
            var floors = new HashSet<string>();
            var numbers = new HashSet<int>();
            var encounters = new HashSet<string>();
            foreach (var floor in run.Floors!)
            {
                Require(floor is not null); Text(floor!.Id); Text(floor.MapId); Text(floor.MapName);
                Require(floor.Number is > 0 and <= 100 && floors.Add(floor.Id) && numbers.Add(floor.Number) && floor.Encounters is not null);
                Snapshot(floor.Totals);
                foreach (var encounter in floor.Encounters!)
                {
                    Snapshot(encounter);
                    Require(encounters.Add(encounter.Id) && encounters.Count <= 1000);
                }
            }
        }
    }
}
