namespace Theuth.Companion.Core;

/// <summary>
/// 149 D18 / §9 Windows: the harness icon beside an agent row or a card's status line. The set is the shared
/// <c>crates/desktop/companion-assets/harness/</c> (149 C: six marks, each rendered to <c>&lt;name&gt;-{16,20,24,32}.png</c>),
/// linked by the csproj beside the exe under <c>Assets\harness\</c>: <c>omp</c> · <c>opencode</c> · <c>claude</c> · <c>codex</c> ·
/// <c>theuth</c> · <c>unknown</c>. The kind is the session's harness key (<c>AgentInfo.agent</c> / <c>SessionSummary.agent</c>, an
/// <c>agents.yaml</c> key such as <c>omp</c>, <c>opencode2</c>, <c>claude-code</c>); null or empty is a native theuth session.
/// </summary>
public static class HarnessIcon
{
    public const string Omp = "omp", OpenCode = "opencode", Claude = "claude", Codex = "codex", Theuth = "theuth", Unknown = "unknown";

    /// <summary>The sizes the shared set ships.</summary>
    public static readonly IReadOnlyList<int> Sizes = new[] { 16, 20, 24, 32 };

    /// <summary>The icon's base name for a harness kind: a known family, <c>theuth</c> for null/empty, else <c>unknown</c>.</summary>
    public static string Family(string? agentKind)
    {
        if (string.IsNullOrWhiteSpace(agentKind)) return Theuth;
        var k = agentKind.Trim().ToLowerInvariant();
        if (k.StartsWith("theuth", StringComparison.Ordinal)) return Theuth;
        if (k.StartsWith("omp", StringComparison.Ordinal) || k.StartsWith("oh-my-pi", StringComparison.Ordinal)) return Omp;
        if (k.StartsWith("opencode", StringComparison.Ordinal)) return OpenCode;
        if (k.StartsWith("claude", StringComparison.Ordinal)) return Claude;
        if (k.StartsWith("codex", StringComparison.Ordinal)) return Codex;
        return Unknown;
    }

    /// <summary>The file name at the row size, 24 px (<c>claude-24.png</c>, <c>unknown-24.png</c>).</summary>
    public static string For(string? agentKind) => For(agentKind, 24);

    /// <summary>The file name for 16 · 20 · 24 · 32 (<c>claude-16.png</c>); any other size falls back to 24.</summary>
    public static string For(string? agentKind, int size)
    {
        int s = Sizes.Contains(size) ? size : 24;
        return $"{Family(agentKind)}-{s}.png";
    }

    /// <summary>Where the files live relative to the exe (the csproj links the shared PNGs there).</summary>
    public const string Folder = "Assets\\harness";

    /// <summary>The path relative to the exe: <c>Assets\harness\claude-24.png</c>.</summary>
    public static string Path(string? agentKind, int size = 24) => System.IO.Path.Combine(Folder, For(agentKind, size));
}
