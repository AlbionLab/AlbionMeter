using System.Collections.Concurrent;

namespace AlbionMeter.Server;

public sealed class WeaponIconCache(IConfiguration configuration, MapCatalog catalog, AlbionMeter.Core.CombatSourceCatalog? sources = null)
{
    private static readonly HttpClient Client = new() { Timeout = TimeSpan.FromSeconds(8) };
    private readonly ConcurrentDictionary<string, Lazy<Task<byte[]?>>> _pending = new();
    private readonly string _directory = Path.Combine(configuration["Storage:Directory"] ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "AlbionMeter"), "assets", "weapon-icons");
    public async Task<byte[]?> Get(string id)
    {
        if (id.Length > 150 || !id.All(character => char.IsAsciiLetterOrDigit(character) || character is '_' or '@')) return null;
        if (sources?.ContainsItem(id) != true && catalog.Data?.IndexedWeapons?.Any(weapon => weapon.Id == id) != true) return null;
        var request = _pending.GetOrAdd(id, key => new(() => Load(key)));
        try { return await request.Value; }
        finally { _pending.TryRemove(id, out _); }
    }
    private async Task<byte[]?> Load(string id)
    {
        var path = Path.Combine(_directory, id + ".png");
        try
        {
            if (File.Exists(path)) return await File.ReadAllBytesAsync(path);
            using var response = await Client.GetAsync($"https://render.albiononline.com/v1/item/{Uri.EscapeDataString(id)}.png?size=128", HttpCompletionOption.ResponseHeadersRead);
            if (!response.IsSuccessStatusCode || response.Content.Headers.ContentLength > 256_000) return null;
            await using var stream = await response.Content.ReadAsStreamAsync();
            using var output = new MemoryStream();
            var buffer = new byte[8192];
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(8));
            int count;
            while ((count = await stream.ReadAsync(buffer, timeout.Token)) > 0)
            {
                if (output.Length + count > 256_000) return null;
                output.Write(buffer, 0, count);
            }
            var bytes = output.ToArray();
            if (bytes.Length < 8 || !bytes.AsSpan(0, 8).SequenceEqual(new byte[] { 137, 80, 78, 71, 13, 10, 26, 10 })) return null;
            Directory.CreateDirectory(_directory);
            await File.WriteAllBytesAsync(path, bytes);
            return bytes;
        }
        catch (Exception exception) when (exception is HttpRequestException or IOException or UnauthorizedAccessException or OperationCanceledException) { return null; }
    }
}
