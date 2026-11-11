using System.Text;

namespace Theuth.Companion.Core;

/// <summary>
/// 153 §12.4 (D13): every state the one AnswerCard can be in, each with its copy. Speaking is a parallel flag
/// (<see cref="AnswerCardState.Speaking"/>) over Answer, as the Mac twin built it (H3-C1): the copy is "click to stop" while it is set.
/// </summary>
public enum CardPhase
{
    Listening,
    Finishing,
    Thinking,
    Answer,
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

    /// <summary>Our answer is being read aloud (<c>Speaker.surface == ours</c>, 149s) — §12.4's "speaking" row, a flag over Answer.</summary>
    public bool Speaking { get; private set; }

    /// <summary>The command card's approval box is up (H3-C1: doneMs never arms while an approval is open).</summary>
    public bool ApprovalOpen { get; set; }

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
    public bool DoneArmed => Visible && Phase == CardPhase.Answer && Finished && !Speaking && !Expanded && !ApprovalOpen;

    /// <summary>§10.1: a mode switch mid-gesture relabels the card (ask ↔ command) without a new generation.</summary>
    public void Relabel(CardSource source) => Source = source;

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
        if (Phase != CardPhase.Failed) Enter(CardPhase.Answer, nowMs);
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
        if (Phase is CardPhase.Thinking or CardPhase.Finishing) Enter(CardPhase.Answer, nowMs);
    }

    /// <summary>Speech ended (or Esc stopped it): the answer stays, where the 6 s rule applies again.</summary>
    public void SpeechEnded(ulong nowMs)
    {
        if (!Speaking) return;
        Speaking = false;
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
        CardPhase.Answer => Speaking ? "click to stop" : Latest.Stopped ? "stopped" : Finished ? "replied" : "replying…",
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
            return AnswerMarkdown.Plain(reply.FirstTwoLines);
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

/// <summary>
/// 153 §12.2 (R1, the Mac's ⌘L): the card's history list — the last <see cref="Max"/> answers from the Ask receipts (24 h), null while
/// closed. Ctrl+L toggles card ↔ list, Esc closes it first, J/K (↑/↓) move the selection, Enter or a click picks a row (the card then
/// shows that answer). Pure; the window renders it.
/// </summary>
public sealed class AnswerCardList
{
    public const int Max = 20;

    /// <summary>The empty list's line (the Mac's "nothing in the last 24 h").</summary>
    public const string EmptyLine = "nothing in the last 24 h";

    public const string Title = "Last answers";

    /// <summary>The rows, newest first; null = the list is closed.</summary>
    public IReadOnlyList<Receipt>? Rows { get; private set; }

    public bool Open => Rows is not null;

    public int Selected { get; private set; }

    /// <summary>Ctrl+L: open with the newest <see cref="Max"/> receipts, or close; true when the list is now open.</summary>
    public bool Toggle(IEnumerable<Receipt> source)
    {
        if (Open)
        {
            Close();
            return false;
        }
        Rows = source.OrderByDescending(r => r.At).Take(Max).ToList();
        Selected = 0;
        return true;
    }

    public void Close()
    {
        Rows = null;
        Selected = 0;
    }

    /// <summary>Esc: closes an open list (true = consumed); a closed list leaves Esc to the card.</summary>
    public bool Escape()
    {
        if (!Open) return false;
        Close();
        return true;
    }

    /// <summary>J/K (↑/↓): move the selection within the rows; false at an end or with no rows.</summary>
    public bool Move(int delta)
    {
        if (Rows is not { Count: > 0 } rows) return false;
        int next = Math.Clamp(Selected + delta, 0, rows.Count - 1);
        if (next == Selected) return false;
        Selected = next;
        return true;
    }

    /// <summary>Enter or a click: the picked receipt (the list closes), or null for no row.</summary>
    public Receipt? Pick(int? index = null)
    {
        if (Rows is not { } rows) return null;
        int i = index ?? Selected;
        if (i < 0 || i >= rows.Count) return null;
        var row = rows[i];
        Close();
        return row;
    }
}

/// <summary>A markdown block for the card's expanded body (the RichTextBlock renderer, §12.2's fallback to MarkdownTextBlock).</summary>
public abstract record MdBlock
{
    private MdBlock() { }

    public sealed record Heading(int Level, IReadOnlyList<MdInline> Inlines) : MdBlock;

    public sealed record Paragraph(IReadOnlyList<MdInline> Inlines) : MdBlock;

    /// <summary><paramref name="Indent"/>: the nesting level (0–4) of a bullet written with leading blanks (189w).</summary>
    public sealed record Bullet(IReadOnlyList<MdInline> Inlines, bool Numbered, int Number, int Indent = 0) : MdBlock;

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
/// The minimal incremental markdown parser (189w D4, §12.2's "tiny block renderer"): headings, bullets (nested by their indent),
/// numbered items, fenced code, table rows (kept as a monospace block) and paragraphs, plus the inline marks **bold**, *italic*, `code`
/// and [links](url) — enough for a desk answer. Pure and total: anything unknown is text. <c>streaming: true</c> is for text that is
/// still arriving: the LAST block's unclosed marks are shown as already closed (<c>**bo</c> renders bold so far, an unclosed code span
/// as code, a link whose <c>)</c> has not come as its label alone) and a last line that is only the start of a block marker
/// (<c>##</c>, <c>-</c>, <c>1.</c>, one or two backticks) is held back until it has a word — so the view never flashes stray marks that
/// the next chunk turns into formatting. An unclosed fence renders as code so far.
/// </summary>
public static class AnswerMarkdown
{
    private enum RawKind
    {
        Heading,
        Paragraph,
        Bullet,
        Code,
    }

    private sealed record Raw(RawKind Kind, string Src, int Level = 0, bool Numbered = false, int Number = 0, int Indent = 0, string Language = "");

    public static IReadOnlyList<MdBlock> Parse(string? markdown) => Parse(markdown, streaming: false);

    public static IReadOnlyList<MdBlock> Parse(string? markdown, bool streaming)
    {
        var raws = new List<Raw>();
        var para = new List<string>();
        var lines = (markdown ?? "").Replace("\r\n", "\n").Split('\n');
        int lastLine = lines.Length - 1;
        void Flush()
        {
            if (para.Count == 0) return;
            raws.Add(new Raw(RawKind.Paragraph, string.Join(" ", para.Select(p => p.Trim()))));
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
                    if (streaming && i == lastLine && IsFenceStub(lines[i])) break; // a closing fence still being typed
                    if (code.Length > 0) code.Append('\n');
                    code.Append(lines[i]);
                }
                raws.Add(new Raw(RawKind.Code, code.ToString(), Language: lang));
                continue;
            }
            if (t.Length == 0) { Flush(); continue; }
            if (streaming && i == lastLine && IsMarkerStub(t)) continue; // the start of a marker: nothing to show yet
            int hashes = 0;
            while (hashes < t.Length && hashes < 6 && t[hashes] == '#') hashes++;
            if (hashes > 0 && hashes < t.Length && t[hashes] == ' ')
            {
                Flush();
                raws.Add(new Raw(RawKind.Heading, t[(hashes + 1)..].Trim(), Level: hashes));
                continue;
            }
            if (t[0] == '|')
            {
                // A table: its rows stay as written, in a monospace block (no cell layout in a "minimal" renderer).
                Flush();
                var rows = new StringBuilder(line.TrimEnd());
                while (i + 1 < lines.Length && lines[i + 1].TrimStart().StartsWith('|'))
                {
                    i++;
                    rows.Append('\n').Append(lines[i].TrimEnd());
                }
                raws.Add(new Raw(RawKind.Code, rows.ToString(), Language: "table"));
                continue;
            }
            if (t.StartsWith("- ", StringComparison.Ordinal) || t.StartsWith("* ", StringComparison.Ordinal) || t.StartsWith("• ", StringComparison.Ordinal))
            {
                Flush();
                raws.Add(new Raw(RawKind.Bullet, t[2..].Trim(), Indent: IndentOf(line)));
                continue;
            }
            int dot = 0;
            while (dot < t.Length && char.IsAsciiDigit(t[dot])) dot++;
            if (dot > 0 && dot < t.Length - 1 && (t[dot] is '.' or ')') && t[dot + 1] == ' ')
            {
                Flush();
                raws.Add(new Raw(RawKind.Bullet, t[(dot + 2)..].Trim(), Numbered: true, Number: int.Parse(t[..dot], System.Globalization.CultureInfo.InvariantCulture), Indent: IndentOf(line)));
                continue;
            }
            para.Add(line);
        }
        Flush();

        var blocks = new List<MdBlock>(raws.Count);
        for (int k = 0; k < raws.Count; k++)
        {
            var r = raws[k];
            bool open = streaming && k == raws.Count - 1; // only the block still being written may hold an unclosed mark
            blocks.Add(r.Kind switch
            {
                RawKind.Heading => new MdBlock.Heading(r.Level, Inline(r.Src, open)),
                RawKind.Paragraph => new MdBlock.Paragraph(Inline(r.Src, open)),
                RawKind.Bullet => new MdBlock.Bullet(Inline(r.Src, open), r.Numbered, r.Number, r.Indent),
                _ => new MdBlock.Code(r.Language, r.Src),
            });
        }
        return blocks;
    }

    /// <summary>Leading blanks → a nesting level (two columns each, a tab is four), at most four deep.</summary>
    private static int IndentOf(string line)
    {
        int columns = 0;
        foreach (var c in line)
        {
            if (c == ' ') columns++;
            else if (c == '\t') columns += 4;
            else break;
        }
        return Math.Min(columns / 2, 4);
    }

    /// <summary>A last line that is nothing but the beginning of a block marker: <c>#</c>…<c>######</c>, a lone bullet, digits, <c>1.</c> / <c>1)</c>, one or two backticks.</summary>
    private static bool IsMarkerStub(string line)
    {
        var s = line.Trim();
        if (s.Length == 0) return false;
        if (s.Length <= 6 && s.All(c => c == '#')) return true;
        if (s is "-" or "*" or "•") return true;
        if (s.All(char.IsAsciiDigit)) return true;
        if (s.Length >= 2 && s[..^1].All(char.IsAsciiDigit) && (s[^1] is '.' or ')')) return true;
        return IsFenceStub(s);
    }

    private static bool IsFenceStub(string line)
    {
        var s = line.Trim();
        return s.Length is 1 or 2 && s.All(c => c == '`');
    }

    /// <summary>The text without its marks (`code`, **bold**, *italic*, [text](url) → text) — the collapsed card's plain two lines.</summary>
    public static string Plain(string? markdown) => Plain(markdown, open: false);

    /// <summary><see cref="Plain(string?)"/> for text still arriving (<c>open</c>: an unclosed mark counts as closed — no stray asterisks while it streams).</summary>
    public static string Plain(string? markdown, bool open) => string.Concat(Inline(markdown ?? "", open).Select(i => i switch
    {
        MdInline.Text t => t.Value,
        MdInline.Bold b => b.Value,
        MdInline.Italic it => it.Value,
        MdInline.Code c => c.Value,
        MdInline.Link l => l.Value,
        _ => "",
    }));

    public static IReadOnlyList<MdInline> Inline(string s) => Inline(s, open: false);

    /// <summary>
    /// <paramref name="open"/>: <paramref name="s"/> is the end of text that is still arriving, so a mark that has not closed yet is
    /// rendered as if it had (see <see cref="Parse(string?, bool)"/>). Closed text parses exactly as before.
    /// </summary>
    public static IReadOnlyList<MdInline> Inline(string s, bool open)
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
                if (open)
                {
                    Push();
                    if (i + 1 < s.Length) outList.Add(new MdInline.Code(s[(i + 1)..]));
                    i = s.Length;
                    continue;
                }
            }
            if (c == '*' && i + 1 < s.Length && s[i + 1] == '*')
            {
                int end = s.IndexOf("**", i + 2, StringComparison.Ordinal);
                if (end > i + 2) { Push(); outList.Add(new MdInline.Bold(s[(i + 2)..end])); i = end + 2; continue; }
                if (open && end < 0)
                {
                    Push();
                    var rest = s[(i + 2)..];
                    if (rest.EndsWith('*')) rest = rest[..^1]; // the first star of the closing pair
                    if (rest.Length > 0) outList.Add(new MdInline.Bold(rest));
                    i = s.Length;
                    continue;
                }
            }
            if ((c == '*' || (c == '_' && (i == 0 || !char.IsLetterOrDigit(s[i - 1])))) && i + 1 < s.Length && s[i + 1] != ' ')
            {
                // `_` opens emphasis only at a word start and closes only at a word end: snake_case_names stay whole (CommonMark's rule).
                int end = c == '*' ? s.IndexOf(c, i + 1) : ClosingUnderscore(s, i + 1);
                if (end > i + 1 && s[end - 1] != ' ') { Push(); outList.Add(new MdInline.Italic(s[(i + 1)..end])); i = end + 1; continue; }
                if (open && end < 0) { Push(); outList.Add(new MdInline.Italic(s[(i + 1)..])); i = s.Length; continue; }
            }
            if (c == '[')
            {
                int close = s.IndexOf("](", i + 1, StringComparison.Ordinal);
                int end = close > i ? s.IndexOf(')', close + 2) : -1;
                if (close > i && end > close) { Push(); outList.Add(new MdInline.Link(s[(i + 1)..close], s[(close + 2)..end])); i = end + 1; continue; }
                if (open && PendingLink(s, i) is { } label)
                {
                    Push();
                    if (label.Length > 0) outList.Add(new MdInline.Text(label));
                    i = s.Length;
                    continue;
                }
            }
            if (open && i == s.Length - 1 && c is '*' or '`') { i++; continue; } // a lone mark at the very end: the start of the next one
            text.Append(c);
            i++;
        }
        Push();
        return outList;
    }

    /// <summary>The index of the <c>_</c> that closes an emphasis opened before <paramref name="from"/>, or -1: not after a blank, not inside a word.</summary>
    private static int ClosingUnderscore(string s, int from)
    {
        for (int j = from; j < s.Length; j++)
        {
            if (s[j] != '_' || j == 0 || s[j - 1] == ' ') continue;
            if (j == s.Length - 1 || !char.IsLetterOrDigit(s[j + 1])) return j;
        }
        return -1;
    }

    /// <summary>
    /// A link still being typed at the end of <paramref name="s"/> (<c>[lab</c>, <c>[label]</c>, <c>[label](ur</c>): its label, else null.
    /// Bounded, so a stray <c>[</c> far from the end is just a character.
    /// </summary>
    private static string? PendingLink(string s, int at)
    {
        var rest = s[at..];
        if (rest.Length > 120) return null;
        int b = rest.IndexOf(']');
        if (b < 0) return rest[1..];
        if (b == rest.Length - 1) return rest[1..b];
        if (rest[b + 1] == '(' && rest.IndexOf(')', b + 2) < 0) return rest[1..b];
        return null;
    }
}

/// <summary>
/// 189w D4: how a view that keeps the blocks it already drew updates them when the text grows. A block's signature is its kind and
/// content; the common prefix of two signature lists is what stays untouched, and the first differing block of the same kind is
/// updated IN PLACE (the paragraph being written) — so a streamed chunk costs one block's layout, not the whole answer's.
/// </summary>
public static class MdDiff
{
    /// <summary>One letter per element type a view builds: <c>H</c> heading · <c>P</c> paragraph · <c>B</c> bullet · <c>C</c> code.</summary>
    public static char Kind(MdBlock b) => b switch
    {
        MdBlock.Heading => 'H',
        MdBlock.Paragraph => 'P',
        MdBlock.Bullet => 'B',
        MdBlock.Code => 'C',
        _ => '?',
    };

    /// <summary>Kind + every attribute + the inline marks, length-prefixed so two different blocks never share a signature.</summary>
    public static string Signature(MdBlock b)
    {
        var sb = new StringBuilder();
        sb.Append(Kind(b));
        switch (b)
        {
            case MdBlock.Heading h:
                sb.Append(h.Level).Append('|');
                Inlines(sb, h.Inlines);
                break;
            case MdBlock.Paragraph p:
                Inlines(sb, p.Inlines);
                break;
            case MdBlock.Bullet bl:
                sb.Append(bl.Numbered ? "n" + bl.Number : "u").Append('/').Append(bl.Indent).Append('|');
                Inlines(sb, bl.Inlines);
                break;
            case MdBlock.Code c:
                sb.Append(c.Language.Length).Append(':').Append(c.Language).Append(c.Text);
                break;
        }
        return sb.ToString();
    }

    private static void Inlines(StringBuilder sb, IReadOnlyList<MdInline> inlines)
    {
        foreach (var i in inlines)
        {
            switch (i)
            {
                case MdInline.Text x: sb.Append('t').Append(x.Value.Length).Append(':').Append(x.Value); break;
                case MdInline.Bold x: sb.Append('b').Append(x.Value.Length).Append(':').Append(x.Value); break;
                case MdInline.Italic x: sb.Append('i').Append(x.Value.Length).Append(':').Append(x.Value); break;
                case MdInline.Code x: sb.Append('c').Append(x.Value.Length).Append(':').Append(x.Value); break;
                case MdInline.Link x: sb.Append('l').Append(x.Value.Length).Append(':').Append(x.Value).Append('>').Append(x.Url.Length).Append(':').Append(x.Url); break;
            }
        }
    }

    /// <summary>How many leading signatures are identical (their elements stay as they are).</summary>
    public static int CommonPrefix(IReadOnlyList<string> before, IReadOnlyList<string> after)
    {
        int n = Math.Min(before.Count, after.Count), k = 0;
        while (k < n && string.Equals(before[k], after[k], StringComparison.Ordinal)) k++;
        return k;
    }
}
