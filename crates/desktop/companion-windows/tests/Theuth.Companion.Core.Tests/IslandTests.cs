namespace Theuth.Companion.Core.Tests;

// 149 WW3: IslandModel (placement per display, snap-back distance, off-screen frame rejected, D10 visibility, persistence)
// and MeetingReducer (one case per event, the brief-stack rule of meeting.ts:181-190, cursor moves, hydrate).

public sealed class IslandModelTests
{
    private static readonly IslandDisplay Primary = new(@"\\.\DISPLAY1", new PxRect(0, 0, 1024, 728), 1.0, true);
    private static readonly IslandDisplay Second = new(@"\\.\DISPLAY2", new PxRect(1024, -200, 2560, 1400), 2.0, false);
    private static readonly (int, int) Size = (300, 36);

    [Fact]
    public void DefaultIsTopCentreOfTheWorkArea()
    {
        var f = IslandModel.DefaultFrame(Primary, Size);
        Assert.Equal(new PxRect(362, 6, 300, 36), f);
        // A top taskbar (work area starts at y=40) and a 2× monitor: the gap scales.
        var topBar = Primary with { WorkArea = new PxRect(0, 40, 1024, 688), Scale = 2.0 };
        Assert.Equal(new PxRect(362, 52, 300, 36), IslandModel.DefaultFrame(topBar, Size));
    }

    [Fact]
    public void PlaceUsesTheDefaultUntilADragIsRemembered()
    {
        var m = new IslandModel();
        var (f0, saved0) = m.Place(Primary, Size);
        Assert.False(saved0);
        Assert.Equal(IslandModel.DefaultFrame(Primary, Size), f0);
        Assert.Equal(IslandMode.Closed, m.Mode);

        var dragged = new PxRect(600, 300, 300, 36);
        Assert.Equal(dragged, m.DragEnded(Primary, dragged));
        Assert.Equal(IslandMode.Detached, m.Mode);
        var (f1, saved1) = m.Place(Primary, Size);
        Assert.True(saved1);
        Assert.Equal(dragged, f1);
        // Memory is per display: the second monitor still gets its default.
        var (f2, saved2) = m.Place(Second, Size);
        Assert.False(saved2);
        Assert.Equal(IslandModel.DefaultFrame(Second, Size), f2);
    }

    [Fact]
    public void SnapBackWithinFortyPointsClearsTheMemory()
    {
        var m = new IslandModel();
        var def = IslandModel.DefaultFrame(Primary, Size);
        // 39 px off → snaps back (1× scale).
        var near = def with { X = def.X + 39 };
        Assert.Equal(def, m.DragEnded(Primary, near));
        Assert.Empty(m.Frames);
        Assert.Equal(IslandMode.Closed, m.Mode);
        // 41 px off → kept.
        var far = def with { X = def.X + 41 };
        Assert.Equal(far, m.DragEnded(Primary, far));
        Assert.Single(m.Frames);
        // On a 2× monitor the threshold is 80 physical px.
        var def2 = IslandModel.DefaultFrame(Second, Size);
        Assert.Equal(def2, m.DragEnded(Second, def2 with { Y = def2.Y + 79 }));
        Assert.Equal(IslandMode.Closed, m.Mode); // the window snapped back; DISPLAY1's memory is per display, not the mode
        Assert.Single(m.Frames);
        Assert.Equal(def, m.SnapBack(Primary, Size)); // double-click
        Assert.Empty(m.Frames);
        Assert.Equal(IslandMode.Closed, m.Mode);
    }

    [Fact]
    public void OffScreenMemoryIsRejectedAndForgotten()
    {
        var m = new IslandModel();
        m.LoadFrames("""{"\\\\.\\DISPLAY1":[2000,2000]}""");
        Assert.Single(m.Frames);
        var (f, saved) = m.Place(Primary, Size);
        Assert.False(saved);
        Assert.Equal(IslandModel.DefaultFrame(Primary, Size), f);
        Assert.Empty(m.Frames);
        // Partly on screen is fine (the user can still grab it).
        m.LoadFrames("""{"\\\\.\\DISPLAY1":[900,700]}""");
        var (f2, saved2) = m.Place(Primary, Size);
        Assert.True(saved2);
        Assert.Equal(new PxRect(900, 700, 300, 36), f2);
    }

    [Fact]
    public void FramesRoundTripThroughJsonAndTheStore()
    {
        var m = new IslandModel();
        m.DragEnded(Primary, new PxRect(10, 20, 300, 36));
        m.DragEnded(Second, new PxRect(1324, 100, 300, 36)); // (300, 300) relative to the work area
        Assert.Equal("""{"\\\\.\\DISPLAY1":[10,20],"\\\\.\\DISPLAY2":[300,300]}""", m.FramesJson);
        var store = new SettingsStore();
        m.AlwaysShow = true;
        m.Save(store);
        var back = new IslandModel();
        back.Load(store);
        Assert.Equal(m.FramesJson, back.FramesJson);
        Assert.True(back.AlwaysShow);
        back.ResetAll();
        back.Save(store);
        Assert.False(store.Contains(IslandModel.FramesKey));
        var bad = new IslandModel();
        bad.LoadFrames("not json");
        Assert.Empty(bad.Frames);
        bad.LoadFrames("""{"x":[1],"y":["a","b"],"z":[3,4]}""");
        Assert.Equal(new[] { "z" }, bad.Frames.Keys.ToArray());
    }

    [Fact]
    public void VisibilityFollowsD10()
    {
        var m = new IslandModel();
        var now = new DateTimeOffset(2026, 10, 9, 2, 0, 0, TimeSpan.Zero);
        foreach (var live in new[] { "Starting", "Recording", "Paused", "Stopping" }) Assert.True(m.ShouldShow(live, null, now), live);
        foreach (var off in new[] { "Idle", "Post", "Review", "Summarizing", "Done", "Failed" }) Assert.False(m.ShouldShow(off, null, now), off);
        Assert.True(m.ShouldShow("Done", now - TimeSpan.FromMinutes(9), now));
        Assert.False(m.ShouldShow("Done", now - TimeSpan.FromMinutes(11), now));
        Assert.Equal(now - TimeSpan.FromMinutes(9) + IslandModel.PendingWindow, IslandModel.HideDue("Done", now - TimeSpan.FromMinutes(9)));
        Assert.Null(IslandModel.HideDue("Recording", now));
        m.AlwaysShow = true;
        Assert.True(m.ShouldShow("Idle", null, now));
    }

    [Fact]
    public void OpenClosePinAndDisplayLookup()
    {
        var m = new IslandModel();
        m.Open(pin: false);
        Assert.Equal(IslandMode.Open, m.Mode);
        Assert.True(m.Close());
        Assert.Equal(IslandMode.Closed, m.Mode);
        m.Open(pin: true);
        Assert.False(m.Close()); // pinned: hover-out does nothing
        Assert.Equal(IslandMode.Open, m.Mode);
        Assert.True(m.Close(force: true)); // Esc
        Assert.Equal(IslandMode.Closed, m.Mode);
        var displays = new[] { Second, Primary };
        Assert.Equal(Primary, IslandModel.DisplayFor(displays, new PxRect(362, 6, 300, 36)));
        Assert.Equal(Second, IslandModel.DisplayFor(displays, new PxRect(2000, 0, 300, 36)));
        Assert.Equal(Primary, IslandModel.DisplayFor(displays, new PxRect(-5000, -5000, 300, 36))); // nowhere → primary
        Assert.Null(IslandModel.DisplayFor(Array.Empty<IslandDisplay>(), new PxRect(0, 0, 1, 1)));
        Assert.Equal("12:31", IslandModel.Clock(751_000));
        Assert.Equal("75:05", IslandModel.Clock(4_505_000));
    }
}

public sealed class MeetingReducerTests
{
    private static readonly DateTimeOffset T0 = new(2026, 10, 9, 2, 0, 0, TimeSpan.Zero);
    private static MeetingTopic Topic(string id, long at = 1000) => new(id, $"topic {id}", at);
    private static MeetingBrief Brief(string id, string source, string? topic = null, long at = 2000) => new(id, topic, $"brief {id}", new[] { "a", "b" }, source, at);
    private static MeetingSnapshot R(MeetingSnapshot s, MeetingChange e, DateTimeOffset? now = null) => MeetingReducer.Reduce(s, e, now ?? T0);

    private static MeetingSnapshot Recording() => R(MeetingSnapshot.Empty, new MeetingChange.Started("m1", "s1"));

    [Fact]
    public void StartedResetsAndRecords()
    {
        var s = Recording();
        Assert.Equal("m1", s.Meeting);
        Assert.Equal("s1", s.Session);
        Assert.Equal("Recording", s.State);
        Assert.Equal(T0, s.ClockAt);
        Assert.Equal(-1, s.Cursor);
        Assert.Null(s.Current);
        Assert.True(s.IsLive);
    }

    [Fact]
    public void StateCarriesTheClockAndEndsTheCues()
    {
        var s = Recording() with { Listening = true, ApprovalWaiting = true, Activity = "thinking…" };
        // Another meeting's State is ignored.
        Assert.Same(s, R(s, new MeetingChange.State("other", "Paused", Array.Empty<string>())));
        var t1 = T0 + TimeSpan.FromSeconds(10);
        var p = R(s, new MeetingChange.State("m1", "Paused", new[] { "MicNoAec" }), t1);
        Assert.Equal(10_000, p.ElapsedMs);
        Assert.Equal(t1, p.ClockAt);
        Assert.True(p.Listening && p.ApprovalWaiting); // live: cues kept
        Assert.Equal("thinking…", p.Activity);
        Assert.Equal(new[] { "MicNoAec" }, p.Degraded);
        Assert.Equal(10_000, p.Elapsed(t1 + TimeSpan.FromSeconds(5))); // paused: the clock stands
        var d = R(p, new MeetingChange.State("m1", "Done", Array.Empty<string>()), t1 + TimeSpan.FromSeconds(5));
        Assert.False(d.Listening || d.ApprovalWaiting);
        Assert.Null(d.Activity);
        Assert.False(d.IsLive);
    }

    [Fact]
    public void TopicsAddReplaceAndRemove()
    {
        var s = Recording();
        s = R(s, new MeetingChange.TopicAdded("m1", Topic("t1")));
        s = R(s, new MeetingChange.TopicAdded("m1", Topic("t2")));
        s = R(s, new MeetingChange.TopicAdded("m1", Topic("t1", 5000))); // re-add moves it to the end
        Assert.Equal(new[] { "t2", "t1" }, s.Topics.Select(t => t.Id));
        Assert.Equal(5000, s.LastTopicOrBriefMs);
        s = MeetingReducer.MarkTapped(s, "t1");
        s = MeetingReducer.MarkTapped(s, "t1");
        Assert.Equal(new[] { "t1" }, s.Tapped);
        s = R(s, new MeetingChange.TopicRemoved("m1", "t1", "Viewed"));
        Assert.Equal(new[] { "t2" }, s.Topics.Select(t => t.Id));
        Assert.Empty(s.Tapped);
    }

    [Fact]
    public void BriefStackRuleAutoQueuesBehindASteppedBackCursor()
    {
        var s = Recording();
        s = R(s, new MeetingChange.Brief("m1", Brief("b1", "Auto")));
        Assert.Equal(0, s.Cursor);
        s = R(s, new MeetingChange.Brief("m1", Brief("b2", "Auto")));
        Assert.Equal(1, s.Cursor); // at the top: an Auto shows
        Assert.Equal(0, s.Unseen);
        s = MeetingReducer.Move(s, -1);
        Assert.Equal(0, s.Cursor);
        s = R(s, new MeetingChange.Brief("m1", Brief("b3", "Auto")));
        s = R(s, new MeetingChange.Brief("m1", Brief("b4", "Cache")));
        Assert.Equal(0, s.Cursor); // stepped back: queued
        Assert.Equal(2, s.Unseen);
        Assert.Equal("b1", s.Current!.Id);
        // Anything the user asked for jumps and clears the badge and the tapped mark.
        s = MeetingReducer.MarkTapped(s, "t9");
        s = R(s, new MeetingChange.Brief("m1", Brief("b5", "Tap", topic: "t9")));
        Assert.Equal(4, s.Cursor);
        Assert.Equal(0, s.Unseen);
        Assert.Empty(s.Tapped);
        Assert.Equal("b5", s.Current!.Id);
    }

    [Fact]
    public void MoveEatsTheBadgeUpwardsAndLeavesItDownwards()
    {
        var s = Recording();
        foreach (var id in new[] { "b1", "b2", "b3", "b4" }) s = R(s, new MeetingChange.Brief("m1", Brief(id, "Auto")));
        s = MeetingReducer.Move(s, -3);
        Assert.Equal(0, s.Cursor);
        s = s with { Unseen = 3 };
        s = MeetingReducer.Move(s, +1);
        Assert.Equal(1, s.Cursor);
        Assert.Equal(2, s.Unseen);
        s = MeetingReducer.Move(s, -1);
        Assert.Equal(2, s.Unseen); // down leaves it
        s = MeetingReducer.Move(s, +10);
        Assert.Equal(3, s.Cursor); // clamped
        Assert.Equal(0, s.Unseen);
        Assert.Same(s, MeetingReducer.Move(s, +1)); // no change → same instance
        Assert.Same(MeetingSnapshot.Empty, MeetingReducer.Move(MeetingSnapshot.Empty, +1));
    }

    [Fact]
    public void CuesPostReviewDeletedAndActivity()
    {
        var s = Recording();
        s = R(s, new MeetingChange.Listening("m1", true));
        s = R(s, new MeetingChange.ApprovalWaiting("m1", true));
        Assert.True(s.Listening && s.ApprovalWaiting);
        s = R(s, new MeetingChange.Activity("s1", "read_file…"));
        Assert.Equal("read_file…", s.Activity);
        Assert.Same(s, R(s, new MeetingChange.Activity("other", null)));
        s = R(s, new MeetingChange.PostProgress("m1", "Transcribing", 40));
        Assert.Equal("Post", s.State);
        Assert.Equal(("Transcribing", 40), s.Progress);
        s = R(s, new MeetingChange.State("m1", "Post", Array.Empty<string>(), "Diarizing"));
        Assert.NotNull(s.Progress); // a Post state keeps the progress
        Assert.Same(s, R(s, new MeetingChange.ReviewReady("other")));
        s = R(s, new MeetingChange.ReviewReady("m1"));
        Assert.Equal("Review", s.State);
        Assert.Null(s.Progress);
        Assert.Same(s, R(s, new MeetingChange.Deleted("other")));
        Assert.Same(MeetingSnapshot.Empty, R(s, new MeetingChange.Deleted("m1")));
    }

    [Fact]
    public void OracleRowsReplaceByBatchAndCap()
    {
        var s = Recording();
        s = R(s, new MeetingChange.OracleDeciding("m1", "batch-1"));
        Assert.Equal("batch-1", s.Deciding);
        s = R(s, new MeetingChange.OracleDecision("m1", new MeetingOracleRow("batch-1", "skip", false, null)));
        Assert.Null(s.Deciding);
        s = R(s, new MeetingChange.OracleDecision("m1", new MeetingOracleRow("batch-1", "brief", true, true))); // outcome update replaces
        Assert.Single(s.Oracle);
        Assert.Equal("brief", s.Oracle[0].Disposition);
        for (int i = 0; i < 60; i++) s = R(s, new MeetingChange.OracleDecision("m1", new MeetingOracleRow($"b{i}", "escalate", false, false)));
        Assert.Equal(MeetingReducer.OracleKeep, s.Oracle.Count);
        Assert.Equal("0 skipped · 0 briefs · 50 escalated · 0 tier 2 calls · agreed 0 %", MeetingReducer.OracleSummary(s.Oracle));
        Assert.Null(MeetingReducer.OracleSummary(Array.Empty<MeetingOracleRow>()));
        Assert.Equal("1 skipped · 1 brief · 0 escalated · 1 tier 2 call",
            MeetingReducer.OracleSummary(new[] { new MeetingOracleRow("x", "skip", true, null), new MeetingOracleRow("y", "brief", false, null) }));
    }

    [Fact]
    public void UtterancesRollPartialsAndRelabelCommittedRows()
    {
        var s = Recording();
        s = R(s, new MeetingChange.Utterance("m1", "System", 100, "hel", false, Final: false, null));
        s = R(s, new MeetingChange.Utterance("m1", "System", 100, "hello th", false, Final: false, null));
        Assert.Single(s.Lines); // the open row is replaced in place
        Assert.True(s.Lines[0].Open);
        s = R(s, new MeetingChange.Utterance("m1", "Mic", 120, "yes", true, Final: true, null));
        s = R(s, new MeetingChange.Utterance("m1", "System", 100, "hello there", false, Final: true, null));
        Assert.Equal(new[] { "yes", "hello there" }, s.Lines.Select(l => l.Text));
        Assert.Equal(new[] { "me:", "them:" }, s.Lines.Select(l => l.Who));
        // A labelled re-publish updates the committed row in place (07 §3).
        s = R(s, new MeetingChange.Utterance("m1", "System", 100, "hello there", false, Final: true, "s2"));
        Assert.Equal(2, s.Lines.Count);
        Assert.Equal("Speaker 2:", s.Lines[1].Who);
        Assert.Same(s, R(s, new MeetingChange.Utterance("other", "Mic", 1, "x", false, true, null)));
        for (int i = 0; i < 250; i++) s = R(s, new MeetingChange.Utterance("m1", "Mic", 1000 + i, $"l{i}", false, true, null));
        Assert.Equal(MeetingReducer.LinesKeep, s.Lines.Count);
    }

    [Fact]
    public void HydratePlacesTheCursorOnTheStatusBrief()
    {
        var briefs = new[] { Brief("b1", "Auto"), Brief("b2", "Tap"), Brief("b3", "Auto") };
        var s = MeetingReducer.Hydrate("m1", "s1", "Recording", Array.Empty<string>(), 5000, "b2", new[] { Topic("t1") }, true, briefs, Array.Empty<MeetingLine>(), T0);
        Assert.Equal(1, s.Cursor);
        Assert.Equal(5000, s.ElapsedMs);
        Assert.True(s.ApprovalWaiting);
        Assert.Equal(5000 + 2000, s.Elapsed(T0 + TimeSpan.FromSeconds(2)));
        var top = MeetingReducer.Hydrate("m1", "s1", "Recording", Array.Empty<string>(), 0, "missing", Array.Empty<MeetingTopic>(), false, briefs, Array.Empty<MeetingLine>(), T0);
        Assert.Equal(2, top.Cursor);
        var none = MeetingReducer.Hydrate(null, null, "Idle", Array.Empty<string>(), 0, null, Array.Empty<MeetingTopic>(), false, Array.Empty<MeetingBrief>(), Array.Empty<MeetingLine>(), T0);
        Assert.Equal(-1, none.Cursor);
        Assert.Null(none.LastTopicOrBriefMs);
    }
}
