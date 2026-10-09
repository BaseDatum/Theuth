namespace Theuth.Companion.Rpc;

/// <summary>
/// Design 149 WW1: the calls the One-parity surfaces (WW2), the native island and point-and-say (WW3) make — typed over
/// <see cref="CallAsync{T}"/> / <see cref="CallRawAsync"/>, positional params in <c>crates/rpc/src/api.rs</c> order (the
/// line numbers below), enums as their wire words. Error mapping, cancellation and reconnect are the client's (a lost
/// socket fails the pending call with <see cref="RpcException"/> <c>-32300</c>; the engine's own errors keep their code).
/// Already there and reused, not re-added: <c>ListActivityAsync</c>, <c>AgentsListAsync</c>, <c>SendInputAsync(session, text)</c>,
/// Core's <c>AskRpc</c> (<c>SteerAsync</c>, <c>MeshStatusAsync</c>, <c>DeskOpenAsync</c>, <c>ActivityListAsync</c>, <c>SetEventFocusAsync</c>,
/// <c>Dictation*</c>).
/// </summary>
public sealed partial class RpcClient
{
    // MARK: agents and sessions (api.rs:485 list_sessions · :489 list_agents · :549 sessions_status · :2088 agent_pause)

    /// <summary><c>list_sessions()</c>: every session with <c>running</c> / <c>owner</c> / <c>here</c> (the agents panel's host grouping).</summary>
    public Task<ValueList<SessionSummary>> ListSessionsAsync() => CallAsync<ValueList<SessionSummary>>("list_sessions");

    /// <summary><c>list_agents(session)</c>: the subagent tree under one session (<c>AgentInfo</c> rows, roots first).</summary>
    public Task<ValueList<AgentInfo>> ListAgentsAsync(string session) => CallAsync<ValueList<AgentInfo>>("list_agents", session);

    /// <summary><c>sessions_status()</c>: this engine's id and the here / elsewhere counts that label the host groups (149 E7 fallback).</summary>
    public Task<SessionsStatus> SessionsStatusAsync() => CallAsync<SessionsStatus>("sessions_status");

    /// <summary><c>agent_pause(name, paused)</c>: pause or resume a roster agent.</summary>
    public Task AgentPauseAsync(string name, bool paused) => CallRawAsync("agent_pause", name, paused);

    // MARK: control (api.rs:255 cancel · :194 send_input · :218 steer)

    /// <summary>
    /// <c>cancel(session, scope)</c>: abort the in-flight turn; <see cref="CancelScope.Tree"/> also stops every running descendant
    /// (the agents panel's ✕ and the card's X after the hold, 149 §8). <c>null</c> = the engine default (<c>root</c>).
    /// </summary>
    public Task CancelAsync(string session, CancelScope? scope) => CallRawAsync("cancel", session, scope?.Word());

    /// <summary>
    /// <c>send_input(session, text, files, delivery, id)</c> → the session id (a new one when <paramref name="session"/> is null).
    /// <paramref name="files"/> are engine-side paths (<c>screen_context</c>'s PNGs, <c>put_attachment</c>'s file) attached to the input.
    /// </summary>
    public Task<string> SendInputAsync(string? session, string text, IReadOnlyList<string> files, Delivery? delivery = null, string? id = null) =>
        CallAsync<string>("send_input", session, text, files.ToArray(), delivery?.Word(), id);

    /// <summary><c>steer(session, text, id, files, delivery)</c> → the input id; the running turn sees it after its current tool batch.</summary>
    public Task<string> SteerAsync(string session, string text, IReadOnlyList<string> files, Delivery? delivery = null, string? id = null) =>
        CallAsync<string>("steer", session, text, id, files.ToArray(), delivery?.Word());

    // MARK: screen (api.rs:335 screen_context · :1426 put_attachment)

    /// <summary>
    /// <c>screen_context(req)</c>: the engine captures the screen under the trail (and, where a reader exists, the element,
    /// app, window and URL under the pointer) and answers the <c>[screen …]</c> lines plus the PNG paths to pass as <c>files</c>.
    /// The overlays must be hidden before this call (149 D13 / 7.4 #4).
    /// </summary>
    public Task<ScreenContextReply> ScreenContextAsync(ScreenContextReq req) => CallAsync<ScreenContextReply>("screen_context", req);

    /// <summary><c>put_attachment(bytes, ext)</c> → the engine-side path of a file written from base64 <paramref name="base64"/> (ext without the dot).</summary>
    public Task<string> PutAttachmentAsync(string base64, string ext) => CallAsync<string>("put_attachment", base64, ext);

    // MARK: meeting (api.rs:1558-1609 · :1855 meeting_oracle_mode · :1580 meeting_permissions)

    /// <summary><c>meeting_status()</c>: state, meeting/session ids, elapsed, degradations, cursor, topics, approval flag, support.</summary>
    public Task<MeetingStatus> MeetingStatusAsync() => CallAsync<MeetingStatus>("meeting_status");

    /// <summary><c>meeting_live(id)</c>: the live transcript so far (the island's transcript view).</summary>
    public Task<ValueList<Utterance>> MeetingLiveAsync(string meeting) => CallAsync<ValueList<Utterance>>("meeting_live", meeting);

    /// <summary><c>meeting_briefs(id)</c>: every brief of the meeting, oldest first.</summary>
    public Task<ValueList<Brief>> MeetingBriefsAsync(string meeting) => CallAsync<ValueList<Brief>>("meeting_briefs", meeting);

    /// <summary><c>meeting_topics()</c>: the live topic strip of the current meeting.</summary>
    public Task<ValueList<Topic>> MeetingTopicsAsync() => CallAsync<ValueList<Topic>>("meeting_topics");

    /// <summary><c>meeting_cursor(brief)</c>: tell the engine which brief is on screen (null = none); it seeds "deeper" and the brief stack.</summary>
    public Task MeetingCursorAsync(string? brief) => CallRawAsync("meeting_cursor", brief);

    /// <summary><c>meeting_pause()</c> (island M).</summary>
    public Task MeetingPauseAsync() => CallRawAsync("meeting_pause");

    /// <summary><c>meeting_resume()</c>.</summary>
    public Task MeetingResumeAsync() => CallRawAsync("meeting_resume");

    /// <summary><c>meeting_stop()</c> (island Ctrl+Shift+M after the hold).</summary>
    public Task MeetingStopAsync() => CallRawAsync("meeting_stop");

    /// <summary><c>meeting_tap(topic)</c>: ask for the brief of a topic now.</summary>
    public Task MeetingTapAsync(string topic) => CallRawAsync("meeting_tap", topic);

    /// <summary><c>meeting_dismiss(topic)</c> (island E).</summary>
    public Task MeetingDismissAsync(string topic) => CallRawAsync("meeting_dismiss", topic);

    /// <summary><c>meeting_deeper(brief)</c> (island D): a longer follow-up on a brief.</summary>
    public Task MeetingDeeperAsync(string brief) => CallRawAsync("meeting_deeper", brief);

    /// <summary><c>meeting_request(text)</c>: the typed question of the island's request box.</summary>
    public Task MeetingRequestAsync(string text) => CallRawAsync("meeting_request", text);

    /// <summary><c>meeting_ptt(held)</c>: the meeting push-to-talk key went down (true) / up (false).</summary>
    public Task MeetingPttAsync(bool held) => CallRawAsync("meeting_ptt", held);

    /// <summary><c>meeting_oracle_mode()</c>: the oracle's <c>ModeInfo</c>.</summary>
    public Task<ModeInfo> MeetingOracleModeAsync() => CallAsync<ModeInfo>("meeting_oracle_mode");

    /// <summary><c>meeting_permissions()</c>: microphone / audio-capture permission words.</summary>
    public Task<MeetingPermissions> MeetingPermissionsAsync() => CallAsync<MeetingPermissions>("meeting_permissions");

    // MARK: briefs and speech (api.rs:1912 brief_get · :1895 brief_search · :1467 speak · :1470 stop_speaking)

    /// <summary><c>brief_get(id)</c>: one cached brief.</summary>
    public Task<CachedBrief> BriefGetAsync(string id) => CallAsync<CachedBrief>("brief_get", id);

    /// <summary><c>brief_search(query, limit)</c>: cached briefs by keywords (limit null = the engine's cap).</summary>
    public Task<ValueList<CachedBrief>> BriefSearchAsync(string query, uint? limit = null) =>
        CallAsync<ValueList<CachedBrief>>("brief_search", query, limit);

    /// <summary><c>speak(session, text)</c> → the speech id; the engine host's speaker says the point-and-say reply (149 Revision 1 #10).</summary>
    public Task<string> SpeakAsync(string session, string text) => CallAsync<string>("speak", session, text);

    /// <summary><c>stop_speaking(id)</c>; null stops whatever is speaking.</summary>
    public Task StopSpeakingAsync(string? id = null) => CallRawAsync("stop_speaking", id);
}

/// <summary>The wire words of the 149 enums (the shared enum converter is snake_case, which also fits; spelled out so a rename cannot drift).</summary>
public static class WireWords149
{
    /// <summary><c>root</c> · <c>tree</c>.</summary>
    public static string Word(this CancelScope s) => s == CancelScope.Tree ? "tree" : "root";

    /// <summary><c>steer</c> · <c>next_turn</c>.</summary>
    public static string Word(this Delivery d) => d == Delivery.NextTurn ? "next_turn" : "steer";
}
