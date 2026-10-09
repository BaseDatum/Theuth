using Theuth.Companion.Core;

namespace Theuth.Companion;

/// <summary>
/// 149 WW1's addition to <c>--probe N</c> (the W1 probe shape, <see cref="EngineProbe"/>): after the Store's first refetch it
/// logs what the live engine answered to the new read-only calls — <c>list_sessions</c> · <c>sessions_status</c> ·
/// <c>mesh_status</c> (from <c>Refetch149Async</c>), <c>meeting_status</c> (from <c>RefetchMeetingAsync</c>) and one
/// <c>list_agents(session)</c> on the first session — as <c>probe149 …</c> lines. Read-only; nothing is created.
/// </summary>
internal static class Probe149
{
    public static void Install(Store store)
    {
        bool agentsLogged = false;
        store.SessionsRefetched += () =>
        {
            var st = store.SessionsStatus;
            var mesh = store.Mesh;
            Log.Write($"probe149 list_sessions={store.Sessions.Count} running={store.Sessions.Count(s => s.Running)} remote={store.Sessions.Count(s => !s.Here)} " +
                      $"sessions_status={(st is null ? "n/a" : $"engine={st.Engine} here={st.Here} elsewhere={st.Elsewhere}")} " +
                      $"mesh_status={(mesh is null ? "n/a" : $"self={mesh.SelfName} peers={mesh.Peers.Count} reachable={mesh.Peers.Count(p => p.Reachable)}")}");
            if (agentsLogged || store.Sessions.Count == 0) return;
            agentsLogged = true;
            var first = store.Sessions[0].Id;
            _ = store.Client.ListAgentsAsync(first).ContinueWith(t =>
                Log.Write(t.IsCompletedSuccessfully
                    ? $"probe149 list_agents({first[..Math.Min(8, first.Length)]}…)={t.Result.Count} [{string.Join(",", t.Result.Select(a => $"{a.Name}:{a.State.Word}"))}]"
                    : $"probe149 list_agents failed: {t.Exception?.GetBaseException().Message}"));
        };
        store.MeetingRefetched += () =>
        {
            var m = store.Meeting;
            Log.Write($"probe149 meeting_status={(m is null ? "n/a" : $"state={m.State} meeting={m.Meeting ?? "none"} topics={m.Topics.Count} supported={m.Supported.Kind} degraded=[{string.Join(",", m.Degraded)}]")} briefs={store.Briefs.Count} live={store.Live.Count}");
        };
        Log.Write("probe149 installed (list_sessions · sessions_status · mesh_status · meeting_status · list_agents)");
    }
}
