using static Theuth.Companion.AnyApp.AnyAppNative;

namespace Theuth.Companion.AnyApp;

/// <summary>
/// 153 §13.2 / §13.3 step 3 / §7.4 — keystrokes into the target. Every burst is ONE <c>SendInput</c> call, so the user's own key events
/// (the auto-repeat of a held chord) can never land in the middle of it. A burst releases the modifiers the user is physically holding
/// (Ctrl/Alt of Ctrl+Alt+D during a hold), does its work, then presses them again so the held chord stays the hotkey's (its repeats are
/// eaten, never typed as Ctrl+D); an unassigned mask key goes after a held Alt/Win so their eventual release opens no menu
/// (AutoHotkey's menu-mask technique). Direct insertion is <c>KEYEVENTF_UNICODE</c>, chunked; the lease's paste is Ctrl+V.
/// </summary>
internal static class Typist
{
    /// <summary>Characters per burst for direct insertion (§13.2 "chunked").</summary>
    public const int ChunkChars = 64;

    private static readonly (ushort Vk, bool Extended)[] Modifiers =
    {
        (VkLControl, false), (VkRControl, true), (VkLMenu, false), (VkRMenu, true), (VkLShift, false), (VkRShift, false),
        (VkLWin, true), (VkRWin, true),
    };

    /// <summary>Is any modifier physically down (§7.4: no Return while one is)?</summary>
    public static bool ModifierDown => Modifiers.Any(m => IsDown(m.Vk));

    /// <summary>§13.2: type <paramref name="text"/> with KEYEVENTF_UNICODE (UTF-16 code units, so surrogate pairs go as two). True when
    /// every event was inserted (UIPI-blocked targets are not reported by SendInput — the assessment marks those elevated).</summary>
    public static bool Type(string text)
    {
        bool ok = true;
        for (int i = 0; i < text.Length; i += ChunkChars)
        {
            var chunk = text.Substring(i, Math.Min(ChunkChars, text.Length - i));
            var events = new List<Input>();
            foreach (char c in chunk)
            {
                events.Add(Key(0, c, KeyEventUnicode));
                events.Add(Key(0, c, KeyEventUnicode | KeyEventKeyUp));
            }
            ok &= Burst(events, keepCtrl: false);
        }
        return ok;
    }

    /// <summary>§13.3 step 3: Ctrl+V (a held Ctrl stays down; Alt/Shift/Win released around it).</summary>
    public static bool Paste() => Burst(new List<Input> { Key(VkV, 0, 0), Key(VkV, 0, KeyEventKeyUp) }, keepCtrl: true);

    /// <summary>§7.4 "send it": Return (the caller checked <see cref="ModifierDown"/> and the assessment first).</summary>
    public static bool Return() => Burst(new List<Input> { Key(VkReturn, 0, 0), Key(VkReturn, 0, KeyEventKeyUp) }, keepCtrl: false);

    /// <summary>§7.4 "undo last": Ctrl+Z once into the target.</summary>
    public static bool Undo() => Burst(new List<Input> { Key(VkZ, 0, 0), Key(VkZ, 0, KeyEventKeyUp) }, keepCtrl: true);

    /// <summary>
    /// One atomic SendInput: [mask if Alt/Win held] [held modifiers up] [Ctrl down if the work needs it and it is not held] work
    /// [Ctrl up if we pressed it] [held modifiers down again] [mask if Alt/Win held].
    /// </summary>
    private static bool Burst(List<Input> work, bool keepCtrl)
    {
        var held = Modifiers.Where(m => IsDown(m.Vk)).ToArray();
        bool ctrlHeld = held.Any(m => IsCtrl(m.Vk));
        bool altOrWin = held.Any(m => m.Vk is VkLMenu or VkRMenu or VkLWin or VkRWin);
        var release = held.Where(m => !(keepCtrl && IsCtrl(m.Vk))).ToArray(); // Ctrl+V / Ctrl+Z keep a held Ctrl down
        var events = new List<Input>();
        if (altOrWin) Mask(events);
        foreach (var m in release) events.Add(Key(m.Vk, 0, KeyEventKeyUp | (m.Extended ? KeyEventExtendedKey : 0)));
        bool pressCtrl = keepCtrl && !ctrlHeld;
        if (pressCtrl) events.Add(Key(VkLControl, 0, 0));
        events.AddRange(work);
        if (pressCtrl) events.Add(Key(VkLControl, 0, KeyEventKeyUp));
        foreach (var m in release) events.Add(Key(m.Vk, 0, m.Extended ? KeyEventExtendedKey : 0));
        if (altOrWin) Mask(events);
        var array = events.ToArray();
        uint sent = SendInput((uint)array.Length, array, System.Runtime.InteropServices.Marshal.SizeOf<Input>());
        if (sent != array.Length) Log.Write($"anyapp: SendInput inserted {sent}/{array.Length} events (err {System.Runtime.InteropServices.Marshal.GetLastWin32Error()})");
        return sent == array.Length;
    }

    private static bool IsCtrl(ushort vk) => vk == VkLControl || vk == VkRControl;

    private static void Mask(List<Input> events)
    {
        events.Add(Key(VkMask, 0, 0));
        events.Add(Key(VkMask, 0, KeyEventKeyUp));
    }

    private static Input Key(ushort vk, ushort scan, uint flags) => new()
    {
        Type = InputKeyboard,
        U = new InputUnion { Ki = new KeyboardInput { Vk = vk, Scan = scan, Flags = flags, Time = 0, ExtraInfo = InjectedTag } },
    };
}
