using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Theuth.Companion.Core;
using Theuth.Companion.PointAndSay;

namespace Theuth.Companion.Settings;

/// <summary>
/// 149 WW3 (§3.8): the Island and Voice key pages of the Settings window. Values are the ones WW3 persists —
/// <see cref="IslandModel"/> (`islandFrames` · `islandAlwaysShow` · `islandHiddenFromCapture`) and <see cref="PointSettings"/>
/// (`voiceKey*`) — in the one shared <see cref="SettingsStore"/> (<see cref="SettingsModel.Store"/>), edited live through
/// <see cref="WW3Install.Current"/>. The chords (PTT, eye, voice key) are rows of WW1's key table: shown here, changed in Keys.
/// Controls: <c>settings-island-always</c> · <c>settings-island-eye</c> · <c>settings-island-reset-&lt;n&gt;</c> ·
/// <c>settings-island-reset-all</c> · <c>settings-voice-enabled</c> · <c>settings-voice-screen</c> · <c>settings-voice-accessibility</c> ·
/// <c>settings-voice-annotate</c> · <c>settings-voice-speak</c>.
/// </summary>
internal sealed partial class SettingsWindow
{
    private void Island(StackPanel page)
    {
        Heading(page, "Island");
        var ww3 = WW3Install.Current;
        if (ww3 is null)
        {
            Caption(page, "The island is not running in this build.");
            return;
        }
        var island = ww3.Island;
        Toggle(page, "Always show the closed pill", "settings-island-always", island.Model.AlwaysShow, on => island.SetAlwaysShow(on));
        Caption(page, "Off: the island appears while a meeting is live and for 10 minutes after the last topic or brief. On: the pill stays; a click opens Meetings.");
        Toggle(page, "Hidden from screen capture (eye)", "settings-island-eye", island.Model.HiddenFromCapture, on => island.SetHiddenFromCapture(on));
        Caption(page, "Applies to the island and the point-and-say overlays (WDA_EXCLUDEFROMCAPTURE). Best effort: Windows honours it for screenshots and most capture apps.");
        Caption(page, $"Push-to-talk {ChordText(KeyAction.MeetingPtt)} · eye {ChordText(KeyAction.MeetingEye)} — while a meeting is live; change them in Keys.");

        page.Children.Add(new TextBlock { Text = "Position", FontSize = 14, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Margin = new Thickness(0, 8, 0, 0) });
        Caption(page, "Drag the island anywhere; double-click its grip to snap it back to the top centre. Each monitor remembers its own place.");
        var frames = island.Model.Frames;
        int n = 1;
        foreach (var m in ScreenMonitor.All())
        {
            bool saved = frames.TryGetValue(m.Device, out var off);
            var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10 };
            var label = new TextBlock { VerticalAlignment = VerticalAlignment.Center, Text = $"{m.Device}{(m.Primary ? " (main)" : "")} — {(saved ? $"moved to +{off.Dx},+{off.Dy}" : "top centre")}" };
            var reset = new Button { Content = "Reset position", IsEnabled = saved };
            AutomationProperties.SetAutomationId(reset, $"settings-island-reset-{n}");
            var device = m;
            reset.Click += (_, _) => { island.ResetPosition(device); ShowPage("island"); };
            row.Children.Add(reset);
            row.Children.Add(label);
            page.Children.Add(row);
            n++;
        }
        var all = new Button { Content = "Reset all positions", IsEnabled = frames.Count > 0 };
        AutomationProperties.SetAutomationId(all, "settings-island-reset-all");
        all.Click += (_, _) => { island.ResetAllPositions(); ShowPage("island"); };
        page.Children.Add(all);
    }

    private void VoiceKey(StackPanel page)
    {
        Heading(page, "Voice key");
        var ww3 = WW3Install.Current;
        if (ww3 is null)
        {
            Caption(page, "Point-and-say is not running in this build.");
            return;
        }
        var island = ww3.Island;
        var s = island.PointSettings;
        Caption(page, $"Hold {ChordText(KeyAction.VoiceKey)} anywhere to ask theuth about what is on your screen; release to send, double-tap to lock hands-free. Change the key in Keys.");
        Toggle(page, "Voice key on", "settings-voice-enabled", s.Enabled, on => Edit(island, p => p.Enabled = on));
        Toggle(page, "Send the screen", "settings-voice-screen", s.IncludeScreen, on => Edit(island, p => p.IncludeScreen = on));
        Caption(page, "The displays the pointer visits are captured by the engine with your trail drawn on them and sent with your words.");
        Combo(page, "Read what the pointer touched", "settings-voice-accessibility",
            new[] { ("auto", "Auto — the engine's UI Automation reader"), ("off", "Off") }, s.Accessibility,
            v => Edit(island, p => p.Accessibility = v == "off" ? "off" : "auto"));
        Caption(page, "Names the links, buttons and text under the pointer from UI Automation (the engine's reader; `accessibility: off` in the [screen …] line until it runs).");
        Combo(page, "Draw the trail", "settings-voice-annotate",
            new[] { ("always", "Always while the key is held"), ("mouse", "Only while the mouse button is held") }, s.Annotate,
            v => Edit(island, p => p.Annotate = v == "mouse" ? "mouse" : "always"));
        Toggle(page, "Speak the answer", "settings-voice-speak", s.Speak, on => Edit(island, p => p.Speak = on));
        Caption(page, "The desk's reply is read aloud through the engine's voice (speak); the answer card shows it either way.");
        var hints = new Button { Content = "Reset hints", IsEnabled = s.HintsShown > 0 };
        AutomationProperties.SetAutomationId(hints, "settings-voice-reset-hints");
        hints.Click += (_, _) => { Edit(island, p => p.HintsShown = 0); ShowPage("voice"); };
        page.Children.Add(hints);
        var disclosure = Caption(page, "While the voice key is held, theuth captures the displays the pointer visits and sends them to the model with your words; screenshots may contain private information, and text redaction cannot remove secrets embedded in pixels.");
        AutomationProperties.SetAutomationId(disclosure, "settings-voice-disclosure");
    }

    private static void Edit(Theuth.Companion.Island.IslandController island, Action<PointSettings> edit)
    {
        var next = new PointSettings
        {
            Enabled = island.PointSettings.Enabled, IncludeScreen = island.PointSettings.IncludeScreen, Accessibility = island.PointSettings.Accessibility,
            Annotate = island.PointSettings.Annotate, Speak = island.PointSettings.Speak, HintsShown = island.PointSettings.HintsShown,
        };
        edit(next);
        island.SavePointSettings(next);
    }

    private string ChordText(KeyAction action) => model.Keys.ChordFor(action)?.Symbol ?? "unassigned";
}
