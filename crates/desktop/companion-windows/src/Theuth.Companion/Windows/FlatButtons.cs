using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Theuth.Companion.Core;

namespace Theuth.Companion.Ui;

/// <summary>
/// Lightweight styling: buttons without the standard chrome (the rail's marks, the panel's lines) — 149 NATIVE: rest is
/// <c>SubtleFillColorTransparent</c>, hover <c>SubtleFillColorSecondary</c>, pressed <c>SubtleFillColorTertiary</c> (the system's own subtle
/// fills, looked up for <paramref name="theme"/>) unless the caller passes its own hover brush.
/// </summary>
internal static class FlatButtons
{
    private static readonly string[] Keys =
    {
        "ButtonBackground", "ButtonBackgroundPointerOver", "ButtonBackgroundPressed", "ButtonBackgroundDisabled",
        "ButtonBorderBrush", "ButtonBorderBrushPointerOver", "ButtonBorderBrushPressed", "ButtonBorderBrushDisabled",
    };

    public static void Apply(ResourceDictionary resources, Brush? hover = null, ElementTheme theme = ElementTheme.Default)
    {
        foreach (var k in Keys)
        {
            resources[k] = k switch
            {
                "ButtonBackgroundPointerOver" => hover ?? Theme.Brush(NativeLook.SubtleFillSecondary, theme),
                "ButtonBackgroundPressed" => hover ?? Theme.Brush(NativeLook.SubtleFillTertiary, theme),
                _ => Theme.Transparent,
            };
        }
    }
}
