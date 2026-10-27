namespace AlbionMeter.Server;

public static class CaptureStorage
{
    public static long RemoveLegacyRecordings(string directory)
    {
        if (!Directory.Exists(directory)) return 0;
        long removed = 0;
        foreach (var path in Directory.EnumerateFiles(directory, "*.pcap"))
        {
            if (!Guid.TryParseExact(Path.GetFileNameWithoutExtension(path), "N", out _)) continue;
            try
            {
                var length = new FileInfo(path).Length;
                File.Delete(path); removed += length;
                if (File.Exists(path + ".json")) { var reportLength = new FileInfo(path + ".json").Length; File.Delete(path + ".json"); removed += reportLength; }
            }
            catch (Exception exception) when (exception is IOException or UnauthorizedAccessException) { }
        }
        var controls = Path.Combine(directory, "record-controls.jsonl");
        try { if (File.Exists(controls)) { var length = new FileInfo(controls).Length; File.Delete(controls); removed += length; } }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException) { }
        return removed;
    }
}
