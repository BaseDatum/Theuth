namespace Theuth.Companion.Core.Tests;

// 149 WW3: IslandModel (placement per display, snap-back distance, off-screen frame rejected, persistence); island fix: D1 open
// while live, no collapse on pointer leave, explicit collapse per meeting, D2 shown iff live, no linger; per-display drag memory
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
        Assert.False(m.Detached);

        var dragged = new PxRect(600, 300, 300, 36);
        Assert.Equal(dragged, m.DragEnded(Primary, dragged));
        Assert.True(m.Detached);
        Assert.Equal(IslandMode.Closed, m.Mode); // island fix: dragging the pill does not open it (the shape is not the place)
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
        Assert.False(m.Detached);
        // 41 px off → kept.
        var far = def with { X = def.X + 41 };
        Assert.Equal(far, m.DragEnded(Primary, far));
        Assert.Single(m.Frames);
        // On a 2× monitor the threshold is 80 physical px.
        var def2 = IslandModel.DefaultFrame(Second, Size);
        Assert.Equal(def2, m.DragEnded(Second, def2 with { Y = def2.Y + 79 }));
        Assert.False(m.Detached); // the window snapped back on DISPLAY2; DISPLAY1's memory is per display
        Assert.Single(m.Frames);
        Assert.Equal(def, m.SnapBack(Primary, Size)); // double-click
        Assert.Empty(m.Frames);
        Assert.False(m.Detached);
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
        m.HiddenFromCapture = false;
        m.Save(store);
        var back = new IslandModel();
        back.Load(store);
        Assert.Equal(m.FramesJson, back.FramesJson);
        Assert.True(back.AlwaysShow);
        Assert.False(back.HiddenFromCapture);
        Assert.True(new IslandModel().HiddenFromCapture); // the default: hidden
        back.ResetAll();
        back.Save(store);
        Assert.False(store.Contains(IslandModel.FramesKey));
        var bad = new IslandModel();
        bad.LoadFrames("not json");
        Assert.Empty(bad.Frames);
        bad.LoadFrames("""{"x":[1],"y":["a","b"],"z":[3,4]}""");
        Assert.Equal(new[] { "z" }, bad.Frames.Keys.ToArray());
    }

    // MARK: island fix (Sky 2026-10-09 15:50) — D1 open while live · D2 visibility, no linger · D3 per-display memory

    [Fact]
    public void ShownIffLiveOrAlwaysShow_NoLinger()
    {
        var m = new IslandModel();
        foreach (var live in new[] { "Starting", "Recording", "Paused", "Stopping" }) Assert.True(m.ShouldShow(live), live);
        // Done / Failed (and every other non-live word) hide at once: there is no 10-minute linger after the last topic/brief.
        foreach (var off in new[] { "Idle", "Post", "Review", "Summarizing", "Done", "Failed" }) Assert.False(m.ShouldShow(off), off);
        Assert.True(IslandModel.CloseAnimation <= TimeSpan.FromMilliseconds(300)); // D2: the close animation, then gone
        m.AlwaysShow = true;
        Assert.True(m.ShouldShow("Idle")); // the idle wordmark
        Assert.True(m.ShouldShow("Done"));
    }

    [Fact]
    public void OpenWhileLiveInEveryLiveState()
    {
        var m = new IslandModel();
        Assert.Equal(IslandMode.Closed, m.Mode);
        foreach (var s in new[] { "Starting", "Recording", "Paused", "Stopping" })
        {
            m.Present("m1", s);
            Assert.Equal(IslandMode.Open, m.Mode); // strip + brief pane, no hover needed
            Assert.Equal("m1", m.Meeting);
        }
        m.Present("m1", "Done");
        Assert.Equal(IslandMode.Closed, m.Mode);
        Assert.Null(m.Meeting);
        m.Present(null, "Idle");
        Assert.Equal(IslandMode.Closed, m.Mode); // nothing live: the closed pill is only the idle wordmark (with Always show)
    }

    [Fact]
    public void PointerLeaveNeverCollapses()
    {
        var m = new IslandModel();
        m.Present("m1", "Recording");
        for (int i = 0; i < 5; i++) Assert.Equal(IslandMode.Open, m.PointerLeft());
        Assert.Equal(IslandMode.Open, m.Mode);
        m.Present("m1", "Paused"); // a state change while the pointer is away keeps it open too
        Assert.Equal(IslandMode.Open, m.PointerLeft());
    }

    [Fact]
    public void ExplicitCollapseIsRememberedPerMeeting()
    {
        var m = new IslandModel();
        Assert.False(m.Collapse()); // nothing live: nothing to collapse
        m.Present("m1", "Recording");
        Assert.True(m.Collapse());
        Assert.Equal(IslandMode.Closed, m.Mode);
        Assert.True(m.IsCollapsed("m1"));
        Assert.False(m.Collapse()); // already closed
        // Every later state of the same meeting stays collapsed (pause, resume, stopping)…
        foreach (var s in new[] { "Paused", "Recording", "Stopping" })
        {
            m.Present("m1", s);
            Assert.Equal(IslandMode.Closed, m.Mode);
        }
        // …and a new meeting opens again: the memory is per meeting, not a global preference.
        m.Present("m1", "Done");
        m.Present("m2", "Starting");
        Assert.Equal(IslandMode.Open, m.Mode);
        Assert.False(m.IsCollapsed("m2"));
        // A click on m2's collapsed pill opens it and forgets m2's collapse; m1's stays remembered.
        Assert.True(m.Collapse());
        Assert.True(m.Expand());
        Assert.Equal(IslandMode.Open, m.Mode);
        Assert.False(m.IsCollapsed("m2"));
        Assert.True(m.IsCollapsed("m1"));
        m.Present("m2", "Recording");
        Assert.Equal(IslandMode.Open, m.Mode);
        // The idle wordmark cannot be expanded (its click opens theuth://meetings).
        m.Present(null, "Idle");
        Assert.False(m.Expand());
        Assert.Equal(IslandMode.Closed, m.Mode);
    }

    [Fact]
    public void PlacementMemoryIsPerDisplayAcrossADragBetweenMonitors()
    {
        // build-B has one display: the second-monitor drag is proven here. A drag that ends on DISPLAY2 remembers DISPLAY2's
        // offset (relative to its work area) and leaves DISPLAY1's alone; each display then places from its own memory.
        var m = new IslandModel();
        var on1 = new PxRect(100, 300, 440, 254);
        Assert.Equal(on1, m.DragEnded(Primary, on1));
        var on2 = new PxRect(1024 + 1800, -200 + 600, 880, 508); // the same window at 2×, far from DISPLAY2's default
        Assert.Equal(Second, IslandModel.DisplayFor(new[] { Primary, Second }, on2));
        Assert.Equal(on2, m.DragEnded(Second, on2));
        Assert.Equal((100, 300), m.Frames[Primary.Device]);
        Assert.Equal((1800, 600), m.Frames[Second.Device]);
        Assert.Equal((new PxRect(100, 300, 440, 254), true), m.Place(Primary, (440, 254)));
        Assert.Equal((new PxRect(2824, 400, 880, 508), true), m.Place(Second, (880, 508)));
        // The second monitor moves in the virtual-screen layout: its island moves with it.
        var moved = Second with { WorkArea = new PxRect(-2560, 0, 2560, 1400) };
        Assert.Equal(new PxRect(-760, 600, 880, 508), m.Place(moved, (880, 508)).Frame);
        // Double-click on DISPLAY2 resets DISPLAY2 only.
        m.SnapBack(Second, (880, 508));
        Assert.False(m.Frames.ContainsKey(Second.Device));
        Assert.True(m.Frames.ContainsKey(Primary.Device));
    }

    [Fact]
    public void GrabOffsetScalesWhenADragCrossesIntoADifferentDpi()
    {
        // The window is grabbed 30 px right of its left edge at 1×; on a 2× monitor the same spot is 60 px in.
        Assert.Equal((60, 24), IslandModel.ScaleGrab((30, 12), 1.0, 2.0));
        Assert.Equal((15, 6), IslandModel.ScaleGrab((30, 12), 2.0, 1.0));
        Assert.Equal((30, 12), IslandModel.ScaleGrab((30, 12), 1.5, 1.5));
        Assert.Equal((30, 12), IslandModel.ScaleGrab((30, 12), 0, 2.0)); // a bad scale changes nothing
    }

    [Fact]
    public void DisplayLookupAndClock()
    {
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
    public void BriefWalkWrapsAndPillsWalkToTheirBrief() // island fix §N + D3
    {
        var s = Recording() with
        {
            Topics = new[] { Topic("t1"), Topic("t2") with { Brief = "b2" }, Topic("t3") },
            Briefs = new[] { Brief("b1", "Auto", "t1"), Brief("b2", "Tap", "t2"), Brief("b3", "Deeper", "t1") },
            Cursor = 2,
        };
        // J/↓ from the newest wraps to the oldest; K/↑ from the oldest wraps to the newest.
        Assert.Equal(0, MeetingReducer.CycleIndex(s, +1));
        Assert.Equal(1, MeetingReducer.CycleIndex(s, -1));
        var first = MeetingReducer.ShowAt(s, 0);
        Assert.Equal("b1", first.Current?.Id);
        Assert.Equal(2, MeetingReducer.CycleIndex(first, -1));
        Assert.Equal(1, MeetingReducer.CycleIndex(first, +1));
        // A pill walks to its topic's brief (its own brief id first, else the newest naming it); none → -1 (tap asks for one).
        Assert.Equal(1, MeetingReducer.TopicBriefIndex(s, "t2"));
        Assert.Equal(2, MeetingReducer.TopicBriefIndex(s, "t1"));
        Assert.Equal(-1, MeetingReducer.TopicBriefIndex(s, "t3"));
        Assert.Equal(-1, MeetingReducer.CycleIndex(Recording(), +1));
        Assert.Same(s, MeetingReducer.ShowAt(s, 2)); // already there
        Assert.Equal(0, MeetingReducer.ShowAt(s, -5).Cursor); // clamped
    }

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