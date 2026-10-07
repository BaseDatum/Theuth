using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>Ordering, focus and the header sentence of the Attention panel (130 §4.2).</summary>
public sealed class PanelModel
{
    public IReadOnlyList<AttentionItem> Items { get; private set; }
    public int Focus { get; private set; }
    public int WorkingCount { get; set; }

    /// <summary>W3: the Done zone's rows follow the items; J/K walk into them, <see cref="Focused"/> is null there.</summary>
    public int ReceiptCount { get; set; }

    public PanelModel(IEnumerable<AttentionItem> items, int workingCount = 0)
    {
        Items = Order(items);
        Focus = 0;
        WorkingCount = workingCount;
    }

    /// <summary>Needs-you oldest first, then finished oldest first; receipts (<c>show</c>, unknown kinds) last.</summary>
    public static IReadOnlyList<AttentionItem> Order(IEnumerable<AttentionItem> items)
    {
        static int Group(AttentionItem i) => i.Kind.NeedsYou ? 0 : (i.Kind == AttentionKind.Finished ? 1 : 2);
        return items.OrderBy(Group).ThenBy(i => i.WaitingSince).ThenBy(i => i.Id, StringComparer.Ordinal).ToList();
    }

    public AttentionItem? Focused => Focus >= 0 && Focus < Items.Count ? Items[Focus] : null;

    /// <summary>Index into the Done zone when the focus has walked past the items.</summary>
    public int? ReceiptFocus => Focus >= Items.Count && Focus < Items.Count + ReceiptCount ? Focus - Items.Count : null;

    /// <summary>Every other item as a line, in panel order (all of them while a receipt is focused).</summary>
    public IReadOnlyList<AttentionItem> Others => Items.Where((_, i) => i != Focus).ToList();

    public int NeedsYouCount => Items.Count(i => i.Kind.NeedsYou);
    public int DoneCount => Items.Count(i => i.Kind == AttentionKind.Finished);

    /// <summary>J/K · ↓/↑: clamped, never wraps.</summary>
    public void Move(int delta)
    {
        int total = Items.Count + ReceiptCount;
        if (total <= 0) { Focus = 0; return; }
        Focus = Math.Min(total - 1, Math.Max(0, Focus + delta));
    }

    /// <summary>Focus the item with this id, if it is listed (Swift <c>focus(id:)</c>).</summary>
    public void FocusOn(string id)
    {
        for (int i = 0; i < Items.Count; i++)
        {
            if (Items[i].Id == id) { Focus = i; return; }
        }
    }

    /// <summary>A fresh list keeps the focused item when it is still there, else clamps.</summary>
    public void Update(IEnumerable<AttentionItem> items)
    {
        var keep = Focused?.Id;
        Items = Order(items);
        int found = -1;
        if (keep is not null)
        {
            for (int i = 0; i < Items.Count; i++)
            {
                if (Items[i].Id == keep) { found = i; break; }
            }
        }
        Focus = found >= 0 ? found : Math.Min(Math.Max(0, Items.Count + ReceiptCount - 1), Focus);
    }

    /// <summary>"3 need you · 5 working · 2 done"; "Nothing needs you" when nothing does and nothing works.</summary>
    public string Headline
    {
        get
        {
            var parts = new List<string>();
            int n = NeedsYouCount;
            if (n > 0) parts.Add(n == 1 ? "1 needs you" : $"{n} need you");
            if (WorkingCount > 0) parts.Add($"{WorkingCount} working");
            if (DoneCount > 0) parts.Add($"{DoneCount} done");
            if (n == 0 && WorkingCount == 0) parts.Insert(0, "Nothing needs you");
            return string.Join(" · ", parts);
        }
    }

    /// <summary>"1 of 3"; empty when there is nothing to count.</summary>
    public string Position => Items.Count == 0 ? "" : $"{Math.Min(Focus, Items.Count - 1) + 1} of {Items.Count}";
}
