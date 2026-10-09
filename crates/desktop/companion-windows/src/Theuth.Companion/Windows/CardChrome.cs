using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Theuth.Companion.Core;

namespace Theuth.Companion.Ui;

/// <summary>
/// 149 WW2 — the card anatomy's pieces (design 149 §3.3, §3.9; mock round 2 screens 3–4), shared by the Attention panel and
/// the agents panel: key-cap buttons that act without typing, position dots, the status line, the faience/dark context
/// bubbles, option rows "1 ‹label› (Recommended) →", the reply row and the footer pills (Stop agent X · Clear E · Snooze S).
/// Radii: cards 24 (the window's DWM corners), option rows 10, key caps 4 (§3.9). Type: question 17 · body 15 · meta 12.5 · caps 10.5.
/// </summary>
internal static class CardChrome
{
    public const double Question = 17, Body = 15, Meta = 12.5, Caps = 10.5;

    public static Border KeyCap(ElementTheme theme, string cap, bool strong = false) => new()
    {
        CornerRadius = new CornerRadius(4), Padding = new Thickness(5, 0, 5, 1), VerticalAlignment = VerticalAlignment.Center,
        Background = Palette.Tint(Palette.Ink, theme, strong ? 0.16 : 0.10),
        Child = new TextBlock { Text = cap, FontSize = Caps, FontFamily = new FontFamily("Consolas"), Foreground = Palette.Brush(strong ? Palette.Ink : Palette.Ink2, theme) },
    };

    /// <summary>A pill button "‹label› ‹cap›" (or the cap alone) that acts without activating the companion (D6: real buttons).</summary>
    public static Button KeyCapButton(ElementTheme theme, string? label, string cap, Action act, string? automationId = null)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
        if (label is { Length: > 0 }) row.Children.Add(new TextBlock { Text = label, FontSize = Meta, Foreground = Palette.Brush(Palette.Ink2, theme), VerticalAlignment = VerticalAlignment.Center });
        row.Children.Add(KeyCap(theme, cap));
        var b = new Button
        {
            Content = row, Padding = new Thickness(label is null ? 6 : 10, 4, label is null ? 6 : 8, 4), CornerRadius = new CornerRadius(999), BorderThickness = new Thickness(0),
            Background = Palette.Tint(Palette.Ink, theme, 0.06), MinWidth = 0, MinHeight = 0, IsTabStop = false, UseSystemFocusVisuals = false,
        };
        b.Resources["ButtonBackgroundPointerOver"] = Palette.Tint(Palette.Ink, theme, 0.12);
        b.Resources["ButtonBackgroundPressed"] = Palette.Tint(Palette.Ink, theme, 0.18);
        b.Click += (_, _) => act();
        var name = label is { Length: > 0 } ? $"{label}, key {cap}" : cap == "✕" ? "Close, key Esc" : $"key {cap}";
        AutomationProperties.SetName(b, name);
        if (automationId is not null) AutomationProperties.SetAutomationId(b, automationId);
        ToolTipService.SetToolTip(b, name);
        return b;
    }

    /// <summary>● ● ▬ — one per item, the focused one long and gold (VoiceOver "card 2 of 3").</summary>
    public static FrameworkElement PositionDots(ElementTheme theme, int position, int count, int max = 8)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
        int shown = Math.Min(count, max);
        int start = Math.Clamp(position - 1 - max / 2, 0, Math.Max(0, count - shown));
        for (int i = start; i < start + shown; i++)
        {
            bool cur = i == position - 1;
            row.Children.Add(new Border
            {
                Width = cur ? 18 : 6, Height = 6, CornerRadius = new CornerRadius(3),
                Background = cur ? Palette.Brush(Palette.Gold, theme) : Palette.Tint(Palette.Ink, theme, 0.25),
            });
        }
        AutomationProperties.SetName(row, $"card {position} of {count}");
        AutomationProperties.SetAutomationId(row, "card-position");
        return row;
    }

    /// <summary>harness icon · state dot · "Finished just now · theuth".</summary>
    public static FrameworkElement StatusLine(ElementTheme theme, CardStatusLine s, bool needsYou)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        var icon = new Grid { Width = 20, Height = 20 };
        icon.Children.Add(PillGlyphs.Harness(s.Harness, theme, 20));
        icon.Children.Add(new Microsoft.UI.Xaml.Shapes.Ellipse
        {
            Width = 8, Height = 8, Fill = Palette.Brush(needsYou ? Palette.Gold : Palette.Malachite, theme), Stroke = Palette.Brush(Palette.Surface, theme), StrokeThickness = 1.5,
            HorizontalAlignment = HorizontalAlignment.Right, VerticalAlignment = VerticalAlignment.Bottom, Margin = new Thickness(0, 0, -3, -3),
        });
        ToolTipService.SetToolTip(icon, s.Harness);
        row.Children.Add(icon);
        var text = new TextBlock { Text = s.Text, FontSize = Meta, Foreground = Palette.Brush(s.SessionGone ? Palette.Carnelian : Palette.Ink2, theme), VerticalAlignment = VerticalAlignment.Center, TextTrimming = TextTrimming.CharacterEllipsis };
        row.Children.Add(text);
        AutomationProperties.SetAutomationId(row, "card-status");
        AutomationProperties.SetName(row, $"{s.Harness}, {s.Text}");
        return row;
    }

    /// <summary>The user's request (faience, right-aligned) or the agent's last message (dark, left), ≤ 160 chars.</summary>
    public static FrameworkElement Bubble(ElementTheme theme, string text, bool mine)
    {
        var b = new Border
        {
            CornerRadius = mine ? new CornerRadius(14, 14, 5, 14) : new CornerRadius(14, 14, 14, 5),
            Background = mine ? Palette.Brush(Palette.Faience, theme) : Palette.Tint(Palette.Ink, theme, 0.08),
            Padding = new Thickness(12, 8, 12, 8), MaxWidth = 300,
            HorizontalAlignment = mine ? HorizontalAlignment.Right : HorizontalAlignment.Left,
            Child = new TextBlock
            {
                Text = text, FontSize = 13.5, TextWrapping = TextWrapping.Wrap, MaxLines = 4, TextTrimming = TextTrimming.CharacterEllipsis,
                Foreground = mine ? new SolidColorBrush(Microsoft.UI.Colors.White) : Palette.Brush(Palette.Ink, theme),
            },
        };
        AutomationProperties.SetAutomationId(b, mine ? "card-request" : "card-last-reply");
        AutomationProperties.SetName(b, mine ? $"You asked: {text}" : $"Agent said: {text}");
        return b;
    }

    /// <summary>"1 ‹label› (Recommended) →" — a 10 pt-radius row; the key cap gold when recommended.</summary>
    public static Button OptionRow(ElementTheme theme, OptionLine line, Action act)
    {
        var o = line.Option;
        bool rec = o.Recommended;
        var grid = new Grid { ColumnSpacing = 10 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var badge = new Border
        {
            Width = 24, Height = 24, CornerRadius = new CornerRadius(7), VerticalAlignment = VerticalAlignment.Center,
            Background = rec ? Palette.Brush(Palette.Gold, theme) : Palette.Tint(Palette.Ink, theme, 0.10),
            Child = new TextBlock
            {
                Text = line.KeyCap, FontSize = 11.5, FontFamily = new FontFamily("Consolas"), FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                Foreground = rec ? Palette.Brush(Palette.Surface, theme) : Palette.Brush(Palette.Ink2, theme),
                HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center,
            },
        };
        grid.Children.Add(badge);
        var label = new TextBlock { FontSize = Body - 1, TextWrapping = TextWrapping.Wrap, MaxLines = 2, TextTrimming = TextTrimming.CharacterEllipsis, VerticalAlignment = VerticalAlignment.Center };
        label.Inlines.Add(new Microsoft.UI.Xaml.Documents.Run { Text = o.Label, Foreground = Palette.Brush(Palette.Ink, theme) });
        if (rec) label.Inlines.Add(new Microsoft.UI.Xaml.Documents.Run { Text = " (Recommended)", Foreground = Palette.Brush(Palette.Sand, theme) });
        if (o.Detail is { } d) label.Inlines.Add(new Microsoft.UI.Xaml.Documents.Run { Text = $" — {d}", FontSize = Meta, Foreground = Palette.Brush(Palette.Sand, theme) });
        Grid.SetColumn(label, 1);
        grid.Children.Add(label);
        var arrow = new TextBlock { Text = "→", FontSize = Body, Foreground = Palette.Brush(Palette.Sand, theme), VerticalAlignment = VerticalAlignment.Center };
        Grid.SetColumn(arrow, 2);
        grid.Children.Add(arrow);
        var b = new Button
        {
            Content = grid, HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch,
            Padding = new Thickness(10, 8, 12, 8), CornerRadius = new CornerRadius(10), BorderThickness = new Thickness(rec ? 1 : 0),
            Background = rec ? Palette.Tint(Palette.Gold, theme, 0.10) : Palette.Tint(Palette.Ink, theme, 0.06), BorderBrush = Palette.Tint(Palette.Gold, theme, 0.7),
        };
        b.Resources["ButtonBackgroundPointerOver"] = rec ? Palette.Tint(Palette.Gold, theme, 0.18) : Palette.Tint(Palette.Ink, theme, 0.11);
        b.Resources["ButtonBackgroundPressed"] = rec ? Palette.Tint(Palette.Gold, theme, 0.26) : Palette.Tint(Palette.Ink, theme, 0.16);
        b.Resources["ButtonBorderBrushPointerOver"] = b.BorderBrush;
        b.Resources["ButtonBorderBrushPressed"] = b.BorderBrush;
        AutomationProperties.SetAutomationId(b, $"option-{o.Key}");
        AutomationProperties.SetName(b, $"{line.Text}, key {o.Key}");
        AutomationProperties.SetHelpText(b, rec ? $"Press {o.Key} or Enter" : $"Press {o.Key}");
        b.Click += (_, _) => act();
        return b;
    }

    /// <summary>The footer pill: "Stop agent X" (carnelian text) · "Clear E" (ink on bone) · "Snooze S".</summary>
    public static Button FooterPill(ElementTheme theme, FooterVerb verb, Action act)
    {
        var (label, cap) = CardAnatomyModel.FooterText(verb);
        var b = KeyCapButton(theme, label, cap, act, verb switch { FooterVerb.Stop => "card-stop", FooterVerb.Clear => "card-clear", _ => "card-snooze" });
        if (verb == FooterVerb.Stop && b.Content is StackPanel sp && sp.Children[0] is TextBlock t) t.Foreground = Palette.Brush(Palette.Carnelian, theme);
        if (verb == FooterVerb.Clear)
        {
            b.Background = Palette.Brush(Palette.Ink, theme);
            b.Resources["ButtonBackgroundPointerOver"] = Palette.Tint(Palette.Ink, theme, 0.85);
            b.Resources["ButtonBackgroundPressed"] = Palette.Tint(Palette.Ink, theme, 0.75);
            if (b.Content is StackPanel sp2)
            {
                if (sp2.Children[0] is TextBlock lt) lt.Foreground = Palette.Brush(Palette.Surface, theme);
                if (sp2.Children[1] is Border kb)
                {
                    kb.Background = Palette.Tint(Palette.Surface, theme, 0.25);
                    if (kb.Child is TextBlock kt) kt.Foreground = Palette.Brush(Palette.Surface, theme);
                }
            }
        }
        return b;
    }

    /// <summary>A round icon button for the reply row ([▣ P] · [🎙 V] · [↑ ⏎]).</summary>
    public static Button IconCap(ElementTheme theme, string glyph, string cap, string name, bool enabled, Action act, string automationId)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 5, VerticalAlignment = VerticalAlignment.Center };
        row.Children.Add(new FontIcon { Glyph = glyph, FontSize = 13, Foreground = Palette.Brush(enabled ? Palette.Ink2 : Palette.Sand, theme), VerticalAlignment = VerticalAlignment.Center });
        row.Children.Add(KeyCap(theme, cap));
        var b = new Button
        {
            Content = row, Height = 36, Padding = new Thickness(9, 0, 7, 0), CornerRadius = new CornerRadius(12), BorderThickness = new Thickness(0),
            Background = Palette.Tint(Palette.Ink, theme, 0.07), MinWidth = 0, IsTabStop = false, UseSystemFocusVisuals = false, Opacity = enabled ? 1 : 0.5, IsEnabled = enabled,
        };
        b.Resources["ButtonBackgroundPointerOver"] = Palette.Tint(Palette.Ink, theme, 0.13);
        b.Resources["ButtonBackgroundPressed"] = Palette.Tint(Palette.Ink, theme, 0.2);
        b.Click += (_, _) => act();
        AutomationProperties.SetName(b, $"{name}, key {cap}");
        AutomationProperties.SetAutomationId(b, automationId);
        ToolTipService.SetToolTip(b, $"{name} · {cap}");
        return b;
    }
}
