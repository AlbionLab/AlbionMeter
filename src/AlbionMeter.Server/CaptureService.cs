using System.Diagnostics;
using System.Text.Json;
using AlbionMeter.Core.Capture;

namespace AlbionMeter.Server;

public sealed record CaptureSessionStatus(string State, string Source, string? File, string? Error,
    CaptureReport? Report, AdapterStatistics? Adapter, string? Library, string? StopReason);

public sealed class CaptureService(IConfiguration configuration, CharacterContextStore? characterContext = null, AlbionMeter.Core.CombatSourceCatalog? sources = null, EncounterStore? store = null) : IHostedService
{
    private readonly CharacterContextStore _characterContext = characterContext ?? new(configuration);
    private readonly object _gate = new();
    private readonly object _analysisGate = new();
    private CaptureAnalysis? _liveAnalysis;
    private string? _device;
    private CaptureSessionStatus _status = new("idle", "none", null, null, null, null, null, null);
    private CancellationTokenSource? _cancellation;
    private Task? _worker;
    private readonly string _directory = Path.Combine(configuration["Storage:Directory"]
        ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "AlbionMeter"), "captures");
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web) { WriteIndented = true };
    private sealed record CaptureSettings(string? Device);
    private string SettingsPath => Path.Combine(Path.GetDirectoryName(_directory)!, "capture-settings.json");
    public string? SelectedDevice { get { lock (_gate) return _device; } }
    public bool DebugEnabled => configuration.GetValue<bool>("Capture:Debug");
    private bool _stopRequested;
    public CaptureSessionStatus Status { get { lock (_gate) return _status; } }
    private void Publish(CaptureSessionStatus value)
    {
        if (value.Source == "live-capture" && value.Report?.Character is { } character) _characterContext.Observe(character);
        if (value.Source == "live-capture" && value.Report?.Party is { } party) _characterContext.ObserveParty(party);
        lock (_gate) _status = value;
    }
    public CaptureSessionStatus Start(string device, bool recordPrepared = false, bool resumeRecord = false)
    {
        if (!PacketCapture.Devices().Any(value => value.Name == device)) throw new ArgumentException("Select an available capture interface.", nameof(device));
        lock (_gate)
        {
            Reserve("live-capture");
            var token = _cancellation!.Token;
            _device = device;
            _stopRequested = false;
            SaveDevice(device);
            _worker = Task.Run(() => RunLive(device, token, recordPrepared, resumeRecord));
            return _status;
        }
    }
    private CaptureAnalysis CreateLiveAnalysis()
    {
        var analysis = new CaptureAnalysis(sources, manualRecording: true);
        if (store is not null) { analysis.SeedCharacters(store.KnownCharacters()); analysis.SeedSkillWeapons(store.KnownSkillWeapons()); }
        if (_characterContext.Snapshot is { } character) { analysis.RestoreCharacter(character); analysis.RestorePlayers(_characterContext.RecentPlayers(character)); }
        if (_characterContext.Party is { } party) analysis.RestoreParty(party);
        return analysis;
    }
    private string RecordDevice()
    {
        if (_device is not null) return _device;
        var devices = PacketCapture.Devices();
        var saved = File.Exists(SettingsPath) ? JsonSerializer.Deserialize<CaptureSettings>(File.ReadAllText(SettingsPath), JsonOptions)?.Device : null;
        return devices.FirstOrDefault(device => device.Name == saved)?.Name ?? PacketCapture.DefaultDevice(devices)
            ?? throw new InvalidOperationException("Choose a capture interface in Data first.");
    }
    public CaptureSessionStatus StartRecord()
    {
        lock (_analysisGate)
        {
            var current = Status;
            if (current.State is "starting" or "stopping") throw new InvalidOperationException("Wait for the capture operation to finish.");
            var device = RecordDevice();
            _liveAnalysis ??= CreateLiveAnalysis();
            var restartCapture = current.State != "running" || current.Source != "live-capture";
            if (_liveAnalysis.RecordState is "running" or "paused") throw new InvalidOperationException("Stop the current record before starting another one.");
            if (restartCapture) _liveAnalysis.BeginCapture(preserveRecord: true, keepIdentities: _reconnecting);
            _liveAnalysis.StartRecord(DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() * 1000);
            // A capture that cannot start leaves no record "running" without packets: Play is available again.
            if (restartCapture)
                try { Start(device, recordPrepared: true, resumeRecord: true); }
                catch { _liveAnalysis.StopRecord(DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() * 1000); throw; }
            var next = Status with { Report = _liveAnalysis.Snapshot() };
            Publish(next); return next;
        }
    }
    public CaptureSessionStatus StopRecord(Action<CaptureSessionStatus> archive)
    {
        lock (_analysisGate)
        {
            if (_liveAnalysis is null) throw new InvalidOperationException("There is no active record to stop.");
            _liveAnalysis.StopRecord(DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() * 1000);
            _reconnecting = false; _resumeAfterReconnect = false;
            Publish(Status with { Report = _liveAnalysis.Snapshot() });
        }
        // Stop ends the record, not the capture: it keeps reading the game, so the next Play already knows every player.
        var current = Status;
        archive(current);
        return current;
    }
    public CaptureSessionStatus ResetRecord()
    {
        lock (_analysisGate)
        {
            if (_liveAnalysis is null) throw new InvalidOperationException("There is no active record to reset.");
            _liveAnalysis.ResetCounts(DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() * 1000);
            var next = Status with { Report = _liveAnalysis.Snapshot() };
            Publish(next); return next;
        }
    }
    public CaptureSessionStatus NewRecord(Action<CaptureSessionStatus> archive)
    {
        if (_liveAnalysis?.RecordState is "running" or "paused") StopRecord(archive);
        return StartRecord();
    }
    public CaptureSessionStatus SetRecordPaused(bool paused)
    {
        lock (_analysisGate)
        {
            var current = Status;
            // While the capture is coming back after a drop the record waits paused; Pause and Resume only change
            // whether it continues by itself once the capture is back.
            if (_reconnecting && _liveAnalysis is { RecordState: "paused" })
            {
                _resumeAfterReconnect = !paused;
                return current with { Report = _liveAnalysis.Snapshot() };
            }
            if (current.State != "running" || current.Source != "live-capture" || _liveAnalysis is null)
                throw new InvalidOperationException("Press Play to start a record first.");
            if (_liveAnalysis.RecordState != (paused ? "running" : "paused")) throw new InvalidOperationException(paused ? "The record is not running." : "The record is not paused.");
            var at = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() * 1000;
            _liveAnalysis.SetPaused(paused, at);
            var next = current with { Report = _liveAnalysis.Snapshot() };
            Publish(next); return next;
        }
    }
    private void AppendControl(RecordControl control)
    {
        if (DebugEnabled) new RecordControls(_directory).Append(control);
    }
    private void Reserve(string source)
    {
        if (_status.State is "starting" or "running" or "stopping") throw new InvalidOperationException("Stop the current capture before starting another operation.");
        Directory.CreateDirectory(_directory);
        if (!DebugEnabled) CaptureStorage.RemoveLegacyRecordings(_directory);
        _cancellation?.Dispose(); _cancellation = new();
        _status = new("starting", source, null, null, null, null, null, null);
    }
    private void RunLive(string device, CancellationToken token, bool recordPrepared, bool resumeRecord)
    {
        var continuing = _liveAnalysis is not null;
        var analysis = _liveAnalysis ?? CreateLiveAnalysis();
        try
        {
            if (!recordPrepared) analysis.BeginCapture(preserveRecord: continuing || resumeRecord, keepIdentities: continuing && _reconnecting);
            lock (_analysisGate)
            {
                _liveAnalysis = analysis;
                // Back after a drop: the record goes on where it was, unless it was paused before or meanwhile.
                if (_reconnecting && _resumeAfterReconnect && analysis.RecordState == "paused") analysis.SetPaused(false, DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() * 1000);
                _reconnecting = false; _resumeAfterReconnect = false;
            }
            if (OperatingSystem.IsWindows() && device.StartsWith(PacketCapture.RawSocketPrefix, StringComparison.Ordinal)) WindowsFirewall.AllowCapture();
            using var capture = PacketCapture.Open(device);
            using var debug = new PacketDebugStore(configuration);
            var library = PacketCapture.Library(device);
            var clock = Stopwatch.StartNew();
            var nextPublish = 0d;
            var nextPlayers = 10d;
            var reason = "stopped";
            while (!token.IsCancellationRequested)
            {
                if (!capture.Read(out var frame)) { reason = "sourceEnded"; break; }
                if (frame is not null)
                {
                    lock (_analysisGate) analysis.Add(frame);
                    debug.Add(frame);
                }
                if (clock.Elapsed.TotalSeconds >= nextPlayers)
                {
                    lock (_analysisGate) if (analysis.Snapshot().Character is { } character) _characterContext.ObservePlayers(character, analysis.KnownPlayers());
                    nextPlayers = clock.Elapsed.TotalSeconds + 10;
                }
                if (clock.Elapsed.TotalSeconds >= nextPublish)
                {
                    debug.Flush();
                    lock (_analysisGate) analysis.AdvanceClock(DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() * 1000);
                    lock (_analysisGate) Publish(new("running", "live-capture", null, null, analysis.Snapshot(), capture.Statistics(), library, null));
                    nextPublish = clock.Elapsed.TotalSeconds + 0.2;
                }
            }
            // The capture source ending by itself (an interface hiccup, the game reconnecting) is not the player stopping:
            // the record is kept, paused, and continues when the capture comes back. Only a requested stop ends it.
            var dropped = !token.IsCancellationRequested && reason == "sourceEnded";
            lock (_analysisGate) { if (dropped) HoldRecord(analysis); else if (analysis.RecordState is "running" or "paused") analysis.StopRecord(DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() * 1000); analysis.SealRecord(); Complete(new("completed", "live-capture", null, null, analysis.Snapshot(), capture.Statistics(), library, reason)); }
            if (!token.IsCancellationRequested && reason == "sourceEnded") RestartLater(device);
        }
        catch (Exception exception)
        {
            var permission = exception.Message.Contains("permission", StringComparison.OrdinalIgnoreCase) || exception.Message.Contains("CAP_NET_RAW", StringComparison.Ordinal);
            lock (_analysisGate)
            {
                if (!token.IsCancellationRequested && !permission) HoldRecord(analysis);
                // Without permission the capture does not come back by itself: the record ends instead of waiting forever.
                else if (permission && analysis.RecordState is "running" or "paused") analysis.StopRecord(DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() * 1000);
                Complete(new("failed", "live-capture", null, exception.Message, analysis.Snapshot(), null, null, "error"));
            }
            if (permission) Console.Error.WriteLine($"Packet capture failed: {exception.Message}");
            if (!token.IsCancellationRequested && !permission) RestartLater(device);
        }
    }
    public Task<CaptureSessionStatus> ReplayAsync(Stream source, CancellationToken cancellationToken)
    {
        lock (_gate)
        {
            Reserve("replay");
            var token = _cancellation!.Token;
            var task = ReplayCoreAsync(source, token, cancellationToken);
            _worker = task;
            return task;
        }
    }
    private async Task<CaptureSessionStatus> ReplayCoreAsync(Stream source, CancellationToken stop, CancellationToken request)
    {
        var path = Path.Combine(Path.GetTempPath(), $"albion-replay-{Guid.NewGuid():N}.pcap");
        using var linked = CancellationTokenSource.CreateLinkedTokenSource(stop, request);
        var token = linked.Token;
        var analysis = new CaptureAnalysis(sources);
        try
        {
            await using (var output = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None, 65536, true))
            {
                var buffer = new byte[65536];
                int read;
                while ((read = await source.ReadAsync(buffer, token)) > 0)
                {
                    if (output.Length + read > 40_000_000) throw new InvalidDataException("Replay files are limited to 40 MB.");
                    await output.WriteAsync(buffer.AsMemory(0, read), token);
                }
            }
            return await Task.Run(() =>
            {
                using var capture = PacketCapture.OpenFile(path);
                using var debug = new PacketDebugStore(configuration);
                var count = 0;
                while (!token.IsCancellationRequested && capture.Read(out var frame))
                {
                    if (frame is null) continue;
                    analysis.Add(frame);
                    debug.Add(frame);
                    if (++count % 1000 == 0)
                        Publish(new("running", "replay", null, null, analysis.Snapshot(), null, "Albion Meter PCAP reader", null));
                }
                return Complete(new("completed", "replay", null, null, analysis.Snapshot(), null, "Albion Meter PCAP reader",
                    token.IsCancellationRequested ? "stopped" : "endOfFile"));
            }, CancellationToken.None);
        }
        catch (Exception exception)
        {
            return Complete(new("failed", "replay", null, exception.Message, analysis.Snapshot(), null, null,
                exception is OperationCanceledException ? "cancelled" : "error"));
        }
        finally
        {
            try { File.Delete(path); } catch (IOException) { }
        }
    }
    private CaptureSessionStatus Complete(CaptureSessionStatus status)
    {
        Publish(status);
        return status;
    }
    public async Task<CaptureSessionStatus> StopAsync()
    {
        Task? task;
        lock (_gate)
        {
            if (_status.State is "starting" or "running") _status = _status with { State = "stopping" };
            _stopRequested = true;
            _cancellation?.Cancel(); task = _worker;
        }
        if (task is not null) await task;
        return Status;
    }
    public Task StartAsync(CancellationToken cancellationToken)
    {
        if (!configuration.GetValue<bool>("Capture:AutoStart")) return Task.CompletedTask;
        try
        {
            var saved = File.Exists(SettingsPath) ? JsonSerializer.Deserialize<CaptureSettings>(File.ReadAllText(SettingsPath), JsonOptions)?.Device : null;
            var devices = PacketCapture.Devices();
            var device = devices.Any(item => item.Name == saved) ? saved : PacketCapture.DefaultDevice(devices);
            if (device is not null) Start(device);
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException or JsonException or DllNotFoundException
            or EntryPointNotFoundException or PlatformNotSupportedException or InvalidOperationException) { }
        return Task.CompletedTask;
    }
    private void SaveDevice(string device)
    {
        try { Directory.CreateDirectory(Path.GetDirectoryName(SettingsPath)!); File.WriteAllText(SettingsPath, JsonSerializer.Serialize(new CaptureSettings(device), JsonOptions)); }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException) { }
    }
    private bool _reconnecting, _resumeAfterReconnect;
    /// <summary>
    /// Marks a capture that dropped so its restart keeps every identity it knew, with or without a record, and pauses a
    /// running record, remembering to continue it once the capture is back.
    /// </summary>
    private void HoldRecord(CaptureAnalysis analysis)
    {
        _reconnecting = true;
        _resumeAfterReconnect = analysis.RecordState == "running";
        if (_resumeAfterReconnect) analysis.SetPaused(true, DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() * 1000);
    }
    private void RestartLater(string device)
    {
        _ = Task.Delay(3_000).ContinueWith(_ =>
        {
            lock (_gate)
            {
                if (_stopRequested || _status.State is "starting" or "running" or "stopping") return;
                try { Start(device, resumeRecord: true); } catch (InvalidOperationException) { }
            }
        }, TaskScheduler.Default);
    }
    public async Task StopAsync(CancellationToken cancellationToken) => await StopAsync().WaitAsync(cancellationToken);
}
