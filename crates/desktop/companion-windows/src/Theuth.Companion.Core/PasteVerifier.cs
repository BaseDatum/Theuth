using System.Text;

namespace Theuth.Companion.Core;

// 153 §13.4 — paste verification, the pure half of the Windows twin (W153D). The app (AnyApp/FocusReader.cs) reads the focused field
// with UI Automation before the paste and at 150 / 350 / 1000 ms after; this decides what the reads mean.

/// <summary>§13.4's verdicts.</summary>
public enum PasteVerdict
{
    /// <summary>The field shows our words.</summary>
    Landed,
    /// <summary>The field did not change (all reads), or it changed with the restored clipboard instead of our words: the card.</summary>
    NotLanded,
    /// <summary>Focus moved, or the field cannot be read (no TextPattern / ValuePattern value, a secure field): no card.</summary>
    Unknown,
    /// <summary>The field changed, but not with our words — the user typed meanwhile: no card.</summary>
    UserActed,
}

/// <summary>How the utterance was delivered (decides the read schedule).</summary>
public enum VerifyMode
{
    /// <summary>SendInput typing (§13.2): its read-back.</summary>
    Direct,
    /// <summary>The clipboard lease (§13.3 step 6): the full triple.</summary>
    Lease,
}

/// <summary>
/// One read of the target field: whether focus is still the captured element, and what UIA answered —
/// <c>TextPattern.DocumentRange.GetText(60000)</c> (or <c>ValuePattern.Value</c>), the text just before the caret (≤ 256 chars, so an
/// insertion beyond the 60 000-char cap is still seen) and the selected text. A null field = that read did not answer.
/// </summary>
public sealed record FieldRead(bool FocusSame, string? Value, string? BeforeCaret, string? Selection, int? Count = null)
{
    public bool Readable => Value is not null || BeforeCaret is not null || Count is not null;

    public static FieldRead FocusMoved { get; } = new(false, null, null, null);

    public static FieldRead Unreadable { get; } = new(true, null, null, null);
}

/// <summary>
/// 153 §13.4: reads before the paste and at 150 / 350 / 1000 ms after (lease) — focus moved → <see cref="PasteVerdict.Unknown"/> (no card);
/// changed with our words → <see cref="PasteVerdict.Landed"/>; changed with the restored clipboard (a paste that landed after the
/// 500 ms settle) → <see cref="PasteVerdict.NotLanded"/>; changed otherwise → <see cref="PasteVerdict.UserActed"/> (no card); unchanged
/// at every read → <see cref="PasteVerdict.NotLanded"/>; unreadable → <see cref="PasteVerdict.Unknown"/>. A direct insertion verifies at
/// 150 ms; on Windows an unchanged field gets more looks (350, 1000 ms) before the lease takes over (SendInput is asynchronous).
/// </summary>
public static class PasteVerifier
{
    /// <summary>§13.4: the value read is capped here.</summary>
    public const int MaxValueChars = 60_000;

    /// <summary>The text-before-caret window each read takes.</summary>
    public const int CaretWindowChars = 256;

    /// <summary>§13.4: the lease's three reads, ms after the paste.</summary>
    public static IReadOnlyList<int> LeaseReadsMs { get; } = new[] { 150, 350, 1000 };

    /// <summary>§13.2's read-back at 150 ms; an unchanged field gets two more looks (350, 1000 ms) before the lease takes over, because
    /// SendInput and the target's UIA tree are both asynchronous on Windows and a late read must never re-deliver words that did land
    /// (Windows twin, as-built deviation). A field that shows the words returns at the first read.</summary>
    public static IReadOnlyList<int> DirectReadsMs { get; } = new[] { 150, 350, 1000 };

    public static IReadOnlyList<int> Schedule(VerifyMode mode) => mode == VerifyMode.Lease ? LeaseReadsMs : DirectReadsMs;

    /// <summary>
    /// The verdict after <paramref name="reads"/> (in schedule order), or null when the next scheduled read is still needed.
    /// <paramref name="restoredText"/>: the user's clipboard text the settle put back (null when none) — a late paste of it is not ours.
    /// </summary>
    public static PasteVerdict? Step(FieldRead before, IReadOnlyList<FieldRead> reads, string text, string? restoredText, VerifyMode mode)
    {
        if (!before.FocusSame || !before.Readable) return PasteVerdict.Unknown;
        if (reads.Count == 0) return null;
        var r = reads[^1];
        bool last = reads.Count >= Schedule(mode).Count;
        if (!r.FocusSame || !r.Readable) return PasteVerdict.Unknown;
        if (before.Value is not null && r.Value is not null && string.Equals(before.Value, r.Value, StringComparison.Ordinal))
        {
            // H3-D1: landed needs one MORE occurrence of our text; with the value unchanged it is not in (yet). A caret-only or
            // selection-only move is the user acting (unknown · userActed); nothing moved at all is notLanded.
            bool moved = !string.Equals(before.BeforeCaret, r.BeforeCaret, StringComparison.Ordinal)
                || !string.Equals(before.Selection, r.Selection, StringComparison.Ordinal);
            return last ? (moved ? PasteVerdict.UserActed : PasteVerdict.NotLanded) : null;
        }
        if (Changed(before, r))
        {
            if (Inserted(before, r, text)) return PasteVerdict.Landed;
            if (restoredText is { Length: > 0 } restored && Inserted(before, r, restored)) return PasteVerdict.NotLanded;
            return last ? PasteVerdict.UserActed : null; // a change that is not ours yet may still become ours (a paste in progress)
        }
        return last ? PasteVerdict.NotLanded : null;
    }

    /// <summary>All the reads at once (the tests' form): the first decided verdict, else the one after the last read.</summary>
    public static PasteVerdict Verdict(FieldRead before, IReadOnlyList<FieldRead> reads, string text, string? restoredText, VerifyMode mode)
    {
        int n = Math.Min(reads.Count, Schedule(mode).Count);
        for (int i = 1; i <= n; i++)
        {
            if (Step(before, reads.Take(i).ToList(), text, restoredText, mode) is { } v) return v;
        }
        return before.FocusSame && before.Readable ? PasteVerdict.NotLanded : PasteVerdict.Unknown;
    }

    /// <summary>Did anything the reads see change?</summary>
    public static bool Changed(FieldRead before, FieldRead after) =>
        !string.Equals(before.Value, after.Value, StringComparison.Ordinal)
        || !string.Equals(before.BeforeCaret, after.BeforeCaret, StringComparison.Ordinal)
        || !string.Equals(before.Selection, after.Selection, StringComparison.Ordinal)
        || before.Count != after.Count;

    /// <summary>
    /// Does <paramref name="after"/> show <paramref name="text"/> newly inserted? Either the text before the caret now ends with it (the
    /// caret sits after a paste or typing), or the value holds it more often than before. Compared normalised (line endings, smart
    /// quotes and dashes, no-break spaces, runs of spaces), so CRLF edits, RichEdit's CR and autocorrected quotes still count.
    /// </summary>
    public static bool Inserted(FieldRead before, FieldRead after, string text)
    {
        var t = Normalize(text).Trim();
        if (t.Length == 0) return false;
        if (after.Value is { } av && before.Value is { } bv)
        {
            // Both values readable: one MORE occurrence of our text than before (H3-D1), nothing else counts.
            return !string.Equals(av, bv, StringComparison.Ordinal) && Count(Normalize(av), t) > Count(Normalize(bv), t);
        }
        // Count-only elements (H3-D1): the count grew.
        if (after.Value is null && before.Value is null && before.Count is int bc && after.Count is int ac2 && after.BeforeCaret is null) return ac2 > bc;
        // Past the value cap (Windows): the 256 characters before the caret end with our text, after a caret move.
        if (after.BeforeCaret is { } ac && !string.Equals(after.BeforeCaret, before.BeforeCaret, StringComparison.Ordinal))
        {
            var tail = t.Length > CaretWindowChars - 56 ? t[^(CaretWindowChars - 56)..] : t;
            if (Normalize(ac).TrimEnd().EndsWith(tail, StringComparison.Ordinal)) return true;
        }
        return false;
    }

    /// <summary>The comparison form: CRLF/CR → LF, typographic quotes/dashes/ellipsis → ASCII, NBSP → space, space runs → one.</summary>
    public static string Normalize(string s)
    {
        var b = new StringBuilder(s.Length);
        char prev = '\0';
        for (int i = 0; i < s.Length; i++)
        {
            char c = s[i];
            if (c == '\r')
            {
                if (i + 1 < s.Length && s[i + 1] == '\n') continue;
                c = '\n';
            }
            c = c switch
            {
                '\u2018' or '\u2019' or '\u201A' or '\u2032' => '\'',
                '\u201C' or '\u201D' or '\u201E' or '\u2033' => '"',
                '\u2013' or '\u2014' or '\u2212' => '-',
                '\u00A0' or '\u2007' or '\u202F' or '\t' => ' ',
                _ => c,
            };
            if (c == '\u2026')
            {
                b.Append("...");
                prev = '.';
                continue;
            }
            if (c == ' ' && prev == ' ') continue;
            b.Append(c);
            prev = c;
        }
        return b.ToString();
    }

    private static int Count(string hay, string needle)
    {
        int n = 0;
        for (int i = hay.IndexOf(needle, StringComparison.Ordinal); i >= 0; i = hay.IndexOf(needle, i + needle.Length, StringComparison.Ordinal)) n++;
        return n;
    }

    /// <summary>The card footer's field state for a verdict ("field unchanged" / "focus moved" / …).</summary>
    public static string FieldState(PasteVerdict v) => v switch
    {
        PasteVerdict.NotLanded => "field unchanged",
        PasteVerdict.Landed => "landed",
        PasteVerdict.UserActed => "you typed meanwhile",
        _ => "not checked",
    };
}