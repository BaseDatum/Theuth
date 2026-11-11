using System.Text;

namespace Theuth.Companion.Core;

/// <summary>
/// 192w P1: which fences are pictures. The engine's <c>fence_snapshot</c> draws <c>json-render</c> (alias <c>jsonrender</c>),
/// <c>mermaid</c> and <c>openui</c>; every other fence stays a code block. A fence is named by the first word of its info string,
/// in any case. Model text that cannot carry a picture — reasoning, the two-line preview, speech — never shows a fence body
/// (design 164's rule, <c>warnings/theuth-channel-reasoning-text-leaks-fence-bodies</c>): <see cref="Flatten"/> replaces a picture
/// fence, closed or still arriving, by one short line.
/// </summary>
public static class PictureFence
{
    public const string JsonRender = "json-render";
    public const string Mermaid = "mermaid";
    public const string OpenUi = "openui";

    /// <summary>The caption under a fence that could not be drawn (it is shown as code).</summary>
    public const string FallbackCaption = "open theuth to see the diagram";

    /// <summary>The one-line placeholder while a picture fence is still arriving or being drawn — never its source.</summary>
    public const string Drawing = "drawing…";

    /// <summary>
    /// What a picture fence is replaced by where no picture can go. Parentheses, not brackets: in the two-line preview the text goes through
    /// the markdown plain-text pass, which reads <c>[label]</c> at the end of streaming text as a link still being typed and drops the brackets.
    /// </summary>
    public const string Flat = "(diagram)";

    private static readonly string[] Names = { JsonRender, "jsonrender", Mermaid, OpenUi };

    /// <summary>The engine's <c>lang</c> for an info string — <c>json-render</c> · <c>mermaid</c> · <c>openui</c> — or null when the fence is not a picture.</summary>
    public static string? Lang(string? info)
    {
        var word = FirstWord(info);
        return word switch
        {
            "json-render" or "jsonrender" => JsonRender,
            "mermaid" => Mermaid,
            "openui" => OpenUi,
            _ => null,
        };
    }

    public static bool Is(string? info) => Lang(info) is not null;

    /// <summary>
    /// A fence-opening line that is still being typed and may become a picture fence: <c>```</c> alone or followed by the start of a
    /// picture name (<c>```mer</c>) — held back while it is the last line of streaming text, so no empty code box flashes first.
    /// </summary>
    public static bool IsOpeningStub(string line)
    {
        var t = line.Trim();
        if (!t.StartsWith("```", StringComparison.Ordinal)) return false;
        var rest = t[3..].Trim();
        if (rest.Length == 0) return t.Length == 3;
        if (rest.Any(char.IsWhiteSpace)) return false;
        var lower = rest.ToLowerInvariant();
        return Names.Any(n => n.Length > lower.Length && n.StartsWith(lower, StringComparison.Ordinal));
    }

    /// <summary>
    /// <paramref name="text"/> with every picture fence — closed, or open to the end because it is still arriving — replaced by
    /// <paramref name="replacement"/> on one line (nothing when empty). Other fences, and everything else, pass through unchanged.
    /// </summary>
    public static string Flatten(string? text, string replacement = Flat)
    {
        if (string.IsNullOrEmpty(text) || !text.Contains("```", StringComparison.Ordinal)) return text ?? "";
        var lines = text.Replace("\r\n", "\n").Split('\n');
        var sb = new StringBuilder(text.Length);
        bool first = true;
        void Line(string s)
        {
            if (!first) sb.Append('\n');
            sb.Append(s);
            first = false;
        }
        for (int i = 0; i < lines.Length; i++)
        {
            var t = lines[i].TrimStart();
            if (!t.StartsWith("```", StringComparison.Ordinal))
            {
                Line(lines[i]);
                continue;
            }
            if (!Is(t[3..]))
            {
                // Another fence: its lines pass through up to and including its closing fence.
                Line(lines[i]);
                for (i++; i < lines.Length; i++)
                {
                    Line(lines[i]);
                    if (lines[i].TrimStart().StartsWith("```", StringComparison.Ordinal)) break;
                }
                continue;
            }
            for (i++; i < lines.Length && !lines[i].TrimStart().StartsWith("```", StringComparison.Ordinal); i++) { }
            if (replacement.Length > 0) Line(replacement);
        }
        return sb.ToString();
    }

    /// <summary>Speech: picture fences are not read aloud (the engine's <c>strip_pictures</c>); blank runs left behind collapse.</summary>
    public static string StripForVoice(string? text)
    {
        var flat = Flatten(text, "");
        while (flat.Contains("\n\n\n", StringComparison.Ordinal)) flat = flat.Replace("\n\n\n", "\n\n", StringComparison.Ordinal);
        return flat.Trim();
    }

    private static string FirstWord(string? info)
    {
        if (string.IsNullOrWhiteSpace(info)) return "";
        var t = info.Trim();
        int end = 0;
        while (end < t.Length && !char.IsWhiteSpace(t[end])) end++;
        return t[..end].ToLowerInvariant();
    }
}