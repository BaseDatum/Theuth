using Microsoft.UI.Dispatching;
using Microsoft.UI.Text;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Theuth.Companion.PointAndSay;
using Windows.Graphics;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.WindowsAndMessaging;

namespace Theuth.Companion.Ui;

/// <summary>The card's colour of state (§12.4: the failed colour, the amber approval box, the accent while working).</summary>
internal enum CardTone
{
    Normal,
    Working,
    Success,
    Caution,
    Failed,
}

/// <summary>One action button of a card (W153D: Copy · Try again · Open in theuth; Replace · Discard). <see cref="Cap"/> = its key cap.</summary>
internal sealed record CardButton(string Id, string Label, string? Cap = null, bool Primary = false);

/// <summary>A command step row as the card draws it (§10.3): verb · text, green/red when returned, a ring on the running row.</summary>
internal sealed record CardStepRow(string Verb, string Text, StepStatus Status, string? Note = null);

/// <summary>The amber approval box (§10.6).</summary>
internal sealed record CardApprovalBox(string Question, string RiskLine, string? Context, string? Line, bool Answered);

/// <summary>
/// What <see cref="AnswerCardWindow"/> shows — one content model for the voice key's and the command key's answers, the Ask bar's, and
/// (PIN-C) W153D's failure card and rewrite panel. Collapsed shows <see cref="Collapsed"/> (or the first lines of <see cref="Body"/>);
/// expanded shows <see cref="Request"/>, the markdown <see cref="Body"/>, steps, tool chips and the follow-up field.
/// </summary>
internal sealed record AnswerCardContent
{
    public string Title { get; init; } = "theuth desk";

    /// <summary>"replied just now" · "2 of 3" · "done · 6 s".</summary>
    public string? Subtitle { get; init; }

    /// <summary>The thread position dots (§12.2); drawn when <see cref="ThreadCount"/> &gt; 1.</summary>
    public int ThreadIndex { get; init; }

    public int ThreadCount { get; init; }

    /// <summary>The request in quotes (expanded).</summary>
    public string? Request { get; init; }

    /// <summary>Markdown (lists, headings, fenced code, inline marks) — <see cref="AnswerMarkdown"/>.</summary>
    public string Body { get; init; } = "";

    /// <summary>The collapsed form's two lines (<c>AskReply.FirstTwoLines</c>); "" = the first lines of <see cref="Body"/>.</summary>
    public string Collapsed { get; init; } = "";

    /// <summary>The phase's copy ("thinking · code_refs · 4 s", "couldn't reply — …").</summary>
    public string Status { get; init; } = "";

    public CardTone Tone { get; init; }

    /// <summary>The status line pulses (thinking never looks frozen, §12.4).</summary>
    public bool Busy { get; init; }

    public IReadOnlyList<ToolChip> Chips { get; init; } = Array.Empty<ToolChip>();

    public IReadOnlyList<CardStepRow> Steps { get; init; } = Array.Empty<CardStepRow>();

    /// <summary>The command summary line under done ("4 steps  ls · mkdir/mv · ls · applescript").</summary>
    public string? StepSummary { get; init; }

    public CardApprovalBox? Approval { get; init; }

    public IReadOnlyList<CardButton> Actions { get; init; } = Array.Empty<CardButton>();

    /// <summary>The key hints line ("Enter expand · O open · Esc").</summary>
    public string? Footer { get; init; }

    /// <summary>Show the follow-up field when expanded (§12.2 "follow up in place").</summary>
    public bool FollowUp { get; init; }

    public string FollowUpHint { get; init; } = "follow up…";

    /// <summary>The speaker glyph (149s): our answer is being read aloud; a click stops it.</summary>
    public bool Speaking { get; init; }

    /// <summary>"O open": the desk session (<c>theuth://c/&lt;desk&gt;</c>).</summary>
    public string? OpenUrl { get; init; }
}

/// <summary>
/// 153 §12.1–§12.3, §12.7 (D12): the ONE answer card window — a <see cref="ToolWindow"/> sibling of the island with the two anatomies:
/// collapsed (the receipt's two lines + key caps) and expanded (440 × ≤ 70 % of the display's visible height, scrolling inside:
/// the request, the markdown answer, command steps, tool chips, the follow-up field). It sits in the island area of the display under
/// the pointer when <see cref="Begin"/> ran (149 island fix P / §18 Q9 — it never moves mid-gesture) and resizes in place. Hides are
/// generation-checked: <see cref="Hide"/> ignores a generation older than the latest <see cref="Begin"/>. Shown without activation;
/// <see cref="TakeKey"/> (a click, an expand, an approval box) makes it key so ⏎ / O / J / K / Ctrl+L / ⎋ arrive as
/// <see cref="KeyIntent"/> (<see cref="CardKeys"/>). The markdown renders through <see cref="AnswerMarkdown"/> into TextBlocks (the
/// §12.2 RichTextBlock fallback: the Community Toolkit's MarkdownTextBlock is not referenced by the 134 build).
/// <para>PIN-C API: <see cref="Shared"/>, <see cref="Begin"/>, <see cref="Present"/>, <see cref="Hide"/>, <see cref="Expanded"/> /
/// <see cref="SetExpanded"/>, <see cref="TakeKey"/>, events <see cref="KeyIntent"/>, <see cref="ActionInvoked"/>,
/// <see cref="FollowUpSubmitted"/>, <see cref="ExpandedChanged"/>, <see cref="Dismissed"/>, <see cref="SpeakerClicked"/>.</para>
/// UIA: <c>answer-card</c>, <c>answer-card-title</c>, <c>answer-card-status</c>, <c>answer-card-body</c>, <c>answer-card-step-&lt;n&gt;</c>,
/// <c>answer-card-approval</c>, <c>answer-card-approve</c>, <c>answer-card-deny</c>, <c>answer-card-chip-&lt;n&gt;</c>,
/// <c>answer-card-followup</c>, <c>answer-card-action-&lt;id&gt;</c>, <c>answer-card-close</c>.
/// </summary>
internal sealed partial class AnswerCardWindow : ToolWindow
{
    public const double CardWidth = 440;
    public const double MaxScreenShare = 0.70;

    private static AnswerCardWindow? shared;
    private readonly UserControl host;
    private readonly Border shell;
    private readonly StackPanel content;
    private AnswerCardContent model = new();
    private ScreenMonitor? anchor;
    private TextBox? followUp;
    private string followUpDraft = "";
    private string? material;
    private bool shownOnce;

    private AnswerCardWindow()
        : base("theuth answer", WINDOW_EX_STYLE.WS_EX_NOACTIVATE, excludeFromCapture: !Flags.Shareable)
    {
        content = new StackPanel { Spacing = 6 };
        shell = new Border { Padding = new Thickness(14, 10, 14, 12), Child = content };
        host = new UserControl { Content = shell, IsTabStop = true, UseSystemFocusVisuals = false };
        AutomationProperties.SetAutomationId(host, "answer-card");
        AutomationProperties.SetName(host, "theuth answer");
        host.PreviewKeyDown += OnKey;
        host.PointerWheelChanged += (_, e) =>
        {
            // §12.2: "any scroll" expands the collapsed card.
            if (!Expanded && IsShown) { SetExpanded(Generation, true); e.Handled = true; }
        };
        host.Tapped += (_, e) =>
        {
            if (e.OriginalSource is TextBox) return;
            TakeKey();
        };
        Content = host;
        if (!Shape.RoundCorners(Hwnd)) Log.Write("answer-card: DWM rounded corners unavailable (rectangular)");
        material = FrameworkBackdrop.Apply(this, "answer-card");
        Theme.Follow(host, () => { FrameworkBackdrop.ImmersiveDark(Hwnd, host.ActualTheme); if (IsShown) Render(); });
        FrameworkBackdrop.ImmersiveDark(Hwnd, host.ActualTheme);
    }

    /// <summary>The one instance (created on first use on the UI thread).</summary>
    public static AnswerCardWindow Shared() => shared ??= new AnswerCardWindow();

    /// <summary>The instance if one was created.</summary>
    public static AnswerCardWindow? Instance => shared;

    /// <summary>The latest <see cref="Begin"/>'s number; a hide or present for an older one is ignored.</summary>
    public int Generation { get; private set; }

    /// <summary>Who began the current generation (<c>voice</c>, <c>command</c>, <c>ask</c>, <c>anyapp-failure</c>, <c>rewrite</c>…), for the log.</summary>
    public string Owner { get; private set; } = "";

    public bool Expanded { get; private set; }

    public AnswerCardContent Model => model;

    /// <summary>A card key while the card is key (<see cref="CardKeys.IntentFor"/>): (generation, intent).</summary>
    public event Action<int, CardKeyIntent>? KeyIntent;

    /// <summary>A <see cref="CardButton"/> was clicked, or the approval box's Yes/No (<c>approve</c> / <c>deny</c>): (generation, id).</summary>
    public event Action<int, string>? ActionInvoked;

    /// <summary>⏎ in the follow-up field: (generation, text).</summary>
    public event Action<int, string>? FollowUpSubmitted;

    /// <summary>Expanded or collapsed by a click, a scroll or a key: (generation, expanded).</summary>
    public event Action<int, bool>? ExpandedChanged;

    /// <summary>The ✕ / Esc cap was clicked: (generation).</summary>
    public event Action<int>? Dismissed;

    /// <summary>The speaker glyph was clicked ("click to stop").</summary>
    public event Action<int>? SpeakerClicked;

    /// <summary>
    /// A new card generation: older hides become no-ops; the card will sit in the island area of the display under the pointer NOW
    /// (§18 Q9: chosen at press, fixed for the gesture). <paramref name="keepPlace"/> keeps the current display (a follow-up in place).
    /// </summary>
    public int Begin(string owner, bool keepPlace = false)
    {
        Generation++;
        Owner = owner;
        if (!keepPlace || anchor is null)
        {
            var all = ScreenMonitor.All();
            anchor = PInvoke.GetCursorPos(out var p) ? ScreenMonitor.At(all, p.X, p.Y) : ScreenMonitor.At(all, 0, 0);
        }
        return Generation;
    }

    /// <summary>Render <paramref name="c"/> for <paramref name="generation"/> (ignored when stale) and show it without taking focus.</summary>
    public bool Present(int generation, AnswerCardContent c, bool? expanded = null)
    {
        if (generation != Generation) return false;
        model = c;
        if (expanded is { } e) Expanded = e;
        Render();
        if (!IsShown)
        {
            ShowPassive();
            if (!shownOnce) { shownOnce = true; Log.Write($"answer-card: first show ({material ?? "solid"}) owner={Owner}"); }
        }
        return true;
    }

    /// <summary>Generation-checked hide (§12.4): false when <paramref name="generation"/> is older than the latest <see cref="Begin"/>.</summary>
    public bool Hide(int generation, string why = "hide")
    {
        if (generation != Generation)
        {
            Log.Write($"answer-card: hide({generation}, {why}) ignored — generation {Generation} is newer");
            return false;
        }
        if (IsShown)
        {
            bool wasForeground = IsForeground;
            HideWindow();
            if (wasForeground) Log.Write($"answer-card: hidden while key ({why})");
        }
        Expanded = false;
        followUpDraft = "";
        return true;
    }

    public void SetExpanded(int generation, bool expanded)
    {
        if (generation != Generation || Expanded == expanded) return;
        Expanded = expanded;
        Render();
        if (expanded) TakeKey();
        Log.Write($"answer-card: {(expanded ? "expanded" : "collapsed")} owner={Owner} gen={generation}");
        ExpandedChanged?.Invoke(generation, expanded);
    }

    /// <summary>Make the card key (keys arrive here); false when Windows refused the foreground (clicks and voice still work).</summary>
    public bool TakeKey()
    {
        if (!IsShown) return false;
        bool ok = ShowActive();
        host.Focus(FocusState.Programmatic);
        if (!ok) Log.Write("answer-card: Windows refused the foreground — the card answers clicks (and voice) until clicked");
        return ok;
    }

    /// <summary>153 probes: the card's XAML rendered to a PNG (the tree itself — capture exclusion does not blank it).</summary>
    public async Task<bool> CapturePng(string path)
    {
        if (!IsShown) return false;
        try
        {
            await XamlPng.Write(shell, path);
            return true;
        }
        catch (Exception e)
        {
            Log.Write($"answer-card: capture {path} failed: {e.GetType().Name}: {e.Message}");
            return false;
        }
    }

    /// <summary>Whether the follow-up field has focus (letters are text there).</summary>
    public bool FollowUpFocused => followUp is { } f && f.FocusState != FocusState.Unfocused;

    protected override void OnCloseRequested() => Dismissed?.Invoke(Generation);

    private void OnKey(object sender, KeyRoutedEventArgs e)
    {
        var press = KeyInput.From(e);
        var ctx = new CardKeyContext(model.Approval is { Answered: false }, Expanded, model.Busy && model.Approval is null, model.Speaking, FollowUpFocused);
        var intent = CardKeys.IntentFor(press, ctx);
        if (intent == CardKeyIntent.None) return;
        if (intent == CardKeyIntent.SendFollowUp)
        {
            var text = (followUp?.Text ?? "").Trim();
            if (text.Length == 0) return;
            e.Handled = true;
            followUpDraft = "";
            if (followUp is not null) followUp.Text = "";
            FollowUpSubmitted?.Invoke(Generation, text);
            return;
        }
        e.Handled = true;
        if (intent == CardKeyIntent.ToggleExpand) { SetExpanded(Generation, !Expanded); return; }
        if (intent == CardKeyIntent.Collapse) { SetExpanded(Generation, false); return; }
        KeyIntent?.Invoke(Generation, intent);
    }

    // MARK: render

    private void Render()
    {
        var theme = host.ActualTheme;
        host.Background = SystemBackdrop is null ? Theme.Brush(NativeLook.SolidBackgroundBase, theme) : Theme.Transparent;
        host.Foreground = Theme.Brush(NativeLook.TextPrimary, theme);
        if (followUp is not null) followUpDraft = followUp.Text;
        content.Children.Clear();
        followUp = null;
        var c = model;
        content.Children.Add(Header(c, theme));
        if (Expanded && c.Request is { Length: > 0 } req)
        {
            var q = Text($"“{req}”", 13, NativeLook.TextSecondary);
            q.FontStyle = global::Windows.UI.Text.FontStyle.Italic;
            q.TextWrapping = TextWrapping.Wrap;
            q.MaxLines = 3;
            content.Children.Add(q);
        }
        if (c.Steps.Count > 0) content.Children.Add(Steps(c, theme));
        if (c.Approval is { } box) content.Children.Add(Approval(box, theme));
        var bodyText = Expanded ? c.Body : (c.Collapsed.Length > 0 ? c.Collapsed : c.Body);
        if (bodyText.Length > 0)
        {
            if (Expanded)
            {
                var md = Markdown(bodyText, theme);
                AutomationProperties.SetAutomationId(md, "answer-card-body");
                var scroll = new ScrollViewer { Content = md, VerticalScrollBarVisibility = ScrollBarVisibility.Auto, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled };
                scroll.MaxHeight = Math.Max(120, MaxBodyHeight());
                content.Children.Add(scroll);
            }
            else
            {
                var t = Text(bodyText, 13.5, NativeLook.TextPrimary);
                t.TextWrapping = TextWrapping.Wrap;
                t.MaxLines = 2;
                t.TextTrimming = TextTrimming.CharacterEllipsis;
                AutomationProperties.SetAutomationId(t, "answer-card-body");
                content.Children.Add(t);
            }
        }
        if (c.StepSummary is { Length: > 0 } sum) content.Children.Add(Text(sum, 12, NativeLook.TextTertiary));
        if (c.Chips.Count > 0 && (Expanded || c.Busy)) content.Children.Add(Chips(c, theme));
        if (c.Status.Length > 0) content.Children.Add(Status(c, theme));
        if (Expanded && c.FollowUp) content.Children.Add(FollowUpRow(c, theme));
        if (c.Actions.Count > 0) content.Children.Add(ActionsRow(c, theme));
        if (c.Footer is { Length: > 0 } foot)
        {
            var f = Text(foot, 11.5, NativeLook.TextTertiary);
            f.TextWrapping = TextWrapping.Wrap;
            content.Children.Add(f);
        }
        Place();
    }

    private FrameworkElement Header(AnswerCardContent c, ElementTheme theme)
    {
        var row = new Grid { ColumnSpacing = 8 };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var left = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Center };
        var title = Text(c.Title, 12.5, NativeLook.TextPrimary, FontWeights.SemiBold);
        AutomationProperties.SetAutomationId(title, "answer-card-title");
        left.Children.Add(title);
        if (c.Subtitle is { Length: > 0 } sub) left.Children.Add(Text("· " + sub, 12, NativeLook.TextSecondary));
        if (c.ThreadCount > 1) left.Children.Add(CardChrome.PositionDots(theme, c.ThreadIndex + 1, c.ThreadCount));
        row.Children.Add(left);
        var right = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
        if (c.Speaking)
        {
            var speaker = new Button { Content = Theme.Icon(NativeLook.Glyph.Volume, theme, NativeLook.AccentText, 13), Padding = new Thickness(6, 3, 6, 3), MinWidth = 0, MinHeight = 0, BorderThickness = new Thickness(0), Background = Theme.Transparent };
            ToolTipService.SetToolTip(speaker, "click to stop");
            AutomationProperties.SetName(speaker, "Stop reading aloud");
            AutomationProperties.SetAutomationId(speaker, "answer-card-speaker");
            speaker.Click += (_, _) => SpeakerClicked?.Invoke(Generation);
            right.Children.Add(speaker);
        }
        right.Children.Add(CardChrome.KeyCapButton(theme, null, "Esc", () => Dismissed?.Invoke(Generation), "answer-card-close"));
        Grid.SetColumn(right, 1);
        row.Children.Add(right);
        return row;
    }

    private FrameworkElement Steps(AnswerCardContent c, ElementTheme theme)
    {
        var list = new StackPanel { Spacing = 2 };
        AutomationProperties.SetAutomationId(list, "answer-card-steps");
        int n = 0;
        foreach (var s in c.Steps)
        {
            var row = new Grid { ColumnSpacing = 10 };
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(72) });
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            string key = s.Status switch
            {
                StepStatus.Ok => NativeLook.Success,
                StepStatus.Failed or StepStatus.Denied => NativeLook.Critical,
                StepStatus.Running => NativeLook.AccentText,
                _ => NativeLook.TextTertiary,
            };
            var verb = Text(s.Verb, 12, key, FontWeights.SemiBold);
            row.Children.Add(verb);
            var text = Text(s.Note is { Length: > 0 } note ? $"{s.Text} — {note}" : s.Text, 12.5, s.Status == StepStatus.Next ? NativeLook.TextSecondary : NativeLook.TextPrimary);
            text.TextTrimming = TextTrimming.CharacterEllipsis;
            Grid.SetColumn(text, 1);
            row.Children.Add(text);
            if (s.Status == StepStatus.Running)
            {
                var ring = new ProgressRing { IsActive = true, Width = 12, Height = 12, MinWidth = 12, MinHeight = 12 };
                Grid.SetColumn(ring, 2);
                row.Children.Add(ring);
            }
            AutomationProperties.SetAutomationId(row, $"answer-card-step-{++n}");
            AutomationProperties.SetName(row, $"{s.Verb} {s.Text}");
            list.Children.Add(row);
        }
        return list;
    }

    private FrameworkElement Approval(CardApprovalBox box, ElementTheme theme)
    {
        var inner = new StackPanel { Spacing = 4 };
        var q = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
        q.Children.Add(Theme.Icon(NativeLook.Glyph.Warning, theme, NativeLook.Caution, 13));
        var question = Text(box.Question, 13, NativeLook.TextPrimary, FontWeights.SemiBold);
        question.FontFamily = new FontFamily("Cascadia Mono, Consolas");
        question.TextWrapping = TextWrapping.Wrap;
        question.MaxWidth = CardWidth - 90;
        q.Children.Add(question);
        inner.Children.Add(q);
        inner.Children.Add(Text(box.RiskLine, 12, NativeLook.TextSecondary));
        if (box.Context is { Length: > 0 } ctx) inner.Children.Add(Text(ctx, 12, NativeLook.TextSecondary));
        var buttons = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        if (!box.Answered)
        {
            buttons.Children.Add(CardChrome.KeyCapButton(theme, "Yes", "Enter", () => ActionInvoked?.Invoke(Generation, "approve"), "answer-card-approve"));
            buttons.Children.Add(CardChrome.KeyCapButton(theme, "No", "Esc", () => ActionInvoked?.Invoke(Generation, "deny"), "answer-card-deny"));
            buttons.Children.Add(Text("or say “yes” / “no”", 12, NativeLook.TextTertiary));
        }
        else
        {
            buttons.Children.Add(Text("answered — waiting for the desk…", 12, NativeLook.TextTertiary));
        }
        inner.Children.Add(buttons);
        if (box.Line is { Length: > 0 } line) inner.Children.Add(Text(line, 12, NativeLook.AccentText));
        var border = new Border
        {
            Child = inner,
            Padding = new Thickness(10, 8, 10, 8),
            CornerRadius = CardChrome.OverlayRadius,
            BorderThickness = new Thickness(1),
            BorderBrush = Theme.Brush(NativeLook.Caution, theme),
            Background = Theme.Brush(NativeLook.ControlFill, theme),
        };
        AutomationProperties.SetAutomationId(border, "answer-card-approval");
        AutomationProperties.SetName(border, $"{box.Question} {box.RiskLine}");
        return border;
    }

    private FrameworkElement Chips(AnswerCardContent c, ElementTheme theme)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4 };
        int n = 0;
        foreach (var chip in c.Chips.TakeLast(6))
        {
            var b = new Border
            {
                CornerRadius = CardChrome.ControlRadius,
                Padding = new Thickness(6, 1, 6, 2),
                BorderThickness = new Thickness(1),
                BorderBrush = Theme.Brush(chip.Running ? NativeLook.Accent : chip.Failed ? NativeLook.Critical : NativeLook.ControlStroke, theme),
                Background = Theme.Brush(NativeLook.ControlFill, theme),
                Child = Text(chip.Name, 11.5, chip.Running ? NativeLook.AccentText : NativeLook.TextSecondary),
            };
            AutomationProperties.SetAutomationId(b, $"answer-card-chip-{++n}");
            AutomationProperties.SetName(b, chip.Name);
            if (c.OpenUrl is { } url) b.Tapped += (_, _) => _ = Links.Open(new Uri(url));
            row.Children.Add(b);
        }
        if (c.Chips.Count > 6) row.Children.Add(Text($"+{c.Chips.Count - 6}", 11.5, NativeLook.TextTertiary));
        return new ScrollViewer { Content = row, HorizontalScrollBarVisibility = ScrollBarVisibility.Hidden, VerticalScrollBarVisibility = ScrollBarVisibility.Disabled, HorizontalScrollMode = ScrollMode.Enabled };
    }

    private FrameworkElement Status(AnswerCardContent c, ElementTheme theme)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
        if (c.Busy) row.Children.Add(new ProgressRing { IsActive = true, Width = 12, Height = 12, MinWidth = 12, MinHeight = 12 });
        string key = c.Tone switch
        {
            CardTone.Failed => NativeLook.Critical,
            CardTone.Caution => NativeLook.Caution,
            CardTone.Success => NativeLook.Success,
            CardTone.Working => NativeLook.AccentText,
            _ => NativeLook.TextSecondary,
        };
        var t = Text(c.Status, 12, key);
        t.TextWrapping = TextWrapping.Wrap;
        t.MaxWidth = CardWidth - 50;
        AutomationProperties.SetAutomationId(t, "answer-card-status");
        row.Children.Add(t);
        return row;
    }

    private FrameworkElement FollowUpRow(AnswerCardContent c, ElementTheme theme)
    {
        var box = new TextBox { PlaceholderText = c.FollowUpHint, Text = followUpDraft, AcceptsReturn = false, FontSize = 13 };
        AutomationProperties.SetAutomationId(box, "answer-card-followup");
        AutomationProperties.SetName(box, "Follow up");
        followUp = box;
        return box;
    }

    private FrameworkElement ActionsRow(AnswerCardContent c, ElementTheme theme)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        foreach (var a in c.Actions)
        {
            Button b;
            if (a.Cap is { Length: > 0 } cap) b = CardChrome.KeyCapButton(theme, a.Label, cap, () => ActionInvoked?.Invoke(Generation, a.Id), $"answer-card-action-{a.Id}");
            else
            {
                b = new Button { Content = a.Label, FontSize = 12, Padding = new Thickness(10, 3, 10, 4) };
                if (a.Primary && Application.Current?.Resources.TryGetValue("AccentButtonStyle", out var st) == true && st is Style style) b.Style = style;
                b.Click += (_, _) => ActionInvoked?.Invoke(Generation, a.Id);
                AutomationProperties.SetAutomationId(b, $"answer-card-action-{a.Id}");
            }
            row.Children.Add(b);
        }
        return row;
    }

    /// <summary>The markdown blocks as TextBlocks: headings, paragraphs with inline marks, bullets, fenced code (monospace, scrolls sideways).</summary>
    private FrameworkElement Markdown(string markdown, ElementTheme theme)
    {
        var panel = new StackPanel { Spacing = 6 };
        foreach (var block in AnswerMarkdown.Parse(markdown))
        {
            switch (block)
            {
                case MdBlock.Heading h:
                    panel.Children.Add(Inlines(h.Inlines, h.Level <= 2 ? 15 : 14, FontWeights.SemiBold, theme));
                    break;
                case MdBlock.Paragraph p:
                    panel.Children.Add(Inlines(p.Inlines, 13.5, FontWeights.Normal, theme));
                    break;
                case MdBlock.Bullet b:
                {
                    var g = new Grid { ColumnSpacing = 6, Margin = new Thickness(6, 0, 0, 0) };
                    g.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                    g.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                    g.Children.Add(Text(b.Numbered ? $"{b.Number}." : "•", 13.5, NativeLook.TextSecondary));
                    var t = Inlines(b.Inlines, 13.5, FontWeights.Normal, theme);
                    Grid.SetColumn(t, 1);
                    g.Children.Add(t);
                    panel.Children.Add(g);
                    break;
                }
                case MdBlock.Code code:
                {
                    var t = new TextBlock { Text = code.Text, FontFamily = new FontFamily("Cascadia Mono, Consolas"), FontSize = 12.5, IsTextSelectionEnabled = true, Foreground = Theme.Brush(NativeLook.TextPrimary, theme) };
                    panel.Children.Add(new Border
                    {
                        Background = Theme.Brush(NativeLook.ControlFill, theme),
                        CornerRadius = CardChrome.ControlRadius,
                        Padding = new Thickness(8, 6, 8, 6),
                        Child = new ScrollViewer { Content = t, HorizontalScrollBarVisibility = ScrollBarVisibility.Auto, VerticalScrollBarVisibility = ScrollBarVisibility.Disabled, HorizontalScrollMode = ScrollMode.Enabled },
                    });
                    break;
                }
            }
        }
        return panel;
    }

    private TextBlock Inlines(IReadOnlyList<MdInline> inlines, double size, global::Windows.UI.Text.FontWeight weight, ElementTheme theme)
    {
        var t = new TextBlock { FontSize = size, FontWeight = weight, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true, Foreground = Theme.Brush(NativeLook.TextPrimary, theme) };
        foreach (var i in inlines)
        {
            switch (i)
            {
                case MdInline.Text x: t.Inlines.Add(new Run { Text = x.Value }); break;
                case MdInline.Bold x: t.Inlines.Add(new Run { Text = x.Value, FontWeight = FontWeights.SemiBold }); break;
                case MdInline.Italic x: t.Inlines.Add(new Run { Text = x.Value, FontStyle = global::Windows.UI.Text.FontStyle.Italic }); break;
                case MdInline.Code x: t.Inlines.Add(new Run { Text = x.Value, FontFamily = new FontFamily("Cascadia Mono, Consolas") }); break;
                case MdInline.Link x:
                {
                    if (Uri.TryCreate(x.Url, UriKind.Absolute, out var uri))
                    {
                        var link = new Hyperlink();
                        link.Inlines.Add(new Run { Text = x.Value });
                        link.Click += (_, _) => _ = Links.Open(uri);
                        t.Inlines.Add(link);
                    }
                    else t.Inlines.Add(new Run { Text = x.Value });
                    break;
                }
            }
        }
        return t;
    }

    private static TextBlock Text(string s, double size, string key, global::Windows.UI.Text.FontWeight? weight = null)
    {
        var t = new TextBlock { Text = s, FontSize = size, TextTrimming = TextTrimming.CharacterEllipsis, VerticalAlignment = VerticalAlignment.Center };
        t.Foreground = Theme.Brush(key, ElementTheme.Default);
        if (weight is { } w) t.FontWeight = w;
        return t;
    }

    /// <summary>≤ 70 % of the display's visible height, minus the chrome around the body (DIPs).</summary>
    private double MaxBodyHeight()
    {
        var m = anchor ?? ScreenMonitor.At(ScreenMonitor.All(), 0, 0);
        double visible = m.WorkArea.Height / Math.Max(0.5, m.Scale);
        return visible * MaxScreenShare - 170;
    }

    /// <summary>Top centre of the anchored display's work area (the island area, <see cref="IslandModel.DefaultFrame"/>), sized to the content.</summary>
    private void Place()
    {
        var m = anchor ?? ScreenMonitor.At(ScreenMonitor.All(), 0, 0);
        double scale = Math.Max(0.5, m.Scale);
        shell.Measure(new global::Windows.Foundation.Size(CardWidth, double.PositiveInfinity));
        double maxH = m.WorkArea.Height / scale * MaxScreenShare;
        double h = Math.Clamp(shell.DesiredSize.Height + 2, 60, Math.Max(60, maxH));
        var size = ((int)Math.Round(CardWidth * scale), (int)Math.Round(h * scale));
        var f = IslandModel.DefaultFrame(m.AsIslandDisplay(), size);
        Place(new RectInt32(f.X, f.Y, f.Width, f.Height));
    }
}

/// <summary>A XAML element rendered to a PNG file (RenderTargetBitmap → BitmapEncoder), for the 153 probes' evidence.</summary>
internal static class XamlPng
{
    public static async Task Write(UIElement element, string path)
    {
        var rtb = new Microsoft.UI.Xaml.Media.Imaging.RenderTargetBitmap();
        await rtb.RenderAsync(element);
        var pixels = await rtb.GetPixelsAsync();
        var bytes = System.Runtime.InteropServices.WindowsRuntime.WindowsRuntimeBufferExtensions.ToArray(pixels);
        if (Path.GetDirectoryName(path) is { Length: > 0 } dir) Directory.CreateDirectory(dir);
        using var stream = new global::Windows.Storage.Streams.InMemoryRandomAccessStream();
        var encoder = await global::Windows.Graphics.Imaging.BitmapEncoder.CreateAsync(global::Windows.Graphics.Imaging.BitmapEncoder.PngEncoderId, stream);
        encoder.SetPixelData(global::Windows.Graphics.Imaging.BitmapPixelFormat.Bgra8, global::Windows.Graphics.Imaging.BitmapAlphaMode.Premultiplied,
            (uint)rtb.PixelWidth, (uint)rtb.PixelHeight, 96, 96, bytes);
        await encoder.FlushAsync();
        stream.Seek(0);
        using var file = File.Create(path);
        await stream.AsStreamForRead().CopyToAsync(file);
    }
}