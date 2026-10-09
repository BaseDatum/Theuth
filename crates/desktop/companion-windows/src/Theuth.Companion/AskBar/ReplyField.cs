using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Theuth.Companion.Core;
using Windows.System;

namespace Theuth.Companion.AskBar;

/// <summary>
/// R on a card — reply in words (design 134 §4.2 / 130 R1-5; Swift <c>ReplyField</c>): one line under the card while
/// <c>Store.Ask.ReplyFor == card.Item.Id</c>; Enter sends through the kind's own path after the same 3 s hold
/// (<see cref="StoreModel.SubmitReply"/>: ask → text answer, decision → <c>task_decide</c>, approval → deny + steer, review →
/// send back with the note, finished → next step), Esc cancels, empty is refused. W2's panel places it in the focused card.
/// </summary>
internal sealed partial class ReplyField : StackPanel
{
    private readonly TextBox box;

    public ReplyField(Store store, Card card)
    {
        Spacing = 4;
        box = new TextBox { PlaceholderText = card.ReplyLabel(), FontSize = 14, IsSpellCheckEnabled = true };
        AutomationProperties.SetAutomationId(box, "reply-field");
        AutomationProperties.SetName(box, card.ReplyLabel());
        box.KeyDown += (_, e) =>
        {
            if (e.Key == VirtualKey.Enter)
            {
                store.SubmitReply(box.Text); // empty is refused by the Store
                box.Text = "";
                e.Handled = true;
            }
            else if (e.Key == VirtualKey.Escape)
            {
                store.Perform(Intent.Escape);
                e.Handled = true;
            }
        };
        Children.Add(box);
        // 149 NATIVE: key names as words, caption size, the theme's secondary text (no opacity standing in for a colour).
        Children.Add(new TextBlock { Text = "Enter sends · Esc cancels · dictation stays the Ask bar's (hold Ctrl+Shift+Space there)", FontSize = 12, Foreground = Ui.Theme.Brush(NativeLook.TextSecondary, ActualTheme) });
        Loaded += (_, _) => box.Focus(FocusState.Programmatic);
    }

    public string Text => box.Text;
}