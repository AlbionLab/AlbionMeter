using System.Security.Cryptography;
using System.Xml;
using AlbionMeter.Core;

namespace AlbionMeter.Server;

public static class CatalogEndpoints
{
    private static readonly SemaphoreSlim ImportGate = new(1, 1);
    public static void MapCatalogEndpoints(this WebApplication app)
    {
        app.MapGet("/api/catalog", (MapCatalog catalog) => catalog.Data is { } data ? Results.Ok(data) : Results.NotFound());
        app.MapPost("/api/catalog/import", async (HttpRequest request, MapCatalog catalog, CancellationToken cancellationToken) =>
        {
            if (!request.HasFormContentType) return Results.BadRequest(new { error = "Select a game folder first." });
            if (!await ImportGate.WaitAsync(0, cancellationToken)) return Results.Conflict(new { error = "Another import is running." });
            try
            {
                var form = await request.ReadFormAsync(cancellationToken);
                var world = form.Files.GetFile("world") ?? throw new InvalidDataException("The selected folder does not contain world.bin.");
                var worldBytes = await Read(world, cancellationToken);
                var items = form.Files.GetFile("items");
                var localization = form.Files.GetFile("localization");
                var itemBytes = items is null ? null : await Read(items, cancellationToken);
                var localizationBytes = localization is null ? null : await Read(localization, cancellationToken);
                var version = form["version"].ToString().Trim();
                if (version.Length > 200) throw new InvalidDataException("The game version is invalid.");
                var result = await Task.Run(() => GameAssetImporter.Import(worldBytes, itemBytes, localizationBytes,
                    string.IsNullOrEmpty(version) ? "unknown" : version, "browser-folder-selection/world.bin"), cancellationToken);
                catalog.Save(result);
                return Results.Ok(result);
            }
            catch (Exception exception) when (exception is InvalidDataException or CryptographicException or XmlException or ArgumentException)
            {
                return Results.BadRequest(new { error = "The game files could not be imported. Select an installed game folder with valid world.bin, items.bin and localization.bin files." });
            }
            finally { ImportGate.Release(); }
        });
    }

    private static async Task<byte[]> Read(IFormFile file, CancellationToken cancellationToken)
    {
        if (file.Length is 0 or > 20_000_000) throw new InvalidDataException("Each game file must be smaller than 20 MB.");
        using var memory = new MemoryStream();
        await file.CopyToAsync(memory, cancellationToken);
        return memory.ToArray();
    }
}
