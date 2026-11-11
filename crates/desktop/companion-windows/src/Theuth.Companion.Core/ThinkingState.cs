using System.Text;

namespace Theuth.Companion.Core;

/// <summary>What a view draws of the model's thinking: the text, whether the "Thinking" section is still live, and the seconds it took.</summary>
/// <param name="Text">All the reasoning so far, with every picture fence flattened (never a fence body).</param>
/// <param name="Active">The model is still thinking (no answer text yet): the section is open and follows the tail.</param>
/// <param name="Seconds">How long it thought (the engine's <c>ReasoningDone.ms</c> when it came, else the clock), at least 1.</param>
public sealed record ThinkingView(string Text, bool Active, int Seconds)
{
    /// <summary>"Thinking" while it is live, then the one-line expander "Thought for N s".</summary>
    public string Header => Active ? "Thinking" : $"Thought for {Seconds} s";

    /// <summary>The live section shows the last <see cref="TailChars"/> characters (about six lines and then some); the expander holds all of it.</summary>
    public const int TailChars = 3000;

    /// <summary>The end of <see cref="Text"/> a live section draws, cut at a word so no half word shows at the top.</summary>
    public string Tail
    {
        get
        {
            if (Text.Length <= TailChars) return Text;
            var cut = Text[^TailChars..];
            int blank = cut.IndexOfAny(new[] { ' ', '\n' });
            return "…" + (blank is >= 0 and < 80 ? cut[(blank + 1)..] : cut);
        }
    }
}

/// <summary>
/// 192w P3: the thinking of one ask / one card exchange. <c>ReasoningDelta</c> text collects while the model has said nothing else;
/// the first <c>TextDelta</c> (or the finish) collapses it to "Thought for N s". No <c>ReasoningDelta</c> means no thinking section —
/// today's spinner. The text is flattened for picture fences as it is read (a model that quotes a diagram in its reasoning must not
/// put the fence body on screen, design 164's rule). Pure: the clock comes in as milliseconds.
/// </summary>
public sealed class ThinkingState
{
    /// <summary>The reasoning kept: past this the oldest text goes (a section of 200 kB helps nobody).</summary>
    public const int MaxChars = 200_000;

    private readonly StringBuilder text = new();
    private long firstMs;
    private long endMs;
    private long doneMs;

    /// <summary>At least one <c>ReasoningDelta</c> arrived.</summary>
    public bool Started { get; private set; }

    /// <summary>The first answer text, a finish or a stop folded the section to its expander.</summary>
    public bool Collapsed { get; private set; }

    /// <summary>The section is live: reasoning arrived and nothing else yet.</summary>
    public bool Active => Started && !Collapsed;

    /// <summary>Everything the model reasoned, picture fences flattened.</summary>
    public string Text => PictureFence.Flatten(text.ToString());

    /// <summary>Add a streamed piece. True for the very first one (the "first ReasoningDelta" log line).</summary>
    public bool Delta(string piece, long nowMs)
    {
        if (piece.Length == 0) return false;
        bool first = !Started;
        if (first)
        {
            Started = true;
            firstMs = nowMs;
        }
        text.Append(piece);
        if (text.Length > MaxChars) text.Remove(0, text.Length - MaxChars);
        return first;
    }

    /// <summary><c>ReasoningDone.ms</c> (one per model call of the turn): the engine's own measure, summed.</summary>
    public void Done(long ms)
    {
        if (Started && ms > 0) doneMs += ms;
    }

    /// <summary>Fold the section: the first answer text, the finish, a stop. True when this call did it.</summary>
    public bool Collapse(long nowMs)
    {
        if (!Started || Collapsed) return false;
        Collapsed = true;
        endMs = nowMs;
        return true;
    }

    /// <summary>Whole seconds spent thinking, at least 1: the engine's measure when it sent one, else first delta to the fold (or to <paramref name="nowMs"/> while live).</summary>
    public int Seconds(long nowMs)
    {
        long ms = doneMs > 0 ? doneMs : Math.Max(0, (Collapsed ? endMs : nowMs) - firstMs);
        return (int)Math.Max(1, Math.Round(ms / 1000.0, MidpointRounding.AwayFromZero));
    }

    public ThinkingView View(long nowMs) => new(Text, Active, Seconds(nowMs));
}
