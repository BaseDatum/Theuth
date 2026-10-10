using System.Text.Json;

namespace Theuth.Companion.Core.Tests;

// 149 WW3: MeetingWire decodes the engine's MeetingEvent JSON (crates/meeting/src/lib.rs, serde tag = "type") and the
// meeting_status / meeting_briefs / meeting_live results into the reducer's records; tolerant of missing fields.
public sealed class MeetingWireTests
{
    private static MeetingChange? D(string json)
    {
        using var doc = JsonDocument.Parse(json);
        return MeetingWire.Decode(doc.RootElement.GetProperty("type").GetString()!, doc.RootElement);
    }

    [Fact]
    public void DecodesEveryIslandEvent()
    {
        Assert.Equal(new MeetingChange.Started("m1", "s1"), D("""{"type":"Started","meeting":"m1","session":"s1","process":{"pid":0,"name":"System audio","bundle_id":null,"active":true},"at":"2026-10-09T01:00:00Z"}"""));
        var st = Assert.IsType<MeetingChange.State>(D("""{"type":"State","meeting":"m1","state":"Paused","degraded":["MicNoAec",{"SttDown":"Mic"}]}"""));
        Assert.Equal("Paused", st.StateWord);
        Assert.Equal(new[] { "MicNoAec", "SttDown:Mic" }, st.Degraded);
        var post = Assert.IsType<MeetingChange.State>(D("""{"type":"State","meeting":"m1","state":{"Post":"Diarizing"},"degraded":[]}"""));
        Assert.Equal(("Post", "Diarizing"), (post.StateWord, post.PostStage));
        var ta = Assert.IsType<MeetingChange.TopicAdded>(D("""{"type":"TopicAdded","meeting":"m1","topic":{"id":"t1","label":"PROJ-12","at_ms":1500,"brief":null}}"""));
        Assert.Equal(new MeetingTopic("t1", "PROJ-12", 1500), ta.Topic);
        Assert.Equal(new MeetingChange.TopicRemoved("m1", "t1", "Viewed"), D("""{"type":"TopicRemoved","meeting":"m1","topic":"t1","why":"Viewed"}"""));
        var br = Assert.IsType<MeetingChange.Brief>(D("""{"type":"Brief","meeting":"m1","brief":{"id":"b1","topic":"t1","title":"PROJ-12","bullets":["a","b","c"],"source":"Tap","at_ms":2000,"parent":null}}"""));
        Assert.Equal("b1", br.Value.Id);
        Assert.Equal(3, br.Value.Bullets.Count);
        Assert.False(br.Value.IsAuto);
        var ut = Assert.IsType<MeetingChange.Utterance>(D("""{"type":"Utterance","meeting":"m1","track":"System","t0_ms":100,"t1_ms":900,"text":"hello","request":false,"final":false,"speaker":"s1"}"""));
        Assert.False(ut.Final);
        Assert.Equal("s1", ut.Speaker);
        // `final` defaults to true (serde default = yes) when absent.
        Assert.True(Assert.IsType<MeetingChange.Utterance>(D("""{"type":"Utterance","meeting":"m1","track":"Mic","t0_ms":1,"t1_ms":2,"text":"x","request":true}""")).Final);
        Assert.Equal(new MeetingChange.Listening("m1", true), D("""{"type":"Listening","meeting":"m1","held":true}"""));
        Assert.Equal(new MeetingChange.ApprovalWaiting("m1", false), D("""{"type":"ApprovalWaiting","meeting":"m1","waiting":false}"""));
        Assert.Equal(new MeetingChange.PostProgress("m1", "Transcribing", 42), D("""{"type":"PostProgress","meeting":"m1","stage":"Transcribing","pct":42}"""));
        Assert.Equal(new MeetingChange.ReviewReady("m1"), D("""{"type":"ReviewReady","meeting":"m1"}"""));
        Assert.Equal(new MeetingChange.Deleted("m1"), D("""{"type":"Deleted","meeting":"m1"}"""));
        Assert.Equal(new MeetingChange.OracleDeciding("m1", "batch-7"), D("""{"type":"OracleDeciding","meeting":"m1","batch_id":"batch-7"}"""));
        var od = Assert.IsType<MeetingChange.OracleDecision>(D("""{"type":"OracleDecision","meeting":"m1","decision":{"batch_id":"batch-7","disposition":{"kind":"brief"},"tier2":{"model":"x"},"outcome":{"agreed":true}}}"""));
        Assert.Equal(new MeetingOracleRow("batch-7", "brief", true, true), od.Decision);
        Assert.Equal(new MeetingChange.Activity("s1", "read_file…"), D("""{"type":"ToolCallStarted","session":"s1","name":"read_file"}"""));
        Assert.Equal(new MeetingChange.Activity("s1", null), D("""{"type":"TurnFinished","session":"s1","turn":"t","stop_reason":"end_turn","context":1}"""));
        var lv = Assert.IsType<MeetingChange.Level>(D("""{"type":"Level","meeting":"m1","mic_rms":0.1,"system_rms":0.2}"""));
        Assert.Equal((0.1f, 0.2f), (lv.MicRms, lv.SystemRms));
        Assert.Equal(new MeetingChange.Summarized("m1", "Standup"), D("""{"type":"Summarized","meeting":"m1","title":"Standup"}""")); // 158 §18 c6
        Assert.Null(D("""{"type":"AttentionChanged"}"""));
        Assert.Null(MeetingWire.Decode("Brief", "not json"u8));
    }

    [Fact]
    public void DecodesThePromptEvents() // 158 §4.5 / §15.1
    {
        var zoom = new MeetingProcess(4242, "Zoom.exe", null, true);
        var det = Assert.IsType<MeetingChange.Detected>(D("""{"type":"Detected","app":{"pid":4242,"name":"Zoom.exe","bundle_id":null,"active":true},"app_id":"zoom.exe","since":"2026-10-10T10:00:00Z","also":[{"pid":7,"name":"Teams.exe","bundle_id":null,"active":false}],"event":null,"project":null,"brief":null}"""));
        Assert.Equal((zoom, "zoom.exe", "2026-10-10T10:00:00Z", "Zoom"), (det.Detection.App, det.Detection.AppId, det.Detection.Since, det.Detection.App.Display));
        Assert.Equal(new[] { new MeetingProcess(7, "Teams.exe") }, det.Detection.Also);
        // `also` / `event` / `project` / `brief` are `#[serde(default)]`: absent is fine; a missing `app` or `app_id` is malformed.
        Assert.Empty(Assert.IsType<MeetingChange.Detected>(D("""{"type":"Detected","app":{"pid":1,"name":"x"},"app_id":"x","since":""}""")).Detection.Also);
        Assert.Null(D("""{"type":"Detected","app_id":"x","since":""}"""));
        Assert.Equal(new MeetingChange.DetectionCleared("zoom.exe", "NotNow"), D("""{"type":"DetectionCleared","app_id":"zoom.exe","reason":"NotNow"}"""));
        Assert.Equal(new MeetingChange.AppReleased("m1", zoom, 30), D("""{"type":"AppReleased","meeting":"m1","app":{"pid":4242,"name":"Zoom.exe","bundle_id":null,"active":true},"released_for_s":30}"""));
        Assert.Null(D("""{"type":"Upcoming","event":{"id":"e1"},"project":null,"brief":null}""")); // W3's island state (c)
        var now = new DateTimeOffset(2026, 10, 10, 10, 0, 20, TimeSpan.Zero);
        var mid = MeetingWire.Hydrate(JsonDocument.Parse("""{"state":"Idle","meeting":null,"session":null,"elapsed_ms":0,"degraded":[],"supported":{"kind":"Full"},"detection":{"app":{"pid":4242,"name":"Zoom.exe","bundle_id":null,"active":true},"app_id":"zoom.exe","since":"2026-10-10T10:00:00Z","also":[]}}""").RootElement, null, null, now);
        var p = Assert.IsType<MeetingPrompt.Detected>(mid.Prompt);
        Assert.Equal((zoom, "zoom.exe", 0), (p.Detection.App, p.Detection.AppId, p.Detection.Also.Count));
        Assert.Null(mid.Meeting);
    }

    [Fact]
    public void HydratesFromStatusBriefsAndLive()
    {
        using var status = JsonDocument.Parse("""{"state":"Recording","meeting":"m1","session":"s1","elapsed_ms":65000,"degraded":[],"cursor":"b2","topics":[{"id":"t1","label":"x","at_ms":3,"brief":null}],"approval_waiting":true,"supported":{"kind":"Partial","reason":"no aec"},"accelerator":""}""");
        using var briefs = JsonDocument.Parse("""[{"id":"b1","topic":null,"title":"one","bullets":[],"source":"Auto","at_ms":1,"parent":null},{"id":"b2","topic":"t1","title":"two","bullets":["b"],"source":"Cache","at_ms":2,"parent":null}]""");
        using var live = JsonDocument.Parse("""[{"track":"Mic","t0_ms":10,"t1_ms":20,"text":"hi","request":false},{"track":"System","t0_ms":30,"t1_ms":40,"text":"yo","request":false,"speaker":"s2"}]""");
        var now = new DateTimeOffset(2026, 10, 9, 2, 0, 0, TimeSpan.Zero);
        var s = MeetingWire.Hydrate(status.RootElement, briefs.RootElement, live.RootElement, now);
        Assert.Equal("Recording", s.State);
        Assert.Equal(65000, s.ElapsedMs);
        Assert.Equal(1, s.Cursor);
        Assert.Equal("two", s.Current!.Title);
        Assert.True(s.Current.IsAuto);
        Assert.True(s.ApprovalWaiting);
        Assert.Single(s.Topics);
        Assert.Equal(new[] { "me:", "Speaker 2:" }, s.Lines.Select(l => l.Who));
        Assert.Equal("01:05", IslandModel.Clock(s.Elapsed(now)));
        var idle = MeetingWire.Hydrate(JsonDocument.Parse("""{"state":"Idle","meeting":null,"session":null,"elapsed_ms":0,"degraded":[],"supported":{"kind":"Full"}}""").RootElement, null, null, now);
        Assert.Null(idle.Meeting);
        Assert.Equal(-1, idle.Cursor);
    }
}
