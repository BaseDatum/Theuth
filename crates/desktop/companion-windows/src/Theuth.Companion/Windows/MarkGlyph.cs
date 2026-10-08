using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using Microsoft.UI.Xaml.Shapes;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;
using Windows.Foundation;

namespace Theuth.Companion.Ui;

/// <summary>
/// The rail's glyphs (130 §4.1 table, Swift <c>MarkView</c>): colour AND shape carry the state (C5) — a gold ring notched at
/// the kind's clock position (ask/decision 12 · approval 3 · review 6 · other 9), a faience disc that beats 380 ms while
/// working (static when animations are off, C13), a faience ring for idle, malachite with a tick for finished, carnelian
/// with a bar for error, a dashed sand ring for lapsed. 10 × 10 epx.
/// </summary>
internal static class MarkGlyph
{
    private const double S = 10;

    /// <summary>Windows' "Animation effects" switch (<c>UISettings.AnimationsEnabled</c>), live through <see cref="SystemEffects"/> (C13).</summary>
    public static bool AnimationsEnabled => SystemEffects.AnimationsOn;

    public static FrameworkElement Create(MarkState state, ElementTheme theme, bool animate)
    {
        switch (state.Kind)
        {
            case MarkStateKind.NeedsYou:
                return Notched(state.Item ?? AttentionKind.Other(""), theme);
            case MarkStateKind.Working:
            {
                var disc = new Ellipse { Width = S, Height = S, Fill = Palette.Brush(Palette.Faience, theme) };
                if (animate) Beat(disc);
                return disc;
            }
            case MarkStateKind.Idle:
                return new Ellipse { Width = S, Height = S, Stroke = Palette.Brush(Palette.Faience, theme), StrokeThickness = 1.5 };
            case MarkStateKind.Finished:
            {
                var g = Square();
                g.Children.Add(new Ellipse { Width = S, Height = S, Fill = Palette.Brush(Palette.Malachite, theme) });
                g.Children.Add(new Polyline
                {
                    Points = { new Point(2.8, 5.2), new Point(4.4, 6.8), new Point(7.4, 3.6) },
                    Stroke = Palette.Brush(Palette.Surface, theme),
                    StrokeThickness = 1.4,
                    StrokeStartLineCap = PenLineCap.Round,
                    StrokeEndLineCap = PenLineCap.Round,
                });
                return g;
            }
            case MarkStateKind.Error:
            {
                var g = Square();
                g.Children.Add(new Ellipse { Width = S, Height = S, Fill = Palette.Brush(Palette.Carnelian, theme) });
                g.Children.Add(new Rectangle
                {
                    Width = 5, Height = 1.5, Fill = Palette.Brush(Palette.Surface, theme),
                    HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center,
                });
                return g;
            }
            default: // lapsed
                return new Ellipse
                {
                    Width = S, Height = S, Stroke = Palette.Brush(Palette.Sand, theme), StrokeThickness = 1.5,
                    StrokeDashArray = new DoubleCollection { 2 / 1.5, 1.5 / 1.5 },
                };
        }
    }

    /// <summary>The glyph a card or line shows for an item (finished → finished, needs-you → its ring, receipts → idle).</summary>
    public static FrameworkElement ForItem(AttentionItem item, ElementTheme theme) =>
        Create(item.Kind == AttentionKind.Finished ? MarkState.Finished : item.Kind.NeedsYou ? MarkState.NeedsYou(item.Kind) : MarkState.Idle, theme, false);

    private static Grid Square() => new() { Width = S, Height = S };

    /// <summary>The needs-you ring: a 3 epx gold arc with a 12 % gap centred on the kind's clock position.</summary>
    private static FrameworkElement Notched(AttentionKind kind, ElementTheme theme)
    {
        double clock = kind.Tag switch
        {
            AttentionKindTag.Ask or AttentionKindTag.Decision => -90,
            AttentionKindTag.Approval => 0,
            AttentionKindTag.Review => 90,
            _ => 180,
        };
        const double r = 3.5, c = 5, gap = 0.12 * 360;
        double a0 = (clock + gap / 2) * Math.PI / 180, a1 = (clock + 360 - gap / 2) * Math.PI / 180;
        var figure = new PathFigure { StartPoint = new Point(c + r * Math.Cos(a0), c + r * Math.Sin(a0)), IsClosed = false, IsFilled = false };
        figure.Segments.Add(new ArcSegment
        {
            Point = new Point(c + r * Math.Cos(a1), c + r * Math.Sin(a1)),
            Size = new Size(r, r),
            IsLargeArc = true,
            SweepDirection = SweepDirection.Clockwise,
        });
        var geometry = new PathGeometry();
        geometry.Figures.Add(figure);
        return new Microsoft.UI.Xaml.Shapes.Path
        {
            Width = S, Height = S, Data = geometry, Stroke = Palette.Brush(Palette.Gold, theme), StrokeThickness = 3,
            StrokeStartLineCap = PenLineCap.Flat, StrokeEndLineCap = PenLineCap.Flat,
        };
    }

    /// <summary>The working beat: opacity 1 ↔ 0.55, 380 ms ease-in-out, forever.</summary>
    private static void Beat(UIElement e)
    {
        var a = new DoubleAnimation
        {
            From = 1, To = 0.55, Duration = new Duration(TimeSpan.FromMilliseconds(380)), AutoReverse = true,
            RepeatBehavior = RepeatBehavior.Forever, EasingFunction = new SineEase { EasingMode = EasingMode.EaseInOut },
        };
        Storyboard.SetTarget(a, e);
        Storyboard.SetTargetProperty(a, "Opacity");
        var sb = new Storyboard();
        sb.Children.Add(a);
        if (e is FrameworkElement fe) fe.Loaded += (_, _) => sb.Begin();
        else sb.Begin();
    }

    /// <summary>The wordmark: the only gold chrome besides the count and the needs-you marks (C7).</summary>
    public static FrameworkElement Wordmark(ElementTheme theme)
    {
        var g = new Grid { Width = 12, Height = 12 };
        g.Children.Add(new Border { CornerRadius = new CornerRadius(3), Background = Palette.Brush(Palette.Gold, theme) });
        g.Children.Add(new Border { CornerRadius = new CornerRadius(1), Background = Palette.Brush(Palette.Surface, theme), Margin = new Thickness(3) });
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(g, "theuth");
        return g;
    }
}
