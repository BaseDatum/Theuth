using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using Microsoft.UI.Xaml.Shapes;
using Theuth.Companion.Core;
using Windows.Foundation;

namespace Theuth.Companion.Ui;

/// <summary>
/// 149 WW2 glyphs (§3.1 dots, §3.5 / D8 level bars, §3.3 harness icons): the 12 epx agent dot — gold disc = needs you,
/// a faience arc turning every 0.38 s = working (the opacity beat under Reduce Motion), faience ring = idle, malachite with
/// a tick = done, carnelian = error, dashed sand = lapsed, a hollow sand ring = remote (E7 fallback). Shape carries the state
/// with colour removed (130 §4.1).
/// </summary>
internal static class PillGlyphs
{
    public const double Dot = RailPillsModel.DotSize;

    public static FrameworkElement AgentDot(AgentDot d, ElementTheme theme, bool animate) =>
        d.Hollow ? Ring(Palette.Sand, theme, dashed: false, thickness: 1.5) : State(d.State, theme, animate);

    public static FrameworkElement RowDot(AgentRowState s, ElementTheme theme, bool animate) => s switch
    {
        AgentRowState.NeedsYou => Disc(Palette.Gold, theme),
        AgentRowState.Working => Spinner(theme, animate),
        AgentRowState.Idle => Ring(Palette.Faience, theme, dashed: false, thickness: 1.5),
        AgentRowState.Done => Done(theme),
        AgentRowState.Error => Error(theme),
        AgentRowState.Lapsed => Ring(Palette.Sand, theme, dashed: true, thickness: 1.5),
        _ => Ring(Palette.Sand, theme, dashed: false, thickness: 1.5), // remote: hollow
    };

    public static FrameworkElement State(MarkState s, ElementTheme theme, bool animate) => s.Kind switch
    {
        MarkStateKind.NeedsYou => Disc(Palette.Gold, theme),
        MarkStateKind.Working => Spinner(theme, animate),
        MarkStateKind.Idle => Ring(Palette.Faience, theme, dashed: false, thickness: 1.5),
        MarkStateKind.Finished => Done(theme),
        MarkStateKind.Error => Error(theme),
        _ => Ring(Palette.Sand, theme, dashed: true, thickness: 1.5),
    };

    private static Ellipse Disc(string key, ElementTheme theme) => new() { Width = Dot, Height = Dot, Fill = Palette.Brush(key, theme) };

    private static Ellipse Ring(string key, ElementTheme theme, bool dashed, double thickness)
    {
        var e = new Ellipse { Width = Dot, Height = Dot, Stroke = Palette.Brush(key, theme), StrokeThickness = thickness };
        if (dashed) e.StrokeDashArray = new DoubleCollection { 2 / thickness, 1.5 / thickness };
        return e;
    }

    private static FrameworkElement Done(ElementTheme theme)
    {
        var g = new Grid { Width = Dot, Height = Dot };
        g.Children.Add(Disc(Palette.Malachite, theme));
        g.Children.Add(new Polyline
        {
            Points = { new Point(3.3, 6.2), new Point(5.2, 8.1), new Point(8.8, 4.2) },
            Stroke = Palette.Brush(Palette.Surface, theme), StrokeThickness = 1.6,
            StrokeStartLineCap = PenLineCap.Round, StrokeEndLineCap = PenLineCap.Round,
        });
        return g;
    }

    private static FrameworkElement Error(ElementTheme theme)
    {
        var g = new Grid { Width = Dot, Height = Dot };
        g.Children.Add(Disc(Palette.Carnelian, theme));
        g.Children.Add(new Rectangle { Width = 6, Height = 1.6, Fill = Palette.Brush(Palette.Surface, theme), HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center });
        return g;
    }

    /// <summary>The working arc: 270° of a 2.5 epx faience ring, one turn per 0.38 s; a beating disc when animations are off (C13).</summary>
    public static FrameworkElement Spinner(ElementTheme theme, bool animate)
    {
        const double t = 2.5, r = (Dot - t) / 2, c = Dot / 2;
        double a0 = -90 * Math.PI / 180, a1 = 180 * Math.PI / 180;
        var figure = new PathFigure { StartPoint = new Point(c + r * Math.Cos(a0), c + r * Math.Sin(a0)), IsClosed = false, IsFilled = false };
        figure.Segments.Add(new ArcSegment { Point = new Point(c + r * Math.Cos(a1), c + r * Math.Sin(a1)), Size = new Size(r, r), IsLargeArc = true, SweepDirection = SweepDirection.Clockwise });
        var geometry = new PathGeometry();
        geometry.Figures.Add(figure);
        var path = new Microsoft.UI.Xaml.Shapes.Path
        {
            Width = Dot, Height = Dot, Data = geometry, Stroke = Palette.Brush(Palette.Faience, theme), StrokeThickness = t,
            StrokeStartLineCap = PenLineCap.Round, StrokeEndLineCap = PenLineCap.Round,
            RenderTransformOrigin = new Point(0.5, 0.5),
        };
        if (animate)
        {
            var rot = new RotateTransform();
            path.RenderTransform = rot;
            var a = new DoubleAnimation { From = 0, To = 360, Duration = new Duration(TimeSpan.FromSeconds(RailPillsModel.SpinnerSeconds)), RepeatBehavior = RepeatBehavior.Forever };
            Storyboard.SetTarget(a, rot);
            Storyboard.SetTargetProperty(a, "Angle");
            var sb = new Storyboard();
            sb.Children.Add(a);
            path.Loaded += (_, _) => sb.Begin();
            return path;
        }
        // Reduce Motion: the W2 opacity beat on a full disc.
        var disc = Disc(Palette.Faience, theme);
        var beat = new DoubleAnimation { From = 1, To = 0.55, Duration = new Duration(TimeSpan.FromMilliseconds(380)), AutoReverse = true, RepeatBehavior = RepeatBehavior.Forever };
        Storyboard.SetTarget(beat, disc);
        Storyboard.SetTargetProperty(beat, "Opacity");
        var sb2 = new Storyboard();
        sb2.Children.Add(beat);
        disc.Loaded += (_, _) => sb2.Begin();
        return disc;
    }

    /// <summary>D8: one carnelian bar per level value, newest right, <paramref name="barWidth"/> × up to <paramref name="maxHeight"/> epx; ≥ 2 epx so silence still shows bars.</summary>
    public static StackPanel LevelBars(IReadOnlyList<float> levels, double maxHeight, double barWidth, ElementTheme theme, int maxBars = 8)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = Math.Max(1, barWidth / 2), VerticalAlignment = VerticalAlignment.Center, Height = maxHeight };
        var brush = Palette.Brush(Palette.Carnelian, theme);
        var take = levels.Count > maxBars ? levels.Skip(levels.Count - maxBars).ToList() : levels.ToList();
        if (take.Count == 0) take = new List<float> { 0, 0, 0 };
        foreach (var v in take)
        {
            double h = Math.Max(2, Math.Min(maxHeight, Math.Clamp(v, 0, 1) * maxHeight));
            row.Children.Add(new Rectangle { Width = barWidth, Height = h, RadiusX = barWidth / 2, RadiusY = barWidth / 2, Fill = brush, VerticalAlignment = VerticalAlignment.Center });
        }
        return row;
    }

    /// <summary>The harness icon (D18): <c>Assets/harness/&lt;key&gt;.png</c> from the shared set once H1 lands; until the file exists, a 2-letter cap tinted ink.</summary>
    public static FrameworkElement Harness(string key, ElementTheme theme, double size = 18)
    {
        var k = key.Length == 0 ? "theuth" : key;
        var path = HarnessIconPath(k);
        if (path is not null)
        {
            return new Image { Width = size, Height = size, Source = new Microsoft.UI.Xaml.Media.Imaging.BitmapImage(new Uri(path)), Stretch = Stretch.Uniform };
        }
        var cap = k switch { "omp" => "om", "opencode" => "oc", "claude" => "cl", "codex" => "cx", "theuth" => "th", _ => k.Length >= 2 ? k[..2] : k };
        return new Border
        {
            Width = size, Height = size, CornerRadius = new CornerRadius(5), Background = Palette.Tint(Palette.Ink, theme, 0.13),
            Child = new TextBlock
            {
                Text = cap, FontSize = Math.Round(size * 0.5), FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Foreground = Palette.Brush(Palette.Ink2, theme),
                HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center,
            },
        };
    }

    private static readonly Dictionary<string, string?> iconPaths = new(StringComparer.Ordinal);

    /// <summary>The shared icon set copied beside the exe as <c>Assets/harness/*.png</c> (the 24 px size); null when absent.</summary>
    private static string? HarnessIconPath(string key)
    {
        lock (iconPaths)
        {
            if (iconPaths.TryGetValue(key, out var cached)) return cached;
            string? found = null;
            try
            {
                var dir = Path.Combine(AppContext.BaseDirectory, "Assets", "harness");
                foreach (var name in new[] { $"{key}-24.png", $"{key}@24.png", $"{key}_24.png", $"{key}.png" })
                {
                    var p = Path.Combine(dir, name);
                    if (File.Exists(p)) { found = p; break; }
                }
                if (found is null && key != "unknown")
                {
                    foreach (var name in new[] { "unknown-24.png", "unknown.png" })
                    {
                        var p = Path.Combine(dir, name);
                        if (File.Exists(p)) { found = p; break; }
                    }
                }
            }
            catch (Exception) { }
            iconPaths[key] = found;
            return found;
        }
    }
}
