using System.Diagnostics;
using System.Net;
using System.Reflection;
using AlbionMeter.Server;
using Microsoft.AspNetCore.SignalR;

var version = typeof(MeterSession).Assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion.Split('+')[0] ?? "0.0.0";

static int PortInUse()
{
    Console.Error.WriteLine();
    Console.Error.WriteLine("  Port 5187 is already in use. Albion Meter may already be running:");
    Console.Error.WriteLine("  open http://127.0.0.1:5187/ in your browser, or close the other window and try again.");
    if (!Console.IsInputRedirected) { Console.Error.WriteLine("  Press Enter to close."); Console.ReadLine(); }
    return 1;
}
try { var probe = new System.Net.Sockets.TcpListener(IPAddress.Loopback, 5187); probe.Start(); probe.Stop(); }
catch (System.Net.Sockets.SocketException) { return PortInUse(); }
var staticRoot = Path.Combine(AppContext.BaseDirectory, "wwwroot");
var builder = WebApplication.CreateBuilder(new WebApplicationOptions
{
    Args = args,
    WebRootPath = Directory.Exists(staticRoot) ? staticRoot : null
});
var portableData = Path.Combine(AppContext.BaseDirectory, "data");
if (builder.Configuration["Storage:Directory"] is null && Directory.Exists(portableData)) builder.Configuration["Storage:Directory"] = portableData;
builder.WebHost.UseUrls("http://127.0.0.1:5187");
if (!builder.Environment.IsDevelopment()) builder.Logging.AddFilter("Microsoft.AspNetCore", LogLevel.Warning);
builder.WebHost.ConfigureKestrel(options => options.Limits.MaxRequestBodySize = 45_000_000);
builder.Services.AddSingleton<MapCatalog>();
builder.Services.AddSingleton(_ => AlbionMeter.Core.CombatSourceCatalog.Load(Path.Combine(AppContext.BaseDirectory, "assets", "combat.json")) ?? new AlbionMeter.Core.CombatSourceCatalog(new("unknown", [], [])));
builder.Services.AddSingleton<WeaponIconCache>();
builder.Services.AddSingleton<CharacterContextStore>();
builder.Services.AddSingleton<CaptureService>();
builder.Services.AddHostedService(provider => provider.GetRequiredService<CaptureService>());
builder.Services.AddSingleton<EncounterStore>();
builder.Services.AddSingleton<MeterSession>();
builder.Services.AddHostedService<MeterWorker>();
builder.Services.AddSingleton<AlbionLabService>();
builder.Services.AddHostedService(provider => provider.GetRequiredService<AlbionLabService>());
builder.Services.AddSignalR(options => options.MaximumReceiveMessageSize = 8192);
var app = builder.Build();
app.Use(async (context, next) =>
{
    var remote = context.Connection.RemoteIpAddress;
    if (remote is null || !IPAddress.IsLoopback(remote)
        || context.Request.Host.Host is not ("127.0.0.1" or "localhost" or "[::1]"))
    {
        context.Response.StatusCode = 403;
        return;
    }
    var origin = context.Request.Headers.Origin.ToString();
    if (origin.Length > 0 && (!Uri.TryCreate(origin, UriKind.Absolute, out var uri)
        || uri.Scheme != "http" || uri.Authority != context.Request.Host.Value))
    {
        context.Response.StatusCode = 403;
        return;
    }
    if (context.Request.Path.StartsWithSegments("/api") && context.Request.Method == "POST"
        && context.Request.Headers["X-Albion-Meter"] != "local")
    {
        context.Response.StatusCode = 403;
        return;
    }
    context.Response.Headers.XContentTypeOptions = "nosniff";
    context.Response.Headers["Referrer-Policy"] = "no-referrer";
    context.Response.Headers["Content-Security-Policy"] = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https://cdn.discordapp.com; connect-src 'self' ws://127.0.0.1:5187 ws://localhost:5187; frame-ancestors 'none'; base-uri 'self'";
    await next();
});
app.UseDefaultFiles();
app.UseStaticFiles();
app.MapGet("/api/health", (MeterSession session) => new { status = "ok", version, captureAvailable = session.Snapshot().Mode == "live", packetCaptureImplemented = true, source = session.Snapshot().Mode, executable = Environment.ProcessPath, platform = OperatingSystem.IsWindows() ? "windows" : OperatingSystem.IsLinux() ? "linux" : "other", transport = "SignalR", storage = "SQLite" });
app.MapGet("/api/session", (MeterSession session) => session.Snapshot());
app.MapGet("/api/encounters", (EncounterStore store) => store.List());
app.MapGet("/api/encounters/{id}", (string id, EncounterStore store) => store.Get(id) is { } json
    ? Results.Text(json, "application/json") : Results.NotFound());
app.MapCatalogEndpoints();
app.MapCaptureEndpoints();
app.MapHistoryEndpoints();
app.MapGet("/api/weapon-icon/{id}", async (string id, WeaponIconCache cache) =>
{
    var bytes = await cache.Get(id);
    return bytes is null ? Results.NotFound() : Results.File(bytes, "image/png");
});
app.MapGet("/api/items/{index:int}", (int index, AlbionMeter.Core.CombatSourceCatalog sources) =>
    sources.FindItem(index) is { } item ? Results.Ok(item) : Results.NotFound());
app.MapGet("/api/item-icon/{id}", async (string id, WeaponIconCache cache) =>
{
    var bytes = await cache.Get(id);
    return bytes is null ? Results.NotFound() : Results.File(bytes, "image/png");
});
app.MapGet("/api/character", (CaptureService capture, CharacterContextStore context, MapCatalog catalog) =>
{
    var status = capture.Status;
    var current = status.Report?.Character;
    var character = current ?? context.Snapshot;
    var source = current is not null
        ? status.Source == "replay" ? "replay" : status.State == "running" && status.Report?.FirstTimestampMicroseconds is { } started && current.ObservedAtMicroseconds >= started ? "live-capture" : "last-observed"
        : character is not null ? "last-observed" : "none";
    string? Name(string? mapId) => mapId is null ? null : catalog.Find(mapId)?.Name
        ?? (mapId.Contains("@RANDOMDUNGEON@", StringComparison.OrdinalIgnoreCase) ? "Random dungeon" : mapId.StartsWith("@ISLAND@", StringComparison.OrdinalIgnoreCase) ? "Player island" : mapId);
    return Results.Ok(new
    {
        source, captureState = status.State,
        party = (status.Report?.Party ?? context.Party) is { } roster ? new
        {
            roster.Complete, roster.ObservedAtMicroseconds, roster.Source,
            confirmed = roster.IsConfirmedSince(status.Report?.FirstTimestampMicroseconds),
            members = roster.Members.Select(member => new { member.Id, member.Name, membership = member.Id == character?.CharacterId || roster.IsConfirmedSince(status.Report?.FirstTimestampMicroseconds) ? "active" : "unconfirmed", weapon = catalog.WeaponName(status.Report?.Weapons?.GetValueOrDefault(member.Id)) })
        } : null,
        partySource = status.Report?.Party is not null ? status.Source == "replay" ? "replay" : status.State == "running" ? "live-capture" : "last-observed" : context.Party is not null ? "last-observed" : "none",
        character = character is null ? null : new
        {
            character.CharacterId, character.Name, character.Guild, character.Alliance,
            character.MapId, mapName = Name(character.MapId), character.DestinationMapId,
            destinationMapName = Name(character.DestinationMapId), character.LocationState,
            character.ObservedAtMicroseconds
        }
    });
});
app.MapGet("/api/storage", (EncounterStore store) => new
{
    databasePath = store.DatabasePath, databaseBytes = new FileInfo(store.DatabasePath).Length,
    encounterCount = store.List().Count, dungeonCount = store.ListDungeons().Count, retention = 100
});
app.MapGet("/api/dungeons", (MeterSession session, EncounterStore store) =>
{
    var current = session.Dungeons().Where(run => run.Status != "idle").ToArray();
    var list = store.ListDungeons().Where(run => !current.Any(active => active.Id == run.Id)).ToList();
    list.InsertRange(0, current.OrderByDescending(run => run.StartedAt).Select(EncounterStore.Summarize));
    return list.Take(100);
});
app.MapGet("/api/dungeons/{id}", (string id, MeterSession session, EncounterStore store) =>
{
    var run = session.Dungeons().FirstOrDefault(run => run.Id == id) ?? store.GetDungeon(id);
    return run is null ? Results.NotFound() : Results.Ok(run);
});
app.MapPost("/api/session/{action}", async (string action, MeterSession session, IHubContext<MeterHub> hub) =>
{
    try
    {
        var snapshot = session.Command(action);
        await hub.Clients.All.SendAsync("Snapshot", snapshot);
        return Results.Ok(snapshot);
    }
    catch (ArgumentException exception) { return Results.BadRequest(new { error = exception.Message }); }
    catch (InvalidOperationException exception) { return Results.Conflict(new { error = exception.Message }); }
});
app.MapGet("/api/albionlab", (AlbionLabService albionLab) => albionLab.Status());
app.MapPost("/api/albionlab/connect", async (AlbionLabService albionLab, CancellationToken cancellation) =>
{
    try { return Results.Ok(await albionLab.Connect(cancellation)); }
    catch (HttpRequestException exception) { return Results.Json(new { error = $"AlbionLab is unreachable: {exception.Message}" }, statusCode: StatusCodes.Status502BadGateway); }
});
app.MapPost("/api/albionlab/disconnect", async (AlbionLabService albionLab) => { await albionLab.Disconnect(); return Results.Ok(albionLab.Status()); });
app.MapPost("/api/albionlab/location", (AlbionLabSettingsRequest request, AlbionLabService albionLab) => Results.Ok(albionLab.ShareLocation(request.ShareLocation)));
app.MapPost("/api/albionlab/session", async (AlbionLabSessionRequest request, AlbionLabService albionLab, CancellationToken cancellation) =>
{
    if (request.Visibility is not (null or "private" or "unlisted" or "public")) return Results.BadRequest(new { error = "Unknown visibility." });
    if (request.Title is { Length: > 80 }) return Results.BadRequest(new { error = "The name is limited to 80 characters." });
    try { return Results.Ok(await albionLab.UpdateSession(request.Title, request.Visibility, request.Anonymize, cancellation)); }
    catch (HttpRequestException exception) { return Results.Json(new { error = $"AlbionLab is unreachable: {exception.Message}" }, statusCode: StatusCodes.Status502BadGateway); }
});
app.MapHub<MeterHub>("/stream");
app.MapFallbackToFile("index.html");
app.Lifetime.ApplicationStarted.Register(() =>
{
    var address = app.Services.GetRequiredService<Microsoft.AspNetCore.Hosting.Server.IServer>()
        .Features.Get<Microsoft.AspNetCore.Hosting.Server.Features.IServerAddressesFeature>()?.Addresses.FirstOrDefault() ?? "http://127.0.0.1:5187";
    var storage = app.Configuration["Storage:Directory"] ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "AlbionMeter");
    Console.WriteLine();
    Console.WriteLine("  ==============================================");
    Console.WriteLine($"   Albion Meter v{version} is running");
    Console.WriteLine();
    Console.WriteLine($"   Open in your browser:  {address.TrimEnd('/')}/");
    Console.WriteLine();
    Console.WriteLine($"   Data folder: {storage}");
    Console.WriteLine("   Keep this window open. Press Ctrl+C to stop.");
    Console.WriteLine("  ==============================================");
    Console.WriteLine();
});
if (args.Contains("--open", StringComparer.Ordinal))
{
    app.Lifetime.ApplicationStarted.Register(() =>
    {
        try { Process.Start(new ProcessStartInfo("http://127.0.0.1:5187") { UseShellExecute = true }); }
        catch (Exception exception) { app.Logger.LogWarning(exception, "Open http://127.0.0.1:5187 in your browser."); }
    });
}
try { app.Run(); }
catch (Exception exception) when (exception is Microsoft.AspNetCore.Connections.AddressInUseException || exception.InnerException is Microsoft.AspNetCore.Connections.AddressInUseException)
{ return PortInUse(); }
return 0;
