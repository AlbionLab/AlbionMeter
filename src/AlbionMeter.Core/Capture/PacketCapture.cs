using System.Buffers.Binary;
using System.Net;
using System.Net.NetworkInformation;
using System.Net.Sockets;
using System.Runtime.Versioning;

namespace AlbionMeter.Core.Capture;

public interface ICaptureSource : IDisposable
{
    int LinkType { get; }
    bool Read(out CaptureFrame? frame);
    AdapterStatistics? Statistics();
}

public static class PacketCapture
{
    public const string RawSocketPrefix = "raw:";
    public const uint DefaultRouteFlag = 1;
    public static CaptureDevice[] Devices()
    {
        var devices = new List<CaptureDevice>();
        if (OperatingSystem.IsWindows()) devices.AddRange(RawSocketCapture.Devices());
        if (OperatingSystem.IsLinux()) devices.AddRange(LinuxSocketCapture.Devices());
        try { devices.AddRange(NativePcap.Devices()); }
        catch (Exception exception) when (exception is IOException or DllNotFoundException or EntryPointNotFoundException or BadImageFormatException) { }
        return devices.ToArray();
    }
    public static string? DefaultDevice(CaptureDevice[] devices)
    {
        if (OperatingSystem.IsLinux() && devices.Any(device => device.Name == "raw:any")) return "raw:any";
        if (OperatingSystem.IsWindows()) return devices.FirstOrDefault(device => device.Name.StartsWith(RawSocketPrefix, StringComparison.Ordinal) && (device.Flags & DefaultRouteFlag) != 0)?.Name ?? devices.FirstOrDefault(device => device.Name.StartsWith(RawSocketPrefix, StringComparison.Ordinal))?.Name;
        return null;
    }
    public static ICaptureSource Open(string device)
    {
        if (device.StartsWith(RawSocketPrefix, StringComparison.Ordinal))
            return OperatingSystem.IsWindows() ? RawSocketCapture.Open(device) : OperatingSystem.IsLinux() ? LinuxSocketCapture.Open(device) : throw new PlatformNotSupportedException("Socket capture is available on Windows and Linux.");
        return NativePcap.OpenDevice(device);
    }
    public static ICaptureSource OpenFile(string path) => PcapFile.Open(path);
    public static bool IsAlbionPacket(ReadOnlySpan<byte> packet)
    {
        if (packet.Length < 20 || packet[0] >> 4 != 4) return false;
        var header = (packet[0] & 0x0f) * 4;
        if (header < 20 || packet.Length < header) return false;
        if ((BinaryPrimitives.ReadUInt16BigEndian(packet[6..]) & 0x3fff) != 0) return true;
        if (packet[9] != 17 || packet.Length < header + 4) return false;
        var source = BinaryPrimitives.ReadUInt16BigEndian(packet[header..]);
        var destination = BinaryPrimitives.ReadUInt16BigEndian(packet[(header + 2)..]);
        return source is 5055 or 5056 or 5058 || destination is 5055 or 5056 or 5058;
    }

    public static string Library(string? device)
    {
        if (device?.StartsWith(RawSocketPrefix, StringComparison.Ordinal) == true) return OperatingSystem.IsLinux() ? "Linux packet socket" : "Windows raw socket";
        try { return NativePcap.Version; }
        catch (Exception exception) when (exception is IOException or DllNotFoundException or EntryPointNotFoundException or BadImageFormatException) { return OperatingSystem.IsWindows() ? "Windows raw socket" : "unavailable"; }
    }
}

[SupportedOSPlatform("windows")]
public sealed class RawSocketCapture : ICaptureSource
{
    public const string Prefix = PacketCapture.RawSocketPrefix;
    private const uint DefaultRouteFlag = PacketCapture.DefaultRouteFlag;
    private const int LinkTypeRaw = 101;
    private readonly Socket _socket;
    private readonly byte[] _buffer = new byte[65536];
    private uint _received;
    public int LinkType => LinkTypeRaw;
    private RawSocketCapture(Socket socket) => _socket = socket;
    public static CaptureDevice[] Devices() => NetworkInterface.GetAllNetworkInterfaces()
        .Where(adapter => adapter.OperationalStatus == OperationalStatus.Up && adapter.NetworkInterfaceType != NetworkInterfaceType.Loopback)
        .SelectMany(adapter =>
        {
            var properties = adapter.GetIPProperties();
            var routed = properties.GatewayAddresses.Any(gateway => gateway.Address.AddressFamily == AddressFamily.InterNetwork && !gateway.Address.Equals(IPAddress.Any));
            return properties.UnicastAddresses.Where(address => address.Address.AddressFamily == AddressFamily.InterNetwork)
                .Select(address => new CaptureDevice($"{Prefix}{address.Address}", $"{adapter.Name} · {address.Address} (Windows raw socket)", routed ? DefaultRouteFlag : 0));
        })
        .OrderByDescending(device => device.Flags & DefaultRouteFlag).ToArray();
    public static RawSocketCapture Open(string device)
    {
        if (!IPAddress.TryParse(device[Prefix.Length..], out var address) || address.AddressFamily != AddressFamily.InterNetwork || !Devices().Any(item => item.Name == device))
            throw new ArgumentException("Select an available capture interface.", nameof(device));
        var socket = new Socket(AddressFamily.InterNetwork, SocketType.Raw, ProtocolType.IP);
        try
        {
            socket.Bind(new IPEndPoint(address, 0));
            socket.SetSocketOption(SocketOptionLevel.IP, SocketOptionName.HeaderIncluded, true);
            socket.ReceiveBufferSize = 16 * 1024 * 1024;
            socket.IOControl(IOControlCode.ReceiveAll, BitConverter.GetBytes(1), new byte[4]);
            return new(socket);
        }
        catch (SocketException exception)
        {
            socket.Dispose();
            throw exception.SocketErrorCode == SocketError.AccessDenied
                ? new IOException("Packet capture permission denied: run Albion Meter as administrator.", exception)
                : new IOException($"Packet capture could not start on {address}: {exception.Message}", exception);
        }
    }
    public bool Read(out CaptureFrame? frame)
    {
        frame = null;
        while (true)
        {
            if (_socket.Available == 0 && !_socket.Poll(50_000, SelectMode.SelectRead)) return true;
            int length;
            try { length = _socket.Receive(_buffer); }
            catch (SocketException exception) when (exception.SocketErrorCode is SocketError.MessageSize or SocketError.WouldBlock) { continue; }
            if (length <= 0) return true;
            if (!PacketCapture.IsAlbionPacket(_buffer.AsSpan(0, length))) continue;
            _received++;
            frame = new((DateTimeOffset.UtcNow.Ticks - DateTimeOffset.UnixEpoch.Ticks) / 10, (uint)length, LinkTypeRaw, _buffer.AsSpan(0, length).ToArray());
            return true;
        }
    }
    public AdapterStatistics? Statistics() => new(_received, 0, 0);
    public void Dispose() => _socket.Dispose();
}

public sealed class PcapFile : ICaptureSource
{
    private const int MaximumPacket = 262144;
    private readonly BinaryReader _reader;
    private readonly bool _next;
    private bool _swapped;
    private long _divisor = 1;
    private readonly List<(int LinkType, long Divisor)> _interfaces = [];
    public int LinkType { get; private set; }
    private PcapFile(Stream stream)
    {
        _reader = new(stream);
        var magic = _reader.ReadUInt32();
        switch (magic)
        {
            case 0xa1b2c3d4: case 0xd4c3b2a1: _swapped = magic == 0xd4c3b2a1; _divisor = 1; ReadClassicHeader(); break;
            case 0xa1b23c4d: case 0x4d3cb2a1: _swapped = magic == 0x4d3cb2a1; _divisor = 1000; ReadClassicHeader(); break;
            case 0x0a0d0d0a: _next = true; ReadSectionHeader(); ReadBlocksUntilInterface(); break;
            default: throw new IOException("The file is not a PCAP or PCAPNG capture.");
        }
    }
    public static PcapFile Open(string path)
    {
        var stream = new FileStream(Path.GetFullPath(path), FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete, 65536);
        try { return new(stream); }
        catch (EndOfStreamException) { stream.Dispose(); throw new IOException("The capture file is truncated."); }
        catch { stream.Dispose(); throw; }
    }
    private uint U32() { var value = _reader.ReadUInt32(); return _swapped ? BinaryPrimitives.ReverseEndianness(value) : value; }
    private ushort U16() { var value = _reader.ReadUInt16(); return _swapped ? BinaryPrimitives.ReverseEndianness(value) : value; }
    private void ReadClassicHeader()
    {
        _reader.ReadBytes(16);
        LinkType = (int)(U32() & 0x0fffffff);
    }
    private void ReadSectionHeader()
    {
        var length = _reader.ReadUInt32();
        var order = _reader.ReadUInt32();
        if (order is not (0x1a2b3c4d or 0x4d3c2b1a)) throw new IOException("The PCAPNG section header is invalid.");
        _swapped = order == 0x4d3c2b1a;
        if (_swapped) length = BinaryPrimitives.ReverseEndianness(length);
        if (length < 28 || length > 1 << 24) throw new IOException("The PCAPNG section header is invalid.");
        _reader.ReadBytes((int)length - 12);
        _interfaces.Clear();
    }
    private void ReadBlocksUntilInterface()
    {
        while (_interfaces.Count == 0 && ReadBlock(out _)) { }
        if (_interfaces.Count == 0) throw new IOException("The PCAPNG file has no interface.");
        LinkType = _interfaces[0].LinkType;
    }
    public bool Read(out CaptureFrame? frame)
    {
        frame = null;
        try
        {
            if (!_next)
            {
                if (_reader.BaseStream.Position >= _reader.BaseStream.Length) return false;
                var seconds = U32(); var fraction = U32(); var captured = U32(); var original = U32();
                if (captured > MaximumPacket || captured > original || fraction >= 1_000_000 * _divisor) throw new InvalidDataException("The capture contains an invalid packet header.");
                var data = _reader.ReadBytes((int)captured);
                if (data.Length < captured) throw new IOException("The capture file is truncated.");
                frame = new(checked(seconds * 1_000_000L + fraction / _divisor), original, LinkType, data);
                return true;
            }
            while (frame is null) if (!ReadBlock(out frame)) return false;
            return true;
        }
        catch (EndOfStreamException) { throw new IOException("The capture file is truncated."); }
    }
    private bool ReadBlock(out CaptureFrame? frame)
    {
        frame = null;
        if (_reader.BaseStream.Position >= _reader.BaseStream.Length) return false;
        var type = _reader.ReadUInt32();
        if (type == 0x0a0d0d0a) { ReadSectionHeader(); return true; }
        if (_swapped) type = BinaryPrimitives.ReverseEndianness(type);
        var length = U32();
        if (length < 12 || length % 4 != 0 || length > MaximumPacket + 64 && type is 6 or 3 || length > 1 << 24) throw new InvalidDataException("The capture contains an invalid block.");
        var body = _reader.ReadBytes((int)length - 12);
        if (body.Length < length - 12) throw new IOException("The capture file is truncated.");
        _reader.ReadUInt32();
        uint Field(int offset) => _swapped ? BinaryPrimitives.ReadUInt32BigEndian(body.AsSpan(offset)) : BinaryPrimitives.ReadUInt32LittleEndian(body.AsSpan(offset));
        ushort Short(int offset) => _swapped ? BinaryPrimitives.ReadUInt16BigEndian(body.AsSpan(offset)) : BinaryPrimitives.ReadUInt16LittleEndian(body.AsSpan(offset));
        switch (type)
        {
            case 1 when body.Length >= 8:
                long divisor = 1;
                for (var offset = 8; offset + 4 <= body.Length;)
                {
                    var code = Short(offset); var size = Short(offset + 2);
                    if (code == 0 || offset + 4 + size > body.Length) break;
                    if (code == 9 && size >= 1)
                    {
                        var resolution = body[offset + 4];
                        var unitsPerSecond = (resolution & 0x80) == 0 ? Math.Pow(10, resolution) : Math.Pow(2, resolution & 0x7f);
                        divisor = (long)Math.Max(1, unitsPerSecond / 1_000_000);
                    }
                    offset += 4 + (size + 3) / 4 * 4;
                }
                if (_interfaces.Count < 256) _interfaces.Add((Short(0), divisor));
                return true;
            case 6 when body.Length >= 20:
                var index = (int)Field(0);
                if (index >= _interfaces.Count) throw new InvalidDataException("The capture refers to an unknown interface.");
                var stamp = ((long)Field(4) << 32 | Field(8)) / _interfaces[index].Divisor;
                var captured = Field(12); var original = Field(16);
                if (captured > MaximumPacket || captured > original || 20 + captured > body.Length) throw new InvalidDataException("The capture contains an invalid packet header.");
                frame = new(stamp, original, _interfaces[index].LinkType, body.AsSpan(20, (int)captured).ToArray());
                return true;
            case 3 when body.Length >= 4 && _interfaces.Count > 0:
                var size2 = (int)Math.Min(Field(0), (uint)(body.Length - 4));
                frame = new(0, Field(0), _interfaces[0].LinkType, body.AsSpan(4, size2).ToArray());
                return true;
            default:
                return true;
        }
    }
    public AdapterStatistics? Statistics() => null;
    public void Dispose() => _reader.Dispose();
}
