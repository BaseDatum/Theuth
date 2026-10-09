using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Island;
using Theuth.Companion.PointAndSay;

namespace Theuth.Companion;

/// <summary>
/// 149 WW3's start (the marked line in <c>App.OnLaunched</c>, after W4's Ask bar, before <c>Store.Start()</c>): the island
/// controller + window over the Store (WW1's meeting plumbing and raw stream hook), then point-and-say over the island's
/// voice mode and W2's <c>Hotkeys</c>. <c>--island-always</c> keeps the closed pill (Settings › Island "Always show" for the
/// probe); <c>--point-probe</c> drives one simulated hold through the machine 3 s after start (build-B has no one to
/// speak; the overlay, the screen_context call and the send path are exercised, the text is empty → "Nothing recognized").
/// </summary>
internal sealed class WW3Install : IDisposable
{
    /// <summary>The running install (the Settings pages edit through it); null in builds without W2's surfaces.</summary>
    public static WW3Install? Current { get; private set; }

    public IslandController Island { get; }

    public PointController Point { get; }

    private WW3Install(IslandController island, PointController point)
    {
        Island = island;
        Point = point;
    }

    public static WW3Install? Install(Store store, DispatcherQueue dispatcher)
    {
        if (Ui.Surfaces.Current is not { } surfaces)
        {
            Log.Write("149 WW3: not installed (no W2 surfaces)");
            return null;
        }
        // The ONE settings store (SettingsModel.Shared's): a second instance's Flush would drop the other's keys.
        var settings = Settings.SettingsModel.Shared.Store;
        var island = new IslandController(store, dispatcher, settings);
        var point = new PointController(store, dispatcher, island, surfaces.Hotkeys, () => island.PointSettings);
        island.EyeChanged += hidden => point.SetHiddenFromCapture(hidden);
        var install = new WW3Install(island, point);
        Current = install;
        if (Environment.GetCommandLineArgs().Contains("--point-probe")) PointProbe.Run(dispatcher, point, island);
        if (Environment.GetCommandLineArgs().Contains("--point-replay")) PointProbe.Replay(dispatcher, point);
        return install;
    }

    public void Dispose()
    {
        if (Current == this) Current = null;
        Point.Dispose();
        Island.Dispose();
    }
}