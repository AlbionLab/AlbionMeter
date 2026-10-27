namespace AlbionMeter.Server;

public static class HistoryEndpoints
{
    public static void MapHistoryEndpoints(this WebApplication app)
    {
        app.MapGet("/api/history/export", async (string? encounterId, string? dungeonId, EncounterStore store, MeterSession session) =>
        {
            try
            {
                if (encounterId is null && dungeonId is null) session.Save();
                if (dungeonId is not null && session.Dungeons().FirstOrDefault(run => run.Id == dungeonId) is { } current) store.SaveDungeon(current, allowEmpty: true);
                var bytes = await Task.Run(() => HistoryArchiveFile.Write(store.ExportHistory(encounterId, dungeonId)));
                return Results.File(bytes, "application/zip", $"albion-{(dungeonId is not null ? "dungeon" : encounterId is not null ? "encounter" : "history")}-{DateTimeOffset.UtcNow:yyyyMMdd-HHmmss}.zip");
            }
            catch (KeyNotFoundException exception) { return Results.NotFound(new { error = exception.Message }); }
            catch (Exception exception) when (exception is ArgumentException or InvalidDataException) { return Results.BadRequest(new { error = exception.Message }); }
        });
        app.MapPost("/api/history/import", async (HttpRequest request, EncounterStore store) =>
        {
            try
            {
                if (!request.HasFormContentType) return Results.BadRequest(new { error = "Choose an Albion Meter history file." });
                var form = await request.ReadFormAsync(request.HttpContext.RequestAborted);
                var file = form.Files.GetFile("history");
                if (file is null || file.Length is 0 or > HistoryArchiveFile.MaxUploadBytes) return Results.BadRequest(new { error = "Choose a history file smaller than 40 MB." });
                using var buffer = new MemoryStream();
                await file.CopyToAsync(buffer, request.HttpContext.RequestAborted);
                var bytes = buffer.ToArray();
                return Results.Ok(await Task.Run(() => store.ImportHistory(HistoryArchiveFile.Read(bytes))));
            }
            catch (Exception exception) when (exception is InvalidDataException or ArgumentException or OverflowException) { return Results.BadRequest(new { error = exception.Message }); }
        });
    }
}
