using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Theuth.Companion.Core;
using Windows.UI;

namespace Theuth.Companion.Ui;

/// <summary>
/// 149 NATIVE (Sky, 2026-10-09 08:33: "no one asked you to theme the companion app. it's supposed to be fully native").
/// The one place the surfaces get colour from: WinUI's own theme resources (<c>XamlControlsResources</c>' Default/Light
/// dictionaries — <c>AccentFillColorDefaultBrush</c>, <c>TextFillColorPrimaryBrush</c>, <c>CardBackgroundFillColorDefaultBrush</c>,
/// <c>SystemFillColorSuccessBrush</c> …), looked up live for a given <see cref="ElementTheme"/>, so every window — including the
/// ones built in code — paints with the user's accent and follows Windows light/dark. The keys are the portable
/// <see cref="NativeLook"/> table (tested on the Mac). No hex anywhere: a key the dictionaries do not hold falls back to the
/// system accent / the theme's text colour and is logged once.
/// </summary>
internal static class Theme
{
    private static readonly Dictionary<(string, ElementTheme), Brush> cache = new();
    private static readonly HashSet<string> missing = new();
    private static global::Windows.UI.ViewManagement.UISettings? ui;

    /// <summary>A brush for a theme-resource key under <paramref name="theme"/> (Default → the system theme).</summary>
    public static Brush Brush(string key, ElementTheme theme)
    {
        var t = Resolve(theme);
        if (cache.TryGetValue((key, t), out var hit)) return hit;
        var b = Lookup(key, t) ?? Fallback(key, t);
        cache[(key, t)] = b;
        return b;
    }

    /// <summary>A brush for a status tone (the shared mapping table).</summary>
    public static Brush Status(StatusTone tone, ElementTheme theme) => Brush(NativeLook.Brush(tone), theme);

    public static Color ColorOf(string key, ElementTheme theme) => Brush(key, theme) is SolidColorBrush s ? s.Color : Microsoft.UI.Colors.Transparent;

    /// <summary>The system accent colour itself (<c>SystemAccentColor</c>) — e.g. for the point-and-say trail raster.</summary>
    public static Color Accent()
    {
        try
        {
            if (Application.Current?.Resources.TryGetValue("SystemAccentColor", out var v) == true && v is Color c) return c;
        }
        catch (Exception) { }
        try { return (ui ??= new global::Windows.UI.ViewManagement.UISettings()).GetColorValue(global::Windows.UI.ViewManagement.UIColorType.Accent); }
        catch (Exception) { return Color.FromArgb(255, 0, 120, 212); } // Windows' default accent, only when every API fails
    }

    public static SolidColorBrush Transparent { get; } = new(Microsoft.UI.Colors.Transparent);

    /// <summary>An element not yet in a live tree reports <c>Default</c>: that is the system's theme (live, from UISettings).</summary>
    public static ElementTheme Resolve(ElementTheme theme) => theme == ElementTheme.Default ? System : theme;

    /// <summary>
    /// Windows' "Choose your mode" for apps, read live: the system background colour's luminance (<c>UISettings.GetColorValue(Background)</c>),
    /// which is what WinUI's <c>ApplicationTheme</c> mirrors at startup — and keeps mirroring here after the user switches.
    /// </summary>
    public static ElementTheme System
    {
        get
        {
            try
            {
                var bg = (ui ??= new global::Windows.UI.ViewManagement.UISettings()).GetColorValue(global::Windows.UI.ViewManagement.UIColorType.Background);
                return bg.R + bg.G + bg.B > 3 * 128 ? ElementTheme.Light : ElementTheme.Dark;
            }
            catch (Exception)
            {
                return Application.Current?.RequestedTheme == ApplicationTheme.Light ? ElementTheme.Light : ElementTheme.Dark;
            }
        }
    }

    /// <summary>
    /// Follow the system on a window's root, live: sets <c>RequestedTheme</c> to the system's mode now and on every
    /// <see cref="SystemEffects.ThemeChanged"/> (UISettings.ColorValuesChanged, marshalled), then calls <paramref name="rerender"/>.
    /// Also fires on the root's own <c>ActualThemeChanged</c>. Returns an action that detaches.
    /// </summary>
    public static Action Follow(FrameworkElement root, Action rerender)
    {
        root.RequestedTheme = System;
        void OnTheme()
        {
            var want = System;
            if (root.RequestedTheme != want) root.RequestedTheme = want;
            cache.Clear();
            rerender();
        }
        void OnActual(FrameworkElement _, object __) => rerender();
        root.ActualThemeChanged += OnActual;
        var effects = SystemEffects.Current;
        if (effects is not null) effects.ThemeChanged += OnTheme;
        return () =>
        {
            root.ActualThemeChanged -= OnActual;
            if (effects is not null) effects.ThemeChanged -= OnTheme;
        };
    }

    /// <summary>The system theme changed: forget the cached brushes (the dictionaries hand out new ones per theme anyway).</summary>
    public static void Invalidate() => cache.Clear();

    // MARK: lookup

    private static Brush? Lookup(string key, ElementTheme theme)
    {
        var app = Application.Current;
        if (app is null) return null;
        string themeKey = theme == ElementTheme.Light ? "Light" : "Default";
        try
        {
            var found = FindInThemeDictionaries(app.Resources, themeKey, key, 0);
            if (found is Brush b) return b;
            // Not theme-specific (or the dictionaries are flattened): the application's current resolution.
            if (app.Resources.TryGetValue(key, out var v) && v is Brush b2) return b2;
        }
        catch (Exception e)
        {
            if (missing.Add(key + "!")) Log.Write($"theme: lookup {key} failed: {e.GetType().Name}: {e.Message}");
        }
        return null;
    }

    private static object? FindInThemeDictionaries(ResourceDictionary dict, string themeKey, string key, int depth)
    {
        if (depth > 6) return null;
        if (dict.ThemeDictionaries.TryGetValue(themeKey, out var td) && td is ResourceDictionary themed)
        {
            if (themed.TryGetValue(key, out var v)) return v;
        }
        foreach (var m in dict.MergedDictionaries)
        {
            var v = FindInThemeDictionaries(m, themeKey, key, depth + 1);
            if (v is not null) return v;
        }
        return null;
    }

    private static Brush Fallback(string key, ElementTheme theme)
    {
        if (missing.Add(key)) Log.Write($"theme: no resource {key} for {theme} — using the system fallback");
        if (key.StartsWith("Accent", StringComparison.Ordinal)) return new SolidColorBrush(Accent());
        if (key.Contains("Background", StringComparison.Ordinal) || key.Contains("Fill", StringComparison.Ordinal) && !key.StartsWith("Text", StringComparison.Ordinal))
            return Transparent;
        // Text and strokes: the theme's foreground from UISettings.
        try
        {
            var fg = (ui ??= new global::Windows.UI.ViewManagement.UISettings()).GetColorValue(global::Windows.UI.ViewManagement.UIColorType.Foreground);
            return new SolidColorBrush(fg);
        }
        catch (Exception)
        {
            return new SolidColorBrush(theme == ElementTheme.Light ? Microsoft.UI.Colors.Black : Microsoft.UI.Colors.White);
        }
    }

    // MARK: Fluent icons

    /// <summary>A <see cref="FontIcon"/> in Segoe Fluent Icons (MDL2 on Windows 10), sized from the type ramp's caption (12) unless told otherwise.</summary>
    public static FontIcon Icon(string glyph, ElementTheme theme, string foregroundKey = NativeLook.TextSecondary, double size = 12) => new()
    {
        Glyph = glyph, FontSize = size, FontFamily = IconFont, Foreground = Brush(foregroundKey, theme),
        VerticalAlignment = VerticalAlignment.Center, HorizontalAlignment = HorizontalAlignment.Center,
    };

    private static FontFamily? iconFont;

    /// <summary>"Segoe Fluent Icons" on Windows 11; "Segoe MDL2 Assets" where the Fluent set is not installed (Windows 10).</summary>
    public static FontFamily IconFont => iconFont ??= new FontFamily(
        Environment.OSVersion.Version.Build >= 22000 ? NativeLook.IconFont : NativeLook.IconFontFallback);
}
