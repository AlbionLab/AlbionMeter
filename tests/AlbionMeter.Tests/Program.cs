using AlbionMeter.Core;
using AlbionMeter.Server;
using Microsoft.Extensions.Configuration;
using System.Text.Json;

var failures = new List<string>();
var count = 0;
PartyMember[] party = [new("a", "Alpha", "Sword", "sword", "#ffffff"), new("b", "Beta", "Staff", "holystaff", "#eeeeee")];
CombatEngine Engine() => new(party);
void Equal(double expected, double actual)
{
    if (Math.Abs(expected - actual) > 0.00001) throw new Exception($"Expected {expected}, received {actual}.");
}
void True(bool value) { if (!value) throw new Exception("Expected true."); }
void Test(string name, Action action)
{
    count++;
    try { action(); Console.WriteLine($"PASS {name}"); }
    catch (Exception exception) { failures.Add(name); Console.WriteLine($"FAIL {name}: {exception.Message}"); }
}
Test("An explicitly archived empty record is saved without changing automatic empty-record filtering", () =>
{
    var directory = Path.Combine(Path.GetTempPath(), $"albion-explicit-history-{Guid.NewGuid():N}");
    try
    {
        var configuration = new ConfigurationBuilder().AddInMemoryCollection(new Dictionary<string, string?> { ["Storage:Directory"] = directory }).Build();
        var store = new EncounterStore(configuration);
        var empty = Engine().Snapshot(true);
        store.Save(empty);
        True(store.List().Count == 0);
        store.Save(empty, allowEmpty: true);
        True(store.List().Single().Id == empty.Id);
        True(store.Get(empty.Id)!.Contains("completed"));
    }
    finally { Directory.Delete(directory, true); }
});
Test("Dungeon aggregates preserve source items equipment histories and immutable timelines", () =>
{
    var run = new DungeonRun("Dungeon", "1315", "Entrance", "live");
    var first = run.EnterFloor("one", "one", "Floor one");
    var engine = Engine(); engine.Advance(2);
    engine.Add(new("one", "a", 1, CombatMetric.Damage, 100, 9022, SourceItemIndex: 9022, SourceSlot: "mainhand", SourceKind: "autoAttack"));
    engine.Add(new("two", "b", 1.5, CombatMetric.Healing, 40, 7108, SourceItemIndex: 7108, SourceSlot: "mainhand", SourceKind: "ability"));
    run.Record("one", engine.Snapshot(true) with { Location = first });
    var second = run.EnterFloor("two", "two", "Floor two");
    var next = Engine(); next.Advance(3);
    next.Add(new("three", "a", 1, CombatMetric.Damage, 300, 9022, SourceItemIndex: 4308, SourceSlot: "chest", SourceKind: "ability"));
    run.Record("two", next.Snapshot(true) with { Location = second });
    var snapshot = run.Snapshot("running");
    Equal(400, snapshot.Totals.TotalDamage); Equal(40, snapshot.Totals.TotalHealing); Equal(5, snapshot.Totals.ElapsedSeconds);
    Equal(400, snapshot.Totals.Timeline.Sum(point => point.Damage.Values.Sum()));
    Equal(400, snapshot.Totals.Timeline.Sum(point => point.ItemDamage!.Values.Sum()));
    Equal(40, snapshot.Totals.Timeline.Sum(point => point.ItemHealing!.Values.Sum()));
    Equal(400, snapshot.Totals.WeaponContributions!.Single(item => item.PlayerId == "a").Damage);
    Equal(100, snapshot.Totals.ItemContributions!.Single(item => item.Slot == "mainhand" && item.PlayerId == "a").Damage);
    Equal(300, snapshot.Totals.ItemContributions!.Single(item => item.Slot == "chest").Damage);
    Equal(80, snapshot.Totals.Members.Single(member => member.Id == "a").EncounterDps);
    snapshot.Floors[0].Encounters[0].Timeline[1].ItemDamage!.Clear();
    Equal(400, run.Snapshot("running").Totals.Timeline.Sum(point => point.ItemDamage!.Values.Sum()));
});
Test("Equipment icons resolve the exact cape tier and reject unknown asset identifiers", () =>
{
    var directory = Path.Combine(Path.GetTempPath(), $"albion-item-icons-{Guid.NewGuid():N}");
    try
    {
        var configuration = new ConfigurationBuilder().AddInMemoryCollection(new Dictionary<string, string?> { ["Storage:Directory"] = directory }).Build();
        var source = new CombatSourceCatalog(new("test", [new(2637, "T4_CAPEITEM_FW_THETFORD@3", "Thetford Cape", "cape", 4, 3)], []));
        True(source.FindItem(2637) is { Tier: 4, Enchantment: 3, Slot: "cape" });
        True(source.FindItem(99999) is null && !source.ContainsItem("../unknown"));
        var folder = Path.Combine(directory, "assets", "weapon-icons"); Directory.CreateDirectory(folder);
        var png = Convert.FromBase64String("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aWQAAAABJRU5ErkJggg==");
        File.WriteAllBytes(Path.Combine(folder, "T4_CAPEITEM_FW_THETFORD@3.png"), png);
        var cache = new WeaponIconCache(configuration, new MapCatalog(configuration), source);
        True(cache.Get("T4_CAPEITEM_FW_THETFORD@3").GetAwaiter().GetResult()!.SequenceEqual(png));
        True(cache.Get("../unknown").GetAwaiter().GetResult() is null);
        True(cache.Get("UNRECOGNIZED_ITEM").GetAwaiter().GetResult() is null);
    }
    finally { Directory.Delete(directory, true); }
});
Test("Record controls persist pause boundaries and recover them in chronological order", () =>
{
    var directory = Path.Combine(Path.GetTempPath(), $"albion-record-controls-{Guid.NewGuid():N}");
    try
    {
        var controls = new RecordControls(directory);
        controls.Append(new(3_000_000, "resume", null)); controls.Append(new(1_000_000, "pause", null));
        File.AppendAllText(Path.Combine(directory, "record-controls.jsonl"), "invalid\n");
        var recovered = new RecordControls(directory).Read();
        True(recovered.Length == 2 && recovered[0].Action == "pause" && recovered[1].Action == "resume");
        var analysis = new AlbionMeter.Core.Capture.CaptureAnalysis();
        RecordControls.Apply(analysis, recovered[0]); True(analysis.Snapshot().RecordPaused);
        RecordControls.Apply(analysis, recovered[1]); True(!analysis.Snapshot().RecordPaused);
        RecordControls.Apply(analysis, new(4_000_000, "pause", Guid.NewGuid())); True(!analysis.Snapshot().RecordPaused);
        RecordControls.Apply(analysis, new(4_000_000, "pause", null));
        analysis.BeginCapture(5_000_000); True(analysis.Snapshot().RecordPaused);
        RecordControls.Apply(analysis, new(6_000_000, "new-record", null)); True(!analysis.Snapshot().RecordPaused);
    }
    finally { Directory.Delete(directory, true); }
});
Test("An empty encounter has finite zero rates", () =>
{
    var snapshot = Engine().Snapshot(true);
    Equal(0, snapshot.Members[0].EncounterDps);
    Equal(0, snapshot.Members[0].RollingDps);
});
Test("Encounter and rolling rates use distinct durations", () =>
{
    var engine = Engine(); engine.Advance(30);
    engine.Add(new("old", "a", 10, CombatMetric.Damage, 42000));
    engine.Add(new("new", "a", 28, CombatMetric.Damage, 18000));
    var member = engine.Snapshot(true).Members[0];
    Equal(2000, member.EncounterDps); Equal(3600, member.RollingDps);
});
Test("Distinct identical hits are both counted", () =>
{
    var engine = Engine(); engine.Advance(1);
    engine.Add(new("one", "a", 0.5, CombatMetric.Damage, 50));
    engine.Add(new("two", "a", 0.5, CombatMetric.Damage, 50));
    Equal(100, engine.Snapshot(true).TotalDamage);
});
Test("A repeated transport identity is counted once", () =>
{
    var engine = Engine(); engine.Advance(1);
    True(engine.Add(new("one", "a", 0.5, CombatMetric.Damage, 50)));
    True(!engine.Add(new("one", "a", 0.5, CombatMetric.Damage, 50)));
    Equal(50, engine.Snapshot(true).TotalDamage);
});
Test("Buckets use half-open second boundaries", () =>
{
    var engine = Engine(); engine.Advance(2);
    engine.Add(new("one", "a", 0.999, CombatMetric.Damage, 70));
    engine.Add(new("two", "a", 1, CombatMetric.Damage, 30));
    var timeline = engine.Snapshot(true).Timeline;
    Equal(70, timeline[0].Damage["a"]); Equal(30, timeline[1].Damage["a"]);
});
Test("Valid idle observation produces zero buckets", () =>
{
    var engine = Engine(); engine.Advance(5);
    Equal(6, engine.Snapshot(true).Timeline.Length);
    Equal(0, engine.Snapshot(true).Timeline[3].Damage["a"]);
});
Test("Healing and damage stay separate", () =>
{
    var engine = Engine(); engine.Advance(2);
    engine.Add(new("one", "a", 1, CombatMetric.Damage, 100));
    engine.Add(new("two", "b", 1, CombatMetric.Healing, 80));
    var snapshot = engine.Snapshot(true);
    Equal(100, snapshot.TotalDamage); Equal(80, snapshot.TotalHealing);
    Equal(40, snapshot.Members[1].EncounterHps); Equal(0, snapshot.Members[1].Damage);
});
Test("Every player's damage reconciles with the timeline", () =>
{
    var engine = Engine(); engine.Advance(60);
    for (var i = 0; i < 600; i++) engine.Add(new($"{i}", i % 2 == 0 ? "a" : "b", i / 10d, CombatMetric.Damage, i + 1));
    var snapshot = engine.Snapshot(true);
    foreach (var member in snapshot.Members) Equal(member.Damage, snapshot.Timeline.Sum(point => point.Damage[member.Id]));
    Equal(snapshot.TotalDamage, snapshot.Members.Sum(member => member.Damage));
});
Test("Rolling rates decay without new combat events", () =>
{
    var engine = Engine(); engine.Advance(1);
    engine.Add(new("one", "a", 0.5, CombatMetric.Damage, 100));
    engine.Advance(6); Equal(0, engine.RollingRate("a", CombatMetric.Damage));
});
Test("Late events revise their original bucket", () =>
{
    var engine = Engine(); engine.Advance(10); engine.Snapshot(false);
    engine.Add(new("late", "a", 2.5, CombatMetric.Damage, 100));
    var delta = engine.Snapshot(false);
    Equal(1, delta.Timeline.Length); Equal(2, delta.Timeline[0].Second);
    Equal(100, delta.Timeline[0].Damage["a"]);
});
Test("A full snapshot does not consume pending changes", () =>
{
    var engine = Engine(); engine.Snapshot(false); engine.Advance(1);
    engine.Add(new("one", "a", 1, CombatMetric.Damage, 10));
    engine.Snapshot(true); True(engine.Snapshot(false).Timeline.Length > 0);
    Equal(0, engine.Snapshot(false).Timeline.Length);
});
Test("Snapshot dictionaries cannot mutate engine state", () =>
{
    var engine = Engine(); var snapshot = engine.Snapshot(true);
    snapshot.Timeline[0].Damage["a"] = 999;
    Equal(0, engine.Snapshot(true).Timeline[0].Damage["a"]);
});
Test("Unknown players and invalid events are rejected", () =>
{
    var engine = Engine(); engine.Advance(1);
    True(!engine.Add(new("unknown", "x", 0, CombatMetric.Damage, 10)));
    True(!engine.Add(new("future", "a", 2, CombatMetric.Damage, 10)));
    True(!engine.Add(new("negative", "a", -1, CombatMetric.Damage, 10)));
    True(!engine.Add(new("nan", "a", 0, CombatMetric.Damage, double.NaN)));
    True(!engine.Add(new("infinity", "a", 0, CombatMetric.Damage, double.PositiveInfinity)));
    True(!engine.Add(new("zero", "a", 0, CombatMetric.Damage, 0)));
    True(!engine.Add(new("metric", "a", 0, (CombatMetric)99, 10)));
    Equal(0, engine.EventCount);
});
Test("Rates are not artificially capped at 9999", () =>
{
    var engine = Engine(); engine.Advance(1);
    engine.Add(new("burst", "a", 0.5, CombatMetric.Damage, 100000));
    Equal(100000, engine.Snapshot(true).Members[0].EncounterDps);
});
Test("Shares use the same party denominator", () =>
{
    var engine = Engine(); engine.Advance(1);
    engine.Add(new("one", "a", 0.5, CombatMetric.Damage, 30));
    engine.Add(new("two", "b", 0.5, CombatMetric.Damage, 70));
    Equal(30, engine.Snapshot(true).Members[0].DamageShare);
    Equal(70, engine.Snapshot(true).Members[1].DamageShare);
});
Test("Early rolling rates use observed duration", () =>
{
    var engine = Engine(); engine.Advance(2);
    engine.Add(new("one", "a", 1, CombatMetric.Damage, 100));
    Equal(50, engine.RollingRate("a", CombatMetric.Damage));
});
Test("Backward and excessive clock advances are rejected", () =>
{
    var engine = Engine(); engine.Advance(2);
    foreach (var time in new[] { 1d, 7201d, double.NaN })
    {
        var rejected = false;
        try { engine.Advance(time); } catch (ArgumentOutOfRangeException) { rejected = true; }
        True(rejected);
    }
});
Test("Separate encounters never share totals", () =>
{
    var first = Engine(); first.Advance(1); first.Add(new("one", "a", 0, CombatMetric.Damage, 100));
    var second = Engine(); Equal(0, second.Snapshot(true).TotalDamage); True(first.Id != second.Id);
});
var temporaryDirectory = Path.Combine(Path.GetTempPath(), $"albion-tests-{Guid.NewGuid():N}");
var configuration = new ConfigurationBuilder().AddInMemoryCollection(new Dictionary<string, string?>
{
    ["Storage:Directory"] = temporaryDirectory
}).Build();
var store = new EncounterStore(configuration);
Test("History persists across storage instances", () =>
{
    var engine = Engine(); engine.Advance(1);
    engine.Add(new("saved", "a", 0.5, CombatMetric.Damage, 123));
    store.Save(engine.Snapshot(true));
    var reopened = new EncounterStore(configuration);
    Equal(123, reopened.List().Single(item => item.Id == engine.Id).Damage);
    using var json = JsonDocument.Parse(reopened.Get(engine.Id)!);
    Equal(123, json.RootElement.GetProperty("totalDamage").GetDouble());
    True(reopened.Get("missing") == null);
});
Test("Empty encounters are not saved", () =>
{
    var engine = Engine(); store.Save(engine.Snapshot(true)); True(store.Get(engine.Id) == null);
});
Test("Unnamed players in saved records are named by elimination, and similar equipment alone never names them", () =>
{
    var delta = Guid.NewGuid(); var echo = Guid.NewGuid();
    string Record(string memberId, Guid? named)
    {
        var engine = new CombatEngine([]);
        engine.RegisterMember(new(memberId, "Unidentified player (obj 5)", "Weapon not observed", "unknown", "#fff"));
        if (named is { } id) engine.RegisterMember(new(id.ToString(), "Delta", "Weapon not observed", "unknown", "#fff"));
        engine.Advance(2); engine.Add(new($"{memberId}:hit", memberId, 1, CombatMetric.Damage, 40));
        if (named is { } present) engine.Add(new($"{memberId}:named-hit", present.ToString(), 1, CombatMetric.Damage, 10));
        store.Save(engine.Snapshot(true));
        return engine.Id;
    }
    var first = Record("unidentified:5:1000", null);
    var second = Record("unidentified:6:1000", delta);
    int?[] gear = [7696, 0, 4224, 5452, 5472, 2909, 2587, 3098];
    var resolved = store.ObserveIdentities(new Dictionary<Guid, AlbionMeter.Core.Observation.KnownCharacter> { [delta] = new("Delta", gear), [echo] = new("Echo", null) },
        [new("unidentified:5:1000", first, 5, [7696, 0, 4224, 5452, 5472, 2909, 2587, 1111], [delta, echo], 1_000_000),
         new("unidentified:6:1000", second, 6, null, [delta, echo], 1_000_000)]);
    True(resolved.Single().MemberId == "unidentified:6:1000" && resolved.Single().Name == "Echo" && resolved.Single().Method == "elimination");
    True(store.Get(first)!.Contains("Delta or Echo (obj 5)"));
    True(store.Get(second)!.Contains("\"Echo\""));
    True(store.KnownCharacters()[delta].Equipment!.SequenceEqual(gear));
});
Test("Skill signatures never name players; the candidates a capture proved are named or shown as a label", () =>
{
    Guid healer = Guid.NewGuid(), archerA = Guid.NewGuid(), archerB = Guid.NewGuid();
    var engine = new CombatEngine([]);
    foreach (var id in new[] { "unidentified:7:1", "unidentified:8:1", "unidentified:9:1" }) engine.RegisterMember(new(id, "Unidentified player", "Weapon not observed", "unknown", "#fff"));
    engine.Advance(2);
    foreach (var id in new[] { "unidentified:7:1", "unidentified:8:1", "unidentified:9:1" }) engine.Add(new($"{id}:hit", id, 1, CombatMetric.Damage, 10));
    store.Save(engine.Snapshot(true));
    var resolved = store.ObserveIdentities(new Dictionary<Guid, AlbionMeter.Core.Observation.KnownCharacter>
    {
        [healer] = new("Healer", null, [3404, 3409, 3420, 4055]), [archerA] = new("ArcherA", null, [3925, 4078, 4253]), [archerB] = new("ArcherB", null, [3925, 4078, 4253])
    }, [new("unidentified:7:1", engine.Id, 7, null, [healer], 1, [3404, 3409, 3420, 4055]),
        new("unidentified:8:1", engine.Id, 8, null, [archerA, archerB], 1, [3925, 4078, 4253]),
        new("unidentified:9:1", engine.Id, 9, null, [healer, archerA, archerB], 1, [3925, 4078, 4253])]);
    True(resolved.Single().MemberId == "unidentified:7:1" && resolved.Single().Name == "Healer" && resolved.Single().Method == "elimination");
    True(store.Get(engine.Id)!.Contains("\"Healer\"") && store.Get(engine.Id)!.Contains("ArcherA or ArcherB (obj 8)"));
});
Test("An unnamed object is named by a later sighting of the same object in the same zone visit", () =>
{
    var later = Guid.NewGuid();
    var engine = new CombatEngine([]);
    engine.RegisterMember(new("unidentified:12:5", "Unidentified player (obj 12)", "Weapon not observed", "unknown", "#fff"));
    engine.Advance(2); engine.Add(new("same-zone-hit", "unidentified:12:5", 1, CombatMetric.Damage, 33));
    store.Save(engine.Snapshot(true));
    var resolved = store.ObserveIdentities(new Dictionary<Guid, AlbionMeter.Core.Observation.KnownCharacter> { [later] = new("Later", null) },
        [new("unidentified:12:5", engine.Id, 12, null, [], 1, null, "visit-a")],
        [new(later, 12, "visit-b", 2, "chat"), new(later, 12, "visit-a", 3, "chat")]);
    True(resolved.Single().Method == "same-zone" && store.Get(engine.Id)!.Contains("\"Later\""));
});
Test("Players whose candidates wear the same equipment stay a labeled group, and the last candidate is named by elimination", () =>
{
    Guid a = Guid.NewGuid(), b = Guid.NewGuid(), c = Guid.NewGuid(), mei = Guid.NewGuid();
    var members = new[] { "unidentified:21:1", "unidentified:22:1", "unidentified:23:1", "unidentified:24:1" };
    var engine = new CombatEngine([]);
    foreach (var id in members) engine.RegisterMember(new(id, "Unidentified player", "Weapon not observed", "unknown", "#fff"));
    engine.Advance(2);
    foreach (var id in members) engine.Add(new($"{id}:group-hit", id, 1, CombatMetric.Damage, 10));
    store.Save(engine.Snapshot(true));
    int[] archer = [3053, 3054, 3074];
    var resolved = store.ObserveIdentities(new Dictionary<Guid, AlbionMeter.Core.Observation.KnownCharacter>
    {
        [a] = new("Lusphu", null, archer), [b] = new("FlameKings", null, archer), [c] = new("LupaMagica", null, archer), [mei] = new("MistressMei", null, [214, 3215, 3216, 3270])
    }, members.Select((id, index) => new AlbionMeter.Core.Observation.UnidentifiedPlayer(id, engine.Id, 21 + index, null, index < 3 ? [a, b, c, mei] : [mei], 1, index < 3 ? archer : [3516, 3518, 3524, 3554])).ToArray());
    True(resolved.Single().MemberId == "unidentified:24:1" && resolved.Single().Name == "MistressMei" && resolved.Single().Method == "elimination");
    True(store.Get(engine.Id)!.Contains("FlameKings, LupaMagica or Lusphu (obj 21)"));
});
Test("A named player replaces its empty roster entry instead of appearing twice", () =>
{
    var character = Guid.NewGuid();
    var engine = new CombatEngine([]);
    engine.RegisterMember(new(character.ToString(), "Sick", "Weapon not observed", "unknown", "#fff"));
    engine.RegisterMember(new("unidentified:355:0", "Unidentified player (obj 355)", "Weapon not observed", "unknown", "#fff"));
    engine.Advance(3); engine.Add(new("merge-hit", "unidentified:355:0", 2, CombatMetric.Damage, 218));
    store.Save(engine.Snapshot(true));
    store.ObserveIdentities(new Dictionary<Guid, AlbionMeter.Core.Observation.KnownCharacter> { [character] = new("Sick", null) },
        [new("unidentified:355:0", engine.Id, 355, null, [], 1, null, "merge-visit")], [new(character, 355, "merge-visit", 2, "chat")]);
    var saved = System.Text.Json.JsonSerializer.Deserialize<CombatSnapshot>(store.Get(engine.Id)!, new System.Text.Json.JsonSerializerOptions(System.Text.Json.JsonSerializerDefaults.Web))!;
    var members = saved.Members.Concat(saved.FormerMembers ?? []).ToArray();
    True(members.Count(member => member.Name == "Sick") == 1 && members.Single(member => member.Name == "Sick").Id == character.ToString());
    Equal(218, members.Single(member => member.Name == "Sick").Damage);
    True(saved.Timeline.Sum(point => point.Damage.GetValueOrDefault(character.ToString())) == 218 && !store.Get(engine.Id)!.Contains("unidentified:355:0"));
});
Test("Saving a live record again keeps names found for its unnamed players", () =>
{
    var character = Guid.NewGuid();
    var engine = new CombatEngine([]);
    engine.RegisterMember(new("unidentified:77:0", "Unidentified player (obj 77)", "Weapon not observed", "unknown", "#fff"));
    engine.Advance(2); engine.Add(new("resave-hit", "unidentified:77:0", 1, CombatMetric.Damage, 44));
    store.Save(engine.Snapshot(true));
    store.ObserveIdentities(new Dictionary<Guid, AlbionMeter.Core.Observation.KnownCharacter> { [character] = new("Later", null) },
        [new("unidentified:77:0", engine.Id, 77, null, [], 1, null, "resave-visit")], [new(character, 77, "resave-visit", 2, "chat")]);
    engine.Advance(3); engine.Add(new("resave-hit-2", "unidentified:77:0", 2.5, CombatMetric.Damage, 6));
    store.Save(engine.Snapshot(true));
    True(store.Get(engine.Id)!.Contains("\"Later\"") && !store.Get(engine.Id)!.Contains("Unidentified player (obj 77)"));
    True(store.Get(engine.Id)!.Contains("\"damage\":50"));
});
Test("History retention discards empty records before records with combat", () =>
{
    var empty = new List<string>();
    for (var i = 0; i < 20; i++) { var engine = Engine(); store.Save(engine.Snapshot(true), allowEmpty: true); empty.Add(engine.Id); }
    for (var i = 0; i < EncounterStore.RetainedEncounters; i++)
    {
        var engine = Engine(); engine.Advance(1);
        engine.Add(new("sample", "a", 0.5, CombatMetric.Damage, 1));
        store.Save(engine.Snapshot(true));
    }
    True(empty.All(id => store.Get(id) == null));
    Equal(EncounterStore.ListedEncounters, store.List().Count);
});
Test("Without a capture the session has no party and rejects commands", () =>
{
    var session = new MeterSession(store);
    var initial = session.Snapshot();
    Equal(0, initial.Members.Length); True(initial.Location is null && initial.PartyId is null);
    True(initial.Mode == "unavailable"); Equal(0, session.Dungeons().Length); True(session.Update() is null);
    foreach (var action in new[] { "start", "pause", "finish", "reset", "unknown-command" })
    {
        var rejected = false;
        try { session.Command(action); } catch (InvalidOperationException) { rejected = true; } catch (ArgumentException) { rejected = true; }
        True(rejected);
    }
});
Test("History content is parameterized by encounter identity", () =>
{
    True(store.Get("' OR 1=1 --") == null);
});
Test("Random dungeons and expeditions, hardcore included, are recorded as dungeon runs", () =>
{
    True(AlbionMeter.Core.Observation.DungeonCombatObservation.Kind("@RANDOMDUNGEON@6d8f0b2a-1c3e-4f5a-9b7d-2e4c6a8f0b1d") == "Random dungeon");
    True(AlbionMeter.Core.Observation.DungeonCombatObservation.Kind("@EXPEDITION@88714a12-4a45-4081-80ec-4f48e5b76565") == "Expedition");
    True(AlbionMeter.Core.Observation.DungeonCombatObservation.Kind("1316") == null);
    True(AlbionMeter.Core.Observation.DungeonCombatObservation.Kind("@ISLAND@1f2e3d4c") == null);
});
Test("Every expedition entry is its own run named by its template", () =>
{
    var character = Guid.NewGuid();
    var observation = new AlbionMeter.Core.Observation.DungeonCombatObservation();
    var timestamp = 1_000_000L;
    void Enter(string map, string template)
    {
        var outside = new AlbionMeter.Core.Observation.LocalCharacterSnapshot(character, 1, "KPey", null, null, "1301", null, "observed", timestamp, "zone");
        observation.Observe(new(3, 0, 0, new() { [0] = map, [1] = template, [253] = 41 }), "zone", "zone:1", timestamp += 1_000_000, outside, null);
        var inside = outside with { MapId = map, ObservedAtMicroseconds = timestamp += 1_000_000 };
        observation.Observe(new(3, 0, 0, new() { [1] = character.ToByteArray(), [8] = map, [66] = "1301", [253] = 2 }), "zone", "zone:2", timestamp, inside, null);
    }
    Enter("@EXPEDITION@84fb7dda-9fab-4aa3-a98d-05252b177b0c", "Mine-SOLO");
    Enter("@EXPEDITION@1a2b3c4d-9fab-4aa3-a98d-05252b177b0c", "Mine-SOLO");
    var runs = observation.Snapshot();
    Equal(2, runs.Length);
    True(runs.All(run => run.Name == "Expedition" && run.Floors.Length == 1 && run.Floors[0].MapName == "Mine-SOLO"));
});
Test("Dungeon checkpoints replace totals and revisits preserve floor identity", () =>
{
    var run = new DungeonRun("Test dungeon", "1207", "Birchcopse", "live");
    var first = run.EnterFloor("cluster-a", "random-a", "Random dungeon");
    var engine = Engine(); engine.Advance(10);
    engine.Add(new("hit", "a", 1, CombatMetric.Damage, 100));
    run.Record(first.FloorId, engine.Snapshot(true));
    run.Record(first.FloorId, engine.Snapshot(true));
    var second = run.EnterFloor("cluster-b", "random-b", "Random dungeon");
    var other = Engine(); other.Advance(30);
    other.Add(new("hit", "a", 2, CombatMetric.Damage, 300));
    run.Record(second.FloorId, other.Snapshot(true));
    Equal(1, run.EnterFloor("cluster-a", "random-a", "Random dungeon").FloorNumber);
    var snapshot = run.Snapshot("completed");
    Equal(2, snapshot.Floors.Length); Equal(400, snapshot.Totals.TotalDamage);
    Equal(10, snapshot.Totals.Members[0].EncounterDps);
    Equal(100, snapshot.Floors[0].Totals.TotalDamage);
    store.SaveDungeon(snapshot);
    var reopened = new EncounterStore(configuration).GetDungeon(run.Id)!;
    Equal(400, reopened.Totals.TotalDamage); Equal(2, reopened.Floors.Length);
    True(new DungeonRun("Test dungeon", "1207", "Birchcopse", "live").Id != run.Id);
    var rejected = false;
    try { run.Record(second.FloorId, engine.Snapshot(true)); } catch (InvalidOperationException) { rejected = true; }
    True(rejected);
    rejected = false;
    try { run.Record(first.FloorId, engine.Snapshot(false)); } catch (ArgumentException) { rejected = true; }
    True(rejected);
});
Test("Leaving removes active membership while preserving party contributions", () =>
{
    var engine = new CombatEngine(party, "party-one"); engine.Advance(2);
    engine.Add(new("damage", "a", 1, CombatMetric.Damage, 100));
    engine.Add(new("healing", "a", 1.5, CombatMetric.Healing, 40));
    engine.SetMembership("a", false);
    var departed = engine.Snapshot(true);
    True(departed.Members.All(member => member.Id != "a"));
    Equal(100, departed.FormerMembers!.Single().Damage); Equal(40, departed.FormerMembers!.Single().Healing);
    Equal(100, departed.TotalDamage);
    engine.Advance(4);
    True(!engine.Add(new("outside", "a", 3, CombatMetric.Damage, 900)));
    engine.SetMembership("a", true);
    var returned = engine.Snapshot(true);
    True(returned.PartyId == "party-one"); Equal(100, returned.Members.Single(member => member.Id == "a").Damage);
    Equal(0, returned.FormerMembers!.Length);
    engine.Advance(5); engine.Add(new("after", "a", 4.5, CombatMetric.Damage, 25));
    Equal(125, engine.Snapshot(true).TotalDamage);
    var another = new CombatEngine(party, "party-two");
    Equal(0, another.Snapshot(true).TotalDamage); True(another.PartyId != engine.PartyId);
});
Test("Late contributions use membership at event time rather than arrival time", () =>
{
    var engine = Engine(); engine.Advance(2); engine.SetMembership("a", false); engine.Advance(4);
    True(engine.Add(new("late-before-leave", "a", 1, CombatMetric.Damage, 75)));
    True(!engine.Add(new("at-leave", "a", 2, CombatMetric.Damage, 100)));
    engine.SetMembership("a", true); engine.Advance(5);
    True(!engine.Add(new("late-during-absence", "a", 3, CombatMetric.Damage, 100)));
    True(engine.Add(new("at-return", "a", 4, CombatMetric.Damage, 25)));
    Equal(100, engine.Snapshot(true).TotalDamage);
});
Test("Dungeon totals and persisted encounters retain former participants", () =>
{
    var engine = Engine(); engine.Advance(2); engine.Add(new("hit", "a", 1, CombatMetric.Damage, 150));
    engine.SetMembership("a", false);
    var snapshot = engine.Snapshot(true);
    var run = new DungeonRun("Dungeon", "1207", "Birchcopse", "live");
    run.EnterFloor("floor-one", "random", "Random dungeon"); run.Record("floor-one", snapshot);
    Equal(150, run.Snapshot("completed").Totals.Members.Single(member => member.Id == "a").Damage);
    store.Save(snapshot);
    using var saved = JsonDocument.Parse(store.Get(snapshot.Id)!);
    Equal(150, saved.RootElement.GetProperty("formerMembers")[0].GetProperty("damage").GetDouble());
});
Test("A newly observed member cannot claim contributions from before joining", () =>
{
    var engine = new CombatEngine([]); engine.Advance(5); engine.RegisterMember(party[0]);
    True(!engine.Add(new("before", "a", 4, CombatMetric.Damage, 100)));
    True(engine.Add(new("now", "a", 5, CombatMetric.Damage, 25)));
    Equal(0, engine.Snapshot(true).Timeline[0].Damage["a"]);
    Equal(25, engine.Snapshot(true).TotalDamage);
});
ManualRecordingChecks.Run(Test, True);
PacketStorageChecks.Run(Test, True);
HistoryChecks.Run(Test, True);
ObservationChecks.Run(Test, True);
MembershipGapChecks.Run(Test, True);
CaptureChecks.Run(Test, True, args.Contains("--native-capture"));
Microsoft.Data.Sqlite.SqliteConnection.ClearAllPools();
Directory.Delete(temporaryDirectory, true);
Console.WriteLine($"{count - failures.Count}/{count} checks passed.");
return failures.Count == 0 ? 0 : 1;
