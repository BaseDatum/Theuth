namespace Theuth.Companion.Core.Tests;

/// <summary>153 §8.6 / W153D-R2: pause what plays, resume only what we paused.</summary>
public sealed class MediaPauseGateTests
{
    [Fact]
    public void APlayingSessionIsPausedAtThePressAndResumedAtTheEnd()
    {
        var g = new MediaPauseGate();
        Assert.Equal(MediaPressAction.Pause, g.AtPress(true, false, true, "MSEdge", MediaStatus.Playing));
        Assert.Equal("MSEdge", g.PausedSession);
        Assert.Equal(MediaEndAction.Resume, g.AtEnd("MSEdge", MediaStatus.Paused));
        Assert.Equal(MediaEndAction.None, g.AtEnd("MSEdge", MediaStatus.Paused)); // once per gesture
    }

    [Fact]
    public void ASessionTheUserPausedIsNeverTouched()
    {
        var g = new MediaPauseGate();
        Assert.Equal(MediaPressAction.None, g.AtPress(true, false, true, "MSEdge", MediaStatus.Paused));
        Assert.Null(g.PausedSession);
        Assert.Equal(MediaEndAction.None, g.AtEnd("MSEdge", MediaStatus.Paused));
    }

    [Fact]
    public void OffRefusedOrNothingPlayingDoesNothing()
    {
        Assert.Equal(MediaPressAction.None, new MediaPauseGate().AtPress(false, false, true, "MSEdge", MediaStatus.Playing));
        Assert.Equal(MediaPressAction.None, new MediaPauseGate().AtPress(true, true, true, "MSEdge", MediaStatus.Playing));
        Assert.Equal(MediaPressAction.None, new MediaPauseGate().AtPress(true, false, true, null, MediaStatus.Unknown));
        Assert.Equal(MediaPressAction.None, new MediaPauseGate().AtPress(true, false, true, "Spotify", MediaStatus.Stopped));
    }

    [Fact]
    public void TheEndLeavesASessionTheUserChangedMeanwhile()
    {
        var resumed = new MediaPauseGate();
        resumed.AtPress(true, false, true, "MSEdge", MediaStatus.Playing);
        Assert.Equal(MediaEndAction.None, resumed.AtEnd("MSEdge", MediaStatus.Playing)); // the user pressed play during the gesture
        var other = new MediaPauseGate();
        other.AtPress(true, false, true, "MSEdge", MediaStatus.Playing);
        Assert.Equal(MediaEndAction.None, other.AtEnd("Spotify", MediaStatus.Paused)); // another app is current now
        var gone = new MediaPauseGate();
        gone.AtPress(true, false, true, "MSEdge", MediaStatus.Playing);
        Assert.Equal(MediaEndAction.None, gone.AtEnd(null, MediaStatus.Unknown));
    }

    [Fact]
    public void AFailedPauseResumesNothing()
    {
        var g = new MediaPauseGate();
        g.AtPress(true, false, true, "MSEdge", MediaStatus.Playing);
        g.PauseFailed();
        Assert.Equal(MediaEndAction.None, g.AtEnd("MSEdge", MediaStatus.Paused));
    }

    [Fact]
    public void WithoutTheApiTheKeyIsTheFallbackAtBothEnds()
    {
        var g = new MediaPauseGate();
        Assert.Equal(MediaPressAction.SendKey, g.AtPress(true, false, false, null, MediaStatus.Unknown));
        Assert.True(g.KeyFallback);
        Assert.Equal(MediaPressAction.None, g.AtPress(true, false, false, null, MediaStatus.Unknown)); // one flag per gesture
        Assert.Equal(MediaEndAction.SendKey, g.AtEnd(null, MediaStatus.Unknown));
    }
}
