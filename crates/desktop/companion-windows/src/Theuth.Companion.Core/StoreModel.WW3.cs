namespace Theuth.Companion.Core;

/// <summary>
/// 149 WW3 R2 (§8 panel P "point — attach the screen"): the PNGs <c>screen_context</c> wrote, parked on the card's session until
/// the reply row sends — <see cref="CardAction.SendInputTo"/> then goes out as <c>send_input(session, text, files)</c>. One list per
/// session; a second P replaces it; clearing the reply (E) or sending drains it. The panel reads <see cref="ReplyAttachmentsFor"/>
/// to say "N screenshots attached".
/// </summary>
public partial class StoreModel
{
    private readonly Dictionary<string, IReadOnlyList<string>> replyAttachments = new(StringComparer.Ordinal);

    /// <summary>Raised on the UI thread when a session's parked screenshots changed (attach · take · clear).</summary>
    public event Action<string>? ReplyAttachmentsChanged;

    /// <summary>Park <paramref name="files"/> for the next reply to <paramref name="session"/> (replaces any earlier set).</summary>
    public void AttachToReply(string session, IReadOnlyList<string> files)
    {
        if (files.Count == 0) replyAttachments.Remove(session);
        else replyAttachments[session] = files.ToArray();
        ReplyAttachmentsChanged?.Invoke(session);
    }

    public IReadOnlyList<string> ReplyAttachmentsFor(string? session) =>
        session is not null && replyAttachments.TryGetValue(session, out var f) ? f : Array.Empty<string>();

    /// <summary>The files for the reply that is being sent now; they leave the park.</summary>
    public IReadOnlyList<string> TakeReplyAttachments(string session)
    {
        if (!replyAttachments.Remove(session, out var f)) return Array.Empty<string>();
        ReplyAttachmentsChanged?.Invoke(session);
        return f;
    }

    public void ClearReplyAttachments(string session)
    {
        if (replyAttachments.Remove(session)) ReplyAttachmentsChanged?.Invoke(session);
    }

    /// <summary>"1 screenshot attached" · "3 screenshots attached" · null.</summary>
    public static string? AttachedText(int n) => n <= 0 ? null : n == 1 ? "1 screenshot attached" : $"{n} screenshots attached";
}
