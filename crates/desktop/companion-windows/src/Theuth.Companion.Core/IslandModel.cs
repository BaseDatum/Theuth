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

/// <summary>The island's visual state (149 §3.6): the closed pill, the open strip + pane, or dragged away from the top centre.</summary>
public enum IslandMode
{
    Closed,
    Open,
    Detached,
}

/// <summary>
/// Design 149 §3.6 / §7.4 #1 / D9–D11, the pure half of the island window: where it sits (top-centre of a monitor's work
/// area; a saved offset per display, keyed by monitor device name, validated before use; snap-back within
/// <see cref="SnapPx"/> of the default; double-click = snap back), whether it is shown (D10: a live meeting, or a topic/brief
/// in the last <see cref="PendingWindow"/>, or "Always show"), and its mode. Mirrors the Swift <c>IslandModel</c>; the
/// WinUI window executes what this decides. Offsets are stored relative to the work area's origin in physical pixels, so
/// a monitor that moves in the virtual-screen layout keeps its island where the user put it.
/// </summary>
public sealed class IslandModel
{
    /// <summary>A released drag whose centre is within this many physical pixels × scale of the top-centre default snaps back (§3.6 "40 pt").</summary>
    public const int SnapPx = 40;

    /// <summary>The pill hugs the top edge of the work area by this gap (effective px); 0 would touch a top taskbar.</summary>
    public const int TopGapPx = 6;

    /// <summary>D10 / Revision 1 #13: a topic or brief keeps the island shown this long after it arrived.</summary>
    public static readonly TimeSpan PendingWindow = TimeSpan.FromMinutes(10);

    private readonly Dictionary<string, (int Dx, int Dy)> frames = new(StringComparer.OrdinalIgnoreCase);

    /// <summary>A saved frame no longer lands on its monitor's work area (it shrank or moved): rejected and forgotten; the window logs it.</summary>
    public event Action<string, (int Dx, int Dy)>? FrameRejected;

    public IslandMode Mode { get; private set; } = IslandMode.Closed;

    /// <summary>Settings › Island "Always show": the closed pill stays when nothing is live.</summary>
    public bool AlwaysShow { get; set; }

    /// <summary>Pinned by a click: hover-out does not close it.</summary>
    public bool Pinned { get; set; }

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
    /// was used (the window is <see cref="IslandMode.Detached"/> then).
    /// </summary>
    public (PxRect Frame, bool Saved) Place(IslandDisplay d, (int Width, int Height) size)
    {
        if (frames.TryGetValue(d.Device, out var off))
        {
            var r = new PxRect(d.WorkArea.X + off.Dx, d.WorkArea.Y + off.Dy, size.Width, size.Height);
            if (r.Intersects(d.WorkArea)) return (r, true);
            frames.Remove(d.Device); // off-screen memory (a monitor shrank): forget it
            FrameRejected?.Invoke(d.Device, off);
        }
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
            if (Mode == IslandMode.Detached) Mode = IslandMode.Closed;
            return def;
        }
        frames[d.Device] = (frame.X - d.WorkArea.X, frame.Y - d.WorkArea.Y);
        Mode = IslandMode.Detached;
        return frame;
    }

    /// <summary>Double-click on the grip, or Settings › Island "reset position" for one display.</summary>
    public PxRect SnapBack(IslandDisplay d, (int Width, int Height) size)
    {
        frames.Remove(d.Device);
        if (Mode == IslandMode.Detached) Mode = IslandMode.Closed;
        return DefaultFrame(d, size);
    }

    /// <summary>Settings › Island "reset all".</summary>
    public void ResetAll()
    {
        frames.Clear();
        if (Mode == IslandMode.Detached) Mode = IslandMode.Closed;
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

    // MARK: visibility (D10)

    /// <summary><c>meeting_status.state</c> words that mean a meeting is live (<c>Island.tsx:70</c>): Starting · Recording · Paused · Stopping.</summary>
    public static bool IsLive(string state) => state is "Starting" or "Recording" or "Paused" or "Stopping";

    /// <summary>
    /// D10 / Revision 1 #13: shown while the meeting is live, or a topic/brief arrived within <see cref="PendingWindow"/>
    /// of <paramref name="now"/>, or "Always show" keeps the closed pill.
    /// </summary>
    public bool ShouldShow(string state, DateTimeOffset? lastTopicOrBrief, DateTimeOffset now) =>
        IsLive(state) || (lastTopicOrBrief is { } t && now - t <= PendingWindow && now >= t) || AlwaysShow;

    /// <summary>When the island must be re-evaluated after the last topic/brief (the hide timer's due time), or null.</summary>
    public static DateTimeOffset? HideDue(string state, DateTimeOffset? lastTopicOrBrief) =>
        !IsLive(state) && lastTopicOrBrief is { } t ? t + PendingWindow : null;

    // MARK: open / close

    /// <summary>Hover 0.3 s or a click opens; a click also pins.</summary>
    public void Open(bool pin)
    {
        if (Mode == IslandMode.Closed) Mode = IslandMode.Open;
        if (pin) Pinned = true;
    }

    /// <summary>The pointer left (100 ms grace, the window's timer) or Esc: closes unless pinned, or when <paramref name="force"/>.</summary>
    public bool Close(bool force = false)
    {
        if (!force && Pinned) return false;
        Pinned = false;
        if (Mode == IslandMode.Open) Mode = IslandMode.Closed;
        return true;
    }

    // MARK: persistence (settings.json key `islandFrames`: {"\\\\.\\DISPLAY1": [dx, dy], …})

    public const string FramesKey = "islandFrames";

    public const string AlwaysShowKey = "islandAlwaysShow";

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
    }

    public void Save(SettingsStore d)
    {
        if (frames.Count == 0) d.Remove(FramesKey); else d.Set(FramesKey, FramesJson);
        d.Set(AlwaysShowKey, AlwaysShow);
        d.Flush();
    }

    /// <summary><c>rec mm:ss</c> on the closed pill (<c>meeting.ts</c> <c>mmss</c>: minutes run past 59).</summary>
    public static string Clock(long elapsedMs)
    {
        long s = Math.Max(0, elapsedMs / 1000);
        return string.Format(CultureInfo.InvariantCulture, "{0:00}:{1:00}", s / 60, s % 60);
    }
}
