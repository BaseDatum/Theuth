using System.Text;

namespace Theuth.Companion.Core;

/// <summary>153 §12.4 (D13): every state the one AnswerCard can be in, each with its copy.</summary>
public enum CardPhase
{
    Listening,
    Finishing,
    Thinking,
    Answer,
    Speaking,
    Failed,
}

/// <summary>Who opened the card — the marker family of its follow-ups (§12.2): none (Ask bar), <c>[voice note, N s]</c>, <c>[command, N s]</c>.</summary>
public enum CardSource
{
    Ask,
    VoiceKey,
    Command,
}

/// <summary>A tool chip in the order it started; a running one pulses (§12.2).</summary>
public sealed record ToolChip(string CallId, string Name, bool Running, bool Failed = false);

/// <summary>One request of the card's thread and its streamed answer (the header dots are the thread positions).</summary>
public sealed record CardExchange(string Request, string Answer, IReadOnlyList<ToolChip> Tools, string? Error = null, bool Stopped = false, bool Finished = false)
{
    public static CardExchange Empty { get; } = new("", "", Array.Empty<ToolChip>());
}

/// <summary>
/// 153 §12.1–§12.4 (D12/D13): the state of the ONE AnswerCard the Ask bar, the voice key and command mode share — the phase with its
/// copy, collapsed/expanded, the speaking flag, the thread of requests (J/K) and the generation that makes every hide honest:
/// <see cref="Hide"/> ignores a hide whose generation is older than the latest <see cref="Begin"/> (a new gesture while a 6 s timer
/// runs), the rule 149f applied to <c>dictation_start</c> replies. <see cref="DoneArmed"/> is the only auto-hide: after the turn
/// finished AND speech ended AND the card is collapsed AND it did not fail. Pure; the window renders it and owns the timers.
/// </summary>
public sealed class AnswerCardState
{
    /// <summary>The answer lingers this long once <see cref="DoneArmed"/> (§3.3 <c>doneMs</c> = <see cref="PointMachine.DoneMs"/>).</summary>
    public const ulong DoneMs = PointMachine.DoneMs;

    /// <summary>finishing waits at most this long for the final text (<c>PointFlow.FinalTextMs</c>, §12.4).</summary>
    public const ulong FinishingCapMs = PointFlow.FinalTextMs;

    /// <summary>The session list shows the last answers of this many hours (§12.2, <c>activity_list</c> 24 h).</summary>
    public const int ListHours = 24;

    private readonly List<CardExchange> thread = new();

    public int Generation { get; private set; }

    public CardPhase Phase { get; private set; } = CardPhase.Listening;

    /// <summary>On screen (a hide or a dismiss clears it; <see cref="Begin"/> sets it).</summary>
    public bool Visible { get; private set; }

    public bool Expanded { get; private set; }

    /// <summary>Our answer is being read aloud (<c>Speaker.surface == ours</c>, 149s).</summary>
    public bool Speaking { get; private set; }

    public CardSource Source { get; private set; } = CardSource.VoiceKey;

    /// <summary>The desk session the card follows (O opens it; tool chips link into it).</summary>
    public string? Session { get; set; }

    /// <summary>Clock (ms) of the phase change; thinking's elapsed seconds tick from <see cref="ThinkingSinceMs"/>.</summary>
    public ulong PhaseSinceMs { get; private set; }

    public ulong ThinkingSinceMs { get; private set; }

    /// <summary>"Nothing recognized" and friends: the line a finishing card shows when there is nothing to send.</summary>
    public string? Hint { get; private set; }

    /// <summary>The thread position J/K select (0-based); the newest request after every <see cref="Begin"/>.</summary>
    public int Index { get; private set; }

    public IReadOnlyList<CardExchange> Thread => thread;

    public CardExchange Current => thread.Count == 0 ? CardExchange.Empty : thread[Math.Clamp(Index, 0, thread.Count - 1)];

    public CardExchange Latest => thread.Count == 0 ? CardExchange.Empty : thread[^1];

    /// <summary>The latest turn has finished (TurnFinished, an error, or a stop).</summary>
    public bool Finished => Latest.Finished;

    /// <summary>
    /// The 6 s timer may run: finished ∧ not speaking ∧ collapsed ∧ showing an answer (never failed, never expanded, never while the
    /// text is still coming) — §12.4 "nothing vanishes before the work is done".
    /// </summary>
    public bool DoneArmed => Visible && Phase == CardPhase.Answer && Finished && !Speaking && !Expanded;

    /// <summary>A new gesture (or the Ask bar's send) opens the card: listening, a new generation, a new thread position.</summary>
    public int Begin(CardSource source, ulong nowMs, bool followUp = false)
    {
        Generation++;
        if (!followUp || !Visible || source != Source) thread.Clear();
        Source = source;
        thread.Add(CardExchange.Empty);
        Index = thread.Count - 1;
        Visible = true;
        Speaking = false;
        Hint = null;
        if (!followUp) Expanded = false;
        Enter(CardPhase.Listening, nowMs);
        return Generation;
    }

    /// <summary>The key came up: words muted, overlays concealed, the screenshot taken.</summary>
    public void Released(ulong nowMs)
    {
        if (Phase == CardPhase.Listening) Enter(CardPhase.Finishing, nowMs);
    }

    /// <summary>The final words (or the live text after <see cref="FinishingCapMs"/>): "" → the hint "Nothing recognized".</summary>
    public void FinalText(string text, ulong nowMs)
    {
        if (text.Trim().Length == 0)
        {
            Hint = "Nothing recognized";
            return;
        }
        SetLatest(Latest with { Request = text.Trim() });
        if (Phase is CardPhase.Listening or CardPhase.Finishing) Enter(CardPhase.Finishing, nowMs);
    }

    /// <summary>
    /// The request reached the desk (<c>send_input</c> returned) or the desk's <c>TurnStarted</c>: thinking. (Writer's call: a busy desk
    /// that the engine steers emits no new <c>TurnStarted</c>, so a successful send enters thinking too.)
    /// </summary>
    public void TurnStarted(ulong nowMs)
    {
        if (Phase is CardPhase.Listening or CardPhase.Finishing or CardPhase.Thinking)
        {
            if (Phase != CardPhase.Thinking) ThinkingSinceMs = nowMs;
            Enter(CardPhase.Thinking, nowMs);
        }
    }

    public void ToolStarted(string callId, string name)
    {
        var tools = Latest.Tools.ToList();
        tools.Add(new ToolChip(callId, name, true));
        SetLatest(Latest with { Tools = tools });
    }

    public void ToolFinished(string callId, bool failed)
    {
        SetLatest(Latest with { Tools = Latest.Tools.Select(t => t.CallId == callId ? t with { Running = false, Failed = failed } : t).ToList() });
    }

    /// <summary>A streamed piece of the answer: the first one turns thinking into answer (the caret shimmers until finished).</summary>
    public void TextDelta(string text, ulong nowMs)
    {
        if (Latest.Stopped || Phase == CardPhase.Failed) return;
        SetLatest(Latest with { Answer = Latest.Answer + text });
        if (Phase is CardPhase.Thinking or CardPhase.Finishing or CardPhase.Listening) Enter(CardPhase.Answer, nowMs);
    }

    /// <summary><c>TurnFinished</c>: answer (or failed for <c>error</c>/<c>cancelled</c>, unless the user's Esc stopped it — that stays the frozen answer).</summary>
    public void TurnFinished(string? stopReason, ulong nowMs)
    {
        var latest = Latest with { Finished = true };
        if (latest.Stopped)
        {
            SetLatest(latest);
            return;
        }
        if (stopReason is "error" or "cancelled")
        {
            SetLatest(latest with { Error = latest.Error ?? stopReason });
            Enter(CardPhase.Failed, nowMs);
            return;
        }
        SetLatest(latest);
        if (Phase != CardPhase.Failed && !Speaking) Enter(CardPhase.Answer, nowMs);
    }

    /// <summary>The desk's <c>Error</c> event or a failed send: failed, which never auto-hides (⎋ only).</summary>
    public void Failed(string why, ulong nowMs)
    {
        SetLatest(Latest with { Error = why, Finished = true });
        Speaking = false;
        Enter(CardPhase.Failed, nowMs);
    }

    /// <summary>
    /// Esc during thinking (§12.4 "stop freezes"): the driver sends <c>cancel(desk, "root")</c>; the card shows the frozen partial answer
    /// with "stopped" and does not disappear. False when there is nothing running to stop.
    /// </summary>
    public bool Stop(ulong nowMs)
    {
        if (Phase is not (CardPhase.Thinking or CardPhase.Answer) || Finished) return false;
        SetLatest(Latest with { Stopped = true, Finished = true, Tools = Latest.Tools.Select(t => t with { Running = false }).ToList() });
        Enter(CardPhase.Answer, nowMs);
        return true;
    }

    public void SpeechStarted(ulong nowMs)
    {
        if (!Visible || Phase == CardPhase.Failed) return;
        Speaking = true;
        Enter(CardPhase.Speaking, nowMs);
    }

    /// <summary>Speech ended (or Esc stopped it): back to answer, where the 6 s rule applies again.</summary>
    public void SpeechEnded(ulong nowMs)
    {
        if (!Speaking) return;
        Speaking = false;
        if (Phase == CardPhase.Speaking) Enter(CardPhase.Answer, nowMs);
    }

    public void SetExpanded(bool expanded) => Expanded = expanded && Visible;

    public void ToggleExpanded() => SetExpanded(!Expanded);

    /// <summary>J/K (← →): move along the thread; false at an end.</summary>
    public bool Move(int delta)
    {
        int next = Index + delta;
        if (next < 0 || next >= thread.Count) return false;
        Index = next;
        return true;
    }

    /// <summary>
    /// A hide for <paramref name="generation"/> (the 6 s timer, a dismiss, a probe): ignored when a newer <see cref="Begin"/> happened;
    /// a timer hide is also refused while the card is not <see cref="DoneArmed"/> (speaking, expanded, failed, still working).
    /// </summary>
    public bool Hide(int generation, bool byTimer = false)
    {
        if (generation != Generation) return false;
        if (byTimer && !DoneArmed) return false;
        Visible = false;
        Expanded = false;
        Speaking = false;
        return true;
    }

    /// <summary>⎋ while settled and collapsed, or while failed: the card goes away (the current generation's own dismiss).</summary>
    public bool Dismiss() => Hide(Generation);

    /// <summary>
    /// The copy of the phase (§12.4, Windows key words). <paramref name="keyText"/> is the chord ("Ctrl+Alt+Space"); thinking says the
    /// running tool and the elapsed seconds, so it changes every second and never looks frozen.
    /// </summary>
    public string Copy(string keyText, ulong nowMs) => Phase switch
    {
        CardPhase.Listening => $"· {keyText} held · theuth's desk",
        CardPhase.Finishing => Hint ?? "finishing…",
        CardPhase.Thinking => ThinkingCopy(nowMs),
        CardPhase.Answer => Latest.Stopped ? "stopped" : Finished ? "replied" : "replying…",
        CardPhase.Speaking => "click to stop",
        CardPhase.Failed => FailedCopy(Latest.Error),
        _ => "",
    };

    /// <summary>"thinking · code_refs · 4 s".</summary>
    public string ThinkingCopy(ulong nowMs)
    {
        ulong secs = nowMs > ThinkingSinceMs ? (nowMs - ThinkingSinceMs) / 1000 : 0;
        var running = Latest.Tools.LastOrDefault(t => t.Running)?.Name;
        return running is null ? $"thinking · {secs} s" : $"thinking · {running} · {secs} s";
    }

    /// <summary>"couldn't reply — ‹why›" (the first line, clipped as the receipt does).</summary>
    public static string FailedCopy(string? why)
    {
        var first = (why ?? "").Split('\n', StringSplitOptions.RemoveEmptyEntries).Select(l => l.Trim()).FirstOrDefault(l => l.Length > 0) ?? "unknown error";
        return $"couldn't reply — {(first.Length > AskReply.FailedClip ? first[..(AskReply.FailedClip - 1)].TrimEnd() + "…" : first)}";
    }

    /// <summary>The collapsed body: the receipt's two lines (<see cref="AskReply.FirstTwoLines"/> reused, §12.2).</summary>
    public string CollapsedText
    {
        get
        {
            var x = Current;
            var reply = new AskReply(Session ?? "") { Text = x.Answer, Finished = x.Finished, Error = Phase == CardPhase.Failed ? x.Error : null };
            return reply.FirstTwoLines;
        }
    }

    /// <summary>"theuth desk · replied just now" / "theuth desk · 2 of 3" (the header; dots are drawn from <see cref="Thread"/>).</summary>
    public string Header(string title = "theuth desk") =>
        thread.Count > 1 ? $"{title} · {Index + 1} of {thread.Count}" : Phase == CardPhase.Answer && Finished && !Latest.Stopped ? $"{title} · replied just now" : title;

    private void Enter(CardPhase phase, ulong nowMs)
    {
        Phase = phase;
        PhaseSinceMs = nowMs;
    }

    private void SetLatest(CardExchange x)
    {
        if (thread.Count == 0) thread.Add(x);
        else thread[^1] = x;
    }
}

/// <summary>A markdown block for the card's expanded body (the RichTextBlock renderer, §12.2's fallback to MarkdownTextBlock).</summary>
public abstract record MdBlock
{
    private MdBlock() { }

    public sealed record Heading(int Level, IReadOnlyList<MdInline> Inlines) : MdBlock;

    public sealed record Paragraph(IReadOnlyList<MdInline> Inlines) : MdBlock;

    public sealed record Bullet(IReadOnlyList<MdInline> Inlines, bool Numbered, int Number) : MdBlock;

    public sealed record Code(string Language, string Text) : MdBlock;
}

/// <summary>Inline marks: text, **bold**, *italic*, `code`, [text](url).</summary>
public abstract record MdInline
{
    private MdInline() { }

    public sealed record Text(string Value) : MdInline;

    public sealed record Bold(string Value) : MdInline;

    public sealed record Italic(string Value) : MdInline;

    public sealed record Code(string Value) : MdInline;

    public sealed record Link(string Value, string Url) : MdInline;
}

/// <summary>
/// The tiny block renderer §12.2 asks for (lists, headings, fenced code, paragraphs) plus inline marks — enough for a desk answer.
/// An unclosed fence (a streaming answer) renders as code so far; anything unknown is text. Pure.
/// </summary>
public static class AnswerMarkdown
{
    public static IReadOnlyList<MdBlock> Parse(string? markdown)
    {
        var blocks = new List<MdBlock>();
        var para = new List<string>();
        var lines = (markdown ?? "").Replace("\r\n", "\n").Split('\n');
        void Flush()
        {
            if (para.Count == 0) return;
            blocks.Add(new MdBlock.Paragraph(Inline(string.Join(" ", para.Select(p => p.Trim())))));
            para.Clear();
        }
        for (int i = 0; i < lines.Length; i++)
        {
            var line = lines[i];
            var t = line.TrimStart();
            if (t.StartsWith("```", StringComparison.Ordinal))
            {
                Flush();
                var lang = t[3..].Trim();
                var code = new StringBuilder();
                i++;
                for (; i < lines.Length && !lines[i].TrimStart().StartsWith("```", StringComparison.Ordinal); i++)
                {
                    if (code.Length > 0) code.Append('\n');
                    code.Append(lines[i]);
                }
                blocks.Add(new MdBlock.Code(lang, code.ToString()));
                continue;
            }
            if (t.Length == 0) { Flush(); continue; }
            int hashes = 0;
            while (hashes < t.Length && hashes < 6 && t[hashes] == '#') hashes++;
            if (hashes > 0 && hashes < t.Length && t[hashes] == ' ')
            {
                Flush();
                blocks.Add(new MdBlock.Heading(hashes, Inline(t[(hashes + 1)..].Trim())));
                continue;
            }
            if ((t.StartsWith("- ", StringComparison.Ordinal) || t.StartsWith("* ", StringComparison.Ordinal) || t.StartsWith("• ", StringComparison.Ordinal)))
            {
                Flush();
                blocks.Add(new MdBlock.Bullet(Inline(t[2..].Trim()), false, 0));
                continue;
            }
            int dot = 0;
            while (dot < t.Length && char.IsAsciiDigit(t[dot])) dot++;
            if (dot > 0 && dot < t.Length - 1 && t[dot] is '.' or ')' && t[dot + 1] == ' ')
            {
                Flush();
                blocks.Add(new MdBlock.Bullet(Inline(t[(dot + 2)..].Trim()), true, int.Parse(t[..dot], System.Globalization.CultureInfo.InvariantCulture)));
                continue;
            }
            para.Add(line);
        }
        Flush();
        return blocks;
    }

    public static IReadOnlyList<MdInline> Inline(string s)
    {
        var outList = new List<MdInline>();
        var text = new StringBuilder();
        void Push()
        {
            if (text.Length > 0) outList.Add(new MdInline.Text(text.ToString()));
            text.Clear();
        }
        int i = 0;
        while (i < s.Length)
        {
            char c = s[i];
            if (c == '`')
            {
                int end = s.IndexOf('`', i + 1);
                if (end > i) { Push(); outList.Add(new MdInline.Code(s[(i + 1)..end])); i = end + 1; continue; }
            }
            if (c == '*' && i + 1 < s.Length && s[i + 1] == '*')
            {
                int end = s.IndexOf("**", i + 2, StringComparison.Ordinal);
                if (end > i + 2) { Push(); outList.Add(new MdInline.Bold(s[(i + 2)..end])); i = end + 2; continue; }
            }
            if (c is '*' or '_' && i + 1 < s.Length && s[i + 1] != ' ')
            {
                int end = s.IndexOf(c, i + 1);
                if (end > i + 1 && s[end - 1] != ' ') { Push(); outList.Add(new MdInline.Italic(s[(i + 1)..end])); i = end + 1; continue; }
            }
            if (c == '[')
            {
                int close = s.IndexOf("](", i + 1, StringComparison.Ordinal);
                int end = close > i ? s.IndexOf(')', close + 2) : -1;
                if (close > i && end > close) { Push(); outList.Add(new MdInline.Link(s[(i + 1)..close], s[(close + 2)..end])); i = end + 1; continue; }
            }
            text.Append(c);
            i++;
        }
        Push();
        return outList;
    }
}
