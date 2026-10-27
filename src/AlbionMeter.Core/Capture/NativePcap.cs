using System.Reflection;
using System.Runtime.InteropServices;
using System.Text;

namespace AlbionMeter.Core.Capture;

public sealed record CaptureDevice(string Name, string Description, uint Flags);
public sealed record CaptureFrame(long TimestampMicroseconds, uint OriginalLength, int LinkType, byte[] Data);
public sealed record AdapterStatistics(uint Received, uint Dropped, uint InterfaceDropped);

public sealed class NativePcap : ICaptureSource
{
    public const string DefaultFilter = "(ip and (udp port 5055 or udp port 5056 or udp port 5058 or (ip[6:2] & 0x3fff != 0))) or (ip6 and (udp port 5055 or udp port 5056 or udp port 5058))";
    private static readonly Lazy<bool> Initialized = new(InitializeLibrary);
    private IntPtr _handle;
    public int LinkType { get; }
    public bool Offline { get; }
    static NativePcap()
    {
        NativeLibrary.SetDllImportResolver(typeof(NativePcap).Assembly, ResolveLibrary);
    }
    private static IntPtr ResolveLibrary(string name, Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (name != "albion-pcap") return IntPtr.Zero;
        if (!Environment.Is64BitProcess) throw new PlatformNotSupportedException("Packet capture requires a 64-bit process.");
        if (OperatingSystem.IsWindows())
            return NativeLibrary.Load(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "Npcap", "wpcap.dll"), assembly, DllImportSearchPath.UseDllDirectoryForDependencies);
        if (OperatingSystem.IsLinux()) return NativeLibrary.Load("libpcap.so.1", assembly, DllImportSearchPath.SafeDirectories);
        throw new PlatformNotSupportedException("Packet capture supports Windows and Linux.");
    }
    private NativePcap(IntPtr handle, bool offline)
    {
        _handle = handle;
        Offline = offline;
        LinkType = pcap_datalink(handle);
        if (LinkType < 0) { var error = Error(); Dispose(); throw new IOException(error); }
    }
    private static bool InitializeLibrary()
    {
        var error = new StringBuilder(256);
        if (pcap_init(1, error) != 0) throw new IOException(error.ToString());
        return true;
    }
    public static string Version { get { _ = Initialized.Value; return Marshal.PtrToStringUTF8(pcap_lib_version()) ?? "unknown"; } }
    public static CaptureDevice[] Devices()
    {
        _ = Initialized.Value;
        var error = new StringBuilder(256);
        if (pcap_findalldevs(out var pointer, error) != 0) throw new IOException(error.ToString());
        try
        {
            var devices = new List<CaptureDevice>();
            for (var next = pointer; next != IntPtr.Zero;)
            {
                var device = Marshal.PtrToStructure<Device>(next);
                devices.Add(new(Marshal.PtrToStringUTF8(device.Name) ?? string.Empty,
                    Marshal.PtrToStringUTF8(device.Description) ?? string.Empty, device.Flags));
                next = device.Next;
            }
            return devices.ToArray();
        }
        finally { pcap_freealldevs(pointer); }
    }
    public static NativePcap OpenFile(string path)
    {
        _ = Initialized.Value;
        var error = new StringBuilder(256);
        var handle = pcap_open_offline(Path.GetFullPath(path), error);
        if (handle == IntPtr.Zero) throw new IOException(error.ToString());
        return new(handle, true);
    }
    public static NativePcap OpenDevice(string name)
    {
        if (!Devices().Any(device => device.Name == name)) throw new ArgumentException("Select an available capture interface.", nameof(name));
        var error = new StringBuilder(256);
        var handle = pcap_open_live(name, 262144, 0, 100, error);
        if (handle == IntPtr.Zero) throw new IOException(error.ToString());
        var capture = new NativePcap(handle, false);
        try
        {
            if (pcap_setnonblock(handle, 1, error) != 0) throw new IOException(error.ToString());
            if (pcap_compile(handle, out var filter, DefaultFilter, 1, uint.MaxValue) != 0) throw new IOException(capture.Error());
            try { if (pcap_setfilter(handle, ref filter) != 0) throw new IOException(capture.Error()); }
            finally { pcap_freecode(ref filter); }
            return capture;
        }
        catch { capture.Dispose(); throw; }
    }
    public bool Read(out CaptureFrame? frame)
    {
        ObjectDisposedException.ThrowIf(_handle == IntPtr.Zero, this);
        frame = null;
        var result = pcap_next_ex(_handle, out var header, out var data);
        if (result == -2) return false;
        if (result == -1) throw new IOException(Error());
        if (result == 0) return true;
        var timeSize = OperatingSystem.IsWindows() ? 4 : 8;
        var seconds = timeSize == 4 ? Marshal.ReadInt32(header) : Marshal.ReadInt64(header);
        var micros = timeSize == 4 ? Marshal.ReadInt32(header, 4) : Marshal.ReadInt64(header, 8);
        var captured = unchecked((uint)Marshal.ReadInt32(header, timeSize * 2));
        var original = unchecked((uint)Marshal.ReadInt32(header, timeSize * 2 + 4));
        if (captured > 262144 || captured > original || micros is < 0 or >= 1_000_000)
            throw new InvalidDataException("The capture contains an invalid packet header.");
        var bytes = new byte[captured];
        Marshal.Copy(data, bytes, 0, bytes.Length);
        frame = new(checked(seconds * 1_000_000 + micros), original, LinkType, bytes);
        return true;
    }
    public AdapterStatistics? Statistics()
    {
        ObjectDisposedException.ThrowIf(_handle == IntPtr.Zero, this);
        if (Offline) return null;
        var buffer = Marshal.AllocHGlobal(64);
        try
        {
            if (pcap_stats(_handle, buffer) != 0) return null;
            return new(unchecked((uint)Marshal.ReadInt32(buffer)), unchecked((uint)Marshal.ReadInt32(buffer, 4)), unchecked((uint)Marshal.ReadInt32(buffer, 8)));
        }
        finally { Marshal.FreeHGlobal(buffer); }
    }
    private string Error() => Marshal.PtrToStringUTF8(pcap_geterr(_handle)) ?? "Packet capture failed.";
    public void Dispose()
    {
        if (_handle == IntPtr.Zero) return;
        pcap_close(_handle);
        _handle = IntPtr.Zero;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct Device { public IntPtr Next, Name, Description, Addresses; public uint Flags; }
    [StructLayout(LayoutKind.Sequential)]
    private struct Filter { public uint Length; public IntPtr Instructions; }
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern int pcap_init(uint options, StringBuilder error);
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern IntPtr pcap_lib_version();
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern int pcap_findalldevs(out IntPtr devices, StringBuilder error);
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern void pcap_freealldevs(IntPtr devices);
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern IntPtr pcap_open_offline([MarshalAs(UnmanagedType.LPUTF8Str)] string path, StringBuilder error);
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern IntPtr pcap_open_live([MarshalAs(UnmanagedType.LPUTF8Str)] string device, int snapshotLength, int promiscuous, int timeout, StringBuilder error);
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern int pcap_datalink(IntPtr handle);
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern int pcap_next_ex(IntPtr handle, out IntPtr header, out IntPtr data);
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern int pcap_setnonblock(IntPtr handle, int value, StringBuilder error);
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern int pcap_compile(IntPtr handle, out Filter filter, [MarshalAs(UnmanagedType.LPUTF8Str)] string expression, int optimize, uint netmask);
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern int pcap_setfilter(IntPtr handle, ref Filter filter);
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern void pcap_freecode(ref Filter filter);
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern int pcap_stats(IntPtr handle, IntPtr statistics);
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern IntPtr pcap_geterr(IntPtr handle);
    [DllImport("albion-pcap", CallingConvention = CallingConvention.Cdecl)] private static extern void pcap_close(IntPtr handle);
}
