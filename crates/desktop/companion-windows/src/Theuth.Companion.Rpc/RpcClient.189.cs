namespace Theuth.Companion.Rpc;

/// <summary>
/// Design 189w: the one transcript read the Ask catch-up makes. <c>load_session</c> is the only RPC that returns a turn's final text
/// (<c>crates/rpc/src/api.rs:552-558</c>, implementation :5237-5266): <c>Loaded { history, running, … }</c>.
/// </summary>
public sealed partial class RpcClient
{
    /// <summary>
    /// <c>load_session(session, before, limit)</c> → the last <paramref name="limit"/> transcript entries plus <c>running</c>.
    /// <c>before</c> is sent as null (the newest page); <c>settled_only</c> is left out (a running session answers too).
    /// </summary>
    public Task<SessionTail> LoadSessionAsync(string session, uint limit) =>
        CallAsync<SessionTail>("load_session", session, null, limit);
}
