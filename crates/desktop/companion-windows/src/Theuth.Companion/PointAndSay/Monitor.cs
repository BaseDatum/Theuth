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
            // EnumDisplayMonitors + GetMonitorInfoW: physical pixels under PerMonitorV2, the device name as the memory key.
            // (DisplayArea.FindAll() threw InvalidCastException on build-B's WinAppSDK 2.5.1 — kept out.)
            Gdi.MonitorEnumProc cb = (nint h, nint _, ref Gdi.Rect r, nint _) =>
            {
                if (Gdi.MonitorInfo(h) is { } info)
                {
                    var mi = new Gdi.MonitorInfoEx { Size = (uint)System.Runtime.InteropServices.Marshal.SizeOf<Gdi.MonitorInfoEx>() };
                    bool primary = Gdi.GetMonitorInfoW(h, ref mi) && (mi.Flags & Gdi.MonitorInfoFPrimary) != 0;
                    list.Add(new ScreenMonitor(info.Device.Length > 0 ? info.Device : $"monitor-{h}",
                        new PxRect(info.Monitor.Left, info.Monitor.Top, info.Monitor.Right - info.Monitor.Left, info.Monitor.Bottom - info.Monitor.Top),
                        new PxRect(info.Work.Left, info.Work.Top, info.Work.Right - info.Work.Left, info.Work.Bottom - info.Work.Top),
                        Gdi.ScaleOf(h), primary, h));
                }
                return true;
            };
            Gdi.EnumDisplayMonitors(0, 0, cb, 0);
            GC.KeepAlive(cb);
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
