using System.ComponentModel;
using System.Text;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Theuth.Companion.Rpc;
using Theuth.Companion.Voice;
using Windows.Graphics;
using Windows.Win32;
using DispatcherQueue = Microsoft.UI.Dispatching.DispatcherQueue;
using DispatcherQueuePriority = Microsoft.UI.Dispatching.DispatcherQueuePriority;
using DispatcherQueueTimer = Microsoft.UI.Dispatching.DispatcherQueueTimer;
using VirtualKey = Windows.System.VirtualKey;
using Windows.Win32.Foundation;

namespace Theuth.Companion.Ui;

/// <summary>What a key does in the agents panel (§8 agents scope): J/K · ↓/↑ · ⏎ open · V talk (hold) · X stop (hold) · ⎋ · ? legend.</summary>
internal enum AgentsVerb { None, Next, Previous, Open, Talk, Stop, Escape, Legend }

/// <summary>
/// 149 WW2 — the agents panel grouped by host (design 149 §3.2, D2/D3/D23; Windows form §7.4 #2): a <see cref="ToolWindow"/>
/// 420 × ≤ 560 epx, opaque with DWM rounded corners, header "Agents" · J K · ✕ esc, then one group per host — "this PC" first,
/// then every other engine "‹name› · on 🌐 ‹mesh›" — rows dot/spinner · harness icon · name · ⑂N · what it is doing · mic · ✕.
/// Roots first, live children indented. Remote rows are hollow with "on ‹node›", mic and ✕ disabled ("open it on ‹node›").
/// <para>Mic: hold (pointer or V) → <see cref="RowTalk"/> dictates into the row's session → release → the text in a field under
/// the row for the 3 s hold (Esc keeps editing) → <c>send_input(session, text)</c>. ✕ / X: the 3 s hold "Stopping ‹name› and N
/// children · Esc undo" → <c>cancel(session, "tree")</c>; a desk row pauses. The hold IS the confirmation (no undo after).</para>
/// UIA: <c>agents-panel</c>, <c>agents-headline</c>, <c>group-&lt;n&gt;</c>, <c>agent-row-&lt;n&gt;</c> (1-based), <c>agent-mic-&lt;n&gt;</c>,
/// <c>agent-stop-&lt;n&gt;</c>, <c>agent-talk-field</c>, <c>agents-empty</c>, <c>receipt-row</c>.
/// </summary>
internal sealed partial class AgentsPanelWindow : ToolWindow
{
    public const double PanelWidth = 420;
    public const double MaxHeight = 560;

    private readonly Store store;
    private readonly DispatcherQueue dispatcher;
    private readonly UserControl host;
    private readonly ScrollViewer scroll;
    private readonly StackPanel content;
    private readonly Border receiptRow;
    private readonly TextBlock receiptText;
    private readonly TextBlock receiptRight;
    private readonly Border receiptBar;
    private readonly TextBox talkBox;
    private readonly RowTalk talk;
    private readonly DispatcherQueueTimer agoTimer;
    private readonly List<(FrameworkElement View, int Index)> rowViews = new();
    private string renderKey = "";
    private bool renderQueued;
    private HWND previous;
    private PanelAnchor anchor = PanelAnchor.Mouse;
    private int focus;
    private string? talkRowId;
    private string talkLive = "";
    private bool talkEditing;
    private string? rowError;
    private string? rowErrorId;
    private bool vDown;

    public AgentsPanelWindow(Store store, DispatcherQueue dispatcher)
        : base("theuth — Agents", default, excludeFromCapture: !Flags.Shareable)
    {
        this.store = store;
        this.dispatcher = dispatcher;
        talk = new RowTalk(store, dispatcher);
        talk.LiveText += t =>
        {
            talkLive = t;
            if (IsOpen) UpdateTalkField();
        };
        talk.Finished += OnTalkFinished;

        content = new StackPanel();
        scroll = new ScrollViewer { Content = content, VerticalScrollBarVisibility = ScrollBarVisibility.Auto, HorizontalScrollMode = ScrollMode.Disabled };
        receiptText = new TextBlock { FontSize = 12.5, TextTrimming = TextTrimming.CharacterEllipsis, MaxLines = 2, TextWrapping = TextWrapping.Wrap, VerticalAlignment = VerticalAlignment.Center };
        receiptRight = new TextBlock { FontSize = 12, FontFamily = new FontFamily("Consolas"), VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(8, 0, 0, 0) };
        receiptBar = new Border { Height = 2, HorizontalAlignment = HorizontalAlignment.Left, VerticalAlignment = VerticalAlignment.Bottom, Width = 0 };
        var receiptGrid = new Grid { Padding = new Thickness(16, 8, 16, 8) };
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
        AutomationProperties.SetLiveSetting(receiptRow, Microsoft.UI.Xaml.Automation.Peers.AutomationLiveSetting.Polite);

        talkBox = new TextBox { PlaceholderText = "Listening…", FontSize = 13, IsSpellCheckEnabled = false, TextWrapping = TextWrapping.Wrap, AcceptsReturn = false, MaxLength = 4000 };
        AutomationProperties.SetAutomationId(talkBox, "agent-talk-field");
        AutomationProperties.SetName(talkBox, "Say to this agent");
        var layout = new Grid();
        layout.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        layout.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        Grid.SetRow(receiptRow, 1);
        layout.Children.Add(scroll);
        layout.Children.Add(receiptRow);
        host = new UserControl { Content = layout, IsTabStop = true, UseSystemFocusVisuals = false };
        AutomationProperties.SetAutomationId(host, "agents-panel");
        AutomationProperties.SetName(host, "Agents panel");
        host.PreviewKeyDown += OnPreviewKeyDown;
        host.PreviewKeyUp += OnPreviewKeyUp;
        talkBox.KeyDown += (_, e) =>
        {
            if (e.Key == VirtualKey.Enter)
            {
                if (FocusedRow is { } r && talkBox.Text.Trim().Length > 0)
                {
                    store.SendToRow(r, talkBox.Text);
                    talkEditing = false;
                    talkRowId = null;
                    talkLive = "";
                    Render(force: true);
                }
                e.Handled = true;
            }
            else if (e.Key == VirtualKey.Escape)
            {
                if (store.HeldNow is not null) store.Perform(Intent.Escape); // Esc keeps editing: the hold is undone, the text stays
                else
                {
                    talkEditing = false;
                    talkRowId = null;
                    talkLive = "";
                    Render(force: true);
                    host.Focus(FocusState.Programmatic);
                }
                e.Handled = true;
            }
        };

        host.ActualThemeChanged += (_, _) => { if (IsOpen) Render(force: true); };
        Content = host;
        if (!Shape.RoundCorners(Hwnd)) Log.Write("agents panel: DWM rounded corners unavailable (rectangular)");
        SystemEffects.ApplyTransitions(Hwnd, SystemEffects.AnimationsOn);
        if (SystemEffects.Current is { } fx) fx.Changed += () => SystemEffects.ApplyTransitions(Hwnd, SystemEffects.AnimationsOn);

        agoTimer = DispatcherQueue.CreateTimer();
        agoTimer.Interval = TimeSpan.FromSeconds(15);
        agoTimer.IsRepeating = true;
        agoTimer.Tick += (_, _) => { if (IsOpen) Render(force: true); };
        store.PropertyChanged += OnStoreChanged;
        Place(new RectInt32(0, 0, (int)Math.Round(PanelWidth * Scale), (int)Math.Round(240 * Scale)));
    }

    public bool IsOpen { get; private set; }

    public bool ProbeMode { get; set; }

    public AgentRow? FocusedRow
    {
        get
        {
            var rows = store.AgentsPanel.Rows;
            return focus >= 0 && focus < rows.Count ? rows[focus] : null;
        }
    }

    /// <summary>Summon (the agents pill, a dot, Ctrl+Shift+A): scrolled to <paramref name="session"/>'s row, else the first working row (§8).</summary>
    public void Open(PanelAnchor at, HWND prev, string? session = null)
    {
        anchor = at;
        previous = prev;
        IsOpen = true;
        var p = store.AgentsPanel;
        int at0 = session is { } s ? p.IndexOfSession(s) : -1;
        focus = at0 >= 0 ? at0 : p.FirstWorking;
        Render(force: true);
        UpdateReceipt();
        agoTimer.Start();
        talk.Prewarm();
        if (ProbeMode)
        {
            ShowPassive();
            Log.Write($"agents panel shown without activation (probe) at {Win32.Text(ScreenRect)} rows={p.Rows.Count}");
            return;
        }
        bool active = ShowActive();
        host.Focus(FocusState.Programmatic);
        DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, () =>
        {
            Relayout();
            ScrollToFocus();
            if (IsOpen && !talkEditing && host.FocusState == FocusState.Unfocused) host.Focus(FocusState.Programmatic);
        });
        Log.Write($"agents panel open ({at.Kind}) rows={p.Rows.Count} groups={p.Groups.Count} focus={focus} prev={Win32.Describe(prev)} active={(active ? "true" : "false")}");
    }

    public void Close(bool returnFocus)
    {
        if (!IsOpen) return;
        IsOpen = false;
        agoTimer.Stop();
        if (talk.Busy) talk.Cancel();
        talkEditing = false;
        talkRowId = null;
        talkLive = "";
        bool wasForeground = IsForeground;
        HideWindow();
        if (returnFocus && wasForeground)
        {
            bool ok = Win32.Foreground(previous);
            Log.Write($"agents panel closed — foreground back to {Win32.Describe(previous)} -> {(ok ? "true" : "false")}");
        }
        else
        {
            Log.Write($"agents panel closed (returnFocus={(returnFocus ? "true" : "false")})");
        }
        previous = default;
    }

    public long ClosedByFocusLossAt { get; private set; }

    protected override void OnActivate(bool active, HWND other)
    {
        if (active || !IsOpen || ProbeMode) return;
        Close(returnFocus: false);
        ClosedByFocusLossAt = Environment.TickCount64;
    }

    protected override void OnCloseRequested() => Close(returnFocus: true);

    // MARK: keys (§8 agents scope)

    /// <summary>The key table's <c>agents</c> scope when WW1's table has it, else the design's defaults.</summary>
    internal static AgentsVerb VerbFor(KeyPress press, KeyTable keys)
    {
        var chord = press.Chord;
        if (Enum.TryParse<KeyScope>("Agents", out var scope) && keys.ActionFor(chord, scope) is { } action)
        {
            var raw = action.RawValue();
            if (raw.Contains("next", StringComparison.OrdinalIgnoreCase)) return AgentsVerb.Next;
            if (raw.Contains("previous", StringComparison.OrdinalIgnoreCase)) return AgentsVerb.Previous;
            if (raw.Contains("open", StringComparison.OrdinalIgnoreCase)) return AgentsVerb.Open;
            if (raw.Contains("talk", StringComparison.OrdinalIgnoreCase)) return AgentsVerb.Talk;
            if (raw.Contains("stop", StringComparison.OrdinalIgnoreCase)) return AgentsVerb.Stop;
            if (raw.Contains("escape", StringComparison.OrdinalIgnoreCase)) return AgentsVerb.Escape;
            if (raw.Contains("legend", StringComparison.OrdinalIgnoreCase)) return AgentsVerb.Legend;
        }
        if (chord == Chord.Escape) return AgentsVerb.Escape;
        if (chord == Chord.Down) return AgentsVerb.Next;
        if (chord == Chord.Up) return AgentsVerb.Previous;
        if (chord == Chord.ReturnKey || chord == Chord.NumpadEnter) return AgentsVerb.Open;
        var c = chord.NormalizedLocal;
        bool ctrl = c.Modifiers.HasFlag(ChordModifiers.Control);
        if (!ctrl)
        {
            switch (c.Chars)
            {
                case "j": return AgentsVerb.Next;
                case "k": return AgentsVerb.Previous;
                case "v": return AgentsVerb.Talk;
                case "x": return AgentsVerb.Stop;
                case "?": return AgentsVerb.Legend;
            }
        }
        else
        {
            if (c.Chars == "o") return AgentsVerb.Open;
            if (c.Chars == "/") return AgentsVerb.Legend;
        }
        return AgentsVerb.None;
    }

    /// <summary>The legend (Ctrl+/ · ?) is the composition root's window.</summary>
    public event Action? LegendRequested;

    private void OnPreviewKeyDown(object sender, KeyRoutedEventArgs e)
    {
        bool textField = host.XamlRoot is { } xr && FocusManager.GetFocusedElement(xr) is TextBox;
        if (textField) return; // the talk field handles Enter / Esc itself; letters type
        var press = KeyInput.From(e);
        var verb = VerbFor(press, store.Keys);
        if (verb == AgentsVerb.None) return;
        e.Handled = true;
        if (verb == AgentsVerb.Talk && e.KeyStatus.WasKeyDown) return; // auto-repeat while held
        Log.Write($"agents key {press.Chord.Symbol} -> {verb}");
        switch (verb)
        {
            case AgentsVerb.Next: Move(1); break;
            case AgentsVerb.Previous: Move(-1); break;
            case AgentsVerb.Open:
                if (FocusedRow?.Session is { } s) OpenSession(s);
                break;
            case AgentsVerb.Talk:
                vDown = true;
                BeginTalk(FocusedRow);
                break;
            case AgentsVerb.Stop:
                if (FocusedRow is { } r) store.StopRow(r);
                break;
            case AgentsVerb.Legend: LegendRequested?.Invoke(); break;
            case AgentsVerb.Escape:
                if (talk.Busy)
                {
                    talk.Cancel();
                    talkRowId = null;
                    talkLive = "";
                    Render(force: true);
                }
                else if (store.HeldNow is not null) store.Perform(Intent.Escape);
                else Close(returnFocus: true);
                break;
        }
    }

    private void OnPreviewKeyUp(object sender, KeyRoutedEventArgs e)
    {
        if (!vDown) return;
        var press = KeyInput.From(e);
        if (VerbFor(press, store.Keys) != AgentsVerb.Talk && press.Chord.NormalizedLocal.Chars != "v") return;
        vDown = false;
        e.Handled = true;
        EndTalk();
    }

    public void OpenSession(string session)
    {
        Close(returnFocus: false);
        _ = Links.Open(DeepLink.Session(session));
    }

    private void Move(int delta)
    {
        int n = store.AgentsPanel.Rows.Count;
        if (n == 0) return;
        focus = Math.Clamp(focus + delta, 0, n - 1);
        Render(force: true);
        ScrollToFocus();
    }

    private void ScrollToFocus()
    {
        foreach (var (v, i) in rowViews)
        {
            if (i != focus) continue;
            try
            {
                var p = v.TransformToVisual(content).TransformPoint(new global::Windows.Foundation.Point(0, 0));
                double top = p.Y, bottom = p.Y + v.ActualHeight;
                if (top < scroll.VerticalOffset) scroll.ChangeView(null, top - 8, null);
                else if (bottom > scroll.VerticalOffset + scroll.ViewportHeight) scroll.ChangeView(null, bottom - scroll.ViewportHeight + 8, null);
            }
            catch (Exception) { }
        }
    }

    // MARK: talk (hold)

    private void BeginTalk(AgentRow? row)
    {
        if (row is null || talk.Busy) return;
        if (!row.CanTalk)
        {
            ShowRowError(row, row.DisabledReason ?? "nothing to talk to");
            return;
        }
        if (row.Session is not { } s) return;
        talkRowId = row.Id;
        talkLive = "";
        talkEditing = false;
        rowError = null;
        talk.Begin(s);
        Render(force: true);
    }

    private void EndTalk()
    {
        if (!talk.Busy) return;
        talk.End();
        UpdateTalkField();
    }

    private void OnTalkFinished(string text, string? error)
    {
        if (talkRowId is null) return;
        var row = store.AgentsPanel.Rows.FirstOrDefault(r => r.Id == talkRowId);
        if (error is not null)
        {
            talkRowId = null;
            talkLive = "";
            if (row is not null) ShowRowError(row, error);
            else Render(force: true);
            return;
        }
        if (text.Length == 0)
        {
            talkRowId = null;
            Render(force: true);
            return;
        }
        // The text shows in the field under the row for the 3 s hold; Esc keeps editing, Enter re-sends.
        talkLive = text;
        talkEditing = true;
        if (row is not null) store.SendToRow(row, text);
        Render(force: true);
        talkBox.Focus(FocusState.Programmatic);
        talkBox.SelectionStart = talkBox.Text.Length;
    }

    private void ShowRowError(AgentRow row, string why)
    {
        rowError = why;
        rowErrorId = row.Id;
        Render(force: true);
        var t = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromSeconds(5);
        t.IsRepeating = false;
        t.Tick += (_, _) =>
        {
            t.Stop();
            if (rowErrorId == row.Id)
            {
                rowError = null;
                rowErrorId = null;
                if (IsOpen) Render(force: true);
            }
        };
        t.Start();
    }

    private void UpdateTalkField()
    {
        if (talkBox.Parent is null) return;
        if (talk.Busy)
        {
            talkBox.IsReadOnly = true;
            if (talkBox.Text != talkLive) talkBox.Text = talkLive;
            talkBox.PlaceholderText = "Listening…";
            talkBox.Opacity = 0.85;
        }
        else
        {
            talkBox.IsReadOnly = false;
            talkBox.Opacity = 1;
        }
    }

    // MARK: store → render

    private void OnStoreChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (!IsOpen) return;
        if (e.PropertyName is nameof(Store.Receipt) or nameof(Store.Now))
        {
            UpdateReceipt();
            return;
        }
        if (e.PropertyName is not (nameof(Store.AgentsPanel) or nameof(Store.HoveredSession) or nameof(Store.Connection))) return;
        if (renderQueued) return;
        renderQueued = true;
        DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, () =>
        {
            renderQueued = false;
            if (IsOpen) Render();
        });
    }

    private string Key()
    {
        var sb = new StringBuilder();
        sb.Append(store.Connection).Append('|').Append(focus).Append('|').Append(store.HoveredSession).Append('|').Append(talkRowId).Append('|').Append(talkEditing).Append('|').Append(rowErrorId).Append(rowError);
        foreach (var g in store.AgentsPanel.Groups)
        {
            sb.Append('#').Append(g.Title);
            foreach (var r in g.Rows) sb.Append('|').Append(r.Id).Append(r.Name).Append(r.State).Append(r.Children).Append(r.Last).Append(r.Depth).Append(r.Running);
        }
        return sb.ToString();
    }

    private void Render(bool force = false)
    {
        var key = Key();
        if (!force && key == renderKey) return;
        renderKey = key;
        bool talkFocused = talkBox.FocusState != FocusState.Unfocused;
        if (talkBox.Parent is Panel tp) tp.Children.Remove(talkBox);
        content.Children.Clear();
        rowViews.Clear();
        var theme = host.ActualTheme;
        host.Background = Palette.Brush(Palette.Surface, theme);
        host.Foreground = Palette.Brush(Palette.Ink, theme);
        receiptRow.BorderBrush = Palette.Tint(Palette.Rule, theme, 0.6);
        var p = store.AgentsPanel;
        var rows = p.Rows;
        if (focus >= rows.Count) focus = Math.Max(0, rows.Count - 1);

        content.Children.Add(Header(theme, p));
        if (store.Connection != RpcClient.State.Open)
        {
            content.Children.Add(Small(AgentsModel.Connecting, Palette.Sand, theme, new Thickness(16, 4, 16, 8), 12));
        }
        if (p.IsEmpty)
        {
            var empty = new StackPanel { Spacing = 6, Padding = new Thickness(26, 22, 26, 30) };
            empty.Children.Add(new TextBlock { Text = AgentsModel.EmptyTitle, FontSize = 20, FontWeight = Microsoft.UI.Text.FontWeights.Bold, Foreground = Palette.Brush(Palette.Ink, theme) });
            empty.Children.Add(new TextBlock { Text = AgentsModel.EmptyBody, FontSize = 14, TextWrapping = TextWrapping.Wrap, Foreground = Palette.Brush(Palette.Sand, theme) });
            AutomationProperties.SetAutomationId(empty, "agents-empty");
            content.Children.Add(empty);
        }
        int index = 0, gi = 0;
        foreach (var g in p.Groups)
        {
            gi++;
            if (g.Rows.Count == 0 && !g.Local) continue;
            if (g.Rows.Count == 0 && p.IsEmpty) continue;
            var head = new TextBlock { Text = g.Title, FontSize = 12.5, Foreground = Palette.Brush(Palette.Sand, theme), Margin = new Thickness(26, gi == 1 ? 6 : 14, 26, 2) };
            AutomationProperties.SetAutomationId(head, $"group-{gi}");
            content.Children.Add(head);
            foreach (var r in g.Rows)
            {
                content.Children.Add(RowView(r, index, theme));
                if (r.Id == talkRowId) content.Children.Add(TalkField(theme));
                if (r.Id == rowErrorId && rowError is { } err)
                {
                    content.Children.Add(Small(err, Palette.Carnelian, theme, new Thickness(44 + 16 * r.Depth, 0, 16, 6), 12));
                }
                index++;
            }
        }
        content.Children.Add(new Border { Height = 10 });
        content.Opacity = store.Connection == RpcClient.State.Open ? 1 : 0.7;
        Relayout();
        if (talkFocused || talkEditing) talkBox.Focus(FocusState.Programmatic);
        UpdateTalkField();
    }

    private FrameworkElement Header(ElementTheme theme, AgentsPanel p)
    {
        var grid = new Grid { Padding = new Thickness(22, 16, 16, 6), ColumnSpacing = 8 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var title = new StackPanel { Spacing = 1, VerticalAlignment = VerticalAlignment.Center };
        var name = new TextBlock { Text = "Agents", FontSize = 20, FontWeight = Microsoft.UI.Text.FontWeights.Bold, Foreground = Palette.Brush(Palette.Ink, theme) };
        title.Children.Add(name);
        var counts = AgentsModel.Counts(p);
        if (counts.Length > 0) title.Children.Add(Small(counts, Palette.Ink2, theme, default, 12));
        AutomationProperties.SetAutomationId(title, "agents-headline");
        AutomationProperties.SetName(title, counts.Length == 0 ? "Agents" : $"Agents, {counts}");
        grid.Children.Add(title);
        var jk = KeyCapRow(theme, ("J", (Action)(() => Move(1))), ("K", (Action)(() => Move(-1))));
        Grid.SetColumn(jk, 1);
        grid.Children.Add(jk);
        var close = CardChrome.KeyCapButton(theme, "✕", "esc", () => Close(returnFocus: true));
        AutomationProperties.SetAutomationId(close, "agents-close");
        Grid.SetColumn(close, 2);
        grid.Children.Add(close);
        return grid;
    }

    private static FrameworkElement KeyCapRow(ElementTheme theme, params (string Cap, Action Act)[] caps)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
        foreach (var (cap, act) in caps) row.Children.Add(CardChrome.KeyCapButton(theme, null, cap, act));
        return row;
    }

    private FrameworkElement RowView(AgentRow r, int index, ElementTheme theme)
    {
        bool focused = index == focus;
        bool hovered = store.HoveredSession is { } hs && r.Session == hs;
        var grid = new Grid { ColumnSpacing = 10, Padding = new Thickness(12 + 16 * r.Depth, 8, 10, 8) };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto }); // dot
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto }); // harness
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) }); // name + last
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto }); // ⑂N
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto }); // mic
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto }); // ✕
        var dot = PillGlyphs.RowDot(r.State, theme, SystemEffects.AnimationsOn);
        dot.VerticalAlignment = VerticalAlignment.Center;
        grid.Children.Add(dot);
        var icon = PillGlyphs.Harness(r.Harness, theme, 18);
        icon.VerticalAlignment = VerticalAlignment.Center;
        ToolTipService.SetToolTip(icon, r.Harness);
        Grid.SetColumn(icon, 1);
        grid.Children.Add(icon);
        var text = new StackPanel { Spacing = 1, VerticalAlignment = VerticalAlignment.Center };
        var name = new TextBlock { Text = r.Name, FontSize = 14.5, FontWeight = Microsoft.UI.Text.FontWeights.Medium, Foreground = Palette.Brush(r.Remote ? Palette.Ink2 : Palette.Ink, theme), TextTrimming = TextTrimming.CharacterEllipsis };
        text.Children.Add(name);
        if (r.Last.Length > 0)
        {
            var last = Small(r.Last, Palette.Sand, theme, default, 12);
            last.TextTrimming = TextTrimming.CharacterEllipsis;
            last.MaxLines = 1;
            text.Children.Add(last);
        }
        Grid.SetColumn(text, 2);
        grid.Children.Add(text);
        if (r.Children > 0)
        {
            var kids = Small($"⑂{r.Children}", Palette.Sand, theme, default, 12.5);
            kids.VerticalAlignment = VerticalAlignment.Center;
            ToolTipService.SetToolTip(kids, $"{r.Children} live {(r.Children == 1 ? "child" : "children")}");
            Grid.SetColumn(kids, 3);
            grid.Children.Add(kids);
        }
        var mic = IconButton(theme, "\uE720", r.CanTalk ? "Hold to talk to this agent" : (r.DisabledReason ?? "nothing to talk to"), r.CanTalk, $"agent-mic-{index + 1}", out var micView);
        if (r.CanTalk)
        {
            // A Button marks pointer presses handled: listen with handledEventsToo so the hold starts on the press, not the click.
            mic.AddHandler(UIElement.PointerPressedEvent, new PointerEventHandler((s, e) =>
            {
                focus = index;
                ((UIElement)s).CapturePointer(e.Pointer);
                BeginTalk(r);
            }), true);
            mic.AddHandler(UIElement.PointerReleasedEvent, new PointerEventHandler((s, e) =>
            {
                ((UIElement)s).ReleasePointerCapture(e.Pointer);
                EndTalk();
            }), true);
            mic.AddHandler(UIElement.PointerCaptureLostEvent, new PointerEventHandler((_, _) => EndTalk()), true);
        }
        if (talk.Busy && r.Id == talkRowId) mic.Foreground = Palette.Brush(Palette.Carnelian, theme);
        Grid.SetColumn(micView, 4);
        grid.Children.Add(micView);
        var stop = IconButton(theme, "\uE711", r.CanStop ? (r.DeskName is null ? "Stop this agent and its children (hold)" : "Pause this desk (hold)") : (r.DisabledReason ?? "nothing is running"), r.CanStop, $"agent-stop-{index + 1}", out var stopView);
        if (r.CanStop)
        {
            stop.Click += (_, _) =>
            {
                focus = index;
                store.StopRow(r);
            };
        }
        Grid.SetColumn(stopView, 5);
        grid.Children.Add(stopView);

        var frame = new Border
        {
            Child = grid, Margin = new Thickness(14, 2, 14, 2), CornerRadius = new CornerRadius(14),
            Background = focused ? Palette.Tint(Palette.Faience, theme, 0.18) : hovered ? Palette.Tint(Palette.Faience, theme, 0.10) : Palette.Transparent,
        };
        frame.PointerPressed += (_, _) =>
        {
            if (focus != index)
            {
                focus = index;
                Render(force: true);
            }
        };
        frame.DoubleTapped += (_, _) =>
        {
            if (r.Session is { } s) OpenSession(s);
        };
        AutomationProperties.SetAutomationId(frame, $"agent-row-{index + 1}");
        AutomationProperties.SetName(frame, r.Detail);
        AutomationProperties.SetAccessibilityView(frame, Microsoft.UI.Xaml.Automation.Peers.AccessibilityView.Content);
        rowViews.Add((frame, index));
        return frame;
    }

    private FrameworkElement TalkField(ElementTheme theme)
    {
        var stack = new StackPanel { Spacing = 3, Margin = new Thickness(44, 0, 24, 6) };
        stack.Children.Add(talkBox);
        var hint = talk.Busy ? "release → sends in 3 s · Esc cancels" : "sends in 3 s · Esc keeps editing · ⏎ sends now";
        stack.Children.Add(Small(hint, Palette.Sand, theme, default, 10.5));
        return stack;
    }

    /// <summary>
    /// A row's mic / ✕ button. Disabled rows (149 recheck N3) are really disabled — <c>IsEnabled = false</c>, so UI Automation
    /// reports <c>enabled=False</c> — and keep their explanation: WinUI drops a disabled control's tooltip, so the tooltip and the
    /// UIA name ride on a wrapper the pointer still reaches. <paramref name="view"/> is what goes into the grid.
    /// </summary>
    private Button IconButton(ElementTheme theme, string glyph, string tip, bool enabled, string id, out FrameworkElement view)
    {
        var b = new Button
        {
            Content = new FontIcon { Glyph = glyph, FontSize = 12 }, Width = 30, Height = 30, Padding = new Thickness(0), CornerRadius = new CornerRadius(15),
            BorderThickness = new Thickness(0), Background = Palette.Tint(Palette.Ink, theme, enabled ? 0.08 : 0.03),
            Foreground = Palette.Brush(enabled ? Palette.Ink2 : Palette.Sand, theme), Opacity = enabled ? 1 : 0.5, IsTabStop = false, UseSystemFocusVisuals = false,
            IsEnabled = enabled,
        };
        b.Resources["ButtonBackgroundPointerOver"] = Palette.Tint(Palette.Ink, theme, enabled ? 0.14 : 0.03);
        b.Resources["ButtonBackgroundPressed"] = Palette.Tint(Palette.Ink, theme, enabled ? 0.2 : 0.03);
        b.Resources["ButtonForegroundPointerOver"] = Palette.Brush(Palette.Ink, theme);
        b.Resources["ButtonForegroundPressed"] = Palette.Brush(Palette.Ink, theme);
        b.Resources["ButtonBackgroundDisabled"] = Palette.Tint(Palette.Ink, theme, 0.03);
        b.Resources["ButtonForegroundDisabled"] = Palette.Brush(Palette.Sand, theme);
        AutomationProperties.SetAutomationId(b, id);
        AutomationProperties.SetName(b, tip);
        if (enabled)
        {
            ToolTipService.SetToolTip(b, tip);
            view = b;
            return b;
        }
        var wrap = new Border { Child = b, Background = Palette.Transparent, Width = 30, Height = 30, CornerRadius = new CornerRadius(15) };
        ToolTipService.SetToolTip(wrap, tip);
        AutomationProperties.SetAutomationId(wrap, $"{id}-hint");
        AutomationProperties.SetName(wrap, tip);
        view = wrap;
        return b;
    }

    private static TextBlock Small(string text, string key, ElementTheme theme, Thickness margin, double size = 11.5) =>
        new() { Text = text, FontSize = size, Foreground = Palette.Brush(key, theme), Margin = margin, TextWrapping = TextWrapping.Wrap };

    private void UpdateReceipt()
    {
        var theme = host.ActualTheme;
        var lines = AskBarModel.ReceiptLines(store.Receipt, store.HoldRemaining);
        if (lines is not { } l)
        {
            if (receiptRow.Visibility == Visibility.Visible)
            {
                receiptRow.Visibility = Visibility.Collapsed;
                Relayout();
            }
            return;
        }
        receiptText.Text = l.Text;
        receiptText.Foreground = store.Receipt switch
        {
            StoreModel.ReceiptRow.Sent => Palette.Brush(Palette.Malachite, theme),
            StoreModel.ReceiptRow.Failed => Palette.Brush(Palette.Carnelian, theme),
            StoreModel.ReceiptRow.Stale => Palette.Brush(Palette.Sand, theme),
            _ => Palette.Brush(Palette.Ink, theme),
        };
        receiptRight.Text = l.Trailing;
        receiptRight.Foreground = Palette.Brush(Palette.Sand, theme);
        if (store.Receipt is StoreModel.ReceiptRow.Holding && store.UndoDelay > 0)
        {
            receiptBar.Width = Math.Max(0, PanelWidth * store.HoldRemaining / store.UndoDelay);
            receiptBar.Background = Palette.Brush(Palette.Carnelian, theme);
            receiptBar.Visibility = Visibility.Visible;
        }
        else receiptBar.Visibility = Visibility.Collapsed;
        AutomationProperties.SetName(receiptRow, $"Receipt: {l.Text}");
        bool was = receiptRow.Visibility == Visibility.Visible;
        receiptRow.Visibility = Visibility.Visible;
        if (!was) Relayout();
    }

    private void Relayout()
    {
        double s = Scale;
        content.Measure(new global::Windows.Foundation.Size(PanelWidth, double.PositiveInfinity));
        receiptRow.Measure(new global::Windows.Foundation.Size(PanelWidth, double.PositiveInfinity));
        double hEpx = content.DesiredSize.Height + (receiptRow.Visibility == Visibility.Visible ? receiptRow.DesiredSize.Height : 0);
        if (hEpx <= 0) hEpx = 240;
        hEpx = Math.Clamp(hEpx, 120, MaxHeight);
        int w = (int)Math.Round(PanelWidth * s), h = (int)Math.Round(hEpx * s);
        int gap = (int)Math.Round(8 * s);
        PointInt32 probe;
        int x, y;
        switch (anchor.Kind)
        {
            case PanelAnchorKind.Rail:
                probe = new PointInt32((anchor.Rect.left + anchor.Rect.right) / 2, (anchor.Rect.top + anchor.Rect.bottom) / 2);
                x = anchor.LeftSide ? anchor.Rect.right + gap : anchor.Rect.left - gap - w;
                y = probe.Y - h / 2;
                break;
            case PanelAnchorKind.Tray:
                probe = new PointInt32((anchor.Rect.left + anchor.Rect.right) / 2, (anchor.Rect.top + anchor.Rect.bottom) / 2);
                x = probe.X - w / 2;
                y = anchor.Rect.top - h - (int)Math.Round(4 * s);
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