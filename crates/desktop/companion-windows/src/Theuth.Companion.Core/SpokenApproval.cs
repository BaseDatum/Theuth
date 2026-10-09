using System.Globalization;
using System.Text;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>
/// 153 §10.6 (D10): the spoken answer to an approval box in the command card. While the box is up the companion opens a short
/// dictation (<c>dictation_start</c>, <c>Surface::Companion</c>, end commands off) and matches the FIRST final utterance here:
/// <c>yes | yeah | go | go ahead | do it | approve</c> → <see cref="Decision.Allow"/>, <c>no | nope | stop | cancel | deny</c> →
/// <see cref="Decision.Deny"/>, anything else → null (the words are ignored and the box stays: "didn't catch that — Enter or Esc");
/// <see cref="ListenSeconds"/> with no match closes the listen (the keys still work). Near misses as §7.1: only phrases of
/// ≥ <see cref="NearMissMinLen"/> chars, within <c>max(1, len / 5)</c> edits. Pure; the driver owns the dictation and the timer.
/// <para>Safety (writer's call): an utterance is decided only when every word belongs to phrases of ONE polarity (politeness words
/// such as "please" aside) — "no, go ahead", "don't do it" or "yes but stop" are null, never a guess — and a near miss never wins
/// over a word of the other polarity.</para>
/// </summary>
public static class SpokenApproval
{
    public static readonly IReadOnlyList<string> AllowPhrases = new[] { "yes", "yeah", "go", "go ahead", "do it", "approve" };

    public static readonly IReadOnlyList<string> DenyPhrases = new[] { "no", "nope", "stop", "cancel", "deny" };

    /// <summary>Words that carry no answer and may surround one ("yes please", "go ahead now").</summary>
    public static readonly IReadOnlyList<string> Fillers = new[] { "please", "now", "thanks", "thank", "you" };

    /// <summary>§7.1 <c>NEAR_MISS_MIN_LEN</c>: shorter phrases need an exact match.</summary>
    public const int NearMissMinLen = 5;

    /// <summary>§10.6: the listen window; with no match by then the listen closes (Q7: a <c>const</c> with one home).</summary>
    public const int ListenSeconds = 8;

    /// <summary>The box's line when an utterance matched nothing.</summary>
    public const string DidntCatch = "didn't catch that — Enter or Esc";

    /// <summary>§7.1 <c>TOLERANCE = max(1, len / 5)</c> for phrases of ≥ 5 chars, else 0.</summary>
    public static int Tolerance(string phrase) => phrase.Length >= NearMissMinLen ? Math.Max(1, phrase.Length / 5) : 0;

    /// <summary>The decision an utterance carries, or null (unknown, mixed, empty).</summary>
    public static Decision? Decide(string? utterance)
    {
        var words = Words(utterance);
        if (words.Count == 0) return null;
        if (Parse(words) is { } exact) return exact;
        // Near miss: the utterance without fillers against the long phrases, never over a word of the other polarity.
        var core = string.Join(' ', words.Where(w => !Fillers.Contains(w)));
        if (core.Length == 0) return null;
        bool allowNear = AllowPhrases.Any(p => Tolerance(p) > 0 && Distance(core, p) <= Tolerance(p));
        bool denyNear = DenyPhrases.Any(p => Tolerance(p) > 0 && Distance(core, p) <= Tolerance(p));
        if (allowNear == denyNear) return null;
        var opposite = allowNear ? DenyPhrases : AllowPhrases;
        if (words.Any(w => opposite.Contains(w))) return null;
        return allowNear ? Decision.Allow : Decision.Deny;
    }

    /// <summary>Every word consumed by phrases of one polarity (multi-word phrases first) plus fillers; null when anything is left or both polarities appear.</summary>
    private static Decision? Parse(IReadOnlyList<string> words)
    {
        bool allow = false, deny = false;
        int i = 0;
        while (i < words.Count)
        {
            if (Fillers.Contains(words[i])) { i++; continue; }
            int n = Match(words, i, AllowPhrases);
            if (n > 0) { allow = true; i += n; continue; }
            n = Match(words, i, DenyPhrases);
            if (n > 0) { deny = true; i += n; continue; }
            return null;
        }
        if (allow == deny) return null;
        return allow ? Decision.Allow : Decision.Deny;
    }

    /// <summary>The longest phrase of <paramref name="phrases"/> starting at word <paramref name="i"/>, in words (0 = none).</summary>
    private static int Match(IReadOnlyList<string> words, int i, IReadOnlyList<string> phrases)
    {
        int best = 0;
        foreach (var p in phrases)
        {
            var parts = p.Split(' ');
            if (i + parts.Length > words.Count || parts.Length <= best) continue;
            bool ok = true;
            for (int k = 0; k < parts.Length && ok; k++) ok = words[i + k] == parts[k];
            if (ok) best = parts.Length;
        }
        return best;
    }

    /// <summary>Lower-cased words; punctuation (the shaper's "Yes." / "Go ahead!") is a separator, apostrophes stay inside a word ("don't").</summary>
    public static IReadOnlyList<string> Words(string? s)
    {
        if (string.IsNullOrWhiteSpace(s)) return Array.Empty<string>();
        var sb = new StringBuilder(s.Length);
        foreach (var ch in s.Normalize(NormalizationForm.FormKC).ToLower(CultureInfo.InvariantCulture))
        {
            if (char.IsLetterOrDigit(ch)) sb.Append(ch);
            else if (ch is '\'' or '\u2019') sb.Append('\'');
            else sb.Append(' ');
        }
        return sb.ToString().Split(' ', StringSplitOptions.RemoveEmptyEntries).Select(w => w.Trim('\'')).Where(w => w.Length > 0).ToList();
    }

    /// <summary>Levenshtein distance (insert, delete, substitute = 1).</summary>
    public static int Distance(string a, string b)
    {
        var prev = new int[b.Length + 1];
        var cur = new int[b.Length + 1];
        for (int j = 0; j <= b.Length; j++) prev[j] = j;
        for (int i = 1; i <= a.Length; i++)
        {
            cur[0] = i;
            for (int j = 1; j <= b.Length; j++)
            {
                int sub = prev[j - 1] + (a[i - 1] == b[j - 1] ? 0 : 1);
                cur[j] = Math.Min(sub, Math.Min(prev[j] + 1, cur[j - 1] + 1));
            }
            (prev, cur) = (cur, prev);
        }
        return prev[b.Length];
    }
}

/// <summary>
/// The listen window around <see cref="SpokenApproval"/> (§10.6): opened when the box shows, fed every final utterance, closed by a
/// match, by <see cref="SpokenApproval.ListenSeconds"/> or by the box going away. Pure; the driver passes its clock.
/// </summary>
public sealed class ApprovalListen
{
    public ApprovalListen(string approvalId, ulong openedAtMs)
    {
        ApprovalId = approvalId;
        OpenedAtMs = openedAtMs;
    }

    public string ApprovalId { get; }

    public ulong OpenedAtMs { get; }

    /// <summary>The box's extra line: null, or "didn't catch that — Enter or Esc" after an unknown utterance.</summary>
    public string? Line { get; private set; }

    public bool Closed { get; private set; }

    public Decision? Decided { get; private set; }

    /// <summary>One final utterance: the decision when it matched (the listen closes), else null and <see cref="Line"/> says so.</summary>
    public Decision? Final(string text, ulong nowMs)
    {
        if (Closed || Expired(nowMs)) { Closed = true; return null; }
        var d = SpokenApproval.Decide(text);
        if (d is { } decision)
        {
            Decided = decision;
            Closed = true;
            Line = null;
            return decision;
        }
        if (SpokenApproval.Words(text).Count > 0) Line = SpokenApproval.DidntCatch;
        return null;
    }

    /// <summary>The window ran out (the listen closes; the keys still answer the box).</summary>
    public bool Expired(ulong nowMs) => nowMs >= OpenedAtMs + (ulong)SpokenApproval.ListenSeconds * 1000;

    public void Close() => Closed = true;
}