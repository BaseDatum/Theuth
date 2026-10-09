using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>
/// 149 E6 — the two context bubbles of a card ("you asked …" / "it last said …"). <see cref="Resolve"/> prefers the engine's
/// <see cref="AttentionItem.Context"/> (W0's <c>ItemContext { request, last_reply }</c>) and, while the engine predates it,
/// uses the interim join design 149 §6 E6 prescribes: <c>SessionSummary.title</c> (the first input, truncated, until the
/// title model runs) as the request and <c>AgentInfo.last</c> (first line of the last assistant text) as the last reply,
/// both looked up by the item's <c>session</c>. Either side may be missing; both are capped at <see cref="Max"/> chars.
/// </summary>
public static class AttentionContext
{
    /// <summary>The engine's <c>SUMMARY_MAX</c> for E6 (149 §6).</summary>
    public const int Max = 160;

    /// <summary>What the card shows; <see cref="FromEngine"/> says whether E6 or the interim join supplied it.</summary>
    public readonly record struct Resolved(string? Request, string? LastReply, bool FromEngine)
    {
        public static Resolved Empty => new(null, null, false);

        public bool IsEmpty => Request is null && LastReply is null;
    }

    public static Resolved Resolve(AttentionItem item, IReadOnlyList<SessionSummary> sessions, IReadOnlyList<AgentInfo> activity)
    {
        if (item.Context is { } c && (c.Request is not null || c.LastReply is not null))
        {
            return new Resolved(Clip(c.Request), Clip(c.LastReply), true);
        }
        if (item.Session is not { } session || session.Length == 0) return Resolved.Empty;
        string? request = null;
        foreach (var s in sessions)
        {
            if (s.Id == session)
            {
                request = Clip(s.Title);
                break;
            }
        }
        string? last = null;
        foreach (var a in activity)
        {
            if (a.Id == session)
            {
                last = Clip(a.Last);
                break;
            }
        }
        return new Resolved(request, last, false);
    }

    /// <summary>Trimmed, first line only, ≤ <see cref="Max"/> chars on a text-element boundary with an ellipsis; null when blank.</summary>
    public static string? Clip(string? text)
    {
        if (text is null) return null;
        var t = text.Trim();
        int nl = t.IndexOfAny(new[] { '\r', '\n' });
        if (nl >= 0) t = t[..nl].TrimEnd();
        if (t.Length == 0) return null;
        var info = new System.Globalization.StringInfo(t);
        if (info.LengthInTextElements <= Max) return t;
        return info.SubstringByTextElements(0, Max - 1) + "…";
    }
}
