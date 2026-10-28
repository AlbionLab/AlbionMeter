using System.ComponentModel;
using System.Net;
using System.Net.NetworkInformation;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;

namespace AlbionMeter.Core.Capture;

[SupportedOSPlatform("linux")]
public sealed class LinuxSocketCapture : ICaptureSource
{
    private int _descriptor;
    private readonly byte[] _buffer = new byte[65536];
    private uint _received, _dropped;
    public int LinkType => 101;
    private LinuxSocketCapture(int descriptor) => _descriptor = descriptor;

    public static CaptureDevice[] Devices() => [new("raw:any", "All interfaces (Linux socket)", PacketCapture.DefaultRouteFlag), .. NetworkInterface.GetAllNetworkInterfaces()
        .Where(adapter => adapter.OperationalStatus == OperationalStatus.Up && adapter.NetworkInterfaceType != NetworkInterfaceType.Loopback)
        .Select(adapter => new CaptureDevice($"raw:{adapter.Name}", $"{adapter.Name} (Linux socket)", Routed(adapter) ? PacketCapture.DefaultRouteFlag : 0))
        .OrderByDescending(device => device.Flags)];

    /// <summary>The main card: the one with an IPv4 gateway, where the internet traffic goes.</summary>
    private static bool Routed(NetworkInterface adapter)
    {
        try { return adapter.GetIPProperties().GatewayAddresses.Any(gateway => gateway.Address.AddressFamily == AddressFamily.InterNetwork && !gateway.Address.Equals(IPAddress.Any)); }
        catch (NetworkInformationException) { return false; }
    }

    public static LinuxSocketCapture Open(string device)
    {
        var name = device[PacketCapture.RawSocketPrefix.Length..];
        if (!Devices().Any(value => value.Name == device)) throw new ArgumentException("Select an available socket interface.", nameof(device));
        var index = name == "any" ? 0 : Native.if_nametoindex(name);
        if (name != "any" && index == 0) throw Error("Interface lookup failed");
        var descriptor = Native.socket(17, 2 | 0x800 | 0x80000, (ushort)IPAddress.HostToNetworkOrder((short)3));
        if (descriptor < 0) throw Error("Socket capture permission denied; CAP_NET_RAW may be required");
        try
        {
            var address = new byte[20];
            BitConverter.GetBytes((ushort)17).CopyTo(address, 0);
            address[2] = 0; address[3] = 3;
            BitConverter.GetBytes(index).CopyTo(address, 4);
            if (Native.bind(descriptor, address, address.Length) != 0) throw Error("Socket interface binding failed");
            var receiveBuffer = 4 * 1024 * 1024;
            Native.setsockopt(descriptor, 1, 8, ref receiveBuffer, sizeof(int));
            return new(descriptor);
        }
        catch { Native.close(descriptor); throw; }
    }

    public bool Read(out CaptureFrame? frame)
    {
        frame = null;
        var descriptor = new PollDescriptor { Descriptor = _descriptor, Events = 1 };
        var ready = Native.poll(ref descriptor, 1, 50);
        if (ready < 0)
        {
            if (Marshal.GetLastPInvokeError() == 4) return true;
            throw Error("Socket polling failed");
        }
        if (ready == 0) return true;
        if ((descriptor.ReturnedEvents & (8 | 16 | 32)) != 0) throw new IOException("The capture socket became unavailable.");
        for (var count = 0; count < 256; count++)
        {
            var length = Native.recv(_descriptor, _buffer, (nuint)_buffer.Length, 0);
            if (length < 0)
            {
                if (Marshal.GetLastPInvokeError() is 11 or 4) return true;
                throw Error("Socket receive failed");
            }
            if (length == 0) return true;
            if (!PacketCapture.IsAlbionPacket(_buffer.AsSpan(0, (int)length))) continue;
            _received++;
            frame = new((DateTimeOffset.UtcNow.Ticks - DateTimeOffset.UnixEpoch.Ticks) / 10, (uint)length, LinkType, _buffer.AsSpan(0, (int)length).ToArray());
            return true;
        }
        return true;
    }

    public AdapterStatistics? Statistics()
    {
        var size = 8u;
        var counters = new byte[8];
        if (Native.getsockopt(_descriptor, 263, 6, counters, ref size) == 0) _dropped += BitConverter.ToUInt32(counters, 4);
        return new(_received, _dropped, 0);
    }
    public void Dispose()
    {
        var descriptor = Interlocked.Exchange(ref _descriptor, -1);
        if (descriptor >= 0) Native.close(descriptor);
    }
    private static IOException Error(string message) => new($"{message}: {new Win32Exception(Marshal.GetLastPInvokeError()).Message}");
    [StructLayout(LayoutKind.Sequential)]
    private struct PollDescriptor { public int Descriptor; public short Events; public short ReturnedEvents; }
    private static class Native
    {
        [DllImport("libc", SetLastError = true)] internal static extern int socket(int domain, int type, int protocol);
        [DllImport("libc", SetLastError = true)] internal static extern int bind(int descriptor, byte[] address, int length);
        [DllImport("libc", SetLastError = true)] internal static extern int poll(ref PollDescriptor descriptor, nuint count, int timeout);
        [DllImport("libc", SetLastError = true)] internal static extern nint recv(int descriptor, [Out] byte[] buffer, nuint length, int flags);
        [DllImport("libc", SetLastError = true)] internal static extern int setsockopt(int descriptor, int level, int option, ref int value, int length);
        [DllImport("libc", SetLastError = true)] internal static extern int getsockopt(int descriptor, int level, int option, [Out] byte[] value, ref uint length);
        [DllImport("libc", SetLastError = true)] internal static extern uint if_nametoindex([MarshalAs(UnmanagedType.LPUTF8Str)] string name);
        [DllImport("libc", SetLastError = true)] internal static extern int close(int descriptor);
    }
}
