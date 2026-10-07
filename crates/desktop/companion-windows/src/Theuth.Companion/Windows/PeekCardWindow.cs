using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Windows.Graphics;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.WindowsAndMessaging;

namespace Theuth.Companion.Ui;

/// <summary>
/// The peek card (design 134 §4.1, 130 §4.1): what an agent mark says in full — name, state, model, open tasks — beside the
/// rail at the mark's height, with Open session when the mark has one. Non-focusable (<c>WS_EX_NOACTIVATE</c>, shown with
/// <c>AppWindow.Show(false)</c>); a press on it activates the island like the rail's, so it hands the foreground straight back
/// the same way. Closes on another click outside it, on a second click of its mark, or 8 s after the pointer left.
/// UIA: <c>peek-card</c>, named "&lt;agent&gt;, &lt;state&gt;"; the button <c>peek-open</c>.
/// </summary>
internal sealed partial class PeekCardWindow : ToolWindow
{
    private const double CardWidth = 240;
    private readonly Store store;
    private readonly Func<HWND> handBackTo;
    private readonly Border root;
    private DateTimeOffset lastInside;
    private bool buttonWasDown;

    public PeekCardWindow(Store store, Func<HWND> handBackTo)
        : base("theuth peek", WINDOW_EX_STYLE.WS_EX_NOACTIVATE, excludeFromCapture: !Flags.Shareable)
    {
        this.store = store;
        this.handBackTo = handBackTo;
        root = new Border { CornerRadius = new CornerRadius(10), BorderThickness = new Thickness(1), Padding = new Thickness(12, 10, 12, 10) };
        AutomationProperties.SetAutomationId(root, "peek-card");
        Content = root;
    }

    public Mark? Showing { get; private set; }

    public void Show(Mark m, RECT markRow, RECT rail, string side)
    {
        Showing = m;
        var theme = root.ActualTheme;
        root.Background = Palette.Brush(Palette.Surface, theme);
        root.BorderBrush = Palette.Brush(Palette.Rule, theme);
        var stack = new StackPanel { Spacing = 4 };
        var head = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 7 };
        var glyph = MarkGlyph.Create(m.State, theme, false);
        glyph.VerticalAlignment = VerticalAlignment.Center;
        head.Children.Add(glyph);
        head.Children.Add(new TextBlock { Text = m.Label, FontSize = 12.5, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Foreground = Palette.Brush(Palette.Ink, theme) });
        stack.Children.Add(head);
        var parts = m.Detail.Split(", ", 2);
        string state = (parts.Length > 1 ? parts[1] : m.State.Kind.ToString().ToLowerInvariant()).TrimEnd(',', ' ');
        stack.Children.Add(new TextBlock { Text = state, FontSize = 11.5, Foreground = Palette.Brush(Palette.Ink2, theme), TextWrapping = TextWrapping.Wrap });
        if (store.Agents.FirstOrDefault(a => $"agent:{a.File.Name}" == m.Id) is { } agent)
        {
            var line = $"{agent.File.Title} · {agent.OpenTasks} open task{(agent.OpenTasks == 1 ? "" : "s")}";
            stack.Children.Add(new TextBlock { Text = line, FontSize = 11, Foreground = Palette.Brush(Palette.Sand, theme), TextWrapping = TextWrapping.Wrap });
        }
        if (m.Session is { } session)
        {
            var open = new Button { Content = "Open session", FontSize = 11.5, Margin = new Thickness(0, 4, 0, 0), Padding = new Thickness(8, 3, 8, 3) };
            AutomationProperties.SetAutomationId(open, "peek-open");
            AutomationProperties.SetName(open, "Open session");
            open.Click += (_, _) =>
            {
                HideCard();
                _ = Links.Open(DeepLink.Session(session));
            };
            stack.Children.Add(open);
        }
        root.Child = stack;
        AutomationProperties.SetName(root, $"{m.Label}, {state}");

        double s = Scale;
        root.Measure(new global::Windows.Foundation.Size(CardWidth, double.PositiveInfinity));
        int w = (int)Math.Round(CardWidth * s);
        int h = (int)Math.Round((Math.Max(48, root.DesiredSize.Height) + 6) * s);
        var wa = DisplayArea.GetFromPoint(new PointInt32((markRow.left + markRow.right) / 2, (markRow.top + markRow.bottom) / 2), DisplayAreaFallback.Primary).WorkArea;
        int gap = (int)Math.Round(8 * s);
        int x = side == "left" ? rail.right + gap : rail.left - gap - w;
        int y = (markRow.top + markRow.bottom) / 2 - h / 2;
        x = Math.Clamp(x, wa.X + gap, Math.Max(wa.X + gap, wa.X + wa.Width - w - gap));
        y = Math.Clamp(y, wa.Y + gap, Math.Max(wa.Y + gap, wa.Y + wa.Height - h - gap));
        Place(new RectInt32(x, y, w, h));
        lastInside = DateTimeOffset.Now;
        buttonWasDown = true; // the click that opened it is still down
        ShowPassive();
        // The first measure runs before the button's template exists: size again once the tree is live.
        DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, () =>
        {
            if (Showing != m) return;
            root.Measure(new global::Windows.Foundation.Size(CardWidth, double.PositiveInfinity));
            int h2 = (int)Math.Round((Math.Max(48, root.DesiredSize.Height) + 6) * s);
            if (h2 > h) Place(new RectInt32(x, Math.Clamp(y - (h2 - h) / 2, wa.Y + gap, Math.Max(wa.Y + gap, wa.Y + wa.Height - h2 - gap)), w, h2));
        });
        Log.Write($"peek card: {m.Label} ({state}) at {Win32.Text(ScreenRect)}; foreground {Win32.Describe(PInvoke.GetForegroundWindow())}");
    }

    public void HideCard()
    {
        if (Showing is null) return;
        Showing = null;
        HideWindow();
    }

    /// <summary>The rail's 40 ms poll: close on a press outside the rail and the card, or 8 s after the pointer left both.</summary>
    public void Poll(RECT rail)
    {
        if (Showing is null) return;
        var c = Win32.Cursor();
        bool inside = Win32.Contains(Win32.Inflate(rail, 2), c) || Win32.Contains(ScreenRect, c);
        if (inside) lastInside = DateTimeOffset.Now;
        bool down = (PInvoke.GetAsyncKeyState(0x01) & 0x8000) != 0; // VK_LBUTTON
        if (down && !buttonWasDown && !inside)
        {
            HideCard();
            return;
        }
        buttonWasDown = down;
        if (DateTimeOffset.Now - lastInside > TimeSpan.FromSeconds(8)) HideCard();
    }

    protected override void OnActivate(bool active, HWND other)
    {
        if (!active) return;
        var back = handBackTo();
        bool ok = Win32.Foreground(back);
        Log.Write($"peek card: activated by a press — hand-back to {Win32.Describe(back)} -> {(ok ? "true" : "false")}");
    }

    protected override void OnCloseRequested() => HideCard();
}
