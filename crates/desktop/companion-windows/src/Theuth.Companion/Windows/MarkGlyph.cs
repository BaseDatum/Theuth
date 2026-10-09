using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;
using Windows.Foundation;

namespace Theuth.Companion.Ui;

/// <summary>
/// The strip rail's glyphs (130 §4.1 table, Swift <c>MarkView</c>), 149 NATIVE look: colour AND shape carry the state (C5), the
/// colour being the system's (<see cref="NativeLook"/>) — an accent ring notched at the kind's clock position (ask/decision 12 ·
/// approval 3 · review 6 · other 9) for needs-you, a small native <c>ProgressRing</c> while working (a static secondary ring when
/// animations are off, C13), a secondary ring for idle, Success with a tick for finished, Critical with a bar for error, Caution
/// with pause bars for paused (lapsed). 10 × 10 epx.
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
                if (!animate) return new Ellipse { Width = S, Height = S, Stroke = Theme.Brush(NativeLook.TextSecondary, theme), StrokeThickness = 2 };
                var ring = new ProgressRing
                {
                    Width = S, Height = S, MinWidth = 0, MinHeight = 0, IsActive = true, IsIndeterminate = true,
                    Foreground = Theme.Brush(NativeLook.TextSecondary, theme), Background = Theme.Transparent,
                };
                Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(ring, "working");
                return ring;
            }
            case MarkStateKind.Idle:
                return new Ellipse { Width = S, Height = S, Stroke = Theme.Brush(NativeLook.TextSecondary, theme), StrokeThickness = 1.5 };
            case MarkStateKind.Finished:
            {
                var g = Square();
                g.Children.Add(new Ellipse { Width = S, Height = S, Fill = Theme.Brush(NativeLook.Success, theme) });
                g.Children.Add(new Polyline
                {
                    Points = { new Point(2.8, 5.2), new Point(4.4, 6.8), new Point(7.4, 3.6) },
                    Stroke = Theme.Brush(NativeLook.OnAccent, theme),
                    StrokeThickness = 1.4,
                    StrokeStartLineCap = PenLineCap.Round,
                    StrokeEndLineCap = PenLineCap.Round,
                });
                return g;
            }
            case MarkStateKind.Error:
            {
                var g = Square();
                g.Children.Add(new Ellipse { Width = S, Height = S, Fill = Theme.Brush(NativeLook.Critical, theme) });
                g.Children.Add(new Rectangle
                {
                    Width = 5, Height = 1.5, Fill = Theme.Brush(NativeLook.OnAccent, theme),
                    HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center,
                });
                return g;
            }
            default: // lapsed = paused
            {
                var g = Square();
                g.Children.Add(new Ellipse { Width = S, Height = S, Fill = Theme.Brush(NativeLook.Caution, theme) });
                var bars = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 1.2, HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center };
                bars.Children.Add(new Rectangle { Width = 1.4, Height = 4, Fill = Theme.Brush(NativeLook.OnAccent, theme) });
                bars.Children.Add(new Rectangle { Width = 1.4, Height = 4, Fill = Theme.Brush(NativeLook.OnAccent, theme) });
                g.Children.Add(bars);
                return g;
            }
        }
    }

    /// <summary>The glyph a card or line shows for an item (finished → finished, needs-you → its ring, receipts → idle).</summary>
    public static FrameworkElement ForItem(AttentionItem item, ElementTheme theme) =>
        Create(item.Kind == AttentionKind.Finished ? MarkState.Finished : item.Kind.NeedsYou ? MarkState.NeedsYou(item.Kind) : MarkState.Idle, theme, false);

    private static Grid Square() => new() { Width = S, Height = S };

    /// <summary>The needs-you ring: a 3 epx accent arc with a 12 % gap centred on the kind's clock position.</summary>
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
            Width = S, Height = S, Data = geometry, Stroke = Theme.Brush(NativeLook.Accent, theme), StrokeThickness = 3,
            StrokeStartLineCap = PenLineCap.Flat, StrokeEndLineCap = PenLineCap.Flat,
        };
    }

    /// <summary>The wordmark (the brand mark, not an icon stand-in): its shape stays, painted in the system accent (PillGlyphs.Wordmark).</summary>
    public static FrameworkElement Wordmark(ElementTheme theme) => PillGlyphs.Wordmark(theme);
}