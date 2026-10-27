using AlbionMeter.Core.Capture;

namespace AlbionMeter.Server;

public static class CaptureEndpoints
{
    public sealed record StartCaptureRequest(string Device, string? Method = null);
    public static void MapCaptureEndpoints(this WebApplication app)
    {
        app.MapGet("/api/capture/status", (CaptureService capture) => capture.Status);
        app.MapGet("/api/capture/devices", (CaptureService capture) =>
        {
            try
            {
                var devices = PacketCapture.Devices();
                return Results.Ok(new
                {
                    available = devices.Length > 0, library = PacketCapture.Library(devices.FirstOrDefault()?.Name),
                    debugEnabled = capture.DebugEnabled,
                    selectedDevice = capture.SelectedDevice ?? PacketCapture.DefaultDevice(devices),
                    methods = new[]
                    {
                        new { id = "socket", name = OperatingSystem.IsLinux() ? "System socket (Linux)" : "System socket (Windows)", available = devices.Any(device => device.Name.StartsWith(PacketCapture.RawSocketPrefix, StringComparison.Ordinal)) },
                        new { id = "pcap", name = OperatingSystem.IsWindows() ? "Npcap" : "libpcap", available = devices.Any(device => !device.Name.StartsWith(PacketCapture.RawSocketPrefix, StringComparison.Ordinal)) }
                    },
                    devices = devices.Select(device => new { device.Name, device.Description, device.Flags, method = device.Name.StartsWith(PacketCapture.RawSocketPrefix, StringComparison.Ordinal) ? "socket" : "pcap" }),
                    error = devices.Length > 0 ? null : "No capture interface is available."
                });
            }
            catch (Exception exception) when (exception is IOException or DllNotFoundException or EntryPointNotFoundException or PlatformNotSupportedException)
            { return Results.Ok(new { available = false, library = (string?)null, devices = Array.Empty<CaptureDevice>(), debugEnabled = capture.DebugEnabled, error = exception.Message }); }
        });
        app.MapPost("/api/capture/start", (StartCaptureRequest request, CaptureService capture) =>
        {
            if (string.IsNullOrWhiteSpace(request.Device) || request.Device.Length > 1024) return Results.BadRequest(new { error = "Select a capture interface." });
            if (request.Method is not null && (request.Method is not ("socket" or "pcap") || (request.Method == "socket") != request.Device.StartsWith(PacketCapture.RawSocketPrefix, StringComparison.Ordinal)))
                return Results.BadRequest(new { error = "The selected interface does not match the capture method." });
            try { return Results.Ok(capture.Start(request.Device)); }
            catch (ArgumentException exception) { return Results.BadRequest(new { error = exception.Message }); }
            catch (InvalidOperationException exception) { return Results.Conflict(new { error = exception.Message }); }
        });
        app.MapPost("/api/capture/stop", async (CaptureService capture) => Results.Ok(await capture.StopAsync()));
        app.MapPost("/api/capture/replay", async (HttpRequest request, CaptureService capture, CancellationToken token) =>
        {
            if (!request.HasFormContentType) return Results.BadRequest(new { error = "Choose a PCAP or PCAPNG file." });
            var form = await request.ReadFormAsync(token);
            var file = form.Files.GetFile("capture");
            if (file is null || file.Length is 0 or > 40_000_000) return Results.BadRequest(new { error = "Choose a capture file smaller than 40 MB." });
            try
            {
                await using var source = file.OpenReadStream();
                return Results.Ok(await capture.ReplayAsync(source, token));
            }
            catch (InvalidOperationException exception) { return Results.Conflict(new { error = exception.Message }); }
        });
    }
}
