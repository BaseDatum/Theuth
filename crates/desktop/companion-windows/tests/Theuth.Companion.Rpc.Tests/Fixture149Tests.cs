using System.Text.Json;

namespace Theuth.Companion.Rpc.Tests;

/// <summary>
/// 149 C's shared fixtures (<c>crates/desktop/companion/Tests/TheuthKitTests/Fixtures/*149*.json</c>, one set for both companions,
/// linked by the csproj) decode with the C# models — the port of <c>FixtureTests.swift:147-264</c>.
/// </summary>
public sealed class Fixture149Tests
{
    private static string FixtureDir => Path.Combine(AppContext.BaseDirectory, "Fixtures");

    private static byte[] Fixture(string name) => File.ReadAllBytes(Path.Combine(FixtureDir, name + ".json"));

    private static T Decode<T>(string name) => TheuthJson.Deserialize<T>(Fixture(name));

    [Fact]
    public void AttentionItemContextPriorityRank()
    {
        var with = Decode<AttentionItem>("attention_item_149_context");
        Assert.Equal("Rename the pricing helper and keep the tests green", with.Context!.Request);
        Assert.Equal("Two call sites disagree on the rounding; which one is right?", with.Context.LastReply);
        Assert.Equal("high", with.Priority);
        Assert.Equal(17u, with.Rank);
        var without = Decode<AttentionItem>("attention_item_149_no_context");
        Assert.Null(without.Context);
        Assert.Null(without.Priority);
        Assert.Equal(5u, without.Rank);
    }

    [Fact]
    public void PacketIfWrong()
    {
        var p = Decode<Packet>("packet_149_if_wrong");
        Assert.Equal("Invoices and the ledger drift by a cent per line until someone notices.", p.IfWrong);
        Assert.Equal(0u, p.Recommended);
    }

    [Fact]
    public void SessionsAndStatus()
    {
        var sessions = Decode<ValueList<SessionSummary>>("list_sessions_149");
        Assert.True(sessions.Count >= 2);
        Assert.Equal(("s-7", "w3", "omp", "theuth", true, "", true), (sessions[0].Id, sessions[0].Name, sessions[0].Agent, sessions[0].Project, sessions[0].Running, sessions[0].Owner, sessions[0].Here));
        Assert.Equal("s-7", sessions[1].Parent);
        Assert.Equal("codex", sessions[1].Agent);
        var status = Decode<SessionsStatus>("sessions_status_149");
        Assert.NotEqual("", status.Engine);
    }

    [Fact]
    public void MeetingStatusInEveryShape()
    {
        var live = Decode<MeetingStatus>("meeting_status_149");
        Assert.Equal(MeetingState.Recording, live.State);
        Assert.True(live.IsLive);
        Assert.Equal(new[] { new Degradation("MicNoAec"), new Degradation("SttDown", "System") }, live.Degraded);
        Assert.Equal(2, live.Topics.Count);
        Assert.Equal("0192c5e0-7a1b-7c2d-9e3f-4a5b6c7d8e31", live.Cursor);
        Assert.Equal("Cache", live.Topics[0].Source);
        var idle = Decode<MeetingStatus>("meeting_status_149_idle");
        Assert.Equal(MeetingState.Idle, idle.State);
        Assert.False(idle.IsLive);
        Assert.True(idle.Supported.IsFull); // the fixture writes the bare word
        Assert.Empty(idle.Topics);
        var post = Decode<MeetingStatus>("meeting_status_149_post");
        Assert.Equal(new MeetingState("Post", "Diarizing"), post.State);
        Assert.False(post.IsLive);
        Assert.Equal(new Degradation("PermissionDenied", "AudioCapture"), post.Degraded[0]);
        Assert.Equal(MeetingSupport.Unknown, post.Supported); // null
        // Round trip of the state through the converter.
        Assert.Equal(post.State, TheuthJson.Deserialize<MeetingState>(TheuthJson.Serialize(post.State)));
        Assert.Equal(live.State, TheuthJson.Deserialize<MeetingState>(TheuthJson.Serialize(live.State)));
        Assert.Equal("""{"kind":"Full"}""", TheuthJson.Serialize(MeetingSupport.Full));
    }

    [Fact]
    public void MeetingLists()
    {
        var topics = Decode<ValueList<Topic>>("meeting_topics_149");
        Assert.Equal(2, topics.Count);
        Assert.Null(topics[1].Brief);
        var briefs = Decode<ValueList<Brief>>("meeting_briefs_149");
        Assert.Equal(2, briefs.Count);
        Assert.Equal("0192c5e0-7a1b-7c2d-9e3f-4a5b6c7d8e31", briefs[1].Parent);
        Assert.Equal(new[] { "Revenue 4.1M, +12% q/q", "Churn 2.1%" }, briefs[0].Bullets);
        var live = Decode<ValueList<Utterance>>("meeting_live_149");
        Assert.Equal(new[] { "me", "s2", "me" }, live.Select(u => u.Who));
        Assert.True(live[2].Request);
        var perms = Decode<MeetingPermissions>("meeting_permissions_149");
        Assert.Equal(("Granted", "NotDetermined"), (perms.Microphone, perms.AudioCapture));
    }

    [Fact]
    public void MeetingEventsEveryVariant()
    {
        using var doc = JsonDocument.Parse(Fixture("meeting_events_149"));
        var items = doc.RootElement.EnumerateArray().Select(e => e.GetRawText()).ToList();
        Assert.Equal(16, items.Count);
        var events = items.Select(WireEvent.Decode).ToList();
        Assert.All(events, e => Assert.IsType<MeetingEvent>(e.Payload));
        var typed = events.Select(e => (MeetingEvent)e.Payload!).ToList();
        Assert.Equal(MeetingEvent.Types.OrderBy(t => t, StringComparer.Ordinal), typed.Select(t => t.Type).Distinct().OrderBy(t => t, StringComparer.Ordinal));
        var started = typed.Single(t => t.Type == "Started");
        Assert.Equal("s-meeting-1", started.Session);
        var utterances = typed.Where(t => t.Type == "Utterance").ToList();
        Assert.Equal(2, utterances.Count);
        Assert.Contains(utterances, u => u.IsFinal == false);
        Assert.Contains(utterances, u => u.IsFinal is null or true);
        var added = typed.Single(t => t.Type == "TopicAdded");
        Assert.NotNull(added.TopicAdded);
        var removed = typed.Single(t => t.Type == "TopicRemoved");
        Assert.NotNull(removed.TopicRemovedId);
        Assert.NotNull(typed.Single(t => t.Type == "Brief").Brief);
        Assert.NotNull(typed.Single(t => t.Type == "OracleDecision").Decision);
        // A non-meeting item is not a MeetingEvent.
        Assert.IsType<TurnFinished>(WireEvent.Decode("""{"type":"TurnFinished","session":"s","turn":"t","stop_reason":"end_turn","context":1}""").Payload);
    }

    [Fact]
    public void PointAndDecisionShapes()
    {
        var reply = Decode<ScreenContextReply>("screen_context_reply_149");
        Assert.Equal(2, reply.Lines.Count);
        Assert.Single(reply.Files);
        var mode = Decode<ModeInfo>("decision_mode_149");
        Assert.True(mode.Enabled);
        Assert.False(mode.Installed);
        Assert.Equal(1500u, mode.BudgetMs);
        Assert.Equal(new JsonValue.String("shadow"), mode.Mode);
        // ScreenContextReq encodes snake_case with `pen`; CancelScope / Delivery as positional words.
        var req = new ScreenContextReq { Samples = new ValueList<PointerSample> { new(0, 1, 2), new(16, 3, 4, pen: false) }, Color = "#ffb000", SinceMs = 3000, Accessibility = true };
        Assert.Equal("""{"samples":[{"t_ms":0,"x":1,"y":2,"pen":true},{"t_ms":16,"x":3,"y":4,"pen":false}],"color":"#ffb000","since_ms":3000,"accessibility":true}""", TheuthJson.Serialize(req));
        Assert.Equal("""["s1","tree"]""", TheuthJson.EncodeParams("s1", CancelScope.Tree.Word()));
        Assert.Equal("""["s1","hi",[],"next_turn",null]""", TheuthJson.EncodeParams("s1", "hi", Array.Empty<string>(), Delivery.NextTurn.Word(), null));
    }
}
