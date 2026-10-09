using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Theuth.Companion.Core;

namespace Theuth.Companion.Ui;

/// <summary>
/// 149 WW2 — the card anatomy's pieces (design 149 §3.3, §3.9; mock round 2 screens 3–4), shared by the Attention panel and
/// the agents panel: key-cap buttons that act without typing, position dots, the status line, the context bubbles, option rows
/// "1 ‹label› (Recommended) ›", the reply row and the footer pills (Stop agent X · Clear E · Snooze S).
/// 149 NATIVE look: colour only from the system's theme resources (<see cref="Theme"/>/<see cref="NativeLook"/>), the Fluent type
/// ramp (caption 12 · body 14 · body-strong 14), ControlCornerRadius 4 on controls and key caps, OverlayCornerRadius 8 on bubbles
/// and option rows; capsules only where the capsule is the layout (footer pills, the long position dot). Icons are Segoe Fluent
/// Icons glyphs, key caps say key names the Windows way.
/// </summary>
internal static class CardChrome
{
    /// <summary>The ramp: question = body-strong 14, body 14, meta = caption 12, caps = caption 12.</summary>
    public const double Question = 14, Body = 14, Meta = 12, Caps = 12;

    public static CornerRadius ControlRadius => Radius("ControlCornerRadius", 4);
    public static CornerRadius OverlayRadius => Radius("OverlayCornerRadius", 8);

    private static CornerRadius Radius(string key, double fallback)
    {
        try
        {
            if (Application.Current?.Resources.TryGetValue(key, out var v) == true && v is CornerRadius r) return r;
        }
        catch (Exception) { }
        return new CornerRadius(fallback);
    }

    public static Border KeyCap(ElementTheme theme, string cap, bool strong = false) => new()
    {
        CornerRadius = ControlRadius, Padding = new Thickness(5, 0, 5, 1), VerticalAlignment = VerticalAlignment.Center,
        Background = Theme.Brush(NativeLook.ControlFill, theme), BorderBrush = Theme.Brush(NativeLook.ControlStroke, theme), BorderThickness = new Thickness(1),
        Child = new TextBlock { Text = NativeLook.KeyCapText(cap), FontSize = Caps, Foreground = Theme.Brush(strong ? NativeLook.TextPrimary : NativeLook.TextSecondary, theme) },
    };

    /// <summary>A button "‹label› ‹cap›" (or the cap alone) that acts without activating the companion (D6: real buttons). A cap of "✕" / "Esc" is the close button: a Cancel glyph beside the Esc cap.</summary>
    public static Button KeyCapButton(ElementTheme theme, string? label, string cap, Action act, string? automationId = null)
    {
        bool close = label is "✕" or "×" || (string.IsNullOrEmpty(label) && cap is "✕" or "×" or "Esc" or "esc");
        if (close) label = null;
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
        if (close) row.Children.Add(Theme.Icon(NativeLook.Glyph.Cancel, theme, NativeLook.TextSecondary, 12));
        if (label is { Length: > 0 }) row.Children.Add(new TextBlock { Text = label, FontSize = Meta, Foreground = Theme.Brush(NativeLook.TextSecondary, theme), VerticalAlignment = VerticalAlignment.Center });
        row.Children.Add(KeyCap(theme, close ? "Esc" : cap));
        var b = new Button
        {
            Content = row, Padding = new Thickness(label is null ? 6 : 10, 4, label is null ? 6 : 8, 4), CornerRadius = new CornerRadius(999), BorderThickness = new Thickness(0),
            Background = Theme.Brush(NativeLook.SubtleFill, theme), MinWidth = 0, MinHeight = 0, IsTabStop = false, UseSystemFocusVisuals = false,
        };
        b.Resources["ButtonBackgroundPointerOver"] = Theme.Brush(NativeLook.SubtleFillSecondary, theme);
        b.Resources["ButtonBackgroundPressed"] = Theme.Brush(NativeLook.SubtleFillTertiary, theme);
        b.Click += (_, _) => act();
        var capWord = NativeLook.KeyCapText(close ? "Esc" : cap);
        var name = label is { Length: > 0 } ? $"{label}, key {capWord}" : close ? "Close, key Esc" : $"key {capWord}";
        AutomationProperties.SetName(b, name);
        if (automationId is not null) AutomationProperties.SetAutomationId(b, automationId);
        ToolTipService.SetToolTip(b, name);
        return b;
    }

    /// <summary>Position dots — drawn: one per item, the focused one a long accent capsule (Narrator "card 2 of 3").</summary>
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
                Background = Theme.Brush(cur ? NativeLook.Accent : NativeLook.TextTertiary, theme),
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
            Width = 8, Height = 8, Fill = Theme.Status(needsYou ? StatusTone.NeedsYou : StatusTone.Done, theme), Stroke = Theme.Brush(NativeLook.SolidBackgroundBase, theme), StrokeThickness = 1.5,
            HorizontalAlignment = HorizontalAlignment.Right, VerticalAlignment = VerticalAlignment.Bottom, Margin = new Thickness(0, 0, -3, -3),
        });
        ToolTipService.SetToolTip(icon, s.Harness);
        row.Children.Add(icon);
        var text = new TextBlock { Text = s.Text, FontSize = Meta, Foreground = Theme.Brush(s.SessionGone ? NativeLook.Critical : NativeLook.TextSecondary, theme), VerticalAlignment = VerticalAlignment.Center, TextTrimming = TextTrimming.CharacterEllipsis };
        row.Children.Add(text);
        AutomationProperties.SetAutomationId(row, "card-status");
        AutomationProperties.SetName(row, $"{s.Harness}, {s.Text}");
        return row;
    }

    /// <summary>The user's request (accent, right-aligned) or the agent's last message (card fill, left), ≤ 160 chars.</summary>
    public static FrameworkElement Bubble(ElementTheme theme, string text, bool mine)
    {
        var b = new Border
        {
            CornerRadius = OverlayRadius,
            Background = Theme.Brush(mine ? NativeLook.Accent : NativeLook.CardBackgroundSecondary, theme),
            Padding = new Thickness(12, 8, 12, 8), MaxWidth = 300,
            HorizontalAlignment = mine ? HorizontalAlignment.Right : HorizontalAlignment.Left,
            Child = new TextBlock
            {
                Text = text, FontSize = Body, TextWrapping = TextWrapping.Wrap, MaxLines = 4, TextTrimming = TextTrimming.CharacterEllipsis,
                Foreground = Theme.Brush(mine ? NativeLook.OnAccent : NativeLook.TextPrimary, theme),
            },
        };
        AutomationProperties.SetAutomationId(b, mine ? "card-request" : "card-last-reply");
        AutomationProperties.SetName(b, mine ? $"You asked: {text}" : $"Agent said: {text}");
        return b;
    }

    /// <summary>"1 ‹label› (Recommended) ›" — an 8 epx-radius card row; the key badge accent when recommended.</summary>
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
            Width = 24, Height = 24, CornerRadius = ControlRadius, VerticalAlignment = VerticalAlignment.Center,
            Background = Theme.Brush(rec ? NativeLook.Accent : NativeLook.ControlFill, theme),
            BorderBrush = Theme.Brush(rec ? NativeLook.Accent : NativeLook.ControlStroke, theme), BorderThickness = new Thickness(1),
            Child = new TextBlock
            {
                Text = NativeLook.KeyCapText(line.KeyCap), FontSize = Caps, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                Foreground = Theme.Brush(rec ? NativeLook.OnAccent : NativeLook.TextSecondary, theme),
                HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center,
            },
        };
        grid.Children.Add(badge);
        var label = new TextBlock { FontSize = Body, TextWrapping = TextWrapping.Wrap, MaxLines = 2, TextTrimming = TextTrimming.CharacterEllipsis, VerticalAlignment = VerticalAlignment.Center };
        label.Inlines.Add(new Microsoft.UI.Xaml.Documents.Run { Text = o.Label, Foreground = Theme.Brush(NativeLook.TextPrimary, theme) });
        if (rec) label.Inlines.Add(new Microsoft.UI.Xaml.Documents.Run { Text = " (Recommended)", Foreground = Theme.Brush(NativeLook.Accent, theme) });
        if (o.Detail is { } d) label.Inlines.Add(new Microsoft.UI.Xaml.Documents.Run { Text = $" — {d}", FontSize = Meta, Foreground = Theme.Brush(NativeLook.TextSecondary, theme) });
        Grid.SetColumn(label, 1);
        grid.Children.Add(label);
        var arrow = Theme.Icon(NativeLook.Glyph.ChevronRight, theme, NativeLook.TextSecondary, 12);
        Grid.SetColumn(arrow, 2);
        grid.Children.Add(arrow);
        var b = new Button
        {
            Content = grid, HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch,
            Padding = new Thickness(10, 8, 12, 8), CornerRadius = OverlayRadius, BorderThickness = new Thickness(1),
            Background = Theme.Brush(NativeLook.CardBackground, theme), BorderBrush = Theme.Brush(rec ? NativeLook.Accent : NativeLook.CardStroke, theme),
        };
        b.Resources["ButtonBackgroundPointerOver"] = Theme.Brush(NativeLook.CardBackgroundSecondary, theme);
        b.Resources["ButtonBackgroundPressed"] = Theme.Brush(NativeLook.ControlFillTertiary, theme);
        b.Resources["ButtonBorderBrushPointerOver"] = b.BorderBrush;
        b.Resources["ButtonBorderBrushPressed"] = b.BorderBrush;
        AutomationProperties.SetAutomationId(b, $"option-{o.Key}");
        AutomationProperties.SetName(b, $"{line.Text}, key {o.Key}");
        AutomationProperties.SetHelpText(b, rec ? $"Press {o.Key} or Enter" : $"Press {o.Key}");
        b.Click += (_, _) => act();
        return b;
    }

    /// <summary>The footer pill: "Stop agent X" (critical text) · "Clear E" (the default action: accent fill) · "Snooze S".</summary>
    public static Button FooterPill(ElementTheme theme, FooterVerb verb, Action act)
    {
        var (label, cap) = CardAnatomyModel.FooterText(verb);
        var b = KeyCapButton(theme, label, cap, act, verb switch { FooterVerb.Stop => "card-stop", FooterVerb.Clear => "card-clear", _ => "card-snooze" });
        b.Background = Theme.Brush(NativeLook.ControlFill, theme);
        b.Resources["ButtonBackgroundPointerOver"] = Theme.Brush(NativeLook.ControlFillSecondary, theme);
        b.Resources["ButtonBackgroundPressed"] = Theme.Brush(NativeLook.ControlFillTertiary, theme);
        if (verb == FooterVerb.Stop && b.Content is StackPanel sp && sp.Children[0] is TextBlock t) t.Foreground = Theme.Brush(NativeLook.Critical, theme);
        if (verb == FooterVerb.Clear)
        {
            b.Background = Theme.Brush(NativeLook.Accent, theme);
            b.Resources["ButtonBackgroundPointerOver"] = Theme.Brush(NativeLook.AccentSecondary, theme);
            b.Resources["ButtonBackgroundPressed"] = Theme.Brush(NativeLook.AccentTertiary, theme);
            if (b.Content is StackPanel sp2)
            {
                if (sp2.Children[0] is TextBlock lt) lt.Foreground = Theme.Brush(NativeLook.OnAccent, theme);
                if (sp2.Children[1] is Border kb)
                {
                    kb.Background = Theme.Transparent;
                    kb.BorderBrush = Theme.Brush(NativeLook.OnAccent, theme);
                    if (kb.Child is TextBlock kt) kt.Foreground = Theme.Brush(NativeLook.OnAccent, theme);
                }
            }
        }
        return b;
    }

    /// <summary>An icon button for the reply row (camera P · microphone V · send Enter): a Fluent glyph beside its key cap.</summary>
    public static Button IconCap(ElementTheme theme, string glyph, string cap, string name, bool enabled, Action act, string automationId)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 5, VerticalAlignment = VerticalAlignment.Center };
        row.Children.Add(Theme.Icon(glyph, theme, enabled ? NativeLook.TextSecondary : NativeLook.TextDisabled, 14));
        row.Children.Add(KeyCap(theme, cap));
        var b = new Button
        {
            Content = row, Height = 32, Padding = new Thickness(9, 0, 7, 0), CornerRadius = ControlRadius, BorderThickness = new Thickness(1),
            Background = Theme.Brush(NativeLook.ControlFill, theme), BorderBrush = Theme.Brush(NativeLook.ControlStroke, theme),
            MinWidth = 0, IsTabStop = false, UseSystemFocusVisuals = false, IsEnabled = enabled,
        };
        b.Resources["ButtonBackgroundPointerOver"] = Theme.Brush(NativeLook.ControlFillSecondary, theme);
        b.Resources["ButtonBackgroundPressed"] = Theme.Brush(NativeLook.ControlFillTertiary, theme);
        b.Click += (_, _) => act();
        var capWord = NativeLook.KeyCapText(cap);
        AutomationProperties.SetName(b, $"{name}, key {capWord}");
        AutomationProperties.SetAutomationId(b, automationId);
        ToolTipService.SetToolTip(b, $"{name} · {capWord}");
        return b;
    }
}
