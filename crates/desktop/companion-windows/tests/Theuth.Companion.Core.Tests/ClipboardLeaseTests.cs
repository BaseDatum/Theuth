namespace Theuth.Companion.Core.Tests;

/// <summary>153 §13.3 (proof 23): the clipboard lease — settle restores only if the clipboard is still ours, a stale generation is a
/// no-op, deliveries go through one slot, every failure keeps the words — the Windows twin of <c>ClipboardLeaseTests</c>.</summary>
public sealed class ClipboardLeaseTests
{
    /// <summary>Runs a lease up to Holding (snapshot, write and paste all succeeded) and returns its generation.</summary>
    private static int Paste(ClipboardLease l, string text)
    {
        Assert.True(l.Begin(text, out var fx));
        var g = Assert.IsType<LeaseEffect.TakeSnapshot>(Assert.Single(fx)).Generation;
        var w = Assert.IsType<LeaseEffect.Write>(Assert.Single(l.Snapshotted(g, ok: true)));
        Assert.Equal(text, w.Text);
        Assert.Equal(l.MarkerFor(g), w.Marker);
        Assert.IsType<LeaseEffect.Paste>(Assert.Single(l.Written(g, ok: true)));
        var held = l.Pasted(g, ok: true);
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.ArmSettle(g, ClipboardLease.SettleMs), new LeaseEffect.Verify(g, text) }, held);
        Assert.Equal(LeasePhase.Holding, l.Phase);
        return g;
    }

    [Fact]
    public void SettleRestoresOnlyIfTheClipboardIsStillOurs()
    {
        var l = new ClipboardLease("g1");
        int g = Paste(l, "git rebase -i HEAD~3");
        Assert.True(l.HoldsSnapshot);
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Restore(g) }, l.SettleDue(g, stillOurs: true));
        Assert.Equal(LeasePhase.Restoring, l.Phase);
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Done(g, true, SettleOutcome.Restored) }, l.Restored(g, ok: true));
        Assert.True(l.Ready);
        Assert.False(l.HoldsSnapshot);
        Assert.Equal(1, l.RestoredCount);
        Assert.Equal(1, l.Delivered);
    }

    [Fact]
    public void SettleLeavesTheUsersNewCopyAlone()
    {
        // The user pressed Ctrl+C between the paste and the settle: their clipboard is not ours any more — never overwrite it.
        var l = new ClipboardLease("g1");
        int g = Paste(l, "hello");
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.DropSnapshot(g), new LeaseEffect.Done(g, false, SettleOutcome.KeptUsers) }, l.SettleDue(g, stillOurs: false));
        Assert.True(l.Ready);
        Assert.Equal(0, l.RestoredCount);
    }

    [Fact]
    public void AStaleGenerationIsANoOp()
    {
        var l = new ClipboardLease("g1");
        int g1 = Paste(l, "one");
        l.SettleNow(stillOurs: true); // the gesture moved on: restore now …
        l.Restored(g1, true);
        int g2 = Paste(l, "two");
        Assert.Equal(g1 + 1, g2);
        // … and the first lease's 500 ms timer fires late, while the second holds: nothing happens.
        Assert.Empty(l.SettleDue(g1, stillOurs: true));
        Assert.Equal(LeasePhase.Holding, l.Phase);
        Assert.Empty(l.Snapshotted(g1, true));
        Assert.Empty(l.Written(g1, true));
        Assert.Empty(l.Pasted(g1, true));
        Assert.Empty(l.Restored(g1, true));
        // An input for the right generation but the wrong phase is a no-op too (a duplicate timer).
        Assert.Empty(l.Restored(g2, true));
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Restore(g2) }, l.SettleDue(g2, true));
        Assert.Empty(l.SettleDue(g2, true));
    }

    [Fact]
    public void ABeginOverOurUnsettledItemKeepsThePreviousSnapshot()
    {
        var l = new ClipboardLease("g1");
        Assert.True(l.Begin("first", out _));
        Assert.False(l.Begin("second", out var none)); // snapshotting: busy, the caller queues it
        Assert.Empty(none);
        int g = l.Generation;
        l.Snapshotted(g, true);
        l.Written(g, true, sequence: 41);
        l.Pasted(g, true);
        // H3-D1: a late second Begin over our own unsettled item keeps the PREVIOUS snapshot — no TakeSnapshot, the old settle turns stale.
        Assert.True(l.Begin("second", out var fx));
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Write(2, "second", "g1:2") }, fx);
        Assert.Empty(l.SettleDue(g, stillOurs: true));
        l.Written(2, true, sequence: 42);
        l.Pasted(2, true);
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Restore(2) }, l.SettleDue(2, stillOurs: true));
        Assert.False(l.Begin("third", out _)); // restoring: busy
        l.Restored(2, true);
        Assert.True(l.Begin("third", out var fx3));
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.TakeSnapshot(3) }, fx3);
    }

    [Fact]
    public void StillOursNeedsTheSequenceNumberAndTheMarker()
    {
        var l = new ClipboardLease("g1");
        l.Begin("words", out _);
        l.Snapshotted(1, true);
        l.Written(1, true, sequence: 7);
        Assert.Equal(7u, l.WriteSequence);
        Assert.True(l.StillOurs(1, 7, "g1:1"));
        Assert.False(l.StillOurs(1, 8, "g1:1"));   // somebody wrote since (even the same text)
        Assert.False(l.StillOurs(1, 7, "g1:0"));   // another generation's marker
        Assert.False(l.StillOurs(1, 7, null));     // no session item
        Assert.True(l.StillOurs(1, 0, "g1:1"));    // no window-station access: the marker alone decides
    }

    [Fact]
    public void AFailedRestoreKeepsTheSnapshotForTheNextBegin()
    {
        var l = new ClipboardLease("g1");
        int g = Paste(l, "first");
        l.SettleDue(g, stillOurs: true);
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Failed(g, DeliveryFailure.RestoreFailed, "first"), new LeaseEffect.Done(g, false) }, l.Restored(g, ok: false));
        Assert.True(l.HoldsSnapshot); // our text may still be on the clipboard: the user's snapshot is the one to keep
        Assert.True(l.Begin("second", out var fx));
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Write(g + 1, "second", $"g1:{g + 1}") }, fx);
    }

    [Fact]
    public void LeaveTheDictatedTextOnTheClipboardSkipsTheRestore()
    {
        var l = new ClipboardLease("g1", leaveOnClipboard: true);
        int g = Paste(l, "keep me");
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.DropSnapshot(g), new LeaseEffect.Done(g, false, SettleOutcome.LeftText) }, l.SettleDue(g, stillOurs: true));
    }

    [Fact]
    public void ASnapshotFailureFailsWithoutTouchingTheClipboard()
    {
        var l = new ClipboardLease("g1");
        l.Begin("words", out _);
        var fx = l.Snapshotted(1, ok: false);
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Failed(1, DeliveryFailure.ClipboardSnapshotFailed, "words"), new LeaseEffect.Done(1, false) }, fx);
        Assert.DoesNotContain(fx, e => e is LeaseEffect.Write);
        Assert.True(l.Ready);
    }

    [Fact]
    public void AWriteFailureRestoresTheSnapshot()
    {
        var l = new ClipboardLease("g1");
        l.Begin("words", out _);
        l.Snapshotted(1, true);
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Failed(1, DeliveryFailure.ClipboardWriteFailed, "words"), new LeaseEffect.Restore(1) }, l.Written(1, ok: false));
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Done(1, true, SettleOutcome.Restored) }, l.Restored(1, true));
    }

    [Fact]
    public void ARefusedPasteSettlesAtOnceAndReportsTheFailure()
    {
        // SendInput into an elevated window is dropped (UIPI): pasteCommandFailed, and the user's clipboard comes back now.
        var l = new ClipboardLease("g1");
        l.Begin("words", out _);
        l.Snapshotted(1, true);
        l.Written(1, true);
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Failed(1, DeliveryFailure.PasteCommandFailed, "words"), new LeaseEffect.ArmSettle(1, 0) }, l.Pasted(1, ok: false));
        Assert.Equal(0, l.Delivered);
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Restore(1) }, l.SettleDue(1, true));
    }

    [Fact]
    public void ARestoreFailureIsReported()
    {
        var l = new ClipboardLease("g1");
        int g = Paste(l, "words");
        l.SettleDue(g, true);
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Failed(g, DeliveryFailure.RestoreFailed, "words"), new LeaseEffect.Done(g, false) }, l.Restored(g, ok: false));
        Assert.True(l.Ready);
    }

    [Fact]
    public void SettleNowAtTheGestureEndUsesTheSameRule()
    {
        var l = new ClipboardLease("g1");
        Assert.Empty(l.SettleNow(true)); // idle: nothing to settle
        int g = Paste(l, "words");
        Assert.Equal(new LeaseEffect[] { new LeaseEffect.Restore(g) }, l.SettleNow(stillOurs: true));
        Assert.Empty(l.SettleNow(true)); // restoring: the in-flight step finishes by itself
    }

    [Fact]
    public void TheMarkerNamesTheGestureAndTheGeneration()
    {
        var l = new ClipboardLease("7f3a");
        Assert.Equal("7f3a:3", l.MarkerFor(3));
        Assert.True(l.IsOurs("7f3a:3"));
        Assert.True(l.IsOurs("7f3a:1"));
        Assert.False(l.IsOurs("7f3b:1")); // another gesture's (or another companion's) item
        Assert.False(l.IsOurs("7f3a")); // no generation: not a marker we wrote
        Assert.False(l.IsOurs(null)); // the user's own copy has no marker
        Assert.Equal("theuth.dictation-session", ClipboardLease.SessionFormat);
        Assert.Equal("ExcludeClipboardContentFromMonitorProcessing", ClipboardLease.ExcludeFromMonitors);
        Assert.Equal("CanIncludeInClipboardHistory", ClipboardLease.CanIncludeInHistory);
        Assert.Equal("CanUploadToCloudClipboard", ClipboardLease.CanUploadToCloud);
    }

    [Fact]
    public void TheSnapshotCopiesHGlobalFormatsAndSkipsGdiAndOurOwn()
    {
        var formats = new[]
        {
            new ClipFormat(ClipboardSnapshotPolicy.CfUnicodeText, "CF_UNICODETEXT", 40),
            new ClipFormat(ClipboardSnapshotPolicy.CfBitmap, "CF_BITMAP", 0),
            new ClipFormat(ClipboardSnapshotPolicy.CfDib, "CF_DIB", 1_000_000),
            new ClipFormat(ClipboardSnapshotPolicy.CfHDrop, "CF_HDROP", 300),
            new ClipFormat(ClipboardSnapshotPolicy.CfEnhMetafile, "CF_ENHMETAFILE", 0),
            new ClipFormat(0xC123, "HTML Format", 900),
            new ClipFormat(0xC200, ClipboardLease.SessionFormat, 16),
            new ClipFormat(0xC201, ClipboardLease.CanIncludeInHistory, 4),
            new ClipFormat(0x0250, "private", 8),
        };
        Assert.Equal(new uint[] { ClipboardSnapshotPolicy.CfUnicodeText, ClipboardSnapshotPolicy.CfDib, ClipboardSnapshotPolicy.CfHDrop, 0xC123 },
            ClipboardSnapshotPolicy.Plan(formats));
    }

    [Fact]
    public void ARepresentationOverTheCapKeepsOnlyTheTextFormats()
    {
        var formats = new[]
        {
            new ClipFormat(ClipboardSnapshotPolicy.CfUnicodeText, "CF_UNICODETEXT", 40),
            new ClipFormat(ClipboardSnapshotPolicy.CfLocale, "CF_LOCALE", 4),
            new ClipFormat(ClipboardSnapshotPolicy.CfDib, "CF_DIB", ClipboardLease.MaxRepresentationBytes + 1),
            new ClipFormat(0xC123, "HTML Format", 900),
        };
        Assert.Equal(new uint[] { ClipboardSnapshotPolicy.CfUnicodeText, ClipboardSnapshotPolicy.CfLocale }, ClipboardSnapshotPolicy.Plan(formats));
    }
}