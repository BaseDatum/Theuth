using Theuth.Companion.Core;

namespace Theuth.Companion;

/// <summary>
/// W3's members of the Store (design 134 §5.4, as-built W1 §7.1): the toast and settings seams. Everything here runs on
/// the UI thread, like the rest of the Store.
/// </summary>
public sealed partial class Store
{
    /// <summary>
    /// A toast body or its Open button was tapped (D8): open the Attention panel on <c>itemId</c> (null = the oldest
    /// needs-you item, e.g. a batch or flood summary). W2's panel subscribes; the Notifier raises it through
    /// <see cref="RequestOpen"/> after focusing the item in <c>Panel</c>.
    /// </summary>
    public event Action<string?>? OpenItemRequested;

    /// <summary>Focus <paramref name="itemId"/> in the panel model (when it is there) and ask the surfaces to open on it.</summary>
    public void RequestOpen(string? itemId)
    {
        if (itemId is not null) FocusOn(itemId);
        OpenItemRequested?.Invoke(itemId);
    }

    /// <summary>Settings › Undo delay: the Store reads <see cref="StoreModel.UndoDelay"/> at every hold (0–5 s).</summary>
    public void ApplySettings(CompanionSettings settings) => UndoDelay = Math.Min(5, Math.Max(0, settings.UndoDelay));
}
