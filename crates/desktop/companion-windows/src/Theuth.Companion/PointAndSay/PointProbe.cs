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

    // MARK: 153 wave C probes (proof 20, the §12.4 screenshots, the SwitchMode log line)

    private static DispatcherQueueTimer? probe153;

    /// <summary>The value after a flag (<c>--probe-card expanded</c>), or null.</summary>
    private static string? Arg(string flag)
    {
        var args = Environment.GetCommandLineArgs();
        int i = Array.IndexOf(args, flag);
        return i >= 0 && i + 1 < args.Length && !args[i + 1].StartsWith("--", StringComparison.Ordinal) ? args[i + 1] : i >= 0 ? "" : null;
    }

    /// <summary>
    /// <c>--probe-command</c> (heard / working / approval / done), <c>--probe-states</c> (every §12.4 state on the island view and
    /// the card window), <c>--probe-card collapsed|expanded|failed</c>, <c>--probe-switch</c> (a simulated voice-key hold switched
    /// to Command mid-hold, released on the Command key) — 5 s after start. Never acting: no send, no resolve, no cancel, no listen;
    /// recorded desk events go through <c>Store.HandleEvent</c> like the engine's stream. Each step holds <see cref="HoldMs"/> so a
    /// screenshot can be taken, logs <c>probe153: …</c> and writes PNGs of both surfaces to <c>%TEMP%\153-*.png</c>. Use with
    /// <c>--shareable</c> for desktop screenshots (the windows are excluded from capture otherwise; the PNGs are not affected).
    /// </summary>
    public static void Install153(DispatcherQueue dispatcher, PointController point)
    {
        string? which = Arg("--probe-command") is not null ? "command" : Arg("--probe-states") is not null ? "states"
            : Arg("--probe-card") is { } c ? "card:" + (c.Length > 0 ? c : "expanded") : Arg("--probe-switch") is not null ? "switch" : null;
        if (which is null) return;
        current = point;
        var t = probe153 = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromSeconds(5);
        t.IsRepeating = false;
        t.Tick += async (_, _) =>
        {
            try
            {
                switch (which)
                {
                    case "command": await CommandSequence(point); break;
                    case "states": await StatesSequence(point); break;
                    case "switch": await SwitchSequence(point); break;
                    default: await CardSequence(point, which[5..]); break;
                }
            }
            catch (Exception e)
            {
                Log.Write($"probe153: failed {e.GetType().Name}: {e.Message}");
            }
        };
        t.Start();
        Log.Write($"probe153: armed ({which}, in 5 s)");
    }

    /// <summary>Per step: long enough for a desktop screenshot over windows-mcp.</summary>
    public const int HoldMs = 4000;

    private static PointController? current;

    /// <summary>The controller's own clock, so the card's elapsed seconds read right.</summary>
    private static ulong Ms => current?.Clock ?? 0;

    private static string Ev(string type, string session, string rest) => "{\"type\":\"" + type + "\",\"session\":\"" + session + "\",\"turn\":\"t1\"" + rest + "}";

    private static async Task Hold(PointController p, string name)
    {
        await Task.Delay(400); // one layout pass
        await p.ProbeCapture(name);
        await Task.Delay(HoldMs);
    }

    /// <summary>§10.3 / proof 20: a recorded desk sequence with purpose strings → heard, working, approval, done.</summary>
    private static async Task CommandSequence(PointController p)
    {
        const string s = "probe-desk";
        const string said = "create a reminder to test at six and clean the old build folder";
        Log.Write("probe153: command sequence — heard · working · approval · done (recorded, never acting)");
        p.BeginProbe(PointMode.Command, s);
        p.ProbeStep(c => { }, null, null, said);
        await Hold(p, "command-listening");
        p.ProbeStep(c => { c.Released(Ms); c.FinalText(said, Ms); }, new CommandInput.Heard(said, Ms), new PointState.Sending(Ms), said, isWorking: false);
        await Hold(p, "command-heard");
        p.ProbeStep(c => { }, null, null, null, isWorking: true);
        p.ProbeEvent(Ev("TurnStarted", s, ",\"text\":\"[command, 4 s] " + said + "\",\"model\":\"probe\""));
        p.ProbeEvent(Ev("ToolCallStarted", s, ",\"call_id\":\"c1\",\"name\":\"powershell\",\"args\":{\"script\":\"Get-ScheduledTask\",\"purpose\":\"checking: Reminders · no task named test yet\"}"));
        await Task.Delay(300);
        p.ProbeEvent(Ev("ToolCallFinished", s, ",\"call_id\":\"c1\",\"output\":\"exit 0 · 0.4 s\",\"is_error\":false,\"files\":[]"));
        p.ProbeEvent(Ev("ToolCallStarted", s, ",\"call_id\":\"c2\",\"name\":\"powershell\",\"args\":{\"script\":\"Register-ScheduledTask …\",\"purpose\":\"executing: reminder “test” at 18:00\"}"));
        await Task.Delay(300);
        p.ProbeEvent(Ev("ToolCallFinished", s, ",\"call_id\":\"c2\",\"output\":\"exit 0 · 0.9 s\",\"is_error\":false,\"files\":[]"));
        p.ProbeEvent(Ev("ToolCallStarted", s, ",\"call_id\":\"c3\",\"name\":\"shell\",\"args\":{\"command\":\"dir C:\\\\Develop\\\\theuth\\\\build\",\"purpose\":\"checking: build\\\\ exists · 1.2 GB · 3 days old\"}"));
        await Task.Delay(300);
        p.ProbeEvent(Ev("ToolCallFinished", s, ",\"call_id\":\"c3\",\"output\":\"exit 0 · 0.2 s\",\"is_error\":false,\"files\":[]"));
        p.ProbeEvent(Ev("ToolCallStarted", s, ",\"call_id\":\"c4\",\"name\":\"shell\",\"args\":{\"command\":\"rmdir /s /q C:\\\\Develop\\\\theuth\\\\build\",\"purpose\":\"executing: remove C:\\\\Develop\\\\theuth\\\\build\"}"));
        await Hold(p, "command-working");
        p.ProbeEvent("{\"type\":\"ApprovalRequested\",\"id\":\"probe-ap1\",\"session\":\"" + s + "\",\"tool\":\"shell\",\"args\":{\"command\":\"rmdir /s /q C:\\\\Develop\\\\theuth\\\\build\"},\"risk\":\"destructive\"}");
        await Hold(p, "command-approval");
        p.ProbeEvent("{\"type\":\"ApprovalResolved\",\"id\":\"probe-ap1\",\"decision\":\"allow\",\"timed_out\":false}");
        p.ProbeEvent(Ev("ToolCallFinished", s, ",\"call_id\":\"c4\",\"output\":\"exit 0 · 1.1 s\",\"is_error\":false,\"files\":[]"));
        p.ProbeEvent(Ev("ToolCallStarted", s, ",\"call_id\":\"c5\",\"name\":\"shell\",\"args\":{\"command\":\"dir C:\\\\Develop\\\\theuth\",\"purpose\":\"verifying: build\\\\ is gone · reminder listed\"}"));
        await Task.Delay(300);
        p.ProbeEvent(Ev("ToolCallFinished", s, ",\"call_id\":\"c5\",\"output\":\"exit 0 · 0.3 s\",\"is_error\":false,\"files\":[]"));
        p.ProbeEvent(Ev("TextDelta", s, ",\"text\":\"✓ Reminder “test” set for 18:00; \""));
        p.ProbeEvent(Ev("TextDelta", s, ",\"text\":\"C:\\\\Develop\\\\theuth\\\\build removed (1.2 GB freed).\""));
        p.ProbeEvent(Ev("TurnFinished", s, ",\"stop_reason\":\"end_turn\",\"context\":1"));
        await Hold(p, "command-done");
        p.ProbeExpand(true);
        await Hold(p, "command-done-expanded");
        p.EndProbe();
    }

    /// <summary>§12.4: every state on both surfaces from fixtures — listening, finishing, thinking, answer, speaking, failed (+ stopped).</summary>
    private static async Task StatesSequence(PointController p)
    {
        const string s = "probe-desk";
        const string asked = "why does the onset test fail";
        const string answer = "The failing test is `vad::gate_onset`. The onset run is now **8 frames** (160 ms) but the fixture feeds 7.\n\n- fix the fixture → vad.rs:112\n- or keep 7 and lower `ONSET_MS` to 140\n\nRecommend the first; it matches design 54 §0.3.";
        Log.Write("probe153: §12.4 states — listening · finishing · thinking · answer · speaking · failed · stopped (fixtures)");
        p.BeginProbe(PointMode.Ask, s);
        p.ProbeStep(c => { }, null, null, asked);
        await Hold(p, "state-listening");
        p.ProbeStep(c => c.Released(Ms), null, new PointState.Sending(Ms), asked, isWorking: false);
        await Hold(p, "state-finishing");
        p.ProbeStep(c => { c.FinalText(asked, Ms); c.TurnStarted(Ms); c.ToolStarted("t1", "code_refs"); }, null, null, asked, isWorking: true);
        await Hold(p, "state-thinking");
        p.ProbeExpand(true);
        await Hold(p, "state-thinking-expanded");
        p.ProbeAttach();
        p.ProbeStep(c => { c.ToolFinished("t1", false); c.ToolStarted("t2", "read_file"); c.ToolFinished("t2", false); c.TextDelta(answer, Ms); c.TurnFinished("end_turn", Ms); }, null, new PointState.Replying(), asked, isWorking: false);
        await Hold(p, "state-answer");
        p.ProbeDetachCollapsed();
        await Hold(p, "state-answer-collapsed-card");
        p.ProbeExpand(true);
        await Hold(p, "state-answer-expanded");
        p.ProbeStep(c => c.SpeechStarted(Ms));
        await Hold(p, "state-speaking-expanded");
        p.ProbeAttach();
        p.ProbeStep(c => { });
        await Hold(p, "state-speaking");
        p.ProbeStep(c => c.SpeechEnded(Ms));
        p.BeginProbe(PointMode.Ask, s);
        p.ProbeStep(c => { c.Released(Ms); c.FinalText("summarise today's meetings", Ms); c.TurnStarted(Ms); c.Failed("model unavailable: no provider credential on this engine", Ms); }, null, new PointState.Replying(), "summarise today's meetings", isWorking: false);
        await Hold(p, "state-failed");
        p.ProbeExpand(true);
        await Hold(p, "state-failed-expanded");
        p.ProbeAttach();
        p.BeginProbe(PointMode.Ask, s);
        p.ProbeStep(c => { c.Released(Ms); c.FinalText(asked, Ms); c.TurnStarted(Ms); c.TextDelta("The failing test is `vad::gate_onset`; the onset", Ms); c.Stop(Ms); }, null, new PointState.Replying(), asked, isWorking: false);
        await Hold(p, "state-stopped");
        p.EndProbe();
    }

    /// <summary>Proof 20's <c>--probe-card</c>: one card state in the window (collapsed / expanded / failed).</summary>
    private static async Task CardSequence(PointController p, string which)
    {
        const string s = "probe-desk";
        Log.Write($"probe153: card {which}");
        p.BeginProbe(PointMode.Ask, s);
        if (which == "failed")
        {
            p.ProbeStep(c => { c.Released(Ms); c.FinalText("what's on my calendar", Ms); c.TurnStarted(Ms); c.Failed("model unavailable: no provider credential on this engine", Ms); }, null, new PointState.Replying(), "what's on my calendar", isWorking: false);
        }
        else
        {
            p.ProbeStep(c => { c.Released(Ms); c.FinalText("why does the onset test fail", Ms); c.TurnStarted(Ms); c.ToolStarted("t1", "code_refs"); c.ToolFinished("t1", false); c.ToolStarted("t2", "cargo test -p voice"); c.ToolFinished("t2", false);
                c.TextDelta("The failing test is `vad::gate_onset`; the 160 ms onset frame count changed in vad.rs:112 …\n\n- fix the fixture → vad.rs:112\n- or keep 7 and lower `ONSET_MS` to 140\n\n```rust\nconst ONSET_MS: u32 = 160;\n```", Ms); c.TurnFinished("end_turn", Ms); },
                null, new PointState.Replying(), "why does the onset test fail", isWorking: false);
        }
        if (which == "collapsed") p.ProbeDetachCollapsed();
        else p.ProbeExpand(true);
        await Hold(p, $"card-{which}");
        await Task.Delay(4000);
        p.EndProbe();
    }

    /// <summary>The SwitchMode log line: a simulated voice-key hold, the Command key pressed mid-hold, released on the Command key.</summary>
    private static async Task SwitchSequence(PointController p)
    {
        Log.Write("probe153: switch — hold the voice key, press Command mid-hold, release Command (simulated keys; no speech, nothing sent)");
        p.SimulateKey(KeyAction.VoiceKey, down: true);
        await Task.Delay(700);
        p.SimulateKey(KeyAction.Command, down: true);
        await Task.Delay(700);
        Log.Write($"probe153: switch — mode now {p.Mode.RawValue()}, state {p.State.Json}");
        p.SimulateKey(KeyAction.Command, down: false);
        await Task.Delay(300);
        p.SimulateKey(KeyAction.VoiceKey, down: false);
        Log.Write($"probe153: switch — released; state {p.State.Json} mode {p.Mode.RawValue()}");
    }
}
