using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Theuth.Companion.Core;
using Point = Windows.Foundation.Point;
using Size = Windows.Foundation.Size;

namespace Theuth.Companion.Ui;

/// <summary>
/// 149 WW2 glyphs (§3.1 dots, §3.5 / D8 level bars, §3.3 harness icons), 149 NATIVE look: the 12 epx agent dot drawn with the
/// system's theme brushes through the shared <see cref="NativeLook"/> table — accent disc = needs you, a small native
/// <c>ProgressRing</c> = working (a static secondary ring under Animation effects off), secondary ring = idle, Success disc with a
/// tick = done, Critical disc with a dash = error, Caution disc with pause bars = paused (lapsed), a hollow tertiary ring = remote
/// (E7 fallback). Shape carries the state with colour removed (130 §4.1).
/// </summary>
internal static class PillGlyphs
{
    public const double Dot = RailPillsModel.DotSize;

    public static FrameworkElement AgentDot(AgentDot d, ElementTheme theme, bool animate) =>
        d.Hollow ? Tone(StatusTone.Remote, theme, animate) : State(d.State, theme, animate);

    public static FrameworkElement RowDot(AgentRowState s, ElementTheme theme, bool animate) => Tone(NativeLook.Tone(s), theme, animate);

    public static FrameworkElement State(MarkState s, ElementTheme theme, bool animate) => Tone(NativeLook.Tone(s.Kind), theme, animate);

    /// <summary>The 12 epx mark for a tone of the shared table: shape by state, colour by the system's theme brushes.</summary>
    public static FrameworkElement Tone(StatusTone tone, ElementTheme theme, bool animate) => tone switch
    {
        StatusTone.Running => Spinner(theme, animate),
        StatusTone.Done => Done(theme),
        StatusTone.Failed => Error(theme),
        StatusTone.Paused => Paused(theme),
        StatusTone.Idle => Ring(NativeLook.TextSecondary, theme, thickness: 1.5),
        StatusTone.Remote => Ring(NativeLook.TextTertiary, theme, thickness: 1.5),
        _ => Disc(NativeLook.Accent, theme),
    };

    private static Ellipse Disc(string key, ElementTheme theme) => new() { Width = Dot, Height = Dot, Fill = Theme.Brush(key, theme) };

    private static Ellipse Ring(string key, ElementTheme theme, double thickness) =>
        new() { Width = Dot, Height = Dot, Stroke = Theme.Brush(key, theme), StrokeThickness = thickness };

    private static FrameworkElement Done(ElementTheme theme)
    {
        var g = new Grid { Width = Dot, Height = Dot };
        g.Children.Add(Disc(NativeLook.Success, theme));
        g.Children.Add(new Polyline
        {
            Points = { new Point(3.3, 6.2), new Point(5.2, 8.1), new Point(8.8, 4.2) },
            Stroke = Theme.Brush(NativeLook.OnAccent, theme), StrokeThickness = 1.6,
            StrokeStartLineCap = PenLineCap.Round, StrokeEndLineCap = PenLineCap.Round,
        });
        return g;
    }

    private static FrameworkElement Error(ElementTheme theme)
    {
        var g = new Grid { Width = Dot, Height = Dot };
        g.Children.Add(Disc(NativeLook.Critical, theme));
        g.Children.Add(new Rectangle { Width = 6, Height = 1.6, Fill = Theme.Brush(NativeLook.OnAccent, theme), HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center });
        return g;
    }

    private static FrameworkElement Paused(ElementTheme theme)
    {
        var g = new Grid { Width = Dot, Height = Dot };
        g.Children.Add(Disc(NativeLook.Caution, theme));
        var bars = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 1.5, HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center };
        bars.Children.Add(new Rectangle { Width = 1.6, Height = 5, Fill = Theme.Brush(NativeLook.OnAccent, theme) });
        bars.Children.Add(new Rectangle { Width = 1.6, Height = 5, Fill = Theme.Brush(NativeLook.OnAccent, theme) });
        g.Children.Add(bars);
        return g;
    }

    /// <summary>Working: the native indeterminate <see cref="ProgressRing"/> at dot size in secondary text (no status colour); a static secondary ring when animations are off (C13).</summary>
    public static FrameworkElement Spinner(ElementTheme theme, bool animate)
    {
        if (!animate) return Ring(NativeLook.TextSecondary, theme, thickness: 2);
        var ring = new ProgressRing
        {
            Width = Dot, Height = Dot, MinWidth = 0, MinHeight = 0, IsActive = true, IsIndeterminate = true,
            Foreground = Theme.Brush(NativeLook.TextSecondary, theme), Background = Theme.Transparent,
        };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(ring, "working");
        return ring;
    }

    /// <summary>D8: one accent bar per level value (recording = needs you), newest right, <paramref name="barWidth"/> × up to <paramref name="maxHeight"/> epx; ≥ 2 epx so silence still shows bars.</summary>
    public static StackPanel LevelBars(IReadOnlyList<float> levels, double maxHeight, double barWidth, ElementTheme theme, int maxBars = 8)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = Math.Max(1, barWidth / 2), VerticalAlignment = VerticalAlignment.Center, Height = maxHeight };
        var brush = Theme.Brush(NativeLook.Accent, theme);
        var take = levels.Count > maxBars ? levels.Skip(levels.Count - maxBars).ToList() : levels.ToList();
        if (take.Count == 0) take = new List<float> { 0, 0, 0 };
        foreach (var v in take)
        {
            double h = Math.Max(2, Math.Min(maxHeight, Math.Clamp(v, 0, 1) * maxHeight));
            row.Children.Add(new Rectangle { Width = barWidth, Height = h, RadiusX = barWidth / 2, RadiusY = barWidth / 2, Fill = brush, VerticalAlignment = VerticalAlignment.Center });
        }
        return row;
    }

    /// <summary>The harness icon (D18): the shared set's PNG (<see cref="HarnessIcon"/>, <c>Assets\harness\&lt;family&gt;-&lt;size&gt;.png</c> beside the exe); until the file exists, a 2-letter caption cap on a control fill.</summary>
    public static FrameworkElement Harness(string key, ElementTheme theme, double size = 18)
    {
        var family = HarnessIcon.Family(key);
        int px = size <= 16 ? 16 : size <= 20 ? 20 : size <= 24 ? 24 : 32;
        var path = HarnessIconPath(family, px);
        if (path is not null)
        {
            var img = new Image { Width = size, Height = size, Source = new Microsoft.UI.Xaml.Media.Imaging.BitmapImage(new Uri(path)), Stretch = Stretch.Uniform };
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(img, family);
            return img;
        }
        var cap = family switch { "omp" => "om", "opencode" => "oc", "claude" => "cl", "codex" => "cx", "theuth" => "th", _ => "?" };
        return new Border
        {
            Width = size, Height = size, CornerRadius = new CornerRadius(4), Background = Theme.Brush(NativeLook.ControlFill, theme),
            Child = new TextBlock
            {
                Text = cap, FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Foreground = Theme.Brush(NativeLook.TextSecondary, theme),
                HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center,
            },
        };
    }

    private static readonly Dictionary<string, string?> iconPaths = new(StringComparer.Ordinal);

    /// <summary><c>Assets\harness\&lt;family&gt;-&lt;px&gt;.png</c> beside the exe (WW1's csproj copies the shared set), else the 1× name, else <c>unknown</c>; null when absent.</summary>
    private static string? HarnessIconPath(string family, int px)
    {
        var key = $"{family}-{px}";
        lock (iconPaths)
        {
            if (iconPaths.TryGetValue(key, out var cached)) return cached;
            string? found = null;
            try
            {
                var dir = System.IO.Path.Combine(AppContext.BaseDirectory, HarnessIcon.Folder);
                foreach (var name in new[] { HarnessIcon.For(family, px), HarnessIcon.For(family), HarnessIcon.For(HarnessIcon.Unknown, px), HarnessIcon.For(HarnessIcon.Unknown) })
                {
                    var p = System.IO.Path.Combine(dir, name);
                    if (System.IO.File.Exists(p)) { found = p; break; }
                }
            }
            catch (Exception) { }
            iconPaths[key] = found;
            return found;
        }
    }

    /// <summary>The wordmark (the brand mark, not an icon stand-in): its shape stays; painted in the system accent with the inner square in text-on-accent — the shared table (the Mac paints it <c>Color.accentColor</c>).</summary>
    public static FrameworkElement Wordmark(ElementTheme theme)
    {
        var g = new Grid { Width = 12, Height = 12 };
        g.Children.Add(new Border { CornerRadius = new CornerRadius(3), Background = Theme.Brush(NativeLook.Accent, theme) });
        g.Children.Add(new Border { CornerRadius = new CornerRadius(1), Background = Theme.Brush(NativeLook.OnAccent, theme), Margin = new Thickness(3) });
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(g, "theuth");
        return g;
    }
}