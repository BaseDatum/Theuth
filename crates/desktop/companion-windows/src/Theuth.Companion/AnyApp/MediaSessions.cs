using Theuth.Companion.Core;
using Windows.Media.Control;

namespace Theuth.Companion.AnyApp;

/// <summary>
/// 153 §8.6 (W153D-R2): the Windows media-session API (GlobalSystemMediaTransportControlsSessionManager) for "pause media while
/// dictating" — deterministic where the Play/Pause key toggles blindly. Every call has a budget; an exception or an overrun answers
/// "unavailable" and the caller falls back to the key.
/// </summary>
internal static class MediaSessions
{
    private const int BudgetMs = 1200;

    public sealed record Current(bool Available, GlobalSystemMediaTransportControlsSession? Session, string? Id, MediaStatus Status);

    public static async Task<Current> CurrentAsync()
    {
        try
        {
            var request = GlobalSystemMediaTransportControlsSessionManager.RequestAsync().AsTask();
            if (await Task.WhenAny(request, Task.Delay(BudgetMs)) != request) return new(false, null, null, MediaStatus.Unknown);
            var manager = await request;
            var session = manager.GetCurrentSession();
            if (session is null) return new(true, null, null, MediaStatus.Unknown);
            return new(true, session, session.SourceAppUserModelId, StatusOf(session));
        }
        catch (Exception e)
        {
            Log.Write($"anyapp: media sessions unavailable ({e.GetType().Name}: {e.Message})");
            return new(false, null, null, MediaStatus.Unknown);
        }
    }

    public static MediaStatus StatusOf(GlobalSystemMediaTransportControlsSession session)
    {
        try
        {
            return session.GetPlaybackInfo().PlaybackStatus switch
            {
                GlobalSystemMediaTransportControlsSessionPlaybackStatus.Closed => MediaStatus.Closed,
                GlobalSystemMediaTransportControlsSessionPlaybackStatus.Opened => MediaStatus.Opened,
                GlobalSystemMediaTransportControlsSessionPlaybackStatus.Changing => MediaStatus.Changing,
                GlobalSystemMediaTransportControlsSessionPlaybackStatus.Stopped => MediaStatus.Stopped,
                GlobalSystemMediaTransportControlsSessionPlaybackStatus.Playing => MediaStatus.Playing,
                GlobalSystemMediaTransportControlsSessionPlaybackStatus.Paused => MediaStatus.Paused,
                _ => MediaStatus.Unknown,
            };
        }
        catch (Exception) { return MediaStatus.Unknown; } // the session closed
    }

    /// <summary>The remembered session's id now (null when it is gone) — read from the manager's session list, not the stale object.</summary>
    public static async Task<(string? Id, MediaStatus Status)> NowAsync(GlobalSystemMediaTransportControlsSession ours)
    {
        try
        {
            var request = GlobalSystemMediaTransportControlsSessionManager.RequestAsync().AsTask();
            if (await Task.WhenAny(request, Task.Delay(BudgetMs)) != request) return (null, MediaStatus.Unknown);
            var manager = await request;
            string id = ours.SourceAppUserModelId;
            foreach (var s in manager.GetSessions())
                if (string.Equals(s.SourceAppUserModelId, id, StringComparison.Ordinal)) return (id, StatusOf(s));
            return (null, MediaStatus.Unknown);
        }
        catch (Exception) { return (null, MediaStatus.Unknown); }
    }

    public static async Task<bool> PauseAsync(GlobalSystemMediaTransportControlsSession s) => await Bounded(() => s.TryPauseAsync().AsTask());

    public static async Task<bool> PlayAsync(GlobalSystemMediaTransportControlsSession s) => await Bounded(() => s.TryPlayAsync().AsTask());

    private static async Task<bool> Bounded(Func<Task<bool>> call)
    {
        try
        {
            var t = call();
            return await Task.WhenAny(t, Task.Delay(BudgetMs)) == t && await t;
        }
        catch (Exception) { return false; }
    }
}
