using System.Globalization;
using System.Text;
using System.Text.Json;

namespace Theuth.Companion.Core.Tests;

/// <summary>192w P1: which fences are pictures, and what the parser makes of them while a reply streams.</summary>
public sealed class PictureFenceTests
{
    [Fact]
    public void LangNamesTheEnginesThreePictureFencesInAnyCase()
    {
        Assert.Equal("json-render", PictureFence.Lang("json-render"));
        Assert.Equal("json-render", PictureFence.Lang("jsonrender")); // the alias
        Assert.Equal("json-render", PictureFence.Lang("JSON-Render"));
        Assert.Equal("mermaid", PictureFence.Lang("  mermaid  with a title "));
        Assert.Equal("openui", PictureFence.Lang("openui"));
        foreach (var other in new[] { "", "  ", "rust", "json", "jsonrenderer", "mermaidx", "diagram", "table" }) Assert.Null(PictureFence.Lang(other));
        Assert.Null(PictureFence.Lang(null));
    }

    [Fact]
    public void AClosedPictureFenceIsAPictureBlockBetweenItsMarkdown()
    {
        var blocks = AnswerMarkdown.Parse("Intro.\n\n```mermaid\ngraph LR\n  A --> B\n```\n\nOutro.");
        Assert.Equal(new[] { 'P', 'G', 'P' }, blocks.Select(MdDiff.Kind).ToArray());
        Assert.Equal(new MdBlock.Picture("mermaid", "graph LR\n  A --> B", true), blocks[1]);
    }

    [Fact]
    public void AliasesAndInfoStringsReachTheViewAsTheEnginesNames()
    {
        Assert.Equal(new MdBlock.Picture("json-render", "{}", true), Assert.Single(AnswerMarkdown.Parse("```jsonrender\n{}\n```")));
        Assert.Equal(new MdBlock.Picture("mermaid", "graph TD", true), Assert.Single(AnswerMarkdown.Parse("```Mermaid some title\ngraph TD\n```")));
        Assert.Equal(new MdBlock.Picture("openui", "root = Stack([])", true), Assert.Single(AnswerMarkdown.Parse("  ```openui\nroot = Stack([])\n  ```")));
    }

    [Fact]
    public void AFenceStillArrivingIsAnOpenPictureWhileStreamingAndCodeOnceTheReplyEnded()
    {
        const string text = "Here:\n\n```mermaid\ngraph LR\n  A -->";
        var open = AnswerMarkdown.Parse(text, streaming: true);
        Assert.Equal(new MdBlock.Picture("mermaid", "graph LR\n  A -->", false), open[1]);
        // A reply that ended with the fence open has no picture to wait for: code.
        var ended = AnswerMarkdown.Parse(text, streaming: false);
        Assert.Equal(new MdBlock.Code("mermaid", "graph LR\n  A -->"), ended[1]);
    }

    [Fact]
    public void AClosingFenceStillBeingTypedKeepsThePictureOpenAndTheFullOneCloses()
    {
        Assert.False(Assert.IsType<MdBlock.Picture>(AnswerMarkdown.Parse("```mermaid\ngraph LR\n``", streaming: true)[0]).Closed);
        var done = Assert.IsType<MdBlock.Picture>(AnswerMarkdown.Parse("```mermaid\ngraph LR\n```", streaming: true)[0]);
        Assert.True(done.Closed);
        Assert.Equal("graph LR", done.Source);
    }

    [Fact]
    public void OtherFencesStayCodeBlocks()
    {
        foreach (var fence in new[] { "```rust\nfn main() {}\n```", "```json\n{}\n```", "```\nplain\n```", "```table\n| a |\n```" })
        {
            Assert.IsType<MdBlock.Code>(Assert.Single(AnswerMarkdown.Parse(fence)));
            Assert.IsType<MdBlock.Code>(Assert.Single(AnswerMarkdown.Parse(fence, streaming: true)));
        }
    }

    [Fact]
    public void AnOpeningFenceStillBeingTypedIsHeldBackAndNeverFlashesAnEmptyCodeBox()
    {
        foreach (var stub in new[] { "```", "```m", "```mer", "```mermai", "```json-", "```op" })
        {
            var blocks = AnswerMarkdown.Parse("Text.\n\n" + stub, streaming: true);
            Assert.Single(blocks); // only the paragraph
            Assert.IsType<MdBlock.Paragraph>(blocks[0]);
        }
        // The complete name is a picture fence (drawing…), a word that cannot become one is code, and a final text holds nothing back.
        Assert.Equal(new MdBlock.Picture("mermaid", "", false), Assert.Single(AnswerMarkdown.Parse("```mermaid", streaming: true)));
        Assert.IsType<MdBlock.Code>(Assert.Single(AnswerMarkdown.Parse("```rust", streaming: true)));
        Assert.IsType<MdBlock.Code>(Assert.Single(AnswerMarkdown.Parse("```mer", streaming: false)));
    }

    [Fact]
    public void SeveralFencesAndTheTextBetweenThemKeepTheirOrder()
    {
        var blocks = AnswerMarkdown.Parse("A\n\n```mermaid\ng\n```\n\nB\n\n```json-render\n{}\n```\n```rust\nx\n```\n\nC");
        Assert.Equal(new[] { 'P', 'G', 'P', 'G', 'C', 'P' }, blocks.Select(MdDiff.Kind).ToArray());
        Assert.Equal(new[] { "mermaid", "json-render" }, blocks.OfType<MdBlock.Picture>().Select(p => p.Lang).ToArray());
    }

    [Fact]
    public void ASignatureTellsAnOpenPictureFromAClosedOneAndOnePictureFromAnother()
    {
        var open = new MdBlock.Picture("mermaid", "graph LR", false);
        var closed = open with { Closed = true };
        var other = open with { Source = "graph TD" };
        var sigs = new[] { open, closed, other, open with { Lang = "openui" } }.Select(MdDiff.Signature).Distinct().ToArray();
        Assert.Equal(4, sigs.Length);
        Assert.Equal('G', MdDiff.Kind(open));
    }

    [Fact]
    public void AFenceThatClosesIsTheBlockBeingWrittenAndTheTextBeforeItStaysUntouched()
    {
        var before = AnswerMarkdown.Parse("Here:\n\n```mermaid\ngraph LR\n  A --> B", streaming: true);
        var after = AnswerMarkdown.Parse("Here:\n\n```mermaid\ngraph LR\n  A --> B\n```\n\nDone.", streaming: true);
        int keep = MdDiff.CommonPrefix(before.Select(MdDiff.Signature).ToList(), after.Select(MdDiff.Signature).ToList());
        Assert.Equal(1, keep); // the paragraph stays; the picture is updated in place (same kind) and the new paragraph is built
        Assert.Equal(MdDiff.Kind(before[1]), MdDiff.Kind(after[1]));
    }
}

/// <summary>192w: model text that cannot carry a picture never shows a fence body — reasoning, the two-line preview, speech.</summary>
public sealed class PictureFlattenTests
{
    private const string Card = "```json-render\n{\"root\":\"card\",\"elements\":{}}\n```";

    [Fact]
    public void ClosedAndOpenPictureFencesBecomeOneLine()
    {
        Assert.Equal("a\n(diagram)\nb", PictureFence.Flatten($"a\n{Card}\nb"));
        Assert.Equal("a\n(diagram)", PictureFence.Flatten("a\n```mermaid\ngraph LR\n  A -->")); // still arriving: nothing after it shows
        Assert.Equal("a\nb", PictureFence.Flatten($"a\n{Card}\nb", ""));
        Assert.Equal("", PictureFence.Flatten(null));
    }

    [Fact]
    public void OtherFencesAndPlainTextPassThroughUnchanged()
    {
        const string code = "x\n```rust\nlet y = 2;\n```\nz";
        Assert.Equal(code, PictureFence.Flatten(code));
        Assert.Equal("no fence at all", PictureFence.Flatten("no fence at all"));
        // A picture fence after another fence is still one; the other fence keeps every line.
        Assert.Equal("```rust\nlet y = 2;\n```\n(diagram)\nz", PictureFence.Flatten("```rust\nlet y = 2;\n```\n```mermaid\ng\n```\nz"));
    }

    [Fact]
    public void SpeechDoesNotReadADiagramAndTheGapsCollapse()
    {
        Assert.Equal("Here it is.\n\nDone.", PictureFence.StripForVoice($"Here it is.\n\n{Card}\n\nDone."));
        Assert.Equal("", PictureFence.StripForVoice(Card));
        Assert.Equal("plain", PictureFence.StripForVoice("  plain \n"));
    }

    [Fact]
    public void TheTwoLinePreviewNeverShowsAFenceBody()
    {
        var closed = new AskReply("s") { Text = $"Here:\n{Card}\nDone" };
        Assert.Equal("Here: — (diagram)", closed.FirstTwoLines);
        var open = new AskReply("s") { Text = "Here:\n```mermaid\ngraph LR\n  A --> B" };
        Assert.Equal("Here: — (diagram)", open.FirstTwoLines);
        Assert.DoesNotContain("graph", open.FirstTwoLines, StringComparison.Ordinal);
        Assert.Equal("(diagram)", new AskReply("s") { Text = Card }.FirstTwoLines);
    }

    [Fact]
    public void AReasoningThatQuotesAFenceNeverShowsItsBodyHoweverItIsCut()
    {
        // Design 164's check: replay the text a few bytes at a time and assert no step shows a fence line, "root" or "elements".
        const string reasoning = "I will answer with a card.\n```json-render\n{\"root\":\"card\",\"elements\":{\"card\":{\"type\":\"Card\"}}}\n```\nand a flowchart:\n```mermaid\ngraph LR\n  A --> B\n```\nDone thinking.";
        var thinking = new ThinkingState();
        for (int i = 0; i < reasoning.Length; i += 3)
        {
            thinking.Delta(reasoning.Substring(i, Math.Min(3, reasoning.Length - i)), 1000 + i);
            var shown = thinking.Text;
            Assert.DoesNotContain("\"root\"", shown, StringComparison.Ordinal);
            Assert.DoesNotContain("elements", shown, StringComparison.Ordinal);
            Assert.DoesNotContain("graph LR", shown, StringComparison.Ordinal);
            Assert.DoesNotContain("```json-render", shown, StringComparison.Ordinal);
            Assert.DoesNotContain("```mermaid", shown, StringComparison.Ordinal);
        }
        Assert.Equal("I will answer with a card.\n(diagram)\nand a flowchart:\n(diagram)\nDone thinking.", thinking.Text);
    }
}

/// <summary>192w P2: the cache key, the outcome → picture-or-fallback mapping (incl. -32601), and the cache itself.</summary>
public sealed class PictureCacheTests
{
    // A real 1×1 PNG, and a header-only "PNG" 446×300 (the mapping reads the signature and the IHDR size, it does not decode).
    private const string TinyPng = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    private static string HeaderPng(int w, int h)
    {
        var b = new byte[33];
        new byte[] { 0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, (byte)'I', (byte)'H', (byte)'D', (byte)'R' }.CopyTo(b, 0);
        for (int i = 0; i < 4; i++)
        {
            b[16 + i] = (byte)(w >> (24 - 8 * i));
            b[20 + i] = (byte)(h >> (24 - 8 * i));
        }
        return Convert.ToBase64String(b);
    }

    private static PictureKey Key(string lang = "mermaid", string source = "graph LR", int width = 446, string appearance = "dark", double scale = 1.25) =>
        PictureKey.Of(lang, source, width, appearance, scale);

    // MARK: the key

    [Fact]
    public void TheKeyClampsWidthAndScaleLikeTheEngineAndNamesTheAppearance()
    {
        Assert.Equal(new PictureKey("mermaid", "g", 240, "dark", 1), PictureKey.Of("mermaid", "g", 100, "Dark", 0.5));
        Assert.Equal(new PictureKey("mermaid", "g", 1200, "light", 3), PictureKey.Of("mermaid", "g", 5000, "light", 5));
        Assert.Equal(new PictureKey("mermaid", "g", 446, "dark", 1.25), PictureKey.Of("mermaid", "g", 446.9, "anything else", 1.25));
        Assert.Equal(PictureKey.MinWidth, PictureKey.Of("mermaid", "g", double.NaN, "dark", double.NaN).Width);
        Assert.Equal(1, PictureKey.Of("mermaid", "g", 400, "dark", double.PositiveInfinity).Scale);
    }

    [Fact]
    public void TheSameFenceAtAnotherWidthThemeScaleOrSourceIsAnotherPicture()
    {
        Assert.Equal(Key(), Key());
        Assert.Equal(Key(width: 100), Key(width: 239)); // both clamp to 240
        Assert.NotEqual(Key(), Key(width: 447));
        Assert.NotEqual(Key(), Key(appearance: "light"));
        Assert.NotEqual(Key(), Key(scale: 1.5));
        Assert.NotEqual(Key(), Key(source: "graph TD"));
        Assert.NotEqual(Key(), Key(lang: "openui"));
    }

    // MARK: the outcome → picture or fallback

    [Fact]
    public void ADoneAnswerWithAPngIsReadyAtItsPixelSize()
    {
        var ready = PictureOutcome.Map(new FenceSnapshot { Outcome = "done", Png = TinyPng, Width = 557, Height = 190 }, null, 1.25, 2314);
        Assert.Equal(PictureState.Ready, ready.State);
        Assert.Equal((557, 190, 1.25, 2314L, "done"), (ready.PixelWidth, ready.PixelHeight, ready.Scale, ready.Ms, ready.Outcome));
        Assert.True(PictureOutcome.IsPng(ready.Png!));
        // Without width and height the PNG's own header says.
        var sized = PictureOutcome.Map(new FenceSnapshot { Outcome = "done", Png = HeaderPng(446, 300) }, null, 1, 5);
        Assert.Equal((446, 300), (sized.PixelWidth, sized.PixelHeight));
        Assert.Equal((1, 1), PictureOutcome.PngSize(Convert.FromBase64String(TinyPng)));
    }

    [Fact]
    public void EveryOtherOutcomeIsAFallbackWithItsOwnReason()
    {
        PictureResult Map(FenceSnapshot s) => PictureOutcome.Map(s, null, 1, 7);
        Assert.Equal(("no_renderer", PictureState.Fallback), (Map(new() { Outcome = "no_renderer" }).Outcome, Map(new() { Outcome = "no_renderer" }).State));
        var failed = Map(new() { Outcome = "failed", Error = " mermaid: parse error at line 2 " });
        Assert.Equal(("failed", "mermaid: parse error at line 2"), (failed.Outcome, failed.Reason));
        Assert.Equal("the renderer failed", Map(new() { Outcome = "failed" }).Reason);
        Assert.Equal("timed_out", Map(new() { Outcome = "timed_out" }).Outcome);
        Assert.Equal("cancelled", Map(new() { Outcome = "cancelled" }).Outcome);
        Assert.Equal(("later_word", PictureState.Fallback), (Map(new() { Outcome = "later_word" }).Outcome, Map(new() { Outcome = "later_word" }).State));
        Assert.Equal("failed", Map(new() { Outcome = "" }).Outcome);
        Assert.Equal("failed", PictureOutcome.Map(null, null, 1, 0).Outcome);
    }

    [Fact]
    public void ADoneAnswerWithNothingUsableIsAFallbackNotAnEmptyPicture()
    {
        Assert.Equal(PictureState.Fallback, PictureOutcome.Map(new FenceSnapshot { Outcome = "done" }, null, 1, 0).State);
        Assert.Equal(PictureState.Fallback, PictureOutcome.Map(new FenceSnapshot { Outcome = "done", Png = "" }, null, 1, 0).State);
        Assert.Equal("the picture is not base64", PictureOutcome.Map(new FenceSnapshot { Outcome = "done", Png = "***" }, null, 1, 0).Reason);
        Assert.Equal("the picture is not a PNG", PictureOutcome.Map(new FenceSnapshot { Outcome = "done", Png = Convert.ToBase64String(new byte[40]) }, null, 1, 0).Reason);
    }

    [Fact]
    public void AnEngineWithoutTheMethodAndOtherFailuresOfTheCallAreFallbacks()
    {
        var old = PictureOutcome.Map(null, new RpcException(-32601, "Method not found"), 1, 3);
        Assert.Equal((PictureState.Fallback, "unsupported"), (old.State, old.Outcome));
        Assert.Equal("timed_out", PictureOutcome.Map(null, new TimeoutException("late"), 1, 0).Outcome);
        Assert.Equal(("error", "rpc -32000: boom"), (PictureOutcome.Map(null, new RpcException(-32000, "boom"), 1, 0).Outcome, PictureOutcome.Map(null, new RpcException(-32000, "boom"), 1, 0).Reason));
        Assert.Equal(("error", "socket closed"), (PictureOutcome.Map(null, new IOException("socket closed"), 1, 0).Outcome, PictureOutcome.Map(null, new IOException("socket closed"), 1, 0).Reason));
        Assert.Equal("error", PictureOutcome.Map(null, RpcException.NotConnected, 1, 0).Outcome);
    }

    // MARK: the cache

    private sealed class Fetcher
    {
        public readonly List<PictureKey> Started = new();
        public readonly Dictionary<PictureKey, TaskCompletionSource<FenceSnapshot>> Pending = new();

        public Task<FenceSnapshot> Fetch(PictureKey key)
        {
            Started.Add(key);
            var tcs = new TaskCompletionSource<FenceSnapshot>();
            Pending[key] = tcs;
            return tcs.Task;
        }

        public void Answer(PictureKey key, FenceSnapshot snapshot) => Pending[key].SetResult(snapshot);

        public void Fail(PictureKey key, Exception e) => Pending[key].SetException(e);
    }

    private readonly ManualScheduler clock = new();
    private readonly Fetcher fetcher = new();
    private readonly List<string> logs = new();
    private readonly PictureCache cache;

    public PictureCacheTests() => cache = new PictureCache(fetcher.Fetch, clock, () => clock.Now, logs.Add);

    private static FenceSnapshot Done(int w = 557, int h = 190) => new() { Outcome = "done", Png = TinyPng, Width = w, Height = h };

    [Fact]
    public void ARequestStartsOneDrawAndTheSameKeyShareIt()
    {
        var key = Key();
        var seen = new List<PictureResult>();
        Assert.Equal(PictureState.Drawing, cache.Request(key).State);
        cache.Watch(key, seen.Add);
        Assert.Equal(PictureState.Drawing, cache.Request(key).State); // a rebuilt view asks again: no second call
        Assert.Equal(1, cache.Fetches);
        fetcher.Answer(key, Done());
        Assert.Equal(PictureState.Ready, cache.Request(key).State);
        Assert.Equal(new[] { PictureState.Ready }, seen.Select(r => r.State).ToArray());
        Assert.Equal(1, cache.Fetches);
    }

    [Fact]
    public void AtMostTwoDrawsAreOutAndTheRestWaitTheirTurn()
    {
        var a = Key(source: "a"); var b = Key(source: "b"); var c = Key(source: "c");
        cache.Request(a); cache.Request(b); cache.Request(c);
        Assert.Equal((2, 1, 2), (cache.InFlight, cache.Queued, cache.Fetches));
        Assert.Equal(new[] { a, b }, fetcher.Started);
        fetcher.Answer(a, Done());
        Assert.Equal((2, 0, 3), (cache.InFlight, cache.Queued, cache.Fetches)); // c took a's place
        Assert.Equal(c, fetcher.Started[2]);
        fetcher.Answer(b, Done());
        fetcher.Answer(c, Done());
        Assert.Equal(0, cache.InFlight);
        Assert.All(new[] { a, b, c }, k => Assert.Equal(PictureState.Ready, cache.Request(k).State));
    }

    [Fact]
    public void FailuresAreKeptTooSoAFenceIsAskedForOnce()
    {
        var key = Key();
        cache.Request(key);
        fetcher.Answer(key, new FenceSnapshot { Outcome = "no_renderer" });
        var again = cache.Request(key);
        Assert.Equal((PictureState.Fallback, "no_renderer"), (again.State, again.Outcome));
        Assert.Equal(1, cache.Fetches);
        Assert.Contains(logs, l => l == "picture: cache hit mermaid (fallback (no_renderer): no renderer is connected)");
    }

    [Fact]
    public void CacheHitsAreLoggedNotMoreOftenThanEveryFiveSeconds()
    {
        var key = Key();
        cache.Request(key);
        fetcher.Answer(key, Done());
        for (int i = 0; i < 5; i++) { cache.Request(key); clock.Advance(1); }
        Assert.Single(logs, l => l.StartsWith("picture: cache hit", StringComparison.Ordinal));
        clock.Advance(5);
        cache.Request(key);
        Assert.Equal(2, logs.Count(l => l.StartsWith("picture: cache hit", StringComparison.Ordinal)));
    }

    [Fact]
    public void AnEngineWithoutTheMethodIsAskedOnceUntilTheConnectionReopens()
    {
        var a = Key(source: "a");
        cache.Request(a);
        fetcher.Fail(a, new RpcException(-32601, "Method not found"));
        Assert.True(cache.Unsupported);
        Assert.Equal("unsupported", cache.Request(a).Outcome);
        var b = Key(source: "b");
        Assert.Equal("unsupported", cache.Request(b).Outcome); // no call goes out
        Assert.Equal(1, cache.Fetches);
        Assert.Contains(logs, l => l.StartsWith("picture: fence_snapshot not asked for mermaid", StringComparison.Ordinal));
        cache.Reset();
        Assert.False(cache.Unsupported);
        Assert.Equal(PictureState.Drawing, cache.Request(b).State); // asked again after a reconnect
        Assert.Equal(2, cache.Fetches);
    }

    [Fact]
    public void ADrawThatNeverAnswersEndsAsATimeoutAfterTheDeadline()
    {
        var key = Key();
        var seen = new List<PictureResult>();
        cache.Request(key);
        cache.Watch(key, seen.Add);
        clock.Advance(29);
        Assert.Equal(PictureState.Drawing, cache.Request(key).State);
        clock.Advance(2); // 31 s
        var result = cache.Request(key);
        Assert.Equal((PictureState.Fallback, "timed_out"), (result.State, result.Outcome));
        Assert.Single(seen);
        Assert.Equal(0, cache.InFlight); // the slot is free again
        fetcher.Answer(key, Done()); // a late answer changes nothing
        Assert.Equal("timed_out", cache.Request(key).Outcome);
    }

    [Fact]
    public void AReconnectEndsDrawsInFlightAsFallbackAndKeepsThePicturesAlreadyDrawn()
    {
        var drawn = Key(source: "drawn"); var waiting = Key(source: "waiting");
        cache.Request(drawn);
        fetcher.Answer(drawn, Done());
        var seen = new List<PictureResult>();
        cache.Request(waiting);
        cache.Watch(waiting, seen.Add);
        cache.Reset();
        Assert.Equal(("reconnect", PictureState.Fallback), (seen.Single().Outcome, seen.Single().State));
        Assert.Equal(PictureState.Ready, cache.Request(drawn).State);
        fetcher.Answer(waiting, Done()); // the dropped call answers late: it does not resurrect the entry
        Assert.Equal(PictureState.Drawing, cache.Request(waiting).State); // a rebuilt view asks again
        Assert.Equal(3, cache.Fetches);
    }

    [Fact]
    public void TheLogCarriesTheRequestAndTheOutcomeWithLangMillisecondsAndBytes()
    {
        var key = Key(source: "graph LR\n  A --> B");
        cache.Request(key);
        clock.Advance(2.3);
        fetcher.Answer(key, Done(557, 190));
        Assert.Equal("picture: fence_snapshot request mermaid (18 chars) width=446 dark x1.25", logs[0]);
        Assert.Equal($"picture: fence_snapshot done mermaid in 2300 ms, {Convert.FromBase64String(TinyPng).Length} bytes (557x190)", logs[1]);
        var bad = Key(source: "bad");
        cache.Request(bad);
        fetcher.Answer(bad, new FenceSnapshot { Outcome = "failed", Error = "boom" });
        Assert.Contains(logs, l => l == "picture: fence_snapshot failed mermaid in 0 ms: boom");
    }

    [Fact]
    public void WatchingAFinishedKeyListensToNothingAndDisposingStopsAWatcher()
    {
        var done = Key(source: "done");
        cache.Request(done);
        fetcher.Answer(done, Done());
        var count = 0;
        cache.Watch(done, _ => count++).Dispose();
        var live = Key(source: "live");
        cache.Request(live);
        cache.Watch(live, _ => count += 10).Dispose();
        cache.Watch(live, _ => count += 100);
        fetcher.Answer(live, Done());
        Assert.Equal(100, count);
    }

    [Fact]
    public void ThePicturesKeptAreCappedAndTheOldestFinishedOnesGoFirst()
    {
        for (int i = 0; i < PictureCache.Capacity + 5; i++)
        {
            var key = Key(source: $"g{i}");
            cache.Request(key);
            fetcher.Answer(key, Done());
        }
        Assert.Equal(PictureCache.Capacity, cache.Count);
        Assert.Equal(PictureState.Drawing, cache.Request(Key(source: "g0")).State); // the oldest was dropped, so it is asked for again
        Assert.Equal(PictureState.Ready, cache.Request(Key(source: $"g{PictureCache.Capacity + 4}")).State);
    }
}

/// <summary>192w P3: the thinking state machine — live "Thinking", then "Thought for N s".</summary>
public sealed class ThinkingStateTests
{
    [Fact]
    public void NoReasoningMeansNoThinking()
    {
        var thinking = new ThinkingState();
        Assert.False(thinking.Started);
        Assert.False(thinking.Active);
        Assert.False(thinking.Collapse(1000)); // nothing to fold
        thinking.Done(2000); // a ReasoningDone with no text is ignored
        Assert.Equal("", thinking.Text);
        Assert.False(thinking.Delta("", 1000)); // an empty piece is not a start
        Assert.False(thinking.Started);
    }

    [Fact]
    public void TheFirstPieceStartsItTextCollectsAndTheFirstAnswerTextFoldsIt()
    {
        var thinking = new ThinkingState();
        Assert.True(thinking.Delta("Let me ", 10_000)); // the first piece (the log line)
        Assert.False(thinking.Delta("think.", 10_400));
        Assert.True(thinking.Active);
        var live = thinking.View(11_000);
        Assert.Equal(("Let me think.", true, "Thinking", 1), (live.Text, live.Active, live.Header, live.Seconds));
        Assert.True(thinking.Collapse(13_200)); // the first TextDelta
        Assert.False(thinking.Collapse(14_000)); // idempotent
        var folded = thinking.View(20_000);
        Assert.Equal((false, "Thought for 3 s"), (folded.Active, folded.Header)); // 10.0 s → 13.2 s
        Assert.True(thinking.Collapsed);
    }

    [Fact]
    public void TheEnginesOwnMeasureWinsAndIsSummedOverModelCalls()
    {
        var thinking = new ThinkingState();
        thinking.Delta("a", 1000);
        thinking.Done(3200);
        thinking.Done(1900); // a second model call of the turn
        thinking.Collapse(1500);
        Assert.Equal(5, thinking.Seconds(99_000)); // 5.1 s, not the 0.5 s the clock saw
        Assert.Equal("Thought for 5 s", thinking.View(0).Header);
    }

    [Fact]
    public void ItNeverSaysLessThanOneSecond()
    {
        var thinking = new ThinkingState();
        thinking.Delta("quick", 5_000);
        thinking.Collapse(5_040);
        Assert.Equal(1, thinking.Seconds(0));
    }

    [Fact]
    public void ReasoningThatArrivesAfterTheFoldIsKeptAndDoesNotReopenIt()
    {
        var thinking = new ThinkingState();
        thinking.Delta("first round. ", 1000);
        thinking.Collapse(2000);
        Assert.False(thinking.Delta("second round.", 5000)); // a later model call of the same turn
        Assert.True(thinking.Collapsed);
        Assert.Equal("first round. second round.", thinking.Text);
    }

    [Fact]
    public void TheLiveSectionShowsTheEndOfALongTextAndTheExpanderHoldsAll()
    {
        var thinking = new ThinkingState();
        var words = string.Join(" ", Enumerable.Range(0, 1200).Select(i => $"word{i}"));
        thinking.Delta(words, 0);
        var view = thinking.View(1000);
        Assert.True(view.Tail.Length <= ThinkingView.TailChars + 1);
        Assert.StartsWith("…", view.Tail, StringComparison.Ordinal);
        Assert.EndsWith("word1199", view.Tail, StringComparison.Ordinal);
        Assert.Equal(words, view.Text);
        Assert.DoesNotContain("word0 ", view.Tail, StringComparison.Ordinal);
    }

    [Fact]
    public void ThePileOfTextIsCappedAndTheOldestGoesFirst()
    {
        var thinking = new ThinkingState();
        thinking.Delta(new string('a', ThinkingState.MaxChars), 0);
        thinking.Delta("tail", 1);
        Assert.Equal(ThinkingState.MaxChars, thinking.Text.Length);
        Assert.EndsWith("tail", thinking.Text, StringComparison.Ordinal);
    }

    [Fact]
    public void TheCardKeepsOneThinkingPerExchangeAndFoldsItAtTheFirstTextAFinishOrAStop()
    {
        var card = new AnswerCardState();
        card.Begin(CardSource.VoiceKey, 0);
        card.FinalText("why is the build red", 100);
        card.TurnStarted(200);
        Assert.Null(card.Thinking(300)); // no ReasoningDelta: today's spinner
        card.ReasoningDone(900); // a Done with no text is ignored
        Assert.Null(card.Thinking(300));
        Assert.True(card.ReasoningDelta("Looking at ", 1000));
        Assert.False(card.ReasoningDelta("the log.", 1500));
        var live = card.Thinking(2000)!;
        Assert.Equal(("Looking at the log.", true), (live.Text, live.Active));
        card.TextDelta("The build fails on", 4000);
        var folded = card.Thinking(9000)!;
        Assert.Equal((false, "Thought for 3 s"), (folded.Active, folded.Header)); // 1.0 s → 4.0 s
        Assert.Equal(CardPhase.Answer, card.Phase);
        // A new exchange starts without it; a finish folds one that never saw text; a stop does too.
        card.Begin(CardSource.VoiceKey, 10_000, followUp: true);
        Assert.Null(card.Thinking(10_100));
        card.TurnStarted(10_200);
        card.ReasoningDelta("hm", 10_300);
        card.TurnFinished("end_turn", 12_000);
        Assert.False(card.Thinking(12_000)!.Active);
        card.Begin(CardSource.VoiceKey, 20_000, followUp: true);
        card.TurnStarted(20_100);
        card.ReasoningDelta("again", 20_200);
        Assert.True(card.Stop(21_000));
        Assert.False(card.Thinking(21_000)!.Active);
    }

    [Fact]
    public void TheCardIgnoresReasoningOfAFailedOrStoppedExchange()
    {
        var card = new AnswerCardState();
        card.Begin(CardSource.VoiceKey, 0);
        card.TurnStarted(100);
        card.Failed("boom", 200);
        Assert.False(card.ReasoningDelta("late", 300));
        Assert.Null(card.Thinking(400));
    }
}

/// <summary>192w: the Store's Ask path with thinking and pictures, on virtual time with a recording engine.</summary>
public sealed class AskThinkingAndPicturesTests
{
    private readonly ManualScheduler clock = new();
    private readonly FakeEngine engine = new();
    private readonly List<string> logs = new();
    private readonly StoreModel store;
    private const string Q = "show me the release flow";

    public AskThinkingAndPicturesTests() => store = new StoreModel(engine, clock, () => clock.Now, logs.Add);

    private static byte[] J(string s) => Encoding.UTF8.GetBytes(s);

    private void Send(string text = Q)
    {
        store.SubmitAsk(text);
        clock.Advance(3);
    }

    private void Reasoning(string text) => store.HandleEvent(J($$"""{"type":"ReasoningDelta","session":"desk-1","turn":"t","text":{{JsonSerializer.Serialize(text)}}}"""));

    private void ReasoningDone(long ms) => store.HandleEvent(J($$"""{"type":"ReasoningDone","session":"desk-1","turn":"t","ms":{{ms}}}"""));

    private void Delta(string text) => store.HandleEvent(J($$"""{"type":"TextDelta","session":"desk-1","turn":"t","text":{{JsonSerializer.Serialize(text)}}}"""));

    private void Finished(string stop = "end_turn") => store.HandleEvent(J($$"""{"type":"TurnFinished","session":"desk-1","turn":"t","stop_reason":"{{stop}}","usage":{},"context":1}"""));

    private StoreModel.ReceiptRow.Reply Row => Assert.IsType<StoreModel.ReceiptRow.Reply>(store.Receipt);

    [Fact]
    public void NoReasoningLeavesTodaysWaitingRow()
    {
        Send();
        Assert.Null(Row.Thinking);
        Assert.Null(AskBarModel.ReplyView(store.Receipt)); // nothing to draw yet: the one-line "waiting" row stays
        Delta("Here you go.");
        Assert.Null(Row.Thinking);
        Assert.Equal(("Here you go.", true), (AskBarModel.ReplyView(store.Receipt)!.Value.Markdown, AskBarModel.ReplyView(store.Receipt)!.Value.Streaming));
    }

    [Fact]
    public void ReasoningBeforeAnyTextShowsALiveThinkingSectionInPlaceOfTheWaitingRow()
    {
        Send();
        Reasoning("The user wants a flow. ");
        Reasoning("A flowchart fits.");
        Assert.Equal(("The user wants a flow. A flowchart fits.", true, "Thinking"), (Row.Thinking!.Text, Row.Thinking.Active, Row.Thinking.Header));
        var view = AskBarModel.ReplyView(store.Receipt)!.Value;
        Assert.Equal(("", true), (view.Markdown, view.Streaming));
        Assert.NotNull(view.Thinking);
        Assert.Equal("theuth · thinking…", AskBarModel.ReplyHeader(Row));
        Assert.Single(logs, l => l.StartsWith("ask: first ReasoningDelta (23 chars)", StringComparison.Ordinal)); // once per ask
    }

    [Fact]
    public void TheFirstAnswerTextFoldsTheThinkingIntoThoughtForNSeconds()
    {
        Send();
        Reasoning("hm");
        clock.Advance(2);
        ReasoningDone(2100); // published before the text that ended the thinking
        Delta("Here is the flow:");
        Assert.Equal((false, "Thought for 2 s"), (Row.Thinking!.Active, Row.Thinking.Header));
        Assert.Equal("theuth · replying…", AskBarModel.ReplyHeader(Row));
        Delta(" it has three stages.");
        Assert.Equal("Thought for 2 s", Row.Thinking!.Header); // later text does not change it
        Assert.Equal("Here is the flow: it has three stages.", Row.Markdown);
    }

    [Fact]
    public void ThinkingWithoutAnyReasoningDoneUsesTheClock()
    {
        Send();
        Reasoning("hm");
        clock.Advance(4);
        Delta("Done.");
        Assert.Equal("Thought for 4 s", Row.Thinking!.Header);
    }

    [Fact]
    public void AFinishWithNoTextFoldsTheThinkingToo()
    {
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(false, Tx.User(Q, 41), Tx.Assistant("The flow is Build, Test, Ship.", 41)));
        Send();
        Reasoning("working it out");
        clock.Advance(3);
        Finished();
        Assert.Equal((false, "Thought for 3 s"), (Row.Thinking!.Active, Row.Thinking.Header));
        Assert.Equal("The flow is Build, Test, Ship.", Row.Markdown);
        Assert.False(Row.Streaming);
    }

    [Fact]
    public void AFailedTurnKeepsItsOneLineAndNoThinking()
    {
        Send();
        Reasoning("trying");
        store.HandleEvent(J("""{"type":"Error","session":"desk-1","turn":"t","message":"model: boom"}"""));
        Assert.Null(Row.Thinking);
        Assert.Null(AskBarModel.ReplyView(store.Receipt));
        Assert.StartsWith("theuth · theuth couldn't reply", AskBarModel.ReceiptLines(store.Receipt, 0)!.Value.Text, StringComparison.Ordinal);
    }

    [Fact]
    public void ReasoningKeepsTheWatchdogQuiet()
    {
        Send();
        clock.Advance(10);
        Reasoning("still thinking");
        clock.Advance(10); // 20 s since the send, 10 s since the last event
        Assert.Equal(0, engine.Count("load_session"));
        clock.Advance(6); // 16 s of silence
        Assert.Equal(1, engine.Count("load_session"));
    }

    [Fact]
    public void AReasoningThatQuotesAFenceNeverShowsItsBodyOnTheRow()
    {
        Send();
        const string reasoning = "I could draw:\n```json-render\n{\"root\":\"x\",\"elements\":{}}\n```\nokay.";
        for (int i = 0; i < reasoning.Length; i += 4)
        {
            Reasoning(reasoning.Substring(i, Math.Min(4, reasoning.Length - i)));
            Assert.DoesNotContain("\"root\"", Row.Thinking!.Text, StringComparison.Ordinal);
            Assert.DoesNotContain("```json-render", Row.Thinking.Text, StringComparison.Ordinal);
        }
        Assert.Equal("I could draw:\n(diagram)\nokay.", Row.Thinking!.Text);
    }

    [Fact]
    public void AnOpenPictureFenceInTheReplyIsADrawingLineNotItsSourceOnTheReceipt()
    {
        Send();
        Delta("Here:\n```mermaid\ngraph LR\n  A --> B");
        Assert.Equal("Here: — (diagram)", Row.Text);
        Assert.DoesNotContain("graph", Row.Text, StringComparison.Ordinal);
        Assert.Equal(new MdBlock.Picture("mermaid", "graph LR\n  A --> B", false), AnswerMarkdown.Parse(Row.Markdown, Row.Streaming)[1]);
    }

    [Fact]
    public async Task TheStoresCacheAsksTheEngineForAClosedFenceWithTheViewsArguments()
    {
        engine.Fence = (_, _, _, _, _) => Task.FromResult(new FenceSnapshot { Outcome = "done", Png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==", Width = 557, Height = 190 });
        var key = PictureKey.Of("mermaid", "graph LR\n  A --> B", 446, "light", 1.25);
        var seen = new List<PictureResult>();
        Assert.Equal(PictureState.Ready, store.Pictures.Request(key).State); // the fake answers at once; a real draw takes seconds (the cache tests)
        store.Pictures.Watch(key, seen.Add); // a finished key has nothing to wait for
        await Task.Yield();
        Assert.Equal(new[] { "fence_snapshot mermaid 446 light 1.25 18" }, engine.Calls.Where(c => c.StartsWith("fence_snapshot", StringComparison.Ordinal)).ToArray());
        Assert.Empty(seen);
        Assert.Equal((557, 190), (store.Pictures.Request(key).PixelWidth, store.Pictures.Request(key).PixelHeight));
    }

    [Fact]
    public async Task AnEngineWithoutTheMethodGivesTheFallbackAndAReconnectAsksAgain()
    {
        var key = PictureKey.Of("openui", "root = Stack([])", 400, "dark", 1);
        var first = store.Pictures.Request(key);
        await Task.Yield();
        Assert.Equal((PictureState.Fallback, "error"), (first.State, first.Outcome)); // FakeEngine has no fence_snapshot: "not supported"
        Assert.Equal(PictureState.Fallback, store.Pictures.Request(key).State);
        Assert.Equal(1, engine.Count("fence_snapshot")); // kept: asked once
        store.HandleState(RpcClient.State.Open); // the first dial
        store.HandleState(RpcClient.State.Open); // a reconnect: the pictures that did not come are asked for again
        Assert.Equal(PictureState.Fallback, store.Pictures.Request(key).State);
        Assert.Equal(2, engine.Count("fence_snapshot"));
    }

    [Fact]
    public void ASpokenReplyNeverReadsADiagramAndADiagramOnlyReplyIsNotSpoken()
    {
        var spoken = new List<string>();
        store.AskReplyFinished += (_, text) => spoken.Add(text);
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(false, Tx.User(Q, 41), Tx.Assistant("Here it is.\n\n```mermaid\ngraph LR\n  A --> B\n```\n\nDone.", 41)));
        Send();
        Finished();
        Assert.Equal(new[] { "Here it is.\n\nDone." }, spoken);
        spoken.Clear();
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(false, Tx.User("again", 42), Tx.Assistant("```mermaid\ngraph LR\n```", 42)));
        Send("again");
        Finished();
        Assert.Empty(spoken);
    }
}