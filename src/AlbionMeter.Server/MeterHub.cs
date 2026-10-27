using Microsoft.AspNetCore.SignalR;

namespace AlbionMeter.Server;

public sealed class MeterHub(MeterSession session) : Hub
{
    public override async Task OnConnectedAsync()
    {
        await Clients.Caller.SendAsync("Snapshot", session.Snapshot());
        await base.OnConnectedAsync();
    }
}

public sealed class MeterWorker(MeterSession session, IHubContext<MeterHub> hub, ILogger<MeterWorker> logger) : BackgroundService
{
    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        using var timer = new PeriodicTimer(TimeSpan.FromMilliseconds(100));
        try
        {
            while (await timer.WaitForNextTickAsync(stoppingToken))
            {
                var snapshot = session.Update();
                if (snapshot != null) await hub.Clients.All.SendAsync("Snapshot", snapshot, stoppingToken);
            }
        }
        catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested) { }
        finally
        {
            try { session.Save(); }
            catch (Exception exception) { logger.LogError(exception, "Failed to save the current encounter."); }
        }
    }
}
