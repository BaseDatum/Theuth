using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Theuth.Companion.PointAndSay;
using Theuth.Companion.Ui;
using Windows.Graphics;

namespace Theuth.Companion.AnyApp;

/// <summary>
/// The chrome both any-app cards share (§13.5 failure card, §13.6 rewrite panel) until W153C's AnswerCardWindow lands (PIN-C): a
/// <see cref="ToolWindow"/> with the 149 NATIVE look (Mica/Acrylic backdrop, DWM round corners, theme following), 440 epx wide, placed at
/// the pointer on the pointer's display (§3.2 "failure card at the pointer"), shown without taking the foreground so the target app keeps
/// its focus; Esc (when the card has focus) and the Esc cap close it. Excluded from capture unless <c>--shareable</c> (C11).
/// </summary>
internal abstract partial class AnyAppCard : ToolWindow
{
    public const double CardWidth = 440;
    protected readonly UserControl Host;
    protected readonly StackPanel Body;
    private string? material;

    protected AnyAppCard(string title, string automationId)
        : base(title, default, excludeFromCapture: !Flags.Shareable)
    {
        Body = new StackPanel { Spacing = 8, Padding = new Thickness(16, 12, 16, 12) };
        Host = new UserControl { Content = Body, IsTabStop = true, UseSystemFocusVisuals = false };
        AutomationProperties.SetAutomationId(Host, automationId);
        AutomationProperties.SetName(Host, title);
        Host.PreviewKeyDown += (_, e) => OnKey(e);
        Content = Host;
        if (!Shape.RoundCorners(Hwnd)) Log.Write($"{automationId}: DWM rounded corners unavailable (rectangular)");
        material = FrameworkBackdrop.Apply(this, automationId);
        Theme.Follow(Host, () => { FrameworkBackdrop.ImmersiveDark(Hwnd, Host.ActualTheme); if (IsOpen) Render(); });
        FrameworkBackdrop.ImmersiveDark(Hwnd, Host.ActualTheme);
    }

    public bool IsOpen { get; private set; }

    public string Material => material ?? "none";

    protected ElementTheme ThemeNow => Host.ActualTheme;

    /// <summary>Esc closes; subclasses add their keys.</summary>
    protected virtual void OnKey(KeyRoutedEventArgs e)
    {
        if (e.Key == global::Windows.System.VirtualKey.Escape)
        {
            e.Handled = true;
            Dismiss("esc");
        }
    }

    /// <summary>The user closed the card (Esc, ✕, Alt+F4).</summary>
    public event Action<string>? Dismissed;

    protected override void OnCloseRequested() => Dismiss("close");

    public void Dismiss(string how)
    {
        if (!IsOpen) return;
        IsOpen = false;
        HideWindow();
        Log.Write($"{Title}: dismissed ({how})");
        Dismissed?.Invoke(how);
    }

    /// <summary>Hide without the user (the controller moved on).</summary>
    public void Retire()
    {
        if (!IsOpen) return;
        IsOpen = false;
        HideWindow();
    }

    /// <summary>Rebuild <see cref="Body"/> for the current state.</summary>
    protected abstract void Build(ElementTheme theme);

    protected void Render()
    {
        var theme = ThemeNow;
        Host.Background = SystemBackdrop is null ? Theme.Brush(NativeLook.SolidBackgroundBase, theme) : Theme.Transparent;
        Host.Foreground = Theme.Brush(NativeLook.TextPrimary, theme);
        Body.Children.Clear();
        Build(theme);
        if (IsOpen) DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, PlaceAtPointer);
    }

    private (int X, int Y)? anchor;

    /// <summary>Show at the pointer (or <paramref name="at"/>) without activating; re-renders when already open.</summary>
    public void Present((int X, int Y)? at = null)
    {
        anchor = at;
        IsOpen = true;
        Render();
        PlaceAtPointer();
        ShowPassive();
    }

    private void PlaceAtPointer()
    {
        var all = ScreenMonitor.All();
        int px, py;
        if (anchor is { } a) (px, py) = (a.X, a.Y);
        else if (AnyAppNative.GetCursorPos(out var p)) (px, py) = (p.X, p.Y);
        else (px, py) = (all[0].WorkArea.X + all[0].WorkArea.Width / 2, all[0].WorkArea.Y + all[0].WorkArea.Height / 3);
        var m = ScreenMonitor.At(all, px, py);
        double s = m.Scale <= 0 ? 1 : m.Scale;
        Body.Measure(new global::Windows.Foundation.Size(CardWidth, double.PositiveInfinity));
        int w = (int)Math.Round(CardWidth * s);
        int h = (int)Math.Round(Math.Max(80, Body.DesiredSize.Height + 2) * s);
        var work = m.WorkArea;
        int x = Math.Clamp(px - w / 2, work.X + 8, Math.Max(work.X + 8, work.X + work.Width - w - 8));
        int below = py + (int)(24 * s);
        int y = below + h <= work.Y + work.Height - 8 ? below : Math.Max(work.Y + 8, py - h - (int)(16 * s));
        Place(new RectInt32(x, y, w, h));
    }

    protected TextBlock Line(string text, double size, string brush, ElementTheme theme, string? automationId = null, bool wrap = true, int maxLines = 0)
    {
        var t = new TextBlock
        {
            Text = text, FontSize = size, Foreground = Theme.Brush(brush, theme),
            TextWrapping = wrap ? TextWrapping.Wrap : TextWrapping.NoWrap, TextTrimming = TextTrimming.CharacterEllipsis,
        };
        if (maxLines > 0) t.MaxLines = maxLines;
        if (automationId is not null) AutomationProperties.SetAutomationId(t, automationId);
        return t;
    }

    /// <summary>The title row: a glyph, the title, the Esc close cap.</summary>
    protected Grid Header(ElementTheme theme, string glyph, string glyphBrush, string title, string automationId)
    {
        var g = new Grid { ColumnSpacing = 8 };
        g.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        g.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        g.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var icon = Theme.Icon(glyph, theme, glyphBrush, 14);
        icon.VerticalAlignment = VerticalAlignment.Center;
        g.Children.Add(icon);
        var t = Line(title, CardChrome.Question, NativeLook.TextPrimary, theme, automationId, wrap: true, maxLines: 2);
        t.FontWeight = Microsoft.UI.Text.FontWeights.SemiBold;
        t.VerticalAlignment = VerticalAlignment.Center;
        Grid.SetColumn(t, 1);
        g.Children.Add(t);
        var close = CardChrome.KeyCapButton(theme, null, "Esc", () => Dismiss("esc-cap"), $"{automationId}-close");
        Grid.SetColumn(close, 2);
        g.Children.Add(close);
        return g;
    }

    /// <summary>A row of actions: "‹label› ‹cap›" buttons; the first is the accent (default) one.</summary>
    protected StackPanel Actions(ElementTheme theme, params (string Label, string Cap, string Id, Action Act)[] actions)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        bool first = true;
        foreach (var (label, cap, id, act) in actions)
        {
            var b = CardChrome.KeyCapButton(theme, label, cap, act, id);
            b.Background = Theme.Brush(first ? NativeLook.Accent : NativeLook.ControlFill, theme);
            b.Resources["ButtonBackgroundPointerOver"] = Theme.Brush(first ? NativeLook.AccentSecondary : NativeLook.ControlFillSecondary, theme);
            b.Resources["ButtonBackgroundPressed"] = Theme.Brush(first ? NativeLook.AccentTertiary : NativeLook.ControlFillTertiary, theme);
            if (first && b.Content is StackPanel sp && sp.Children.Count > 0 && sp.Children[0] is TextBlock lt) lt.Foreground = Theme.Brush(NativeLook.OnAccent, theme);
            row.Children.Add(b);
            first = false;
        }
        return row;
    }
}

/// <summary>What the failure card shows (§13.5): the typed kind, the words that did not land, the footer's evidence.</summary>
internal sealed record FailureCardModel(string App, DeliveryFailure Kind, string Text, string Footer, bool CanRetry, string? Detail = null)
{
    public string Title => AnyAppCopy.CardTitle(App, Kind) + (Detail is { Length: > 0 } d ? $" — {d}" : "");
}

/// <summary>
/// 153 §13.5 — "Couldn't type into ‹app› · ‹kind›", the words, [Copy] [Try again] [Open in theuth], the footer
/// ("checked at 150 · 350 · 1000 ms — field unchanged · clipboard restored"). It stays until acted on (D13). UIA: <c>anyapp-card</c>,
/// <c>anyapp-card-title</c>, <c>anyapp-card-text</c>, <c>anyapp-card-copy</c> / <c>-retry</c> / <c>-open</c> / <c>-close</c>, <c>anyapp-card-footer</c>.
/// </summary>
internal sealed partial class FailureCardWindow : AnyAppCard
{
    private FailureCardModel? model;
    private string? status;

    public FailureCardWindow() : base("theuth — Couldn't type", "anyapp-card") { }

    public event Action? CopyRequested;
    public event Action? RetryRequested;
    public event Action? OpenRequested;

    public FailureCardModel? Model => model;

    public void Show(FailureCardModel m)
    {
        model = m;
        status = null;
        Present();
        Log.Write($"anyapp-card: shown — {m.Title} · {m.Text.Length} chars · footer \"{m.Footer}\" · backdrop {Material}");
    }

    /// <summary>A one-line status under the buttons ("copied", "trying again…", "still not landed").</summary>
    public void SetStatus(string s)
    {
        status = s;
        if (IsOpen) Render();
    }

    protected override void OnKey(KeyRoutedEventArgs e)
    {
        switch (e.Key)
        {
            case global::Windows.System.VirtualKey.C: e.Handled = true; CopyRequested?.Invoke(); return;
            case global::Windows.System.VirtualKey.Enter: if (model?.CanRetry == true) { e.Handled = true; RetryRequested?.Invoke(); } return;
            case global::Windows.System.VirtualKey.O: e.Handled = true; OpenRequested?.Invoke(); return;
        }
        base.OnKey(e);
    }

    protected override void Build(ElementTheme theme)
    {
        if (model is not { } m) return;
        Body.Children.Add(Header(theme, NativeLook.Glyph.Warning, NativeLook.Caution, m.Title, "anyapp-card-title"));
        var words = Line(m.Text, CardChrome.Body, NativeLook.TextPrimary, theme, "anyapp-card-text", wrap: true, maxLines: 8);
        words.IsTextSelectionEnabled = true;
        Body.Children.Add(new Border
        {
            Child = words, Padding = new Thickness(10, 8, 10, 8), CornerRadius = CardChrome.OverlayRadius,
            Background = Theme.Brush(NativeLook.CardBackgroundSecondary, theme), BorderBrush = Theme.Brush(NativeLook.CardStroke, theme), BorderThickness = new Thickness(1),
        });
        var acts = new List<(string, string, string, Action)> { ("Copy", "C", "anyapp-card-copy", () => CopyRequested?.Invoke()) };
        if (m.CanRetry) acts.Add(("Try again", "Enter", "anyapp-card-retry", () => RetryRequested?.Invoke()));
        acts.Add(("Open in theuth", "O", "anyapp-card-open", () => OpenRequested?.Invoke()));
        Body.Children.Add(Actions(theme, acts.ToArray()));
        if (status is { Length: > 0 } s) Body.Children.Add(Line(s, CardChrome.Meta, NativeLook.TextSecondary, theme, "anyapp-card-status"));
        if (m.Footer.Length > 0) Body.Children.Add(Line(m.Footer, CardChrome.Meta, NativeLook.TextTertiary, theme, "anyapp-card-footer"));
    }
}

/// <summary>What the rewrite panel shows (§13.6).</summary>
internal sealed record RewriteModel(string App, string Instruction, string Selection, string? Proposal, string State, string? Meta, bool Busy, bool Failed);

/// <summary>
/// 153 §13.6 — the Rewrite panel: the spoken instruction, the selection excerpt, the proposal; Replace (Enter) restores focus to the captured
/// element, then delivers so the selection is replaced; holding the dictate-anywhere key again refines; Esc discards. UIA:
/// <c>anyapp-rewrite</c>, <c>anyapp-rewrite-instruction</c> / <c>-selection</c> / <c>-proposal</c> / <c>-state</c> / <c>-replace</c> / <c>-close</c>.
/// </summary>
internal sealed partial class RewritePanelWindow : AnyAppCard
{
    private RewriteModel? model;

    public RewritePanelWindow() : base("theuth — Rewrite", "anyapp-rewrite") { }

    public event Action? ReplaceRequested;

    public RewriteModel? Model => model;

    public void Show(RewriteModel m)
    {
        bool was = IsOpen;
        model = m;
        if (was) Render();
        else Present();
        Log.Write($"anyapp-rewrite: {(was ? "updated" : "shown")} — {m.State} · instruction {m.Instruction.Length} chars · selection {m.Selection.Length} chars · proposal {(m.Proposal?.Length ?? 0)} chars");
    }

    protected override void OnKey(KeyRoutedEventArgs e)
    {
        if (e.Key == global::Windows.System.VirtualKey.Enter && model is { Proposal: { Length: > 0 }, Busy: false })
        {
            e.Handled = true;
            ReplaceRequested?.Invoke();
            return;
        }
        base.OnKey(e);
    }

    protected override void Build(ElementTheme theme)
    {
        if (model is not { } m) return;
        Body.Children.Add(Header(theme, NativeLook.Glyph.Microphone, NativeLook.AccentText, $"Rewrite in {(m.App.Length > 0 ? m.App : "the app")}", "anyapp-rewrite-title"));
        if (m.Instruction.Length > 0)
        {
            var ins = Line($"“{m.Instruction}”", CardChrome.Body, NativeLook.TextSecondary, theme, "anyapp-rewrite-instruction", maxLines: 3);
            ins.FontStyle = global::Windows.UI.Text.FontStyle.Italic;
            Body.Children.Add(ins);
        }
        var excerpt = m.Selection.Length > 220 ? m.Selection[..220] + "…" : m.Selection;
        Body.Children.Add(new Border
        {
            Child = Line(excerpt, CardChrome.Meta, NativeLook.TextTertiary, theme, "anyapp-rewrite-selection", maxLines: 4),
            Padding = new Thickness(10, 6, 10, 6), CornerRadius = CardChrome.OverlayRadius, BorderThickness = new Thickness(1),
            BorderBrush = Theme.Brush(NativeLook.CardStroke, theme),
        });
        if (m.Proposal is { Length: > 0 } p)
        {
            var prop = Line(p, CardChrome.Body, NativeLook.TextPrimary, theme, "anyapp-rewrite-proposal", maxLines: 12);
            prop.IsTextSelectionEnabled = true;
            Body.Children.Add(new Border
            {
                Child = prop, Padding = new Thickness(10, 8, 10, 8), CornerRadius = CardChrome.OverlayRadius,
                Background = Theme.Brush(NativeLook.CardBackgroundSecondary, theme), BorderBrush = Theme.Brush(NativeLook.Accent, theme), BorderThickness = new Thickness(1),
            });
        }
        var state = Line(m.State, CardChrome.Meta, m.Failed ? NativeLook.Critical : NativeLook.TextSecondary, theme, "anyapp-rewrite-state");
        if (m.Busy)
        {
            var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
            row.Children.Add(new ProgressRing { IsActive = true, Width = 14, Height = 14 });
            row.Children.Add(state);
            Body.Children.Add(row);
        }
        else Body.Children.Add(state);
        if (m.Proposal is { Length: > 0 } && !m.Busy)
        {
            Body.Children.Add(Actions(theme, ("Replace", "Enter", "anyapp-rewrite-replace", () => ReplaceRequested?.Invoke())));
        }
        if (m.Meta is { Length: > 0 } meta) Body.Children.Add(Line(meta, CardChrome.Meta, NativeLook.TextTertiary, theme, "anyapp-rewrite-meta"));
    }
}
