using Microsoft.UI.Xaml.Input;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Windows.Win32;
using Windows.Win32.Foundation;

namespace Theuth.Companion.Ui;

/// <summary>
/// WinUI <c>KeyDown</c> → the Core's <see cref="KeyPress"/> (W1 as-built §5): <c>KeyCode</c> = the virtual key,
/// <see cref="VirtualKeys.NumpadEnter"/> for the keypad's Enter (extended-key flag), modifiers from the key states, and
/// <c>Chars</c> = the character the key types with Shift (and Caps Lock) but without Ctrl/Alt — AppKit's
/// <c>charactersIgnoringModifiers</c>, so <c>?</c> is Shift+/ and <c>K</c> folds to <c>k</c> as in the Mac table.
/// </summary>
internal static class KeyInput
{
    private const int VkShift = 0x10, VkControl = 0x11, VkMenu = 0x12, VkCapital = 0x14, VkLWin = 0x5B, VkRWin = 0x5C;

    public static KeyPress From(KeyRoutedEventArgs e)
    {
        ushort vk = (ushort)e.Key;
        var mods = ChordModifiers.None;
        if (Win32.IsDown(VkControl)) mods |= ChordModifiers.Control;
        if (Win32.IsDown(VkMenu)) mods |= ChordModifiers.Alt;
        bool shift = Win32.IsDown(VkShift);
        if (shift) mods |= ChordModifiers.Shift;
        if (Win32.IsDown(VkLWin) || Win32.IsDown(VkRWin)) mods |= ChordModifiers.Win;
        string chars = Chars(vk, e.KeyStatus.ScanCode, shift);
        if (vk == VirtualKeys.Return && e.KeyStatus.IsExtendedKey) vk = VirtualKeys.NumpadEnter;
        return new KeyPress(chars, vk, mods);
    }

    private static unsafe string Chars(uint vk, uint scan, bool shift)
    {
        byte* state = stackalloc byte[256];
        new Span<byte>(state, 256).Clear();
        if (shift) state[VkShift] = 0x80;
        if ((PInvoke.GetKeyState(VkCapital) & 1) != 0) state[VkCapital] = 0x01;
        char* buf = stackalloc char[8];
        // wFlags 0x4: leave the keyboard's dead-key state alone (Windows 10 1607+).
        int n = PInvoke.ToUnicodeEx(vk, scan, state, new PWSTR(buf), 8, 0x4, PInvoke.GetKeyboardLayout(0));
        return n > 0 ? new string(buf, 0, n) : "";
    }
}
