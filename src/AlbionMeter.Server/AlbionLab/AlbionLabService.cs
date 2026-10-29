using System.Net;
using System.Net.Http.Headers;
using System.Net.Http.Json;
using System.Text.Json;
using AlbionLab.Protocol;
using AlbionMeter.Core;
using Microsoft.AspNetCore.SignalR;
using Microsoft.AspNetCore.SignalR.Client;

namespace AlbionMeter.Server;

public sealed record AlbionLabAccount(string Name, string AvatarUrl);
public sealed record AlbionLabPairing(string Code, string VerifyUrl, DateTimeOffset ExpiresAt);
public sealed record AlbionLabSession(string Id, string Url, string Visibility, bool Anonymize, string State, string? Error, DateTimeOffset? PublishedAt, string? Title = null);
public sealed record AlbionLabStatus(string Server, bool Connected, AlbionLabAccount? Account, AlbionLabPairing? Pairing, string? PairingError, AlbionLabSession? Session, bool ShareLocation, string Visibility, bool Anonymize, string? SuggestedTitle = null);

public sealed class AlbionLabService(IConfiguration configuration, MeterSession meter, CaptureService capture, CombatSourceCatalog sources, MapCatalog catalog, ILogger<AlbionLabService> logger) : BackgroundService
{
    sealed class Settings
    {
        public string? DeviceToken { get; set; }
        public string? AccountName { get; set; }
        public string? AvatarUrl { get; set; }
        public bool ShareLocation { get; set; }
        public string Visibility { get; set; } = "unlisted";
        public bool Anonymize { get; set; }
    }

    static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web);
    readonly HttpClient http = new() { Timeout = TimeSpan.FromSeconds(15) };
    readonly Lock gate = new();
    readonly string server = Server(configuration["AlbionLab:Url"]);
    readonly string settingsPath = Path.Combine(configuration["Storage:Directory"] ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "AlbionMeter"), "albionlab.json");
    Settings? settings;
    AlbionLabPairing? pairing;
    string? pollToken;
    string? pairingError;
    HubConnection? hub;
    SessionCompactor compactor = new();
    string? recordId;
    string? finishedRecord;
    AlbionLabSession? session;
    (DateTimeOffset StartedAt, string? Self, SourceSnapshot Record, List<SourceSnapshot> Encounters, List<SourceDungeon> Dungeons)? lastPublished;

    Settings Current
    {
        get
        {
            if (settings is not null) return settings;
            try { settings = File.Exists(settingsPath) ? JsonSerializer.Deserialize<Settings>(File.ReadAllText(settingsPath), Json) ?? new() : new(); }
            catch (Exception error) when (error is IOException or JsonException) { settings = new(); }
            return settings;
        }
    }

    void Save()
    {
        Directory.CreateDirectory(Path.GetDirectoryName(settingsPath)!);
        File.WriteAllText(settingsPath, JsonSerializer.Serialize(Current, Json));
        if (!OperatingSystem.IsWindows()) File.SetUnixFileMode(settingsPath, UnixFileMode.UserRead | UnixFileMode.UserWrite);
    }

    public AlbionLabStatus Status()
    {
        lock (gate)
            return new(server, Current.DeviceToken is not null, Current.DeviceToken is null ? null : new(Current.AccountName ?? "", Current.AvatarUrl ?? ""),
                pairing is { } pending && pending.ExpiresAt > DateTimeOffset.UtcNow ? pending : null, pairingError, session, Current.ShareLocation, Current.Visibility, Current.Anonymize,
                lastPublished is { } published ? SuggestTitle(published.Self, published.Record, published.Dungeons) : null);
    }

    /// <summary>
    /// A session name from what was recorded: the dungeon or expedition runs, their entrance when the location is shared
    /// (unshared runs carry none), and the party size; without runs, the recording player and weapon family.
    /// </summary>
    public static string? SuggestTitle(string? self, SourceSnapshot record, IReadOnlyList<SourceDungeon> runs)
    {
        var count = record.Members.Count;
        var party = count <= 1 ? "solo" : $"{count} players";
        string title;
        if (runs.Count > 0)
        {
            var names = string.Join(" + ", runs.GroupBy(run => run.Name).Select(group => group.Count() > 1 ? $"{group.Key} ×{group.Count()}" : group.Key));
            var entrance = runs.Select(run => run.Entrance).FirstOrDefault(name => !string.IsNullOrWhiteSpace(name));
            title = string.Join(" · ", new[] { names, entrance, party }.Where(part => !string.IsNullOrWhiteSpace(part)));
        }
        else if (count > 1) title = $"Group fight · {party}";
        else if ((record.Members.FirstOrDefault(member => member.Id == self) ?? record.Members.FirstOrDefault()) is { } player)
            title = player.Family is { Length: > 0 } family && family is not ("unknown" or "seen in combat") ? $"{player.Name} solo · {family}" : $"{player.Name} solo";
        else return null;
        return title.Length > 80 ? title[..80] : title;
    }

    static string Server(string? configured)
    {
        if (Uri.TryCreate(configured?.TrimEnd('/'), UriKind.Absolute, out var uri)
            && (uri.Scheme == Uri.UriSchemeHttps || uri.Scheme == Uri.UriSchemeHttp && uri.IsLoopback))
            return uri.GetLeftPart(UriPartial.Authority);
        return "https://albionlab.app";
    }

    string VerifyUrl(string? offered, string code) =>
        Uri.TryCreate(offered, UriKind.Absolute, out var uri) && uri.GetLeftPart(UriPartial.Authority) == server
            ? uri.AbsoluteUri : $"{server}/link?code={Uri.EscapeDataString(code)}";

    public async Task<AlbionLabStatus> Connect(CancellationToken cancellation)
    {
        using var response = await http.PostAsJsonAsync($"{server}/api/devices/pair", new { name = $"Albion Meter · {Environment.MachineName}" }, cancellation);
        response.EnsureSuccessStatusCode();
        var body = await response.Content.ReadFromJsonAsync<JsonElement>(cancellation);
        lock (gate)
        {
            pollToken = body.GetProperty("pollToken").GetString();
            var code = body.GetProperty("code").GetString()!;
            pairing = new(code, VerifyUrl(body.GetProperty("verifyUrl").GetString(), code), DateTimeOffset.UtcNow.AddSeconds(body.GetProperty("expiresIn").GetInt32()));
            pairingError = null;
        }
        return Status();
    }

    public async Task Disconnect()
    {
        HubConnection? old;
        lock (gate)
        {
            Current.DeviceToken = null; Current.AccountName = null; Current.AvatarUrl = null;
            Save();
            old = hub; hub = null; session = null; recordId = null; pairing = null; pollToken = null;
        }
        if (old is not null) await old.DisposeAsync();
    }

    public AlbionLabStatus ShareLocation(bool share)
    {
        lock (gate) { Current.ShareLocation = share; Save(); }
        return Status();
    }

    public async Task<AlbionLabStatus> UpdateSession(string? title, string? visibility, bool? anonymize, CancellationToken cancellation)
    {
        lock (gate)
        {
            if (visibility is not null) Current.Visibility = visibility;
            if (anonymize is { } hide) Current.Anonymize = hide;
            Save();
        }
        var (token, current) = (Current.DeviceToken, session);
        if (token is null || current is not { Id.Length: > 0 }) return Status();
        using var request = new HttpRequestMessage(HttpMethod.Patch, $"{server}/api/sessions/{current.Id}") { Content = JsonContent.Create(new { title, visibility, anonymize }) };
        request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", token);
        using var response = await http.SendAsync(request, cancellation);
        if (response.IsSuccessStatusCode)
        {
            var body = await response.Content.ReadFromJsonAsync<JsonElement>(cancellation);
            lock (gate) session = current with { Visibility = body.GetProperty("visibility").GetString()!, Anonymize = body.GetProperty("anonymize").GetBoolean(), Title = body.TryGetProperty("title", out var named) && named.ValueKind == JsonValueKind.String ? named.GetString() : null };
        }
        return Status();
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        using var timer = new PeriodicTimer(TimeSpan.FromSeconds(1));
        while (await timer.WaitForNextTickAsync(stoppingToken))
        {
            try
            {
                await PollPairing(stoppingToken);
                await Publish(stoppingToken);
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested) { break; }
            catch (Exception error)
            {
                logger.LogDebug(error, "AlbionLab publish failed");
                lock (gate) { compactor.Reset(); if (session is { } current) session = current with { State = "offline", Error = error.Message }; }
            }
        }
    }

    async Task PollPairing(CancellationToken cancellation)
    {
        string? token;
        lock (gate) token = pollToken;
        if (token is null || DateTime.UtcNow.Second % 3 != 0) return;
        using var response = await http.PostAsJsonAsync($"{server}/api/devices/poll", new { pollToken = token }, cancellation);
        if (response.StatusCode == HttpStatusCode.Accepted) return;
        var body = await response.Content.ReadFromJsonAsync<JsonElement>(cancellation);
        lock (gate)
        {
            pollToken = null; pairing = null;
            if (response.IsSuccessStatusCode && body.TryGetProperty("deviceToken", out var device))
            {
                Current.DeviceToken = device.GetString();
                var account = body.GetProperty("account");
                Current.AccountName = account.GetProperty("name").GetString();
                Current.AvatarUrl = account.GetProperty("avatarUrl").GetString();
                Save();
            }
            else pairingError = response.StatusCode == HttpStatusCode.Forbidden ? "denied" : "expired";
        }
    }

    DateTimeOffset? publishedStart;
    async Task Publish(CancellationToken cancellation)
    {
        var token = Current.DeviceToken;
        if (token is null) return;
        var record = meter.Snapshot();
        var active = record.Mode == "live" && record.Status is "running" or "paused" or "completed";
        if (!active || record.Id == finishedRecord) return;
        if (record.Id != recordId)
        {
            if (recordId is not null) await FinishPrevious(token, cancellation);
            if (record.Status == "completed") return;
            await CreateSession(token, record, cancellation);
        }
        // Reset keeps the record but starts it again from zero (a new start time): the session is sent again in full.
        else if (publishedStart is { } start && start != record.StartedAt) lock (gate) compactor.Reset();
        publishedStart = record.StartedAt;
        var connection = await Hub(token, cancellation);
        if (connection is null || session is null) return;
        var self = capture.Status.Report?.Character?.CharacterId.ToString();
        var share = Current.ShareLocation;
        var runs = meter.Dungeons().Where(run => run.StartedAt >= record.StartedAt - TimeSpan.FromSeconds(5)).ToList();
        fallback = CombatWeapons(record);
        var encounters = runs.SelectMany(run => run.Floors.SelectMany(floor => floor.Encounters)).Select(Source).ToList();
        var dungeons = runs.Select(run => new SourceDungeon(run.Id, run.Name, run.StartedAt, run.Status, share ? run.EntranceMapName : null,
            [.. run.Floors.Select(floor => new SourceFloor($"floor-{floor.Number}", floor.Number, share ? floor.MapName : null, [.. floor.Encounters.Select(encounter => encounter.Id)]))])).ToList();
        SessionDelta? delta;
        var source = Source(record);
        lock (gate)
        {
            lastPublished = (record.StartedAt, self, source, encounters, dungeons);
            delta = compactor.Next(record.Status, record.StartedAt, self, source, encounters, dungeons);
        }
        if (delta is null) return;
        try
        {
            await connection.InvokeAsync<long>("Publish", session.Id, delta, cancellation);
            lock (gate) session = session with { State = "live", Error = null, PublishedAt = DateTimeOffset.UtcNow };
            if (record.Status == "completed") { finishedRecord = record.Id; lock (gate) session = session with { State = "completed" }; }
        }
        catch (HubException error)
        {
            lock (gate) { compactor.Reset(); session = session with { State = "error", Error = error.Message }; }
            if (error.Message.Contains("ended", StringComparison.Ordinal)) finishedRecord = record.Id;
        }
    }

    async Task FinishPrevious(string token, CancellationToken cancellation)
    {
        AlbionLabSession? previous;
        SessionDelta? delta = null;
        lock (gate)
        {
            previous = session;
            if (previous is { Id.Length: > 0, State: not "completed" } && finishedRecord != recordId && lastPublished is { } last)
                delta = compactor.Next("completed", last.StartedAt, last.Self, last.Record, last.Encounters, last.Dungeons);
        }
        if (delta is not null && previous is not null && await Hub(token, cancellation) is { } connection)
        {
            try { await connection.InvokeAsync<long>("Publish", previous.Id, delta, cancellation); }
            catch (HubException) { }
        }
        lock (gate) { finishedRecord = recordId; recordId = null; session = null; lastPublished = null; compactor = new SessionCompactor(); }
    }

    async Task CreateSession(string token, CombatSnapshot record, CancellationToken cancellation)
    {
        using var request = new HttpRequestMessage(HttpMethod.Post, $"{server}/api/sessions") { Content = JsonContent.Create(new { startedAt = record.StartedAt, visibility = Current.Visibility, anonymize = Current.Anonymize }) };
        request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", token);
        using var response = await http.SendAsync(request, cancellation);
        if (response.StatusCode == HttpStatusCode.Unauthorized) { await Disconnect(); lock (gate) pairingError = "revoked"; return; }
        if (!response.IsSuccessStatusCode)
        {
            var reason = (await response.Content.ReadFromJsonAsync<JsonElement>(cancellation)).TryGetProperty("error", out var error) ? error.GetString() : response.StatusCode.ToString();
            lock (gate) session = new("", "", "unlisted", false, "error", reason, null);
            return;
        }
        var body = await response.Content.ReadFromJsonAsync<JsonElement>(cancellation);
        lock (gate)
        {
            recordId = record.Id;
            compactor = new SessionCompactor();
            session = new(body.GetProperty("id").GetString()!, body.GetProperty("url").GetString()!, body.GetProperty("visibility").GetString()!,
                body.GetProperty("anonymize").GetBoolean(), "live", null, null);
        }
    }

    async Task<HubConnection?> Hub(string token, CancellationToken cancellation)
    {
        HubConnection? connection;
        lock (gate)
        {
            if (hub is null)
            {
                hub = new HubConnectionBuilder()
                    .WithUrl($"{server}{Protocol.HubPathPublish}", options => options.AccessTokenProvider = () => Task.FromResult<string?>(Current.DeviceToken))
                    .AddJsonProtocol(options => options.PayloadSerializerOptions.PropertyNamingPolicy = JsonNamingPolicy.CamelCase)
                    .WithAutomaticReconnect()
                    .Build();
                hub.Reconnected += _ => { lock (gate) compactor.Reset(); return Task.CompletedTask; };
            }
            connection = hub;
        }
        if (connection.State == HubConnectionState.Disconnected)
        {
            try { await connection.StartAsync(cancellation); lock (gate) compactor.Reset(); }
            catch (HttpRequestException error) when (error.StatusCode == HttpStatusCode.Unauthorized)
            {
                await Disconnect();
                lock (gate) pairingError = "revoked";
                return null;
            }
        }
        return connection.State == HubConnectionState.Connected ? connection : null;
    }

    SourceSnapshot Source(CombatSnapshot snapshot)
    {
        var members = snapshot.Members.Select(player => Member(player, player.Membership))
            .Concat((snapshot.FormerMembers ?? []).Select(player => Member(player, "left"))).ToList();
        var weapons = (snapshot.WeaponContributions ?? []).Select(weapon => new SourceWeapon(weapon.Id, weapon.PlayerId, weapon.WeaponIndex, weapon.Weapon,
            weapon.WeaponIndex is { } index ? sources.FindItem(index)?.Id : null)).ToList();
        var items = (snapshot.ItemContributions ?? []).Select(item => new SourceItem(item.Id, item.PlayerId, item.Slot, item.ItemIndex, item.Item, item.Kind, item.TierObserved,
            item.ItemIndex is { } index ? sources.FindItem(index)?.Id : null)).ToList();
        var timeline = snapshot.Timeline.Select(point => new SourceSecond(point.Second, point.Damage, point.Healing, point.WeaponDamage, point.WeaponHealing, point.ItemDamage, point.ItemHealing)).ToList();
        return new(snapshot.Id, snapshot.StartedAt, snapshot.ElapsedSeconds, snapshot.Status, snapshot.EventCount, members, weapons, items, timeline);
    }

    Dictionary<string, ItemContribution> fallback = [];

    static Dictionary<string, ItemContribution> CombatWeapons(CombatSnapshot record) =>
        (record.ItemContributions ?? []).Where(item => item.Slot == "mainhand" && item.ItemIndex is > 0 && item.TierObserved)
            .GroupBy(item => (item.PlayerId, item.ItemIndex))
            .Select(group => group.First() with { Damage = group.Sum(item => item.Damage), Healing = group.Sum(item => item.Healing) })
            .GroupBy(item => item.PlayerId)
            .ToDictionary(group => group.Key, group => group.OrderByDescending(item => item.Damage).ThenByDescending(item => item.Healing).First());

    SourceMember Member(PlayerStats player, string membership)
    {
        var observed = player.Weapon != "Weapon not observed";
        var seen = !observed && fallback.TryGetValue(player.Id, out var weapon) ? weapon : null;
        var label = seen?.Item ?? player.Weapon;
        // A weapon identified from the abilities used still has a family; the site marks it as seen in combat by itself.
        return new(player.Id, player.Name, label, seen is null ? player.WeaponFamily : catalog.WeaponFamily(seen.ItemIndex) ?? player.WeaponFamily, player.Color,
            membership is "left" or "unconfirmed" ? membership : "active", sources.ItemIdByName(label));
    }
}

public sealed record AlbionLabSettingsRequest(bool ShareLocation);
public sealed record AlbionLabSessionRequest(string? Visibility, bool? Anonymize, string? Title = null);
