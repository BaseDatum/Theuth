using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace Theuth.Companion.Ui;

/// <summary>
/// theuth's palette (design 134 §4): the exact <c>UI/styles.css</c> dark/light pairs Swift's <c>Palette.swift</c> holds,
/// declared once as a <see cref="ResourceDictionary"/> of theme brushes (<c>ThemeDictionaries</c> Dark/Light) and merged
/// into <c>Application.Resources</c>, so XAML can say <c>{ThemeResource TheuthGold}</c>; code reads the same dictionary for
/// an element's <c>ActualTheme</c>. Gold is reserved for the wordmark, the count and needs-you marks (C7).
/// </summary>
internal static class Palette
{
    public const string Gold = "TheuthGold";
    public const string Faience = "TheuthFaience";
    public const string Malachite = "TheuthMalachite";
    public const string Carnelian = "TheuthCarnelian";

    /// <summary><c>--fg-tertiary</c>: the lapsed ring and dim text.</summary>
    public const string Sand = "TheuthSand";

    /// <summary><c>--fg-primary</c>.</summary>
    public const string Ink = "TheuthInk";

    /// <summary><c>--fg-secondary</c>.</summary>
    public const string Ink2 = "TheuthInk2";

    /// <summary><c>--bg-layer-1</c>: the rail's and the panels' surface.</summary>
    public const string Surface = "TheuthSurface";

    /// <summary><c>--border-strong</c>.</summary>
    public const string Rule = "TheuthRule";

    private static readonly (string Key, uint Dark, uint Light)[] Pairs =
    {
        (Gold, 0xc47a2c, 0xb4531e),
        (Faience, 0x5b8f8a, 0x3d6b66),
        (Malachite, 0x4a8a6e, 0x2f5d4a),
        (Carnelian, 0xc45a48, 0x9b2d1f),
        (Sand, 0x9a8b72, 0x8f7f6a),
        (Ink, 0xe8dcc8, 0x1a1612),
        (Ink2, 0xc9b89a, 0x6b5e4f),
        (Surface, 0x1c1915, 0xf3eadf),
        (Rule, 0x3a3228, 0xc9b48a),
    };

    private static ResourceDictionary? dictionary;

    /// <summary>The one dictionary (created on the UI thread at first use).</summary>
    public static ResourceDictionary Dictionary => dictionary ??= Build();

    private static ResourceDictionary Build()
    {
        var dark = new ResourceDictionary();
        var light = new ResourceDictionary();
        foreach (var (key, d, l) in Pairs)
        {
            dark[key] = new SolidColorBrush(Hex(d));
            light[key] = new SolidColorBrush(Hex(l));
        }
        var all = new ResourceDictionary();
        all.ThemeDictionaries["Dark"] = dark;
        all.ThemeDictionaries["Light"] = light;
        return all;
    }

    /// <summary>Merges the palette into the application's resources (once).</summary>
    public static void Install()
    {
        var merged = Application.Current.Resources.MergedDictionaries;
        if (!merged.Contains(Dictionary)) merged.Add(Dictionary);
    }

    public static SolidColorBrush Brush(string key, ElementTheme theme) =>
        (SolidColorBrush)((ResourceDictionary)Dictionary.ThemeDictionaries[Resolve(theme) == ElementTheme.Light ? "Light" : "Dark"])[key];

    /// <summary>An element not yet in a live tree reports <c>Default</c>: use the application's theme then.</summary>
    public static ElementTheme Resolve(ElementTheme theme) =>
        theme != ElementTheme.Default ? theme
        : Application.Current?.RequestedTheme == ApplicationTheme.Light ? ElementTheme.Light : ElementTheme.Dark;

    public static Color ColorOf(string key, ElementTheme theme) => Brush(key, theme).Color;

    /// <summary>The colour at <paramref name="opacity"/> (Swift's <c>.opacity(x)</c>), as a fresh brush.</summary>
    public static SolidColorBrush Tint(string key, ElementTheme theme, double opacity)
    {
        var c = ColorOf(key, theme);
        return new SolidColorBrush(Color.FromArgb((byte)Math.Round(255 * Math.Clamp(opacity, 0, 1)), c.R, c.G, c.B));
    }

    public static SolidColorBrush Transparent { get; } = new(Color.FromArgb(0, 0, 0, 0));

    private static Color Hex(uint v) => Color.FromArgb(255, (byte)((v >> 16) & 0xff), (byte)((v >> 8) & 0xff), (byte)(v & 0xff));
}
