using System.Text.Json;
using AlbionMeter.Core.Capture;

namespace AlbionMeter.Server;

public sealed record RecordControl(long TimestampMicroseconds, string Action, Guid? CharacterId);

public sealed class RecordControls(string directory)
{
    private readonly string _path = Path.Combine(directory, "record-controls.jsonl");
    private static readonly JsonSerializerOptions Options = new(JsonSerializerDefaults.Web);
    public void Append(RecordControl control)
    {
        Directory.CreateDirectory(directory);
        if (File.Exists(_path) && new FileInfo(_path).Length > 5_000_000) throw new IOException("The record control journal is full.");
        File.AppendAllText(_path, JsonSerializer.Serialize(control, Options) + Environment.NewLine);
    }
    public RecordControl[] Read()
    {
        if (!File.Exists(_path)) return [];
        if (new FileInfo(_path).Length > 5_000_000) throw new IOException("The record control journal is too large.");
        var result = new List<RecordControl>();
        foreach (var line in File.ReadLines(_path))
        {
            if (line.Length > 1000 || result.Count >= 10_000) throw new IOException("The record control journal limit was reached.");
            try
            {
                var value = JsonSerializer.Deserialize<RecordControl>(line, Options);
                if (value is { TimestampMicroseconds: > 0, Action: "pause" or "resume" or "new-record" }) result.Add(value);
            }
            catch (JsonException) { }
        }
        return result.OrderBy(control => control.TimestampMicroseconds).ToArray();
    }
    public static void Apply(CaptureAnalysis analysis, RecordControl control)
    {
        if (control.CharacterId is { } id && analysis.Snapshot().Character?.CharacterId != id) return;
        if (control.Action == "new-record") analysis.BeginRecord(control.TimestampMicroseconds);
        else analysis.SetPaused(control.Action == "pause", control.TimestampMicroseconds);
    }
}
