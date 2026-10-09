namespace Theuth.Companion.Core;

/// <summary>
/// 149 D18 / §9 Windows: the harness icon beside an agent row or a card's status line. The set is the shared
/// <c>crates/desktop/companion-assets/harness/</c> PNGs (16/20/24/32) copied beside the exe under <c>Assets\harness\</c>:
/// <c>omp</c> · <c>opencode</c> · <c>claude</c> · <c>codex</c> · <c>theuth</c> · <c>unknown</c>. The kind is the session's harness key
/// (<c>AgentInfo.agent</c> / <c>SessionSummary.agent</c>, an <c>agents.yaml</c> key such as <c>omp</c>, <c>opencode2</c>, <c>claude-code</c>);
/// null or empty is a native theuth session.
/// </summary>
public static class HarnessIcon
{
    public const string Omp = "omp", OpenCode = "opencode", Claude = "claude", Codex = "codex", Theuth = "theuth", Unknown = "unknown";

    /// <summary>The icon's base name for a harness kind: a known family, <c>theuth</c> for null/empty, else <c>unknown</c>.</summary>
    public static string Family(string? agentKind)
    {
        if (string.IsNullOrWhiteSpace(agentKind)) return Theuth;
        var k = agentKind.Trim().ToLowerInvariant();
        if (k == Theuth || k.StartsWith("theuth", StringComparison.Ordinal)) return Theuth;
        if (k.StartsWith("omp", StringComparison.Ordinal) || k.StartsWith("oh-my-pi", StringComparison.Ordinal)) return Omp;
        if (k.StartsWith("opencode", StringComparison.Ordinal)) return OpenCode;
        if (k.StartsWith("claude", StringComparison.Ordinal)) return Claude;
        if (k.StartsWith("codex", StringComparison.Ordinal)) return Codex;
        return Unknown;
    }

    /// <summary>The file name (<c>claude.png</c>, <c>unknown.png</c>) — the 1× asset; <see cref="For(string?, int)"/> picks a size.</summary>
    public static string For(string? agentKind) => Family(agentKind) + ".png";

    /// <summary>The sized file name (<c>claude-24.png</c>) for 16 · 20 · 24 · 32; any other size falls back to 24.</summary>
    public static string For(string? agentKind, int size)
    {
        int s = size is 16 or 20 or 24 or 32 ? size : 24;
        return $"{Family(agentKind)}-{s}.png";
    }

    /// <summary>Where the files live relative to the exe (the csproj copies the shared PNGs there).</summary>
    public const string Folder = "Assets\\harness";
}
