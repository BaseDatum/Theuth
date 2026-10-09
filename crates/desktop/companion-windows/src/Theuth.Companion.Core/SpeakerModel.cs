namespace Theuth.Companion.Core;

/// <summary>Who asked for the utterance: the voice key's answer card or an Ask bar reply.</summary>
public enum SpeechSource { VoiceKey, Ask }

/// <summary>Why the current speech must stop (Sky 2026-10-09: "Esc, a new ask, or a new voice-key gesture stops the current speech").</summary>
public enum SpeechStop { Esc, NewAsk, NewGesture, Glyph, Finished }

/// <summary>
/// ONE shared speaker for the companion (spoken replies, 2026-10-09): one "currently speaking" utterance across the voice key
/// and Ask theuth. Pure state for the app's <c>Speaker</c> (SpeechPlayer + <c>speak</c>/<c>stop_speaking</c>) and the glyph.
/// Rules: a new utterance replaces the current one (whatever its source); Esc, a new ask, a new voice-key gesture or the
/// speaker glyph stop it; a stop with nothing speaking is a no-op; speech is requested only once a reply is final — the
/// gate <see cref="ShouldSpeak"/> says whether a finished reply is spoken at all (the per-source opt-in, non-empty text).
/// </summary>
public sealed class SpeakerModel
{
    /// <summary>The utterance speaking now: its id (the engine's speak id, or a request token until SpeechStarted names it), session and source.</summary>
    public sealed record Current(string Id, string Session, SpeechSource Source);

    public Current? Speaking { get; private set; }

    /// <summary>The utterance a <see cref="Begin"/> displaced, for the caller to cut (one speaker: never two at once).</summary>
    public Current? Displaced { get; private set; }

    public bool IsSpeaking => Speaking is not null;

    /// <summary>A reply is final: speak it? Only when its source's switch is on, the text has words, and nothing replays.</summary>
    public static bool ShouldSpeak(SpeechSource source, bool voiceKeySpeak, bool askSpeak, string text, bool replaying = false)
    {
        if (replaying || text.Trim().Length == 0) return false;
        return source == SpeechSource.VoiceKey ? voiceKeySpeak : askSpeak;
    }

    /// <summary>A new utterance starts: whatever spoke before is displaced (returned in <see cref="Displaced"/>).</summary>
    public Current Begin(string id, string session, SpeechSource source)
    {
        Displaced = Speaking;
        Speaking = new Current(id, session, source);
        return Speaking;
    }

    /// <summary>The engine named the utterance (SpeechStarted) for the session we follow: adopt its id.</summary>
    public void Named(string engineId)
    {
        if (Speaking is { } s) Speaking = s with { Id = engineId };
    }

    /// <summary>Stop for <paramref name="why"/>: the utterance that was speaking (null when nothing was — a no-op stop).</summary>
    public Current? Stop(SpeechStop why)
    {
        var was = Speaking;
        Speaking = null;
        Displaced = null;
        return was;
    }

    /// <summary>SpeechFinished for <paramref name="engineId"/>: the speaker falls silent only if that is the current utterance.</summary>
    public bool Finished(string engineId)
    {
        if (Speaking is not { } s || s.Id != engineId) return false;
        Speaking = null;
        return true;
    }
}

/// <summary>The surface a global key opens (ruling 2, Sky 2026-10-09 16:02).</summary>
public enum KeySurface { Panel, AskBar, Agents, None }

/// <summary>
/// Routing of the global keys to surfaces. "Open the panel" ALWAYS opens the Attention panel (its empty state included; a hold
/// still talks), "Open the Ask bar" opens only the Ask bar. The 130-W3 fallback — the panel when something needs you, else the
/// Ask bar — is retired: the two keys opened the same thing and the panel has an empty state now.
/// </summary>
public static class KeyRouting
{
    public static KeySurface Route(KeyAction action, int needsYou, bool askBarAvailable) => action switch
    {
        KeyAction.OpenPanel => KeySurface.Panel,
        KeyAction.OpenAskBar => askBarAvailable ? KeySurface.AskBar : KeySurface.None,
        KeyAction.OpenAgents => KeySurface.Agents,
        _ => KeySurface.None,
    };
}
