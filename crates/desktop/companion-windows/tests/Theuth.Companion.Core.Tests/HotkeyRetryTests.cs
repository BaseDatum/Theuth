namespace Theuth.Companion.Core.Tests;

/// <summary>149 recheck fix D1: the startup register retry for global rows another app (the shell) still held; fake clock.</summary>
public sealed class HotkeyRetryTests
{
    private static readonly DateTimeOffset T0 = new(2026, 10, 9, 7, 37, 19, TimeSpan.Zero);
    private static readonly Chord VoiceChord = new KeyTable().ChordFor(KeyAction.VoiceKey)!;
    private static readonly Chord AgentsChord = new KeyTable().ChordFor(KeyAction.OpenAgents)!;

    private static HotkeyFailure Voice() => new(KeyAction.VoiceKey, VoiceChord, $"{VoiceChord.Symbol} is held by another app — no hotkey until you pick another");
    private static HotkeyFailure Agents() => new(KeyAction.OpenAgents, AgentsChord, $"{AgentsChord.Symbol} is held by another app — no hotkey until you pick another");

    [Fact]
    public void RegistersOnceTheShellLetsGo()
    {
        // The shell's 2 s poll unregisters its binding a few seconds after our start: the third attempt wins.
        var r = new HotkeyRetry(new[] { Voice() }, T0);
        Assert.True(r.Active);
        Assert.Equal(HotkeyRetry.Interval, r.NextDelay(T0));
        Assert.Equal("Ctrl+Alt+Space is held by another app — retrying for up to 20 s", r.Notice);
        int calls = 0;
        for (int s = 1; s <= 2; s++)
        {
            var won = r.Attempt(T0.AddSeconds(s), _ => { calls++; return false; });
            Assert.Empty(won);
            Assert.True(r.Active);
        }
        var third = r.Attempt(T0.AddSeconds(3), _ => { calls++; return true; });
        Assert.Equal(KeyAction.VoiceKey, Assert.Single(third).Action);
        Assert.Equal(3, calls);
        Assert.Equal(3, r.Attempts);
        Assert.False(r.Active);
        Assert.False(r.GaveUp);
        Assert.Null(r.Notice);
        Assert.Null(r.NextDelay(T0.AddSeconds(3)));
        // Nothing more is tried.
        Assert.Empty(r.Attempt(T0.AddSeconds(4), _ => throw new InvalidOperationException("no more attempts")));
    }

    [Fact]
    public void GivesUpAfterTheWindowAndKeepsTheNotice()
    {
        // A genuine third-party holder never lets go: 20 attempts over 20 s, then the original notice stands.
        var r = new HotkeyRetry(new[] { Voice() }, T0);
        int calls = 0;
        var now = T0;
        while (r.NextDelay(now) is { } d)
        {
            now += d;
            r.Attempt(now, _ => { calls++; return false; });
        }
        Assert.Equal(20, calls);
        Assert.Equal(20, r.Attempts);
        Assert.True(r.GaveUp);
        Assert.False(r.Active);
        Assert.Equal(T0.AddSeconds(20), now);
        Assert.Equal("Ctrl+Alt+Space is held by another app — no hotkey until you pick another", r.Notice);
        Assert.Single(r.Pending);
        // Late ticks past the window try nothing.
        Assert.Empty(r.Attempt(now.AddSeconds(5), _ => throw new InvalidOperationException("past the window")));
    }

    [Fact]
    public void ATickPastTheWindowGivesUpWithoutTrying()
    {
        var r = new HotkeyRetry(new[] { Voice() }, T0);
        Assert.Empty(r.Attempt(T0.AddSeconds(21), _ => throw new InvalidOperationException("past the window")));
        Assert.True(r.GaveUp);
        Assert.Equal(0, r.Attempts);
        Assert.Null(r.NextDelay(T0.AddSeconds(21)));
    }

    [Fact]
    public void RowsLeaveTheSetIndependently()
    {
        var r = new HotkeyRetry(new[] { Voice(), Agents() }, T0);
        var won = r.Attempt(T0.AddSeconds(1), f => f.Action == KeyAction.OpenAgents);
        Assert.Equal(KeyAction.OpenAgents, Assert.Single(won).Action);
        Assert.Equal(KeyAction.VoiceKey, Assert.Single(r.Pending).Action);
        Assert.True(r.Active);
        Assert.Equal("Ctrl+Alt+Space is held by another app — retrying for up to 20 s", r.Notice);
        Assert.Contains(" is held by another app — retrying for up to 20 s", new HotkeyRetry(new[] { Voice(), Agents() }, T0).Notice);
        Assert.Contains(" · ", new HotkeyRetry(new[] { Voice(), Agents() }, T0).Notice);
    }

    [Fact]
    public void NothingFailedMeansNothingToDo()
    {
        var r = new HotkeyRetry(Array.Empty<HotkeyFailure>(), T0);
        Assert.False(r.Active);
        Assert.Null(r.NextDelay(T0));
        Assert.Null(r.Notice);
        Assert.Empty(r.Attempt(T0.AddSeconds(1), _ => true));
    }

    [Fact]
    public void ReapplyingTheTableCancelsTheRetry()
    {
        // Settings › Keys re-applies the table: the old retry stops; the new apply decides afresh.
        var r = new HotkeyRetry(new[] { Voice() }, T0);
        r.Cancel();
        Assert.False(r.Active);
        Assert.Null(r.Notice);
        Assert.Empty(r.Attempt(T0.AddSeconds(1), _ => throw new InvalidOperationException("cancelled")));
    }

    [Fact]
    public void TheLastDelayFitsTheWindow()
    {
        // A late first tick (the UI thread was busy): the next delay is the remainder, never past the window.
        var r = new HotkeyRetry(new[] { Voice() }, T0);
        Assert.Equal(TimeSpan.FromMilliseconds(400), r.NextDelay(T0.AddSeconds(19.6)));
        Assert.Null(r.NextDelay(T0.AddSeconds(20)));
    }
}
