namespace Theuth.Companion.Core.Tests;

/// <summary>149 voice-key fix: the dictation lease — a start that returns after the gesture ended is stopped, never kept.</summary>
public sealed class DictationLeaseTests
{
    [Fact]
    public void ANormalGestureKeepsItsIdAndStopsItOnceOnClose()
    {
        var l = new DictationLease();
        int g = l.Begin();
        Assert.True(l.Starting);
        Assert.Equal(LeaseVerdict.Keep, l.Started(g, "d-1"));
        Assert.True(l.Live);
        Assert.Equal("d-1", l.Id);
        Assert.Equal("d-1", l.Close());
        Assert.Equal(DictationLease.Phase.Closed, l.State);
        Assert.Null(l.Id);
        Assert.Null(l.Close()); // idempotent
    }

    [Fact]
    public void ALateStartAfterTheGestureEndedIsStopped()
    {
        // The Mac's "dictating elsewhere": released (send) before dictation_start returned.
        var l = new DictationLease();
        int g = l.Begin();
        Assert.Null(l.Close()); // nothing live yet: the send path has nothing to stop now …
        Assert.Equal(LeaseVerdict.Stop, l.Started(g, "d-late")); // … and the late answer stops itself
        Assert.Null(l.Id);
        Assert.False(l.Live);
    }

    [Fact]
    public void AStartFromAnOlderPressIsStopped()
    {
        var l = new DictationLease();
        int g1 = l.Begin();
        l.Close();
        int g2 = l.Begin();
        Assert.Equal(LeaseVerdict.Stop, l.Started(g1, "d-old"));
        Assert.True(l.Starting); // the new press is untouched
        Assert.Equal(LeaseVerdict.Keep, l.Started(g2, "d-new"));
        Assert.Equal("d-new", l.Close());
    }

    [Fact]
    public void ARefusedStartCancelsOnlyTheOpenLease()
    {
        var l = new DictationLease();
        int g = l.Begin();
        Assert.True(l.StartFailed(g));
        Assert.Equal(DictationLease.Phase.Closed, l.State);
        Assert.Null(l.Close());
        int g2 = l.Begin();
        Assert.False(l.StartFailed(g)); // the old refusal arriving late does not touch the new press
        Assert.True(l.Starting);
        Assert.Equal(LeaseVerdict.Keep, l.Started(g2, "d-2"));
    }

    [Fact]
    public void ASecondStartedForTheSameGenerationIsStopped()
    {
        var l = new DictationLease();
        int g = l.Begin();
        Assert.Equal(LeaseVerdict.Keep, l.Started(g, "d-1"));
        Assert.Equal(LeaseVerdict.Stop, l.Started(g, "d-dup")); // one press = one dictation
        Assert.Equal("d-1", l.Id);
    }

    [Fact]
    public void BeginAfterCloseStartsClean()
    {
        var l = new DictationLease();
        l.Begin();
        l.Close();
        int g = l.Begin();
        Assert.True(l.Starting);
        Assert.Null(l.Id);
        Assert.Equal(g, l.Generation);
    }
}
