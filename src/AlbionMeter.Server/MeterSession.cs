using AlbionMeter.Core;

namespace AlbionMeter.Server;

public sealed class MeterSession(EncounterStore store, MapCatalog? catalog = null, CaptureService? capture = null, CombatSourceCatalog? sources = null)
{
    private readonly object _gate = new();
    private readonly CombatEngine _empty = new([]);
    private double _lastSaved;
    private string? _lastLiveId;
    private long _lastLiveSequence = -1;
    private string? _lastLiveStatus;
    private CombatSnapshot? Live()
    {
        var status = capture?.Status;
        return ResolveLive(status);
    }
    private CombatSnapshot? ResolveLive(CaptureSessionStatus? status)
    {
        if (status?.Report?.Combat is not { } combat) return null;
        var rosterConfirmed = status.Report?.Party?.IsConfirmedSince(status.Report.FirstTimestampMicroseconds) == true;
        PlayerStats Resolve(PlayerStats player) => player with { Membership = player.Id == status.Report?.Character?.CharacterId.ToString() || rosterConfirmed ? "active" : "unconfirmed", Weapon = (Guid.TryParse(player.Id, out var playerId) ? catalog?.WeaponName(status.Report?.Weapons?.GetValueOrDefault(playerId)) : null) ?? (player.Weapon.StartsWith("Item #", StringComparison.Ordinal) && int.TryParse(player.Weapon[6..], out var index) ? catalog?.WeaponName(index) : null) ?? player.Weapon };
        return combat with { Mode = status.Source == "replay" ? "replay" : "live", Status = status.Report?.RecordState ?? (status.Report?.RecordPaused == true ? "paused" : status.State is "running" or "starting" ? "running" : "completed"), Members = combat.Members.Select(Resolve).ToArray(), FormerMembers = combat.FormerMembers?.Select(player => Resolve(player) with { Membership = "left" }).ToArray(), ItemContributions = combat.ItemContributions?.Select(item => item with { Item = (item.TierObserved ? sources?.ItemName(item.ItemIndex) : sources?.FamilyName(item.ItemIndex)) ?? item.Item }).ToArray(), WeaponContributions = combat.WeaponContributions?.Select(weapon => weapon with { Weapon = catalog?.WeaponName(weapon.WeaponIndex) ?? weapon.Weapon }).ToArray() };
    }
    private CombatSnapshot Current(bool full) => Live() ?? (_empty.Snapshot(full, "idle") with { Mode = "unavailable", Location = null, PartyId = null });

    private DateTimeOffset _lastDungeonSave;
    private readonly Dictionary<string, long> _savedDungeonEncounters = [];
    private DungeonRunSnapshot[] LiveDungeons()
    {
        var status = capture?.Status;
        if (status?.Source != "live-capture") return [];
        DungeonLocation? ResolveLocation(DungeonLocation? location) => location is null ? null : location with
        {
            EntranceMapName = catalog?.ResolveName(location.EntranceMapId) ?? location.EntranceMapName,
            MapName = catalog?.Find(location.MapId)?.Name ?? location.MapName
        };
        CombatSnapshot ResolveSnapshot(CombatSnapshot snapshot)
        {
            PlayerStats ResolvePlayer(PlayerStats player) => player with
            {
                Weapon = player.Weapon.StartsWith("Item #", StringComparison.Ordinal) && int.TryParse(player.Weapon[6..], out var index)
                    ? catalog?.WeaponName(index) ?? player.Weapon : player.Weapon
            };
            return snapshot with
            {
                Members = snapshot.Members.Select(ResolvePlayer).ToArray(), FormerMembers = snapshot.FormerMembers?.Select(ResolvePlayer).ToArray(),
                Location = ResolveLocation(snapshot.Location),
                WeaponContributions = snapshot.WeaponContributions?.Select(item => item with { Weapon = catalog?.WeaponName(item.WeaponIndex) ?? item.Weapon }).ToArray(),
                ItemContributions = snapshot.ItemContributions?.Select(item => item with { Item = (item.TierObserved ? sources?.ItemName(item.ItemIndex) : sources?.FamilyName(item.ItemIndex)) ?? item.Item }).ToArray()
            };
        }
        return (status.Report?.Dungeons ?? []).Select(run => run with
        {
            EntranceMapName = catalog?.ResolveName(run.EntranceMapId) ?? run.EntranceMapName,
            Status = run.Status == "running" && status.State is not ("running" or "starting") ? "paused" : run.Status,
            Totals = ResolveSnapshot(run.Totals) with { Location = null },
            Floors = run.Floors.Select(floor => floor with
            {
                Totals = ResolveSnapshot(floor.Totals), Encounters = floor.Encounters.Select(ResolveSnapshot).ToArray()
            }).ToArray()
        }).ToArray();
    }
    public DungeonRunSnapshot[] Dungeons()
    {
        lock (_gate) return LiveDungeons();
    }

    private void SaveLiveDungeons(bool force = false)
    {
        if (!force && DateTimeOffset.UtcNow - _lastDungeonSave < TimeSpan.FromSeconds(5)) return;
        foreach (var run in LiveDungeons())
        {
            store.SaveDungeon(run, allowEmpty: true);
            foreach (var encounter in run.Floors.SelectMany(floor => floor.Encounters))
            {
                if (_savedDungeonEncounters.GetValueOrDefault(encounter.Id, -1) == encounter.Sequence) continue;
                store.Save(encounter);
                _savedDungeonEncounters[encounter.Id] = encounter.Sequence;
            }
        }
        _lastDungeonSave = DateTimeOffset.UtcNow;
    }

    private void SaveCurrent()
    {
        SaveLiveDungeons(true);
        if (Live() is { EventCount: > 0 } live) store.Save(live);
    }

    public CombatSnapshot Snapshot()
    {
        lock (_gate) return Current(true);
    }

    public CombatSnapshot Command(string action)
    {
        lock (_gate)
        {
            if (capture is null) throw new InvalidOperationException("Record controls require a live capture.");
            void Archive(CaptureSessionStatus previous)
            {
                if (ResolveLive(previous) is { } record) store.Save(record with { Status = "completed" }, allowEmpty: true);
            }
            switch (action)
            {
                case "start-record":
                    capture.StartRecord();
                    break;
                case "stop-record":
                    capture.StopRecord(Archive);
                    SaveCurrent();
                    break;
                case "pause-record":
                case "resume-record":
                    capture.SetRecordPaused(action == "pause-record");
                    SaveCurrent();
                    break;
                case "new-record":
                    capture.NewRecord(Archive);
                    break;
                default:
                    throw new ArgumentException("Unknown session command.", nameof(action));
            }
            return Current(true);
        }
    }

    private DateTimeOffset _lastIdentitySave;
    private void SaveIdentities()
    {
        if (DateTimeOffset.UtcNow - _lastIdentitySave < TimeSpan.FromSeconds(15) || capture?.Status.Report is not { Characters: { } characters } report) return;
        _lastIdentitySave = DateTimeOffset.UtcNow;
        try { store.ObserveIdentities(characters, report.Unidentified ?? [], report.Sightings ?? []); if (report.SkillWeapons is { } skills) store.ObserveSkillWeapons(skills); if (capture.DebugEnabled && report.Timeline is { } timeline) store.ObserveTimeline(timeline); }
        catch (Exception exception) when (exception is Microsoft.Data.Sqlite.SqliteException or IOException or System.Text.Json.JsonException) { }
    }

    public CombatSnapshot? Update()
    {
        lock (_gate)
        {
            SaveIdentities();
            SaveLiveDungeons();
            if (Live() is not { } live) return null;
            if (_lastLiveId != live.Id) { _lastLiveId = live.Id; _lastLiveSequence = -1; _lastSaved = 0; }
            if (_lastLiveSequence == live.Sequence && _lastLiveStatus == live.Status) return null;
            _lastLiveStatus = live.Status;
            _lastLiveSequence = live.Sequence;
            if (live.ElapsedSeconds - _lastSaved >= 30) { store.Save(live); _lastSaved = live.ElapsedSeconds; }
            return live;
        }
    }

    public void Save()
    {
        lock (_gate) { _lastIdentitySave = default; SaveIdentities(); SaveCurrent(); }
    }
}
