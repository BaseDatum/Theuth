using Theuth.Companion.Interop;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.WindowsAndMessaging;

namespace Theuth.Companion.Ui;

/// <summary>
/// The tray's right-click menu (design 134 §4.5, D9): Open theuth · Ask theuth… · Pause notifications ▸ · Launch at login ✓ ·
/// Keyboard shortcuts… Ctrl+/ · Settings… · Quit theuth · Quit theuth Companion, as a Win32 popup menu (<c>TrackPopupMenuEx</c> on the host window,
/// <c>TPM_RETURNCMD</c>, the documented SetForegroundWindow / WM_NULL pair around it). Deviation: §4.5 names a WinUI
/// <c>MenuFlyout</c>; hosted in a 1 × 1 tool window it terminated the process on build-B (no managed exception), and the
/// native menu is what the shell's other tray icons use (keyboard, Narrator and dismissal for free).
/// </summary>
internal sealed class TrayMenu
{
    private const uint TpmRightButton = 0x0002, TpmRightAlign = 0x0008, TpmBottomAlign = 0x0020, TpmReturnCmd = 0x0100;
    private readonly HostWindow host;

    public TrayMenu(HostWindow host) => this.host = host;

    public sealed record Item(string Text, Action? Act = null, bool Enabled = true, bool Checked = false, IReadOnlyList<Item>? Sub = null)
    {
        public static Item Separator { get; } = new("-");
        public bool IsSeparator => Text == "-";
    }

    /// <summary>Shows the menu at <paramref name="at"/> (screen px) and runs the chosen item's action after the menu closed.</summary>
    public void Show(IReadOnlyList<Item> items, System.Drawing.Point at)
    {
        var actions = new Dictionary<uint, (string Text, Action Act)>();
        uint next = 1;
        var menus = new List<HMENU>();
        var root = Build(items, actions, menus, ref next);
        PInvoke.SetForegroundWindow(host.Handle); // else the menu does not close on an outside click
        uint cmd;
        unsafe
        {
            var r = PInvoke.TrackPopupMenuEx(root,
                TpmReturnCmd | TpmRightButton | TpmBottomAlign | TpmRightAlign,
                at.X, at.Y, host.Handle, null);
            cmd = (uint)r.Value;
        }
        PInvoke.PostMessage(host.Handle, PInvoke.WM_NULL, default, default);
        PInvoke.DestroyMenu(root); // destroys the submenus with it
        if (cmd != 0 && actions.TryGetValue(cmd, out var hit))
        {
            Log.Write($"tray menu: {hit.Text}");
            hit.Act();
        }
        else
        {
            Log.Write("tray menu dismissed");
        }
    }

    private static unsafe HMENU Build(IReadOnlyList<Item> items, Dictionary<uint, (string, Action)> actions, List<HMENU> menus, ref uint next)
    {
        var menu = PInvoke.CreatePopupMenu();
        menus.Add(menu);
        foreach (var it in items)
        {
            if (it.IsSeparator)
            {
                PInvoke.AppendMenu(menu, MENU_ITEM_FLAGS.MF_SEPARATOR, 0, default(PCWSTR));
                continue;
            }
            var flags = MENU_ITEM_FLAGS.MF_STRING;
            if (!it.Enabled) flags |= MENU_ITEM_FLAGS.MF_GRAYED;
            if (it.Checked) flags |= MENU_ITEM_FLAGS.MF_CHECKED;
            nuint id;
            if (it.Sub is { } sub)
            {
                var child = Build(sub, actions, menus, ref next);
                flags |= MENU_ITEM_FLAGS.MF_POPUP;
                id = (nuint)(nint)child.Value;
            }
            else
            {
                id = next++;
                if (it.Act is { } act) actions[(uint)id] = (it.Text, act);
            }
            fixed (char* text = it.Text) PInvoke.AppendMenu(menu, flags, id, text);
        }
        return menu;
    }
}
