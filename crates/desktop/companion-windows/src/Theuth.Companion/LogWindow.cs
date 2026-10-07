using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Windows.Graphics;

namespace Theuth.Companion;

/// <summary>
/// W1's only window: the companion log in a read-only TextBox whose UIA id is <c>log</c> (design 134 §7 W1) — what the
/// <c>windows-build-b</c> harness reads with <c>ui_read</c>. The real surfaces (rail, panel, Ask bar) are W2–W4's.
/// </summary>
internal sealed partial class LogWindow : Window
{
    private readonly TextBox box;

    public LogWindow()
    {
        Title = "theuth companion — log";
        box = new TextBox
        {
            IsReadOnly = true,
            AcceptsReturn = true,
            TextWrapping = TextWrapping.Wrap,
            FontFamily = new FontFamily("Consolas"),
            FontSize = 12,
            IsSpellCheckEnabled = false,
            HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Stretch,
        };
        AutomationProperties.SetAutomationId(box, "log");
        AutomationProperties.SetName(box, "Companion log");
        ScrollViewer.SetVerticalScrollBarVisibility(box, ScrollBarVisibility.Auto);
        Content = box;
        Log.Attach(box);
        Place(widthPx: 900, heightPx: 560);
    }

    /// <summary>Shown without taking the foreground (<c>AppWindow.Show(false)</c>, spike S1).</summary>
    public void ShowWithoutActivating() => AppWindow.Show(false);

    /// <summary>Centred on the primary display's work area, in physical pixels (<c>MoveAndResize</c> is physical, A6).</summary>
    private void Place(int widthPx, int heightPx)
    {
        var wa = DisplayArea.Primary.WorkArea;
        int w = Math.Min(widthPx, wa.Width), h = Math.Min(heightPx, wa.Height);
        AppWindow.MoveAndResize(new RectInt32(wa.X + (wa.Width - w) / 2, wa.Y + (wa.Height - h) / 2, w, h));
    }
}
