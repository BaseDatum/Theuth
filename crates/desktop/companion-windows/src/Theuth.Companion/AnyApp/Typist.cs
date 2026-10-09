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
    private static readonly (ushort Vk, bool Extended)[] Modifiers =
    {
        (VkLControl, false), (VkRControl, true), (VkLMenu, false), (VkRMenu, true), (VkLShift, false), (VkRShift, false),
        (VkLWin, true), (VkRWin, true),
    };

    /// <summary>Is any modifier physically down (§7.4: no Return while one is)?</summary>
    public static bool ModifierDown => Modifiers.Any(m => IsDown(m.Vk));

    /// <summary>The pause between characters of a direct insertion. Measured on build-B: a batch of KEYEVENTF_UNICODE events into Win11
    /// Notepad (a XAML/TSF text surface) keeps the first few characters and turns every later one into the batch's LAST character, because
    /// the surface reads the packet character after the next one was injected — so characters go one per SendInput, paced.</summary>
    public const int CharDelayMs = 15; // 8 ms still garbled a 107-char run in Win11 Notepad (live round 4)

    /// <summary>
    /// §13.2: type <paramref name="text"/> with KEYEVENTF_UNICODE, one character (a surrogate pair stays together) per SendInput, paced by
    /// <see cref="CharDelayMs"/>. Only called while no modifier is physically down (held keys take the lease: a paced sequence cannot stay
    /// atomic against a held chord's auto-repeat). False when SendInput refused an event (UIPI-blocked targets are not reported — the
    /// assessment marks those elevated).
    /// </summary>
    public static async Task<bool> TypeAsync(string text)
    {
        bool ok = true;
        for (int i = 0; i < text.Length; i++)
        {
            int n = char.IsHighSurrogate(text[i]) && i + 1 < text.Length && char.IsLowSurrogate(text[i + 1]) ? 2 : 1;
            var events = new Input[n * 2];
            for (int k = 0; k < n; k++)
            {
                events[k * 2] = Key(0, text[i + k], KeyEventUnicode);
                events[k * 2 + 1] = Key(0, text[i + k], KeyEventUnicode | KeyEventKeyUp);
            }
            i += n - 1;
            uint sent = SendInput((uint)events.Length, events, System.Runtime.InteropServices.Marshal.SizeOf<Input>());
            if (sent != events.Length) { ok = false; Log.Write($"anyapp: SendInput inserted {sent}/{events.Length} events at char {i}"); break; }
            await Task.Delay(CharDelayMs);
        }
        return ok;
    }

    /// <summary>§13.3 step 3: Ctrl+V (a held Ctrl stays down; Alt/Shift/Win released around it).</summary>
    public static bool Paste() => Burst(new List<Input> { Key(VkV, 0, 0), Key(VkV, 0, KeyEventKeyUp) }, keepCtrl: true);

    /// <summary>§7.4 "send it": Return (the caller checked <see cref="ModifierDown"/> and the assessment first).</summary>
    public static bool Return() => Burst(new List<Input> { Key(VkReturn, 0, 0), Key(VkReturn, 0, KeyEventKeyUp) }, keepCtrl: false);

    /// <summary>"scratch that" (H3-D2): Backspace × <paramref name="n"/> in one burst (n = 1 deletes a selection).</summary>
    public static bool Backspace(int n)
    {
        var work = new List<Input>();
        for (int i = 0; i < Math.Clamp(n, 1, 4096); i++) { work.Add(Key(VkBack, 0, 0)); work.Add(Key(VkBack, 0, KeyEventKeyUp)); }
        return Burst(work, keepCtrl: false);
    }

    /// <summary>§8.6 media toggle (H3-D2, opt-in): one Play/Pause key press, like the keyboard's media key.</summary>
    public static bool MediaPlayPause()
    {
        var events = new[] { Key(VkMediaPlayPause, 0, KeyEventExtendedKey), Key(VkMediaPlayPause, 0, KeyEventExtendedKey | KeyEventKeyUp) };
        return SendInput((uint)events.Length, events, System.Runtime.InteropServices.Marshal.SizeOf<Input>()) == events.Length;
    }

    private const ushort VkMediaPlayPause = 0xB3;

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