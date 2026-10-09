using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion;

/// <summary>
/// 149 WW2 visual fixture (<c>--ww2-fixture</c>): build-B's engine has no model credential, so approval / ask / finished cards
/// and live agent rows cannot arise there tonight. With the flag the Store overlays fixture attention items, agent rows and
/// rail dots after every refetch (real seeded items — decision · blocked · hire — stay underneath). Never on without the flag;
/// the as-built names every screenshot taken with it.
/// </summary>
internal static class Ww2Fixture
{
    public static bool Enabled { get; } = Environment.GetCommandLineArgs().Contains("--ww2-fixture");

    private static ulong Ms(double secondsAgo) => (ulong)(DateTimeOffset.UtcNow.AddSeconds(-secondsAgo).ToUnixTimeMilliseconds());

    public const string AskSession = "fx-ask-session";
    public const string DoneSession = "fx-done-session";
    public const string ApprovalSession = "fx-approval-session";

    public static IReadOnlyList<AttentionItem> Items() => new[]
    {
        new AttentionItem
        {
            Id = "ask:fx-1", Kind = AttentionKind.Ask, Title = "Which spacing between the rail pills?", AtMs = Ms(120), WaitingSinceMs = Ms(120),
            Agent = "w1-149-parity", Session = AskSession, Summary = "Three layouts are drafted. One question before I build.",
            Ask = new AskRequested
            {
                Id = "fx-ask-1", Session = AskSession,
                Questions = ValueList.Of(new Question
                {
                    Id = "q1", Text = "Which spacing between the rail pills?", Recommended = 0,
                    Options = ValueList.Of(
                        new Choice { Label = "12 pt gaps, 52 pt pills" },
                        new Choice { Label = "8 pt gaps, 44 pt pills" },
                        new Choice { Label = "One continuous capsule as today" }),
                }),
            },
        },
        new AttentionItem
        {
            Id = "approval:fx-2", Kind = AttentionKind.Approval, Title = "shell: git push origin feat/149w-ww2", AtMs = Ms(40), WaitingSinceMs = Ms(40),
            Agent = "w2-149-point", Session = ApprovalSession, Approval = "fx-approval-2", Detail = "shell · mutating · git push origin feat/149w-ww2",
        },
        new AttentionItem
        {
            Id = "finished:fx-3", Kind = AttentionKind.Finished, Title = "Island panel is in; two next steps", AtMs = Ms(20), WaitingSinceMs = Ms(20),
            Agent = "w3-149-island", Session = DoneSession,
            Summary = "Island panel is in. Drag, snap-back and per-display position memory; 14 tests green.",
            NextSteps = ValueList.Of("Looks good — merge it", "Show me the island on a screen without a notch"),
        },
    };

    public static IReadOnlyList<AgentInfo> Activity() => new[]
    {
        Info(AskSession, "w1-149-parity", "omp", AgentState.Awaiting("user"), last: "Three layouts are drafted. One question before I build.", project: "theuth"),
        Info("fx-island", "w3-149-island", "omp", AgentState.Running, children: 2, last: "Wiring the island's drag and snap-back", project: "theuth"),
        Info("fx-island-kid", "overlay-probe", "omp", AgentState.Running, parent: "fx-island", last: "Drawing the trail overlay"),
        Info("fx-docs", "docs-149 writer", "opencode", AgentState.Idle, last: "Waiting for the as-builts", project: "theuth"),
        Info(ApprovalSession, "w2-149-point", "claude", AgentState.Approval("shell"), last: "Pushing the branch", project: "theuth"),
    };

    public static IReadOnlyList<SessionRow> Sessions() => new[]
    {
        new SessionRow(AskSession, "Bring the rail to One's pill layout", "fx-self", true, "omp", "/Users/sky/Develop/theuth", "theuth"),
        new SessionRow("fx-island", "149 island native", "fx-self", true, "omp", "/Users/sky/Develop/theuth", "theuth"),
        new SessionRow(DoneSession, "149 W3 — the island", "fx-self", false, "omp", "/Users/sky/Develop/theuth", "theuth"),
        new SessionRow("fx-remote-1", "134-W7 WinUI rail", "fx-build-b", true, "opencode", null, "theuth", UpdatedAt: DateTimeOffset.UtcNow.AddMinutes(-1)),
        new SessionRow("fx-remote-2", "134 verifier", "fx-build-b", false, "omp", null, "theuth", UpdatedAt: DateTimeOffset.UtcNow.AddMinutes(-3)),
    };

    public static IReadOnlyList<HostPeer> Peers() => new[] { new HostPeer("fx-build-b", "build-B") };

    private static AgentInfo Info(string id, string name, string agent, AgentState state, string parent = "", ushort children = 0, string last = "", string? project = null) => new()
    {
        Id = id, Name = name, Parent = parent, Depth = (byte)(parent.Length == 0 ? 0 : 1), Agent = agent, Model = "claude", State = state, Last = last, Project = project,
        StartedAt = DateTimeOffset.UtcNow.AddMinutes(-9),
        Activity = new SessionActivity { State = state, TurnStartedAt = state.Kind == AgentStateKind.Running ? DateTimeOffset.UtcNow.AddMinutes(-4) : null, Children = children, Live = true },
    };
}
