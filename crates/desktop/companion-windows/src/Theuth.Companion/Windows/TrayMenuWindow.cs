using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Theuth.Companion.Interop;
using Windows.Graphics;
using Windows.Win32;
using Windows.Win32.Foundation;

namespace Theuth.Companion.Ui;

/// <summary>
/// The tray's right-click menu (design 134 §4.5, D9) as a WinUI <see cref="MenuFlyout"/>: a 1 × 1 px tool window at the
/// pointer hosts it (a flyout needs a XAML root; <c>ShouldConstrainToRootBounds = false</c> lets it draw outside), is
/// activated so the flyout light-dismisses on an outside click, and hides when the flyout closes. A dismissed menu gives the
/// foreground back only if it still holds it (a click elsewhere chose another window).
/// </summary>
internal sealed partial class TrayMenuWindow : ToolWindow
{
    private readonly Grid root;
    private HWND previous;
    private MenuFlyout? open;

    public TrayMenuWindow()
        : base("theuth — menu", default, excludeFromCapture: !Flags.Shareable)
    {
        root = new Grid { Background = Palette.Transparent };
        Content = root;
    }

    /// <summary>Set by a menu item before it acts: the window it opens keeps the foreground.</summary>
    public bool ActionTaken { get; set; }

    public void Show(MenuFlyout menu, System.Drawing.Point at, HWND prev)
    {
        previous = prev;
        ActionTaken = false;
        open?.Hide();
        open = menu;
        menu.ShouldConstrainToRootBounds = false;
        menu.Closed += (_, _) =>
        {
            if (open != menu) return;
            open = null;
            bool held = IsForeground;
            HideWindow();
            // An item's Click can follow Closed: decide after it ran.
            DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, () =>
            {
                if (held && !ActionTaken) Win32.Foreground(previous);
                ActionTaken = false;
            });
        };
        Place(new RectInt32(at.X, at.Y, 1, 1));
        ShowActive();
        DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Normal, () =>
        {
            try
            {
                menu.ShowAt(root, new FlyoutShowOptions
                {
                    Position = new global::Windows.Foundation.Point(0, 0),
                    Placement = FlyoutPlacementMode.TopEdgeAlignedRight,
                    ShowMode = FlyoutShowMode.Standard,
                });
                Log.Write($"tray menu open at ({at.X},{at.Y}) with {menu.Items.Count} items");
            }
            catch (Exception e)
            {
                Log.Write($"tray menu: ShowAt failed: {e.GetType().Name}: {e.Message}");
                HideWindow();
            }
        });
    }

    protected override void OnActivate(bool active, HWND other)
    {
        if (!active && open is { } m) m.Hide();
    }
}
