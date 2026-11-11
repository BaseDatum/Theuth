using System.Text.Json;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.AskBar;

/// <summary>
/// 192w P6: <c>--ask-replay --ask-pictures ok|fail|mixed|old</c> — a recorded desk reply whose model THINKS first (three
/// <c>ReasoningDelta</c> chunks, one of them quoting a <c>json-render</c> fence, then <c>ReasoningDone</c>) and then answers with prose
/// and a <c>mermaid</c> and a <c>json-render</c> fence cut where a stream cuts them. It runs through the real Store, the real
/// <see cref="PictureCache"/>, the real outcome mapping and the real Ask-bar views; only the renderer is a stub (the scratch engine
/// has no <c>fence_snapshot</c> and no renderer client is connected on build-B): <c>ok</c> draws both, <c>fail</c> answers
/// <c>no_renderer</c>, <c>mixed</c> draws the json-render fence only, <c>old</c> answers <c>-32601</c> like an engine before 192.
/// </summary>
internal static partial class AskProbe
{
    private static string Reasoning(string text) =>
        $$"""{"type":"ReasoningDelta","session":"replay-desk","turn":"t-replay","text":{{JsonSerializer.Serialize(text)}}}""";

    private const string ReasoningDoneLine = """{"type":"ReasoningDone","session":"replay-desk","turn":"t-replay","ms":3200}""";

    private const string FinishedLine = """{"type":"TurnFinished","session":"replay-desk","turn":"t-replay","stop_reason":"end_turn","usage":{"input":905,"output":88},"context":993}""";

    private static readonly string[] ThoughtPieces =
    {
        "The user wants the release flow as a diagram. ",
        "A mermaid flowchart fits the three stages, and a status card for the result. ",
        "My first draft of the card was:\n```json-render\n{\"root\":\"status\",\"elements\":{\"status\":{\"type\":\"Card\"}}}\n```\nI will keep that and add the flowchart.",
    };

    // The fences are cut mid-fence on purpose: the mermaid fence is open after the first chunk ("drawing…"), closed in the second, which
    // also opens the json-render fence; the third closes it.
    private static readonly string[] PicturePieces =
    {
        "Here is the release flow:\n\n```mermaid\ngraph LR\n  Build --> Test",
        "\n  Test --> Ship\n```\n\nAnd the status card for the last run:\n\n```json-render\n{\"root\":\"status\"",
        ",\"elements\":{\"status\":{\"type\":\"Card\",\"props\":{\"title\":\"Release\"}}}}\n```\n\nBoth open in theuth if you want them bigger.",
    };

    /// <summary>The transcript holds the streamed text and one more line: the finish replaces the stream with it.</summary>
    public static readonly string PictureFinalText = string.Concat(PicturePieces) + "\n\nNothing else is waiting.";

    public static readonly SessionTail PictureTranscript = new()
    {
        Running = false,
        History = new ValueList<TranscriptEntry>
        {
            new() { Role = "user", Text = "show me the release flow", Turn = 14 },
            new() { Role = "reasoning", Text = string.Concat(ThoughtPieces), Turn = 14 },
            new() { Role = "assistant", Text = PictureFinalText, Turn = 14 },
        },
    };

    private static readonly string[] RecordedPictureReply =
    {
        Reasoning(ThoughtPieces[0]),
        Reasoning(ThoughtPieces[1]),
        Reasoning(ThoughtPieces[2]),
        ReasoningDoneLine,
        Delta(PicturePieces[0]),
        Delta(PicturePieces[1]),
        Delta(PicturePieces[2]),
        FinishedLine,
    };

    private static string StubWords(string mode) => mode switch
    {
        "ok" => "draws both fences",
        "mixed" => "draws json-render, answers no_renderer for mermaid",
        "old" => "answers -32601 (an engine before 192)",
        _ => "answers no_renderer",
    };

    /// <summary>A renderer that takes a moment (2–6 s on the real engine; here ≈ 1 s, long enough to photograph "drawing…") and then answers per <paramref name="mode"/>.</summary>
    private static Func<PictureKey, Task<FenceSnapshot>> Stub(string mode) => async key =>
    {
        await Task.Delay(key.Lang == PictureFence.Mermaid ? 1400 : 1000);
        if (mode == "old") throw new RpcException(-32601, "Method not found");
        bool draw = mode == "ok" || (mode == "mixed" && key.Lang == PictureFence.JsonRender);
        if (!draw) return new FenceSnapshot { Outcome = "no_renderer" };
        var (png, w, h) = await StubPng.Draw(key);
        return new FenceSnapshot { Outcome = "done", Png = Convert.ToBase64String(png), Width = w, Height = h };
    };

    private static async Task ReplayPictures(Store store, AskBarInstall install, string mode)
    {
        AskLog.Write($"ask-pictures start ({mode}): {RecordedPictureReply.Length} recorded lines for session replay-desk, 450 ms apart — three ReasoningDelta chunks (one quotes a json-render fence) and ReasoningDone, then prose with a mermaid and a json-render fence cut mid-fence; stub fence_snapshot {StubWords(mode)}");
        install.Bar.ReplyRendered += line => AskLog.Write($"ask-pictures view: {line}");
        store.ReplayTranscript("replay-desk", PictureTranscript);
        store.ReplayPictures(Stub(mode));
        var queue = new Queue<string>(RecordedPictureReply);
        store.ReplayDeskReply("replay-desk", "show me the release flow", new[] { queue.Dequeue() });
        AskLog.Write($"ask-pictures receipt: {Describe(store.Receipt)}");
        int step = 1;
        while (queue.Count > 0)
        {
            await Task.Delay(450);
            store.HandleEvent(System.Text.Encoding.UTF8.GetBytes(queue.Dequeue()));
            step++;
            AskLog.Write($"ask-pictures receipt: {Describe(store.Receipt)} finished={Word(store.Ask.Router.Reply?.Finished ?? false)} cache: {store.Pictures.Fetches} draws, {store.Pictures.InFlight} out");
            // Evidence: the bar's tree as a PNG while the model thinks (step 3), with the thinking folded and the mermaid fence still arriving (5),
            // once the stub has answered for both closed fences (7, after the draws), and after the finish replaced the stream (8).
            string? shot = step switch { 3 => "1-thinking", 5 => "2-drawing", 7 => "3-drawn", 8 => "4-finished", _ => null };
            if (shot is null) continue;
            await Task.Delay(step == 7 ? 2600 : 350);
            var path = AskFlags.Png192("ask-pictures-" + shot);
            var ok = await install.Bar.CapturePng(path);
            AskLog.Write($"ask-pictures shot: {path} ok={Word(ok)}");
        }
        var reply = store.Ask.Router.Reply;
        var thinking = reply?.Thinking;
        AskLog.Write($"ask-pictures done ({mode}): finished={Word(reply?.Finished ?? false)} streamed={reply?.StreamedChars} chars, final text {reply?.Text.Length} chars, replaced-by-transcript={Word(reply?.Text == PictureFinalText)}, thinking {(thinking is { Started: true } ? $"\"{thinking.View(0).Header}\" ({thinking.Text.Length} chars, fence body shown: {Word(thinking.Text.Contains("\"root\"", StringComparison.Ordinal))})" : "none")}, pictures: {store.Pictures.Fetches} draws, unsupported={Word(store.Pictures.Unsupported)}");
    }
}

/// <summary>192w P6: a small raster "diagram" the stub renderer returns — boxes, arrows and block letters drawn into a BGRA buffer and PNG-encoded; the theme's colours, at the asked width × scale.</summary>
internal static class StubPng
{
    private static readonly Dictionary<char, string[]> Font = new()
    {
        ['A'] = new[] { " ### ", "#   #", "#   #", "#####", "#   #", "#   #", "#   #" },
        ['B'] = new[] { "#### ", "#   #", "#   #", "#### ", "#   #", "#   #", "#### " },
        ['D'] = new[] { "#### ", "#   #", "#   #", "#   #", "#   #", "#   #", "#### " },
        ['E'] = new[] { "#####", "#    ", "#    ", "#### ", "#    ", "#    ", "#####" },
        ['H'] = new[] { "#   #", "#   #", "#   #", "#####", "#   #", "#   #", "#   #" },
        ['I'] = new[] { "#####", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", "#####" },
        ['L'] = new[] { "#    ", "#    ", "#    ", "#    ", "#    ", "#    ", "#####" },
        ['P'] = new[] { "#### ", "#   #", "#   #", "#### ", "#    ", "#    ", "#    " },
        ['S'] = new[] { " ####", "#    ", "#    ", " ### ", "    #", "    #", "#### " },
        ['T'] = new[] { "#####", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  " },
        ['U'] = new[] { "#   #", "#   #", "#   #", "#   #", "#   #", "#   #", " ### " },
    };

    private sealed class Canvas(int w, int h, bool dark)
    {
        public readonly byte[] Px = new byte[w * h * 4];
        public int W => w;
        public int H => h;
        public (byte R, byte G, byte B) Bg = dark ? ((byte)31, (byte)31, (byte)31) : ((byte)250, (byte)250, (byte)250);
        public (byte R, byte G, byte B) Ink = dark ? ((byte)236, (byte)236, (byte)236) : ((byte)24, (byte)24, (byte)24);
        public (byte R, byte G, byte B) Muted = dark ? ((byte)150, (byte)150, (byte)150) : ((byte)100, (byte)100, (byte)100);
        public (byte R, byte G, byte B) Box = dark ? ((byte)38, (byte)92, (byte)168) : ((byte)196, (byte)218, (byte)246);
        public (byte R, byte G, byte B) Good = dark ? ((byte)64, (byte)170, (byte)110) : ((byte)40, (byte)140, (byte)80);

        public void Fill((byte R, byte G, byte B) c)
        {
            for (int i = 0; i < Px.Length; i += 4) { Px[i] = c.B; Px[i + 1] = c.G; Px[i + 2] = c.R; Px[i + 3] = 255; }
        }

        public void Rect(int x, int y, int rw, int rh, (byte R, byte G, byte B) c)
        {
            for (int j = Math.Max(0, y); j < Math.Min(h, y + rh); j++)
            {
                for (int i = Math.Max(0, x); i < Math.Min(w, x + rw); i++)
                {
                    int p = (j * w + i) * 4;
                    Px[p] = c.B; Px[p + 1] = c.G; Px[p + 2] = c.R; Px[p + 3] = 255;
                }
            }
        }

        public void Frame(int x, int y, int rw, int rh, int t, (byte R, byte G, byte B) c)
        {
            Rect(x, y, rw, t, c); Rect(x, y + rh - t, rw, t, c); Rect(x, y, t, rh, c); Rect(x + rw - t, y, t, rh, c);
        }

        public void Text(int x, int y, string s, int unit, (byte R, byte G, byte B) c)
        {
            foreach (var ch in s)
            {
                if (Font.TryGetValue(ch, out var glyph))
                {
                    for (int row = 0; row < 7; row++)
                        for (int col = 0; col < 5; col++)
                            if (glyph[row][col] == '#') Rect(x + col * unit, y + row * unit, unit, unit, c);
                }
                x += 6 * unit;
            }
        }

        public int TextWidth(string s, int unit) => s.Length * 6 * unit - unit;

        public void Arrow(int x0, int x1, int y, int t, (byte R, byte G, byte B) c)
        {
            Rect(x0, y - t / 2, x1 - x0 - 3 * t, t, c);
            for (int k = 0; k < 3 * t; k++) Rect(x1 - 3 * t + k, y - (3 * t - k) / 2, 1, (3 * t - k), c); // the head, a triangle
        }
    }

    public static async Task<(byte[] Png, int Width, int Height)> Draw(PictureKey key)
    {
        int w = (int)Math.Round(key.Width * key.Scale);
        bool flow = key.Lang == PictureFence.Mermaid;
        int h = (int)Math.Round(w * (flow ? 0.34 : 0.5));
        bool dark = key.Appearance == "dark";
        var c = new Canvas(w, h, dark);
        c.Fill(c.Bg);
        int u = Math.Max(1, (int)Math.Round(key.Scale * 2.2)); // one "pixel" of the block letters
        int t = Math.Max(2, (int)Math.Round(key.Scale * 2));
        if (flow)
        {
            string[] names = { "BUILD", "TEST", "SHIP" };
            int bw = (int)(w * 0.22), bh = (int)(h * 0.44), by = (h - bh) / 2;
            int[] xs = { (int)(w * 0.07), (int)(w * 0.39), (int)(w * 0.71) };
            for (int i = 0; i < 3; i++)
            {
                c.Rect(xs[i], by, bw, bh, c.Box);
                c.Frame(xs[i], by, bw, bh, t, c.Muted);
                c.Text(xs[i] + (bw - c.TextWidth(names[i], u)) / 2, by + (bh - 7 * u) / 2, names[i], u, c.Ink);
                if (i < 2) c.Arrow(xs[i] + bw + t, xs[i + 1] - t, h / 2, t, c.Muted);
            }
        }
        else
        {
            int m = (int)(w * 0.05);
            c.Frame(m, m, w - 2 * m, h - 2 * m, t, c.Muted);
            c.Rect(m, m, w - 2 * m, (int)(h * 0.2), c.Box);
            c.Text(m + 3 * t, m + ((int)(h * 0.2) - 7 * u) / 2, "STATUS", u, c.Ink);
            for (int row = 0; row < 3; row++)
            {
                int y = m + (int)(h * 0.28) + row * (int)(h * 0.18);
                c.Rect(m + 3 * t, y, 4 * u, 4 * u, c.Good);
                c.Rect(m + 3 * t + 7 * u, y + u, (int)((w - 2 * m) * (0.62 - row * 0.12)), 2 * u, c.Muted);
            }
        }
        using var stream = new global::Windows.Storage.Streams.InMemoryRandomAccessStream();
        var encoder = await global::Windows.Graphics.Imaging.BitmapEncoder.CreateAsync(global::Windows.Graphics.Imaging.BitmapEncoder.PngEncoderId, stream);
        encoder.SetPixelData(global::Windows.Graphics.Imaging.BitmapPixelFormat.Bgra8, global::Windows.Graphics.Imaging.BitmapAlphaMode.Premultiplied, (uint)w, (uint)h, 96, 96, c.Px);
        await encoder.FlushAsync();
        stream.Seek(0);
        var png = new byte[stream.Size];
        using (var reader = new global::Windows.Storage.Streams.DataReader(stream))
        {
            await reader.LoadAsync((uint)stream.Size);
            reader.ReadBytes(png);
        }
        return (png, w, h);
    }
}
