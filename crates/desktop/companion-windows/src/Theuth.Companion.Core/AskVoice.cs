using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>
/// Hold-to-dictate (D7, W6 live): the hotkey's key-down starts a press; past <see cref="HoldThreshold"/> the microphone
/// streams to the engine's dictation; key-up stops it and the machine waits for <c>DictationFinished</c>; the text stays in
/// the field and the same 3 s hold starts. A press shorter than the threshold is a tap (the hotkey's toggle). Pure — the
/// app performs the effects.
/// </summary>
public sealed class AskVoiceMachine
{
    public enum StateKind { Idle, Pressed, Recording, Stopping }

    /// <summary>idle · pressed(since) · recording(since) · stopping (<c>dictation_stop</c> sent; the closed utterance is still flushing).</summary>
    public readonly record struct State(StateKind Kind, DateTimeOffset? Since = null)
    {
        public static State Idle => new(StateKind.Idle);
        public static State Pressed(DateTimeOffset since) => new(StateKind.Pressed, since);
        public static State Recording(DateTimeOffset since) => new(StateKind.Recording, since);
        public static State Stopping => new(StateKind.Stopping);
    }

    public abstract record Event
    {
        private Event() { }

        public sealed record KeyDown(DateTimeOffset At) : Event;

        public sealed record KeyUp(DateTimeOffset At) : Event;

        public sealed record Tick(DateTimeOffset At) : Event;

        /// <summary><c>DictationFinished</c>: the whole dictated text.</summary>
        public sealed record Dictated(string Text) : Event;

        public sealed record Failed(string Why) : Event;
    }

    public enum EffectKind
    {
        StartRecording,
        StopDictation,

        /// <summary>A short press: the caller's toggle.</summary>
        Tap,

        /// <summary>Put <see cref="Effect.Text"/> in the field and start the hold.</summary>
        SetText,
        Fail,
    }

    public readonly record struct Effect(EffectKind Kind, string? Text = null)
    {
        public static Effect StartRecording => new(EffectKind.StartRecording);
        public static Effect StopDictation => new(EffectKind.StopDictation);
        public static Effect Tap => new(EffectKind.Tap);
        public static Effect SetText(string text) => new(EffectKind.SetText, text);
        public static Effect Fail(string why) => new(EffectKind.Fail, why);
    }

    /// <summary>Below this a press is a tap; at or above it the microphone opens (seconds).</summary>
    public const double HoldThreshold = 0.25;

    /// <summary>The longest dictation (seconds).</summary>
    public const double MaxDuration = 60;

    public State Current { get; private set; } = State.Idle;

    public bool IsRecording => Current.Kind == StateKind.Recording;
    public bool IsBusy => Current != State.Idle;

    public IReadOnlyList<Effect> Handle(Event e)
    {
        switch (Current.Kind, e)
        {
            case (StateKind.Idle, Event.KeyDown d):
                Current = State.Pressed(d.At);
                return Array.Empty<Effect>();
            case (StateKind.Pressed, Event.Tick t) when (t.At - Current.Since!.Value).TotalSeconds >= HoldThreshold:
                Current = State.Recording(t.At);
                return new[] { Effect.StartRecording };
            case (StateKind.Pressed, Event.KeyUp):
                Current = State.Idle;
                return new[] { Effect.Tap };
            case (StateKind.Recording, Event.KeyUp):
                Current = State.Stopping;
                return new[] { Effect.StopDictation };
            case (StateKind.Recording, Event.Tick t) when (t.At - Current.Since!.Value).TotalSeconds >= MaxDuration:
                Current = State.Stopping;
                return new[] { Effect.StopDictation };
            case (StateKind.Stopping, Event.Dictated d):
            {
                Current = State.Idle;
                var text = d.Text.Trim();
                return text.Length == 0 ? new[] { Effect.Fail("Nothing recognized") } : new[] { Effect.SetText(text) };
            }
            case (StateKind.Stopping or StateKind.Recording or StateKind.Pressed, Event.Failed f):
                Current = State.Idle;
                return new[] { Effect.Fail(f.Why) };
            default:
                return Array.Empty<Effect>();
        }
    }
}

/// <summary>
/// The engine's streamed dictation (design 54 §8) reduced for the Ask bar: <c>dictation_start</c> → <c>DictationStarted</c>
/// → <c>DictationText</c> per utterance (append-only; <c>final</c> once) → <c>dictation_stop</c> → <c>DictationFinished</c>.
/// <see cref="LiveText"/> = the closed utterances joined + the open partial. Pure.
/// </summary>
public sealed class AskDictation
{
    public enum PhaseKind { Idle, Starting, Live, Stopping }

    /// <summary>idle · starting (<c>dictation_start</c> in flight) · live(id) · stopping(id) (<c>dictation_stop</c> sent).</summary>
    public readonly record struct DictationPhase(PhaseKind Kind, string? Id = null)
    {
        public static DictationPhase Idle => new(PhaseKind.Idle);
        public static DictationPhase Starting => new(PhaseKind.Starting);
        public static DictationPhase Live(string id) => new(PhaseKind.Live, id);
        public static DictationPhase Stopping(string id) => new(PhaseKind.Stopping, id);
    }

    public sealed record Utterance(string Text, bool Final);

    public abstract record Event
    {
        private Event() { }

        public sealed record Start : Event
        {
            public static readonly Start Instance = new();
        }

        /// <summary>The <c>dictation_start</c> reply.</summary>
        public sealed record Started(string Id) : Event;

        public sealed record StartFailed(string Why) : Event;

        public sealed record Text(string Id, int Utterance, string Value, bool Final) : Event;

        public sealed record Level(string Id, ValueList<float> Levels) : Event;

        public sealed record Stop : Event
        {
            public static readonly Stop Instance = new();
        }

        /// <summary><c>DictationFinished</c>; <see cref="Error"/> is <c>Error { message }</c>'s message.</summary>
        public sealed record Finished(string Id, string? Error) : Event;

        /// <summary>The user edited the field after the stop: late partials must not overwrite it.</summary>
        public sealed record Typed : Event
        {
            public static readonly Typed Instance = new();
        }
    }

    public DictationPhase Phase { get; private set; } = DictationPhase.Idle;

    private readonly List<Utterance> utterances = new();

    public IReadOnlyList<Utterance> Utterances => utterances;

    /// <summary>The last <c>DictationLevel</c> packet, 0–1, oldest first.</summary>
    public IReadOnlyList<float> Levels { get; private set; } = Array.Empty<float>();

    /// <summary><c>already dictating</c> → "dictating elsewhere"; a model error; a finish with <c>Error</c>.</summary>
    public string? Error { get; private set; }

    private bool typedWins;

    public bool IsActive => Phase.Kind != PhaseKind.Idle;

    public string? Id => Phase.Kind is PhaseKind.Live or PhaseKind.Stopping ? Phase.Id : null;

    public string FinalText => string.Join(" ", utterances.Where(u => u.Final).Select(u => u.Text).Where(t => t.Length > 0));

    public string PartialText => utterances.Count > 0 && !utterances[^1].Final ? utterances[^1].Text : "";

    public string LiveText => string.Join(" ", new[] { FinalText, PartialText }.Where(t => t.Length > 0));

    /// <summary>true when the field should take <see cref="LiveText"/> (never after the user typed).</summary>
    public bool Handle(Event e)
    {
        switch (e)
        {
            case Event.Start:
                Phase = DictationPhase.Starting;
                utterances.Clear();
                Levels = Array.Empty<float>();
                Error = null;
                typedWins = false;
                return true;
            case Event.Started s:
                if (Phase != DictationPhase.Starting) return false;
                Phase = DictationPhase.Live(s.Id);
                return false;
            case Event.StartFailed f:
                Phase = DictationPhase.Idle;
                Error = f.Why.Contains("already dictating", StringComparison.Ordinal) ? "dictating elsewhere" : f.Why;
                return false;
            case Event.Text t:
                if (t.Id != Id) return false;
                while (utterances.Count <= t.Utterance) utterances.Add(new Utterance("", true));
                utterances[t.Utterance] = new Utterance(t.Value, t.Final);
                return !typedWins;
            case Event.Level l:
                if (l.Id != Id) return false;
                Levels = l.Levels.ToArray();
                return false;
            case Event.Stop:
                if (Phase.Kind == PhaseKind.Live) Phase = DictationPhase.Stopping(Phase.Id!);
                return false;
            case Event.Finished f:
                if (f.Id != Id) return false;
                Phase = DictationPhase.Idle;
                Levels = Array.Empty<float>();
                if (f.Error is not null) Error = f.Error;
                return !typedWins;
            case Event.Typed:
                typedWins = true;
                return false;
            default:
                return false;
        }
    }
}
