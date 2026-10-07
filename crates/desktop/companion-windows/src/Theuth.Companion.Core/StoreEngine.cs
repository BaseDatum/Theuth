using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>The engine calls the Store makes (Swift: the <c>RpcClient</c> extensions); <see cref="RpcStoreEngine"/> in the app, a fake in tests.</summary>
public interface IStoreEngine
{
    Task<ValueList<AttentionItem>> AttentionListAsync();
    Task<ValueList<AgentSummary>> AgentsListAsync();
    Task<ValueList<AgentInfo>> ListActivityAsync();
    Task<Packet?> PacketGetAsync(string key);
    Task<bool> AnswerAskAsync(string id, IReadOnlyList<Answer>? answers);
    Task<bool> ResolveApprovalAsync(string id, Decision decision);
    Task TaskDecideAsync(string decision, string answer);
    Task TaskMoveAsync(string id, string state, string? note);
    Task<string> SendInputAsync(string session, string text);
    Task AttentionDismissAsync(string id);
    Task AttentionSnoozeAsync(string id, ulong untilMs);
    Task<string> SteerAsync(string session, string text);
    Task<MeshStatus> MeshStatusAsync();
    Task<string> DeskOpenAsync(string name);
    Task SetEventFocusAsync(IReadOnlyList<string> sessions);
    Task<ValueList<ActivityEntry>> ActivityListAsync(ActivityFilter filter);
}

/// <summary>Timers on the Store's owner thread (the app: <c>DispatcherQueueTimer</c>; tests: virtual time). Dispose cancels.</summary>
public interface IStoreScheduler
{
    IDisposable After(TimeSpan delay, Action action);

    IDisposable Every(TimeSpan interval, Action action);
}

/// <summary><see cref="IStoreEngine"/> over the JSON-RPC client.</summary>
public sealed class RpcStoreEngine(RpcClient client) : IStoreEngine
{
    public RpcClient Client { get; } = client;
    public Task<ValueList<AttentionItem>> AttentionListAsync() => Client.AttentionListAsync();
    public Task<ValueList<AgentSummary>> AgentsListAsync() => Client.AgentsListAsync();
    public Task<ValueList<AgentInfo>> ListActivityAsync() => Client.ListActivityAsync();
    public Task<Packet?> PacketGetAsync(string key) => Client.PacketGetAsync(key);
    public Task<bool> AnswerAskAsync(string id, IReadOnlyList<Answer>? answers) => Client.AnswerAskAsync(id, answers);
    public Task<bool> ResolveApprovalAsync(string id, Decision decision) => Client.ResolveApprovalAsync(id, decision);
    public Task TaskDecideAsync(string decision, string answer) => Client.TaskDecideAsync(decision, answer);
    public Task TaskMoveAsync(string id, string state, string? note) => Client.TaskMoveAsync(id, state, note);
    public Task<string> SendInputAsync(string session, string text) => Client.SendInputAsync(session, text);
    public Task AttentionDismissAsync(string id) => Client.AttentionDismissAsync(id);
    public Task AttentionSnoozeAsync(string id, ulong untilMs) => Client.AttentionSnoozeAsync(id, untilMs);
    public Task<string> SteerAsync(string session, string text) => Client.SteerAsync(session, text);
    public Task<MeshStatus> MeshStatusAsync() => Client.MeshStatusAsync();
    public Task<string> DeskOpenAsync(string name) => Client.DeskOpenAsync(name);
    public Task SetEventFocusAsync(IReadOnlyList<string> sessions) => Client.SetEventFocusAsync(sessions);
    public Task<ValueList<ActivityEntry>> ActivityListAsync(ActivityFilter filter) => Client.ActivityListAsync(filter);
}
