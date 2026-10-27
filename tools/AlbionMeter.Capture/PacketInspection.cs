using AlbionMeter.Core.Capture;

internal static class PacketInspection
{
    private sealed class EventSummary
    {
        public long Count { get; set; }
        public Dictionary<string, long> Shapes { get; } = [];
        public List<Dictionary<byte, object?>> MembershipSamples { get; } = [];
        public List<Dictionary<byte, object?>> NumericSamples { get; } = [];
    }
    public static object Read(string file)
    {
        using var capture = NativePcap.OpenFile(file);
        var network = new NetworkDecoder();
        var photon = new PhotonTransport();
        var events = new SortedDictionary<long, EventSummary>();
        var statuses = new Dictionary<string, long>();
        long frames = 0, incoming = 0;
        while (capture.Read(out var frame))
        {
            if (frame is null) continue;
            frames++;
            var datagram = network.Decode(frame).Datagram;
            if (datagram is null || !NetworkDecoder.IsGamePort(datagram.SourcePort) || NetworkDecoder.IsGamePort(datagram.DestinationPort)) continue;
            incoming++;
            foreach (var message in photon.Decode(datagram).Messages)
            {
                var decoded = Protocol18.Decode(message.Payload);
                statuses[decoded.Status] = statuses.GetValueOrDefault(decoded.Status) + 1;
                if (decoded.Message is not { Kind: 4 } wire || !wire.Parameters.TryGetValue(252, out var value) || value is not (byte or short or int or long)) continue;
                var code = Convert.ToInt64(value);
                if (!events.TryGetValue(code, out var summary))
                {
                    if (events.Count >= 1024) continue;
                    summary = new(); events.Add(code, summary);
                }
                summary.Count++;
                var shape = string.Join(",", wire.Parameters.OrderBy(p => p.Key).Select(p => $"{p.Key}:{Shape(p.Value)}"));
                if (!summary.Shapes.ContainsKey(shape) && summary.Shapes.Count >= 32) shape = "other";
                summary.Shapes[shape] = summary.Shapes.GetValueOrDefault(shape) + 1;
                if (code is >= 230 and <= 234 && summary.MembershipSamples.Count < 4)
                    summary.MembershipSamples.Add(wire.Parameters);
                if (code is 6 or 7 && summary.NumericSamples.Count < 4)
                    summary.NumericSamples.Add(wire.Parameters.Where(p => p.Key <= 7).ToDictionary(p => p.Key, p => Numeric(p.Value)));
            }
        }
        return new { frames, incomingDatagrams = incoming, statuses, duplicateCommands = photon.Duplicates, events };
    }
    private static string Shape(object? value) => value switch
    {
        null => "null",
        byte[] bytes => $"bytes[{bytes.Length}]",
        Array array => $"{array.GetType().GetElementType()?.Name}[{array.Length}]",
        string => "string",
        _ => value.GetType().Name
    };
    private static object? Numeric(object? value) => value switch
    {
        byte or short or int or long or float or double or bool => value,
        Array array => array.Cast<object?>().Take(16).Select(item => item is byte or short or int or long or float or double or bool ? item : null).ToArray(),
        _ => null
    };
}
