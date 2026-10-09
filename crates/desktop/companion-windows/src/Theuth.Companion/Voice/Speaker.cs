using Theuth.Companion.Core;
using Theuth.Companion.PointAndSay;

namespace Theuth.Companion.Voice;

/// <summary>
/// The ONE shared speaker (spoken replies, Sky 2026-10-09): the voice key's answer and an Ask theuth reply are read aloud through
/// the same <c>speak</c> → SpeechStarted/SpeechAudio(Opus)/SpeechFinished client-sink path and the same <see cref="SpeechPlayer"/>,
/// so there is one "currently speaking" utterance (<see cref="SpeakerModel"/>). <see cref="Speak"/> only when a reply is final
/// (callers gate with <see cref="SpeakerModel.ShouldSpeak"/>); <see cref="Stop"/> cuts playback and sends <c>stop_speaking</c>.
/// <see cref="Changed"/> drives the speaker glyphs (the rail's actions pill, the Ask bar's receipt row); clicking one calls
/// <see cref="Stop"/>. UI thread only (the Store raises its events there).
/// </summary>
internal sealed class Speaker : IDisposable
{
    private readonly Store store;
    private readonly SpeechPlayer player;
    private readonly SpeakerModel model = new();

    public Speaker(Store store)
    {
        this.store = store;
        player = new SpeechPlayer(() => model.Speaking?.Session) { Dump = Environment.GetCommandLineArgs().Contains("--speech-dump") };
        store.EventReceived += OnEvent;
    }

    /// <summary>Something started or stopped speaking (the glyphs re-render).</summary>
    public event Action? Changed;

    public bool IsSpeaking => model.IsSpeaking;

    public SpeakerModel.Current? Speaking => model.Speaking;

    /// <summary>The player, for the voice key's answer card bookkeeping (<c>Utterance</c>, device) and the probes.</summary>
    public SpeechPlayer Player => player;

    /// <summary>Read <paramref name="text"/> aloud for <paramref name="session"/>: whatever spoke before is cut first (one speaker).</summary>
    public void Speak(string session, string text, SpeechSource source)
    {
        var token = $"req-{Guid.NewGuid():N}";
        model.Begin(token, session, source);
        if (model.Displaced is { } d)
        {
            Log.Write($"speaker: {d.Source} utterance {d.Id} displaced by a new {source} reply");
            player.Stop();
        }
        Log.Write($"speaker: speak({session}, {text.Length} chars) for {source}");
        Changed?.Invoke();
        _ = store.Client.CallRawAsync("speak", session, text).ContinueWith(t =>
        {
            if (t.IsFaulted) store.Dispatcher.TryEnqueue(() =>
            {
                Log.Write($"speaker: speak failed: {t.Exception?.GetBaseException().Message}");
                if (model.Speaking?.Id == token) { model.Stop(SpeechStop.Finished); Changed?.Invoke(); }
            });
        }, TaskScheduler.Default);
    }

    /// <summary>Esc, a new ask, a new voice-key gesture, the glyph: cut playback now and tell the engine; a no-op when silent.</summary>
    public void Stop(SpeechStop why)
    {
        var was = model.Stop(why);
        player.Stop();
        if (was is null) return;
        Log.Write($"speaker: stop ({why}) — {was.Source} utterance {was.Id}");
        _ = store.Client.CallRawAsync("stop_speaking", (object?)null).ContinueWith(_ => { }, TaskScheduler.Default);
        Changed?.Invoke();
    }

    /// <summary>Raw-stream hook: speech events for the followed session go to the player; true when consumed.</summary>
    private void OnEvent(string type, byte[] json)
    {
        if (!type.StartsWith("Speech", StringComparison.Ordinal)) return;
        if (SpeechWire.Decode(type, json) is not { } w || model.Speaking is not { } cur || w.Session != cur.Session) return;
        switch (w.Type)
        {
            case "SpeechStarted":
                model.Named(w.Id ?? cur.Id);
                player.Handle(type, json);
                Changed?.Invoke();
                break;
            case "SpeechAudio":
                player.Handle(type, json);
                break;
            case "SpeechFinished":
                player.Handle(type, json);
                if (model.Finished(w.Id ?? "")) Changed?.Invoke();
                break;
        }
    }

    public void Dispose()
    {
        store.EventReceived -= OnEvent;
        player.Dispose();
    }
}
