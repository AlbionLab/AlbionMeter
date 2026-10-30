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
    private LinuxSocketCapture(int descriptor, bool filtered) { _descriptor = descriptor; KernelFiltered = filtered; }

    /// <summary>Whether the kernel took the filter, so only game packets reach the socket queue.</summary>
    public bool KernelFiltered { get; }

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
            // The socket sees every packet of the interface: without a kernel filter, a download or a stream fills the
            // queue and the kernel drops game packets with the rest. The filter keeps only what IsAlbionPacket accepts.
            var filtered = AttachFilter(descriptor);
            // SO_RCVBUFFORCE needs CAP_NET_ADMIN; without it SO_RCVBUF asks for the same and the kernel caps it at rmem_max.
            var receiveBuffer = 8 * 1024 * 1024;
            if (Native.setsockopt(descriptor, 1, 33, ref receiveBuffer, sizeof(int)) != 0)
                Native.setsockopt(descriptor, 1, 8, ref receiveBuffer, sizeof(int));
            return new(descriptor, filtered);
        }
        catch { Native.close(descriptor); throw; }
    }

    /// <summary>
    /// Classic BPF over the IPv4 header (the cooked socket starts at the network layer): IPv4 fragments, and UDP to or
    /// from the game ports 5055, 5056 and 5058. Anything else never reaches the socket queue.
    /// </summary>
    public static readonly (ushort Code, byte True, byte False, uint Value)[] Filter =
    [
        (0x30, 0, 0, 0),        // 0  A = ip[0]
        (0x54, 0, 0, 0xf0),     // 1  A &= 0xf0
        (0x15, 0, 13, 0x40),    // 2  version 4, or drop
        (0x28, 0, 0, 6),        // 3  A = ip[6:2]
        (0x45, 12, 0, 0x3fff),  // 4  a fragment: accept
        (0x30, 0, 0, 9),        // 5  A = ip[9]
        (0x15, 0, 9, 17),       // 6  UDP, or drop
        (0xb1, 0, 0, 0),        // 7  X = IP header length
        (0x48, 0, 0, 0),        // 8  A = source port
        (0x15, 7, 0, 5055),     // 9
        (0x15, 6, 0, 5056),     // 10
        (0x15, 5, 0, 5058),     // 11
        (0x48, 0, 0, 2),        // 12 A = destination port
        (0x15, 3, 0, 5055),     // 13
        (0x15, 2, 0, 5056),     // 14
        (0x15, 1, 0, 5058),     // 15
        (0x06, 0, 0, 0),        // 16 drop
        (0x06, 0, 0, 0x40000),  // 17 accept
    ];

    private static bool AttachFilter(int descriptor)
    {
        var instructions = Marshal.AllocHGlobal(Filter.Length * 8);
        try
        {
            for (var index = 0; index < Filter.Length; index++)
            {
                var (code, jumpTrue, jumpFalse, value) = Filter[index];
                Marshal.WriteInt16(instructions, index * 8, (short)code);
                Marshal.WriteByte(instructions, index * 8 + 2, jumpTrue);
                Marshal.WriteByte(instructions, index * 8 + 3, jumpFalse);
                Marshal.WriteInt32(instructions, index * 8 + 4, (int)value);
            }
            var program = new FilterProgram { Length = (ushort)Filter.Length, Instructions = instructions };
            // A kernel that refuses the filter still captures; the packets are then checked one by one as before.
            return Native.setsockopt(descriptor, 1, 26, ref program, Marshal.SizeOf<FilterProgram>()) == 0;
        }
        finally { Marshal.FreeHGlobal(instructions); }
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
    [StructLayout(LayoutKind.Sequential)]
    private struct FilterProgram { public ushort Length; public nint Instructions; }
    private static class Native
    {
        [DllImport("libc", SetLastError = true)] internal static extern int socket(int domain, int type, int protocol);
        [DllImport("libc", SetLastError = true)] internal static extern int bind(int descriptor, byte[] address, int length);
        [DllImport("libc", SetLastError = true)] internal static extern int poll(ref PollDescriptor descriptor, nuint count, int timeout);
        [DllImport("libc", SetLastError = true)] internal static extern nint recv(int descriptor, [Out] byte[] buffer, nuint length, int flags);
        [DllImport("libc", SetLastError = true)] internal static extern int setsockopt(int descriptor, int level, int option, ref int value, int length);
        [DllImport("libc", SetLastError = true)] internal static extern int setsockopt(int descriptor, int level, int option, ref FilterProgram value, int length);
        [DllImport("libc", SetLastError = true)] internal static extern int getsockopt(int descriptor, int level, int option, [Out] byte[] value, ref uint length);
        [DllImport("libc", SetLastError = true)] internal static extern uint if_nametoindex([MarshalAs(UnmanagedType.LPUTF8Str)] string name);
        [DllImport("libc", SetLastError = true)] internal static extern int close(int descriptor);
    }
}
