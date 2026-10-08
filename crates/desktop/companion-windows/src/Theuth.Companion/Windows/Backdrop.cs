using Microsoft.UI.Composition;
using Microsoft.UI.Composition.SystemBackdrops;
using Microsoft.UI.Xaml;
using WinRT;

namespace Theuth.Companion.Ui;

/// <summary>Which material a <see cref="Backdrop"/> wants (design 134 A7, §4.1–4.2).</summary>
internal enum BackdropKind
{
    /// <summary>The rail: <c>DesktopAcrylicController</c>, <c>DesktopAcrylicKind.Thin</c>.</summary>
    AcrylicThin,
    /// <summary>The panel: <c>MicaController</c> Base, falling back to <c>DesktopAcrylicController</c> Base where Mica is unsupported.</summary>
    MicaBase,
}

/// <summary>
/// A system backdrop on a <see cref="Window"/> through the controller API (A7), not <c>Window.SystemBackdrop</c>: the
/// <see cref="SystemBackdropConfiguration"/> is ours, so <c>IsInputActive = true</c> is held constant — a never-activated
/// window (the rail) keeps its material instead of the framework's "inactive" fallback — and <c>FallbackColor</c> (the
/// surface colour) covers unsupported systems and "Transparency effects" off. <see cref="Attach"/> returns null when no
/// controller is supported (the caller paints the opaque surface); <see cref="Dispose"/> removes the target again (the
/// caller paints opaque — used when the transparency switch goes off live, C13).
/// </summary>
internal sealed class Backdrop : IDisposable
{
    private readonly ISystemBackdropControllerWithTargets controller;
    private readonly SystemBackdropConfiguration configuration;
    private bool disposed;

    private Backdrop(ISystemBackdropControllerWithTargets controller, SystemBackdropConfiguration configuration, string material)
    {
        this.controller = controller;
        this.configuration = configuration;
        Material = material;
    }

    /// <summary>"acrylic-thin" · "mica-base" · "acrylic-base" (the Mica fallback).</summary>
    public string Material { get; }

    /// <summary>What <see cref="Attach"/> would pick right now, without attaching ("none" when nothing is supported).</summary>
    public static string Pick(BackdropKind kind)
    {
        bool acrylic = Supported(DesktopAcrylicController.IsSupported), mica = Supported(MicaController.IsSupported);
        return kind switch
        {
            BackdropKind.AcrylicThin when acrylic => "acrylic-thin",
            BackdropKind.MicaBase when mica => "mica-base",
            BackdropKind.MicaBase when acrylic => "acrylic-base",
            _ => "none",
        };
    }

    /// <summary>
    /// Attaches the material to <paramref name="window"/>; <paramref name="fallback"/> and <paramref name="tint"/> are the
    /// surface colour for the element theme. Null = unsupported (log says why); exceptions are logged, never thrown.
    /// </summary>
    public static Backdrop? Attach(Window window, BackdropKind kind, global::Windows.UI.Color fallback, ElementTheme theme, string who)
    {
        string pick = Pick(kind);
        if (pick == "none")
        {
            Log.Write($"{who}: backdrop none — DesktopAcrylicController.IsSupported={Supported(DesktopAcrylicController.IsSupported)} MicaController.IsSupported={Supported(MicaController.IsSupported)}");
            return null;
        }
        try
        {
            var configuration = new SystemBackdropConfiguration
            {
                IsInputActive = true, // A7: the app decides; held constant so the rail never falls back for want of focus
                IsHighContrast = false,
                Theme = Palette.Resolve(theme) == ElementTheme.Light ? SystemBackdropTheme.Light : SystemBackdropTheme.Dark,
            };
            ISystemBackdropControllerWithTargets controller = pick switch
            {
                "mica-base" => new MicaController { Kind = MicaKind.Base, FallbackColor = fallback },
                "acrylic-base" => new DesktopAcrylicController { Kind = DesktopAcrylicKind.Base, FallbackColor = fallback },
                // Thin acrylic tinted toward the surface so the marks keep their contrast over bright windows (C5).
                _ => new DesktopAcrylicController { Kind = DesktopAcrylicKind.Thin, FallbackColor = fallback, TintColor = fallback, TintOpacity = 0.55f, LuminosityOpacity = 0.9f },
            };
            var target = window.As<ICompositionSupportsSystemBackdrop>();
            controller.AddSystemBackdropTarget(target);
            controller.SetSystemBackdropConfiguration(configuration);
            Log.Write($"{who}: backdrop {pick} attached (IsInputActive held true, fallback #{fallback.R:X2}{fallback.G:X2}{fallback.B:X2}, theme {configuration.Theme})");
            return new Backdrop(controller, configuration, pick);
        }
        catch (Exception e)
        {
            Log.Write($"{who}: backdrop {pick} FAILED hr=0x{e.HResult:X8} {e.GetType().Name}: {e.Message} — opaque surface");
            return null;
        }
    }

    /// <summary>The element theme changed: keep the material's tint in step.</summary>
    public void SetTheme(ElementTheme theme)
    {
        if (disposed) return;
        configuration.Theme = Palette.Resolve(theme) == ElementTheme.Light ? SystemBackdropTheme.Light : SystemBackdropTheme.Dark;
    }

    private static bool Supported(Func<bool> probe)
    {
        try { return probe(); }
        catch (Exception) { return false; }
    }

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        try
        {
            controller.RemoveAllSystemBackdropTargets();
            controller.Dispose();
        }
        catch (Exception e)
        {
            Log.Write($"backdrop {Material}: dispose failed: {e.Message}");
        }
    }
}
