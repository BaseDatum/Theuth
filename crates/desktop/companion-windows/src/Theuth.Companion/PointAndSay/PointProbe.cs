using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Island;

namespace Theuth.Companion.PointAndSay;

/// <summary>
/// <c>--point-probe</c> (149 §10 probes): build-B has nobody to hold the key or speak, so this drives one gesture through the
/// real path 3 s after start — a simulated press, a 1.5 s hold while the overlays draw the real pointer, a release → the
/// overlays hide, <c>dictation_stop</c> and <c>screen_context</c> run against the engine and the island shows the voice mode;
/// with no speech the machine lands on "Nothing recognized" and nothing is sent. The log carries every step
/// (<c>point:</c> lines) and the screen_context result; the gate reads it.
/// </summary>
internal static class PointProbe
{
    public static void Run(DispatcherQueue dispatcher, PointController point, IslandController island)
    {
        var t = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromSeconds(3);
        t.IsRepeating = false;
        t.Tick += (_, _) =>
        {
            Log.Write("point-probe: press");
            point.Simulate(new PointEvent.Pressed());
            var hold = dispatcher.CreateTimer();
            hold.Interval = TimeSpan.FromMilliseconds(1500);
            hold.IsRepeating = false;
            hold.Tick += (_, _) =>
            {
                Log.Write($"point-probe: release (state {point.State.Json}, island {(island.Window.IsShown ? "shown" : "hidden")} voice={(island.Window.InVoiceMode ? "on" : "off")})");
                point.Simulate(new PointEvent.Released());
            };
            hold.Start();
        };
        t.Start();
        Log.Write("point-probe: armed (press in 3 s, hold 1.5 s)");
    }
}
