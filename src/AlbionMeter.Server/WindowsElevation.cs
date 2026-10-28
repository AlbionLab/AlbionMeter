using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.Versioning;
using System.Security.Principal;

namespace AlbionMeter.Server;

[SupportedOSPlatform("windows")]
public static class WindowsElevation
{
    public static bool IsAdministrator()
    {
        using var identity = WindowsIdentity.GetCurrent();
        return new WindowsPrincipal(identity).IsInRole(WindowsBuiltInRole.Administrator);
    }

    public static bool RestartElevated(string[] args)
    {
        var arguments = string.Join(' ', args.Select(argument => argument.Length > 0 && !argument.Any(character => char.IsWhiteSpace(character) || character == '"')
            ? argument : $"\"{argument.Replace("\\\"", "\\\\\"").Replace("\"", "\\\"")}\""));
        var start = new ProcessStartInfo(Environment.ProcessPath!, arguments) { UseShellExecute = true, Verb = "runas", WorkingDirectory = AppContext.BaseDirectory };
        try { Process.Start(start); return true; }
        catch (Win32Exception) { return false; }
    }
}
