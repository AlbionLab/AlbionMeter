using System.Collections.Concurrent;
using System.Net;
using System.Reflection;
using System.Text;
using System.Text.Json;
using AlbionMeter.Core;
using AlbionMeter.Core.Capture;
using AlbionMeter.Server;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Hosting.Server;
using Microsoft.AspNetCore.Hosting.Server.Features;
using Microsoft.AspNetCore.SignalR;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;

/// <summary>
/// Regression checks for the races fixed in the server: each drives the shared object from two sides with the
/// interleaving held open (a gate or a blocked request) so the old code fails every time, not only now and then.
/// </summary>
internal static class RaceChecks
{
    const BindingFlags Private = BindingFlags.NonPublic | BindingFlags.Instance;
    static readonly byte[] Png = Convert.FromBase64String("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aWQAAAABJRU5ErkJggg==");

    sealed class FakeHandler(Func<HttpRequestMessage, Task<HttpResponseMessage>> answer) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) => answer(request);
    }

    static IConfiguration Settings(string directory, string? albionLab = null) => new ConfigurationBuilder().AddInMemoryCollection(new Dictionary<string, string?>
    {
        ["Storage:Directory"] = directory, ["AlbionLab:Url"] = albionLab ?? "http://127.0.0.1:9"
    }).Build();

    /// <summary>The icon cache downloads through one static client; its handler is swapped for the test and put back after.</summary>
    static T WithIconHandler<T>(HttpMessageHandler fake, Func<T> action)
    {
        var client = (HttpClient)typeof(WeaponIconCache).GetField("Client", BindingFlags.NonPublic | BindingFlags.Static)!.GetValue(null)!;
        var field = typeof(HttpMessageInvoker).GetFields(Private).Single(field => field.FieldType == typeof(HttpMessageHandler));
        var original = field.GetValue(client);
        field.SetValue(client, fake);
        try { return action(); }
        finally { field.SetValue(client, original); }
    }

    static AlbionLabService LabService(IConfiguration configuration, CaptureService? capture = null)
    {
        capture ??= new CaptureService(configuration);
        var store = new EncounterStore(configuration);
        return new AlbionLabService(configuration, new MeterSession(store, null, capture), capture, new CombatSourceCatalog(new("test", [], [])), new MapCatalog(configuration), NullLogger<AlbionLabService>.Instance);
    }

    static void SetHttp(AlbionLabService service, HttpMessageHandler handler) =>
        typeof(AlbionLabService).GetField("http", Private)!.SetValue(service, new HttpClient(handler));

    static void SetSession(AlbionLabService service, AlbionLabSession? session) => typeof(AlbionLabService).GetField("session", Private)!.SetValue(service, session);

    static void Cleanup(string directory)
    {
        Microsoft.Data.Sqlite.SqliteConnection.ClearAllPools();
        try { Directory.Delete(directory, true); } catch (IOException) { }
    }

    public static void Run(Action<string, Action> test, Action<bool> check)
    {
        test("Concurrent requests for one weapon icon download it once and all receive the full image", () =>
        {
            var directory = Path.Combine(Path.GetTempPath(), $"albion-icon-race-{Guid.NewGuid():N}");
            try
            {
                var ids = new[] { "T4_MAIN_SWORD", "T5_2H_HOLYSTAFF", "T6_MAIN_FROSTSTAFF" };
                var sources = new CombatSourceCatalog(new("test", [.. ids.Select((id, index) => new CombatItemDefinition(index + 1, id, id, "mainhand", 4, 0))], []));
                var cache = new WeaponIconCache(Settings(directory), new MapCatalog(Settings(directory)), sources);
                var downloads = new ConcurrentDictionary<string, int>();
                var results = WithIconHandler(new FakeHandler(async request =>
                {
                    var id = Path.GetFileNameWithoutExtension(request.RequestUri!.AbsolutePath);
                    downloads.AddOrUpdate(id, 1, (_, value) => value + 1);
                    await Task.Delay(30);
                    return new HttpResponseMessage(HttpStatusCode.OK) { Content = new ByteArrayContent(Png) };
                }), () => Task.WhenAll(Enumerable.Range(0, 96).Select(index => Task.Run(() => cache.Get(ids[index % ids.Length])))).GetAwaiter().GetResult());
                check(results.All(bytes => bytes is not null && bytes.SequenceEqual(Png)));
                check(ids.All(id => downloads.GetValueOrDefault(id) == 1));
                check(ids.All(id => File.ReadAllBytes(Path.Combine(directory, "assets", "weapon-icons", id + ".png")).SequenceEqual(Png)));
            }
            finally { Cleanup(directory); }
        });
        test("A weapon icon that keeps failing is never downloaded by two requests at once", () =>
        {
            var directory = Path.Combine(Path.GetTempPath(), $"albion-icon-overlap-{Guid.NewGuid():N}");
            try
            {
                const string id = "T4_MAIN_SWORD";
                var sources = new CombatSourceCatalog(new("test", [new(1, id, id, "mainhand", 4, 0)], []));
                var cache = new WeaponIconCache(Settings(directory), new MapCatalog(Settings(directory)), sources);
                int inFlight = 0, overlap = 0, total = 0;
                // Nothing is written to disk on a failure, so every load downloads again: two at a time means two loads ran.
                WithIconHandler(new FakeHandler(async _ =>
                {
                    Interlocked.Increment(ref total);
                    if (Interlocked.Increment(ref inFlight) > 1) Interlocked.Increment(ref overlap);
                    await Task.Delay(1);
                    Interlocked.Decrement(ref inFlight);
                    return new HttpResponseMessage(HttpStatusCode.BadGateway);
                }), () =>
                {
                    var until = DateTime.UtcNow.AddMilliseconds(600);
                    Task.WhenAll(Enumerable.Range(0, 32).Select(_ => Task.Run(async () => { while (DateTime.UtcNow < until) check(await cache.Get(id) is null); }))).GetAwaiter().GetResult();
                    return 0;
                });
                check(total > 0);
                if (overlap > 0) throw new Exception($"{overlap} of {total} downloads overlapped another download of the same icon.");
            }
            finally { Cleanup(directory); }
        });
        test("Loading the AlbionLab settings from the publish loop cannot undo a setting changed at the same moment", () =>
        {
            var current = typeof(AlbionLabService).GetProperty("Current", Private)!;
            for (var round = 0; round < 300; round++)
            {
                var directory = Path.Combine(Path.GetTempPath(), $"albion-lab-settings-{Guid.NewGuid():N}");
                try
                {
                    Directory.CreateDirectory(directory);
                    File.WriteAllText(Path.Combine(directory, "albionlab.json"), """{"deviceToken":"device","shareLocation":false,"visibility":"unlisted","anonymize":false}""");
                    var service = LabService(Settings(directory));
                    using var start = new Barrier(3);
                    // The loop reads Current.DeviceToken without the lock (Publish); the HTTP handlers go through the lock.
                    var loop = Task.Run(() => { start.SignalAndWait(); for (var index = 0; index < 4; index++) current.GetValue(service); });
                    var share = Task.Run(() => { start.SignalAndWait(); return service.ShareLocation(true); });
                    var status = Task.Run(() => { start.SignalAndWait(); return service.Status(); });
                    Task.WaitAll(loop, share, status);
                    check(share.Result.ShareLocation && service.Status().ShareLocation && service.Status().Connected);
                    check(File.ReadAllText(Path.Combine(directory, "albionlab.json")).Contains("\"shareLocation\": true") || File.ReadAllText(Path.Combine(directory, "albionlab.json")).Contains("\"shareLocation\":true"));
                }
                finally { Cleanup(directory); }
            }
        });
        test("A visibility change answered after the record moved to a new AlbionLab session does not bring the old session back", () =>
        {
            var directory = Path.Combine(Path.GetTempPath(), $"albion-lab-patch-{Guid.NewGuid():N}");
            try
            {
                Directory.CreateDirectory(directory);
                File.WriteAllText(Path.Combine(directory, "albionlab.json"), """{"deviceToken":"device"}""");
                var service = LabService(Settings(directory));
                var entered = new TaskCompletionSource();
                var release = new TaskCompletionSource();
                var patched = new List<string>();
                SetHttp(service, new FakeHandler(async request =>
                {
                    lock (patched) patched.Add($"{request.Method} {request.RequestUri!.AbsolutePath}");
                    entered.TrySetResult();
                    await release.Task;
                    return new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent("""{"visibility":"public","anonymize":true,"title":"Renamed"}""", Encoding.UTF8, "application/json") };
                }));
                var first = new AlbionLabSession("first", "https://albionlab.app/s/first", "unlisted", false, "live", null, null);
                var second = new AlbionLabSession("second", "https://albionlab.app/s/second", "unlisted", false, "live", null, null);
                SetSession(service, first);
                var update = service.UpdateSession("Renamed", "public", true, CancellationToken.None);
                check(entered.Task.Wait(5000));
                // Meanwhile the publish loop finished the first session and created one for the next record.
                SetSession(service, second);
                release.SetResult();
                var result = update.GetAwaiter().GetResult();
                check(patched.Single() == "PATCH /api/sessions/first");
                check(result.Session?.Id == "second" && service.Status().Session == second);

                // Without a replacement the answer is applied to the session it was sent for.
                SetSession(service, first);
                release = new TaskCompletionSource(); release.SetResult();
                var applied = service.UpdateSession("Renamed", "public", true, CancellationToken.None).GetAwaiter().GetResult().Session!;
                check(applied.Id == "first" && applied.Visibility == "public" && applied.Anonymize && applied.Title == "Renamed");
            }
            finally { Cleanup(directory); }
        });
        test("A record published while its AlbionLab session is disconnected does not fail or bring the session back", () =>
        {
            var directory = Path.Combine(Path.GetTempPath(), $"albion-lab-publish-{Guid.NewGuid():N}");
            WebApplication? site = null;
            try
            {
                var entered = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
                var release = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
                var builder = WebApplication.CreateSlimBuilder();
                builder.WebHost.UseUrls("http://127.0.0.1:0");
                builder.Logging.ClearProviders();
                builder.Services.AddSignalR();
                builder.Services.AddSingleton(new PublishGate(entered, release));
                site = builder.Build();
                site.MapHub<FakePublishHub>(AlbionLab.Protocol.Protocol.HubPathPublish);
                site.StartAsync().GetAwaiter().GetResult();
                var address = site.Services.GetRequiredService<IServer>().Features.Get<IServerAddressesFeature>()!.Addresses.First();

                Directory.CreateDirectory(directory);
                File.WriteAllText(Path.Combine(directory, "albionlab.json"), """{"deviceToken":"device"}""");
                var configuration = Settings(directory, address);
                // A live record running, as the capture would publish it.
                var capture = new CaptureService(configuration);
                var analysis = new CaptureAnalysis(manualRecording: true);
                analysis.BeginCapture(1_000_000);
                analysis.StartRecord(2_000_000);
                typeof(CaptureService).GetField("_status", Private)!.SetValue(capture, new CaptureSessionStatus("running", "live-capture", null, null, analysis.Snapshot(), null, null, null));
                var service = LabService(configuration, capture);
                SetHttp(service, new FakeHandler(request => Task.FromResult(request.Method == HttpMethod.Post && request.RequestUri!.AbsolutePath == "/api/sessions"
                    ? new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent("""{"id":"live","url":"https://albionlab.app/s/live","visibility":"unlisted","anonymize":false}""", Encoding.UTF8, "application/json") }
                    : new HttpResponseMessage(HttpStatusCode.NotFound))));

                var publish = (Task)typeof(AlbionLabService).GetMethod("Publish", Private)!.Invoke(service, [CancellationToken.None])!;
                check(entered.Task.Wait(10000));
                check(service.Status().Session?.Id == "live");
                // Disconnect clears the session under the lock; the hub answers the publish before it is disposed.
                SetSession(service, null);
                release.SetResult();
                publish.GetAwaiter().GetResult();
                check(service.Status().Session is null);
            }
            finally
            {
                site?.StopAsync().GetAwaiter().GetResult();
                Cleanup(directory);
            }
        });
        test("A capture restart clears the live analysis only under the analysis gate", () =>
        {
            // Reachable without packet capture rights: RunLive prepares the analysis before it opens the socket, which then
            // fails on permission (or opens, when the tests run with CAP_NET_RAW, and is stopped below).
            if (!OperatingSystem.IsLinux()) return;
            var directory = Path.Combine(Path.GetTempPath(), $"albion-capture-gate-{Guid.NewGuid():N}");
            try
            {
                var capture = new CaptureService(Settings(directory));
                var analysis = new CaptureAnalysis(manualRecording: true);
                analysis.BeginCapture(1_000_000);
                for (var index = 0; index < 5; index++) analysis.Add(new CaptureFrame(1_000_000 + index, 4, 101, [1, 2, 3, 4]));
                check(analysis.Snapshot().Frames == 5);
                typeof(CaptureService).GetField("_liveAnalysis", Private)!.SetValue(capture, analysis);
                var gate = typeof(CaptureService).GetField("_analysisGate", Private)!.GetValue(capture)!;
                long held;
                lock (gate)
                {
                    // Another request (Pause, Reset, Stop) holds the gate while the capture comes back.
                    capture.Start("raw:any", resumeRecord: true);
                    Thread.Sleep(300);
                    held = analysis.Snapshot().Frames;
                }
                var deadline = DateTime.UtcNow.AddSeconds(5);
                while (capture.Status.State is "starting" && DateTime.UtcNow < deadline) Thread.Sleep(10);
                capture.StopAsync().GetAwaiter().GetResult();
                if (held != 5) throw new Exception($"The restart cleared the analysis while another request held the gate ({held} frames left).");
                check(analysis.Snapshot().Frames == 0);
            }
            finally { Cleanup(directory); }
        });
    }

    sealed record PublishGate(TaskCompletionSource Entered, TaskCompletionSource Release);

    sealed class FakePublishHub(PublishGate gate) : Hub
    {
        public async Task<long> Publish(string session, JsonElement delta)
        {
            gate.Entered.TrySetResult();
            await gate.Release.Task;
            return 1;
        }
    }
}
