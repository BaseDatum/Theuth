#pragma warning disable CS0618 // NAudio 3.1: WasapiOut is marked obsolete in favour of WasapiPlayerBuilder; SpeechPlayer pairs the same classic API.
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Media;
using NAudio.CoreAudioApi;
using NAudio.Wave;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Theuth.Companion.Rpc;
using Windows.Graphics;
using Windows.Win32.Foundation;

namespace Theuth.Companion.Ui;

/// <summary>
/// 153 §6.4 (mock frame D) — the dictionary suggestion card, the Windows twin of the macOS <c>GlassPanel</c> at the Ask bar's anchor:
/// "Dictionary · 2 times this week" / "You changed <s>parakeet tee dee tee</s> → <c>Parakeet TDT</c> after dictating." / "Heard the same
/// way twice (Companion, 7 days). Add it as a replacement?" / [Add] [Not now] [Never for this] ▶ play what you said. Shown for a
/// <c>DictionarySuggestion</c> event whose surface is the companion's (its own fields or any-app — the surface that produced the last
/// occurrence shows the card, W-o); each button answers <c>dictionary_suggestion_answer(id, add | not_now | never)</c>; after
/// <see cref="SuggestionCopy.AutoDismiss"/> it goes away unanswered (counts as "Not now" without the cooldown — nothing is sent).
/// "▶ play" fetches <c>dictation_audio_get(dictation, null)</c> (16 kHz i16) and plays it, shown only when <c>has_audio</c>. A
/// <see cref="ToolWindow"/> with the card chrome (Mica Base, round corners, the Fluent ramp); shown without taking the foreground.
/// UIA: <c>suggestion-card</c>, <c>suggestion-add</c> · <c>suggestion-not-now</c> · <c>suggestion-never</c> · <c>suggestion-play</c>.
/// </summary>
internal sealed partial class SuggestionCardWindow : ToolWindow
{
    public const double CardWidth = 420;

    private readonly Store store;
    private readonly Func<RECT?> anchor;
    private readonly UserControl host;
    private readonly StackPanel content;
    private DispatcherQueueTimer? dismiss;
    private DictionarySuggestion? current;
    private string? audioDictation;
    private WasapiOut? output;
    private string? material;

    private SuggestionCardWindow(Store store, Func<RECT?> anchor)
        : base("theuth — Dictionary suggestion", default, excludeFromCapture: !Flags.Shareable)
    {
        this.store = store;
        this.anchor = anchor;
        content = new StackPanel { Padding = new Thickness(14, 12, 14, 12), Spacing = 8 };
        host = new UserControl { Content = content, IsTabStop = false, UseSystemFocusVisuals = false };
        AutomationProperties.SetAutomationId(host, "suggestion-card");
        AutomationProperties.SetName(host, "Dictionary suggestion");
        Content = host;
        if (!Shape.RoundCorners(Hwnd)) Log.Write("suggestion card: DWM rounded corners unavailable (rectangular)");
        material = FrameworkBackdrop.Apply(this, "suggestion card");
        Theme.Follow(host, () => { FrameworkBackdrop.ImmersiveDark(Hwnd, host.ActualTheme); if (current is not null) Render(); });
        FrameworkBackdrop.ImmersiveDark(Hwnd, host.ActualTheme);
    }

    /// <summary>The card on screen now, or null.</summary>
    public DictionarySuggestion? Current => current;

    /// <summary>The last answer sent ("add" / "not_now" / "never" / "dismissed"), for the probe log.</summary>
    public string? LastOutcome { get; private set; }

    /// <summary>
    /// Wires the card to the Store's raw stream (153 W153B; no hook in another wave's file): a <c>DictionarySuggestion</c> for a companion
    /// surface shows it; <paramref name="anchor"/> is the Ask bar's rect while it is on screen (the card sits under it), else null.
    /// </summary>
    public static SuggestionCardWindow Install(Store store, Func<RECT?> anchor)
    {
        var card = new SuggestionCardWindow(store, anchor);
        store.EventReceived += (type, data) =>
        {
            if (type != "DictionarySuggestion") return;
            DictionarySuggestion s;
            try { s = DictionarySuggestion.Decode(data); }
            catch (Exception e) { Log.Write($"suggestion card: undecodable DictionarySuggestion ({e.Message})"); return; }
            if (!ForCompanion(s))
            {
                Log.Write($"suggestion card: {s.Id} is for surface {s.Surface?.Word() ?? "unknown"} — not ours");
                return;
            }
            card.Show(s, store.Ask.Dictation.LastId);
        };
        Log.Write("suggestion card: installed (DictionarySuggestion for companion / any_app surfaces)");
        return card;
    }

    /// <summary>The companion shows the card for its own fields and for any-app dictation (the surface of the last occurrence, W-o).</summary>
    public static bool ForCompanion(DictionarySuggestion s) => s.Surface is Surface.Companion or Surface.AnyApp;

    public void Show(DictionarySuggestion s, string? dictation)
    {
        current = s;
        audioDictation = dictation;
        LastOutcome = null;
        Render();
        ShowPassive();
        DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, Place);
        dismiss?.Stop();
        dismiss = DispatcherQueue.CreateTimer();
        dismiss.Interval = SuggestionCopy.AutoDismiss;
        dismiss.IsRepeating = false;
        dismiss.Tick += (_, _) =>
        {
            dismiss?.Stop();
            if (current?.Id != s.Id) return;
            Log.Write($"suggestion card: {s.Id} auto-dismissed after {SuggestionCopy.AutoDismiss.TotalSeconds:F0} s — counts as not now without the cooldown (nothing sent)");
            LastOutcome = "dismissed";
            Hide();
        };
        dismiss.Start();
        Log.Write($"suggestion card: shown {s.Id} heard=\"{s.Heard}\" typed=\"{s.Typed}\" occurrences={s.Occurrences} surface={s.Surface?.Word() ?? "?"} audio={(s.HasAudio ? "yes" : "no")}");
    }

    private void Hide()
    {
        dismiss?.Stop();
        current = null;
        StopAudio();
        HideWindow();
    }

    protected override void OnCloseRequested() => Hide();

    private async void Answer(SuggestionAnswer answer)
    {
        if (current is not { } s) return;
        LastOutcome = answer.Word();
        Hide();
        try
        {
            await store.Client.DictionarySuggestionAnswerAsync(s.Id, answer);
            Log.Write($"suggestion card: dictionary_suggestion_answer({s.Id}, {answer.Word()}) ok");
        }
        catch (Exception e)
        {
            Log.Write($"suggestion card: dictionary_suggestion_answer({s.Id}, {answer.Word()}) failed: {(e is RpcException r ? r.ToString() : e.Message)}");
        }
    }

    private async void Play()
    {
        if (current is not { } s || audioDictation is not { } id) return;
        try
        {
            var b64 = await store.Client.DictationAudioGetAsync(id, null);
            var bytes = Convert.FromBase64String(b64);
            StopAudio();
            var buffer = new BufferedWaveProvider(new WaveFormat(16_000, 16, 1), TimeSpan.FromSeconds(Math.Max(5, bytes.Length / 32_000.0 + 1))) { DiscardOnBufferOverflow = true };
            buffer.AddSamples(bytes, 0, bytes.Length);
            using var en = new MMDeviceEnumerator();
            var dev = en.GetDefaultAudioEndpoint(DataFlow.Render, Role.Multimedia);
            output = new WasapiOut(dev, AudioClientShareMode.Shared, true, 60);
            output.Init(buffer);
            output.Play();
            Log.Write($"suggestion card: playing {bytes.Length / 2} samples of {id} for {s.Id} on \"{dev.FriendlyName}\"");
        }
        catch (Exception e)
        {
            Log.Write($"suggestion card: play failed: {(e is RpcException r ? r.ToString() : e.Message)}");
        }
    }

    private void StopAudio()
    {
        try { output?.Stop(); output?.Dispose(); }
        catch (Exception) { }
        output = null;
    }

    private void Render()
    {
        if (current is not { } s) return;
        var theme = host.ActualTheme;
        host.Background = SystemBackdrop is null ? Theme.Brush(NativeLook.SolidBackgroundBase, theme) : Theme.Transparent;
        host.Foreground = Theme.Brush(NativeLook.TextPrimary, theme);
        content.Children.Clear();

        var head = new Grid();
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        head.Children.Add(new TextBlock { Text = SuggestionCopy.Header, FontSize = CardChrome.Meta, Foreground = Theme.Brush(NativeLook.TextSecondary, theme) });
        var count = new TextBlock { Text = SuggestionCopy.CountLabel(s), FontSize = CardChrome.Meta, Foreground = Theme.Brush(NativeLook.TextTertiary, theme) };
        AutomationProperties.SetAutomationId(count, "suggestion-count");
        Grid.SetColumn(count, 1);
        head.Children.Add(count);
        content.Children.Add(head);

        var body = new TextBlock { FontSize = CardChrome.Body, TextWrapping = TextWrapping.Wrap };
        body.Inlines.Add(new Run { Text = "You changed " });
        body.Inlines.Add(new Run { Text = s.Heard, Foreground = Theme.Brush(NativeLook.TextSecondary, theme), TextDecorations = global::Windows.UI.Text.TextDecorations.Strikethrough });
        body.Inlines.Add(new Run { Text = " → " });
        body.Inlines.Add(new Run { Text = s.Typed, FontFamily = new FontFamily("Cascadia Mono, Consolas"), FontSize = CardChrome.Body - 1 });
        body.Inlines.Add(new Run { Text = " after dictating." });
        AutomationProperties.SetAutomationId(body, "suggestion-text");
        AutomationProperties.SetName(body, SuggestionCopy.Sentence(s));
        content.Children.Add(body);
        content.Children.Add(new TextBlock { Text = SuggestionCopy.Detail(s), FontSize = CardChrome.Meta, TextWrapping = TextWrapping.Wrap, Foreground = Theme.Brush(NativeLook.TextSecondary, theme) });

        var row = new Grid { Margin = new Thickness(0, 2, 0, 0) };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var buttons = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
        buttons.Children.Add(Button("Add", "suggestion-add", true, () => Answer(SuggestionAnswer.Add), theme));
        buttons.Children.Add(Button("Not now", "suggestion-not-now", false, () => Answer(SuggestionAnswer.NotNow), theme));
        buttons.Children.Add(Button("Never for this", "suggestion-never", false, () => Answer(SuggestionAnswer.Never), theme));
        row.Children.Add(buttons);
        if (s.HasAudio && audioDictation is not null)
        {
            var play = new HyperlinkButton { Content = SuggestionCopy.Play, FontSize = CardChrome.Meta, Padding = new Thickness(4, 2, 0, 2), VerticalAlignment = VerticalAlignment.Center };
            AutomationProperties.SetAutomationId(play, "suggestion-play");
            play.Click += (_, _) => Play();
            Grid.SetColumn(play, 2);
            row.Children.Add(play);
        }
        content.Children.Add(row);
        Place();
    }

    private static Button Button(string label, string id, bool primary, Action act, ElementTheme theme)
    {
        var b = new Button { Content = label, FontSize = CardChrome.Meta, Padding = new Thickness(10, 4, 10, 5), MinWidth = 0, MinHeight = 0, CornerRadius = CardChrome.ControlRadius };
        if (primary && Application.Current?.Resources.TryGetValue("AccentButtonStyle", out var st) == true && st is Style style) b.Style = style;
        AutomationProperties.SetAutomationId(b, id);
        AutomationProperties.SetName(b, label);
        b.Click += (_, _) => act();
        return b;
    }

    /// <summary>Under the Ask bar when it is on screen (the Mac card's anchor); else where the bar opens: top-centre of the display under the pointer.</summary>
    private void Place()
    {
        if (current is null) return;
        double s = Scale;
        content.Measure(new global::Windows.Foundation.Size(CardWidth, double.PositiveInfinity));
        double hEpx = Math.Clamp(content.DesiredSize.Height <= 0 ? 150 : content.DesiredSize.Height, 110, 320);
        int w = (int)Math.Round(CardWidth * s), h = (int)Math.Round(hEpx * s), gap = (int)Math.Round(8 * s);
        int x, y;
        PointInt32 probe;
        if (anchor() is { } a)
        {
            probe = new PointInt32((a.left + a.right) / 2, (a.top + a.bottom) / 2);
            x = probe.X - w / 2;
            y = a.bottom + gap;
        }
        else
        {
            var c = Win32.Cursor();
            probe = new PointInt32(c.X, c.Y);
            var wa0 = DisplayArea.GetFromPoint(probe, DisplayAreaFallback.Primary).WorkArea;
            x = wa0.X + (wa0.Width - w) / 2;
            y = wa0.Y + (int)Math.Round(wa0.Height * 0.2);
        }
        var wa = DisplayArea.GetFromPoint(probe, DisplayAreaFallback.Primary).WorkArea;
        x = Math.Clamp(x, wa.X + gap, Math.Max(wa.X + gap, wa.X + wa.Width - w - gap));
        y = Math.Clamp(y, wa.Y + gap, Math.Max(wa.Y + gap, wa.Y + wa.Height - h - gap));
        Place(new RectInt32(x, y, w, h));
    }

    public string BackdropText => material is null or "none" ? "solid" : material;
}
