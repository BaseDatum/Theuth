using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.AskBar;

/// <summary>
/// 153 W153B — <c>--ask-probe N --ask-fixture shaped|send-it|suggestion|all</c>: RECORDED engine items (the wave-A wire shapes of
/// <c>VoiceEvent::DictationText</c> with <c>shaped</c> / <c>command</c> / <c>actions</c>, <c>DictationFinished</c>,
/// <c>DictionarySuggestion</c>) through the Store's real reducer and the Ask bar's real voice machine
/// (<see cref="AskBarModel.ProbeFixture"/>) — the installed engine on build-B has no model and the companion streams its own
/// microphone, so the shaped text, the "send it" path and the suggestion card are proven from fixtures before H-ENGINE.
/// <c>shaped</c>: partials and finals shaped → the field shows them → release → the 3 s hold → the desk. <c>send-it</c>: "ship it
/// tonight send it" → "sending in 1.5 s…" → the text goes to the desk with no hold. <c>suggestion</c>: the card (frame D).
/// <c>all</c>: the three, 8 s apart. Every step to <c>logs\ask-probe.log</c>.
/// </summary>
internal static class AskFixtures
{
    private const string Rec = "0199c3a2-7f00-7000-8000-00000000153b";

    /// <summary>Two utterances, each a muted partial then its shaped final (§4.3 rows 1 and 8).</summary>
    public static readonly string[] Shaped =
    {
        """{"type":"DictationText","id":"0199c3a2-7f00-7000-8000-00000000153b","utterance":0,"text":"hi dana comma the deploy","final":false,"shaped":"Hi Dana, the deploy","command":null,"actions":[]}""",
        """{"type":"DictationText","id":"0199c3a2-7f00-7000-8000-00000000153b","utterance":0,"text":"hi dana comma the deploy is done period","final":true,"shaped":"Hi Dana, the deploy is done.","command":null,"actions":[]}""",
        """{"type":"DictationText","id":"0199c3a2-7f00-7000-8000-00000000153b","utterance":1,"text":"version two point five","final":false,"shaped":"version 2.5","command":null,"actions":[]}""",
        """{"type":"DictationText","id":"0199c3a2-7f00-7000-8000-00000000153b","utterance":1,"text":"version two point five ships tonight","final":true,"shaped":"Version 2.5 ships tonight.","command":null,"actions":[]}""",
        """{"type":"DictationFinished","id":"0199c3a2-7f00-7000-8000-00000000153b","reason":{"reason":"done"}}""",
    };

    /// <summary>"ship it tonight send it" (§4.3 row 26): the phrase arms on a partial, sticks to the final, the engine stops (§7.3).</summary>
    public static readonly string[] SendIt =
    {
        """{"type":"DictationText","id":"0199c3a2-7f00-7000-8000-00000000153b","utterance":0,"text":"ship it tonight","final":false,"shaped":"Ship it tonight","command":null,"actions":[]}""",
        """{"type":"DictationText","id":"0199c3a2-7f00-7000-8000-00000000153b","utterance":0,"text":"ship it tonight send it","final":false,"shaped":"Ship it tonight","command":{"command":"send","near_miss":false},"actions":[]}""",
        """{"type":"DictationText","id":"0199c3a2-7f00-7000-8000-00000000153b","utterance":0,"text":"ship it tonight send it","final":true,"shaped":"Ship it tonight","command":{"command":"send","near_miss":false},"actions":[]}""",
        """{"type":"DictationFinished","id":"0199c3a2-7f00-7000-8000-00000000153b","reason":{"reason":"done"}}""",
    };

    /// <summary>The §6.3 step-3 event for a companion correction (frame D's pair).</summary>
    public const string Suggestion =
        """{"type":"DictionarySuggestion","id":"0199c3a2-7f00-7000-8000-0000000153d0","heard":"parakeet tee dee tee","typed":"Parakeet TDT","occurrences":2,"surface":"companion","has_audio":false}""";

    public static void Run(string name, Store store, AskBarInstall install, DispatcherQueue dispatcher)
    {
        AskLog.Write($"ask-fixture {name}: recorded id {Rec} → probe ids; pace 900 ms");
        switch (name)
        {
            case "shaped": Dictate("shaped", Shaped, store, install, dispatcher); break;
            case "send-it": Dictate("send-it", SendIt, store, install, dispatcher); break;
            case "suggestion": Suggest(store, install, dispatcher); break;
            default:
                Dictate("shaped", Shaped, store, install, dispatcher);
                After(dispatcher, 10, () => Dictate("send-it", SendIt, store, install, dispatcher));
                After(dispatcher, 18, () => Suggest(store, install, dispatcher));
                break;
        }
    }

    private static void Dictate(string name, string[] items, Store store, AskBarInstall install, DispatcherQueue dispatcher)
    {
        var model = install.Model;
        var id = $"probe-{name}-{Guid.NewGuid():N}"[..24];
        int step = 0;
        void Trace(string what) => AskLog.Write($"ask-fixture {name} {what}: field=\"{model.FieldText}\" live={(model.Live ? "true" : "false")} status=\"{model.Status ?? ""}\" voice={model.VoiceState.Kind} held={(store.HeldNow is null ? "no" : "yes")} receipt={Describe(store.Receipt)}");
        void OnChange(object? s, System.ComponentModel.PropertyChangedEventArgs e)
        {
            if (e.PropertyName is nameof(AskBarModel.FieldText) or nameof(AskBarModel.Text) or nameof(AskBarModel.Partial)) Trace($"step {step}");
        }
        model.PropertyChanged += OnChange;
        // PNG evidence (RenderTargetBitmap of the bar): a live partial (muted) after the third item, the landing after the finish.
        After(dispatcher, name == "shaped" ? 3.1 : 2.2, () => Shot(install, name == "shaped" ? "1-shaped-live" : "3-sendit-armed"));
        After(dispatcher, name == "shaped" ? 5.2 : 4.3, () => Shot(install, name == "shaped" ? "2-shaped-hold" : "4-sendit-sent"));
        model.ProbeFixture(items, id, TimeSpan.FromMilliseconds(900), () =>
        {
            model.PropertyChanged -= OnChange;
            Trace("done");
            After(dispatcher, 1.2, () => Trace("+1.2 s"));
            After(dispatcher, 4.0, () => Trace("+4 s"));
        });
        store.EventReceived += (type, _) => { if (type.StartsWith("Dictation", StringComparison.Ordinal)) step++; };
    }

    private static async void Shot(AskBarInstall install, string name)
    {
        var path = AskFlags.Png(name);
        bool ok = await install.Bar.CapturePng(path);
        AskLog.Write($"ask-fixture shot {name}: {(ok ? path : "failed")} field=\"{install.Model.FieldText}\" status=\"{install.Model.Status ?? ""}\"");
    }

    private static void Suggest(Store store, AskBarInstall install, DispatcherQueue dispatcher)
    {
        store.HandleEvent(System.Text.Encoding.UTF8.GetBytes(Suggestion));
        var card = install.Suggestions;
        After(dispatcher, 0.8, async () =>
        {
            var path = AskFlags.Png("5-suggestion");
            bool ok = card is not null && await card.CapturePng(path);
            AskLog.Write($"ask-fixture shot 5-suggestion: {(ok ? path : "failed")}");
        });
        AskLog.Write($"ask-fixture suggestion: card shown={(card?.IsShown == true ? "true" : "false")} current={card?.Showing?.Id ?? "none"} backdrop={card?.BackdropText ?? "-"} text=\"{(card?.Showing is { } c ? SuggestionCopy.Sentence(c) + " " + SuggestionCopy.Detail(c) : "")}\"");
    }

    private static string Describe(StoreModel.ReceiptRow? r) =>
        AskBarModel.ReceiptLines(r, 0) is { } l ? $"\"{l.Text}\" / \"{l.Trailing}\"" : "none";

    private static void After(DispatcherQueue dispatcher, double seconds, Action action)
    {
        var timer = dispatcher.CreateTimer();
        timer.Interval = TimeSpan.FromSeconds(seconds);
        timer.IsRepeating = false;
        timer.Tick += (_, _) => { timer.Stop(); action(); };
        timer.Start();
        Keep.Add(timer);
    }

    private static readonly List<DispatcherQueueTimer> Keep = new();
}