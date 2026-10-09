using System.ComponentModel;
using System.Text;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Theuth.Companion.Rpc;
using Theuth.Companion.Voice;
using Windows.Graphics;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.WindowsAndMessaging;

namespace Theuth.Companion.Ui;

/// <summary>Where the panel opens (design 134 §4.2): above the tray icon, beside the rail mark, or top-centre of the display under the mouse.</summary>
internal enum PanelAnchorKind { Tray, Rail, Mouse }

internal readonly record struct PanelAnchor(PanelAnchorKind Kind, RECT Rect, bool LeftSide = false)
{
    public static PanelAnchor Mouse => new(PanelAnchorKind.Mouse, default);
}

/// <summary>
/// The Attention panel (design 134 §4.2, D5): 372 × ≤ 560 epx, Mica Base (Acrylic Base where Mica is unsupported, the opaque
/// surface when transparency effects are off — live, C13),
/// topmost, <c>TOOLWINDOW | TOPMOST</c> only — keyboard input needs activation, so <b>summoned = activated</b> and Esc hands
/// the foreground back to the window it took it from (C10). Content in the order a person decides (130 §4.2): header →
/// focused card → other items as lines → the Done zone → key hints → the receipt row. Keys are read on <c>PreviewKeyDown</c>
/// through the Core's <see cref="KeyGrammar"/> and the Store's <see cref="Store.Keys"/>; every option is a <c>Button</c>
/// named "&lt;label&gt;, key &lt;n&gt;" with <c>AutomationId</c> <c>option-&lt;n&gt;</c> (A10, C12).
/// UIA: <c>panel</c>, <c>panel-headline</c>, <c>option-&lt;n&gt;</c> / <c>option-open</c>, <c>line-&lt;n&gt;</c>, <c>done-&lt;n&gt;</c>,
/// <c>note-field</c>, <c>reply-field</c>, <c>receipt-row</c>.
/// </summary>
internal sealed partial class AttentionPanelWindow : ToolWindow
{
    public const double PanelWidth = 420; // 149 §3.9 (372 → ≈ 420)
    public const double MaxHeight = 560;
    private const int MaxLines = 6;
    private const int DoneRows = 4;

    private readonly Store store;
    private readonly UserControl host;
    private readonly ScrollViewer scroll;
    private readonly StackPanel content;
    private readonly Border receiptRow;
    private readonly TextBlock receiptText;
    private readonly TextBlock receiptRight;
    private readonly Border receiptBar;
    private readonly TextBox noteBox;
    private readonly TextBox replyBox;
    private Backdrop? backdrop;
    private readonly List<(TextBlock Block, DateTimeOffset Since, string Prefix, string Suffix)> agoLabels = new();
    private readonly DispatcherQueueTimer agoTimer;
    private string renderKey = "";
    private bool renderQueued;
    private HWND previous;
    private PanelAnchor anchor = PanelAnchor.Mouse;
    // 149 WW2: V hold-to-talk into the card's session, P point, X stop.
    private readonly RowTalk talk;
    private bool talkBusy;
    private string? talkError;
    private bool vDown;
    private Border? replyCap;

    /// <summary>P on a card / the ▣ button (§3.3): the composition root closes the panel and runs <c>screen_context</c> (WW3's point-and-say seam).</summary>
    public event Action<Card>? PointRequested;

    public AttentionPanelWindow(Store store)
        : base("theuth — Attention", default, excludeFromCapture: !Flags.Shareable)
    {
        this.store = store;

        content = new StackPanel();
        scroll = new ScrollViewer { Content = content, VerticalScrollBarVisibility = ScrollBarVisibility.Auto, HorizontalScrollMode = ScrollMode.Disabled };
        receiptText = new TextBlock { FontSize = 12, TextTrimming = TextTrimming.CharacterEllipsis, MaxLines = 2, TextWrapping = TextWrapping.Wrap, VerticalAlignment = VerticalAlignment.Center };
        receiptRight = new TextBlock { FontSize = 12, FontFamily = new FontFamily("Consolas"), VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(8, 0, 0, 0) };
        receiptBar = new Border { Height = 2, HorizontalAlignment = HorizontalAlignment.Left, VerticalAlignment = VerticalAlignment.Bottom, Width = 0 };
        var receiptGrid = new Grid { Padding = new Thickness(12, 8, 12, 8) };
        receiptGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        receiptGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(receiptRight, 1);
        receiptGrid.Children.Add(receiptText);
        receiptGrid.Children.Add(receiptRight);
        var receiptStack = new Grid();
        receiptStack.Children.Add(receiptGrid);
        receiptStack.Children.Add(receiptBar);
        receiptRow = new Border { Child = receiptStack, Visibility = Visibility.Collapsed, BorderThickness = new Thickness(0, 1, 0, 0) };
        AutomationProperties.SetAutomationId(receiptRow, "receipt-row");
        AutomationProperties.SetLiveSetting(receiptRow, AutomationLiveSetting.Polite);

        var layout = new Grid();
        layout.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        layout.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        Grid.SetRow(receiptRow, 1);
        layout.Children.Add(scroll);
        layout.Children.Add(receiptRow);
        host = new UserControl { Content = layout, IsTabStop = true, UseSystemFocusVisuals = false };
        AutomationProperties.SetAutomationId(host, "panel");
        AutomationProperties.SetName(host, "Attention panel");
        host.PreviewKeyDown += OnPreviewKeyDown;
        host.ActualThemeChanged += (_, _) =>
        {
            backdrop?.SetTheme(host.ActualTheme);
            if (IsOpen) Render(force: true);
        };
        Content = host;
        // 149 WW2 (§7.4 #2): opaque with DWM rounded corners (the system shadow stays); the W6 backdrop path is kept as-is below.
        if (!Shape.RoundCorners(Hwnd)) Log.Write("panel: DWM rounded corners unavailable (rectangular)");
        // W6 item 2 (§4.2, A7): Mica Base, Acrylic Base where Mica is unsupported, the opaque surface when transparency effects are off.
        ApplyEffects(initial: true);
        if (SystemEffects.Current is { } fx) fx.Changed += () => ApplyEffects(initial: false);

        noteBox = new TextBox { PlaceholderText = "Why it goes back (required)", FontSize = 12.5 };
        AutomationProperties.SetAutomationId(noteBox, "note-field");
        AutomationProperties.SetName(noteBox, "Why it goes back (required)");
        noteBox.KeyDown += (_, e) =>
        {
            if (e.Key != global::Windows.System.VirtualKey.Enter) return;
            e.Handled = true;
            var text = noteBox.Text;
            if (text.Trim().Length == 0) return; // the note is required
            noteBox.Text = "";
            store.SubmitNote(text);
        };
        replyBox = new TextBox { FontSize = 12.5 };
        AutomationProperties.SetAutomationId(replyBox, "reply-field");
        replyBox.KeyDown += (_, e) =>
        {
            if (e.Key != global::Windows.System.VirtualKey.Enter) return;
            e.Handled = true;
            var text = replyBox.Text;
            if (text.Trim().Length == 0) return;
            replyBox.Text = "";
            store.SubmitReply(text);
        };

        talk = new RowTalk(store, DispatcherQueue);
        talk.LiveText += t =>
        {
            if (!talkBusy || replyBox.Parent is null) return;
            replyBox.Text = t;
            replyBox.SelectionStart = t.Length;
        };
        talk.Finished += OnCardTalkFinished;
        host.PreviewKeyUp += OnPreviewKeyUp;
        replyBox.GotFocus += (_, _) => { if (replyCap is { } c) c.Visibility = Visibility.Collapsed; };
        replyBox.LostFocus += (_, _) => { if (replyCap is { } c) c.Visibility = replyBox.Text.Length == 0 ? Visibility.Visible : Visibility.Collapsed; };

        Activated += (_, e) =>
        {
            // Keys reach PreviewKeyDown only with focus inside the panel: take it whenever the window activates.
            if (e.WindowActivationState == WindowActivationState.Deactivated || !IsOpen || ProbeMode) return;
            if (store.NoteFor is null && store.Ask.ReplyFor is null) host.Focus(FocusState.Programmatic);
            else FocusField();
        };
        agoTimer = DispatcherQueue.CreateTimer();
        agoTimer.Interval = TimeSpan.FromSeconds(5);
        agoTimer.IsRepeating = true;
        agoTimer.Tick += (_, _) => RefreshAgo();
        store.PropertyChanged += OnStoreChanged;
    }

    public bool IsOpen { get; private set; }

    /// <summary>--probe: shown without activation and click-through, so nothing can lose focus to it.</summary>
    public bool ProbeMode { get; set; }

    /// <summary>The window the panel took the foreground from (Esc gives it back).</summary>
    public HWND Previous => previous;

    /// <summary>"mica-base" · "acrylic-base" while attached; "opaque" when transparency effects are off or nothing is supported.</summary>
    public string BackdropText => backdrop?.Material ?? "opaque";

    /// <summary>At quit the controller goes before the window: a window destroyed under a live backdrop target faulted in Microsoft.UI.Xaml.dll (build-B, Application Error 1000).</summary>
    protected override void OnClosingForGood()
    {
        backdrop?.Dispose();
        backdrop = null;
    }

    /// <summary>C13 live (W6 item 3): the transparency switch attaches/detaches the material; the animation switch pins the window transitions.</summary>
    private void ApplyEffects(bool initial)
    {
        // W6 (as-built §2): on build-B the MicaController painted Mica LIGHT (#F3F3F3, its light tint) under Theme=Dark with
        // host.ActualTheme=Dark and app=Dark, and the host's 82 % dark surface tint did not show over it (rounds 2–3, 04:55–05:13;
        // round 1's first open at 04:40 was dark) — dark-palette text on a light ground. Until that is understood the panel is
        // W2's opaque surface; `--panel-mica` attaches the controller for a retest.
        bool wantBackdrop = Environment.GetCommandLineArgs().Contains("--panel-mica");
        bool transparency = SystemEffects.TransparencyOn;
        if (wantBackdrop && transparency && backdrop is null)
        {
            backdrop = Backdrop.Attach(this, BackdropKind.MicaBase, Palette.ColorOf(Palette.Surface, host.ActualTheme), host.ActualTheme, "panel");
        }
        else if ((!transparency || !wantBackdrop) && backdrop is not null)
        {
            backdrop.Dispose();
            backdrop = null;
            Log.Write("panel: backdrop detached — opaque surface (transparency effects off)");
        }
        SystemEffects.ApplyTransitions(Hwnd, SystemEffects.AnimationsOn);
        if (initial) return;
        Log.Write($"panel: effects applied — backdrop={BackdropText} animations={(SystemEffects.AnimationsOn ? "on" : "off")}");
        if (IsOpen) Render(force: true);
    }

    /// <summary>Summon: remember who had the foreground, render, place, activate (the WM_HOTKEY / tray / rail grant, B5–B6).</summary>
    public void Open(PanelAnchor at, HWND prev)
    {
        anchor = at;
        previous = prev;
        IsOpen = true;
        Log.Write($"panel theme: host.ActualTheme={host.ActualTheme} app={Application.Current?.RequestedTheme} backdrop={BackdropText}");
        Render(force: true);
        UpdateReceipt();
        agoTimer.Start();
        if (ProbeMode)
        {
            var ex = Win32.ExStyle(Hwnd) | WINDOW_EX_STYLE.WS_EX_NOACTIVATE | WINDOW_EX_STYLE.WS_EX_LAYERED | WINDOW_EX_STYLE.WS_EX_TRANSPARENT;
            Win32.SetExStyle(Hwnd, ex);
            PInvoke.SetLayeredWindowAttributes(Hwnd, default, 255, LAYERED_WINDOW_ATTRIBUTES_FLAGS.LWA_ALPHA);
            ShowPassive();
            Log.Write($"panel shown without activation (probe) at {Win32.Text(ScreenRect)}");
            return;
        }
        bool active = ShowActive();
        host.Focus(FocusState.Programmatic);
        FocusField();
        DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, () =>
        {
            Relayout(); // the first real layout pass may change the height
            if (IsOpen && store.NoteFor is null && store.Ask.ReplyFor is null && host.FocusState == FocusState.Unfocused) host.Focus(FocusState.Programmatic);
        });
        Log.Write($"panel open ({at.Kind}) prev={Win32.Describe(prev)} active={(active ? "true" : "false")} foreground={Win32.Describe(PInvoke.GetForegroundWindow())}");
    }

    /// <summary>Hide; with <paramref name="returnFocus"/> hand the foreground back to <see cref="Previous"/> (Esc, the hotkey).</summary>
    public void Close(bool returnFocus)
    {
        if (!IsOpen) return;
        IsOpen = false;
        agoTimer.Stop();
        store.NoteFor = null;
        store.Ask.ReplyFor = null;
        bool wasForeground = IsForeground;
        HideWindow();
        if (returnFocus && wasForeground)
        {
            bool ok = Win32.Foreground(previous);
            Log.Write($"panel closed — foreground back to {Win32.Describe(previous)} -> {(ok ? "true" : "false")}; now {Win32.Describe(PInvoke.GetForegroundWindow())}");
        }
        else
        {
            Log.Write($"panel closed (returnFocus={(returnFocus ? "true" : "false")}, wasForeground={(wasForeground ? "true" : "false")})");
        }
        previous = default;
    }

    /// <summary>When focus loss last closed it (<c>Environment.TickCount64</c>): a tray click right after is the same gesture — a toggle.</summary>
    public long ClosedByFocusLossAt { get; private set; }

    protected override void OnActivate(bool active, HWND other)
    {
        // Focus loss closes it (Swift: didResignKey → close without returning key): the user chose another window.
        if (active || !IsOpen || ProbeMode) return;
        Close(returnFocus: false);
        ClosedByFocusLossAt = Environment.TickCount64;
    }

    protected override void OnCloseRequested() => Close(returnFocus: true);

    private void OnPreviewKeyDown(object sender, KeyRoutedEventArgs e)
    {
        var press = KeyInput.From(e);
        bool textField = store.NoteFor is not null
            || (host.XamlRoot is { } xr && FocusManager.GetFocusedElement(xr) is TextBox);
        // 149 WW2 (§8 panel ★ rows): V hold-to-talk · P point · X stop (hold) · space = focus the reply field. Letters type while a field has focus.
        if (!textField && CardVerbFor(press) is { } verb)
        {
            e.Handled = true;
            if (verb == CardVerb.Talk && e.KeyStatus.WasKeyDown) return; // auto-repeat while held
            Log.Write($"panel key {press.Chord.Symbol} -> {verb}");
            switch (verb)
            {
                case CardVerb.Talk:
                    vDown = true;
                    if (store.FocusedCard is { } tc) BeginCardTalk(tc);
                    break;
                case CardVerb.Point:
                    if (store.FocusedCard is { } pc && store.FocusedAnatomy is { Reply.Point: true }) PointRequested?.Invoke(pc);
                    break;
                case CardVerb.Stop:
                    if (store.FocusedAnatomy is { } sa) store.StopCardAgent(sa);
                    break;
                case CardVerb.Reply:
                    if (replyBox.Parent is not null) replyBox.Focus(FocusState.Programmatic);
                    break;
            }
            return;
        }
        if (textField && press.Chord == Chord.Escape && host.XamlRoot is { } xr2 && ReferenceEquals(FocusManager.GetFocusedElement(xr2), replyBox) && store.HeldNow is null && store.NoteFor is null)
        {
            // Esc in the always-visible reply field: leave the field first (a second Esc closes the panel).
            e.Handled = true;
            if (talkBusy) { talk.Cancel(); talkBusy = false; }
            host.Focus(FocusState.Programmatic);
            return;
        }
        var intent = KeyGrammar.IntentFor(press, KeyScope.Panel, store.Keys, textField);
        if (intent.Kind == IntentKind.None) return;
        e.Handled = true;
        Log.Write($"panel key {press.Chord.Symbol} -> {intent.Kind}{(intent.Kind == IntentKind.Answer ? $" {intent.Option}" : "")}");
        if (intent.Kind == IntentKind.Reply)
        {
            if (replyBox.Parent is not null) replyBox.Focus(FocusState.Programmatic);
            else store.Perform(intent);
            return;
        }
        store.Perform(intent);
    }

    private enum CardVerb { Talk, Point, Stop, Reply }

    /// <summary>The key table's panel rows for the 149 verbs (WW1's <c>TalkReply</c> · <c>PointAttach</c> · <c>StopAgent</c> · space), else the design's defaults.</summary>
    private CardVerb? CardVerbFor(KeyPress press)
    {
        var chord = press.Chord;
        if (store.Keys.ActionFor(chord, KeyScope.Panel) is { } action)
        {
            switch (action.RawValue())
            {
                case "talkReply": return CardVerb.Talk;
                case "pointAttach": return CardVerb.Point;
                case "stopAgent": return CardVerb.Stop;
            }
        }
        if (chord == Chord.Space) return CardVerb.Reply;
        var c = chord.NormalizedLocal;
        if (c.Modifiers != ChordModifiers.None) return null;
        return c.Chars switch { "v" => CardVerb.Talk, "p" => CardVerb.Point, "x" => CardVerb.Stop, _ => null };
    }

    private void OnPreviewKeyUp(object sender, KeyRoutedEventArgs e)
    {
        if (!vDown) return;
        var press = KeyInput.From(e);
        if (CardVerbFor(press) != CardVerb.Talk && press.Chord.NormalizedLocal.Chars != "v") return;
        vDown = false;
        e.Handled = true;
        EndCardTalk();
    }

    /// <summary>V / the 🎙 button: dictate into the field with the card's session; the release lands the words in the field, ⏎ sends.</summary>
    private void BeginCardTalk(Card card)
    {
        if (talk.Busy || card.Item.Session is not { } s) return;
        if (store.FocusedAnatomy is { Reply.Talk: false }) return;
        talkBusy = true;
        talkError = null;
        talk.Begin(s);
        Render(force: true);
    }

    private void EndCardTalk()
    {
        if (!talk.Busy) return;
        talk.End();
    }

    private void OnCardTalkFinished(string text, string? error)
    {
        talkBusy = false;
        talkError = error;
        if (error is null && text.Length > 0 && replyBox.Parent is not null)
        {
            replyBox.Text = text;
            replyBox.SelectionStart = text.Length;
            replyBox.Focus(FocusState.Programmatic);
        }
        Render(force: true);
        if (error is not null)
        {
            var t = DispatcherQueue.CreateTimer();
            t.Interval = TimeSpan.FromSeconds(5);
            t.IsRepeating = false;
            t.Tick += (_, _) =>
            {
                t.Stop();
                talkError = null;
                if (IsOpen) Render(force: true);
            };
            t.Start();
        }
    }

    private void OnStoreChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName is nameof(Store.Receipt) or nameof(Store.Now))
        {
            if (IsOpen) UpdateReceipt();
            return;
        }
        if (!IsOpen || renderQueued) return;
        renderQueued = true;
        DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, () =>
        {
            renderQueued = false;
            if (IsOpen) Render();
        });
    }

    // MARK: rendering

    private string Key()
    {
        var sb = new StringBuilder();
        var p = store.Panel;
        sb.Append(p.Headline).Append('|').Append(p.Position).Append('|').Append(store.Connection).Append('|').Append(p.Focus).Append('|');
        sb.Append(store.NoteFor).Append('|').Append(store.Ask.ReplyFor).Append('|').Append(store.HeldNow is null).Append('|').Append(store.UndoDelay).Append('|').Append(talkBusy).Append(talkError).Append('|').Append(store.Activity.Count).Append('|');
        if (store.FocusedCard is { } c)
        {
            sb.Append(c.Item.Id).Append('|').Append(c.Question).Append('|').Append(c.Why).Append('|').Append(c.Model).Append('|').Append(c.MoreOptions).Append(c.Clearable);
            foreach (var o in c.Options) sb.Append('|').Append(o.Key).Append(o.Label).Append(o.Detail).Append(o.Recommended);
            foreach (var ch in c.Chips) sb.Append('|').Append(ch.Text).Append(ch.Ok);
        }
        foreach (var i in p.Others.Take(MaxLines + 1)) sb.Append('|').Append(i.Id).Append(i.Title).Append(i.Agent);
        sb.Append('|').Append(p.Others.Count).Append('|').Append(p.ReceiptFocus);
        foreach (var r in store.Ask.Receipts.Entries.Take(DoneRows + 8)) sb.Append('|').Append(r.Id).Append(r.Text);
        sb.Append('|').Append(store.Ask.Receipts.Count);
        return sb.ToString();
    }

    private void Render(bool force = false)
    {
        var key = Key();
        if (!force && key == renderKey)
        {
            RefreshAgo();
            return;
        }
        renderKey = key;
        bool noteFocused = noteBox.FocusState != FocusState.Unfocused;
        bool replyFocused = replyBox.FocusState != FocusState.Unfocused;
        Detach(noteBox);
        Detach(replyBox);
        agoLabels.Clear();
        content.Children.Clear();
        var theme = host.ActualTheme;
        backdrop?.SetTheme(theme);
        // With a material attached the surface is painted at 82 % over it (the material shows through the rest): on build-B the
        // MicaController painted a LIGHT material under Theme=Dark (rounds 2–3, as-built W6 §2), which left dark-palette text
        // on a cream ground — legibility never depends on what the controller paints.
        host.Background = backdrop is null ? Palette.Brush(Palette.Surface, theme) : Palette.Tint(Palette.Surface, theme, 0.82);
        host.Foreground = Palette.Brush(Palette.Ink, theme);
        receiptRow.BorderBrush = Palette.Tint(Palette.Rule, theme, 0.6);

        content.Children.Add(Header(theme));
        content.Children.Add(Divider(theme, 0.6));
        var p = store.Panel;
        if (store.FocusedCard is { } card)
        {
            content.Children.Add(CardView(card, theme));
            var others = p.Others;
            int n = 0;
            foreach (var other in others.Take(MaxLines))
            {
                content.Children.Add(Divider(theme, 0.4));
                content.Children.Add(Line(other, ++n, theme));
            }
            if (others.Count > MaxLines)
            {
                content.Children.Add(Divider(theme, 0.4));
                content.Children.Add(Small($"+{others.Count - MaxLines} more · J to walk them", Palette.Sand, theme, new Thickness(12, 6, 12, 6)));
            }
        }
        else if (p.ReceiptFocus is not null)
        {
            int n = 0;
            foreach (var item in p.Others.Take(MaxLines))
            {
                content.Children.Add(Line(item, ++n, theme));
                content.Children.Add(Divider(theme, 0.4));
            }
            if (p.Others.Count > MaxLines) content.Children.Add(Small($"+{p.Others.Count - MaxLines} more · K to walk them", Palette.Sand, theme, new Thickness(12, 6, 12, 6)));
        }
        else
        {
            // 149 §3.4 / Revision 1 #8: the wordmark glyph · "Nothing needs you." at 24 · the sub-line; "Connecting…" while the engine is away.
            bool open = store.Connection == RpcClient.State.Open;
            var empty = new Grid { Padding = new Thickness(26, 22, 26, 28), ColumnSpacing = 18 };
            empty.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            empty.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            var mark = MarkGlyph.Wordmark(theme);
            mark.Width = 40;
            mark.Height = 40;
            mark.VerticalAlignment = VerticalAlignment.Top;
            if (!open) mark.Opacity = 0.45;
            empty.Children.Add(mark);
            var words = new StackPanel { Spacing = 6 };
            var title = new TextBlock { Text = open ? CardAnatomyModel.EmptyTitle : CardAnatomyModel.Connecting, FontSize = 24, FontWeight = Microsoft.UI.Text.FontWeights.Bold, Foreground = Palette.Brush(Palette.Ink, theme), TextWrapping = TextWrapping.Wrap };
            AutomationProperties.SetAutomationId(title, "panel-empty");
            words.Children.Add(title);
            if (open) words.Children.Add(new TextBlock { Text = CardAnatomyModel.EmptyBody, FontSize = 15, Foreground = Palette.Brush(Palette.Sand, theme), TextWrapping = TextWrapping.Wrap });
            Grid.SetColumn(words, 1);
            empty.Children.Add(words);
            content.Children.Add(empty);
        }
        DoneZone(theme);
        content.Children.Add(Divider(theme, 0.6));
        content.Children.Add(Footer(theme));
        Relayout();
        if (noteFocused || replyFocused || store.NoteFor is not null || store.Ask.ReplyFor is not null) FocusField();
    }

    private void FocusField()
    {
        if (store.NoteFor is not null && noteBox.Parent is not null) noteBox.Focus(FocusState.Programmatic);
        else if (store.Ask.ReplyFor is not null && replyBox.Parent is not null) replyBox.Focus(FocusState.Programmatic);
    }

    private static void Detach(FrameworkElement e)
    {
        if (e.Parent is Panel panel) panel.Children.Remove(e);
    }

    /// <summary>149 §3.3 header row: ‹agent name› (bold; "Inbox" when nothing is focused) · position dots · ‹J K› · ? · ✕ esc, the glance sentence as the sub-line (Revision 1 #7).</summary>
    private FrameworkElement Header(ElementTheme theme)
    {
        var grid = new Grid { Padding = new Thickness(20, 14, 14, 6), ColumnSpacing = 8 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var anatomy = store.FocusedAnatomy;
        var titleStack = new StackPanel { Spacing = 1, VerticalAlignment = VerticalAlignment.Center };
        var nameRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10, VerticalAlignment = VerticalAlignment.Center };
        var name = new TextBlock
        {
            Text = anatomy?.Header.Name ?? CardAnatomyModel.InboxTitle, FontSize = 19, FontWeight = Microsoft.UI.Text.FontWeights.Bold,
            Foreground = Palette.Brush(Palette.Ink, theme), TextTrimming = TextTrimming.CharacterEllipsis, MaxWidth = 200, VerticalAlignment = VerticalAlignment.Center,
        };
        nameRow.Children.Add(name);
        if (anatomy is { } an && an.Header.Count > 1) nameRow.Children.Add(CardChrome.PositionDots(theme, an.Header.Position, an.Header.Count));
        titleStack.Children.Add(nameRow);
        bool gold = store.Rail.NeedsYouCount > 0;
        var glance = new TextBlock { Text = store.Panel.Headline, FontSize = 11.5, Foreground = Palette.Brush(gold ? Palette.Gold : Palette.Ink2, theme), TextTrimming = TextTrimming.CharacterEllipsis };
        titleStack.Children.Add(glance);
        AutomationProperties.SetAutomationId(titleStack, "panel-headline");
        AutomationProperties.SetName(titleStack, store.Panel.Position.Length == 0 ? store.Panel.Headline : $"{anatomy?.Header.Spoken ?? store.Panel.Headline}, {store.Panel.Headline}");
        grid.Children.Add(titleStack);
        if (anatomy is { } a2 && a2.Header.Count > 1)
        {
            var jk = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
            jk.Children.Add(CardChrome.KeyCapButton(theme, null, "J", () => store.Perform(Intent.Next), "card-next"));
            jk.Children.Add(CardChrome.KeyCapButton(theme, null, "K", () => store.Perform(Intent.Previous), "card-previous"));
            Grid.SetColumn(jk, 1);
            grid.Children.Add(jk);
        }
        var legend = CardChrome.KeyCapButton(theme, null, "?", () => store.Perform(Intent.Legend), "card-legend");
        Grid.SetColumn(legend, 2);
        grid.Children.Add(legend);
        var close = CardChrome.KeyCapButton(theme, "✕", "esc", () => store.Perform(Intent.Escape), "card-close");
        Grid.SetColumn(close, 3);
        grid.Children.Add(close);
        return grid;
    }

    /// <summary>
    /// 149 §3.3 (+ Revision 1 #2–3, #6, #18): status line · context bubbles (E6 or the interim join) · the bold question · why ·
    /// chips · numbered options "(Recommended)" · the reply row [▣ P] [field R] [🎙 V] [↑ ⏎] · the footer "Next steps" + pills
    /// (Stop agent X only while the agent runs or waits · Clear E on finished/info · Snooze S on asks).
    /// </summary>
    private FrameworkElement CardView(Card card, ElementTheme theme)
    {
        var anatomy = store.FocusedAnatomy ?? CardAnatomyModel.Make(card, store.Panel.Focus + 1, store.Panel.Items.Count, null, null, null, DateTimeOffset.Now);
        var stack = new StackPanel { Spacing = 9, Padding = new Thickness(20, 6, 20, 12) };

        stack.Children.Add(CardChrome.StatusLine(theme, anatomy.Status, card.Item.Kind.NeedsYou));
        if (anatomy.Bubbles.Any)
        {
            var bubbles = new StackPanel { Spacing = 6 };
            if (anatomy.Bubbles.Request is { } rq) bubbles.Children.Add(CardChrome.Bubble(theme, rq, mine: true));
            if (anatomy.Bubbles.LastReply is { } lr) bubbles.Children.Add(CardChrome.Bubble(theme, lr, mine: false));
            stack.Children.Add(bubbles);
        }

        var question = new TextBlock { Text = anatomy.Question, FontSize = CardChrome.Question, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap, MaxLines = 3, TextTrimming = TextTrimming.CharacterEllipsis, Foreground = Palette.Brush(Palette.Ink, theme) };
        AutomationProperties.SetAutomationId(question, "card-question");
        stack.Children.Add(question);
        if (anatomy.Why is { } why && why != anatomy.Bubbles.LastReply)
        {
            var w = new TextBlock { Text = why, FontSize = 13, Foreground = Palette.Brush(Palette.Ink2, theme), TextWrapping = TextWrapping.Wrap, MaxLines = 4, TextTrimming = TextTrimming.CharacterEllipsis };
            AutomationProperties.SetAutomationId(w, "card-why");
            stack.Children.Add(w);
        }
        if (card.Chips.Count > 0)
        {
            var chips = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
            foreach (var chip in card.Chips)
            {
                var key = chip.Ok == true ? Palette.Malachite : chip.Ok == false ? Palette.Carnelian : Palette.Ink2;
                chips.Children.Add(new Border
                {
                    CornerRadius = new CornerRadius(9), BorderThickness = new Thickness(1), BorderBrush = Palette.Brush(key, theme), Padding = new Thickness(7, 2, 7, 2),
                    Child = new TextBlock { Text = chip.Text, FontSize = 10.5, Foreground = Palette.Brush(key, theme), TextTrimming = TextTrimming.CharacterEllipsis },
                });
            }
            stack.Children.Add(chips);
        }

        if (store.NoteFor is not null)
        {
            stack.Children.Add(noteBox);
            stack.Children.Add(Small("Enter send back · Esc cancel", Palette.Sand, theme, default));
        }
        else
        {
            if (anatomy.NextStepsHeader) stack.Children.Add(Small("Next steps", Palette.Sand, theme, new Thickness(0, 2, 0, -3), 11.5));
            var options = new StackPanel { Spacing = 6 };
            foreach (var line in anatomy.Options)
            {
                var o = line.Option;
                options.Children.Add(CardChrome.OptionRow(theme, line, () => store.Pick(o, card)));
            }
            if (options.Children.Count > 0) stack.Children.Add(options);
            if (anatomy.MoreOptions > 0) stack.Children.Add(Small($"+{anatomy.MoreOptions} more — O opens the full list in theuth", Palette.Sand, theme, default));
            if (card.Options.Count == 0 && !card.Clearable && !anatomy.Reply.Send)
            {
                var open = OptionButton(new CardOption(0, "Open in theuth", null, false, new CardAction.Dismiss(card.Item.Id)), card, theme, keyLabel: "O");
                stack.Children.Add(open);
            }
        }

        if (anatomy.Reply.Send && store.NoteFor is null)
        {
            // The reply row is always visible (§3.3): [▣ P] [field "Reply to ‹name›" R] [🎙 V] [↑ ⏎].
            var row = new Grid { ColumnSpacing = 6 };
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            var point = CardChrome.IconCap(theme, "\uE722", "P", "Point — attach the screen", anatomy.Reply.Point, () => PointRequested?.Invoke(card), "reply-point");
            row.Children.Add(point);
            replyBox.PlaceholderText = anatomy.Reply.Placeholder;
            replyBox.Height = 36;
            replyBox.VerticalAlignment = VerticalAlignment.Center;
            replyBox.CornerRadius = new CornerRadius(12);
            AutomationProperties.SetName(replyBox, anatomy.Reply.Placeholder);
            var fieldWrap = new Grid();
            fieldWrap.Children.Add(replyBox);
            var cap = CardChrome.KeyCap(theme, ReplyRowSpec.KeyCap);
            cap.HorizontalAlignment = HorizontalAlignment.Right;
            cap.Margin = new Thickness(0, 0, 8, 0);
            cap.IsHitTestVisible = false;
            cap.Visibility = replyBox.FocusState == FocusState.Unfocused && replyBox.Text.Length == 0 ? Visibility.Visible : Visibility.Collapsed;
            replyCap = cap;
            fieldWrap.Children.Add(cap);
            Grid.SetColumn(fieldWrap, 1);
            row.Children.Add(fieldWrap);
            var mic = CardChrome.IconCap(theme, "\uE720", "V", talkBusy ? "Listening" : "Hold to talk", anatomy.Reply.Talk, () => { }, "reply-mic");
            if (anatomy.Reply.Talk)
            {
                mic.AddHandler(UIElement.PointerPressedEvent, new PointerEventHandler((s, e) =>
                {
                    ((UIElement)s).CapturePointer(e.Pointer);
                    BeginCardTalk(card);
                }), true);
                mic.AddHandler(UIElement.PointerReleasedEvent, new PointerEventHandler((s, e) =>
                {
                    ((UIElement)s).ReleasePointerCapture(e.Pointer);
                    EndCardTalk();
                }), true);
                mic.AddHandler(UIElement.PointerCaptureLostEvent, new PointerEventHandler((_, _) => EndCardTalk()), true);
                if (talkBusy && mic.Content is StackPanel sp && sp.Children[0] is FontIcon fi) fi.Foreground = Palette.Brush(Palette.Carnelian, theme);
            }
            Grid.SetColumn(mic, 2);
            row.Children.Add(mic);
            var send = CardChrome.IconCap(theme, "\uE74A", "⏎", "Send", true, () =>
            {
                var text = replyBox.Text;
                if (text.Trim().Length == 0) return;
                replyBox.Text = "";
                store.SubmitReply(text);
            }, "reply-send");
            Grid.SetColumn(send, 3);
            row.Children.Add(send);
            AutomationProperties.SetAutomationId(row, "reply-row");
            stack.Children.Add(row);
            if (talkBusy || talkError is not null)
            {
                stack.Children.Add(Small(talkError ?? "Listening… release → the words land in the field", talkError is null ? Palette.Sand : Palette.Carnelian, theme, default));
            }
        }

        if (anatomy.Footer.Count > 0)
        {
            var footer = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, HorizontalAlignment = HorizontalAlignment.Right, Margin = new Thickness(0, 2, 0, 0) };
            foreach (var verb in anatomy.Footer)
            {
                var v = verb;
                footer.Children.Add(CardChrome.FooterPill(theme, v, () => store.RunFooterVerb(anatomy, v)));
            }
            AutomationProperties.SetAutomationId(footer, "card-footer");
            stack.Children.Add(footer);
        }

        var hints = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10 };
        foreach (var k in anatomy.Keys.Where(k => k.Cap is "D" or "O" or "Ctrl+O" or "?" or "Esc")) hints.Children.Add(Hint(k.Cap, k.Label, theme));
        stack.Children.Add(hints);

        var frame = new Grid { Background = Palette.Tint(Palette.Ink, theme, 0.035) };
        frame.Children.Add(stack);
        if (card.Item.Kind.NeedsYou)
        {
            frame.Children.Add(new Border { Width = 3, HorizontalAlignment = HorizontalAlignment.Left, Background = Palette.Brush(Palette.Gold, theme) });
        }
        AutomationProperties.SetAutomationId(frame, "card");
        AutomationProperties.SetName(frame, $"{card.Who}, {card.Item.Kind.RawValue}: {card.Question}");
        return frame;
    }

    private Button OptionButton(CardOption o, Card card, ElementTheme theme, string? keyLabel = null)
    {
        bool rec = o.Recommended;
        var grid = new Grid { ColumnSpacing = 9 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var badge = new Border
        {
            MinWidth = 18, MinHeight = 18, CornerRadius = new CornerRadius(4), Padding = new Thickness(4, 0, 4, 0),
            Background = rec ? Palette.Brush(Palette.Gold, theme) : Palette.Tint(Palette.Ink, theme, 0.08), VerticalAlignment = VerticalAlignment.Center,
            Child = new TextBlock
            {
                Text = keyLabel ?? $"{o.Key}", FontSize = 10.5, FontFamily = new FontFamily("Consolas"),
                Foreground = rec ? Palette.Brush(Palette.Surface, theme) : Palette.Brush(Palette.Ink, theme),
                HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center,
            },
        };
        grid.Children.Add(badge);
        var label = new TextBlock { FontSize = 12.5, TextWrapping = TextWrapping.Wrap, MaxLines = 2, TextTrimming = TextTrimming.CharacterEllipsis, VerticalAlignment = VerticalAlignment.Center };
        label.Inlines.Add(new Run { Text = o.Label, Foreground = Palette.Brush(Palette.Ink, theme) });
        if (o.Detail is { } d) label.Inlines.Add(new Run { Text = $" — {d}", FontSize = 11, Foreground = Palette.Brush(Palette.Sand, theme) });
        Grid.SetColumn(label, 1);
        grid.Children.Add(label);
        if (rec)
        {
            var r = Small("recommended · Enter", Palette.Gold, theme, default, 10);
            r.VerticalAlignment = VerticalAlignment.Center;
            Grid.SetColumn(r, 2);
            grid.Children.Add(r);
        }
        var fill = rec ? Palette.Tint(Palette.Gold, theme, 0.12) : Palette.Tint(Palette.Ink, theme, 0.06);
        var border = rec ? Palette.Brush(Palette.Gold, theme) : Palette.Tint(Palette.Rule, theme, 0.6);
        var b = new Button
        {
            Content = grid, HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch,
            Padding = new Thickness(8, 6, 8, 6), CornerRadius = new CornerRadius(9), BorderThickness = new Thickness(1),
            Background = fill, BorderBrush = border,
        };
        b.Resources["ButtonBackgroundPointerOver"] = rec ? Palette.Tint(Palette.Gold, theme, 0.2) : Palette.Tint(Palette.Ink, theme, 0.1);
        b.Resources["ButtonBackgroundPressed"] = rec ? Palette.Tint(Palette.Gold, theme, 0.28) : Palette.Tint(Palette.Ink, theme, 0.14);
        b.Resources["ButtonBorderBrushPointerOver"] = border;
        b.Resources["ButtonBorderBrushPressed"] = border;
        var key = keyLabel ?? $"{o.Key}";
        AutomationProperties.SetAutomationId(b, keyLabel is null ? $"option-{o.Key}" : "option-open");
        AutomationProperties.SetName(b, $"{o.Label}, key {key}");
        AutomationProperties.SetHelpText(b, rec ? $"Press {key} or Enter" : $"Press {key}");
        if (keyLabel is null) b.Click += (_, _) => store.Pick(o, card);
        else b.Click += (_, _) => store.Perform(Intent.OpenBoard);
        return b;
    }

    private Button Line(AttentionItem item, int n, ElementTheme theme)
    {
        var grid = new Grid { ColumnSpacing = 8 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var glyph = MarkGlyph.ForItem(item, theme);
        glyph.VerticalAlignment = VerticalAlignment.Center;
        grid.Children.Add(glyph);
        var who = new TextBlock { Text = item.Agent ?? item.Kind.RawValue, FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.Medium, VerticalAlignment = VerticalAlignment.Center, Foreground = Palette.Brush(Palette.Ink, theme) };
        Grid.SetColumn(who, 1);
        grid.Children.Add(who);
        var title = Small(item.Title, Palette.Ink2, theme, default, 12);
        title.TextTrimming = TextTrimming.CharacterEllipsis;
        title.VerticalAlignment = VerticalAlignment.Center;
        Grid.SetColumn(title, 2);
        grid.Children.Add(title);
        var ago = Small("", Palette.Sand, theme, default, 12);
        ago.FontFamily = new FontFamily("Consolas");
        ago.VerticalAlignment = VerticalAlignment.Center;
        Ago(ago, item.WaitingSince, "", "");
        Grid.SetColumn(ago, 3);
        grid.Children.Add(ago);
        var b = new Button
        {
            Content = grid, HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch,
            Padding = new Thickness(12, 7, 12, 7), BorderThickness = new Thickness(0), CornerRadius = new CornerRadius(0), Background = Palette.Transparent,
        };
        FlatButtons.Apply(b.Resources, Palette.Tint(Palette.Ink, theme, 0.06));
        AutomationProperties.SetAutomationId(b, $"line-{n}");
        AutomationProperties.SetName(b, $"{item.Agent ?? item.Kind.RawValue}, {item.Title}, {RailModel.Ago(item.WaitingSince, DateTimeOffset.Now)}");
        b.Click += (_, _) => store.FocusOn(item.Id);
        return b;
    }

    /// <summary>"Done · last 24 h": asks handed to theuth and keystroke answers the engine stamped <c>user@companion</c> (E4).</summary>
    private void DoneZone(ElementTheme theme)
    {
        var all = store.Ask.Receipts.Entries;
        if (all.Count == 0) return;
        var focus = store.Panel.ReceiptFocus;
        int start = Math.Max(0, Math.Min((focus ?? 0) - DoneRows + 1, all.Count - DoneRows));
        var rows = all.Select((r, i) => (r, i)).Skip(start).Take(DoneRows).ToList();
        content.Children.Add(Divider(theme, 0.6));
        var head = new Grid { Padding = new Thickness(12, 7, 12, 3) };
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var title = new TextBlock { FontSize = 11 };
        title.Inlines.Add(new Run { Text = "Done", FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Foreground = Palette.Brush(Palette.Malachite, theme) });
        title.Inlines.Add(new Run { Text = " · last 24 h", Foreground = Palette.Brush(Palette.Sand, theme) });
        head.Children.Add(title);
        if (all.Count > rows.Count)
        {
            var more = Small($"{all.Count} · J to walk them", Palette.Sand, theme, default, 10.5);
            Grid.SetColumn(more, 1);
            head.Children.Add(more);
        }
        content.Children.Add(head);
        foreach (var (r, i) in rows)
        {
            var grid = new Grid { ColumnSpacing = 8, Padding = new Thickness(12, 5, 12, 5) };
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            if (focus == i) grid.Background = Palette.Tint(Palette.Ink, theme, 0.045);
            var glyph = MarkGlyph.Create(MarkState.Finished, theme, false);
            glyph.VerticalAlignment = VerticalAlignment.Center;
            grid.Children.Add(glyph);
            var who = new TextBlock { Text = r.Who, FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.Medium, MaxWidth = 140, TextTrimming = TextTrimming.CharacterEllipsis };
            Grid.SetColumn(who, 1);
            grid.Children.Add(who);
            var text = Small(r.Text, Palette.Ink2, theme, default, 12);
            text.TextTrimming = TextTrimming.CharacterEllipsis;
            Grid.SetColumn(text, 2);
            grid.Children.Add(text);
            var ago = Small("", Palette.Sand, theme, default, 12);
            ago.FontFamily = new FontFamily("Consolas");
            Ago(ago, r.At, "", "");
            Grid.SetColumn(ago, 3);
            grid.Children.Add(ago);
            var row = new Grid();
            row.Children.Add(grid);
            if (focus == i) row.Children.Add(new Border { Width = 3, HorizontalAlignment = HorizontalAlignment.Left, Background = Palette.Brush(Palette.Malachite, theme) });
            AutomationProperties.SetAutomationId(row, $"done-{i + 1}");
            AutomationProperties.SetName(row, $"{r.Who}, {r.Text}, {RailModel.Ago(r.At, DateTimeOffset.Now)}");
            AutomationProperties.SetAccessibilityView(row, AccessibilityView.Content); // W6 (C12): a row Narrator lands on, not a layout pane
            content.Children.Add(row);
        }
        if (focus is not null)
        {
            var hints = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10, Padding = new Thickness(12, 5, 12, 5) };
            hints.Children.Add(Hint("E", "clear", theme));
            if (store.FocusedReceipt?.Session is not null) hints.Children.Add(Hint("Ctrl+O", "session", theme));
            content.Children.Add(hints);
        }
    }

    private FrameworkElement Footer(ElementTheme theme)
    {
        // Two lines: Segoe UI at these sizes does not fit the hints and the hold sentence side by side in 372 epx.
        var stack = new StackPanel { Padding = new Thickness(12, 7, 12, 7), Spacing = 4 };
        var hints = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10 };
        hints.Children.Add(Hint("J K", "move", theme));
        hints.Children.Add(Hint("E", "clear", theme));
        hints.Children.Add(Hint("Esc", store.HeldNow is null ? "back to work" : "undo", theme));
        stack.Children.Add(hints);
        var wait = Small($"answers wait {(int)store.UndoDelay} s · Esc undoes", Palette.Sand, theme, default, 10.5);
        AutomationProperties.SetAutomationId(wait, "panel-hold-hint");
        stack.Children.Add(wait);
        return stack;
    }

    private static FrameworkElement Hint(string key, string label, ElementTheme theme)
    {
        var s = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4 };
        s.Children.Add(new Border
        {
            CornerRadius = new CornerRadius(3), BorderThickness = new Thickness(1), BorderBrush = Palette.Brush(Palette.Rule, theme),
            Padding = new Thickness(4, 0, 4, 1),
            Child = new TextBlock { Text = key, FontSize = 10, FontFamily = new FontFamily("Consolas"), Foreground = Palette.Brush(Palette.Sand, theme) },
        });
        s.Children.Add(new TextBlock { Text = label, FontSize = 10.5, Foreground = Palette.Brush(Palette.Sand, theme), VerticalAlignment = VerticalAlignment.Center });
        return s;
    }

    private static TextBlock Small(string text, string color, ElementTheme theme, Thickness margin, double size = 10.5) =>
        new() { Text = text, FontSize = size, Foreground = Palette.Brush(color, theme), Margin = margin };

    private static Border Divider(ElementTheme theme, double opacity) => new() { Height = 1, Background = Palette.Tint(Palette.Rule, theme, opacity) };

    private void Ago(TextBlock block, DateTimeOffset since, string prefix, string suffix)
    {
        block.Text = prefix + RailModel.Ago(since, DateTimeOffset.Now) + suffix;
        agoLabels.Add((block, since, prefix, suffix));
    }

    private void RefreshAgo()
    {
        var now = DateTimeOffset.Now;
        foreach (var (block, since, prefix, suffix) in agoLabels) block.Text = prefix + RailModel.Ago(since, now) + suffix;
    }

    /// <summary>"w2 · Three tiers — sending · Esc undo · 2" with the countdown bar (Swift <c>ReceiptView</c>); the row's UIA name is its text.</summary>
    private void UpdateReceipt()
    {
        var theme = host.ActualTheme;
        if (store.Receipt is not { } r)
        {
            receiptRow.Visibility = Visibility.Collapsed;
            AutomationProperties.SetName(receiptRow, "");
            if (IsOpen) Relayout();
            return;
        }
        bool wasHidden = receiptRow.Visibility == Visibility.Collapsed;
        receiptRow.Visibility = Visibility.Visible;
        receiptRow.Background = Palette.Tint(Palette.Malachite, theme, 0.12);
        string right = "";
        double fraction = 0;
        switch (r)
        {
            case StoreModel.ReceiptRow.Holding h:
                receiptText.Text = $"{h.Held.Who} · {h.Held.Label} — sending";
                receiptText.Foreground = Palette.Brush(Palette.Ink, theme);
                right = $"Esc undo · {(int)Math.Ceiling(store.HoldRemaining)}";
                fraction = store.UndoDelay > 0 ? store.HoldRemaining / store.UndoDelay : 0;
                break;
            case StoreModel.ReceiptRow.Sent s:
                receiptText.Text = $"Sent · {s.Text}";
                receiptText.Foreground = Palette.Brush(Palette.Malachite, theme);
                break;
            case StoreModel.ReceiptRow.Undone:
                receiptText.Text = "Taken back";
                receiptText.Foreground = Palette.Brush(Palette.Ink, theme);
                break;
            case StoreModel.ReceiptRow.Stale:
                receiptText.Text = "Already answered elsewhere";
                receiptText.Foreground = Palette.Brush(Palette.Sand, theme);
                break;
            case StoreModel.ReceiptRow.Failed f:
                receiptText.Text = $"Failed · {f.Error}";
                receiptText.Foreground = Palette.Brush(Palette.Carnelian, theme);
                break;
            case StoreModel.ReceiptRow.Reply rp:
                receiptText.Text = $"{rp.Who} · {rp.Text}";
                receiptText.Foreground = Palette.Brush(Palette.Ink, theme);
                if (rp.Session is not null) right = "Open Ctrl+O";
                break;
        }
        receiptRight.Text = right;
        receiptRight.Foreground = Palette.Brush(Palette.Ink2, theme);
        receiptBar.Background = Palette.Brush(Palette.Malachite, theme);
        receiptBar.Width = Math.Max(0, PanelWidth * Math.Clamp(fraction, 0, 1));
        AutomationProperties.SetName(receiptRow, right.Length == 0 ? receiptText.Text : $"{receiptText.Text} · {right}");
        if (wasHidden && IsOpen) Relayout();
    }

    /// <summary>Size to the content (80–560 epx) and place it at the anchor, clamped to that display's work area.</summary>
    private void Relayout()
    {
        if (!IsOpen) return;
        double s = Scale;
        content.Measure(new global::Windows.Foundation.Size(PanelWidth, double.PositiveInfinity));
        receiptRow.Measure(new global::Windows.Foundation.Size(PanelWidth, double.PositiveInfinity));
        double hEpx = content.DesiredSize.Height + (receiptRow.Visibility == Visibility.Visible ? receiptRow.DesiredSize.Height : 0);
        if (hEpx <= 0) hEpx = 300;
        hEpx = Math.Clamp(hEpx, 80, MaxHeight);
        int w = (int)Math.Round(PanelWidth * s), h = (int)Math.Round(hEpx * s);
        int gap = (int)Math.Round(8 * s);
        var a = anchor;
        PointInt32 probe;
        int x, y;
        switch (a.Kind)
        {
            case PanelAnchorKind.Tray:
                probe = new PointInt32((a.Rect.left + a.Rect.right) / 2, (a.Rect.top + a.Rect.bottom) / 2);
                x = probe.X - w / 2;
                y = a.Rect.top - h - (int)Math.Round(4 * s);
                break;
            case PanelAnchorKind.Rail:
                probe = new PointInt32((a.Rect.left + a.Rect.right) / 2, (a.Rect.top + a.Rect.bottom) / 2);
                x = a.LeftSide ? a.Rect.right + gap : a.Rect.left - gap - w;
                y = probe.Y - h / 2;
                break;
            default:
            {
                var c = Win32.Cursor();
                probe = new PointInt32(c.X, c.Y);
                var wa0 = DisplayArea.GetFromPoint(probe, DisplayAreaFallback.Primary).WorkArea;
                x = wa0.X + (wa0.Width - w) / 2;
                y = wa0.Y + (int)Math.Round(wa0.Height * 0.2);
                break;
            }
        }
        var wa = DisplayArea.GetFromPoint(probe, DisplayAreaFallback.Primary).WorkArea;
        x = Math.Clamp(x, wa.X + gap, Math.Max(wa.X + gap, wa.X + wa.Width - w - gap));
        y = Math.Clamp(y, wa.Y + gap, Math.Max(wa.Y + gap, wa.Y + wa.Height - h - gap));
        var r = ScreenRect;
        if (r.left != x || r.top != y || r.right - r.left != w || r.bottom - r.top != h) Place(new RectInt32(x, y, w, h));
    }

    public void Stop()
    {
        agoTimer.Stop();
        store.PropertyChanged -= OnStoreChanged;
    }
}