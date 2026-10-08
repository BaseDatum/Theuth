using System.ComponentModel;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml.Input;
using Microsoft.Windows.AppLifecycle;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Windows.Win32;
using Windows.Win32.Foundation;

namespace Theuth.Companion.Ui;

/// <summary>
/// W2's composition root (design 134 §4.1–4.5, §5.4–5.5): the hidden host window (class <c>TheuthCompanionRail</c>, the
/// shell's WM_CLOSE target), the tray icon and its menu, the global hotkeys, the rail and its peek card, the Attention panel
/// and the legend — all on the UI thread, all bound to the one <see cref="Store"/>. Started at the W2 line of
/// <c>App.OnLaunched</c>, before <c>Store.Start()</c> so the first refetch is seen; disposed by <c>App.Quit</c>.
/// <para>Hooks for the sibling waves: <see cref="PauseRequested"/>, <see cref="LaunchAtLoginToggled"/> /
/// <see cref="LaunchAtLoginState"/> and <see cref="ApplySettings"/> (W3), <see cref="AskBarAvailable"/> and
/// <see cref="Hotkeys"/>.<c>Released</c> (W4), <see cref="Redirected"/> (toast activations, W3). Store events W2 handles:
/// <c>CloseRequested</c>, <c>OpenUrlRequested</c>, <c>LegendRequested</c>; <c>SettingsRequested</c> and
/// <c>AskBarRequested</c> are W3's and W4's.</para>
/// </summary>
internal sealed class Surfaces : IDisposable
{
    private readonly Store store;
    private readonly DispatcherQueue dispatcher;
    private readonly Action<int> quit;
    private readonly bool probe;
    private readonly HostWindow host;
    private readonly Tray tray;
    private readonly RailWindow rail;
    private readonly AttentionPanelWindow panel;
    private readonly PeekCardWindow peek;
    private readonly LegendWindow legend;
    private readonly TrayMenu menu;
    private readonly List<DispatcherQueueTimer> timers = new();
    private bool disposed;

    private Surfaces(Store store, DispatcherQueue dispatcher, Action<int> quit)
    {
        this.store = store;
        this.dispatcher = dispatcher;
        this.quit = quit;
        probe = Flags.Probe is not null;
        Palette.Install();
        var settings = LoadSettings();
        store.Keys = settings.Keys;
        store.UndoDelay = settings.UndoDelay;

        host = new HostWindow();
        host.CloseRequested += () => quit(0);
        host.SystemChanged += msg =>
        {
            if (msg == PInvoke.WM_DISPLAYCHANGE || msg == PInvoke.WM_SETTINGCHANGE) rail?.Layout();
        };
        // W6 item 3 (C13): the transparency / animation switches, live — the rail and the panel subscribe in their constructors.
        var effects = SystemEffects.Start(dispatcher, host);

        tray = new Tray(host);
        tray.Activated += OnTrayClick;
        tray.MenuRequested += OnTrayMenu;
        tray.Update(0);

        Hotkeys = new Hotkeys(host, dispatcher);
        Hotkeys.Apply(store.Keys);
        store.HotkeyNotice = Hotkeys.Notice;
        Hotkeys.Pressed += OnHotkey;

        rail = new RailWindow(store, settings.RailPresence, settings.RailSide);
        peek = new PeekCardWindow(store, () => rail.LastForeign);
        rail.MarkClicked += OnMark;
        rail.Polled += () => peek.Poll(rail.ScreenRect);
        panel = new AttentionPanelWindow(store) { ProbeMode = probe };
        legend = new LegendWindow(store);
        menu = new TrayMenu(host);
        SystemEffects.ApplyTransitions(legend.Hwnd, effects.AnimationsEnabled);
        SystemEffects.ApplyTransitions(peek.Hwnd, effects.AnimationsEnabled);
        effects.Changed += () =>
        {
            SystemEffects.ApplyTransitions(legend.Hwnd, effects.AnimationsEnabled);
            SystemEffects.ApplyTransitions(peek.Hwnd, effects.AnimationsEnabled);
        };

        store.PropertyChanged += OnStoreChanged;
        store.CloseRequested += () => panel.Close(returnFocus: true);
        store.OpenUrlRequested += url =>
        {
            panel.Close(returnFocus: false);
            _ = Links.Open(url);
        };
        store.LegendRequested += () => ShowLegend(fromPanel: true);
        store.SettingsRequested += () => Log.Write("settings requested (Settings is W3's window)");
        store.AskBarRequested += () => Log.Write($"Ask bar requested (W4's window){(AskBarAvailable is null ? " — not installed in this build" : "")}");

        try
        {
            AppInstance.GetCurrent().Activated += (_, e) => dispatcher.TryEnqueue(() => OnRedirected(e));
        }
        catch (Exception e)
        {
            Log.Write($"single instance: AppInstance.Activated unavailable: {e.Message}");
        }

        Log.Write($"surfaces up: tray={(tray.Added ? "added" : "FAILED")} hotkeys=[{string.Join(", ", Hotkeys.Registered.Select(kv => $"{kv.Key.RawValue()}={kv.Value.Symbol}"))}] rail={Win32.Text(rail.ScreenRect)} shareable={(Flags.Shareable ? "true" : "false")} probe={(probe ? "true" : "false")}");
        if (probe) StartProbe();
        if (Environment.GetCommandLineArgs().Contains("--probe-legend")) StartLegendProbe();
    }

    public static Surfaces? Current { get; private set; }

    /// <summary>The global hotkeys (W4: <c>Released</c> for hold-to-talk).</summary>
    public Hotkeys Hotkeys { get; }

    /// <summary>W6 (item 1): when the tray icon's first <c>NIM_ADD</c> succeeded — the pin tip checks promotion ≥ 60 s later; null = no icon.</summary>
    public DateTimeOffset? TrayAddedAt => tray.AddedAt;

    /// <summary>W3: Pause notifications ▸ "1h" · "tomorrow" · "off". The menu rows are disabled until something listens.</summary>
    public event Action<string>? PauseRequested;

    /// <summary>W6 item 9: when notifications are paused until (null = not paused) — the submenu's check marks and the row's "(until HH:mm)".</summary>
    public Func<DateTimeOffset?>? PausedUntil { get; set; }

    /// <summary>W6 item 10: the companion is quitting on purpose (tray › Quit theuth / Quit theuth Companion) — the engine going away is expected.</summary>
    public event Action? QuitRequested;

    /// <summary>W3: the Launch at login row was toggled to this value (the Run key + companion.json are W3's).</summary>
    public event Action<bool>? LaunchAtLoginToggled;

    /// <summary>W3: the row's check mark.</summary>
    public Func<bool>? LaunchAtLoginState { get; set; }

    /// <summary>W4: true when an Ask bar answers <c>Store.AskBarRequested</c> — then the hotkey opens it when nothing needs you.</summary>
    public Func<bool>? AskBarAvailable { get; set; }

    /// <summary>A second launch's (or a toast's) redirected activation, after W2 handled a plain launch (W3: toast arguments).</summary>
    public event Action<AppActivationArguments>? Redirected;

    public static Surfaces Start(Store store, DispatcherQueue dispatcher, Action<int> quit)
    {
        Current = new Surfaces(store, dispatcher, quit);
        return Current;
    }

    private static CompanionSettings LoadSettings()
    {
        try
        {
            var s = CompanionSettings.Load(SettingsStore.Load(SettingsStore.DefaultPath));
            Log.Write($"settings {SettingsStore.DefaultPath}: rail {s.RailPresence.RawValue()} {s.RailSide}, undo {s.UndoDelay} s, {s.Keys.Overrides.Count} key overrides");
            return s;
        }
        catch (Exception e)
        {
            Log.Write($"settings unreadable ({e.Message}) — defaults");
            return new CompanionSettings();
        }
    }

    /// <summary>W3: Settings changed — keys (hotkeys + grammar), undo delay, rail presence and side, applied live.</summary>
    public void ApplySettings(CompanionSettings s)
    {
        store.Keys = s.Keys;
        store.UndoDelay = s.UndoDelay;
        rail.Apply(s.RailPresence, s.RailSide);
    }

    /// <summary>
    /// W3 (integration): a toast body or its Open button — the panel on <paramref name="itemId"/> (null: the oldest needs-you
    /// item, e.g. a batch or flood summary), anchored at the mouse; the click on the toast granted us the foreground.
    /// </summary>
    public void OpenPanelOn(string? itemId)
    {
        if (probe) return;
        legend.CloseLegend(returnFocus: false);
        peek.HideCard();
        if (itemId is not null) store.FocusOn(itemId); else store.FocusOldest();
        if (panel.IsOpen)
        {
            panel.ShowActive();
            return;
        }
        panel.Open(PanelAnchor.Mouse, PreviousFor(PInvoke.GetForegroundWindow()));
    }

    /// <summary>W3 (integration): double-tap Alt — the same as the Open-the-panel hotkey (130 §4.2, D6).</summary>
    public void TogglePanel() => OnHotkey(KeyAction.OpenPanel);

    private void OnStoreChanged(object? sender, PropertyChangedEventArgs e)
    {
        switch (e.PropertyName)
        {
            case nameof(Store.Rail):
                tray.Update(store.Rail.NeedsYouCount);
                break;
            case nameof(Store.Keys):
                Hotkeys.Apply(store.Keys);
                store.HotkeyNotice = Hotkeys.Notice;
                break;
        }
    }

    /// <summary>The window to give the foreground back to: <paramref name="fg"/> unless it is one of ours or the taskbar.</summary>
    private HWND PreviousFor(HWND fg) => fg.IsNull || Win32.IsOurs(fg) || Win32.IsShellChrome(fg) ? rail.LastUserApp : fg;

    private void OnHotkey(KeyAction action)
    {
        if (probe)
        {
            Log.Write("hotkey ignored (probe)");
            return;
        }
        switch (action)
        {
            case KeyAction.OpenPanel:
                if (panel.IsOpen)
                {
                    panel.Close(returnFocus: true);
                    return;
                }
                // 130 §4.2: the panel on the oldest needs-you item; the Ask bar (W4) when nothing needs you.
                if (store.Rail.NeedsYouCount == 0 && AskBarAvailable?.Invoke() == true)
                {
                    store.Perform(Intent.AskBar);
                    return;
                }
                var prev = PreviousFor(PInvoke.GetForegroundWindow());
                legend.CloseLegend(returnFocus: false);
                peek.HideCard();
                store.FocusOldest();
                panel.Open(PanelAnchor.Mouse, prev);
                break;
            case KeyAction.OpenAskBar:
                store.Perform(Intent.AskBar);
                break;
        }
    }

    private void OnTrayClick(System.Drawing.Point at)
    {
        if (probe) return;
        // The click on the icon deactivated an open panel a moment ago: that was the toggle closing it.
        if (Environment.TickCount64 - panel.ClosedByFocusLossAt < 500) return;
        if (panel.IsOpen)
        {
            panel.Close(returnFocus: true);
            return;
        }
        peek.HideCard();
        var anchor = tray.IconRect() is { } r ? new PanelAnchor(PanelAnchorKind.Tray, r) : PanelAnchor.Mouse;
        panel.Open(anchor, rail.LastUserApp);
    }

    private void OnTrayMenu(System.Drawing.Point at)
    {
        if (probe) return;
        panel.Close(returnFocus: false);
        legend.CloseLegend(returnFocus: false);
        peek.HideCard();
        menu.Show(BuildMenu(), at);
    }

    /// <summary>§4.5: Open theuth · Ask theuth… · Pause notifications ▸ · Launch at login ✓ · Keyboard shortcuts… Ctrl+/ · Settings… · Quit theuth
    /// · Quit theuth Companion (Swift W7: Quit theuth is the whole app, the second item only the companion).</summary>
    private List<TrayMenu.Item> BuildMenu()
    {
        bool login = LaunchAtLoginState?.Invoke() ?? false;
        // W6 item 9 (parity with the Mac menu, NotificationsInstaller.swift appendMenu): the active pause carries the check mark.
        var now = DateTimeOffset.Now;
        var pausedUntil = PausedUntil?.Invoke();
        var pause = PauseMenu.State(pausedUntil, now, new DateTimeOffset(DateTime.Today.AddDays(1)));
        return new List<TrayMenu.Item>
        {
            new("Open theuth", Links.OpenTheuth),
            new("Ask theuth…", () => store.Perform(Intent.AskBar)),
            new(PauseMenu.Title(pausedUntil, now), Enabled: PauseRequested is not null, Sub: new List<TrayMenu.Item>
            {
                new("For 1 hour", () => PauseRequested?.Invoke("1h"), Checked: pause == PauseMenuState.Hour),
                new("Until tomorrow", () => PauseRequested?.Invoke("tomorrow"), Checked: pause == PauseMenuState.Tomorrow),
                new("Off", () => PauseRequested?.Invoke("off"), Checked: pause == PauseMenuState.Off),
            }),
            new("Launch at login", () => LaunchAtLoginToggled?.Invoke(!login), Enabled: LaunchAtLoginToggled is not null, Checked: login),
            TrayMenu.Item.Separator,
            new("Keyboard shortcuts…\tCtrl+/", () => ShowLegend(fromPanel: false)),
            new("Settings…", () => store.Perform(Intent.Settings)),
            TrayMenu.Item.Separator,
            new("Quit theuth", QuitTheuth),
            new("Quit theuth Companion", () => { QuitRequested?.Invoke(); quit(0); }),
        };
    }

    /// <summary>
    /// Quit theuth = the whole app (Swift W7): the shell's quit handshake (<c>theuth://quit</c>) stops the engine and then
    /// closes this companion. With no shell, or if the handshake has not closed the companion within 10 s, it quits itself.
    /// </summary>
    private void QuitTheuth()
    {
        QuitRequested?.Invoke(); // before theuth://quit: the shell stops the engine ~60 ms before it closes us (item 10)
        if (!Links.QuitTheuth())
        {
            quit(0);
            return;
        }
        _ = Task.Delay(TimeSpan.FromSeconds(10)).ContinueWith(_ => dispatcher.TryEnqueue(() => quit(0)));
    }

    private void OnMark(Mark m, RECT row)
    {
        if (probe) return;
        if (RailWindow.OpensPanel(m))
        {
            peek.HideCard();
            store.FocusOn(m.ItemId!);
            panel.Open(new PanelAnchor(PanelAnchorKind.Rail, row, rail.Side == "left"), rail.LastForeign);
            return;
        }
        if (peek.Showing?.Id == m.Id)
        {
            peek.HideCard();
            return;
        }
        peek.Show(m, row, rail.ScreenRect, rail.Side);
    }

    /// <summary>The legend under the tray icon; from the panel (Ctrl+/ · ?) it replaces the panel and returns to the panel's window.</summary>
    private void ShowLegend(bool fromPanel)
    {
        if (probe) return;
        var prev = fromPanel && panel.IsOpen ? panel.Previous : rail.LastUserApp;
        panel.Close(returnFocus: false);
        legend.Toggle(tray.IconRect(), prev);
    }

    private void OnRedirected(AppActivationArguments e)
    {
        Log.Write($"activation redirected from another launch (kind={e.Kind})");
        Redirected?.Invoke(e);
        if (probe || e.Kind != ExtendedActivationKind.Launch) return;
        // A second launch brings the first forward: the panel, activated (that launch granted us the foreground).
        if (panel.IsOpen)
        {
            panel.ShowActive();
            return;
        }
        var prev = PreviousFor(PInvoke.GetForegroundWindow());
        store.FocusOldest();
        panel.Open(PanelAnchor.Mouse, prev);
    }

    private void After(double seconds, Action act)
    {
        var t = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromSeconds(seconds);
        t.IsRepeating = false;
        t.Tick += (_, _) =>
        {
            t.Stop();
            timers.Remove(t);
            if (!disposed) act();
        };
        timers.Add(t);
        t.Start();
    }

    /// <summary>
    /// <c>--probe N</c> (Swift <c>runProbe</c>): the surfaces up for N s, never activated, never answering — after 1.5 s the
    /// panel shown without activation; the rail/panel line then, the window list and the surfaces line just before the
    /// EngineProbe's done line (its counts and exit lines are unchanged).
    /// </summary>
    private void StartProbe()
    {
        double seconds = Flags.Probe ?? 15;
        After(1.5, () =>
        {
            panel.Open(PanelAnchor.Mouse, default);
            Log.Write($"probe rail rect={Win32.Text(rail.ScreenRect)} visible={B(rail.IsShown)} exstyle={Win32.ExStyleText(Win32.ExStyle(rail.Hwnd))} affinity=0x{Win32.DisplayAffinity(rail.Hwnd):X} collapsed={B(rail.Collapsed)} marks={rail.MarkCount}; " +
                      $"panel rect={Win32.Text(panel.ScreenRect)} visible={B(panel.IsShown)} affinity=0x{Win32.DisplayAffinity(panel.Hwnd):X} foreground={Win32.Describe(PInvoke.GetForegroundWindow())}");
        });
        After(Math.Max(0.5, seconds - 0.25), () =>
        {
            var wins = Win32.OurWindows().Where(h => PInvoke.IsWindowVisible(h))
                .Select(h => $"{Win32.ClassOf(h)} \"{Win32.TitleOf(h)}\" {Win32.Text(Win32.Rect(h))} {Win32.ExStyleText(Win32.ExStyle(h))}");
            Log.Write($"probe windows: [{string.Join("; ", wins)}]");
            var fg = PInvoke.GetForegroundWindow();
            Log.Write($"probe surfaces: tray={(tray.Added ? "added" : "missing")} \"{tray.Tooltip}\" hotkeys=[{string.Join(", ", Hotkeys.Registered.Select(kv => $"{kv.Key.RawValue()}={kv.Value.Symbol}"))}] " +
                      $"rail marks={rail.MarkCount} panel visible={B(panel.IsShown)} foreground={Win32.Describe(fg)} ours={B(Win32.IsOurs(fg))}");
        });
    }

    /// <summary><c>--probe-legend</c> (Swift W6): the legend for 3 s without activation, logged, then quit 0.</summary>
    private void StartLegendProbe()
    {
        Log.Write($"probe-legend start rows={store.Keys.Bindings.Count} unique={B(store.Keys.IsUniquePerScope)} shareable={B(Flags.Shareable)}");
        After(1, () =>
        {
            legend.Open(tray.IconRect(), default, activate: false);
            Log.Write($"probe-legend shown rect={Win32.Text(legend.ScreenRect)} visible={B(legend.IsShown)} foreground={Win32.Describe(PInvoke.GetForegroundWindow())}");
        });
        After(4, () =>
        {
            Log.Write("probe-legend done — quitting 0");
            quit(0);
        });
    }

    private static string B(bool v) => v ? "true" : "false";

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        foreach (var t in timers) t.Stop();
        store.PropertyChanged -= OnStoreChanged;
        rail.Stop();
        panel.Stop();
        Hotkeys.Dispose();
        tray.Dispose();
        SystemEffects.Current?.Dispose();
        foreach (var w in new ToolWindow[] { legend, peek, panel, rail })
        {
            try { w.CloseForGood(); }
            catch (Exception) { }
        }
        host.Dispose();
        Log.Write("surfaces down: tray removed, hotkeys released");
    }
}