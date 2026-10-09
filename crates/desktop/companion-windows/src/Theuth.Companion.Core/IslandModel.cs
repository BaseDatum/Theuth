using System.Globalization;
using System.Text.Json;

namespace Theuth.Companion.Core;

/// <summary>A rectangle in physical pixels of the virtual screen (Win32 <c>RECT</c> semantics, right/bottom exclusive).</summary>
public readonly record struct PxRect(int X, int Y, int Width, int Height)
{
    public int Right => X + Width;

    public int Bottom => Y + Height;

    public double CenterX => X + Width / 2.0;

    public double CenterY => Y + Height / 2.0;

    public bool Intersects(PxRect o) => X < o.Right && o.X < Right && Y < o.Bottom && o.Y < Bottom;

    public bool Contains(int px, int py) => px >= X && px < Right && py >= Y && py < Bottom;
}

/// <summary>One monitor as the island sees it: its device name (the memory key), its work area (a top taskbar respected) and its scale.</summary>
public sealed record IslandDisplay(string Device, PxRect WorkArea, double Scale, bool Primary);

/// <summary>The island's shape (149 §3.6 + island fix D1): the closed pill or the open strip + pane. Where it sits is <see cref="IslandModel.Detached"/>.</summary>
public enum IslandMode
{
    Closed,
    Open,
}

/// <summary>
/// Design 149 §3.6 / §7.4 #1 / D9–D11, the pure half of the island window: where it sits (top-centre of a monitor's work
/// area; a saved offset per display, keyed by monitor device name, validated before use; snap-back within
/// <see cref="SnapPx"/> of the default; double-click = snap back), whether it is shown (island fix D2: a live meeting — Starting ·
/// Recording · Paused · Stopping — or "Always show"; no linger), and its shape (D1: open while live unless explicitly collapsed
/// for that meeting; no hover model). Mirrors the Swift <c>IslandModel</c>; the
/// WinUI window executes what this decides. Offsets are stored relative to the work area's origin in physical pixels, so
/// a monitor that moves in the virtual-screen layout keeps its island where the user put it.
/// </summary>
public sealed class IslandModel
{
    /// <summary>A released drag whose centre is within this many physical pixels × scale of the top-centre default snaps back (§3.6 "40 pt").</summary>
    public const int SnapPx = 40;

    /// <summary>The pill hugs the top edge of the work area by this gap (effective px); 0 would touch a top taskbar.</summary>
    public const int TopGapPx = 6;

    private readonly Dictionary<string, (int Dx, int Dy)> frames = new(StringComparer.OrdinalIgnoreCase);

    /// <summary>A saved frame no longer lands on its monitor's work area (it shrank or moved): rejected and forgotten; the window logs it.</summary>
    public event Action<string, (int Dx, int Dy)>? FrameRejected;

    public IslandMode Mode { get; private set; } = IslandMode.Closed;

    /// <summary>Settings › Island "Always show": the closed pill stays when nothing is live.</summary>
    public bool AlwaysShow { get; set; }

    /// <summary>Settings › Island eye default: the companion's own windows are excluded from capture (<c>WDA_EXCLUDEFROMCAPTURE</c>).</summary>
    public bool HiddenFromCapture { get; set; } = true;

    /// <summary>Dragged away from the top-centre default on the current display (a saved offset is in use). Orthogonal to <see cref="Mode"/>.</summary>
    public bool Detached { get; private set; }

    /// <summary>The saved offsets, device → (dx, dy) from the work area's origin; read-only view for tests and Settings' "reset".</summary>
    public IReadOnlyDictionary<string, (int Dx, int Dy)> Frames => frames;

    // MARK: placement

    /// <summary>The top-centre default for a pill of <paramref name="size"/> on <paramref name="d"/>.</summary>
    public static PxRect DefaultFrame(IslandDisplay d, (int Width, int Height) size)
    {
        var wa = d.WorkArea;
        int gap = (int)Math.Round(TopGapPx * d.Scale);
        int x = wa.X + (wa.Width - size.Width) / 2;
        return new PxRect(x, wa.Y + gap, size.Width, size.Height);
    }

    /// <summary>
    /// Where to place a window of <paramref name="size"/> on <paramref name="d"/>: the saved offset when it still lands on the
    /// work area (validated: the frame must intersect it), else the default. The second value says whether the saved frame
    /// was used (<see cref="Detached"/> then).
    /// </summary>
    public (PxRect Frame, bool Saved) Place(IslandDisplay d, (int Width, int Height) size)
    {
        if (frames.TryGetValue(d.Device, out var off))
        {
            var r = new PxRect(d.WorkArea.X + off.Dx, d.WorkArea.Y + off.Dy, size.Width, size.Height);
            if (r.Intersects(d.WorkArea)) { Detached = true; return (r, true); }
            frames.Remove(d.Device); // off-screen memory (a monitor shrank): forget it
            FrameRejected?.Invoke(d.Device, off);
        }
        Detached = false;
        return (DefaultFrame(d, size), false);
    }

    /// <summary>
    /// A manual drag ended with the window at <paramref name="frame"/> on <paramref name="d"/>: within <see cref="SnapPx"/> of the
    /// default's centre the frame snaps back and the memory for that display is cleared (returns the default); otherwise the
    /// offset is remembered for the display and the frame is kept. The caller animates/moves the window to the result.
    /// </summary>
    public PxRect DragEnded(IslandDisplay d, PxRect frame)
    {
        var def = DefaultFrame(d, (frame.Width, frame.Height));
        if (Distance(def, frame) <= SnapPx * d.Scale)
        {
            frames.Remove(d.Device);
            Detached = false;
            return def;
        }
        frames[d.Device] = (frame.X - d.WorkArea.X, frame.Y - d.WorkArea.Y);
        Detached = true;
        return frame;
    }

    /// <summary>Double-click on the grip, or Settings › Island "reset position" for one display.</summary>
    public PxRect SnapBack(IslandDisplay d, (int Width, int Height) size)
    {
        frames.Remove(d.Device);
        Detached = false;
        return DefaultFrame(d, size);
    }

    /// <summary>Settings › Island "reset all".</summary>
    public void ResetAll()
    {
        frames.Clear();
        Detached = false;
    }

    /// <summary>
    /// A drag crossed into a monitor of another scale (<c>WM_DPICHANGED</c> mid-drag; the window is resized by the system): the
    /// grab point's offset inside the window, in physical px, scales with it, so the window stays under the cursor.
    /// </summary>
    public static (int Dx, int Dy) ScaleGrab((int Dx, int Dy) grab, double oldScale, double newScale)
    {
        if (oldScale <= 0 || newScale <= 0 || oldScale == newScale) return grab;
        double k = newScale / oldScale;
        return ((int)Math.Round(grab.Dx * k), (int)Math.Round(grab.Dy * k));
    }

    /// <summary>Centre-to-centre distance of two frames.</summary>
    public static double Distance(PxRect a, PxRect b) => Math.Sqrt(Math.Pow(a.CenterX - b.CenterX, 2) + Math.Pow(a.CenterY - b.CenterY, 2));

    /// <summary>The display whose work area contains the frame's centre; the primary (else the first) when none does.</summary>
    public static IslandDisplay? DisplayFor(IReadOnlyList<IslandDisplay> displays, PxRect frame)
    {
        if (displays.Count == 0) return null;
        int cx = (int)frame.CenterX, cy = (int)frame.CenterY;
        foreach (var d in displays) if (d.WorkArea.Contains(cx, cy)) return d;
        foreach (var d in displays) if (d.Primary) return d;
        return displays[0];
    }

    // MARK: lifecycle and visibility (149 island fix, Sky 2026-10-09 15:50 — D1 · D2)

    /// <summary><c>meeting_status.state</c> words that mean a meeting is live (<c>Island.tsx:73</c>): Starting · Recording · Paused · Stopping.</summary>
    public static bool IsLive(string state) => state is "Starting" or "Recording" or "Paused" or "Stopping";

    /// <summary>D2: the close animation when a meeting ends (Done · Failed · anything not live); ≤ 300 ms, then the window is gone.</summary>
    public static readonly TimeSpan CloseAnimation = TimeSpan.FromMilliseconds(180);

    /// <summary>
    /// The engine connection has been closed this long: a live meeting is presented as ended (the island goes) — the engine that
    /// recorded it is gone or unreachable, and nothing else would ever end it (build-B: the engine died at <c>meeting_stop</c> and the
    /// island stayed on "Stopping"). A reconnect hydrates the real state back.
    /// </summary>
    public static readonly TimeSpan EngineLostGrace = TimeSpan.FromSeconds(3);

    /// <summary>The state the island presents: <paramref name="state"/>, or <c>Failed</c> once the engine has been gone for <see cref="EngineLostGrace"/>.</summary>
    public static string EffectiveState(string state, TimeSpan? disconnectedFor) =>
        disconnectedFor is { } d && d >= EngineLostGrace && IsLive(state) ? "Failed" : state;

    private readonly HashSet<string> collapsed = new(StringComparer.Ordinal);

    /// <summary>The meeting the island last presented (null when none is live).</summary>
    public string? Meeting { get; private set; }

    /// <summary>
    /// D2 — the original's rule (<c>Island.tsx:73</c>): shown iff the meeting is Starting · Recording · Paused · Stopping, or
    /// Settings › Island "Always show" keeps the idle wordmark. No linger after the last topic or brief: Done · Failed · Idle ·
    /// Post hide the island at once (the window plays <see cref="CloseAnimation"/> first).
    /// </summary>
    public bool ShouldShow(string state) => IsLive(state) || AlwaysShow;

    /// <summary>
    /// D1: the meeting's state arrived. While live the island is OPEN (strip + brief pane) unless the user collapsed THIS meeting
    /// with an explicit click (remembered per meeting id); a new meeting opens again. Not live: the closed pill — the idle
    /// wordmark when "Always show" is on, else nothing (<see cref="ShouldShow"/> is false). Returns true when the mode changed.
    /// </summary>
    public bool Present(string? meeting, string state)
    {
        var before = Mode;
        bool live = meeting is not null && IsLive(state);
        Meeting = live ? meeting : null;
        Mode = live && !collapsed.Contains(meeting!) ? IslandMode.Open : IslandMode.Closed;
        return Mode != before;
    }

    /// <summary>Whether the user collapsed <paramref name="meeting"/> (the per-meeting memory).</summary>
    public bool IsCollapsed(string meeting) => collapsed.Contains(meeting);

    /// <summary>
    /// D1: the explicit collapse (the strip's collapse button, or Esc on the island): the live meeting shows the closed pill
    /// from now on, for this meeting only. False when nothing is live (the idle wordmark is already closed).
    /// </summary>
    public bool Collapse()
    {
        if (Meeting is null || Mode == IslandMode.Closed) return false;
        collapsed.Add(Meeting);
        Mode = IslandMode.Closed;
        return true;
    }

    /// <summary>
    /// A click on the collapsed pill of a live meeting opens it again and forgets the collapse. False when nothing is live
    /// (the idle wordmark's click opens <c>theuth://meetings</c> instead).
    /// </summary>
    public bool Expand()
    {
        if (Meeting is null) return false;
        collapsed.Remove(Meeting);
        Mode = IslandMode.Open;
        return true;
    }

    /// <summary>
    /// D1: the pointer left the island. Never collapses it (the hover-open/close model is gone; WinUI also drops pointer
    /// events on the inactive NOACTIVATE window, which made hover-driven state unreliable). Returns the unchanged mode.
    /// </summary>
    public IslandMode PointerLeft() => Mode;

    // MARK: persistence (settings.json key `islandFrames`: {"\\\\.\\DISPLAY1": [dx, dy], …})

    public const string FramesKey = "islandFrames";

    public const string AlwaysShowKey = "islandAlwaysShow";

    public const string HiddenFromCaptureKey = "islandHiddenFromCapture";

    public string FramesJson
    {
        get
        {
            using var ms = new MemoryStream();
            using (var w = new Utf8JsonWriter(ms))
            {
                w.WriteStartObject();
                foreach (var (k, v) in frames.OrderBy(kv => kv.Key, StringComparer.Ordinal))
                {
                    w.WritePropertyName(k);
                    w.WriteStartArray();
                    w.WriteNumberValue(v.Dx);
                    w.WriteNumberValue(v.Dy);
                    w.WriteEndArray();
                }
                w.WriteEndObject();
            }
            return System.Text.Encoding.UTF8.GetString(ms.ToArray());
        }
    }

    /// <summary>Loads the saved offsets; malformed input leaves the model empty (never throws).</summary>
    public void LoadFrames(string? json)
    {
        frames.Clear();
        if (string.IsNullOrWhiteSpace(json)) return;
        try
        {
            using var doc = JsonDocument.Parse(json);
            if (doc.RootElement.ValueKind != JsonValueKind.Object) return;
            foreach (var p in doc.RootElement.EnumerateObject())
            {
                if (p.Value.ValueKind != JsonValueKind.Array || p.Value.GetArrayLength() != 2) continue;
                var a = p.Value[0];
                var b = p.Value[1];
                if (a.ValueKind != JsonValueKind.Number || b.ValueKind != JsonValueKind.Number) continue;
                frames[p.Name] = ((int)Math.Round(a.GetDouble()), (int)Math.Round(b.GetDouble()));
            }
        }
        catch (JsonException) { }
    }

    public void Load(SettingsStore d)
    {
        LoadFrames(d.String(FramesKey));
        AlwaysShow = d.Bool(AlwaysShowKey);
        if (d.Contains(HiddenFromCaptureKey)) HiddenFromCapture = d.Bool(HiddenFromCaptureKey);
    }

    public void Save(SettingsStore d)
    {
        if (frames.Count == 0) d.Remove(FramesKey); else d.Set(FramesKey, FramesJson);
        d.Set(AlwaysShowKey, AlwaysShow);
        d.Set(HiddenFromCaptureKey, HiddenFromCapture);
        d.Flush();
    }

    /// <summary><c>rec mm:ss</c> on the closed pill (<c>meeting.ts</c> <c>mmss</c>: minutes run past 59).</summary>
    public static string Clock(long elapsedMs)
    {
        long s = Math.Max(0, elapsedMs / 1000);
        return string.Format(CultureInfo.InvariantCulture, "{0:00}:{1:00}", s / 60, s % 60);
    }
}