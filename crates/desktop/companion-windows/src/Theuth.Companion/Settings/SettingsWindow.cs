using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Media;
using Microsoft.Windows.AppNotifications;
using Theuth.Companion.Breakpoints;
using Theuth.Companion.Core;
using Theuth.Companion.Notifications;
using Theuth.Companion.Rpc;
using Windows.Graphics;

namespace Theuth.Companion.Settings;

/// <summary>
/// Owns the one Settings window (130 §4.6 D12; design 134 §4.6): created on first use, hidden on close, re-shown on
/// the next ask. <c>activate</c>: a user asked (tray menu, Ctrl+, in the panel, <c>--settings</c>) — the window takes
/// the foreground; a probe shows it without activating anything.
/// </summary>
internal sealed class SettingsController
{
    private readonly Store store;
    private readonly Notifier notifier;
    private readonly DoubleTapSource doubleTap;
    private SettingsWindow? window;

    public SettingsController(Store store, Notifier notifier, DoubleTapSource doubleTap)
    {
        this.store = store;
        this.notifier = notifier;
        this.doubleTap = doubleTap;
    }

    public bool IsOpen => window is not null && window.Visible;

    public void Show(bool activate, string? page = null) => Guard.Run("settings window", () =>
    {
        if (window is null)
        {
            window = new SettingsWindow(store, notifier, doubleTap);
            window.Closed += (_, _) => window = null;
        }
        if (!string.IsNullOrEmpty(page)) window.SelectPage(page);
        if (activate) window.Activate();
        else window.AppWindow.Show(false);
        Log.Write($"settings: window shown page={window.CurrentPage} activate={(activate ? "true" : "false")} hwnd=0x{window.Hwnd:X}");
    });

    public void Close() => window?.Close();

    public string? CurrentPage => window?.CurrentPage;
}

/// <summary>
/// A WinUI window with a <c>NavigationView</c> and every §4.6 page — Rail · Keys · Notifications · Undo delay · Launch at
/// login · Engine — each item <c>AutomationId="settings-&lt;page&gt;"</c>, each control <c>settings-&lt;page&gt;-&lt;what&gt;</c>.
/// Values live in <see cref="SettingsModel.Shared"/>; the Store, the rail, the policy and the Run key read them live.
/// Built in code (no XAML); every control is rebuilt from the model when the page is shown.
/// </summary>
internal sealed partial class SettingsWindow : Window
{
    public static readonly string[] Pages = { "rail", "keys", "notifications", "undo", "login", "engine" };

    private readonly Store store;
    private readonly Notifier notifier;
    private readonly DoubleTapSource doubleTap;
    private readonly SettingsModel model = SettingsModel.Shared;
    private readonly NavigationView nav = new();
    private readonly ScrollViewer host = new() { Padding = new Thickness(24, 12, 24, 24), HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled };
    private readonly DispatcherQueueTimer refresh;
    /// <summary>The current page's live texts (Engine counts, the Run value): refreshed in place every 2 s.</summary>
    private readonly List<Action> live = new();
    private ToggleSwitch? loginToggle;

    public string CurrentPage { get; private set; } = Pages[0];

    public nint Hwnd => WinRT.Interop.WindowNative.GetWindowHandle(this);

    public SettingsWindow(Store store, Notifier notifier, DoubleTapSource doubleTap)
    {
        this.store = store;
        this.notifier = notifier;
        this.doubleTap = doubleTap;
        Title = "theuth Companion";
        AppWindow.IsShownInSwitchers = true;
        nav.IsBackButtonVisible = NavigationViewBackButtonVisible.Collapsed;
        nav.IsSettingsVisible = false;
        nav.PaneDisplayMode = NavigationViewPaneDisplayMode.Left;
        nav.OpenPaneLength = 180;
        nav.IsPaneToggleButtonVisible = false;
        nav.Content = host;
        AutomationProperties.SetAutomationId(nav, "settings");
        foreach (var (key, title) in new[] { ("rail", "Rail"), ("keys", "Keys"), ("notifications", "Notifications"), ("undo", "Undo delay"), ("login", "Launch at login"), ("engine", "Engine") })
        {
            var item = new NavigationViewItem { Content = title, Tag = key };
            AutomationProperties.SetAutomationId(item, $"settings-{key}");
            AutomationProperties.SetName(item, title);
            nav.MenuItems.Add(item);
        }
        nav.SelectionChanged += (_, e) =>
        {
            if (e.SelectedItem is NavigationViewItem i && i.Tag is string key) ShowPage(key);
        };
        Content = nav;
        Place(720, 520);
        nav.SelectedItem = nav.MenuItems[0];
        model.Changed += OnSettingsChanged;
        model.NoticesChanged += OnNoticesChanged;
        notifier.Changed += OnNotifierChanged;
        store.PropertyChanged += OnStoreChanged;
        // The Engine page's live counts and the Login page's Run-value line re-read every 2 s while the window is up — text
        // updated in place (W6 item 4, C12): rebuilding the page every 2 s replaced the controls under UI Automation, so a
        // ToggleSwitch found by ui_find was detached by the time ui_click ran and UIA reported it off-screen.
        refresh = DispatcherQueue.CreateTimer();
        refresh.Interval = TimeSpan.FromSeconds(2);
        refresh.IsRepeating = true;
        refresh.Tick += (_, _) => Guard.Run("settings refresh", () => { foreach (var update in live) update(); });
        refresh.Start();
        Closed += (_, _) =>
        {
            refresh.Stop();
            model.Changed -= OnSettingsChanged;
            model.NoticesChanged -= OnNoticesChanged;
            notifier.Changed -= OnNotifierChanged;
            store.PropertyChanged -= OnStoreChanged;
        };
    }

    public void SelectPage(string key)
    {
        foreach (var o in nav.MenuItems)
        {
            if (o is NavigationViewItem i && (string)i.Tag == key)
            {
                nav.SelectedItem = i;
                ShowPage(key);
                return;
            }
        }
    }

    private void OnSettingsChanged(CompanionSettings old, CompanionSettings now)
    {
        if (CurrentPage == "keys" && old.Keys != now.Keys) ShowPage(CurrentPage);
        if (CurrentPage == "login" && old.LaunchAtLogin != now.LaunchAtLogin)
        {
            // In place: the switch the user (or UI Automation) just toggled stays the same element.
            if (loginToggle is { } t && t.IsOn != now.LaunchAtLogin) t.IsOn = now.LaunchAtLogin;
            foreach (var update in live) update();
        }
    }

    private void OnNoticesChanged()
    {
        if (CurrentPage == "keys") ShowPage(CurrentPage);
    }

    private void OnNotifierChanged()
    {
        if (CurrentPage == "notifications") ShowPage(CurrentPage);
    }

    private void OnStoreChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        if (CurrentPage == "engine" && e.PropertyName is nameof(Store.Connection)) foreach (var update in live) update();
        if (CurrentPage == "keys" && e.PropertyName is nameof(Store.HotkeyNotice) or nameof(Store.Keys)) ShowPage(CurrentPage); // W2's resolution notice
    }

    private void ShowPage(string key)
    {
        CurrentPage = key;
        live.Clear();
        loginToggle = null;
        var page = new StackPanel { Spacing = 10, MaxWidth = 560, HorizontalAlignment = HorizontalAlignment.Left };
        AutomationProperties.SetAutomationId(page, $"settings-page-{key}");
        Guard.Run($"settings page {key}", () =>
        {
            switch (key)
            {
                case "rail": Rail(page); break;
                case "keys": Keys(page); break;
                case "notifications": Notifications(page); break;
                case "undo": Undo(page); break;
                case "login": Login(page); break;
                default: Engine(page); break;
            }
        });
        host.Content = page;
    }

    // MARK: pages

    private void Rail(StackPanel page)
    {
        Heading(page, "Rail");
        Combo(page, "Side", "settings-rail-side", new[] { ("right", "Right"), ("left", "Left") }, model.Value.RailSide,
            v => model.Update(s => s.RailSide = v));
        var displays = new List<(string, string)> { ("", "Main") };
        foreach (var d in Displays()) displays.Add((d.Device, d.Title));
        Combo(page, "Display", "settings-rail-display", displays, model.Value.RailDisplay, v => model.Update(s => s.RailDisplay = v));
        var presences = Enum.GetValues<RailPresence>().Select(p => (p.RawValue(), p.Title())).ToList();
        Combo(page, "Rail presence", "settings-rail-presence", presences, model.Value.RailPresence.RawValue(),
            v => model.Update(s => s.RailPresence = RailPresenceText.FromRaw(v) ?? s.RailPresence));
        Caption(page, model.Value.RailPresence == RailPresence.AlwaysSliver
            ? "The rail stays a 4 px sliver; the tray icon's count is the indicator. Hover the sliver to see it."
            : "Sliver when quiet: 4 px after 8 s with nothing needing you and nothing working; hover reveals it.");
        Caption(page, "Side and display apply on the next layout; the rail never takes focus.");
    }

    private void Keys(StackPanel page)
    {
        Heading(page, "Keys");
        if ((store.HotkeyNotice ?? model.HotkeyNotice) is { } n)
        {
            var notice = Caption(page, n);
            notice.Foreground = Theme.Brush("SystemFillColorCautionBrush", 0xC0);
            AutomationProperties.SetAutomationId(notice, "settings-keys-hotkey-notice");
        }
        foreach (var scope in Enum.GetValues<KeyScope>())
        {
            var header = new TextBlock { Text = scope.Title(), FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Margin = new Thickness(0, 6, 0, 0) };
            header.Foreground = Theme.Brush("TextFillColorSecondaryBrush");
            page.Children.Add(header);
            foreach (var b in model.Keys.BindingsIn(scope)) page.Children.Add(KeyRow.Make(b, model, () => ShowPage("keys")));
        }
        Caption(page, "Global keys need a modifier and are refused when Windows or another app holds them; panel and Ask bar keys fire only while that surface is active, so they cannot collide with other apps. Windows-key chords belong to Windows.");
        var resetAll = new Button { Content = "Reset all to defaults", IsEnabled = model.Keys.Overrides.Count > 0 };
        AutomationProperties.SetAutomationId(resetAll, "settings-keys-reset-all");
        resetAll.Click += (_, _) =>
        {
            model.Update(s => s.Keys.ResetAll());
            model.ClearRowNotices();
            ShowPage("keys");
        };
        page.Children.Add(resetAll);
        page.Children.Add(new Border { Height = 1, Background = Theme.Brush("DividerStrokeColorDefaultBrush"), Margin = new Thickness(0, 6, 0, 6) });
        Toggle(page, "Double-tap Alt as well", "settings-keys-double-tap", model.Value.DoubleTapAlt, on =>
        {
            if (on)
            {
                bool ok = doubleTap.Install();
                model.Update(s => s.DoubleTapAlt = ok);
                if (!ok) ShowPage("keys");
            }
            else
            {
                doubleTap.Uninstall();
                model.Update(s => s.DoubleTapAlt = false);
            }
        });
        Caption(page, "Installs a low-level keyboard hook that sees key codes (not text) only while the companion runs — that is how Windows lets an app notice Alt pressed twice in other apps. Nothing else is read; no permission dialog exists for this.");
    }

    private void Notifications(StackPanel page)
    {
        Heading(page, "Notifications");
        var modes = new[] { (NotifyMode.Immediate.RawValue(), "Immediately"), (NotifyMode.AtBreak.RawValue(), "At a break"), (NotifyMode.Off.RawValue(), "Off") };
        var n = model.Value.Notify;
        Combo(page, "Questions, approvals, decisions", "settings-notifications-blocking", modes, n.Blocking.RawValue(),
            v => model.Update(s => s.Notify = s.Notify with { Blocking = NotifyModeText.FromRaw(v) ?? s.Notify.Blocking }));
        Combo(page, "Reviews, finished, budget at 80 %", "settings-notifications-deferrable", modes, n.Deferrable.RawValue(),
            v => model.Update(s => s.Notify = s.Notify with { Deferrable = NotifyModeText.FromRaw(v) ?? s.Notify.Deferrable }));
        Combo(page, "Errors (paused, unreachable)", "settings-notifications-error", modes, n.Error.RawValue(),
            v => model.Update(s => s.Notify = s.Notify with { Error = NotifyModeText.FromRaw(v) ?? s.Notify.Error }));
        Toggle(page, "Sound on errors", "settings-notifications-sound", n.SoundOnErrors,
            on => model.Update(s => s.Notify = s.Notify with { SoundOnErrors = on }));
        Toggle(page, "Errors break through Do not disturb", "settings-notifications-urgent", n.TimeSensitiveAvailable, on =>
        {
            model.Update(s => s.Notify = s.Notify with { TimeSensitiveAvailable = on });
            if (on) notifier.RequestUrgentConsent(); // Windows asks once per app: "Important Notification Request"
        });
        Caption(page, "Off: under Do not disturb an error lands in the notification centre and on the rail, with a sound. On: errors are posted as important notifications; Windows asks once whether to allow them when Do not disturb is on.");
        page.Children.Add(new Border { Height = 1, Background = Theme.Brush("DividerStrokeColorDefaultBrush"), Margin = new Thickness(0, 6, 0, 6) });
        var status = Caption(page, $"Windows notifications for this app: {notifier.SettingText}");
        AutomationProperties.SetAutomationId(status, "settings-notifications-status");
        var open = new Button { Content = "Open Windows notification settings" };
        AutomationProperties.SetAutomationId(open, "settings-notifications-open-windows");
        open.Click += async (_, _) => { try { await Windows.System.Launcher.LaunchUriAsync(new Uri("ms-settings:notifications")); } catch (Exception e) { Log.Write($"settings: ms-settings failed: {e.Message}"); } };
        page.Children.Add(open);
        Caption(page, "Banners carry the agent's own buttons — Allow · Deny, the question's options, Reply — so you answer without opening anything. Questions arrive at once; reviews and finished work wait for a pause in your typing or an app switch. Nothing is sent until you press a button.");
        page.Children.Add(new Border { Height = 1, Background = Theme.Brush("DividerStrokeColorDefaultBrush"), Margin = new Thickness(0, 6, 0, 6) });
        var paused = Caption(page, notifier.PausedUntil is { } until
            ? $"Paused until {until.LocalDateTime:HH:mm} — nothing is posted, the rail stays live."
            : "Not paused.");
        AutomationProperties.SetAutomationId(paused, "settings-notifications-paused");
        if (notifier.PausedUntil is not null) paused.Foreground = Theme.Brush("SystemFillColorCautionBrush", 0xC0);
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        var hour = new Button { Content = "Pause 1 hour" };
        AutomationProperties.SetAutomationId(hour, "settings-notifications-pause-hour");
        hour.Click += (_, _) => notifier.Pause(DateTimeOffset.Now.AddHours(1));
        var tomorrow = new Button { Content = "Until tomorrow" };
        AutomationProperties.SetAutomationId(tomorrow, "settings-notifications-pause-tomorrow");
        tomorrow.Click += (_, _) => notifier.Pause(Notifier.Tomorrow);
        var resume = new Button { Content = "Resume", IsEnabled = notifier.PausedUntil is not null };
        AutomationProperties.SetAutomationId(resume, "settings-notifications-resume");
        resume.Click += (_, _) => notifier.Pause(null);
        row.Children.Add(hour);
        row.Children.Add(tomorrow);
        row.Children.Add(resume);
        page.Children.Add(row);
    }

    private void Undo(StackPanel page)
    {
        Heading(page, "Undo delay");
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12 };
        var slider = new Slider { Minimum = 0, Maximum = 5, StepFrequency = 0.5, TickFrequency = 0.5, Width = 320, Value = model.Value.UndoDelay, SnapsTo = SliderSnapsTo.StepValues };
        AutomationProperties.SetAutomationId(slider, "settings-undo-slider");
        AutomationProperties.SetName(slider, "Undo delay in seconds");
        var value = new TextBlock { Text = $"{model.Value.UndoDelay:0.0} s", VerticalAlignment = VerticalAlignment.Center, Width = 48, TextAlignment = TextAlignment.Right };
        AutomationProperties.SetAutomationId(value, "settings-undo-value");
        slider.ValueChanged += (_, e) =>
        {
            value.Text = $"{e.NewValue:0.0} s";
            model.Update(s => s.UndoDelay = Math.Round(e.NewValue * 2) / 2);
        };
        row.Children.Add(slider);
        row.Children.Add(value);
        page.Children.Add(row);
        Caption(page, "Answers wait this long before they are sent; Esc undoes. 0 s sends at once.");
    }

    private void Login(StackPanel page)
    {
        Heading(page, "Launch at login");
        loginToggle = Toggle(page, "Launch at login", "settings-login-toggle", model.Value.LaunchAtLogin, on => model.Update(s => s.LaunchAtLogin = on));
        var status = Caption(page, LoginStatusText());
        AutomationProperties.SetAutomationId(status, "settings-login-status");
        AutomationProperties.SetLiveSetting(status, AutomationLiveSetting.Polite);
        live.Add(() => { var t = LoginStatusText(); if (status.Text != t) status.Text = t; });
        Caption(page, $"Written by the companion itself to HKCU\\{LoginItemRegistration.RunKey}\\{LoginItemRegistration.ValueName} and mirrored to {LoginItem.FilePath}. Windows Settings › Apps › Startup can also switch it off; uninstalling theuth removes it.");
    }

    private static string LoginStatusText() => $"Run value: {(LoginItem.ReadRunValue() is { } v ? v : "absent")} · status={LoginItem.Status()}";

    private void Engine(StackPanel page)
    {
        Heading(page, "Engine");
        string engine;
        try { engine = EngineLocation.Discover().Url.ToString(); }
        catch (Exception) { engine = "not found"; }
        var grid = new Grid { ColumnSpacing = 16, RowSpacing = 6 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        void Row(string label, Func<string> text, string id, bool isLive = false)
        {
            int r = grid.RowDefinitions.Count;
            grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            var l = new TextBlock { Text = label, Foreground = Theme.Brush("TextFillColorSecondaryBrush") };
            var t = new TextBlock { Text = text(), TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true };
            AutomationProperties.SetAutomationId(t, $"settings-engine-{id}");
            AutomationProperties.SetName(t, label);
            Grid.SetRow(l, r); Grid.SetColumn(l, 0);
            Grid.SetRow(t, r); Grid.SetColumn(t, 1);
            grid.Children.Add(l);
            grid.Children.Add(t);
            if (isLive) live.Add(() => { var s = text(); if (t.Text != s) t.Text = s; });
        }
        Row("Engine", () => engine, "url");
        Row("Connection", () => store.Connection.ToString().ToLowerInvariant(), "connection", isLive: true);
        Row("Data dir", () => EngineLocation.DataDir(), "data-dir");
        Row("Client", () => "companion", "client");
        Row("Hub", () => "— (Ask bar wave)", "hub");
        Row("Items · desks · sessions", () => $"{store.Items.Count} · {store.Agents.Count} · {store.Activity.Count}", "counts", isLive: true);
        Row("Version", () => $"{AppInfo.Version} · {AppInfo.WinUI} · .NET {Environment.Version}", "version");
        Row("Log", () => Log.DailyFile, "log");
        page.Children.Add(grid);
    }

    // MARK: building blocks

    private static void Heading(StackPanel page, string text) =>
        page.Children.Add(new TextBlock { Text = text, FontSize = 20, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Margin = new Thickness(0, 0, 0, 4) });

    private static TextBlock Caption(StackPanel page, string text)
    {
        var t = new TextBlock { Text = text, FontSize = 12, TextWrapping = TextWrapping.Wrap, Foreground = Theme.Brush("TextFillColorSecondaryBrush") };
        page.Children.Add(t);
        return t;
    }

    private static ComboBox Combo(StackPanel page, string label, string id, IReadOnlyList<(string Value, string Title)> options, string selected, Action<string> onPick)
    {
        var row = new Grid { ColumnSpacing = 12 };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var l = new TextBlock { Text = label, VerticalAlignment = VerticalAlignment.Center };
        Grid.SetColumn(l, 0);
        var combo = new ComboBox { MinWidth = 180 };
        AutomationProperties.SetAutomationId(combo, id);
        AutomationProperties.SetName(combo, label);
        foreach (var (value, title) in options)
        {
            var item = new ComboBoxItem { Content = title, Tag = value };
            AutomationProperties.SetAutomationId(item, $"{id}-{(value.Length == 0 ? "main" : value)}");
            combo.Items.Add(item);
            if (value == selected) combo.SelectedItem = item;
        }
        if (combo.SelectedItem is null && combo.Items.Count > 0) combo.SelectedIndex = 0;
        combo.SelectionChanged += (_, _) => { if (combo.SelectedItem is ComboBoxItem i && i.Tag is string v && v != selected) { selected = v; onPick(v); } };
        Grid.SetColumn(combo, 1);
        row.Children.Add(l);
        row.Children.Add(combo);
        page.Children.Add(row);
        return combo;
    }

    private static ToggleSwitch Toggle(StackPanel page, string label, string id, bool on, Action<bool> onToggle)
    {
        var t = new ToggleSwitch { Header = null, OnContent = label, OffContent = label, IsOn = on };
        AutomationProperties.SetAutomationId(t, id);
        AutomationProperties.SetName(t, label);
        t.Toggled += (_, _) => { if (t.IsOn != on) { on = t.IsOn; onToggle(t.IsOn); } };
        page.Children.Add(t);
        return t;
    }

    /// <summary>The attached displays: `\\.\DISPLAYn` device names (what <c>RailDisplay</c> stores) with a readable title.</summary>
    private static IEnumerable<(string Device, string Title)> Displays()
    {
        var result = new List<(string, string)>();
        // Indexed, not foreach: CsWinRT cannot QI this WinRT vector for IIterable ("ClassFactory cannot supply requested class").
        var all = DisplayArea.FindAll();
        for (int n = 1; n <= all.Count; n++)
        {
            var area = all[n - 1];
            string device = $"display-{n}";
            try
            {
                nint hmon = Microsoft.UI.Win32Interop.GetMonitorFromDisplayId(area.DisplayId);
                var info = new W3Native.MONITORINFOEXW { cbSize = (uint)System.Runtime.InteropServices.Marshal.SizeOf<W3Native.MONITORINFOEXW>() };
                if (W3Native.GetMonitorInfoW(hmon, ref info) && !string.IsNullOrEmpty(info.szDevice)) device = info.szDevice;
            }
            catch (Exception) { }
            var b = area.OuterBounds;
            result.Add((device, $"Display {n} — {b.Width}×{b.Height}{(area.IsPrimary ? " (main)" : "")}"));
        }
        return result;
    }

    /// <summary>Centred on the primary work area; sizes are effective px scaled by the window's DPI.</summary>
    private void Place(int width, int height)
    {
        double scale = W3Native.GetDpiForWindow(Hwnd) / 96.0;
        if (scale <= 0) scale = 1;
        var wa = DisplayArea.Primary.WorkArea;
        int w = Math.Min((int)(width * scale), wa.Width), h = Math.Min((int)(height * scale), wa.Height);
        AppWindow.MoveAndResize(new RectInt32(wa.X + (wa.Width - w) / 2, wa.Y + (wa.Height - h) / 2, w, h));
    }
}

/// <summary>Theme brushes by resource key, with a plain fallback instead of an exception when a key is missing.</summary>
internal static class Theme
{
    public static Brush Brush(string key, byte fallbackGray = 0x80)
    {
        try
        {
            if (Application.Current.Resources.TryGetValue(key, out var v) && v is Brush b) return b;
        }
        catch (Exception) { }
        return new SolidColorBrush(Windows.UI.Color.FromArgb(0xFF, fallbackGray, fallbackGray, fallbackGray));
    }
}