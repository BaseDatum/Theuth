namespace Theuth.Companion.Core.Tests;

public sealed class ReceiptModelTests
{
    private static readonly DateTimeOffset T0 = DateTimeOffset.FromUnixTimeSeconds(1_791_448_800);

    private static ActivityEntry Entry(long id, double agoS, string actor = Receipt.CompanionActor, string summary = "answered question q") => new()
    {
        Id = id,
        TsMs = (ulong)((T0.ToUnixTimeSeconds() - agoS) * 1000),
        Kind = "ask_answered",
        Actor = actor,
        Subject = $"ask:{id}",
        Summary = summary,
    };

    [Fact]
    public void NewestFirstAcrossAsksAndAnswers()
    {
        var m = new ReceiptModel();
        m.Add(new Receipt { Id = "ask:a", Kind = ReceiptKind.Ask, At = T0.AddSeconds(-30), Who = "theuth", Text = "handed to w3", Session = "desk" }, T0);
        m.Merge(new[] { Entry(1, agoS: 300), Entry(2, agoS: 5) }, T0);
        Assert.Equal(new[] { "activity:2", "ask:a", "activity:1" }, m.Entries.Select(e => e.Id));
        Assert.True(m.Entries[0].Kind == ReceiptKind.Answer && m.Entries[1].Kind == ReceiptKind.Ask);
    }

    [Fact]
    public void OnlyCompanionAnswersBecomeReceipts()
    {
        var m = new ReceiptModel();
        m.Merge(new[] { Entry(1, agoS: 1, actor: "user@desktop"), Entry(2, agoS: 1, actor: "agent:w2"), Entry(3, agoS: 1) }, T0);
        Assert.Equal(new[] { "activity:3" }, m.Entries.Select(e => e.Id));
    }

    [Fact]
    public void DedupeByActivityIdKeepsTheLatestText()
    {
        var m = new ReceiptModel();
        m.Merge(new[] { Entry(7, agoS: 10) }, T0);
        m.Merge(new[] { Entry(7, agoS: 10, summary: "denied approval ap-7") }, T0);
        m.Add(new Receipt { Id = "ask:x", Kind = ReceiptKind.Ask, At = T0, Who = "theuth", Text = "…" }, T0);
        m.Update("ask:x", "handed to w3 — it owns crates/rpc");
        Assert.Equal(2, m.Count);
        Assert.Equal("denied approval ap-7", m.Entries.First(e => e.Id == "activity:7").Text);
        Assert.Equal("handed to w3 — it owns crates/rpc", m.Entries.First(e => e.Id == "ask:x").Text);
    }

    [Fact]
    public void TwentyFourHourWindow()
    {
        var m = new ReceiptModel();
        m.Merge(new[] { Entry(1, agoS: ReceiptModel.Window - 1), Entry(2, agoS: ReceiptModel.Window + 1) }, T0);
        Assert.Equal(new[] { "activity:1" }, m.Entries.Select(e => e.Id));
        m.Prune(T0.AddSeconds(2));
        Assert.True(m.IsEmpty); // a receipt expires as time passes, not only on arrival
    }

    [Fact]
    public void ClearedStaysClearedThroughRefetches()
    {
        var m = new ReceiptModel();
        m.Merge(new[] { Entry(1, agoS: 1), Entry(2, agoS: 2) }, T0);
        m.Clear("activity:1");
        Assert.Equal(new[] { "activity:2" }, m.Entries.Select(e => e.Id));
        m.Merge(new[] { Entry(1, agoS: 1), Entry(2, agoS: 2) }, T0);
        Assert.Equal(new[] { "activity:2" }, m.Entries.Select(e => e.Id)); // the engine still lists it; the Done zone does not
    }

    [Fact]
    public void ActivityEntryDecodesTheWireShape()
    {
        var json = """{"id":41,"ts_ms":1791448700000,"kind":"approval_resolved","actor":"user@companion","subject":"ap-9","summary":"denied approval ap-9","cost":null}""";
        var e = TheuthJson.Deserialize<ActivityEntry>(json);
        Assert.True(e.Id == 41 && e.Actor == "user@companion" && e.Cost is null);
        var r = Receipt.FromActivity(e);
        Assert.NotNull(r);
        Assert.True(r.Id == "activity:41" && r.Who == "ap-9" && r.At == DateTimeOffset.FromUnixTimeSeconds(1_791_448_700));
        Assert.Equal("""{"limit":100,"since_ms":5}""", TheuthJson.Serialize(new ActivityFilter { SinceMs = 5, Limit = 100 }));
    }
}
