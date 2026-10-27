using System.Diagnostics;
using System.Runtime.Versioning;

namespace AlbionMeter.Server;

[SupportedOSPlatform("windows")]
public static class WindowsFirewall
{
    private const string RuleName = "Albion Meter (packet capture)";
    public static void AllowCapture()
    {
        if (Environment.ProcessPath is not { } program) return;
        try
        {
            if (Run($"advfirewall firewall show rule name=\"{RuleName}\"") == 0) return;
            var added = Run($"advfirewall firewall add rule name=\"{RuleName}\" dir=in action=allow program=\"{program}\" enable=yes profile=any");
            Console.WriteLine(added == 0 ? $"  Firewall: added an inbound rule for {program} so packet capture sees incoming game traffic." : "  Firewall: the inbound rule for packet capture could not be added.");
        }
        catch (Exception exception) when (exception is System.ComponentModel.Win32Exception or InvalidOperationException) { }
    }
    private static int Run(string arguments)
    {
        using var process = Process.Start(new ProcessStartInfo("netsh", arguments) { CreateNoWindow = true, UseShellExecute = false, RedirectStandardOutput = true, RedirectStandardError = true });
        if (process is null) return -1;
        process.StandardOutput.ReadToEnd(); process.StandardError.ReadToEnd();
        return process.WaitForExit(10_000) ? process.ExitCode : -1;
    }
}
