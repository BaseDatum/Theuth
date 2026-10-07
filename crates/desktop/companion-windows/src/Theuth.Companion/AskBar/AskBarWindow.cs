using System.ComponentModel;
using Microsoft.UI;
using Microsoft.UI.Composition.SystemBackdrops;
using Microsoft.UI.Input;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Theuth.Companion.Core;
using Theuth.Companion.Voice;
using Windows.Graphics;
using Windows.System;
using Windows.UI.Core;
using DispatcherQueue = Microsoft.UI.Dispatching.DispatcherQueue;
using DispatcherQueueTimer = Microsoft.UI.Dispatching.DispatcherQueueTimer;

namespace Theuth.Companion.AskBar;

/// <summary>
/// The Ask bar (design 134 §4.3, D7; mock round 2 frame 4): a borderless, topmost, switcher-hidden 480 epx window at the
/// top-centre of the display under the mouse — the field "Ask theuth…" (<c>AutomationId="ask-field"</c>), the mic glyph,
/// the hints row and the receipt row (<c>receipt-row</c>). It is <see cref="AskBarModel"/>'s surface: the model decides,
/// this renders; <see cref="VoiceRecorder"/> is its microphone. Summoned it takes the foreground (the WM_HOTKEY grant makes
/// that legitimate) and on Esc hands it back to the window it took it from (D5, verified in spike S3). Keys go through
/// the Ask-bar scope of the key table (Enter sends, Esc undoes then closes, Ctrl+O opens the desk's session, Ctrl+/ the
/// legend); the live dictation owns the field until it lands (finals, then the open partial after a space — a TextBox
/// cannot mute a range, so the partial is told apart by the status "listening…" and the mic pulse).
/// </summary>
internal sealed partial class AskBarWindow : Window, IAskBarSurface
{
    public const int Width = 480;

    private readonly Store store;
    private readonly DispatcherQueue dispatcher;
    private readonly VoiceRecorder recorder;
    private readonly nint hwnd;
    private readonly Grid root;
    private readonly TextBox field;
    private readonly FontIcon mic;
    private readonly ScaleTransform micScale = new() { CenterX = 8, CenterY = 8 };
    private readonly StackPanel escHint;
    private readonly StackPanel holdHint;
    private readonly TextBlock status;
    private readonly HyperlinkButton privacyLink;
    private readonly Border receiptRow;
    private readonly TextBlock receiptText;
    private readonly TextBlock receiptTrailing;
    private readonly Rectangle holdBar;
    private readonly Brush ink, ink2, sand, carnelian, malachite, rule;
    private AskBarModel? model;
    private nint previousForeground;
    private bool settingText;
    private DispatcherQueueTimer? pressPoll;
    private int pressVk;
    private bool contentLoaded;

    public AskBarWindow(Store store, DispatcherQueue dispatcher)
    {
        this.store = store;
        this.dispatcher = dispatcher;
        recorder = new VoiceRecorder(dispatcher);
        Title = "Ask theuth";
        hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);

        bool dark = Application.Current.RequestedTheme == ApplicationTheme.Dark;
        ink = Solid(dark ? 0xe8dcc8u : 0x1a1612u);
        ink2 = Solid(dark ? 0xc9b89au : 0x6b5e4fu);
        sand = Solid(dark ? 0x9a8b72u : 0x8f7f6au);
        carnelian = Solid(dark ? 0xc45a48u : 0x9b2d1fu);
        malachite = Solid(dark ? 0x4a8a6eu : 0x2f5d4au);
        rule = Solid(dark ? 0x3a3228u : 0xc9b48au);

        // Glass like the Attention panel (D5): acrylic when supported, else the opaque surface colour.
        if (DesktopAcrylicController.IsSupported()) SystemBackdrop = new DesktopAcrylicBackdrop();

        root = new Grid { Width = Width, Background = SystemBackdrop is null ? Solid(dark ? 0x1c1915u : 0xf3eadfu) : new SolidColorBrush(Colors.Transparent) };
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        AutomationProperties.SetName(root, "Ask bar");
        AutomationProperties.SetAutomationId(root, "ask-bar");

        // Row 0: glyph · field · mic
        var fieldRow = new Grid { Padding = new Thickness(14, 13, 14, 10), ColumnSpacing = 10 };
        fieldRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        fieldRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        fieldRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var cursorGlyph = new FontIcon { Glyph = "\uE8E9", FontSize = 13, Foreground = sand, VerticalAlignment = VerticalAlignment.Center };
        Grid.SetColumn(cursorGlyph, 0);
        field = new TextBox
        {
            PlaceholderText = "Ask theuth…",
            FontSize = 15,
            BorderThickness = new Thickness(0),
            Background = new SolidColorBrush(Colors.Transparent),
            Foreground = ink,
            AcceptsReturn = false,
            TextWrapping = TextWrapping.Wrap,
            MaxLength = 4000,
            IsSpellCheckEnabled = false,
            VerticalAlignment = VerticalAlignment.Center,
            Padding = new Thickness(0, 4, 0, 4),
        };
        AutomationProperties.SetAutomationId(field, "ask-field");
        AutomationProperties.SetName(field, "Ask theuth");
        AutomationProperties.SetHelpText(field, "Enter sends to theuth's desk, which routes it");
        field.TextChanged += (_, _) =>
        {
            if (!settingText) model?.FieldEdited(field.Text);
        };
        Grid.SetColumn(field, 1);
        mic = new FontIcon { Glyph = "\uE720", FontSize = 14, Foreground = ink2, VerticalAlignment = VerticalAlignment.Center, RenderTransform = micScale };
        AutomationProperties.SetAutomationId(mic, "ask-mic");
        AutomationProperties.SetName(mic, "Hold the hotkey to dictate");
        Grid.SetColumn(mic, 2);
        fieldRow.Children.Add(cursorGlyph);
        fieldRow.Children.Add(field);
        fieldRow.Children.Add(mic);
        Grid.SetRow(fieldRow, 0);

        // Row 1: hints · status
        var hints = new Grid { Padding = new Thickness(14, 0, 14, 9), ColumnSpacing = 12 };
        hints.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        hints.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        hints.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        hints.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        hints.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        AutomationProperties.SetAutomationId(hints, "ask-hints");
        var sendHint = Hint("⏎", "send to theuth · it routes");
        Grid.SetColumn(sendHint, 0);
        holdHint = Hint("hold Ctrl+Shift+Space", "to talk · release to send in 3 s");
        Grid.SetColumn(holdHint, 1);
        escHint = Hint("Esc", "cancel");
        Grid.SetColumn(escHint, 2);
        var statusPanel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
        status = new TextBlock { FontSize = 10.5, Foreground = sand, TextTrimming = TextTrimming.CharacterEllipsis, MaxLines = 1, VerticalAlignment = VerticalAlignment.Center };
        AutomationProperties.SetAutomationId(status, "ask-status");
        AutomationProperties.SetName(status, "Ask bar status");
        privacyLink = new HyperlinkButton { Content = "Settings › Privacy", FontSize = 10.5, Padding = new Thickness(2, 0, 2, 0), Visibility = Visibility.Collapsed, NavigateUri = new Uri(VoiceRecorder.PrivacySettingsUri) };
        AutomationProperties.SetAutomationId(privacyLink, "ask-privacy-link");
        statusPanel.Children.Add(status);
        statusPanel.Children.Add(privacyLink);
        Grid.SetColumn(statusPanel, 4);
        hints.Children.Add(sendHint);
        hints.Children.Add(holdHint);
        hints.Children.Add(escHint);
        hints.Children.Add(statusPanel);
        Grid.SetRow(hints, 1);

        // Row 2: the receipt row (Swift ReceiptView, 480 wide)
        receiptRow = new Border { BorderBrush = rule, BorderThickness = new Thickness(0, 1, 0, 0), Visibility = Visibility.Collapsed };
        var receiptGrid = new Grid { Padding = new Thickness(12, 8, 12, 8), ColumnSpacing = 8, Background = Translucent(dark ? 0x4a8a6eu : 0x2f5d4au, 0.12) };
        receiptGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        receiptGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        receiptText = new TextBlock { FontSize = 12, Foreground = ink, TextWrapping = TextWrapping.Wrap, MaxLines = 2, TextTrimming = TextTrimming.CharacterEllipsis };
        AutomationProperties.SetAutomationId(receiptText, "receipt-row");
        AutomationProperties.SetName(receiptText, "Receipt");
        Grid.SetColumn(receiptText, 0);
        receiptTrailing = new TextBlock { FontSize = 12, Foreground = ink2, FontFamily = new FontFamily("Consolas"), VerticalAlignment = VerticalAlignment.Top };
        AutomationProperties.SetAutomationId(receiptTrailing, "receipt-hint");
        Grid.SetColumn(receiptTrailing, 1);
        receiptGrid.Children.Add(receiptText);
        receiptGrid.Children.Add(receiptTrailing);
        holdBar = new Rectangle { Height = 2, Fill = malachite, HorizontalAlignment = HorizontalAlignment.Left, VerticalAlignment = VerticalAlignment.Bottom, Width = 0 };
        var receiptStack = new Grid();
        receiptStack.Children.Add(receiptGrid);
        receiptStack.Children.Add(holdBar);
        receiptRow.Child = receiptStack;
        Grid.SetRow(receiptRow, 2);

        root.Children.Add(fieldRow);
        root.Children.Add(hints);
        root.Children.Add(receiptRow);
        root.PreviewKeyDown += OnPreviewKeyDown;
        root.PreviewKeyUp += OnPreviewKeyUp;
        root.Loaded += (_, _) =>
        {
            contentLoaded = true;
            Fit();
        };
        Content = root;

        // Window chrome: borderless, topmost, hidden from Alt+Tab and the taskbar, never resizable (D5's shape).
        if (AppWindow.Presenter is OverlappedPresenter p)
        {
            p.SetBorderAndTitleBar(false, false);
            p.IsResizable = false;
            p.IsMaximizable = false;
            p.IsMinimizable = false;
            p.IsAlwaysOnTop = true;
        }
        AppWindow.IsShownInSwitchers = false;
        AppWindow.Closing += (_, e) =>
        {
            e.Cancel = true; // Alt+F4: close like Esc, never destroy the window
            model?.Close(returnFocus: true);
        };
        if (!Flags.Shareable) Win32.SetWindowDisplayAffinity(hwnd, Win32.WdaExcludeFromCapture); // C11
        AppWindow.Resize(new SizeInt32(Physical(Width), Physical(96)));

        recorder.OnChunk = c => model?.Feed(c);
        recorder.OnFailed = why => model?.MicrophoneFailed(why);
        store.PropertyChanged += OnStoreChanged;
    }

    /// <summary>The model this window renders (set once by <see cref="AskBarInstall"/>; the model needs the surface first).</summary>
    public void Bind(AskBarModel m)
    {
        model = m;
        m.PropertyChanged += OnModelChanged;
        Render();
    }

    public VoiceRecorder Recorder => recorder;

    public nint Hwnd => hwnd;

    public bool IsVisible => AppWindow.IsVisible;

    // MARK: IAskBarSurface

    public void Show(bool activate)
    {
        if (activate && !AppWindow.IsVisible)
        {
            var fg = Win32.GetForegroundWindow();
            previousForeground = fg == hwnd ? 0 : fg;
        }
        Place();
        if (activate)
        {
            Activate();
            field.Focus(FocusState.Programmatic);
            field.SelectionStart = field.Text.Length;
        }
        else
        {
            AppWindow.Show(false);
        }
        Fit();
    }

    public void Hide(bool returnFocus)
    {
        StopPressPoll();
        AppWindow.Hide();
        var prev = previousForeground;
        previousForeground = 0;
        if (returnFocus && prev != 0) Win32.SetForegroundWindow(prev);
    }

    public Task<string?> StartMicrophoneAsync()
    {
        recorder.PreferMixFormat = AskFlags.MicMixFormat;
        return recorder.StartAsync().ContinueWith(t =>
        {
            Log.Write($"microphone: {(t.Result is null ? "open" : t.Result)} — {recorder.Description ?? "?"}");
            return t.Result;
        }, TaskScheduler.FromCurrentSynchronizationContext());
    }

    public void StopMicrophone() => recorder.Stop();

    public void CancelMicrophone() => recorder.Cancel();

    public void OpenUrl(Uri url)
    {
        Log.Write($"open {url.OriginalString}");
        _ = Launcher.LaunchUriAsync(url);
    }

    // MARK: the hotkey (W2's RegisterHotKey delivers the key-down; the release is seen here)

    /// <summary>
    /// W2 calls this when Ctrl+Shift+Space goes down and the press is the Ask bar's (nothing needs you). The key-up never
    /// reaches <c>WM_HOTKEY</c>, so the release is observed: the window's own KeyUp once it has the foreground, and a
    /// 50 ms poll of the key's physical state (<c>GetAsyncKeyState</c>) for a release before that.
    /// </summary>
    public void HotkeyDown(ushort keyCode = VirtualKeys.Space)
    {
        if (model is null) return;
        pressVk = keyCode;
        model.HotkeyDown();
        if (!model.Automated) StartPressPoll();
    }

    /// <summary>The key came up (W2 may also call this; idempotent).</summary>
    public void HotkeyUp()
    {
        if (pressVk == 0) return;
        pressVk = 0;
        StopPressPoll();
        model?.HotkeyUp();
    }

    private void StartPressPoll()
    {
        if (pressPoll is not null) return;
        pressPoll = dispatcher.CreateTimer();
        pressPoll.Interval = TimeSpan.FromMilliseconds(50);
        pressPoll.IsRepeating = true;
        pressPoll.Tick += (_, _) =>
        {
            if (pressVk != 0 && !Win32.IsKeyDown(pressVk)) HotkeyUp();
        };
        pressPoll.Start();
    }

    private void StopPressPoll()
    {
        pressPoll?.Stop();
        pressPoll = null;
    }

    // MARK: keys

    private void OnPreviewKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (model is null) return;
        if (pressVk != 0 && (int)e.Key == pressVk)
        {
            e.Handled = true; // the held hotkey's own key must not type
            return;
        }
        var press = KeyPressFrom(e);
        if (model.HandleKey(press)) e.Handled = true;
    }

    private void OnPreviewKeyUp(object sender, KeyRoutedEventArgs e)
    {
        if (pressVk != 0 && (int)e.Key == pressVk)
        {
            e.Handled = true;
            HotkeyUp();
        }
    }

    /// <summary>A WinUI key event as the key table reads it (134 as-built W1 §5: chars without modifiers, VK code, modifier state).</summary>
    internal static KeyPress KeyPressFrom(KeyRoutedEventArgs e)
    {
        var mods = ChordModifiers.None;
        if (Down(VirtualKey.Control)) mods |= ChordModifiers.Control;
        if (Down(VirtualKey.Shift)) mods |= ChordModifiers.Shift;
        if (Down(VirtualKey.Menu)) mods |= ChordModifiers.Alt;
        if (Down(VirtualKey.LeftWindows) || Down(VirtualKey.RightWindows)) mods |= ChordModifiers.Win;
        var vk = (ushort)e.Key;
        if (e.Key == VirtualKey.Enter && e.KeyStatus.IsExtendedKey) vk = VirtualKeys.NumpadEnter;
        return new KeyPress(CharsFor(e.Key), vk, mods);
    }

    private static bool Down(VirtualKey k) => InputKeyboardSource.GetKeyStateForCurrentThread(k).HasFlag(CoreVirtualKeyStates.Down);

    /// <summary>The key's character ignoring modifiers for the keys the table binds; "" for special keys.</summary>
    private static string CharsFor(VirtualKey k) => k switch
    {
        >= VirtualKey.A and <= VirtualKey.Z => ((char)('a' + (k - VirtualKey.A))).ToString(),
        >= VirtualKey.Number0 and <= VirtualKey.Number9 => ((char)('0' + (k - VirtualKey.Number0))).ToString(),
        >= VirtualKey.NumberPad0 and <= VirtualKey.NumberPad9 => ((char)('0' + (k - VirtualKey.NumberPad0))).ToString(),
        (VirtualKey)VirtualKeys.Oem2Slash => "/",
        (VirtualKey)VirtualKeys.OemComma => ",",
        (VirtualKey)VirtualKeys.OemPeriod => ".",
        _ => "",
    };

    // MARK: rendering

    private void OnModelChanged(object? sender, PropertyChangedEventArgs e) => Render();

    private void OnStoreChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName is nameof(StoreModel.Receipt) or nameof(StoreModel.Now) or nameof(StoreModel.Ask)) RenderReceipt();
    }

    private void Render()
    {
        if (model is null) return;
        var text = model.FieldText;
        if (field.Text != text)
        {
            settingText = true;
            field.Text = text;
            field.SelectionStart = text.Length;
            settingText = false;
        }
        field.PlaceholderText = model.Placeholder;
        field.IsReadOnly = model.Live;
        field.Foreground = model.Live && model.Text.Length == 0 ? sand : ink;
        AutomationProperties.SetName(field, model.Live ? $"Dictating: {model.FieldText}" : "Ask theuth");
        mic.Foreground = model.Recording ? carnelian : ink2;
        var scale = model.Recording ? 1 + Math.Min(1, model.Level) * 0.5 : 1;
        micScale.ScaleX = scale;
        micScale.ScaleY = scale;
        AutomationProperties.SetName(mic, model.Recording ? "Listening" : "Hold the hotkey to dictate");
        ((TextBlock)holdHint.Children[0]).Text = model.HoldHint;
        status.Text = model.Status ?? "";
        status.Foreground = model.Recording ? carnelian : sand;
        privacyLink.Visibility = model.Status == VoiceRecorder.BlockedStatus ? Visibility.Visible : Visibility.Collapsed;
        RenderReceipt();
    }

    private void RenderReceipt()
    {
        ((TextBlock)escHint.Children[1]).Text = store.HeldNow is null ? "cancel" : "undo";
        var lines = AskBarModel.ReceiptLines(store.Receipt, store.HoldRemaining);
        if (lines is not { } l)
        {
            if (receiptRow.Visibility == Visibility.Visible)
            {
                receiptRow.Visibility = Visibility.Collapsed;
                Fit();
            }
            return;
        }
        receiptText.Text = l.Text;
        receiptText.Foreground = store.Receipt switch
        {
            StoreModel.ReceiptRow.Sent => malachite,
            StoreModel.ReceiptRow.Stale => sand,
            StoreModel.ReceiptRow.Failed => carnelian,
            _ => ink,
        };
        receiptTrailing.Text = l.Trailing;
        receiptTrailing.Visibility = l.Trailing.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        if (store.Receipt is StoreModel.ReceiptRow.Holding && store.UndoDelay > 0)
        {
            holdBar.Width = Math.Max(0, Math.Min(Width, Width * store.HoldRemaining / store.UndoDelay));
            holdBar.Visibility = Visibility.Visible;
        }
        else
        {
            holdBar.Visibility = Visibility.Collapsed;
        }
        AutomationProperties.SetName(receiptText, $"Receipt: {l.Text}");
        if (receiptRow.Visibility != Visibility.Visible)
        {
            receiptRow.Visibility = Visibility.Visible;
            Fit();
        }
    }

    // MARK: placement (top-centre of the display under the mouse, ≈ 20 % down, like the Attention panel's `.mouse`)

    private int Physical(double epx) => (int)Math.Round(epx * Win32.GetDpiForWindow(hwnd) / 96.0);

    private void Place()
    {
        var area = Win32.GetCursorPos(out var cursor)
            ? DisplayArea.GetFromPoint(new PointInt32(cursor.X, cursor.Y), DisplayAreaFallback.Primary)
            : DisplayArea.Primary;
        var wa = area.WorkArea;
        int w = Physical(Width);
        int h = Math.Max(AppWindow.Size.Height, Physical(72));
        int x = wa.X + (wa.Width - w) / 2;
        int y = wa.Y + (int)(wa.Height * 0.2);
        x = Math.Min(Math.Max(wa.X + 8, x), wa.X + wa.Width - w - 8);
        y = Math.Min(Math.Max(wa.Y + 8, y), wa.Y + wa.Height - h - 8);
        AppWindow.MoveAndResize(new RectInt32(x, y, w, h));
    }

    /// <summary>Size the window to its content (the receipt row comes and goes).</summary>
    private void Fit()
    {
        if (!contentLoaded || root.XamlRoot is null) return;
        root.Measure(new Windows.Foundation.Size(Width, double.PositiveInfinity));
        var h = Physical(Math.Max(72, root.DesiredSize.Height));
        if (Math.Abs(AppWindow.Size.Height - h) > 1) AppWindow.Resize(new SizeInt32(Physical(Width), h));
    }

    // MARK: brushes and small views

    private static SolidColorBrush Solid(uint rgb) => new(ColorHelper.FromArgb(0xFF, (byte)(rgb >> 16), (byte)(rgb >> 8), (byte)rgb));

    private static SolidColorBrush Translucent(uint rgb, double opacity) => new(ColorHelper.FromArgb((byte)(opacity * 255), (byte)(rgb >> 16), (byte)(rgb >> 8), (byte)rgb));

    private StackPanel Hint(string key, string words)
    {
        var panel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
        panel.Children.Add(new TextBlock { Text = key, FontSize = 10.5, FontFamily = new FontFamily("Consolas"), Foreground = ink2 });
        panel.Children.Add(new TextBlock { Text = words, FontSize = 10.5, Foreground = sand });
        return panel;
    }
}
