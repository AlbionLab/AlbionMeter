using System.Buffers.Binary;
using System.Net;

namespace AlbionMeter.Core.Capture;

public sealed record UdpDatagram(long TimestampMicroseconds, string Source, string Destination,
    ushort SourcePort, ushort DestinationPort, byte[] Payload)
{
    public string Flow => $"{Source}:{SourcePort}>{Destination}:{DestinationPort}";
}
public sealed record NetworkResult(string Status, UdpDatagram? Datagram = null);

public sealed class NetworkDecoder
{
    private sealed class FragmentSet(long timestamp)
    {
        public long Timestamp { get; } = timestamp;
        public byte[] Bytes { get; } = new byte[65535];
        public bool[] Received { get; } = new bool[65535];
        public int Count { get; set; }
        public int Highest { get; set; }
        public int? Length { get; set; }
    }
    private readonly Dictionary<string, FragmentSet> _fragments = [];
    private long _latestTimestamp;
    public long ExpiredFragments { get; private set; }
    public long EvictedFragments { get; private set; }
    public int PendingFragments => _fragments.Count;
    public NetworkResult Decode(CaptureFrame frame)
    {
        _latestTimestamp = Math.Max(_latestTimestamp, frame.TimestampMicroseconds);
        foreach (var key in _fragments.Where(pair => _latestTimestamp - pair.Value.Timestamp > 15_000_000).Select(pair => pair.Key).ToArray())
        { _fragments.Remove(key); ExpiredFragments++; }
        if (frame.Data.Length < frame.OriginalLength) return new("truncated");
        var bytes = frame.Data.AsSpan();
        int offset;
        ushort protocol;
        switch (frame.LinkType)
        {
            case 1:
                if (bytes.Length < 14) return new("malformed");
                offset = 14; protocol = U16(bytes[12..]);
                for (var count = 0; protocol is 0x8100 or 0x88a8; count++)
                {
                    if (count >= 4 || bytes.Length < offset + 4) return new("malformed");
                    protocol = U16(bytes[(offset + 2)..]); offset += 4;
                }
                break;
            case 113:
                if (bytes.Length < 16) return new("malformed");
                offset = 16; protocol = U16(bytes[14..]); break;
            case 276:
                if (bytes.Length < 20) return new("malformed");
                offset = 20; protocol = U16(bytes); break;
            case 101:
            case 12:
                if (bytes.Length < 1) return new("malformed");
                offset = 0; protocol = (bytes[0] >> 4) == 6 ? (ushort)0x86dd : (ushort)0x0800; break;
            case 0:
            case 108:
                if (bytes.Length < 5) return new("malformed");
                offset = 4; protocol = (bytes[4] >> 4) == 6 ? (ushort)0x86dd : (ushort)0x0800; break;
            default: return new("unsupportedLink");
        }
        bytes = bytes[offset..];
        if (protocol == 0x0800) return IPv4(bytes, frame.TimestampMicroseconds);
        if (protocol == 0x86dd) return IPv6(bytes, frame.TimestampMicroseconds);
        return new("ignored");
    }
    private NetworkResult IPv4(ReadOnlySpan<byte> bytes, long timestamp)
    {
        if (bytes.Length < 20 || bytes[0] >> 4 != 4) return new("malformed");
        var headerLength = (bytes[0] & 15) * 4;
        var length = U16(bytes[2..]);
        if (headerLength < 20 || length < headerLength || length > bytes.Length) return new("malformed");
        if (bytes[9] != 17) return new("ignored");
        var source = new IPAddress(bytes.Slice(12, 4)).ToString();
        var destination = new IPAddress(bytes.Slice(16, 4)).ToString();
        var flags = U16(bytes[6..]);
        var payload = bytes.Slice(headerLength, length - headerLength);
        if ((flags & 0x3fff) == 0) return Udp(payload, timestamp, source, destination);
        var key = $"{source}>{destination}:{U16(bytes[4..])}";
        var offset = (flags & 0x1fff) * 8;
        var more = (flags & 0x2000) != 0;
        if (payload.Length == 0 || offset + payload.Length > 65515 || (more && payload.Length % 8 != 0))
        { _fragments.Remove(key); return new("malformed"); }
        if (!_fragments.TryGetValue(key, out var set))
        {
            if (_fragments.Count >= 128)
            {
                _fragments.Remove(_fragments.MinBy(pair => pair.Value.Timestamp).Key); EvictedFragments++;
            }
            _fragments[key] = set = new(_latestTimestamp);
        }
        var end = offset + payload.Length;
        if ((set.Length is { } known && (end > known || (!more && end != known))) || (!more && set.Highest > end))
        { _fragments.Remove(key); return new("fragmentConflict"); }
        for (var i = 0; i < payload.Length; i++)
        {
            var position = offset + i;
            if (set.Received[position])
            {
                if (set.Bytes[position] != payload[i]) { _fragments.Remove(key); return new("fragmentConflict"); }
            }
            else { set.Received[position] = true; set.Bytes[position] = payload[i]; set.Count++; }
        }
        set.Highest = Math.Max(set.Highest, end);
        if (!more) set.Length = end;
        if (set.Length is not { } total || set.Count != total) return new("fragmentPending");
        _fragments.Remove(key);
        return Udp(set.Bytes.AsSpan(0, total), timestamp, source, destination);
    }
    private static NetworkResult IPv6(ReadOnlySpan<byte> bytes, long timestamp)
    {
        if (bytes.Length < 40 || bytes[0] >> 4 != 6) return new("malformed");
        var length = 40 + U16(bytes[4..]);
        if (length > bytes.Length) return new("malformed");
        var next = bytes[6]; var offset = 40;
        for (var count = 0; next is 0 or 43 or 60 or 51; count++)
        {
            if (count >= 8 || offset + 2 > length) return new("malformed");
            var extensionLength = next == 51 ? (bytes[offset + 1] + 2) * 4 : (bytes[offset + 1] + 1) * 8;
            next = bytes[offset]; offset += extensionLength;
            if (offset > length) return new("malformed");
        }
        if (next == 44) return new("unsupportedIpv6Fragment");
        if (next != 17) return new("ignored");
        return Udp(bytes.Slice(offset, length - offset), timestamp,
            new IPAddress(bytes.Slice(8, 16)).ToString(), new IPAddress(bytes.Slice(24, 16)).ToString());
    }
    private static NetworkResult Udp(ReadOnlySpan<byte> bytes, long timestamp, string source, string destination)
    {
        if (bytes.Length < 8) return new("malformed");
        var length = U16(bytes[4..]);
        if (length < 8 || length > bytes.Length) return new("malformed");
        var sourcePort = U16(bytes); var destinationPort = U16(bytes[2..]);
        if (!IsGamePort(sourcePort) && !IsGamePort(destinationPort)) return new("ignored");
        return new("udp", new(timestamp, source, destination, sourcePort, destinationPort, bytes.Slice(8, length - 8).ToArray()));
    }
    public static bool IsGamePort(ushort port) => port is 5055 or 5056 or 5058;
    private static ushort U16(ReadOnlySpan<byte> bytes) => BinaryPrimitives.ReadUInt16BigEndian(bytes);
}
