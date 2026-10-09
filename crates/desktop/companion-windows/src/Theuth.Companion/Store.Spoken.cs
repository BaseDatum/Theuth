using System.ComponentModel;
using Theuth.Companion.Core;
using Theuth.Companion.Voice;

namespace Theuth.Companion;

/// <summary>
/// Spoken replies (Sky 2026-10-09 15:58): the app side of the ONE shared speaker. <see cref="Speaker"/> is created once per Store
/// (the voice key's controller, the Ask bar, the rail glyph and the Settings pages all use it); <see cref="InstallSpokenReplies"/>
/// speaks an Ask theuth reply once it is final when Settings › Ask bar › "Speak the reply aloud" is on, and a new ask cuts
/// whatever is being read. Both switches are opt-in and off by default (<see cref="PointSettings.Speak"/>, <see cref="CompanionSettings.AskSpeak"/>).
/// </summary>
public sealed partial class Store
{
    private Speaker? speaker;
    private bool spokenInstalled;

    internal Speaker Speaker => speaker ??= new Speaker(this);

    internal void InstallSpokenReplies(Func<bool> askSpeak)
    {
        if (spokenInstalled) return;
        spokenInstalled = true;
        AskReplyFinished += (session, text) =>
        {
            bool on = askSpeak();
            bool speakIt = SpeakerModel.ShouldSpeak(SpeechSource.Ask, voiceKeySpeak: false, askSpeak: on, text);
            Log.Write($"ask: reply final on {session} ({text.Length} chars) — {(speakIt ? "speaking" : on ? "nothing to say" : "not spoken (Ask bar › Speak the reply aloud is off)")}");
            if (speakIt) Speaker.Speak(session, text, SpeechSource.Ask);
        };
        PropertyChanged += OnSpokenStoreChanged;
        Log.Write($"spoken replies: installed (Ask bar speak={(askSpeak() ? "on" : "off")}; one shared speaker)");
    }

    private void OnSpokenStoreChanged(object? sender, PropertyChangedEventArgs e)
    {
        // A new ask (the hold starts) cuts the current speech, whichever surface asked for it.
        if (e.PropertyName == nameof(Receipt) && Receipt is ReceiptRow.Holding { Held.Action: CardAction.AskTheuth } && speaker is { IsSpeaking: true })
        {
            Speaker.Stop(SpeechStop.NewAsk);
        }
    }
}
