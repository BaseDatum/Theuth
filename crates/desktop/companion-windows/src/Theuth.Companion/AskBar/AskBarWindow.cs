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

    /// <summary>189w D4: the markdown reply grows to this height (epx), then scrolls inside the bar.</summary>
    public const double ReplyMaxHeight = 320;

    private readonly Store store;
    private readonly DispatcherQueue dispatcher;
    private readonly VoiceRecorder recorder;
    private readonly Grid root;
    private readonly TextBox field;
    private readonly FontIcon mic;
    private readonly Grid micSlot; // 149 WW2 (D8): the mic glyph, or the accent level bars while recording
    private readonly TextBlock liveView; // 149 WW2 (§3.5): finals in primary text, the open partial in tertiary, while the dictation owns the field
    private readonly ScaleTransform micScale = new() { CenterX = 8, CenterY = 8 };
    private readonly Run escWords;
    private readonly Run holdKey;
    private readonly TextBlock status;
    private readonly HyperlinkButton privacyLink;
    private readonly Border receiptRow;
    private readonly TextBlock receiptText;
    private readonly TextBlock receiptTrailing;
    private readonly MarkdownView replyView; // 189w D4: the desk's reply as markdown, updated in place as text arrives
    private readonly ScrollViewer replyScroll;
    private bool followTail = true;
    private readonly Button speakingGlyph; // spoken replies: Segoe Fluent Volume (E767) while the reply is read aloud; click stops
    private readonly Rectangle holdBar;
    // 149 NATIVE: the system's theme brushes, read live for the root's actual theme (no palette, no startup snapshot).
    private Brush ink => Theme.Brush(NativeLook.TextPrimary, root.ActualTheme);
    private Brush ink2 => Theme.Brush(NativeLook.TextSecondary, root.ActualTheme);
    private Brush sand => Theme.Brush(NativeLook.TextTertiary, root.ActualTheme);
    private Brush accent => Theme.Brush(NativeLook.Accent, root.ActualTheme);
    private Brush accentText => Theme.Brush(NativeLook.AccentText, root.ActualTheme); // R1: accent-coloured text/glyphs
    private Brush critical => Theme.Brush(NativeLook.Critical, root.ActualTheme);
    private Brush success => Theme.Brush(NativeLook.Success, root.ActualTheme);
    private Brush rule => Theme.Brush(NativeLook.Divider, root.ActualTheme);
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

        // Desktop Acrylic (Window.SystemBackdrop, the framework's own controller) when supported — 149 NATIVE: nothing painted over
        // it; without it the theme's SolidBackgroundFillColorBase.
        if (DesktopAcrylicController.IsSupported()) SystemBackdrop = new DesktopAcrylicBackdrop();

        root = new Grid { Width = Width, Background = Theme.Transparent };
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
        var cursorGlyph = Theme.Icon("\uE8E9", ElementTheme.Default, NativeLook.TextTertiary, 12); // MDL2/Fluent "Edit"-style prompt glyph
        Grid.SetColumn(cursorGlyph, 0);
        field = new TextBox
        {
            PlaceholderText = "Ask theuth…",
            FontSize = 14,
            BorderThickness = new Thickness(0),
            Background = Theme.Transparent,
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
        mic = Theme.Icon(NativeLook.Glyph.Microphone, ElementTheme.Default, NativeLook.TextSecondary, 14);
        mic.RenderTransform = micScale;
        AutomationProperties.SetAutomationId(mic, "ask-mic");
        AutomationProperties.SetName(mic, "Hold the hotkey to dictate");
        micSlot = new Grid { Width = 44, Height = 24, VerticalAlignment = VerticalAlignment.Center, HorizontalAlignment = HorizontalAlignment.Right };
        micSlot.Children.Add(mic);
        Grid.SetColumn(micSlot, 2);
        liveView = new TextBlock { FontSize = 14, TextWrapping = TextWrapping.Wrap, VerticalAlignment = VerticalAlignment.Center, Visibility = Visibility.Collapsed, Padding = new Thickness(0, 4, 0, 4) };
        AutomationProperties.SetAutomationId(liveView, "ask-live");
        AutomationProperties.SetLiveSetting(liveView, Microsoft.UI.Xaml.Automation.Peers.AutomationLiveSetting.Polite);
        Grid.SetColumn(liveView, 1);
        fieldRow.Children.Add(cursorGlyph);
        fieldRow.Children.Add(field);
        fieldRow.Children.Add(liveView);
        fieldRow.Children.Add(micSlot);
        Grid.SetRow(fieldRow, 0);

        // Row 1: hints · status — one wrapping line of runs (Ctrl+Shift+Space is longer than ⌃⇧Space; 480 epx holds it on two lines)
        var hints = new Grid { Padding = new Thickness(14, 0, 14, 9), ColumnSpacing = 12 };
        hints.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        hints.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var hintText = new TextBlock { FontSize = 12, Foreground = sand, TextWrapping = TextWrapping.Wrap, MaxLines = 2, VerticalAlignment = VerticalAlignment.Center };
        AutomationProperties.SetAutomationId(hintText, "ask-hints");
        // W6 (C12): no fixed Name — Narrator reads the hint text itself (a TextBlock's Name falls back to its Text).
        holdKey = Key("hold Ctrl+Shift+Space");
        escWords = Words(" keeps editing");
        // 149 §3.5 hint row: "hold Ctrl+Shift+Space to talk · release to send to theuth in 3 s · Esc keeps editing · theuth routes it · Enter sends"
        hintText.Inlines.Add(holdKey);
        hintText.Inlines.Add(Words(" to talk · release to send to theuth in 3 s · "));
        hintText.Inlines.Add(Key("Esc"));
        hintText.Inlines.Add(escWords);
        hintText.Inlines.Add(Words(" · theuth routes it · "));
        hintText.Inlines.Add(Key("Enter"));
        hintText.Inlines.Add(Words(" sends"));
        Grid.SetColumn(hintText, 0);
        var statusPanel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
        status = new TextBlock { FontSize = 12, Foreground = sand, TextTrimming = TextTrimming.CharacterEllipsis, MaxLines = 1, MaxWidth = 200, VerticalAlignment = VerticalAlignment.Center };
        AutomationProperties.SetAutomationId(status, "ask-status");
        AutomationProperties.SetLiveSetting(status, Microsoft.UI.Xaml.Automation.Peers.AutomationLiveSetting.Polite); // W6 (C12): read as its text, announced when it changes
        privacyLink = new HyperlinkButton { Content = "Settings › Privacy", FontSize = 12, Padding = new Thickness(2, 0, 2, 0), Visibility = Visibility.Collapsed, NavigateUri = new Uri(VoiceRecorder.PrivacySettingsUri) };
        AutomationProperties.SetAutomationId(privacyLink, "ask-privacy-link");
        statusPanel.Children.Add(status);
        statusPanel.Children.Add(privacyLink);
        Grid.SetColumn(statusPanel, 1);
        hints.Children.Add(hintText);
        hints.Children.Add(statusPanel);
        Grid.SetRow(hints, 1);

        // Row 2: the receipt row (Swift ReceiptView, 480 wide)
        receiptRow = new Border { BorderBrush = rule, BorderThickness = new Thickness(0, 1, 0, 0), Visibility = Visibility.Collapsed };
        var receiptGrid = new Grid { Padding = new Thickness(12, 8, 12, 8), ColumnSpacing = 8 };
        receiptGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        receiptGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        receiptGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        receiptGrid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        receiptGrid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        receiptText = new TextBlock { FontSize = 12, Foreground = ink, TextWrapping = TextWrapping.Wrap, MaxLines = 2, TextTrimming = TextTrimming.CharacterEllipsis };
        AutomationProperties.SetAutomationId(receiptText, "receipt-row");
        AutomationProperties.SetName(receiptText, "Receipt");
        Grid.SetColumn(receiptText, 0);
        receiptTrailing = new TextBlock { FontSize = 12, Foreground = ink2, VerticalAlignment = VerticalAlignment.Top };
        AutomationProperties.SetAutomationId(receiptTrailing, "receipt-hint");
        Grid.SetColumn(receiptTrailing, 1);
        receiptGrid.Children.Add(receiptText);
        receiptGrid.Children.Add(receiptTrailing);
        // 189w D4: row 1 of the receipt is the reply itself, rendered as markdown (≤ ReplyMaxHeight epx, then it scrolls and follows the tail).
        replyView = new MarkdownView(() => root.ActualTheme, 13);
        AutomationProperties.SetAutomationId(replyView, "ask-reply");
        replyScroll = new ScrollViewer
        {
            Content = replyView, MaxHeight = ReplyMaxHeight, Visibility = Visibility.Collapsed, Padding = new Thickness(0, 6, 10, 0),
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
        };
        AutomationProperties.SetAutomationId(replyScroll, "ask-reply-scroll");
        Grid.SetRow(replyScroll, 1);
        Grid.SetColumnSpan(replyScroll, 3);
        receiptGrid.Children.Add(replyScroll);
        replyView.SizeChanged += (_, _) =>
        {
            if (followTail) replyScroll.ChangeView(null, replyScroll.ExtentHeight, null, disableAnimation: true);
        };
        replyScroll.ViewChanged += (_, e) =>
        {
            if (!e.IsIntermediate) followTail = replyScroll.VerticalOffset >= replyScroll.ScrollableHeight - 24; // the user scrolled up: stop following
        };
        speakingGlyph = new Button
        {
            Content = new FontIcon { Glyph = "\uE767", FontSize = 14 }, Width = 28, Height = 24, Padding = new Thickness(0), BorderThickness = new Thickness(0),
            Background = Theme.Transparent, VerticalAlignment = VerticalAlignment.Top, Visibility = Visibility.Collapsed, IsTabStop = false,
        };
        ToolTipService.SetToolTip(speakingGlyph, "Speaking — click to stop");
        AutomationProperties.SetAutomationId(speakingGlyph, "ask-speaking");
        AutomationProperties.SetName(speakingGlyph, "Speaking — click to stop");
        speakingGlyph.Click += (_, _) => store.Speaker.Stop(SpeechStop.Glyph);
        Grid.SetColumn(speakingGlyph, 2);
        receiptGrid.Children.Add(speakingGlyph);
        store.Speaker.Changed += RenderSpeaking;
        holdBar = new Rectangle { Height = 2, Fill = accent, HorizontalAlignment = HorizontalAlignment.Left, VerticalAlignment = VerticalAlignment.Bottom, Width = 0 };
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
        Theme.Follow(root, Retheme);
        Retheme();
        Place(new RectInt32(0, 0, Physical(Width), Physical(96)));
        Capsule(Physical(96));

        recorder.OnChunk = c => model?.Feed(c);
        recorder.OnFailed = why => model?.MicrophoneFailed(why);
        store.PropertyChanged += OnStoreChanged;
    }

    /// <summary>The model this window renders (set once by <see cref="AskBarInstall"/>; the model needs the surface first).</summary>
    /// <summary>153 W153B probe evidence: the bar's content rendered to a PNG (RenderTargetBitmap — independent of the desktop's capture).</summary>
    public async Task<bool> CapturePng(string path)
    {
        var before = root.Background;
        try
        {
            // 189w: the acrylic sits behind a transparent tree; for the file only, the theme's solid base goes behind the text.
            root.Background = Theme.Brush(NativeLook.SolidBackgroundBase, root.ActualTheme);
            await Task.Delay(80);
            await Ui.XamlPng.Write(root, path);
            return true;
        }
        catch (Exception e)
        {
            Log.Write($"ask bar: capture {path} failed: {e.GetType().Name}: {e.Message}");
            return false;
        }
        finally
        {
            root.Background = before;
        }
    }

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
        if (e.Key == VirtualKey.Escape && store.Speaker.IsSpeaking) store.Speaker.Stop(SpeechStop.Esc); // Esc cuts a reply being read aloud (then does its usual work)
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

    /// <summary>149 NATIVE: re-paint the elements built in the constructor with the live theme's brushes (the system switched light/dark).</summary>
    private void Retheme()
    {
        root.Background = SystemBackdrop is null ? Theme.Brush(NativeLook.SolidBackgroundBase, root.ActualTheme) : Theme.Transparent;
        receiptRow.BorderBrush = rule;
        receiptText.Foreground = ink;
        receiptTrailing.Foreground = ink2;
        replyView.Retheme();
        holdBar.Fill = accent;
        if (holdKey is not null) holdKey.Foreground = ink2;
        if (escWords is not null) escWords.Foreground = sand;
        Render();
        RenderReceipt();
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
        mic.Foreground = model.Recording ? accentText : ink2; // recording = needs you = accent (text resource)
        var scale = model.Recording ? 1 + Math.Min(1, model.Level) * 0.5 : 1;
        micScale.ScaleX = scale;
        micScale.ScaleY = scale;
        AutomationProperties.SetName(mic, model.Recording ? "Listening" : "Hold the hotkey to dictate");
        // 149 WW2 (D8): N accent bars from DictationLevel.levels replace the mic glyph while recording (mirrored in the rail's actions pill).
        micSlot.Children.Clear();
        if (model.Recording)
        {
            var bars = Ui.PillGlyphs.LevelBars(store.Ask.Dictation.Levels, 22, 4, root.ActualTheme);
            AutomationProperties.SetAutomationId(bars, "ask-levels");
            AutomationProperties.SetName(bars, "Listening");
            micSlot.Children.Add(bars);
        }
        else micSlot.Children.Add(mic);
        // 149 WW2 (§3.5): while the dictation owns the field, finals in ink and the open partial in sand (a TextBox cannot colour a range).
        if (model.Live && (model.Text.Length > 0 || model.Partial.Length > 0))
        {
            liveView.Inlines.Clear();
            if (model.Text.Length > 0) liveView.Inlines.Add(new Run { Text = model.Text, Foreground = ink });
            if (model.Partial.Length > 0) liveView.Inlines.Add(new Run { Text = (model.Text.Length > 0 ? " " : "") + model.Partial, Foreground = sand });
            liveView.Visibility = Visibility.Visible;
            field.Visibility = Visibility.Collapsed;
        }
        else
        {
            liveView.Visibility = Visibility.Collapsed;
            field.Visibility = Visibility.Visible;
        }
        holdKey.Text = model.HoldHint;
        status.Text = model.Status ?? "";
        status.Foreground = model.Recording ? accentText : sand;
        privacyLink.Visibility = model.Status == VoiceRecorder.BlockedStatus ? Visibility.Visible : Visibility.Collapsed;
        RenderReceipt();
        Fit();
    }

    private void RenderSpeaking()
    {
        bool speaking = store.Speaker.IsSpeaking;
        speakingGlyph.Visibility = speaking ? Visibility.Visible : Visibility.Collapsed;
        if (speakingGlyph.Content is FontIcon fi) fi.Foreground = accentText;
        if (speaking && receiptRow.Visibility != Visibility.Visible && store.Receipt is not null) RenderReceipt();
    }

    private void RenderReceipt()
    {
        escWords.Text = store.HeldNow is null ? " keeps editing" : " undo";
        var lines = AskBarModel.ReceiptLines(store.Receipt, store.HoldRemaining);
        if (lines is not { } l)
        {
            if (receiptRow.Visibility == Visibility.Visible)
            {
                receiptRow.Visibility = Visibility.Collapsed;
                HideReply();
                Fit();
            }
            return;
        }
        // 189w D4: a desk reply with text is drawn as markdown under a one-line header; every other receipt keeps its one-line row.
        if (AskBarModel.ReplyMarkdown(store.Receipt) is { } reply && store.Receipt is StoreModel.ReceiptRow.Reply replyRow)
        {
            receiptText.Text = AskBarModel.ReplyHeader(replyRow);
            if (replyScroll.Visibility != Visibility.Visible)
            {
                followTail = true;
                replyScroll.Visibility = Visibility.Visible;
            }
            var diff = replyView.Set(reply.Markdown, reply.Streaming);
            ReplyRendered?.Invoke($"{reply.Markdown.Length} chars → {replyView.Children.Count} blocks: kept {diff.Kept}, updated {diff.Updated}, built {diff.Built}");
        }
        else
        {
            receiptText.Text = l.Text;
            HideReply();
        }
        receiptText.Foreground = store.Receipt switch
        {
            StoreModel.ReceiptRow.Sent => success,
            StoreModel.ReceiptRow.Stale => sand,
            StoreModel.ReceiptRow.Failed => critical,
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
        Fit(); // the row grows to two lines as the reply streams — and, for a markdown reply, line by line up to ReplyMaxHeight
    }

    /// <summary>189w probe: one line per markdown render — how many blocks a chunk left alone, updated in place and built.</summary>
    public event Action<string>? ReplyRendered;

    private void HideReply()
    {
        if (replyScroll.Visibility != Visibility.Visible) return;
        replyScroll.Visibility = Visibility.Collapsed;
        replyView.Reset();
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
        Capsule(h);
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
            Capsule(h);
        }
    }

    /// <summary>149 §3.5 / §7.4 #2: the bar is a 28 epx-radius capsule (<c>SetWindowRgn</c>); a regioned window has no DWM shadow (as-built WW2 §Deviations).</summary>
    private void Capsule(int physicalHeight)
    {
        int r = Math.Min(Physical(28), physicalHeight / 2);
        if (!Interop.Shape.Set(Hwnd, new[] { new Interop.Shape.Blob(0, 0, Physical(Width), physicalHeight, r) })) Log.Write($"ask bar: SetWindowRgn failed {Win32.LastError()}");
    }

    // MARK: brushes and small views



    private Run Key(string text) => new() { Text = NativeLook.KeyCapText(text), Foreground = ink2 };

    private Run Words(string text) => new() { Text = text, Foreground = sand };
}