using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Theuth.Companion.Breakpoints;
using Theuth.Companion.Core;
using Windows.System;
using Windows.UI.Core;

namespace Theuth.Companion.Settings;

/// <summary>
/// The <c>anywhere</c> scope's registrar questions (Hotkeys.swift, Windows form): "does another app hold this chord?"
/// is an exclusive <c>RegisterHotKey</c> on this thread, released at once (<c>ERROR_HOTKEY_ALREADY_REGISTERED</c> = held).
/// Win-key chords are refused by <see cref="KeyTable.Check"/> itself (design 134 §4.6, B7), so the reserved list is empty.
/// W2's Hotkeys owns the real registration and re-applies the table on <see cref="SettingsModel.Changed"/>.
/// </summary>
internal static class HotkeyProbe
{
    private const int ProbeId = 0x7433; // 't3'

    public static IReadOnlyCollection<Chord> Reserved { get; } = Array.Empty<Chord>();

    public static bool CanRegister(Chord c)
    {
        bool ok = W3Native.RegisterHotKey(0, ProbeId, (uint)c.Modifiers | W3Native.MOD_NOREPEAT, c.KeyCode);
        if (ok)
        {
            W3Native.UnregisterHotKey(0, ProbeId);
            return true;
        }
        int err = System.Runtime.InteropServices.Marshal.GetLastWin32Error();
        // Any other failure is treated as registrable: the real registration will tell (Swift's rule).
        return err != W3Native.ERROR_HOTKEY_ALREADY_REGISTERED;
    }
}

/// <summary>
/// Our own recorder (130 W6 item 1b; design 134 §4.6): a button that renders the chord itself. Click → "Type a shortcut…";
/// a key-down records (global rows need a modifier or an F-key, the collision policy is <see cref="KeyTable.Check"/>,
/// "Already used by …" offers "Use here instead"); Backspace/Delete clears; Esc cancels; Tab moves on; losing focus
/// cancels. <c>AutomationId="settings-keys-&lt;action&gt;"</c>.
/// </summary>
internal sealed partial class ShortcutRecorder : Button
{
    private readonly KeyAction action;
    private readonly SettingsModel model;
    private readonly TextBlock text = new() { FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.Medium, TextAlignment = TextAlignment.Center };
    private bool recording;

    public ShortcutRecorder(KeyAction action, SettingsModel model)
    {
        this.action = action;
        this.model = model;
        Content = text;
        Width = 170;
        Height = 30;
        MinWidth = 170;
        Padding = new Thickness(6, 0, 6, 0);
        HorizontalContentAlignment = HorizontalAlignment.Center;
        AutomationProperties.SetAutomationId(this, $"settings-keys-{action.RawValue()}");
        AutomationProperties.SetName(this, $"Shortcut for {model.Keys.Binding(action).Label}");
        Click += (_, _) => { if (recording) End(); else Begin(); };
        PreviewKeyDown += OnPreviewKeyDown;
        LostFocus += (_, _) => { if (recording) End(); };
        Redraw();
    }

    public void Redraw()
    {
        var chord = model.Keys.ChordFor(action);
        if (recording)
        {
            text.Text = "Type a shortcut…";
            text.Foreground = Theme.Brush("TextFillColorSecondaryBrush");
        }
        else if (chord is not null)
        {
            text.Text = chord.Symbol;
            text.Foreground = Theme.Brush("TextFillColorPrimaryBrush", 0xE0);
        }
        else
        {
            text.Text = "unassigned";
            // Lost to a move → the warning colour; unassigned by default → secondary.
            text.Foreground = model.Keys.Binding(action).DefaultChord is null
                ? Theme.Brush("TextFillColorSecondaryBrush")
                : Theme.Brush("SystemFillColorCautionBrush", 0xC0);
        }
    }

    private void Begin()
    {
        recording = true;
        Focus(FocusState.Programmatic);
        Redraw();
    }

    private void End()
    {
        recording = false;
        Redraw();
    }

    private void OnPreviewKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (!recording) return;
        var key = e.Key;
        if (IsModifier(key)) { e.Handled = true; return; }
        var mods = CurrentModifiers();
        if (mods == ChordModifiers.None)
        {
            switch (key)
            {
                case VirtualKey.Escape: End(); e.Handled = true; return;
                case VirtualKey.Back or VirtualKey.Delete:
                    model.Update(s => s.Keys.Set(null, action));
                    model.SetRowNotice(action, null);
                    End(); e.Handled = true; return;
                case VirtualKey.Tab: End(); return; // not handled: focus moves on
            }
        }
        Record(ChordFrom(key, e.KeyStatus, mods));
        End();
        e.Handled = true;
    }

    /// <summary>The recorded chord through the collision policy (internal so a probe can drive it without key events).</summary>
    public void Record(Chord chord)
    {
        var scope = KeyTable.ScopeOf(action);
        var table = model.Keys;
        var verdict = table.Check(chord, action, scope == KeyScope.Anywhere ? HotkeyProbe.Reserved : null);
        switch (verdict.Kind)
        {
            case VerdictKind.Unchanged:
                model.SetRowNotice(action, null);
                break;
            case VerdictKind.ReservedByWindows:
                model.SetRowNotice(action, new RowNotice($"{chord.Symbol} is reserved by Windows"));
                break;
            case VerdictKind.UsedBy:
                model.SetRowNotice(action, new RowNotice($"Already used by {table.Binding(verdict.Owner!.Value).Label}", chord));
                break;
            default:
                if (scope == KeyScope.Anywhere && !HotkeyProbe.CanRegister(chord))
                {
                    model.SetRowNotice(action, new RowNotice($"{chord.Symbol} is held by another app — pick another"));
                    return;
                }
                model.Update(s => s.Keys.Set(chord, action));
                model.SetRowNotice(action, null);
                break;
        }
    }

    private static bool IsModifier(VirtualKey k) => k is VirtualKey.Shift or VirtualKey.LeftShift or VirtualKey.RightShift
        or VirtualKey.Control or VirtualKey.LeftControl or VirtualKey.RightControl
        or VirtualKey.Menu or VirtualKey.LeftMenu or VirtualKey.RightMenu
        or VirtualKey.LeftWindows or VirtualKey.RightWindows;

    private static bool Down(VirtualKey k) =>
        InputKeyboardSource.GetKeyStateForCurrentThread(k).HasFlag(CoreVirtualKeyStates.Down);

    /// <summary>The modifier keys held right now, as Win32 MOD_* bits (Core's <see cref="ChordModifiers"/>).</summary>
    public static ChordModifiers CurrentModifiers()
    {
        var m = ChordModifiers.None;
        if (Down(VirtualKey.Shift)) m |= ChordModifiers.Shift;
        if (Down(VirtualKey.Control)) m |= ChordModifiers.Control;
        if (Down(VirtualKey.Menu)) m |= ChordModifiers.Alt;
        if (Down(VirtualKey.LeftWindows) || Down(VirtualKey.RightWindows)) m |= ChordModifiers.Win;
        return m;
    }

    /// <summary>W1's KeyPress rule: chars = the key's character with no modifiers, keyCode = the VK, numpad Enter kept apart.</summary>
    public static Chord ChordFrom(VirtualKey key, CorePhysicalKeyStatus status, ChordModifiers mods)
    {
        ushort vk = (ushort)key;
        if (key == VirtualKey.Enter && status.IsExtendedKey) return new Chord("\r", VirtualKeys.NumpadEnter, mods);
        var chars = CharsOf(vk, status.ScanCode);
        return new Chord(chars, vk, mods);
    }

    private static string CharsOf(uint vk, uint scanCode)
    {
        var state = new byte[256]; // empty: no modifiers → the key's plain character
        var buf = new char[8];
        if (scanCode == 0) scanCode = W3Native.MapVirtualKeyW(vk, 0);
        int n = W3Native.ToUnicodeEx(vk, scanCode, state, buf, buf.Length, 0x4, W3Native.GetKeyboardLayout(0));
        if (n <= 0) return "";
        var s = new string(buf, 0, n);
        return s.Length == 1 && char.IsControl(s[0]) && vk != 0x0D && vk != 0x1B && vk != 0x09 ? "" : s;
    }
}

/// <summary>One Settings › Keys row: label · recorder (or the fixed chord) · reset, with the verdict line under it.</summary>
internal static class KeyRow
{
    public static FrameworkElement Make(KeyBinding binding, SettingsModel model, Action redrawAll)
    {
        var column = new StackPanel { Spacing = 3 };
        var row = new Grid { ColumnSpacing = 8 };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        // 149 recheck fix R1: a long label ("Voice key — point and say (hold; double-tap locks)") wraps in its star column instead of clipping at 720×520.
        var label = new TextBlock { Text = binding.Label, VerticalAlignment = VerticalAlignment.Center, TextWrapping = TextWrapping.Wrap };
        Grid.SetColumn(label, 0);
        row.Children.Add(label);
        FrameworkElement control;
        if (binding.Fixed)
        {
            var t = new TextBlock
            {
                Text = binding.Chord?.Symbol ?? "",
                FontSize = 12,
                Width = 170,
                TextAlignment = TextAlignment.Center,
                VerticalAlignment = VerticalAlignment.Center,
                Foreground = Theme.Brush("TextFillColorSecondaryBrush"),
            };
            ToolTipService.SetToolTip(t, "Fixed: Esc cancels the recorder and closes every surface");
            AutomationProperties.SetAutomationId(t, $"settings-keys-{binding.Action.RawValue()}");
            control = t;
        }
        else
        {
            control = new ShortcutRecorder(binding.Action, model);
        }
        Grid.SetColumn(control, 1);
        row.Children.Add(control);
        var reset = new Button { Content = "\u21BA", Width = 36, Height = 30, Padding = new Thickness(0), IsEnabled = !binding.IsDefault && !binding.Fixed };
        ToolTipService.SetToolTip(reset, $"Reset to {binding.DefaultChord?.Symbol ?? "unassigned"}");
        AutomationProperties.SetAutomationId(reset, $"settings-keys-reset-{binding.Action.RawValue()}");
        AutomationProperties.SetName(reset, $"Reset {binding.Label}");
        reset.Click += (_, _) =>
        {
            model.Update(s => s.Keys.Reset(binding.Action));
            model.SetRowNotice(binding.Action, null);
            redrawAll();
        };
        Grid.SetColumn(reset, 2);
        row.Children.Add(reset);
        column.Children.Add(row);
        if (model.RowNotices.TryGetValue(binding.Action, out var notice))
        {
            var line = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
            line.Children.Add(new TextBlock
            {
                Text = notice.Text,
                FontSize = 12,
                VerticalAlignment = VerticalAlignment.Center,
                TextWrapping = TextWrapping.Wrap,
                MaxWidth = 340,
                Foreground = Theme.Brush("SystemFillColorCautionBrush", 0xC0),
            });
            AutomationProperties.SetAutomationId(line, $"settings-keys-notice-{binding.Action.RawValue()}");
            if (notice.MoveChord is { } chord)
            {
                var use = new Button { Content = "Use here instead", FontSize = 12, Padding = new Thickness(8, 2, 8, 2) };
                AutomationProperties.SetAutomationId(use, $"settings-keys-move-{binding.Action.RawValue()}");
                use.Click += (_, _) =>
                {
                    model.Update(s => s.Keys.Move(chord, binding.Action));
                    model.SetRowNotice(binding.Action, null);
                    redrawAll();
                };
                line.Children.Add(use);
            }
            column.Children.Add(line);
        }
        return column;
    }
}