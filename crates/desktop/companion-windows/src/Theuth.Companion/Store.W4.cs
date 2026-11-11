using System.Text;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion;

/// <summary>
/// W4's members (design 134 §7 W4; the Store contract in as-built W1 §7.1): the dictation calls over the engine connection,
/// and the recorded-stream replay that renders a desk reply through the real path when no model can answer. The ask flow
/// itself — <c>SubmitAsk</c> → hold → <c>mesh_status</c> → <c>desk_open("theuth")</c> → <c>send_input</c> → <c>TextDelta</c>/
/// <c>TurnFinished</c> → <c>Receipt</c>, <c>set_event_focus([desk])</c> on every <c>PendingPrompts</c> — is <see cref="StoreModel"/>'s.
/// </summary>
public sealed partial class Store
{
    private AskBar.DictationCalls? dictation;

    /// <summary><c>dictation_prewarm/start(16000)/audio/stop</c> over <see cref="Client"/> (<see cref="AskRpc"/>).</summary>
    internal IDictationCalls Dictation => dictation ??= new AskBar.DictationCalls(Client);

    /// <summary>
    /// <c>--ask-replay</c>: a recorded desk stream (the engine's <c>TextDelta</c> / <c>TurnFinished</c> lines) fed through
    /// <see cref="StoreModel.HandleEvent"/> as if <c>send_input</c> had just gone to <paramref name="session"/> — the
    /// receipt row shows the reply's first two lines exactly as it would live (the scratch engine has no model to answer).
    /// </summary>
    public void ReplayDeskReply(string session, string text, IEnumerable<string> jsonLines)
    {
        AskSent(session, text);
        var id = $"ask:{Guid.NewGuid().ToString().ToUpperInvariant()}";
        Ask.Current = id;
        Ask.Receipts.Add(new Core.Receipt { Id = id, Kind = ReceiptKind.Ask, At = DateTimeOffset.Now, Who = "theuth", Text = text, Session = session }, DateTimeOffset.Now);
        OnPropertyChanged(nameof(Ask));
        SetReceiptCount(Ask.Receipts.Count);
        SetReceipt(new ReceiptRow.Reply("theuth", "sent — waiting for the desk", session));
        foreach (var line in jsonLines) HandleEvent(Encoding.UTF8.GetBytes(line));
    }

    private string? replaySession;
    private SessionTail? replayTail;

    /// <summary>
    /// 189w: the transcript <c>--ask-replay</c>'s catch-up reads for <paramref name="session"/> — the recorded desk exists on no
    /// engine, so <c>load_session</c> has nothing to answer; every other session still goes to the engine.
    /// </summary>
    public void ReplayTranscript(string session, SessionTail tail)
    {
        replaySession = session;
        replayTail = tail;
    }

    private Func<PictureKey, Task<FenceSnapshot>>? replayPictures;

    /// <summary>
    /// 192w: the stub renderer <c>--ask-replay --ask-pictures</c> draws with — the real cache, the real outcome mapping and the real views,
    /// with no engine behind them (the scratch engine has no <c>fence_snapshot</c>, and no renderer client is connected on build-B).
    /// </summary>
    public void ReplayPictures(Func<PictureKey, Task<FenceSnapshot>> stub) => replayPictures = stub;

    protected override Task<FenceSnapshot> FetchPictureAsync(PictureKey key) =>
        replayPictures is { } stub ? stub(key) : base.FetchPictureAsync(key);

    protected override Task<SessionTail> LoadTailAsync(string session, uint limit) =>
        replayTail is { } tail && session == replaySession ? Task.FromResult(tail) : base.LoadTailAsync(session, limit);
}
