using System.Diagnostics;
using System.Text.Json;
using AlbionMeter.Core.Capture;

var sourceCatalog = AlbionMeter.Core.CombatSourceCatalog.Load(Path.Combine(AppContext.BaseDirectory, "assets", "combat.json"));
var options = new JsonSerializerOptions(JsonSerializerDefaults.Web) { WriteIndented = true };
try
{
    if (args is ["devices"])
    {
        Console.WriteLine(JsonSerializer.Serialize(new { library = NativePcap.Version, devices = NativePcap.Devices() }, options));
        return 0;
    }
    if (args is ["trace", var traceFile])
    {
        using var capture = NativePcap.OpenFile(traceFile);
        var network = new NetworkDecoder();
        var photon = new PhotonTransport();
        while (capture.Read(out var frame))
        {
            if (frame is null) continue;
            var datagram = network.Decode(frame).Datagram;
            if (datagram is null || !NetworkDecoder.IsGamePort(datagram.SourcePort) || NetworkDecoder.IsGamePort(datagram.DestinationPort)) continue;
            foreach (var message in photon.Decode(datagram).Messages)
            {
                var decoded = Protocol18.Decode(message.Payload);
                if (decoded.Message is not { } wire) continue;
                Console.WriteLine(JsonSerializer.Serialize(new { message.TimestampMicroseconds, message.Connection, message.Identity, wire.Kind, wire.ReturnCode, wire.Parameters }));
            }
        }
        return 0;
    }
    if (args is ["inspect", var inspectionFile])
    {
        Console.WriteLine(JsonSerializer.Serialize(PacketInspection.Read(inspectionFile), options));
        return 0;
    }
    if (args is ["replay-session", var sessionFile, .. var contextFiles])
    {
        var analysis = new CaptureAnalysis(sourceCatalog);
        foreach (var contextFile in contextFiles)
        {
            using var context = NativePcap.OpenFile(contextFile);
            while (context.Read(out var frame)) if (frame is not null) analysis.Add(frame);
        }
        using var capture = NativePcap.OpenFile(sessionFile);
        var started = false;
        while (capture.Read(out var frame))
        {
            if (frame is null) continue;
            if (!started) { analysis.BeginCapture(frame.TimestampMicroseconds); started = true; }
            analysis.Add(frame);
        }
        Console.WriteLine(JsonSerializer.Serialize(analysis.Snapshot(), options));
        return 0;
    }
    if (args is ["replay", var file])
    {
        using var capture = NativePcap.OpenFile(file);
        var analysis = new CaptureAnalysis(sourceCatalog);
        while (capture.Read(out var frame))
        {
            if (frame is not null) analysis.Add(frame);
        }
        Console.WriteLine(JsonSerializer.Serialize(analysis.Snapshot(), options));
        return 0;
    }
    if (args is ["record", var device, var output, var durationText] && int.TryParse(durationText, out var duration) && duration is >= 1 and <= 600)
    {
        using var cancellation = new CancellationTokenSource();
        Console.CancelKeyPress += (_, eventArgs) => { eventArgs.Cancel = true; cancellation.Cancel(); };
        using var capture = NativePcap.OpenDevice(device);
        using var writer = new PcapWriter(output, capture.LinkType);
        var analysis = new CaptureAnalysis(sourceCatalog);
        var clock = Stopwatch.StartNew();
        var stopReason = "duration";
        while (!cancellation.IsCancellationRequested && clock.Elapsed.TotalSeconds < duration)
        {
            if (!capture.Read(out var frame)) break;
            if (frame is null) { Thread.Sleep(10); continue; }
            if (writer.BytesWritten + frame.Data.Length + 16 > 128_000_000) { stopReason = "sizeLimit"; break; }
            writer.Write(frame); analysis.Add(frame);
        }
        writer.Flush();
        if (cancellation.IsCancellationRequested) stopReason = "cancelled";
        var report = new { source = "live-capture", stopReason, filter = NativePcap.DefaultFilter, library = NativePcap.Version,
            file = Path.GetFullPath(output), adapter = capture.Statistics(), report = analysis.Snapshot() };
        var json = JsonSerializer.Serialize(report, options);
        File.WriteAllText(output + ".json", json);
        Console.WriteLine(json);
        return 0;
    }
    Console.Error.WriteLine("Usage: AlbionMeter.Capture devices | replay-session <pcap-file> [context-files...] | trace <pcap-file> | inspect <pcap-file> | replay <pcap-file> | record <device> <new-pcap-file> <seconds:1-600>");
    return 2;
}
catch (Exception exception) when (exception is IOException or UnauthorizedAccessException or ArgumentException or DllNotFoundException or EntryPointNotFoundException or PlatformNotSupportedException)
{
    Console.Error.WriteLine(JsonSerializer.Serialize(new { error = exception.Message, combatDecoded = false }, options));
    return 1;
}
