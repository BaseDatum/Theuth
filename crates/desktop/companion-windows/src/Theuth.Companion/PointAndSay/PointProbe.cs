using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Island;

namespace Theuth.Companion.PointAndSay;

/// <summary>
/// <c>--point-probe</c> (149 §10 probes): build-B has nobody to hold the key or speak, so this drives one gesture through the
/// real path 3 s after start — a simulated press, a 1.5 s hold while the overlays draw the real pointer, a release → the
/// overlays hide, <c>dictation_stop</c> and <c>screen_context</c> run against the engine and the island shows the voice mode;
/// with no speech the machine lands on "Nothing recognized" and nothing is sent. <c>--point-replay</c>: 4 s after start a
/// recorded desk reply streams into the answer card through <c>Store.HandleEvent</c> (no model, no speech). The log carries
/// every step (<c>point:</c> lines) and the screen_context result; the gate reads it.
/// </summary>
internal static class PointProbe
{
    // Dispatcher timers are not kept alive by the queue: hold them here until they fire.
    private static DispatcherQueueTimer? arm, hold, replay;

    public static void Run(DispatcherQueue dispatcher, PointController point, IslandController island)
    {
        var t = arm = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromSeconds(3);
        t.IsRepeating = false;
        t.Tick += (_, _) =>
        {
            Log.Write("point-probe: press");
            point.Simulate(new PointEvent.Pressed());
            hold = dispatcher.CreateTimer();
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

    public static void Replay(DispatcherQueue dispatcher, PointController point)
    {
        var t = replay = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromSeconds(4);
        t.IsRepeating = false;
        t.Tick += (_, _) =>
        {
            const string s = "replay-desk";
            static string Ev(string type, string session, string rest) => "{\"type\":\"" + type + "\",\"session\":\"" + session + "\",\"turn\":\"t1\"" + rest + "}";
            var lines = new List<string>
            {
                Ev("TurnStarted", s, ""),
                Ev("ToolCallStarted", s, ",\"call_id\":\"c1\",\"name\":\"screen_context\""),
                Ev("ToolCallFinished", s, ",\"call_id\":\"c1\",\"name\":\"screen_context\""),
                Ev("TextDelta", s, ",\"text\":\"That's the theuth island — \""),
                Ev("TextDelta", s, ",\"text\":\"the meeting pill showing a 12-minute recording with two topics. \""),
                Ev("TextDelta", s, ",\"text\":\"Click it to see the brief on PROJ-12.\""),
                Ev("TurnFinished", s, ",\"stop_reason\":\"end_turn\",\"context\":1"),
            };
            _ = point.ReplayAsync("what is this at the top of my screen", lines);
        };
        t.Start();
        Log.Write("point-replay: armed (in 4 s)");
    }
}
