using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;

namespace Theuth.Companion.Ui;

/// <summary>Lightweight styling: buttons without hover/press chrome (the rail's marks, the panel's lines).</summary>
internal static class FlatButtons
{
    private static readonly string[] Keys =
    {
        "ButtonBackground", "ButtonBackgroundPointerOver", "ButtonBackgroundPressed", "ButtonBackgroundDisabled",
        "ButtonBorderBrush", "ButtonBorderBrushPointerOver", "ButtonBorderBrushPressed", "ButtonBorderBrushDisabled",
    };

    public static void Apply(ResourceDictionary resources, Brush? hover = null)
    {
        foreach (var k in Keys)
        {
            resources[k] = k is "ButtonBackgroundPointerOver" or "ButtonBackgroundPressed" && hover is not null ? hover : Palette.Transparent;
        }
    }
}