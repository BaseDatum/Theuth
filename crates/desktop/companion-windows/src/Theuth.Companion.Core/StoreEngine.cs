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

    /// <summary>134 W2 — Hire: <c>agent_approve(name)</c>. Default members so engines written before W2 (tests, W3/W4) still compile.</summary>
    Task AgentApproveAsync(string name) => Task.FromException(new NotSupportedException("agent_approve"));

    /// <summary>134 W2 — Decline a hire: <c>agent_remove(name, note)</c>.</summary>
    Task AgentRemoveAsync(string name, string? note) => Task.FromException(new NotSupportedException("agent_remove"));

    // MARK: 149 WW1 (default members: engines written before 149 — the W1–W6 test fakes — still compile; the real one overrides every member)

    Task<ValueList<SessionSummary>> ListSessionsAsync() => Task.FromException<ValueList<SessionSummary>>(new NotSupportedException("list_sessions"));
    Task<SessionsStatus> SessionsStatusAsync() => Task.FromException<SessionsStatus>(new NotSupportedException("sessions_status"));
    Task CancelAsync(string session, CancelScope? scope) => Task.FromException(new NotSupportedException("cancel"));
    Task<string> SendInputAsync(string? session, string text, IReadOnlyList<string> files) => Task.FromException<string>(new NotSupportedException("send_input"));
    Task<string> SteerAsync(string session, string text, IReadOnlyList<string> files) => Task.FromException<string>(new NotSupportedException("steer"));
    Task<ScreenContextReply> ScreenContextAsync(ScreenContextReq req) => Task.FromException<ScreenContextReply>(new NotSupportedException("screen_context"));
    Task<string> PutAttachmentAsync(string base64, string ext) => Task.FromException<string>(new NotSupportedException("put_attachment"));
    Task<MeetingStatus> MeetingStatusAsync() => Task.FromException<MeetingStatus>(new NotSupportedException("meeting_status"));
    Task<ValueList<Utterance>> MeetingLiveAsync(string meeting) => Task.FromException<ValueList<Utterance>>(new NotSupportedException("meeting_live"));
    Task<ValueList<Brief>> MeetingBriefsAsync(string meeting) => Task.FromException<ValueList<Brief>>(new NotSupportedException("meeting_briefs"));
    Task<ValueList<Topic>> MeetingTopicsAsync() => Task.FromException<ValueList<Topic>>(new NotSupportedException("meeting_topics"));
    Task MeetingCursorAsync(string? brief) => Task.FromException(new NotSupportedException("meeting_cursor"));
    Task MeetingPauseAsync() => Task.FromException(new NotSupportedException("meeting_pause"));
    Task MeetingResumeAsync() => Task.FromException(new NotSupportedException("meeting_resume"));
    Task MeetingStopAsync() => Task.FromException(new NotSupportedException("meeting_stop"));
    Task MeetingTapAsync(string topic) => Task.FromException(new NotSupportedException("meeting_tap"));
    Task MeetingDismissAsync(string topic) => Task.FromException(new NotSupportedException("meeting_dismiss"));
    Task MeetingDeeperAsync(string brief) => Task.FromException(new NotSupportedException("meeting_deeper"));
    Task MeetingRequestAsync(string text) => Task.FromException(new NotSupportedException("meeting_request"));
    Task MeetingPttAsync(bool held) => Task.FromException(new NotSupportedException("meeting_ptt"));
    // 158 §5.4 / §15.2 (W1W): the island prompt.
    Task<MeetingStarted> MeetingStartAsync(StartMeeting start) => Task.FromException<MeetingStarted>(new NotSupportedException("meeting_start"));
    Task MeetingDetectionDismissAsync(string appId, bool never) => Task.FromException(new NotSupportedException("meeting_detection_dismiss"));
    Task<ModeInfo> MeetingOracleModeAsync() => Task.FromException<ModeInfo>(new NotSupportedException("meeting_oracle_mode"));
    Task<MeetingPermissions> MeetingPermissionsAsync() => Task.FromException<MeetingPermissions>(new NotSupportedException("meeting_permissions"));
    Task<CachedBrief> BriefGetAsync(string id) => Task.FromException<CachedBrief>(new NotSupportedException("brief_get"));
    Task<ValueList<CachedBrief>> BriefSearchAsync(string query, uint? limit) => Task.FromException<ValueList<CachedBrief>>(new NotSupportedException("brief_search"));
    Task<string> SpeakAsync(string session, string text) => Task.FromException<string>(new NotSupportedException("speak"));
    Task StopSpeakingAsync(string? id) => Task.FromException(new NotSupportedException("stop_speaking"));
    Task AgentPauseAsync(string name, bool paused) => Task.FromException(new NotSupportedException("agent_pause"));
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
    public Task AgentApproveAsync(string name) => Client.CallRawAsync("agent_approve", name);
    public Task AgentRemoveAsync(string name, string? note) => Client.CallRawAsync("agent_remove", name, note);

    // 149 WW1
    public Task<ValueList<SessionSummary>> ListSessionsAsync() => Client.ListSessionsAsync();
    public Task<SessionsStatus> SessionsStatusAsync() => Client.SessionsStatusAsync();
    public Task CancelAsync(string session, CancelScope? scope) => Client.CancelAsync(session, scope);
    public Task<string> SendInputAsync(string? session, string text, IReadOnlyList<string> files) => Client.SendInputAsync(session, text, files);
    public Task<string> SteerAsync(string session, string text, IReadOnlyList<string> files) => Client.SteerAsync(session, text, files);
    public Task<ScreenContextReply> ScreenContextAsync(ScreenContextReq req) => Client.ScreenContextAsync(req);
    public Task<string> PutAttachmentAsync(string base64, string ext) => Client.PutAttachmentAsync(base64, ext);
    public Task<MeetingStatus> MeetingStatusAsync() => Client.MeetingStatusAsync();
    public Task<ValueList<Utterance>> MeetingLiveAsync(string meeting) => Client.MeetingLiveAsync(meeting);
    public Task<ValueList<Brief>> MeetingBriefsAsync(string meeting) => Client.MeetingBriefsAsync(meeting);
    public Task<ValueList<Topic>> MeetingTopicsAsync() => Client.MeetingTopicsAsync();
    public Task MeetingCursorAsync(string? brief) => Client.MeetingCursorAsync(brief);
    public Task MeetingPauseAsync() => Client.MeetingPauseAsync();
    public Task MeetingResumeAsync() => Client.MeetingResumeAsync();
    public Task MeetingStopAsync() => Client.MeetingStopAsync();
    public Task MeetingTapAsync(string topic) => Client.MeetingTapAsync(topic);
    public Task MeetingDismissAsync(string topic) => Client.MeetingDismissAsync(topic);
    public Task MeetingDeeperAsync(string brief) => Client.MeetingDeeperAsync(brief);
    public Task MeetingRequestAsync(string text) => Client.MeetingRequestAsync(text);
    public Task MeetingPttAsync(bool held) => Client.MeetingPttAsync(held);
    public Task<MeetingStarted> MeetingStartAsync(StartMeeting start) => Client.MeetingStartAsync(start);
    public Task MeetingDetectionDismissAsync(string appId, bool never) => Client.MeetingDetectionDismissAsync(appId, never);
    public Task<ModeInfo> MeetingOracleModeAsync() => Client.MeetingOracleModeAsync();
    public Task<MeetingPermissions> MeetingPermissionsAsync() => Client.MeetingPermissionsAsync();
    public Task<CachedBrief> BriefGetAsync(string id) => Client.BriefGetAsync(id);
    public Task<ValueList<CachedBrief>> BriefSearchAsync(string query, uint? limit) => Client.BriefSearchAsync(query, limit);
    public Task<string> SpeakAsync(string session, string text) => Client.SpeakAsync(session, text);
    public Task StopSpeakingAsync(string? id) => Client.StopSpeakingAsync(id);
    public Task AgentPauseAsync(string name, bool paused) => Client.AgentPauseAsync(name, paused);
}
