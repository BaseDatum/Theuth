using System.Text;
using Theuth.Companion.Core;

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
        Ask.Router.Sent(session);
        var id = $"ask:{Guid.NewGuid().ToString().ToUpperInvariant()}";
        Ask.Current = id;
        Ask.Receipts.Add(new Core.Receipt { Id = id, Kind = ReceiptKind.Ask, At = DateTimeOffset.Now, Who = "theuth", Text = text, Session = session }, DateTimeOffset.Now);
        OnPropertyChanged(nameof(Ask));
        SetReceiptCount(Ask.Receipts.Count);
        SetReceipt(new ReceiptRow.Reply("theuth", "sent — waiting for the desk", session));
        foreach (var line in jsonLines) HandleEvent(Encoding.UTF8.GetBytes(line));
    }
}
