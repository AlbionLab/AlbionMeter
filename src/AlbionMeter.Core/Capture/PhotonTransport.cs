using System.Buffers.Binary;

namespace AlbionMeter.Core.Capture;

public sealed record PhotonMessage(string Identity, long TimestampMicroseconds, byte[] Payload, string? Connection = null);
public sealed record PhotonTransportResult(string Status, PhotonMessage[] Messages);

public sealed class PhotonTransport
{
    private sealed class Assembly(int length, int fragments, long timestamp)
    {
        public byte[] Bytes { get; } = new byte[length];
        public bool[] Received { get; } = new bool[length];
        public Dictionary<uint, (int Offset, int Length)> Parts { get; } = [];
        public int ExpectedFragments { get; } = fragments;
        public int Count { get; set; }
        public long Timestamp { get; } = timestamp;
    }
    private readonly Dictionary<string, Assembly> _assemblies = [];
    private readonly HashSet<string> _seen = [];
    private readonly Queue<string> _seenOrder = [];
    private long _latestTimestamp;
    public long Duplicates { get; private set; }
    public long ExpiredAssemblies { get; private set; }
    public long EvictedAssemblies { get; private set; }
    public int PendingAssemblies => _assemblies.Count;
    public PhotonTransportResult Decode(UdpDatagram datagram)
    {
        _latestTimestamp = Math.Max(_latestTimestamp, datagram.TimestampMicroseconds);
        foreach (var key in _assemblies.Where(pair => _latestTimestamp - pair.Value.Timestamp > 15_000_000).Select(pair => pair.Key).ToArray())
        { _assemblies.Remove(key); ExpiredAssemblies++; }
        var bytes = datagram.Payload.AsSpan();
        var validation = Validate(bytes);
        if (validation is not null) return new(validation, []);
        var messages = new List<PhotonMessage>();
        while (bytes.Length > 0)
        {
            if (bytes.Length < 12 || bytes[3] == 0) return new("malformedPhoton", []);
            if (bytes[2] != 0) return new(bytes[2] == 1 ? "encryptedPhoton" : "unsupportedPhotonFlags", []);
            var connection = Connection(datagram, BinaryPrimitives.ReadUInt16BigEndian(bytes), U32(bytes[8..]));
            var count = bytes[3];
            var offset = 12;
            var commands = new List<(int Offset, int Length)>();
            for (var i = 0; i < count; i++)
            {
                if (bytes.Length - offset < 12) return new("malformedPhoton", []);
                var length = U32(bytes[(offset + 4)..]);
                if (length < 12 || length > bytes.Length - offset) return new("malformedPhoton", []);
                var type = bytes[offset];
                if (type is < 1 or > 12 || (type == 7 && length < 16) || (type == 8 && length < 32)) return new("malformedPhoton", []);
                commands.Add((offset, (int)length)); offset += (int)length;
            }
            foreach (var command in commands)
            {
                var data = bytes.Slice(command.Offset, command.Length);
                var type = data[0]; var channel = data[1]; var sequence = U32(data[8..]);
                var identity = $"{connection}:{channel}:{type}:{sequence}";
                if (type is 6 or 7)
                {
                    var header = type == 6 ? 12 : 16;
                    if (type == 7) identity += $":{U32(data[12..])}";
                    if (data.Length == header) continue;
                    if (!Remember(identity)) continue;
                    messages.Add(new(identity, datagram.TimestampMicroseconds, data[header..].ToArray(), connection));
                }
                else if (type == 8)
                {
                    var start = U32(data[12..]); var fragmentCount = U32(data[16..]);
                    var number = U32(data[20..]); var total = U32(data[24..]); var position = U32(data[28..]);
                    var payload = data[32..]; var key = $"{connection}:{channel}:8:{start}";
                    if (total is 0 or > 1_048_576 || fragmentCount is 0 or > 4096 || number >= fragmentCount
                        || position >= total || payload.Length == 0 || payload.Length > total - position)
                    { _assemblies.Remove(key); return new("malformedPhotonFragment", messages.ToArray()); }
                    if (_seen.Contains(key)) { Duplicates++; continue; }
                    if (!_assemblies.TryGetValue(key, out var assembly))
                    {
                        if (_assemblies.Count >= 16)
                        { _assemblies.Remove(_assemblies.MinBy(pair => pair.Value.Timestamp).Key); EvictedAssemblies++; }
                        _assemblies[key] = assembly = new((int)total, (int)fragmentCount, _latestTimestamp);
                    }
                    if (assembly.Bytes.Length != total || assembly.ExpectedFragments != fragmentCount
                        || (assembly.Parts.TryGetValue(number, out var previous) && previous != ((int)position, payload.Length)))
                    { _assemblies.Remove(key); return new("photonFragmentConflict", messages.ToArray()); }
                    for (var index = 0; index < payload.Length; index++)
                    {
                        var target = (int)position + index;
                        if (assembly.Received[target])
                        {
                            if (assembly.Bytes[target] != payload[index]) { _assemblies.Remove(key); return new("photonFragmentConflict", messages.ToArray()); }
                        }
                        else { assembly.Received[target] = true; assembly.Bytes[target] = payload[index]; assembly.Count++; }
                    }
                    assembly.Parts[number] = ((int)position, payload.Length);
                    if (assembly.Count == total && assembly.Parts.Count == fragmentCount)
                    {
                        _assemblies.Remove(key); Remember(key);
                        messages.Add(new(key, datagram.TimestampMicroseconds, assembly.Bytes, connection));
                    }
                }
            }
            bytes = bytes[offset..];
        }
        return new("photon", messages.ToArray());
    }
    /// <summary>
    /// One Photon connection, named by its peer id and challenge and the direction, never by addresses: a route changer
    /// (ExitLag, NoPing…) moves the same connection to another IP or port, and copies sent over several routes are
    /// still the same commands. Only a packet without a challenge falls back to the network flow.
    /// </summary>
    private static string Connection(UdpDatagram datagram, ushort peer, uint challenge) => challenge == 0
        ? $"{datagram.Flow}:{peer}:0"
        : $"{(NetworkDecoder.IsGamePort(datagram.SourcePort) ? "server" : "client")}:{peer}:{challenge}";
    private static string? Validate(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length == 0) return "malformedPhoton";
        while (bytes.Length > 0)
        {
            if (bytes.Length < 12 || bytes[3] == 0) return "malformedPhoton";
            if (bytes[2] != 0) return bytes[2] == 1 ? "encryptedPhoton" : "unsupportedPhotonFlags";
            var count = bytes[3];
            var offset = 12;
            for (var i = 0; i < count; i++)
            {
                if (bytes.Length - offset < 12) return "malformedPhoton";
                var length = U32(bytes[(offset + 4)..]); var type = bytes[offset];
                if (length < 12 || length > bytes.Length - offset || type is < 1 or > 12
                    || (type == 7 && length < 16) || (type == 8 && length < 32)) return "malformedPhoton";
                offset += (int)length;
            }
            bytes = bytes[offset..];
        }
        return null;
    }
    private bool Remember(string identity)
    {
        if (!_seen.Add(identity)) { Duplicates++; return false; }
        _seenOrder.Enqueue(identity);
        if (_seenOrder.Count > 32768) _seen.Remove(_seenOrder.Dequeue());
        return true;
    }
    private static uint U32(ReadOnlySpan<byte> bytes) => BinaryPrimitives.ReadUInt32BigEndian(bytes);
}
