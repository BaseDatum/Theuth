using Microsoft.UI;
using Microsoft.UI.Windowing;
using Theuth.Companion.Core;

namespace Theuth.Companion.PointAndSay;

/// <summary>
/// The attached monitors as the island and the overlays see them (149 §7.4 #1, #4): WinAppSDK's <see cref="DisplayArea"/>
/// for the physical-pixel bounds and work area, the HMONITOR behind its <c>DisplayId</c> for the device name (the island's
/// memory key) and the effective DPI. Enumerated on demand — <c>WM_DISPLAYCHANGE</c> re-reads it.
/// </summary>
internal sealed record ScreenMonitor(string Device, PxRect Bounds, PxRect WorkArea, double Scale, bool Primary, nint Handle)
{
    public IslandDisplay AsIslandDisplay() => new(Device, WorkArea, Scale, Primary);

    public bool Contains(int x, int y) => Bounds.Contains(x, y);

    public static IReadOnlyList<ScreenMonitor> All()
    {
        var list = new List<ScreenMonitor>();
        try
        {
            var primary = DisplayArea.Primary;
            foreach (var area in DisplayArea.FindAll())
            {
                nint h = Win32Interop.GetMonitorFromDisplayId(area.DisplayId);
                var info = Gdi.MonitorInfo(h);
                var ob = area.OuterBounds;
                var wa = area.WorkArea;
                string device = info?.Device is { Length: > 0 } d ? d : $"display-{area.DisplayId.Value}";
                bool isPrimary = primary is not null && primary.DisplayId.Value == area.DisplayId.Value;
                list.Add(new ScreenMonitor(device, new PxRect(ob.X, ob.Y, ob.Width, ob.Height), new PxRect(wa.X, wa.Y, wa.Width, wa.Height), Gdi.ScaleOf(h), isPrimary, h));
            }
        }
        catch (Exception e)
        {
            Log.Write($"monitors: enumeration failed: {e.GetType().Name}: {e.Message}");
        }
        if (list.Count == 0) list.Add(new ScreenMonitor(@"\\.\DISPLAY1", new PxRect(0, 0, 1024, 768), new PxRect(0, 0, 1024, 728), 1.0, true, 0));
        return list;
    }

    /// <summary>The monitor under a virtual-screen point, else the primary.</summary>
    public static ScreenMonitor At(IReadOnlyList<ScreenMonitor> all, int x, int y)
    {
        foreach (var m in all) if (m.Contains(x, y)) return m;
        foreach (var m in all) if (m.Primary) return m;
        return all[0];
    }

    public string Describe() => $"{Device} bounds={Bounds.X},{Bounds.Y} {Bounds.Width}x{Bounds.Height} work={WorkArea.X},{WorkArea.Y} {WorkArea.Width}x{WorkArea.Height} scale={Scale:0.##}{(Primary ? " primary" : "")}";
}
