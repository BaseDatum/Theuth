using System.ComponentModel;
using Microsoft.UI;
using Microsoft.UI.Composition.SystemBackdrops;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Theuth.Companion.Ui;
using Theuth.Companion.Voice;
using Windows.Graphics;
using Windows.System;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.WindowsAndMessaging;
using DispatcherQueue = Microsoft.UI.Dispatching.DispatcherQueue;
using DispatcherQueueTimer = Microsoft.UI.Dispatching.DispatcherQueueTimer;

namespace Theuth.Companion.AskBar;

/// <summary>
/// The Ask bar (design 134 §4.3, D7; mock round 2 frame 4): a <see cref="ToolWindow"/> like the Attention panel — 480 epx,
/// borderless, topmost, hidden from Alt+Tab, acrylic, excluded from capture unless <c>--shareable</c> — at the top-centre of
/// the display under the mouse: the field "Ask theuth…" (<c>AutomationId="ask-field"</c>), the mic glyph, the hints row and
/// the receipt row (<c>receipt-row</c>). It is <see cref="AskBarModel"/>'s surface: the model decides, this renders;
/// <see cref="VoiceRecorder"/> is its microphone. Summoned it takes the foreground (the WM_HOTKEY grant makes that
/// legitimate) and on Esc hands it back to the window it took it from (D5, spike S3); losing the foreground closes it
/// (Swift: <c>didResignKey</c>), except under a probe. Keys go through the Ask-bar scope of the key table (Enter sends, Esc
/// undoes then closes, Ctrl+O opens the desk's session, Ctrl+/ the legend); the live dictation owns the field until it
/// lands (finals, then the open partial after a space — a TextBox cannot mute a range, so the partial is told apart by the
/// status "listening…" and the mic pulse).
/// </summary>
internal sealed partial class AskBarWindow : ToolWindow, IAskBarSurface
{
    public const int Width = 480;

    private readonly Store store;
    private readonly DispatcherQueue dispatcher;
    private readonly VoiceRecorder recorder;
    private readonly Grid root;
    private readonly TextBox field;
    private readonly FontIcon mic;
    private readonly ScaleTransform micScale = new() { CenterX = 8, CenterY = 8 };
    private readonly Run escWords;
    private readonly Run holdKey;
    private readonly TextBlock status;
    private readonly HyperlinkButton privacyLink;
    private readonly Border receiptRow;
    private readonly TextBlock receiptText;
    private readonly TextBlock receiptTrailing;
    private readonly Rectangle holdBar;
    private readonly Brush ink, ink2, sand, carnelian, malachite, rule;
    private AskBarModel? model;
    private HWND previousForeground;
    private bool settingText;
    private DispatcherQueueTimer? pressPoll;
    private int pressVk;
    private bool contentLoaded;

    public AskBarWindow(Store store, DispatcherQueue dispatcher)
        : base("Ask theuth", default, excludeFromCapture: !Flags.Shareable)
    {
        this.store = store;
        this.dispatcher = dispatcher;
        recorder = new VoiceRecorder(dispatcher);

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
            // TextChanged is raised asynchronously after a programmatic set, so the flag alone is not enough: a text equal to
            // the model's is ours, not the user's (seen live: a dictation partial echoed back as an edit grew the field 5×).
            if (settingText || model is null || field.Text == model.FieldText) return;
            model.FieldEdited(field.Text);
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

        // Row 1: hints · status — one wrapping line of runs (Ctrl+Shift+Space is longer than ⌃⇧Space; 480 epx holds it on two lines)
        var hints = new Grid { Padding = new Thickness(14, 0, 14, 9), ColumnSpacing = 12 };
        hints.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        hints.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var hintText = new TextBlock { FontSize = 10.5, Foreground = sand, TextWrapping = TextWrapping.Wrap, MaxLines = 2, VerticalAlignment = VerticalAlignment.Center };
        AutomationProperties.SetAutomationId(hintText, "ask-hints");
        // W6 (C12): no fixed Name — Narrator reads the hint text itself (a TextBlock's Name falls back to its Text).
        holdKey = Key("hold Ctrl+Shift+Space");
        escWords = Words(" cancel");
        hintText.Inlines.Add(Key("⏎"));
        hintText.Inlines.Add(Words(" send to theuth · it routes    "));
        hintText.Inlines.Add(holdKey);
        hintText.Inlines.Add(Words(" to talk · release to send in 3 s    "));
        hintText.Inlines.Add(Key("Esc"));
        hintText.Inlines.Add(escWords);
        Grid.SetColumn(hintText, 0);
        var statusPanel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
        status = new TextBlock { FontSize = 10.5, Foreground = sand, TextTrimming = TextTrimming.CharacterEllipsis, MaxLines = 1, MaxWidth = 200, VerticalAlignment = VerticalAlignment.Center };
        AutomationProperties.SetAutomationId(status, "ask-status");
        AutomationProperties.SetLiveSetting(status, Microsoft.UI.Xaml.Automation.Peers.AutomationLiveSetting.Polite); // W6 (C12): read as its text, announced when it changes
        privacyLink = new HyperlinkButton { Content = "Settings › Privacy", FontSize = 10.5, Padding = new Thickness(2, 0, 2, 0), Visibility = Visibility.Collapsed, NavigateUri = new Uri(VoiceRecorder.PrivacySettingsUri) };
        AutomationProperties.SetAutomationId(privacyLink, "ask-privacy-link");
        statusPanel.Children.Add(status);
        statusPanel.Children.Add(privacyLink);
        Grid.SetColumn(statusPanel, 1);
        hints.Children.Add(hintText);
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
        Place(new RectInt32(0, 0, Physical(Width), Physical(96)));

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

    // MARK: ToolWindow

    /// <summary>Alt+F4: close like Esc, never destroy the window.</summary>
    protected override void OnCloseRequested() => model?.Close(returnFocus: true);

    /// <summary>Losing the foreground closes the bar (Swift's <c>didResignKey</c>); a probe keeps it (the harness activates other windows).</summary>
    protected override void OnActivate(bool active, HWND other)
    {
        if (active || model is not { IsOpen: true, Automated: false }) return;
        if (!IsShown) return;
        dispatcher.TryEnqueue(() =>
        {
            if (model is { IsOpen: true } && !IsForeground) model.Close(returnFocus: false);
        });
    }

    // MARK: IAskBarSurface

    public void Show(bool activate)
    {
        if (activate && !IsShown)
        {
            var fg = PInvoke.GetForegroundWindow();
            previousForeground = fg == Hwnd || Win32.IsOurs(fg) ? default : fg;
        }
        PlaceAtMouse();
        if (activate)
        {
            if (!ShowActive()) Log.Write("ask bar: Windows refused the foreground");
            field.Focus(FocusState.Programmatic);
            field.SelectionStart = field.Text.Length;
        }
        else
        {
            ShowPassive();
        }
        Fit();
    }

    public void Hide(bool returnFocus)
    {
        StopPressPoll();
        HideWindow();
        var prev = previousForeground;
        previousForeground = default;
        if (returnFocus && !prev.IsNull) Win32.Foreground(prev);
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

    public void OpenUrl(Uri url) => _ = Links.Open(url);

    // MARK: the hotkey (W2's RegisterHotKey delivers the key-down; the release is seen here or reported by W2's Hotkeys.Released)

    /// <summary>
    /// The hotkey went down and the press is the Ask bar's (W2 routes it here when nothing needs you — see
    /// <see cref="AskBarInstall"/>). The key-up never reaches <c>WM_HOTKEY</c>, so the release is observed: the window's own
    /// KeyUp once it has the foreground, a 50 ms poll of the key's physical state before that, or W2's
    /// <c>Hotkeys.Released</c> — whichever comes first; the rest are no-ops.
    /// </summary>
    public void HotkeyDown(ushort keyCode = VirtualKeys.Space)
    {
        if (model is null) return;
        pressVk = keyCode;
        model.HotkeyDown();
        if (!model.Automated) StartPressPoll();
    }

    /// <summary>The key came up (idempotent).</summary>
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
            if (pressVk != 0 && (PInvoke.GetAsyncKeyState(pressVk) & 0x8000) == 0) HotkeyUp();
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
        if (model.HandleKey(KeyInput.From(e))) e.Handled = true;
    }

    private void OnPreviewKeyUp(object sender, KeyRoutedEventArgs e)
    {
        if (pressVk != 0 && (int)e.Key == pressVk)
        {
            e.Handled = true;
            HotkeyUp();
        }
    }

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
        holdKey.Text = model.HoldHint;
        status.Text = model.Status ?? "";
        status.Foreground = model.Recording ? carnelian : sand;
        privacyLink.Visibility = model.Status == VoiceRecorder.BlockedStatus ? Visibility.Visible : Visibility.Collapsed;
        RenderReceipt();
        Fit();
    }

    private void RenderReceipt()
    {
        escWords.Text = store.HeldNow is null ? " cancel" : " undo";
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
        receiptRow.Visibility = Visibility.Visible;
        Fit(); // the row grows to two lines as the reply streams
    }

    // MARK: placement (top-centre of the display under the mouse, ≈ 20 % down, like the Attention panel's `.mouse`)

    private int Physical(double epx) => (int)Math.Round(epx * Scale);

    private void PlaceAtMouse()
    {
        var cursor = Win32.Cursor();
        var area = DisplayArea.GetFromPoint(new PointInt32(cursor.X, cursor.Y), DisplayAreaFallback.Primary);
        var wa = area.WorkArea;
        int w = Physical(Width);
        int h = Math.Max(AppWindow.Size.Height, Physical(72));
        int x = wa.X + (wa.Width - w) / 2;
        int y = wa.Y + (int)(wa.Height * 0.2);
        x = Math.Min(Math.Max(wa.X + 8, x), wa.X + wa.Width - w - 8);
        y = Math.Min(Math.Max(wa.Y + 8, y), wa.Y + wa.Height - h - 8);
        Place(new RectInt32(x, y, w, h));
    }

    /// <summary>Size the window to its content (the receipt row comes and goes and grows to two lines).</summary>
    private void Fit()
    {
        if (!contentLoaded || root.XamlRoot is null) return;
        root.Measure(new Windows.Foundation.Size(Width, double.PositiveInfinity));
        var h = Physical(Math.Max(72, root.DesiredSize.Height));
        if (Math.Abs(AppWindow.Size.Height - h) > 1)
        {
            var p = AppWindow.Position;
            Place(new RectInt32(p.X, p.Y, Physical(Width), h));
        }
    }

    // MARK: brushes and small views

    private static SolidColorBrush Solid(uint rgb) => new(ColorHelper.FromArgb(0xFF, (byte)(rgb >> 16), (byte)(rgb >> 8), (byte)rgb));

    private static SolidColorBrush Translucent(uint rgb, double opacity) => new(ColorHelper.FromArgb((byte)(opacity * 255), (byte)(rgb >> 16), (byte)(rgb >> 8), (byte)rgb));

    private Run Key(string text) => new() { Text = text, FontFamily = new FontFamily("Consolas"), Foreground = ink2 };

    private Run Words(string text) => new() { Text = text, Foreground = sand };
}
