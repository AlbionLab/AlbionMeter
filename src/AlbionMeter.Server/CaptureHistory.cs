using System.Text.Json;

namespace AlbionMeter.Server;

public static class CaptureHistory
{
    public static string[] LocalFiles(string directory)
    {
        if (!Directory.Exists(directory)) return [];
        var result = new List<string>();
        foreach (var path in Directory.EnumerateFiles(directory, "*.pcap").OrderBy(File.GetLastWriteTimeUtc))
        {
            try
            {
                if (new FileInfo(path).Length > 128_000_000 || !File.Exists(path + ".json")) continue;
                using var metadata = JsonDocument.Parse(File.ReadAllText(path + ".json"));
                if (metadata.RootElement.TryGetProperty("session", out var session)
                    && session.TryGetProperty("source", out var source) && source.GetString() == "live-capture") result.Add(path);
            }
            catch (Exception exception) when (exception is IOException or UnauthorizedAccessException or JsonException) { }
        }
        var cutoff = DateTime.UtcNow - TimeSpan.FromHours(24);
        long budget = 512_000_000;
        var recent = result.Where(path => File.GetLastWriteTimeUtc(path) >= cutoff).Reverse()
            .TakeWhile(path => (budget -= new FileInfo(path).Length) >= 0).Reverse();
        return recent.ToArray();
    }
}
