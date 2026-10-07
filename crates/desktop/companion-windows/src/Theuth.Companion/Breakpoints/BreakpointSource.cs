using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;

namespace Theuth.Companion.Breakpoints;

/// <summary>
/// Feeds <see cref="BreakpointDetector"/> (130 §5.5, R1-3; design 134 §5.4, W3): <c>SetWinEventHook(EVENT_SYSTEM_FOREGROUND,
/// WINEVENT_OUTOFCONTEXT)</c> on the UI thread — the hook is delivered through this thread's message loop, so no
/// marshalling — plus <c>GetLastInputInfo</c> sampled by a 1 Hz <c>DispatcherQueueTimer</c>. The same tick drives the
/// policy's 2-minute cap (<see cref="Tick"/>). The companion's own exe is the detector's <c>own</c>: its panel taking
/// the foreground is not an app switch. Permission-free.
/// </summary>
internal sealed class BreakpointSource : IDisposable
{
    public event Action? Breakpoint;
    public event Action<DateTimeOffset>? Tick;

    private readonly DispatcherQueue dispatcher;
    private readonly BreakpointDetector detector = new(Environment.ProcessPath?.ToLowerInvariant());
    private readonly W3Native.WinEventProc callback; // kept alive for the hook's lifetime
    private nint hook;
    private DispatcherQueueTimer? timer;
    private int switches;
    private int idleBreaks;

    public BreakpointSource(DispatcherQueue dispatcher)
    {
        this.dispatcher = dispatcher;
        callback = OnWinEvent;
    }

    /// <summary>Foreground switches seen · idle breakpoints fired (for the probe lines).</summary>
    public (int Switches, int IdleBreaks) Counts => (switches, idleBreaks);

    public void Start()
    {
        if (!dispatcher.HasThreadAccess) throw new InvalidOperationException("BreakpointSource.Start: UI thread only");
        hook = W3Native.SetWinEventHook(W3Native.EVENT_SYSTEM_FOREGROUND, W3Native.EVENT_SYSTEM_FOREGROUND, 0, callback, 0, 0, W3Native.WINEVENT_OUTOFCONTEXT);
        Log.Write(hook != 0
            ? $"breakpoints: WinEvent hook on EVENT_SYSTEM_FOREGROUND installed (own={Environment.ProcessPath})"
            : $"breakpoints: SetWinEventHook FAILED err={System.Runtime.InteropServices.Marshal.GetLastWin32Error()} — app switches will not be seen");
        timer = dispatcher.CreateTimer();
        timer.Interval = TimeSpan.FromSeconds(1);
        timer.IsRepeating = true;
        timer.Tick += (_, _) =>
        {
            double idle = W3Native.IdleSeconds();
            if (detector.Feed(new BreakpointDetector.Signal.Idle(idle)))
            {
                idleBreaks++;
                Log.Write($"breakpoint: idle {idle:0} s");
                Breakpoint?.Invoke();
            }
            Tick?.Invoke(DateTimeOffset.Now);
        };
        timer.Start();
    }

    private void OnWinEvent(nint hWinEventHook, uint @event, nint hwnd, int idObject, int idChild, uint idEventThread, uint dwmsEventTime)
    {
        if (@event != W3Native.EVENT_SYSTEM_FOREGROUND || hwnd == 0) return;
        var id = W3Native.ProcessIdentity(hwnd);
        if (id.Length == 0) return;
        if (detector.Feed(new BreakpointDetector.Signal.AppActivated(id)))
        {
            switches++;
            Log.Write($"breakpoint: app switch → {Path.GetFileName(id)}");
            Breakpoint?.Invoke();
        }
    }

    public void Dispose()
    {
        timer?.Stop();
        timer = null;
        if (hook != 0)
        {
            W3Native.UnhookWinEvent(hook);
            hook = 0;
        }
    }
}

/// <summary>
/// Double-tap Alt (design 134 D6, §3, §4.6 — the analogue of ⌥⌥): a <c>WH_KEYBOARD_LL</c> hook feeding spike SB's
/// <see cref="DoubleTapDetector"/> with the modifier state. Installed only while the Settings toggle is on; it sees key
/// <em>codes</em>, never text, and only while the companion runs — no permission dialog exists on Windows, so the opt-in
/// is a privacy choice, not a TCC one. The hook runs on the installing (UI) thread's message loop.
/// </summary>
internal sealed class DoubleTapSource : IDisposable
{
    public event Action? DoubleTap;

    private readonly DoubleTapDetector detector = new();
    private readonly W3Native.LowLevelKeyboardProc proc; // kept alive for the hook's lifetime
    private nint hook;
    private Modifiers held = Modifiers.None;

    public DoubleTapSource() => proc = Hook;

    public bool Installed => hook != 0;

    /// <summary>true when the hook is in; false (logged) when <c>SetWindowsHookEx</c> refused.</summary>
    public bool Install()
    {
        if (hook != 0) return true;
        hook = W3Native.SetWindowsHookExW(W3Native.WH_KEYBOARD_LL, proc, W3Native.GetModuleHandleW(null), 0);
        if (hook == 0)
        {
            Log.Write($"double-tap Alt: SetWindowsHookEx FAILED err={System.Runtime.InteropServices.Marshal.GetLastWin32Error()}");
            return false;
        }
        Log.Write("double-tap Alt: keyboard hook installed (key codes only, while the companion runs)");
        return true;
    }

    public void Uninstall()
    {
        if (hook == 0) return;
        W3Native.UnhookWindowsHookEx(hook);
        hook = 0;
        held = Modifiers.None;
        detector.Cancel();
        Log.Write("double-tap Alt: keyboard hook removed");
    }

    private nint Hook(int nCode, nuint wParam, nint lParam)
    {
        if (nCode >= 0)
        {
            var k = System.Runtime.InteropServices.Marshal.PtrToStructure<W3Native.KBDLLHOOKSTRUCT>(lParam);
            uint message = (uint)wParam;
            bool down = message is W3Native.WM_KEYDOWN or W3Native.WM_SYSKEYDOWN;
            var bit = ModifierOf(k.vkCode);
            if (bit != Modifiers.None)
            {
                var next = down ? held | bit : held & ~bit;
                if (next != held)
                {
                    held = next;
                    if (detector.Feed(held, k.time / 1000.0))
                    {
                        Log.Write("double-tap Alt");
                        DoubleTap?.Invoke();
                    }
                }
            }
            else if (down)
            {
                detector.Cancel(); // any other key between the taps cancels (spike SB)
            }
        }
        return W3Native.CallNextHookEx(hook, nCode, wParam, lParam);
    }

    private static Modifiers ModifierOf(uint vk) => vk switch
    {
        0x10 or 0xA0 or 0xA1 => Modifiers.Shift,   // VK_SHIFT, VK_LSHIFT, VK_RSHIFT
        0x11 or 0xA2 or 0xA3 => Modifiers.Control, // VK_CONTROL, VK_LCONTROL, VK_RCONTROL
        0x12 or 0xA4 or 0xA5 => Modifiers.Alt,     // VK_MENU, VK_LMENU, VK_RMENU
        0x5B or 0x5C => Modifiers.Win,             // VK_LWIN, VK_RWIN
        _ => Modifiers.None,
    };

    public void Dispose() => Uninstall();
}
