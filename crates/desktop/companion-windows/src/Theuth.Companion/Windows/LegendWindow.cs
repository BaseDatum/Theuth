using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Windows.Graphics;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.WindowsAndMessaging;

namespace Theuth.Companion.Ui;

/// <summary>
/// "Keyboard shortcuts" (130 W6 item 4b, design 134 §4.2/§4.5): the key table by scope, rendered FROM the table so it is
/// always true — every row's chord with its fixed aliases, "unassigned" rows, the hold-to-talk line, the hotkey notice.
/// Opened from the tray menu (Ctrl+/ shown there) or Ctrl+/ · ? in the panel; activated so Esc reaches it, and Esc hands
/// the foreground back. UIA: <c>legend</c>, rows <c>legend-&lt;actionId&gt;</c>, the button <c>legend-settings</c>.
/// </summary>
internal sealed partial class LegendWindow : ToolWindow
{
    public const double LegendWidth = 360;
    private readonly Store store;
    private readonly UserControl host;
    private readonly StackPanel content;
    private HWND previous;
    private RECT? trayAnchor;

    public LegendWindow(Store store)
        : base("theuth — Keyboard shortcuts", default, excludeFromCapture: !Flags.Shareable)
    {
        this.store = store;
        content = new StackPanel();
        host = new UserControl { Content = new ScrollViewer { Content = content, VerticalScrollBarVisibility = ScrollBarVisibility.Auto }, IsTabStop = true, UseSystemFocusVisuals = false };
        AutomationProperties.SetAutomationId(host, "legend");
        AutomationProperties.SetName(host, "Keyboard shortcuts");
        host.PreviewKeyDown += OnKey;
        Content = host;
        // 149 NATIVE: a transient over the desktop — Mica Base (Acrylic Base where unsupported) like the panels, DWM round corners,
        // nothing painted over the material; follows Windows light/dark live.
        if (!Shape.RoundCorners(Hwnd)) Log.Write("legend: DWM rounded corners unavailable (rectangular)");
        ApplyEffects();
        if (SystemEffects.Current is { } fx) fx.Changed += () => { ApplyEffects(); if (IsOpen) Render(); };
        Theme.Follow(host, () => { backdrop?.SetTheme(host.ActualTheme); if (IsOpen) Render(); });
        Activated += (_, e) =>
        {
            if (e.WindowActivationState != WindowActivationState.Deactivated && IsOpen) host.Focus(FocusState.Programmatic);
        };
    }

    private Backdrop? backdrop;

    public string BackdropText => backdrop?.Material ?? "solid";

    private void ApplyEffects()
    {
        bool transparency = SystemEffects.TransparencyOn;
        if (transparency && backdrop is null) backdrop = Backdrop.Attach(this, BackdropKind.MicaBase, Theme.ColorOf(NativeLook.SolidBackgroundBase, host.ActualTheme), host.ActualTheme, "legend");
        else if (!transparency && backdrop is not null) { backdrop.Dispose(); backdrop = null; }
    }

    public bool IsOpen { get; private set; }

    /// <summary><paramref name="tray"/>: the tray icon's rect (above it), else top-centre of the display under the mouse.</summary>
    public void Open(RECT? tray, HWND prev, bool activate = true)
    {
        previous = prev;
        trayAnchor = tray;
        IsOpen = true;
        Render();
        if (!activate)
        {
            ShowPassive();
            return;
        }
        ShowActive();
        host.Focus(FocusState.Programmatic);
        DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, Place);
        Log.Write($"legend open prev={Win32.Describe(prev)} foreground={Win32.Describe(PInvoke.GetForegroundWindow())}");
    }

    public void Toggle(RECT? tray, HWND prev)
    {
        if (IsOpen) CloseLegend(returnFocus: true);
        else Open(tray, prev);
    }

    public void CloseLegend(bool returnFocus)
    {
        if (!IsOpen) return;
        IsOpen = false;
        bool wasForeground = IsForeground;
        HideWindow();
        if (returnFocus && wasForeground) Win32.Foreground(previous);
        previous = default;
    }

    protected override void OnActivate(bool active, HWND other)
    {
        if (!active && IsOpen) CloseLegend(returnFocus: false);
    }

    protected override void OnCloseRequested() => CloseLegend(returnFocus: true);

    private void OnKey(object sender, KeyRoutedEventArgs e)
    {
        var press = KeyInput.From(e);
        var intent = KeyGrammar.IntentFor(press, KeyScope.Panel, store.Keys);
        if (press.KeyCode == VirtualKeys.Escape || intent.Kind == IntentKind.Legend)
        {
            e.Handled = true;
            CloseLegend(returnFocus: true);
        }
    }

    private void Render()
    {
        var theme = host.ActualTheme;
        backdrop?.SetTheme(theme);
        host.Background = backdrop is null ? Theme.Brush(NativeLook.SolidBackgroundBase, theme) : Theme.Transparent;
        host.Foreground = Theme.Brush(NativeLook.TextPrimary, theme);
        content.Children.Clear();
        var head = new Grid { Padding = new Thickness(14, 12, 14, 8) };
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        head.Children.Add(new TextBlock { Text = "Keyboard shortcuts", FontSize = 14, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        var esc = new TextBlock { Text = "Esc close", FontSize = 12, Foreground = Theme.Brush(NativeLook.TextTertiary, theme), VerticalAlignment = VerticalAlignment.Center };
        Grid.SetColumn(esc, 1);
        head.Children.Add(esc);
        content.Children.Add(head);
        var table = store.Keys;
        foreach (var scope in Enum.GetValues<KeyScope>())
        {
            content.Children.Add(new TextBlock
            {
                Text = scope.Title().ToUpperInvariant(), FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                Foreground = Theme.Brush(NativeLook.TextTertiary, theme), CharacterSpacing = 60, Margin = new Thickness(14, 7, 14, 2),
            });
            foreach (var b in table.BindingsIn(scope)) content.Children.Add(Row(b.Label, ChordText(b), b.Chord is null && b.DefaultChord is not null, $"legend-{b.Action.RawValue()}", theme));
            if (scope == KeyScope.AskBar && table.ChordFor(KeyAction.OpenPanel) is { } hold)
            {
                content.Children.Add(Row("Hold to talk · release to send in 3 s", $"hold {hold.Symbol}", false, "legend-holdToTalk", theme));
            }
        }
        content.Children.Add(new Border { Height = 1, Margin = new Thickness(14, 8, 14, 0), Background = Theme.Brush(NativeLook.Divider, theme) });
        var foot = new StackPanel { Spacing = 4, Padding = new Thickness(14, 10, 14, 12) };
        if (store.HotkeyNotice is { } n)
        {
            var notice = new TextBlock { Text = n, FontSize = 12, Foreground = Theme.Brush(NativeLook.Critical, theme), TextWrapping = TextWrapping.Wrap };
            AutomationProperties.SetAutomationId(notice, "legend-notice");
            foot.Children.Add(notice);
        }
        foot.Children.Add(new TextBlock
        {
            Text = "Panel and Ask bar keys fire only while that surface is active, so they never collide with other apps — which is why the global set stays tiny.",
            FontSize = 12, Foreground = Theme.Brush(NativeLook.TextTertiary, theme), TextWrapping = TextWrapping.Wrap,
        });
        var settings = new HyperlinkButton { Content = "Change in Settings…  Ctrl+,", FontSize = 12, Padding = new Thickness(0, 2, 0, 0) };
        AutomationProperties.SetAutomationId(settings, "legend-settings");
        settings.Click += (_, _) =>
        {
            CloseLegend(returnFocus: false);
            store.Perform(Intent.Settings);
        };
        foot.Children.Add(settings);
        content.Children.Add(foot);
        Place();
    }

    private static string ChordText(KeyBinding b)
    {
        if (b.Chord is not { } c) return "unassigned";
        var aliases = KeyTable.Aliases.Where(a => a.Action == b.Action).Select(a => a.Chord.Symbol);
        return string.Join(" · ", new[] { c.Symbol }.Concat(aliases));
    }

    private static FrameworkElement Row(string label, string chord, bool lost, string id, ElementTheme theme)
    {
        var g = new Grid { Padding = new Thickness(14, 1.5, 14, 1.5) };
        g.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        g.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        g.Children.Add(new TextBlock { Text = label, FontSize = 12, TextTrimming = TextTrimming.CharacterEllipsis });
        var key = lost ? NativeLook.Caution : chord == "unassigned" ? NativeLook.TextTertiary : NativeLook.TextSecondary;
        var c = new TextBlock { Text = chord, FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Foreground = Theme.Brush(key, theme), Margin = new Thickness(8, 0, 0, 0) };
        Grid.SetColumn(c, 1);
        g.Children.Add(c);
        AutomationProperties.SetAutomationId(g, id);
        AutomationProperties.SetName(g, $"{label}: {chord}");
        return g;
    }

    private void Place()
    {
        if (!IsOpen) return;
        double s = Scale;
        content.Measure(new global::Windows.Foundation.Size(LegendWidth, double.PositiveInfinity));
        double hEpx = Math.Clamp(content.DesiredSize.Height <= 0 ? 420 : content.DesiredSize.Height, 200, 760);
        int w = (int)Math.Round(LegendWidth * s), h = (int)Math.Round(hEpx * s), gap = (int)Math.Round(8 * s);
        PointInt32 probe;
        int x, y;
        if (trayAnchor is { } t)
        {
            probe = new PointInt32((t.left + t.right) / 2, (t.top + t.bottom) / 2);
            x = probe.X - w / 2;
            y = t.top - h - (int)Math.Round(4 * s);
        }
        else
        {
            var c = Win32.Cursor();
            probe = new PointInt32(c.X, c.Y);
            var wa0 = DisplayArea.GetFromPoint(probe, DisplayAreaFallback.Primary).WorkArea;
            x = wa0.X + (wa0.Width - w) / 2;
            y = wa0.Y + (int)Math.Round(wa0.Height * 0.2);
        }
        var wa = DisplayArea.GetFromPoint(probe, DisplayAreaFallback.Primary).WorkArea;
        x = Math.Clamp(x, wa.X + gap, Math.Max(wa.X + gap, wa.X + wa.Width - w - gap));
        y = Math.Clamp(y, wa.Y + gap, Math.Max(wa.Y + gap, wa.Y + wa.Height - h - gap));
        Place(new RectInt32(x, y, w, h));
    }
}