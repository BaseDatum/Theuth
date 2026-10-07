using System.Text.Json;

namespace Theuth.Companion.Rpc.Tests;

/// <summary>
/// The contract test (130 §5.6, 134 §5.3): every fixture under the shared <c>Fixtures/</c> decodes with the hand-written
/// models; ported test-for-test from <c>Tests/TheuthKitTests/FixtureTests.swift</c>, plus one case per fixture file.
/// </summary>
public sealed class FixtureTests
{
    private static string FixtureDir => Path.Combine(AppContext.BaseDirectory, "Fixtures");

    private static byte[] Fixture(string name) => File.ReadAllBytes(Path.Combine(FixtureDir, name + ".json"));

    private static T Decode<T>(string name) => TheuthJson.Deserialize<T>(Fixture(name));

    private static WireEvent Event(string name) => WireEvent.Decode(Fixture(name));

    [Fact]
    public void AttentionListEveryKind()
    {
        var items = Decode<ValueList<AttentionItem>>("attention_list");
        Assert.Equal(
            new[] { AttentionKind.Approval, AttentionKind.Decision, AttentionKind.Ask, AttentionKind.Review, AttentionKind.Hire, AttentionKind.Show },
            items.Select(i => i.Kind));
        Assert.Equal("3f6c1a2e-9b1d-4e0a-8c4b-5d2a7f1e9c01", items[0].Approval);
        Assert.Equal("decision:dec-2", items[1].Packet);
        Assert.Equal("task-7", items[1].Task);
        var ask = items[2].Ask;
        Assert.NotNull(ask);
        Assert.Equal(4, ask.Questions[0].Options.Count);
        Assert.Equal(1, ask.Questions[0].Recommended);
        Assert.Equal("safer; ~20 min later", ask.Questions[0].Options[1].Description);
        Assert.Equal(DateTimeOffset.FromUnixTimeSeconds(1_791_448_700), items[2].WaitingSince);
        Assert.True(items[0].Kind.NeedsYou);
        Assert.False(items[5].Kind.NeedsYou);
        // Pre-W0 engine: the optional fields default.
        Assert.Null(items[0].WaitingSinceMs);
        Assert.Empty(items[0].NextSteps);
    }

    [Fact]
    public void FinishedItemCarriesW0Fields()
    {
        var item = Decode<AttentionItem>("attention_item_finished");
        Assert.Equal(AttentionKind.Finished, item.Kind);
        Assert.Equal((uint?)12, item.Turn);
        Assert.Equal(2, item.NextSteps.Count);
        Assert.Equal("Signed tokens", item.Summary?[..13]);
        Assert.False(item.Kind.NeedsYou);
    }

    [Fact]
    public void UnknownKindDecodesToOther()
    {
        var item = Decode<AttentionItem>("attention_item_unknown_kind");
        Assert.Equal(AttentionKind.Other("weather"), item.Kind);
        Assert.Equal("weather", item.Kind.RawValue);
        Assert.False(item.Kind.NeedsYou);
    }

    [Fact]
    public void AgentsList()
    {
        var agents = Decode<ValueList<AgentSummary>>("agents_list");
        Assert.Equal(new[] { AgentStatus.Working, AgentStatus.Paused, AgentStatus.Invalid }, agents.Select(a => a.Status));
        Assert.Equal("claude-fable", agents[0].File.Model);
        Assert.Equal("0192c5e0-7a1b-7c2d-9e3f-4a5b6c7d8e02", agents[0].Desk);
        Assert.Equal(12.5, agents[0].SpentMonth);
        Assert.True(agents[1].File.Paused);
        Assert.NotNull(agents[2].Error);
    }

    [Fact]
    public void ListActivity()
    {
        var rows = Decode<ValueList<AgentInfo>>("list_activity");
        Assert.Equal(AgentState.Running, rows[0].State);
        Assert.True(rows[0].Activity.Live);
        Assert.Equal(2, (int)rows[0].Activity.Children);
        // Fractional and whole-second RFC 3339 both parse.
        Assert.NotNull(rows[0].Activity.TurnStartedAt);
        Assert.Equal(DateTimeOffset.FromUnixTimeSeconds(1_791_367_920), rows[0].FinishedAt);
        Assert.Equal(AgentState.Approval("shell"), rows[1].State);
        Assert.Equal((ulong?)1_791_448_800_000, rows[1].Activity.WaitingSinceMs);
        // No `activity` on the wire (pre-62 engine): defaults from `state`.
        Assert.Equal(AgentState.Error, rows[2].Activity.State);
        Assert.False(rows[2].Activity.Live);
        Assert.Equal("codex", rows[2].Agent);
    }

    [Fact]
    public void Packets()
    {
        var decision = Decode<Packet>("packet_decision");
        Assert.Equal(PacketKind.Decision, decision.Kind);
        Assert.Equal(4, decision.Options.Count);
        Assert.Equal((uint?)0, decision.Recommended);
        Assert.Equal(2, decision.PreferredOption); // the forwarded recommendation wins
        Assert.Equal("src/pricing.tsx#L10-L24", decision.Pointers[0].Ref);
        var review = Decode<Packet>("packet_review");
        Assert.Equal(PacketKind.Review, review.Kind);
        Assert.Null(review.PreferredOption);
        Assert.Equal(new[] { true, false }, review.Checks.Select(c => c.Done));
        Assert.Null(review.To);
    }

    [Fact]
    public void Events()
    {
        var a = Assert.IsType<ApprovalRequested>(Event("approval_requested").Payload);
        Assert.Equal(Risk.Mutating, a.Risk);
        Assert.Equal("git push origin v2.10.1", a.Args["command"]?.AsString);

        var r = Assert.IsType<ApprovalResolved>(Event("approval_resolved").Payload);
        Assert.Equal(Decision.Deny, r.Decision);
        Assert.True(r.TimedOut);

        var q = Assert.IsType<AskRequested>(Event("ask_requested").Payload);
        Assert.Equal(2, q.Questions.Count);
        Assert.Empty(q.Questions[1].Options); // `#[serde(default)]` options

        var ar = Assert.IsType<AskResolved>(Event("ask_resolved").Payload);
        Assert.False(ar.Dismissed);

        var t = Assert.IsType<TurnFinished>(Event("turn_finished").Payload);
        Assert.Equal("end_turn", t.StopReason);
        Assert.Equal(1500u, t.Context);

        var c = Assert.IsType<AttentionChanged>(Event("attention_changed").Payload);
        Assert.Equal(3u, c.Count);
        Assert.Equal(new[] { "approval:b2" }, c.IdsRemoved);

        var l = Assert.IsType<Lagged>(Event("lagged").Payload);
        Assert.Equal(17ul, l.Count);

        var p = Assert.IsType<PendingPrompts>(Event("pending_prompts").Payload);
        Assert.Equal(Risk.Destructive, p.Approvals.First().Risk);
        Assert.Single(p.Secrets);

        Assert.Equal(WireEvent.Other("TextDelta"), WireEvent.Decode("""{"type":"TextDelta","session":"s","text":"x"}"""));
    }

    [Fact]
    public void AnswerAndDecisionEncodeLikeSerde()
    {
        Assert.Equal("""{"index":1,"kind":"choice"}""", TheuthJson.Serialize<Answer>(new Answer.Choice(1)));
        Assert.Equal("""{"kind":"text","text":"hi"}""", TheuthJson.Serialize<Answer>(new Answer.Text("hi")));
        Assert.Equal("""{"kind":"skipped"}""", TheuthJson.Serialize<Answer>(Answer.Skipped.Instance));
        Assert.Equal("""["allow"]""", TheuthJson.Serialize(new[] { Decision.Allow }));
        // Positional params with a trailing `null` (answers: None).
        Assert.Equal("""["id-1",null]""", TheuthJson.EncodeParams("id-1", null));
    }

    [Fact]
    public void Discovery()
    {
        var dir = Path.Combine(Path.GetTempPath(), "theuth-kit-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(dir);
        try
        {
            Assert.Throws<EngineLocation.NoTokenException>(() => EngineLocation.Discover(dir));
            File.WriteAllText(Path.Combine(dir, "token"), "abc123\n");
            Assert.Equal(new EngineLocation(new Uri("ws://127.0.0.1:7777/"), "abc123"), EngineLocation.Discover(dir));
            File.WriteAllText(Path.Combine(dir, "engine.json"), """{"pid":1,"listen":"127.0.0.1:49431"}""");
            Assert.Equal(new Uri("ws://127.0.0.1:49431/"), EngineLocation.Discover(dir).Url);
            Assert.Equal("/tmp/x", EngineLocation.DataDir(new Dictionary<string, string?> { ["THEUTH_DATA_DIR"] = "/tmp/x" }));
        }
        finally
        {
            Directory.Delete(dir, recursive: true);
        }
    }

    // MARK: C# additions — one case per shared fixture file, so a new fixture without a shape fails here.

    /// <summary>The shape each shared fixture decodes as; a fixture that is not listed fails <see cref="EveryFixtureDecodes"/>.</summary>
    private static readonly Dictionary<string, Func<byte[], object>> Shapes = new(StringComparer.Ordinal)
    {
        ["agents_list"] = b => TheuthJson.Deserialize<ValueList<AgentSummary>>(b),
        ["approval_requested"] = b => WireEvent.Decode(b),
        ["approval_resolved"] = b => WireEvent.Decode(b),
        ["ask_requested"] = b => WireEvent.Decode(b),
        ["ask_resolved"] = b => WireEvent.Decode(b),
        ["attention_changed"] = b => WireEvent.Decode(b),
        ["attention_item_finished"] = b => TheuthJson.Deserialize<AttentionItem>(b),
        ["attention_item_unknown_kind"] = b => TheuthJson.Deserialize<AttentionItem>(b),
        ["attention_list"] = b => TheuthJson.Deserialize<ValueList<AttentionItem>>(b),
        ["lagged"] = b => WireEvent.Decode(b),
        ["list_activity"] = b => TheuthJson.Deserialize<ValueList<AgentInfo>>(b),
        ["packet_decision"] = b => TheuthJson.Deserialize<Packet>(b),
        ["packet_review"] = b => TheuthJson.Deserialize<Packet>(b),
        ["pending_prompts"] = b => WireEvent.Decode(b),
        ["turn_finished"] = b => WireEvent.Decode(b),
    };

    public static TheoryData<string> FixtureNames()
    {
        var data = new TheoryData<string>();
        foreach (var f in Directory.GetFiles(FixtureDir, "*.json").OrderBy(f => f, StringComparer.Ordinal))
        {
            data.Add(Path.GetFileNameWithoutExtension(f));
        }
        return data;
    }

    [Theory]
    [MemberData(nameof(FixtureNames))]
    public void EveryFixtureDecodes(string name)
    {
        Assert.True(Shapes.TryGetValue(name, out var decode), $"Fixtures/{name}.json has no shape — add it to FixtureTests.Shapes");
        var value = decode(Fixture(name));
        if (value is WireEvent e) Assert.False(e.IsOther, $"{name}: {e.Type} must decode to its typed payload");
    }

    [Fact]
    public void TheSharedFixtureDirectoryIsLinked()
    {
        // The csproj links ../../../companion/Tests/TheuthKitTests/Fixtures/*.json; an empty glob must not pass silently.
        Assert.True(Directory.GetFiles(FixtureDir, "*.json").Length >= 15, $"expected the Swift fixture set in {FixtureDir}");
    }

    [Fact]
    public void AnAnswerSubtypeEncodesThroughTheConverter()
    {
        Assert.Equal("""[{"index":2,"kind":"choice"}]""", TheuthJson.EncodeParams(new Answer.Choice(2)));
        Assert.Equal("""["s",[{"kind":"text","text":"no"}]]""", TheuthJson.EncodeParams("s", ValueList.Of<Answer>(new Answer.Text("no"))));
    }

    [Fact]
    public void AMissingRequiredFieldOrANullOneFailsAsInSwift()
    {
        // Swift's synthesized Decodable throws on a missing or null non-optional field; optional ones read as nil.
        Assert.ThrowsAny<JsonException>(() => TheuthJson.Deserialize<AttentionItem>("""{"id":"x","kind":"ask","at_ms":1}"""));
        Assert.ThrowsAny<JsonException>(() => TheuthJson.Deserialize<AttentionItem>("""{"id":"x","kind":"ask","title":null,"at_ms":1}"""));
        var ok = TheuthJson.Deserialize<AttentionItem>("""{"id":"x","kind":"ask","title":"t","at_ms":1,"detail":null,"next_steps":null}""");
        Assert.Null(ok.Detail);
        Assert.Empty(ok.NextSteps);
        Assert.Equal("2026-10-07T10:30:02.9637421+00:00",
            TheuthJson.Deserialize<AgentInfo>("""{"id":"a","name":"","parent":"","depth":0,"model":"m","state":{"state":"idle"},"last":"","started_at":"2026-10-07T10:30:02.963742123Z"}""")
                .StartedAt?.ToString("O"));
    }
}
