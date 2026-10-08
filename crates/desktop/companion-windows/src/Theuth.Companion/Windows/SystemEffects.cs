using System.Runtime.InteropServices;
using Microsoft.UI.Dispatching;
using Theuth.Companion.Interop;
using Windows.Win32;

namespace Theuth.Companion.Ui;

/// <summary>
/// Windows' two accessibility/personalization switches the surfaces follow live (design 134 §6 C13, W6 item 3):
/// "Transparency effects" (<c>UISettings.AdvancedEffectsEnabled</c>; Settings › Personalization › Colors, registry
/// <c>…\Themes\Personalize\EnableTransparency</c>) and "Animation effects" (<c>UISettings.AnimationsEnabled</c>; Settings ›
/// Accessibility › Visual effects, <c>SPI_SETCLIENTAREAANIMATION</c>). Sources: the <c>UISettings</c> change events where
/// they fire (a background thread → marshalled) and the host window's <c>WM_SETTINGCHANGE</c> (always broadcast by both
/// switches); both re-read the values and <see cref="Changed"/> fires only on a real change. UI thread only.
/// </summary>
internal sealed class SystemEffects : IDisposable
{
    private const uint DwmwaTransitionsForceDisabled = 3;

    private readonly DispatcherQueue dispatcher;
    private readonly global::Windows.UI.ViewManagement.UISettings? ui;
    private readonly HostWindow? host;
    private readonly Action<uint>? onSystemChanged;
    private readonly Windows.Foundation.TypedEventHandler<global::Windows.UI.ViewManagement.UISettings, object>? onEffects;
    private readonly Windows.Foundation.TypedEventHandler<global::Windows.UI.ViewManagement.UISettings, object>? onAnimations;
    private bool disposed;

    private SystemEffects(DispatcherQueue dispatcher, HostWindow? host)
    {
        this.dispatcher = dispatcher;
        this.host = host;
        try
        {
            ui = new global::Windows.UI.ViewManagement.UISettings();
            TransparencyEnabled = ui.AdvancedEffectsEnabled;
            AnimationsEnabled = ui.AnimationsEnabled;
            onEffects = (_, _) => Marshal("UISettings.AdvancedEffectsEnabledChanged");
            onAnimations = (_, _) => Marshal("UISettings.AnimationsEnabledChanged");
            ui.AdvancedEffectsEnabledChanged += onEffects;
            ui.AnimationsEnabledChanged += onAnimations;
        }
        catch (Exception e)
        {
            Log.Write($"effects: UISettings unavailable ({e.GetType().Name}: {e.Message}) — assuming both on, WM_SETTINGCHANGE only");
            TransparencyEnabled = true;
            AnimationsEnabled = true;
        }
        if (host is not null)
        {
            onSystemChanged = msg => { if (msg == PInvoke.WM_SETTINGCHANGE) Reread("WM_SETTINGCHANGE"); };
            host.SystemChanged += onSystemChanged;
        }
        Log.Write($"effects: transparency={(TransparencyEnabled ? "on" : "off")} animations={(AnimationsEnabled ? "on" : "off")}");
    }

    /// <summary>The one instance (created by <see cref="Surfaces"/>); null before that (probes read once through the static helpers).</summary>
    public static SystemEffects? Current { get; private set; }

    public static SystemEffects Start(DispatcherQueue dispatcher, HostWindow host)
    {
        Current?.Dispose();
        Current = new SystemEffects(dispatcher, host);
        return Current;
    }

    /// <summary>The live value when the surfaces are up, else one read of <c>UISettings</c> (true when that fails).</summary>
    public static bool TransparencyOn => Current?.TransparencyEnabled ?? ReadOnce(static u => u.AdvancedEffectsEnabled);

    public static bool AnimationsOn => Current?.AnimationsEnabled ?? ReadOnce(static u => u.AnimationsEnabled);

    private static bool ReadOnce(Func<global::Windows.UI.ViewManagement.UISettings, bool> read)
    {
        try { return read(new global::Windows.UI.ViewManagement.UISettings()); }
        catch (Exception) { return true; }
    }

    /// <summary>"Transparency effects": false → every backdrop is the opaque surface colour.</summary>
    public bool TransparencyEnabled { get; private set; }

    /// <summary>"Animation effects": false → the 380 ms working beat is static and window transitions are forced off.</summary>
    public bool AnimationsEnabled { get; private set; }

    /// <summary>After either value changed (UI thread).</summary>
    public event Action? Changed;

    private void Marshal(string source)
    {
        if (disposed) return;
        dispatcher.TryEnqueue(() => Reread(source));
    }

    private void Reread(string source)
    {
        if (disposed || ui is null) return;
        bool t, a;
        try
        {
            t = ui.AdvancedEffectsEnabled;
            a = ui.AnimationsEnabled;
        }
        catch (Exception e)
        {
            Log.Write($"effects: re-read failed ({e.Message})");
            return;
        }
        if (t == TransparencyEnabled && a == AnimationsEnabled) return;
        Log.Write($"effects: changed via {source}: transparency {(TransparencyEnabled ? "on" : "off")}→{(t ? "on" : "off")} animations {(AnimationsEnabled ? "on" : "off")}→{(a ? "on" : "off")}");
        TransparencyEnabled = t;
        AnimationsEnabled = a;
        Changed?.Invoke();
    }

    /// <summary>
    /// C13 "no window transitions": <c>DWMWA_TRANSITIONS_FORCEDISABLED</c> on a top-level window while animations are off
    /// (Windows disables its own transitions with the switch; this pins it for the companion's tool windows either way).
    /// </summary>
    public static void ApplyTransitions(Windows.Win32.Foundation.HWND hwnd, bool animations)
    {
        int disable = animations ? 0 : 1;
        nint h;
        unsafe { h = (nint)hwnd.Value; }
        int hr = DwmSetWindowAttribute(h, DwmwaTransitionsForceDisabled, ref disable, sizeof(int));
        if (hr != 0) Log.Write($"effects: DwmSetWindowAttribute(TRANSITIONS_FORCEDISABLED={disable}) hr=0x{hr:X8}");
    }

    [DllImport("dwmapi.dll")]
    private static extern int DwmSetWindowAttribute(nint hwnd, uint attribute, ref int value, uint size);

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        if (host is not null && onSystemChanged is not null) host.SystemChanged -= onSystemChanged;
        try
        {
            if (ui is not null && onEffects is not null) ui.AdvancedEffectsEnabledChanged -= onEffects;
            if (ui is not null && onAnimations is not null) ui.AnimationsEnabledChanged -= onAnimations;
        }
        catch (Exception) { }
    }
}
