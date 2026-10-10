using System.Text.Json;

namespace Theuth.Companion.Rpc.Tests;

/// <summary>Design 149 WW1 (§10 unit tests): the new wire shapes and the stream's tolerance — unknown types and unknown fields never throw.</summary>
public sealed class Models149Tests
{
    [Fact]
    public void UnknownEventTypeDecodesAsOther()
    {
        var e = WireEvent.Decode("""{"type":"SomethingFromNextYear","session":"s1","extra":{"deep":[1,2,3]}}""");
        Assert.True(e.IsOther);
        Assert.Equal("SomethingFromNextYear", e.Type);
    }

    [Fact]
    public void KnownEventWithExtraFieldsDecodes()
    {
        var e = WireEvent.Decode("""{"type":"SubagentSpawned","parent":"p","child":"c","name":"ww1","cap":"read_only","future_field":true,"nested":{"x":1}}""");
        var s = Assert.IsType<SubagentSpawned>(e.Payload);
        Assert.Equal(("p", "c", "ww1", "read_only"), (s.Parent, s.Child, s.Name, s.Cap));
    }

    [Fact]
    public void MeetingEventsEveryVariantDecodes()
    {
        string[] items =
        {
            """{"type":"Started","meeting":"m1","session":"s1","process":{"pid":12,"name":"zoom","bundle_id":null,"active":true},"at":"2026-10-09T01:00:00Z"}""",
            """{"type":"State","meeting":"m1","state":"Recording","degraded":["MicNoAec",{"SttDown":"Mic"},{"PermissionDenied":"Microphone"}]}""",
            """{"type":"State","meeting":"m1","state":{"Post":"Transcribing"},"degraded":[]}""",
            """{"type":"Level","meeting":"m1","mic_rms":0.25,"system_rms":0.5}""",
            """{"type":"Utterance","meeting":"m1","track":"System","t0_ms":10,"t1_ms":900,"text":"hello","request":false,"speaker":"Speaker 1"}""",
            """{"type":"Utterance","meeting":"m1","track":"Mic","t0_ms":10,"t1_ms":900,"text":"partial","request":true,"final":false}""",
            """{"type":"TopicAdded","meeting":"m1","topic":{"id":"t1","label":"ticket 149","at_ms":5,"brief":null,"cached":"b9","source":"Auto"}}""",
            """{"type":"TopicRemoved","meeting":"m1","topic":"t1","why":"Dismissed"}""",
            """{"type":"Brief","meeting":"m1","brief":{"id":"b1","topic":"t1","title":"149","bullets":["a","b"],"source":"Tap","at_ms":7,"parent":null}}""",
            """{"type":"Listening","meeting":"m1","held":true}""",
            """{"type":"ApprovalWaiting","meeting":"m1","waiting":true}""",
            """{"type":"PostProgress","meeting":"m1","stage":"Diarizing","pct":40}""",
            """{"type":"ReviewReady","meeting":"m1"}""",
            """{"type":"Summarized","meeting":"m1","title":"Standup"}""",
            """{"type":"Deleted","meeting":"m1"}""",
            """{"type":"OracleDeciding","meeting":"m1","batch_id":"k"}""",
            """{"type":"OracleDecision","meeting":"m1","decision":{"batch_id":"k","family":"x","mode":"shadow","disposition":"Skip","stages":[["a",0.5]],"mentions":[],"subject":null,"tier2":null}}""",
        };
        var decoded = items.Select(WireEvent.Decode).ToList();
        Assert.All(decoded, e => Assert.IsType<MeetingEvent>(e.Payload));
        var events = decoded.Select(e => (MeetingEvent)e.Payload!).ToList();
        Assert.Equal(new MeetingState("Recording"), events[1].State);
        Assert.Equal(new[] { new Degradation("MicNoAec"), new Degradation("SttDown", "Mic"), new Degradation("PermissionDenied", "Microphone") }, events[1].Degraded);
        Assert.Equal(new MeetingState("Post", "Transcribing"), events[2].State);
        // Words beyond the design's list (NoNote, DiarizationDown) and a word from a newer engine decode as themselves; Support keeps its reason.
        var later = WireEvent.Decode("""{"type":"State","meeting":"m1","state":"Stopping","degraded":["NoNote","DiarizationDown","BrandNew2027",{"Future":{"x":1}}]}""");
        Assert.Equal(new[] { "NoNote", "DiarizationDown", "BrandNew2027", "Future" }, ((MeetingEvent)later.Payload!).Degraded.Select(d => d.Kind));
        Assert.Equal(new MeetingState("Stopping"), ((MeetingEvent)later.Payload!).State);
        var partial = TheuthJson.Deserialize<MeetingSupport>("""{"kind":"Partial","reason":"no system audio"}""");
        Assert.Equal(("Partial", "no system audio"), (partial.Kind, partial.Reason));
        Assert.Equal(("None", "no devices"), (TheuthJson.Deserialize<MeetingSupport>("""{"kind":"None","reason":"no devices"}""").Kind, TheuthJson.Deserialize<MeetingSupport>("""{"kind":"None","reason":"no devices"}""").Reason));
        Assert.Equal(0.25f, events[3].MicRms);
        Assert.Equal("Speaker 1", events[4].AsUtterance!.Who);
        Assert.True(events[4].IsFinal ?? true);
        Assert.False(events[5].IsFinal);
        Assert.Equal("ticket 149", events[6].TopicAdded!.Label);
        Assert.Null(events[6].TopicRemovedId);
        Assert.Equal("t1", events[7].TopicRemovedId);
        Assert.Null(events[7].TopicAdded);
        Assert.Equal(new[] { "a", "b" }, events[8].Brief!.Bullets);
        Assert.True(events[9].Held);
        Assert.Equal((byte)40, events[11].Pct);
        Assert.Equal(15, MeetingEvent.Types.Count);
    }

    [Fact]
    public void DictationEventsAreTyped()
    {
        var e = WireEvent.Decode("""{"type":"DictationLevel","id":"d1","levels":[0.1,0.2],"new_field":1}""");
        var d = Assert.IsType<DictationEvent>(e.Payload);
        Assert.Equal(new[] { 0.1f, 0.2f }, d.Levels);
        var t = Assert.IsType<DictationEvent>(WireEvent.Decode("""{"type":"DictationText","id":"d1","utterance":0,"text":"hi","final":true}""").Payload);
        Assert.True(t.IsFinal);
    }

    [Fact]
    public void AttentionItemContextIsOptional()
    {
        var with = TheuthJson.Deserialize<AttentionItem>("""{"id":"ask:1","kind":"ask","title":"T","at_ms":1,"session":"s1","context":{"request":"ship it?","last_reply":"done with WW1"}}""");
        Assert.Equal("ship it?", with.Context!.Request);
        Assert.Equal("done with WW1", with.Context.LastReply);
        var without = TheuthJson.Deserialize<AttentionItem>("""{"id":"ask:1","kind":"ask","title":"T","at_ms":1}""");
        Assert.Null(without.Context);
        var nulled = TheuthJson.Deserialize<AttentionItem>("""{"id":"ask:1","kind":"ask","title":"T","at_ms":1,"context":null}""");
        Assert.Null(nulled.Context);
        var half = TheuthJson.Deserialize<AttentionItem>("""{"id":"ask:1","kind":"ask","title":"T","at_ms":1,"context":{"request":null,"last_reply":"x"}}""");
        Assert.Null(half.Context!.Request);
    }

    [Fact]
    public void SessionSummaryOldAndNewEngines()
    {
        var old = TheuthJson.Deserialize<SessionSummary>("""{"id":"s1","created_at":"2026-10-09T00:00:00Z","updated_at":"2026-10-09T00:00:01.123456789Z","title":"fix the build","model":"m","parent":null,"name":null,"effort":null,"agent":null,"cwd":null,"pinned":false}""");
        Assert.False(old.Running);
        Assert.Equal("", old.Owner);
        Assert.True(old.Here);
        var now = TheuthJson.Deserialize<SessionSummary>("""{"id":"s2","created_at":"2026-10-09T00:00:00Z","updated_at":"2026-10-09T00:00:00Z","title":"t","tags":[],"model":"m","parent":"s1","name":"ww1","effort":"high","agent":"omp","cwd":"/x","desk":null,"pinned":false,"archived":false,"project":"theuth","running":true,"owner":"eng-b","epoch":3,"here":false,"kind":"chat","memory":"epochs"}""");
        Assert.True(now.Running);
        Assert.Equal(("eng-b", false, "omp", "theuth"), (now.Owner, now.Here, now.Agent, now.Project));
        var status = TheuthJson.Deserialize<SessionsStatus>("""{"engine":"eng-a","here":12,"elsewhere":3,"log_seq":99,"last_snapshot":null}""");
        Assert.Equal(("eng-a", 12u, 3u), (status.Engine, status.Here, status.Elsewhere));
    }

    [Fact]
    public void MeetingStatusAndCachedBriefDecode()
    {
        var s = TheuthJson.Deserialize<MeetingStatus>("""{"state":"Idle","meeting":null,"session":null,"elapsed_ms":0,"degraded":[],"supported":{"kind":"Full"},"accelerator":"cpu"}""");
        Assert.False(s.IsLive);
        Assert.Empty(s.Topics);
        Assert.True(s.Supported.IsFull);
        var live = TheuthJson.Deserialize<MeetingStatus>("""{"state":"Paused","meeting":"m1","session":"s1","elapsed_ms":1200,"degraded":["DiskLow"],"cursor":"b1","topics":[{"id":"t1","label":"x","at_ms":1,"brief":"b1"}],"approval_waiting":true,"supported":{"kind":"Partial","reason":"no aec"},"accelerator":""}""");
        Assert.True(live.IsLive);
        Assert.Equal("b1", live.MeetingCursorOrNull());
        Assert.Equal("Partial", live.Supported.Kind);
        var brief = TheuthJson.Deserialize<CachedBrief>("""{"id":"cb1","title":"PR 12","bullets":["x"],"aliases":["twelve"],"tags":["standup"],"source":"user","created_at":"2026-10-08","updated_at":"2026-10-08","uses":2,"last_used_at":null}""");
        Assert.Equal(2u, brief.Uses);
        var perms = TheuthJson.Deserialize<MeetingPermissions>("""{"microphone":"Granted","audio_capture":"NotDetermined"}""");
        Assert.Equal("NotDetermined", perms.AudioCapture);
        var mode = TheuthJson.Deserialize<ModeInfo>("""{"mode":"shadow","effective":"off","reason":"not installed","enabled":true,"installed":false,"budget_ms":400,"escalate_budget_ms":1500}""");
        Assert.False(mode.Installed);
    }

    [Fact]
    public void ParamsEncodeInApiOrder()
    {
        Assert.Equal("""["s1","tree"]""", TheuthJson.EncodeParams("s1", CancelScope.Tree.Word()));
        Assert.Equal("""["s1",null]""", TheuthJson.EncodeParams("s1", ((CancelScope?)null)?.Word()));
        Assert.Equal("""[null,"look",["/tmp/a.png"],"next_turn",null]""", TheuthJson.EncodeParams(null, "look", new[] { "/tmp/a.png" }, Delivery.NextTurn.Word(), null));
        var req = new ScreenContextReq { Samples = new ValueList<PointerSample> { new(0, 1.5f, 2) }, Color = "#ff0000", SinceMs = 3000, Accessibility = true };
        Assert.Equal("""[{"samples":[{"t_ms":0,"x":1.5,"y":2,"pen":true}],"color":"#ff0000","since_ms":3000,"accessibility":true}]""", TheuthJson.EncodeParams(req));
        var reply = TheuthJson.Deserialize<ScreenContextReply>("""{"lines":["[screen 1 …]"],"files":["C:\\a.png"]}""");
        Assert.Single(reply.Files);
    }

    [Fact]
    public void DetectionDecodesAndStartMeetingEncodesThePromptStart() // 158 §5.4, §15.1
    {
        var s = TheuthJson.Deserialize<MeetingStatus>("""{"state":"Idle","meeting":null,"session":null,"elapsed_ms":0,"degraded":[],"supported":{"kind":"Full"},"accelerator":"","detection":{"app":{"pid":4242,"name":"Zoom.exe","bundle_id":null,"active":true},"app_id":"zoom.exe","since":"2026-10-10T10:00:00Z","also":[],"event":null,"project":null,"brief":null}}""");
        Assert.Equal((4242u, "Zoom.exe", "zoom.exe"), (s.Detection!.App.Pid, s.Detection.App.Name, s.Detection.AppId));
        Assert.Null(TheuthJson.Deserialize<MeetingStatus>("""{"state":"Idle","meeting":null,"session":null,"elapsed_ms":0,"degraded":[],"supported":{"kind":"Full"},"accelerator":""}""").Detection);
        // The one-tap start: pid, consent, source — everything else omitted so the engine fills its defaults (§6).
        Assert.Equal("""[{"process":4242,"consent":true,"source":{"kind":"detected","app_id":"zoom.exe"}}]""",
            TheuthJson.EncodeParams(new StartMeeting { Process = 4242, Consent = true, Source = StartSource.Detected("zoom.exe") }));
        Assert.Equal("""{"kind":"calendar","event":"ics:u1:"}""", TheuthJson.Serialize(StartSource.Calendar("ics:u1:")));
    }
}

internal static class MeetingStatusTestExt
{
    public static string? MeetingCursorOrNull(this MeetingStatus s) => s.Cursor;
}
