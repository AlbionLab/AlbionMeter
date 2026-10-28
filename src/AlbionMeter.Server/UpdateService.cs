using System.Net.Http.Json;
using System.Text.Json.Serialization;
using Velopack;
using Velopack.Sources;

namespace AlbionMeter.Server;

public sealed record UpdateStatus(string Current, bool Installed, string State, string? Available, string? Error);

/// <summary>
/// Keeps Albion Meter up to date from the GitHub releases. The Windows installer (Velopack) downloads a newer version in
/// the background and applies it the next time the meter opens, or right away when the user asks; a ZIP copy only
/// learns that a newer version exists, so the page can point to the download.
/// </summary>
public sealed class UpdateService(ILogger<UpdateService> logger) : BackgroundService
{
    public const string Repository = "https://github.com/AlbionLab/AlbionMeter";
    static readonly TimeSpan CheckEvery = TimeSpan.FromHours(6);
    readonly object gate = new();
    readonly UpdateManager? manager = Create();
    UpdateInfo? ready;
    UpdateStatus status = new(Program.Version, false, "idle", null, null);

    static UpdateManager? Create()
    {
        try { return new UpdateManager(new GithubSource(Repository, null, false)); }
        catch (Exception exception) when (exception is not OutOfMemoryException) { return null; }
    }

    public bool Installed => manager?.IsInstalled == true;

    public UpdateStatus Status { get { lock (gate) return status with { Installed = Installed }; } }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        try
        {
            await Task.Delay(TimeSpan.FromSeconds(20), stoppingToken);
            while (!stoppingToken.IsCancellationRequested)
            {
                await Check(stoppingToken);
                await Task.Delay(CheckEvery, stoppingToken);
            }
        }
        catch (OperationCanceledException) { }
    }

    async Task Check(CancellationToken cancellation)
    {
        try
        {
            if (manager is { IsInstalled: true })
            {
                if (ready is not null) return;
                Set("checking", null, null);
                var update = await manager.CheckForUpdatesAsync();
                if (update is null) { Set("current", null, null); return; }
                var version = update.TargetFullRelease.Version.ToString();
                Set("downloading", version, null);
                await manager.DownloadUpdatesAsync(update, cancelToken: cancellation);
                lock (gate) ready = update;
                Set("ready", version, null);
                logger.LogInformation("Albion Meter {Version} is downloaded and will be installed on the next start", version);
            }
            else
            {
                Set("checking", null, null);
                var latest = await Latest(cancellation);
                var newer = latest is not null && Newer(latest, Program.Version);
                Set(newer ? "available" : "current", newer ? latest : null, null);
            }
        }
        catch (Exception exception) when (exception is HttpRequestException or IOException or InvalidOperationException or TaskCanceledException or System.Text.Json.JsonException)
        {
            Set("failed", null, exception.Message);
        }
    }

    /// <summary>Installs the downloaded version and starts it; the meter process ends.</summary>
    public bool ApplyAndRestart()
    {
        UpdateInfo? update;
        lock (gate) update = ready;
        if (manager is not { IsInstalled: true } || update is null) return false;
        manager.ApplyUpdatesAndRestart(update.TargetFullRelease);
        return true;
    }

    void Set(string state, string? available, string? error)
    {
        lock (gate) status = new(Program.Version, Installed, state, available ?? (state is "checking" or "failed" ? status.Available : null), error);
    }

    sealed record GithubLatest([property: JsonPropertyName("tag_name")] string TagName);

    static async Task<string?> Latest(CancellationToken cancellation)
    {
        using var client = new HttpClient { Timeout = TimeSpan.FromSeconds(15) };
        client.DefaultRequestHeaders.UserAgent.ParseAdd($"AlbionMeter/{Program.Version}");
        var latest = await client.GetFromJsonAsync<GithubLatest>("https://api.github.com/repos/AlbionLab/AlbionMeter/releases/latest", cancellation);
        return latest?.TagName.TrimStart('v');
    }

    static bool Newer(string candidate, string current) =>
        Version.TryParse(candidate, out var next) && Version.TryParse(current, out var running) && next > running;
}
