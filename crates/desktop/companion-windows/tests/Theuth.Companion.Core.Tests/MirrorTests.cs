namespace Theuth.Companion.Core.Tests;

// 149 H3: the Mac W3's test names (Tests/CompanionCoreTests/MeetingReducerTests.swift 21 · IslandModelTests.swift 8), mirrored one for
// one over the C# port; the deeper behavioural cases live in IslandTests.cs (the same code paths).
public sealed class MeetingReducerMirrorTests
{
    private static readonly DateTimeOffset T0 = new(2026, 10, 9, 2, 0, 0, TimeSpan.Zero);
    private static MeetingSnapshot R(MeetingSnapshot s, MeetingChange e, DateTimeOffset? now = null) => MeetingReducer.Reduce(s, e, now ?? T0);
    private static MeetingSnapshot Rec() => R(MeetingSnapshot.Empty, new MeetingChange.Started("m1", "s1"));
    private static MeetingBrief B(string id, string source, string? topic = null) => new(id, topic, id, Array.Empty<string>(), source, 1);

    [Fact] public void StartedResetsToARecordingSlice() { var s = Rec(); Assert.Equal(("m1", "s1", "Recording", -1), (s.Meeting, s.Session, s.State, s.Cursor)); }

    [Fact]
    public void StateUpdatesAndEndsTheCuesWhenNotLive()
    {
        var s = Rec() with { Listening = true, ApprovalWaiting = true, Activity = "x" };
        var d = R(s, new MeetingChange.State("m1", "Done", Array.Empty<string>()));
        Assert.Equal("Done", d.State);
        Assert.False(d.Listening || d.ApprovalWaiting || d.Activity is not null);
    }

    [Fact] public void StateForAnotherMeetingIsIgnored() { var s = Rec(); Assert.Same(s, R(s, new MeetingChange.State("m2", "Paused", Array.Empty<string>()))); }

    [Fact]
    public void LevelKeepsTheMeters()
    {
        var s = R(Rec(), new MeetingChange.Level("m1", 0.25f, 0.5f));
        Assert.Equal((0.25f, 0.5f), (s.MicLevel, s.SystemLevel));
        Assert.Same(s, R(s, new MeetingChange.Level("m2", 1, 1)));
    }

    [Fact]
    public void UtterancePartialReplacesTheOpenLineOfItsTrack()
    {
        var s = R(Rec(), new MeetingChange.Utterance("m1", "System", 1, "a", false, false, null));
        s = R(s, new MeetingChange.Utterance("m1", "System", 1, "ab", false, false, null));
        Assert.Single(s.Lines);
        Assert.Equal("ab", s.Lines[0].Text);
    }

    [Fact]
    public void UtteranceFinalWithSpeakerUpdatesInPlace()
    {
        var s = R(Rec(), new MeetingChange.Utterance("m1", "System", 1, "a", false, true, null));
        s = R(s, new MeetingChange.Utterance("m1", "System", 1, "a", false, true, "s3"));
        Assert.Single(s.Lines);
        Assert.Equal("Speaker 3:", s.Lines[0].Who);
    }

    [Fact]
    public void TopicAddedReplacesById()
    {
        var s = R(Rec(), new MeetingChange.TopicAdded("m1", new MeetingTopic("t1", "a", 1)));
        s = R(s, new MeetingChange.TopicAdded("m1", new MeetingTopic("t1", "b", 2)));
        Assert.Single(s.Topics);
        Assert.Equal("b", s.Topics[0].Label);
    }

    [Fact]
    public void TopicRemovedDropsTopicAndTap()
    {
        var s = MeetingReducer.MarkTapped(R(Rec(), new MeetingChange.TopicAdded("m1", new MeetingTopic("t1", "a", 1))), "t1");
        s = R(s, new MeetingChange.TopicRemoved("m1", "t1", "Dismissed"));
        Assert.Empty(s.Topics);
        Assert.Empty(s.Tapped);
    }

    [Fact] public void BriefAutoAtTheTopShows() { var s = R(R(Rec(), new MeetingChange.Brief("m1", B("b1", "Auto"))), new MeetingChange.Brief("m1", B("b2", "Auto"))); Assert.Equal((1, 0), (s.Cursor, s.Unseen)); }

    [Fact]
    public void BriefAutoSteppedBackQueuesAndBadges()
    {
        var s = MeetingReducer.Move(R(R(Rec(), new MeetingChange.Brief("m1", B("b1", "Auto"))), new MeetingChange.Brief("m1", B("b2", "Auto"))), -1);
        s = R(s, new MeetingChange.Brief("m1", B("b3", "Auto")));
        Assert.Equal((0, 1), (s.Cursor, s.Unseen));
    }

    [Fact]
    public void BriefTapOrRequestJumps()
    {
        var s = MeetingReducer.Move(R(R(Rec(), new MeetingChange.Brief("m1", B("b1", "Auto"))), new MeetingChange.Brief("m1", B("b2", "Auto"))), -1);
        s = R(MeetingReducer.MarkTapped(s, "t1"), new MeetingChange.Brief("m1", B("b3", "Tap", "t1")));
        Assert.Equal((2, 0), (s.Cursor, s.Unseen));
        Assert.Empty(s.Tapped);
        s = R(MeetingReducer.Move(s, -2), new MeetingChange.Brief("m1", B("b4", "Request")));
        Assert.Equal(3, s.Cursor);
    }

    [Fact] public void ListeningFollowsHeld() { Assert.True(R(Rec(), new MeetingChange.Listening("m1", true)).Listening); Assert.False(R(Rec(), new MeetingChange.Listening("m1", false)).Listening); }

    [Fact] public void ApprovalWaitingFollowsWaiting() { Assert.True(R(Rec(), new MeetingChange.ApprovalWaiting("m1", true)).ApprovalWaiting); }

    [Fact]
    public void PostProgressSetsPostStateAndProgress()
    {
        var s = R(Rec(), new MeetingChange.PostProgress("m1", "Aligning", 90));
        Assert.Equal("Post", s.State);
        Assert.Equal(("Aligning", 90), s.Progress);
    }

    [Fact] public void ReviewReadyClearsProgress() { var s = R(R(Rec(), new MeetingChange.PostProgress("m1", "Aligning", 90)), new MeetingChange.ReviewReady("m1")); Assert.Equal(("Review", (( string, int)?)null), (s.State, s.Progress)); }

    [Fact]
    public void SummarizedKeepsTheSlice()
    {
        // `Summarized` is not an island change: MeetingWire yields nothing for it and the slice stands.
        Assert.Null(MeetingWire.Decode("Summarized", """{"type":"Summarized","meeting":"m1","title":"x"}"""u8));
    }

    [Fact] public void DeletedEmptiesTheSlice() => Assert.Same(MeetingSnapshot.Empty, R(Rec(), new MeetingChange.Deleted("m1")));

    [Fact] public void OracleDecidingFlashes() => Assert.Equal("b9", R(Rec(), new MeetingChange.OracleDeciding("m1", "b9")).Deciding);

    [Fact]
    public void OracleDecisionReplacesOrAppendsAndClearsDeciding()
    {
        var s = R(Rec(), new MeetingChange.OracleDeciding("m1", "b9"));
        s = R(s, new MeetingChange.OracleDecision("m1", new MeetingOracleRow("b9", "skip", false, null)));
        s = R(s, new MeetingChange.OracleDecision("m1", new MeetingOracleRow("b9", "brief", false, null)));
        s = R(s, new MeetingChange.OracleDecision("m1", new MeetingOracleRow("b10", "skip", false, null)));
        Assert.Null(s.Deciding);
        Assert.Equal(new[] { "brief", "skip" }, s.Oracle.Select(o => o.Disposition));
    }

    [Fact]
    public void SeedFromMeetingStatus()
    {
        var briefs = new[] { B("b1", "Auto"), B("b2", "Auto") };
        var s = MeetingReducer.Hydrate("m1", "s1", "Paused", new[] { "DiskLow" }, 1234, "b1", Array.Empty<MeetingTopic>(), false, briefs, Array.Empty<MeetingLine>(), T0);
        Assert.Equal((0, 1234L, "Paused"), (s.Cursor, s.ElapsedMs, s.State));
    }

    [Fact]
    public void ElapsedExcludesPauses()
    {
        var s = R(Rec(), new MeetingChange.State("m1", "Paused", Array.Empty<string>()), T0 + TimeSpan.FromSeconds(10));
        Assert.Equal(10_000, s.Elapsed(T0 + TimeSpan.FromSeconds(60)));
        s = R(s, new MeetingChange.State("m1", "Recording", Array.Empty<string>()), T0 + TimeSpan.FromSeconds(60));
        Assert.Equal(15_000, s.Elapsed(T0 + TimeSpan.FromSeconds(65)));
    }
}

public sealed class IslandModelMirrorTests
{
    private static readonly IslandDisplay D1 = new(@"\\.\DISPLAY1", new PxRect(0, 0, 1920, 1032), 1.0, true);
    private static readonly DateTimeOffset Now = new(2026, 10, 9, 2, 0, 0, TimeSpan.Zero);
    private static readonly (int, int) Size = (300, 36);

    [Fact] public void VisibleWhileLive() => Assert.True(new IslandModel().ShouldShow("Recording", null, Now));

    [Fact] public void VisibleTenMinutesAfterTheLastTopicOrBrief() => Assert.True(new IslandModel().ShouldShow("Done", Now - TimeSpan.FromMinutes(10), Now));

    [Fact] public void VisibleWhenAlwaysShow() => Assert.True(new IslandModel { AlwaysShow = true }.ShouldShow("Idle", null, Now));

    [Fact] public void HiddenOtherwise() => Assert.False(new IslandModel().ShouldShow("Idle", Now - TimeSpan.FromMinutes(11), Now));

    [Fact]
    public void DragWithinFortyPointsSnapsBackClosed()
    {
        var m = new IslandModel();
        var def = IslandModel.DefaultFrame(D1, Size);
        Assert.Equal(def, m.DragEnded(D1, def with { X = def.X + 40 }));
        Assert.Equal(IslandMode.Closed, m.Mode);
        Assert.Empty(m.Frames);
    }

    [Fact]
    public void DragBeyondFortyPointsDetachesAndRemembers()
    {
        var m = new IslandModel();
        var def = IslandModel.DefaultFrame(D1, Size);
        var far = def with { Y = def.Y + 41 };
        Assert.Equal(far, m.DragEnded(D1, far));
        Assert.Equal(IslandMode.Detached, m.Mode);
        Assert.Equal((far.X, far.Y), m.Frames[D1.Device]);
    }

    [Fact]
    public void OffScreenFrameIsRejected()
    {
        var m = new IslandModel();
        string? rejected = null;
        m.FrameRejected += (device, _) => rejected = device;
        m.LoadFrames("""{"\\\\.\\DISPLAY1":[5000,5000]}""");
        var (f, saved) = m.Place(D1, Size);
        Assert.False(saved);
        Assert.Equal(IslandModel.DefaultFrame(D1, Size), f);
        Assert.Equal(D1.Device, rejected);
        Assert.Empty(m.Frames);
    }

    [Fact]
    public void FramesKeyedByUuidSurviveATopologyChange()
    {
        // The device name is the key (Windows' stand-in for the display UUID): the monitor moves in the virtual screen, the offset holds.
        var m = new IslandModel();
        m.DragEnded(D1, new PxRect(100, 200, 300, 36));
        var moved = D1 with { WorkArea = new PxRect(-1920, 300, 1920, 1032) };
        var (f, saved) = m.Place(moved, Size);
        Assert.True(saved);
        Assert.Equal(new PxRect(-1820, 500, 300, 36), f);
        var other = new IslandDisplay(@"\\.\DISPLAY2", new PxRect(0, 0, 1920, 1032), 1.0, false);
        Assert.False(m.Place(other, Size).Saved);
    }
}
