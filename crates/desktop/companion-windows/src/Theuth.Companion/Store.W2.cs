using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion;

/// <summary>
/// W2's Store members (design 134 §4.1–4.2, D6). <see cref="Keys"/> is the one key table the panel grammar, the legend and the
/// global hotkeys read — loaded from <c>settings.json</c> at start; W3's Settings assigns a new table (raises <c>Keys</c>) and
/// W4's Ask bar reads its <c>AskBar</c> scope from it. <see cref="HotkeyNotice"/> is the "held by another app" line from
/// <c>KeyTable.ResolveGlobal</c> (the legend and Settings › Keys show it). UI thread only, like the rest of the Store.
/// </summary>
public sealed partial class Store
{
    private KeyTable keys = new();
    private string? hotkeyNotice;

    public KeyTable Keys
    {
        get => keys;
        set
        {
            keys = value;
            OnPropertyChanged();
        }
    }

    public string? HotkeyNotice
    {
        get => hotkeyNotice;
        set
        {
            hotkeyNotice = value;
            OnPropertyChanged();
        }
    }

    /// <summary>The oldest item that needs you (the panel's first): where the hotkey opens the panel (130 §4.2).</summary>
    public AttentionItem? OldestNeedsYou => Panel.Items.FirstOrDefault(i => i.Kind.NeedsYou);

    /// <summary>Focus the oldest needs-you item, if there is one.</summary>
    public void FocusOldest()
    {
        if (OldestNeedsYou is { } item) FocusOn(item.Id);
    }
}
