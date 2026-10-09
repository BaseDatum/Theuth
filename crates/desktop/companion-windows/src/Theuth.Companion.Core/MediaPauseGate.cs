namespace Theuth.Companion.Core;

/// <summary>A media session's playback status (Windows.Media.Control's GlobalSystemMediaTransportControlsSessionPlaybackStatus).</summary>
public enum MediaStatus { Unknown, Closed, Opened, Changing, Stopped, Playing, Paused }

public enum MediaPressAction { None, Pause, SendKey }

public enum MediaEndAction { None, Resume, SendKey }

/// <summary>
/// 153 §8.6 media pause (W153D-R2): the per-gesture decision, opt-in and default off. At a non-refused press the CURRENT media
/// session is paused only if it is playing, and that session is remembered; at the finish or the abandon it is resumed only if we
/// paused it and it is still paused — a session the user paused (or resumed meanwhile) is never touched. When the media-session API
/// is unavailable the Play/Pause key is the fallback at the press and again at the end (resume not guaranteed). One gesture, one gate.
/// </summary>
public sealed class MediaPauseGate
{
    /// <summary>The session we paused (its source app id), or null.</summary>
    public string? PausedSession { get; private set; }

    /// <summary>The fallback key was pressed at the press: the end presses it again.</summary>
    public bool KeyFallback { get; private set; }

    public bool Ended { get; private set; }

    public MediaPressAction AtPress(bool enabled, bool refused, bool apiAvailable, string? session, MediaStatus status)
    {
        if (!enabled || refused || Ended || PausedSession is not null || KeyFallback) return MediaPressAction.None;
        if (!apiAvailable)
        {
            KeyFallback = true;
            return MediaPressAction.SendKey;
        }
        if (session is null || status != MediaStatus.Playing) return MediaPressAction.None; // nothing playing, or the user paused it
        PausedSession = session;
        return MediaPressAction.Pause;
    }

    /// <summary>The pause call failed: nothing of ours to resume.</summary>
    public void PauseFailed() => PausedSession = null;

    /// <summary>
    /// The finish or the abandon (once): resume the session we paused if it is still there and still paused; the fallback key again.
    /// </summary>
    public MediaEndAction AtEnd(string? sessionNow, MediaStatus statusNow)
    {
        if (Ended) return MediaEndAction.None;
        Ended = true;
        if (KeyFallback) return MediaEndAction.SendKey;
        if (PausedSession is not { } ours) return MediaEndAction.None;
        if (!string.Equals(sessionNow, ours, StringComparison.Ordinal)) return MediaEndAction.None; // gone, or another app is current
        return statusNow == MediaStatus.Paused ? MediaEndAction.Resume : MediaEndAction.None;   // the user resumed or stopped it meanwhile
    }
}
