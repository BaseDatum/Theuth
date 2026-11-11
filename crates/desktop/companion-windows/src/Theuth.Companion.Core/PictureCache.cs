using System.Globalization;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>
/// 192w P2: everything that makes one drawing — the engine's <c>fence_snapshot</c> arguments. The width is the content width in
/// effective pixels (clamped 240..1200 like the engine), the appearance <c>dark</c> | <c>light</c>, the scale the XamlRoot's
/// rasterization scale (clamped 1..3): the same fence at another width, theme or scale is another picture.
/// </summary>
public readonly record struct PictureKey(string Lang, string Source, int Width, string Appearance, double Scale)
{
    public const int MinWidth = 240;
    public const int MaxWidth = 1200;
    public const double MinScale = 1;
    public const double MaxScale = 3;

    public static PictureKey Of(string lang, string source, double widthEpx, string appearance, double scale)
    {
        int width = double.IsFinite(widthEpx) ? (int)Math.Clamp(Math.Floor(widthEpx), MinWidth, MaxWidth) : MinWidth;
        double s = double.IsFinite(scale) ? Math.Round(Math.Clamp(scale, MinScale, MaxScale), 2) : 1;
        return new PictureKey(lang, source, width, string.Equals(appearance, "light", StringComparison.OrdinalIgnoreCase) ? "light" : "dark", s);
    }
}

public enum PictureState
{
    /// <summary>Requested or queued: the view shows the one-line "drawing…".</summary>
    Drawing,

    /// <summary>A PNG came back: the view shows it at content width.</summary>
    Ready,

    /// <summary>No picture will come (no renderer, failed, timed out, cancelled, an engine without the method): the view shows the fence as code with <see cref="PictureFence.FallbackCaption"/>.</summary>
    Fallback,
}

/// <summary>What the cache knows of one <see cref="PictureKey"/>.</summary>
/// <param name="Png">The decoded PNG (Ready only).</param>
/// <param name="PixelWidth">The PNG's pixels; shown at <c>PixelWidth / Scale</c> effective pixels.</param>
/// <param name="Reason">Why there is no picture (Fallback) — for the log, never shown.</param>
/// <param name="Outcome">The engine's outcome word, or <c>unsupported</c> (-32601) · <c>error</c> · <c>reconnect</c>.</param>
public sealed record PictureResult(PictureState State, byte[]? Png = null, int PixelWidth = 0, int PixelHeight = 0, double Scale = 1, string Reason = "", string Outcome = "", long Ms = 0)
{
    public static PictureResult Drawing { get; } = new(PictureState.Drawing);

    public static PictureResult Fallback(string reason, string outcome, long ms = 0) => new(PictureState.Fallback, Reason: reason, Outcome: outcome, Ms: ms);
}

/// <summary>192w P2: a <c>fence_snapshot</c> answer (or its failure) as the view needs it.</summary>
public static class PictureOutcome
{
    /// <summary>
    /// <c>done</c> with a decodable PNG → Ready; every other outcome — <c>no_renderer</c>, <c>failed</c>, <c>timed_out</c>,
    /// <c>cancelled</c>, one the engine adds later, an empty or broken picture — and every failure of the call (<c>-32601</c> for an
    /// engine without the method, a closed socket, a deadline) → Fallback.
    /// </summary>
    public static PictureResult Map(FenceSnapshot? snapshot, Exception? error, double scale, long ms)
    {
        if (error is not null)
        {
            return error switch
            {
                RpcException { Code: -32601 } => PictureResult.Fallback("this engine has no fence_snapshot (-32601)", "unsupported", ms),
                TimeoutException => PictureResult.Fallback("no answer in time", "timed_out", ms),
                RpcException r => PictureResult.Fallback(r.ToString(), "error", ms),
                _ => PictureResult.Fallback(error.Message, "error", ms),
            };
        }
        if (snapshot is null) return PictureResult.Fallback("no answer", "failed", ms);
        switch (snapshot.Outcome)
        {
            case "done":
            {
                if (string.IsNullOrEmpty(snapshot.Png)) return PictureResult.Fallback("done without a picture", "failed", ms);
                byte[] png;
                try { png = Convert.FromBase64String(snapshot.Png); }
                catch (FormatException) { return PictureResult.Fallback("the picture is not base64", "failed", ms); }
                if (!IsPng(png)) return PictureResult.Fallback("the picture is not a PNG", "failed", ms);
                var (w, h) = PngSize(png);
                int width = snapshot.Width is > 0 ? (int)Math.Round(snapshot.Width.Value) : w;
                int height = snapshot.Height is > 0 ? (int)Math.Round(snapshot.Height.Value) : h;
                return new PictureResult(PictureState.Ready, png, width, height, scale, Outcome: "done", Ms: ms);
            }
            case "no_renderer": return PictureResult.Fallback("no renderer is connected", "no_renderer", ms);
            case "failed": return PictureResult.Fallback(string.IsNullOrWhiteSpace(snapshot.Error) ? "the renderer failed" : snapshot.Error.Trim(), "failed", ms);
            case "timed_out": return PictureResult.Fallback("the renderer timed out", "timed_out", ms);
            case "cancelled": return PictureResult.Fallback("the draw was cancelled", "cancelled", ms);
            default: return PictureResult.Fallback($"unknown outcome \"{snapshot.Outcome}\"", snapshot.Outcome.Length == 0 ? "failed" : snapshot.Outcome, ms);
        }
    }

    public static bool IsPng(byte[] b) =>
        b.Length >= 24 && b[0] == 0x89 && b[1] == 0x50 && b[2] == 0x4E && b[3] == 0x47 && b[4] == 0x0D && b[5] == 0x0A && b[6] == 0x1A && b[7] == 0x0A;

    /// <summary>The pixels in the PNG's IHDR (0×0 when it is not a PNG).</summary>
    public static (int Width, int Height) PngSize(byte[] b) => IsPng(b) ? (Int32At(b, 16), Int32At(b, 20)) : (0, 0);

    private static int Int32At(byte[] b, int i) => (b[i] << 24) | (b[i + 1] << 16) | (b[i + 2] << 8) | b[i + 3];

    /// <summary>The log's words for a result: <c>ready 48231 bytes 520x310</c> · <c>fallback (no_renderer): no renderer is connected</c>.</summary>
    public static string Describe(PictureResult r) => r.State switch
    {
        PictureState.Ready => $"ready, {r.Png?.Length ?? 0} bytes, {r.PixelWidth}x{r.PixelHeight}",
        PictureState.Fallback => $"fallback ({r.Outcome}): {r.Reason}",
        _ => "drawing",
    };
}

/// <summary>
/// 192w P2: the pictures of the replies on screen. <see cref="Request"/> answers at once with what is known — Ready, Fallback or
/// Drawing — and starts the draw when the key is new: at most <see cref="MaxInFlight"/> <c>fence_snapshot</c> calls at a time (the rest
/// wait their turn), each with a <see cref="Deadline"/> of its own (an engine that never answers must not leave "drawing…" up for
/// good). Every result is kept, failures too, so a fence is asked for once however often a view is rebuilt; an engine that answers
/// <c>-32601</c> is not asked again until <see cref="Reset"/> (the connection was re-opened). Owner-thread only, like the Store; the
/// continuation of a finished call runs there and tells the <see cref="Watch"/>ers.
/// </summary>
public sealed class PictureCache
{
    public const int MaxInFlight = 2;

    /// <summary>Results kept; the oldest finished one goes first. A PNG is ~50–200 kB.</summary>
    public const int Capacity = 48;

    /// <summary>The engine draws in 2–6 s and gives up after about 20 s: this is the client's own cap on top.</summary>
    public static readonly TimeSpan Deadline = TimeSpan.FromSeconds(30);

    private static readonly TimeSpan HitLogEvery = TimeSpan.FromSeconds(5);

    private sealed class Entry(PictureKey key, long seq)
    {
        public readonly PictureKey Key = key;
        public readonly long Seq = seq;
        public PictureResult Result = PictureResult.Drawing;
        public readonly List<Action<PictureResult>> Watchers = new();
        public DateTimeOffset? HitLogged;
    }

    private sealed class Watcher(Entry entry, Action<PictureResult> handler) : IDisposable
    {
        public void Dispose() => entry.Watchers.Remove(handler);
    }

    private sealed class NoWatch : IDisposable
    {
        public static readonly NoWatch Instance = new();

        public void Dispose() { }
    }

    private readonly Dictionary<PictureKey, Entry> entries = new();
    private readonly Queue<Entry> queue = new();
    private readonly Func<PictureKey, Task<FenceSnapshot>> fetch;
    private readonly IStoreScheduler scheduler;
    private readonly Func<DateTimeOffset> clock;
    private readonly Action<string> log;
    private int inFlight;
    private long seq;

    public PictureCache(Func<PictureKey, Task<FenceSnapshot>> fetch, IStoreScheduler scheduler, Func<DateTimeOffset> clock, Action<string> log)
    {
        this.fetch = fetch;
        this.scheduler = scheduler;
        this.clock = clock;
        this.log = log;
    }

    /// <summary>The engine answered <c>-32601</c>: no call goes out until <see cref="Reset"/>.</summary>
    public bool Unsupported { get; private set; }

    public int InFlight => inFlight;

    public int Queued => queue.Count;

    /// <summary>How many <c>fence_snapshot</c> calls were made.</summary>
    public int Fetches { get; private set; }

    public int Count => entries.Count;

    /// <summary>What is known of <paramref name="key"/> now; a new key starts (or queues) its draw.</summary>
    public PictureResult Request(PictureKey key)
    {
        if (entries.TryGetValue(key, out var known))
        {
            if (known.Result.State != PictureState.Drawing && (known.HitLogged is not { } at || clock() - at >= HitLogEvery))
            {
                known.HitLogged = clock();
                log($"picture: cache hit {key.Lang} ({PictureOutcome.Describe(known.Result)})");
            }
            return known.Result;
        }
        var entry = new Entry(key, ++seq);
        entries[key] = entry;
        if (Unsupported)
        {
            entry.Result = PictureResult.Fallback("this engine has no fence_snapshot (-32601)", "unsupported");
            log($"picture: fence_snapshot not asked for {key.Lang}: this engine has no fence_snapshot (-32601)");
            Evict();
            return entry.Result;
        }
        queue.Enqueue(entry);
        Pump();
        return entry.Result;
    }

    /// <summary>Call <paramref name="changed"/> when a Drawing <paramref name="key"/> gets its result (once). Dispose to stop listening.</summary>
    public IDisposable Watch(PictureKey key, Action<PictureResult> changed)
    {
        if (!entries.TryGetValue(key, out var entry) || entry.Result.State != PictureState.Drawing) return NoWatch.Instance;
        entry.Watchers.Add(changed);
        return new Watcher(entry, changed);
    }

    /// <summary>
    /// The connection was re-opened (maybe to a newer engine, maybe with a renderer now): pictures that did not come are asked for
    /// again when next shown, a draw that was out ends as a fallback for whoever waits, PNGs already drawn stay.
    /// </summary>
    public void Reset()
    {
        Unsupported = false;
        queue.Clear();
        foreach (var entry in entries.Values.Where(e => e.Result.State != PictureState.Ready).ToList())
        {
            entries.Remove(entry.Key);
            if (entry.Result.State != PictureState.Drawing) continue;
            entry.Result = PictureResult.Fallback("the connection was re-opened", "reconnect");
            Notify(entry);
        }
    }

    private void Pump()
    {
        while (inFlight < MaxInFlight && queue.Count > 0)
        {
            var entry = queue.Dequeue();
            if (!entries.TryGetValue(entry.Key, out var current) || !ReferenceEquals(current, entry)) continue;
            inFlight++;
            _ = Run(entry);
        }
    }

    private async Task Run(Entry entry)
    {
        var key = entry.Key;
        var started = clock();
        Fetches++;
        log($"picture: fence_snapshot request {key.Lang} ({key.Source.Length} chars) width={key.Width} {key.Appearance} x{key.Scale.ToString("0.##", CultureInfo.InvariantCulture)}"
            + (inFlight >= MaxInFlight || queue.Count > 0 ? $" ({inFlight} out, {queue.Count} waiting)" : ""));
        FenceSnapshot? snapshot = null;
        Exception? error = null;
        var late = new TaskCompletionSource<FenceSnapshot>();
        using var timer = scheduler.After(Deadline, () => late.TrySetException(new TimeoutException($"fence_snapshot did not answer within {Deadline.TotalSeconds:0} s")));
        try
        {
            var call = fetch(key);
            _ = call.ContinueWith(static t => { _ = t.Exception; }, TaskContinuationOptions.OnlyOnFaulted); // observed even when the deadline won
            var first = await Task.WhenAny(call, late.Task);
            snapshot = await first;
        }
        catch (Exception e)
        {
            error = e;
        }
        long ms = (long)Math.Round((clock() - started).TotalMilliseconds);
        Finish(entry, PictureOutcome.Map(snapshot, error, key.Scale, ms), error);
    }

    private void Finish(Entry entry, PictureResult result, Exception? error)
    {
        inFlight--;
        if (error is RpcException { Code: -32601 } && !Unsupported)
        {
            Unsupported = true;
            log("picture: this engine has no fence_snapshot (-32601) — fences stay code until the connection is re-opened");
        }
        log(result.State == PictureState.Ready
            ? $"picture: fence_snapshot done {entry.Key.Lang} in {result.Ms} ms, {result.Png?.Length ?? 0} bytes ({result.PixelWidth}x{result.PixelHeight})"
            : $"picture: fence_snapshot {result.Outcome} {entry.Key.Lang} in {result.Ms} ms: {result.Reason}");
        if (entries.TryGetValue(entry.Key, out var current) && ReferenceEquals(current, entry))
        {
            entry.Result = result;
            Notify(entry);
            Evict();
        }
        Pump();
    }

    private static void Notify(Entry entry)
    {
        var watchers = entry.Watchers.ToArray();
        entry.Watchers.Clear();
        foreach (var watcher in watchers)
        {
            try { watcher(entry.Result); }
            catch (Exception) { /* a view that went away must not stop the others */ }
        }
    }

    private void Evict()
    {
        while (entries.Count > Capacity)
        {
            var oldest = entries.Values.Where(e => e.Result.State != PictureState.Drawing).MinBy(e => e.Seq);
            if (oldest is null) return;
            entries.Remove(oldest.Key);
        }
    }
}