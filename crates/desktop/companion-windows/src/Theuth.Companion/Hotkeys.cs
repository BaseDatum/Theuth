using System.Runtime.InteropServices;
using Microsoft.UI.Dispatching;
using Theuth.Companion.Breakpoints;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.Input.KeyboardAndMouse;
using Windows.Win32.UI.Shell;

namespace Theuth.Companion;

/// <summary>
/// The <c>anywhere</c> scope (design 134 D6, §4.2): every global row of the key table registered with <c>RegisterHotKey</c>
/// on the host window, <c>MOD_NOREPEAT</c> added (the chord's modifiers are Win32's <c>MOD_*</c> already, W1). A chord another
/// app holds falls back exactly as <see cref="KeyTable.ResolveGlobal"/> does — the saved chord, else the default with a
/// notice, else none (never a dialog); the probe is a register/unregister pair. <c>WM_HOTKEY</c> is the key-down, which
/// carries the foreground grant the panel needs (B6); the release is observed by polling the key every 50 ms
/// (<see cref="Released"/>, for W4's hold-to-talk).
/// <para>149 recheck D1: a row the shell still holds at our start (it re-binds the voice key when the companion exits and
/// lets go on its next 2 s poll) is retried by <see cref="HotkeyRetry"/> — every 1 s for up to 20 s; the notice says
/// "retrying" meanwhile, clears when the row registers, and stands when the window closes on a genuine holder.</para>
/// <para>153 wave C (§15, W-d): the two new rows Dictate anywhere Ctrl+Alt+D and Command Ctrl+Alt+C. Every exact Ctrl+Alt chord on a
/// typing key is probed on the active layout with <c>ToUnicodeEx(vk, keystate Ctrl+Alt)</c> before it registers
/// (<see cref="AltGrRule"/>): a glyph → the row registers as Ctrl+Shift+Alt+&lt;key&gt; and says "AltGr on this layout — using …"
/// in Settings › Keys and the log; the probe re-runs on <c>WM_INPUTLANGCHANGE</c> and when the foreground's layout changes (a 2 s
/// look, because the hidden host window rarely gets that message). Each held key has its own release watch, so with two mode keys
/// down (a mode switch, §10.1) the release of either ends the hold. §12.6: a family row may hold a modifier-only chord (Right Ctrl…)
/// when Settings › Keys "Modifier-only keys" is on — watched by a <c>WH_KEYBOARD_LL</c> hook; another key during its hold is not a
/// release (<see cref="Chorded"/>).</para>
/// <para>PIN-C: <see cref="DictateAnywhere"/> / <see cref="CommandKey"/> report each mode key's gesture after the one
/// <see cref="PointMachine"/> (raised by <c>PointController</c>, which runs it); W153D's AnyApp controller subscribes to
/// <see cref="DictateAnywhere"/> and answers <see cref="TakeDictateWords"/> on a switch out of dictation.</para>
/// </summary>
internal sealed class Hotkeys : IDisposable
{
    private const int FirstId = 0x0134;
    private const int ProbeId = 0x7FFF;
    private const uint WmInputLangChange = 0x0051;
    private const nuint HostSubclassId = 0x153C;
    private readonly HostWindow host;
    private readonly DispatcherQueue dispatcher;
    private readonly Dictionary<int, (KeyAction Action, Chord Chord)> registered = new();
    private readonly Dictionary<KeyAction, int> idOf = new();
    private readonly Dictionary<KeyAction, DispatcherQueueTimer> releaseTimers = new();
    private readonly Dictionary<KeyAction, AltGrDecision> altGr = new();
    private readonly Dictionary<ushort, KeyAction> modifierOnly = new();
    private readonly SUBCLASSPROC hostSubclass; // referenced for the host's lifetime
    private readonly DispatcherQueueTimer layoutTimer;
    private HotkeyRetry? retry;
    private DispatcherQueueTimer? retryTimer;
    private string? retryNotice;
    private nint probedLayout;
    private nint llHook;
    private W3Native.LowLevelKeyboardProc? llProc;
    private ushort llDown;
    private bool llChorded;
    private DateTimeOffset llStarted;
    private bool disposed;

    public Hotkeys(HostWindow host, DispatcherQueue dispatcher)
    {
        this.host = host;
        this.dispatcher = dispatcher;
        host.Hotkey += OnHotkey;
        hostSubclass = HostProc;
        if (!PInvoke.SetWindowSubclass(host.Handle, hostSubclass, HostSubclassId, 0)) Log.Write($"hotkey: SetWindowSubclass(host) failed {Win32.LastError()} — WM_INPUTLANGCHANGE unseen (the layout look still runs)");
        layoutTimer = dispatcher.CreateTimer();
        layoutTimer.Interval = TimeSpan.FromSeconds(2);
        layoutTimer.IsRepeating = true;
        layoutTimer.Tick += (_, _) => CheckLayout("foreground layout");
        layoutTimer.Start();
        Current = this;
    }

    /// <summary>The running instance (Settings › Keys reads the AltGr lines from it).</summary>
    public static Hotkeys? Current { get; private set; }

    /// <summary>A global chord went down (once per press: MOD_NOREPEAT; a modifier-only chord once per hold).</summary>
    public event Action<KeyAction>? Pressed;

    /// <summary>The chord's key came up, with how long it was held (hold-to-talk, W4).</summary>
    public event Action<KeyAction, TimeSpan>? Released;

    /// <summary>153 §12.6: a modifier-only chord's hold saw another key (Ctrl+C with Right Ctrl as the key) — not a release: the gesture is cancelled.</summary>
    public event Action<KeyAction>? Chorded;

    /// <summary><see cref="Notice"/> changed after <see cref="Apply"/> returned (a retry registered a row or gave up, or the layout's AltGr answer changed).</summary>
    public event Action? NoticeChanged;

    /// <summary>
    /// PIN-C — the Dictate anywhere key's gesture after the PointMachine: <see cref="ModeGesture.Started"/> (start your capture),
    /// <see cref="ModeGesture.Locked"/>, <see cref="ModeGesture.Stopped"/> (deliver), <see cref="ModeGesture.Discarded"/> (drop),
    /// <see cref="ModeGesture.Switched"/> (into or out of dictation mid-gesture), <see cref="ModeGesture.Refused"/>. Raised on the UI thread.
    /// </summary>
    public event Action<ModeGesture>? DictateAnywhere;

    /// <summary>PIN-C — the Command key's gesture after the PointMachine (the companion's own command card consumes it; observers may log).</summary>
    public event Action<ModeGesture>? CommandKey;

    /// <summary>
    /// PIN-C — set by the AnyApp controller: when a live dictation gesture switches to ask/command (§10.1), the companion calls this with
    /// the gesture id; the controller stops its capture without delivering and returns the words heard so far (the companion continues
    /// the recording under the new mode with them in front). Null or "" = nothing heard.
    /// </summary>
    public Func<int, string>? TakeDictateWords { get; set; }

    /// <summary>Whether an AnyApp controller listens to <see cref="DictateAnywhere"/> (without one the key says so and does nothing).</summary>
    public bool HasDictateAnywhereSubscriber => DictateAnywhere is not null;

    /// <summary>"Ctrl+Shift+Space is held by another app — …"; null when every global row registered as saved.</summary>
    public string? Notice { get; private set; }

    public IReadOnlyDictionary<KeyAction, Chord> Registered => registered.Values.ToDictionary(v => v.Action, v => v.Chord);

    /// <summary>153: the rows registered as their AltGr fallback on the current layout, with the Keys-tab line.</summary>
    public IReadOnlyDictionary<KeyAction, string> AltGrNotices => altGr.Where(kv => kv.Value.Notice is not null).ToDictionary(kv => kv.Key, kv => kv.Value.Notice!);

    /// <summary>The chord a row is registered as right now (after the AltGr rule and the held-by-another-app fallback); null = none.</summary>
    public Chord? EffectiveChord(KeyAction action)
    {
        foreach (var (a, c) in registered.Values) if (a == action) return c;
        foreach (var (vk, a) in modifierOnly) if (a == action) return new Chord("", vk);
        return null;
    }

    /// <summary>149 WW1: rows this answers false for are skipped by <see cref="Apply"/> (the meeting PTT / eye rows only while a meeting is live, §8); null = every row.</summary>
    public Func<KeyAction, bool>? Gate { get; set; }

    /// <summary>153 §14.3: the second gate the point-and-say controller sets (<c>dictateAnywhereEnabled</c>, <c>commandKeyEnabled</c>); null = every row.</summary>
    public Func<KeyAction, bool>? ModeGate { get; set; }

    /// <summary>153 §12.6: Settings › Keys "Modifier-only keys" — only then are modifier-only rows watched (re-<see cref="Apply"/> after a change).</summary>
    public bool ModifierOnlyEnabled { get; set; }

    /// <summary>The table last applied, for a re-<see cref="Apply"/> when the gate's answer changes.</summary>
    public KeyTable? Applied { get; private set; }

    /// <summary>The running startup retry (tests and the probe log); null when every row registered or the window closed.</summary>
    public HotkeyRetry? Retry => retry is { Active: true } r ? r : null;

    /// <summary>A mode key is held right now (a re-registration waits until it comes up).</summary>
    public bool AnyHeld => releaseTimers.Count > 0 || llDown != 0;

    /// <summary>(Re-)registers the table's global rows; W3 calls it again when Settings › Keys changes. A running retry stops first.</summary>
    public void Apply(KeyTable table)
    {
        CancelRetry();
        UnregisterAll();
        Applied = table;
        altGr.Clear();
        modifierOnly.Clear();
        var layout = ForegroundLayout();
        probedLayout = layout;
        var notices = new List<string>();
        var failures = new List<HotkeyFailure>();
        int id = FirstId;
        foreach (var b in table.BindingsIn(KeyScope.Anywhere))
        {
            int rowId = id++; // one id per row, so a retry can take the row's slot later
            if ((Gate is { } gate && !gate(b.Action)) || (ModeGate is { } mg && !mg(b.Action)))
            {
                Log.Write($"hotkey {b.Action.RawValue()}: not registered now (gated)");
                continue;
            }
            idOf[b.Action] = rowId;
            if (b.Chord is { IsModifierOnly: true } lone)
            {
                if (ModifierOnlyEnabled)
                {
                    modifierOnly[lone.KeyCode] = b.Action;
                    Log.Write($"hotkey {b.Action.RawValue()} = {lone.Symbol} watched (WH_KEYBOARD_LL, modifier-only)");
                }
                else
                {
                    Log.Write($"hotkey {b.Action.RawValue()} = {lone.Symbol}: not registered (Settings › Keys › Modifier-only keys is off)");
                }
                continue;
            }
            var saved = b.Chord is { } sc ? AltGrFor(b.Action, sc, layout) : null;
            var def = b.DefaultChord is { } dc ? AltGrRule.Decide(dc, AltGrRule.Applies(dc) ? ProbeAltGr(dc, layout) : null).Register : null;
            var r = KeyTable.ResolveGlobal(saved, def, CanRegister);
            if (r.Notice is { } n)
            {
                notices.Add(n);
                failures.Add(new HotkeyFailure(b.Action, saved!, n));
                Log.Write($"hotkey: {n}");
            }
            if (r.Chord is not { } c)
            {
                if (b.Chord is null) Log.Write($"hotkey {b.Action.RawValue()}: unassigned");
                continue;
            }
            if (Register(rowId, c))
            {
                registered[rowId] = (b.Action, c);
                Log.Write($"hotkey {b.Action.RawValue()} = {c.Symbol} registered (RegisterHotKey, MOD_NOREPEAT) id=0x{rowId:X}");
            }
            else
            {
                Log.Write($"hotkey {b.Action.RawValue()} = {c.Symbol}: RegisterHotKey failed {Win32.LastError()}");
            }
        }
        UpdateHook();
        if (failures.Count > 0)
        {
            retry = new HotkeyRetry(failures, DateTimeOffset.Now);
            retryNotice = retry.Notice;
            Log.Write($"hotkey: retrying {string.Join(", ", failures.Select(f => $"{f.Action.RawValue()}={f.Chord.Symbol}"))} every {HotkeyRetry.Interval.TotalSeconds:0} s for up to {HotkeyRetry.Window.TotalSeconds:0} s (the shell lets go on its next poll)");
            ScheduleRetry();
        }
        else
        {
            retryNotice = notices.Count == 0 ? null : string.Join(" · ", notices);
        }
        Notice = ComposeNotice();
    }

    /// <summary>The held-by-another-app line(s), then one line per AltGr fallback ("Dictate anywhere: AltGr on this layout — using Ctrl+Shift+Alt+D").</summary>
    private string? ComposeNotice()
    {
        var parts = new List<string>();
        if (retryNotice is { } rn) parts.Add(rn);
        foreach (var (a, d) in altGr)
        {
            if (d.Notice is { } n) parts.Add($"{Short(a)}: {n}");
        }
        return parts.Count == 0 ? null : string.Join(" · ", parts);
    }

    private static string Short(KeyAction a) => a switch
    {
        KeyAction.DictateAnywhere => "Dictate anywhere",
        KeyAction.Command => "Command",
        KeyAction.VoiceKey => "Voice key",
        _ => a.RawValue(),
    };

    // MARK: 153 AltGr (§15, W-d)

    /// <summary>The chord a row registers on <paramref name="layout"/>: itself, or its Ctrl+Shift+Alt fallback when AltGr types a glyph there.</summary>
    private Chord AltGrFor(KeyAction action, Chord chord, nint layout)
    {
        if (!AltGrRule.Applies(chord)) return chord;
        var probe = ProbeAltGr(chord, layout);
        var d = AltGrRule.Decide(chord, probe);
        altGr[action] = d;
        if (d.Substituted)
        {
            Log.Write($"hotkey {action.RawValue()}: AltGr on this layout — using {AltGrRule.FallbackText(chord)} ({chord.Symbol} types '{probe}' on layout {Hkl(layout)})");
        }
        else
        {
            Log.Write($"hotkey {action.RawValue()}: AltGr probe {chord.Symbol} on layout {Hkl(layout)} → no glyph, registering as it is");
        }
        return d.Register;
    }

    /// <summary>
    /// <c>ToUnicodeEx(vk, scan, keystate Ctrl+Alt, flags 0x4 = keep the keyboard state)</c> on <paramref name="layout"/>: the characters
    /// Ctrl+Alt+key types there (AltGr layouts answer from their AltGr column), the dead key's character, or null.
    /// </summary>
    public static string? ProbeAltGr(Chord chord, nint layout)
    {
        try
        {
            var state = new byte[256];
            state[0x11] = 0x80; // VK_CONTROL
            state[0x12] = 0x80; // VK_MENU
            state[0xA2] = 0x80; // VK_LCONTROL
            state[0xA5] = 0x80; // VK_RMENU (AltGr = LCtrl + RAlt)
            var buf = new char[8];
            uint vk = (uint)(chord.KeyCode & 0xFF);
            uint scan = W3Native.MapVirtualKeyW(vk, 0);
            int n = W3Native.ToUnicodeEx(vk, scan, state, buf, buf.Length, 0x4, layout);
            if (n == 0) return null;
            return new string(buf, 0, Math.Clamp(Math.Abs(n), 1, buf.Length)); // n < 0: a dead key — its character is in the buffer
        }
        catch (Exception e)
        {
            Log.Write($"hotkey: AltGr probe failed {e.GetType().Name}: {e.Message}");
            return null;
        }
    }

    /// <summary>An HKL as Windows writes it: the low 32 bits ("0xF0010409" for US-International), not the sign-extended handle.</summary>
    private static string Hkl(nint layout) => $"0x{(ulong)layout & 0xFFFFFFFF:X8}";

    /// <summary>The keyboard layout the user is typing with: the foreground window's thread's, else ours.</summary>
    public static nint ForegroundLayout()
    {
        var fg = PInvoke.GetForegroundWindow();
        if (!fg.IsNull)
        {
            uint tid = PInvoke.GetWindowThreadProcessId(fg, out _);
            if (tid != 0 && W3Native.GetKeyboardLayout(tid) is var hkl && hkl != 0) return hkl;
        }
        return W3Native.GetKeyboardLayout(0);
    }

    /// <summary>The layout changed (WM_INPUTLANGCHANGE or the 2 s look): re-register only when an AltGr answer differs.</summary>
    private void CheckLayout(string why)
    {
        if (disposed || Applied is not { } table) return;
        var layout = ForegroundLayout();
        if (layout == probedLayout || layout == 0) return;
        if (AnyHeld) return; // never pull a chord out from under a held key; the next look retries
        var old = probedLayout;
        probedLayout = layout;
        bool differs = false;
        foreach (var b in table.BindingsIn(KeyScope.Anywhere))
        {
            if (b.Chord is not { } c || !AltGrRule.Applies(c) || !idOf.ContainsKey(b.Action)) continue;
            var d = AltGrRule.Decide(c, ProbeAltGr(c, layout));
            bool had = altGr.TryGetValue(b.Action, out var prev) && prev.Substituted;
            if (d.Substituted != had) differs = true;
        }
        if (!differs)
        {
            Log.Write($"hotkey: keyboard layout {Hkl(old)} → {Hkl(layout)} ({why}) — AltGr answers unchanged");
            return;
        }
        Log.Write($"hotkey: keyboard layout {Hkl(old)} → {Hkl(layout)} ({why}) — re-probing AltGr and re-registering");
        var before = Notice;
        Apply(table);
        if (Notice != before) NoticeChanged?.Invoke();
    }

    private LRESULT HostProc(HWND h, uint msg, WPARAM w, LPARAM l, nuint id, nuint data)
    {
        if (msg == WmInputLangChange)
        {
            try { dispatcher.TryEnqueue(() => CheckLayout("WM_INPUTLANGCHANGE")); }
            catch (Exception) { }
        }
        return PInvoke.DefSubclassProc(h, msg, w, l);
    }

    // MARK: PIN-C gesture events

    /// <summary>The controller that runs the PointMachine reports a mode key's gesture here; the event of its mode is raised.</summary>
    internal void RaiseGesture(ModeGesture g)
    {
        var target = g switch
        {
            ModeGesture.Switched s when s.From == PointMode.Dictate || s.To == PointMode.Dictate => DictateAnywhere,
            ModeGesture.Switched s when s.From == PointMode.Command || s.To == PointMode.Command => CommandKey,
            _ when g.Mode == PointMode.Dictate => DictateAnywhere,
            _ when g.Mode == PointMode.Command => CommandKey,
            _ => null,
        };
        if (target is null) return;
        try { target(g); }
        catch (Exception e) { Log.Write($"hotkey: a {g.Mode.RawValue()} gesture subscriber failed: {e.GetType().Name}: {e.Message}"); }
    }

    /// <summary>The words a dictation gesture heard, from the AnyApp controller (a switch out of dictation, §10.1).</summary>
    internal string TakeWords(int gestureId)
    {
        try { return TakeDictateWords?.Invoke(gestureId) ?? ""; }
        catch (Exception e)
        {
            Log.Write($"hotkey: TakeDictateWords failed: {e.GetType().Name}: {e.Message}");
            return "";
        }
    }

    // MARK: retry (149 recheck D1)

    private void ScheduleRetry()
    {
        if (retry?.NextDelay(DateTimeOffset.Now) is not { } delay)
        {
            retryTimer = null;
            return;
        }
        var t = dispatcher.CreateTimer();
        t.Interval = delay;
        t.IsRepeating = false;
        t.Tick += (_, _) =>
        {
            t.Stop();
            if (retryTimer != t) return;
            retryTimer = null;
            RetryTick();
        };
        retryTimer = t;
        t.Start();
    }

    private void RetryTick()
    {
        if (retry is not { } r) return;
        var now = DateTimeOffset.Now;
        var won = r.Attempt(now, TryAgain);
        foreach (var f in won)
        {
            Log.Write($"hotkey {f.Action.RawValue()} = {f.Chord.Symbol} registered after retry {r.Attempts} ({(now - r.StartedAt).TotalSeconds:0.0} s after apply) id=0x{idOf[f.Action]:X}");
        }
        if (r.GaveUp)
        {
            Log.Write($"hotkey: gave up after {r.Attempts} retries — {r.Notice}");
        }
        retryNotice = r.Notice;
        var notice = ComposeNotice();
        if (notice != Notice)
        {
            Notice = notice;
            NoticeChanged?.Invoke();
        }
        if (r.Active) ScheduleRetry();
        else retryTimer = null;
    }

    /// <summary>One retry of a failed row: the saved chord into the row's id slot, replacing a fallback default registered there.</summary>
    private bool TryAgain(HotkeyFailure f)
    {
        if (!idOf.TryGetValue(f.Action, out int id)) return false;
        if (registered.TryGetValue(id, out var fallback))
        {
            if (!CanRegister(f.Chord)) return false;
            PInvoke.UnregisterHotKey(host.Handle, id);
            registered.Remove(id);
            if (Register(id, f.Chord))
            {
                registered[id] = (f.Action, f.Chord);
                return true;
            }
            if (Register(id, fallback.Chord)) registered[id] = fallback; // keep the default we had
            return false;
        }
        if (!Register(id, f.Chord)) return false;
        registered[id] = (f.Action, f.Chord);
        return true;
    }

    private void CancelRetry()
    {
        retryTimer?.Stop();
        retryTimer = null;
        retry?.Cancel();
        retry = null;
        idOf.Clear();
    }

    /// <summary>The registrar probe: another app's registration makes <c>RegisterHotKey</c> answer 0.</summary>
    private bool CanRegister(Chord c)
    {
        if (!Register(ProbeId, c)) return false;
        PInvoke.UnregisterHotKey(host.Handle, ProbeId);
        return true;
    }

    private bool Register(int id, Chord c) =>
        PInvoke.RegisterHotKey(host.Handle, id, (HOT_KEY_MODIFIERS)((uint)c.Modifiers) | HOT_KEY_MODIFIERS.MOD_NOREPEAT, (uint)(c.KeyCode & 0xFF));

    private void OnHotkey(int id)
    {
        if (!registered.TryGetValue(id, out var hit)) return;
        Log.Write($"WM_HOTKEY {hit.Chord.Symbol} ({hit.Action.RawValue()}) foreground={Win32.Describe(PInvoke.GetForegroundWindow())}");
        Pressed?.Invoke(hit.Action);
        WatchRelease(hit.Action, hit.Chord);
    }

    /// <summary>Each held key has its own 50 ms watch (153: two mode keys down during a switch — the release of either ends the hold).</summary>
    private void WatchRelease(KeyAction action, Chord chord)
    {
        if (releaseTimers.Remove(action, out var old)) old.Stop();
        var started = DateTimeOffset.Now;
        int vk = chord.KeyCode & 0xFF;
        var t = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromMilliseconds(50);
        t.IsRepeating = true;
        t.Tick += (_, _) =>
        {
            if ((PInvoke.GetAsyncKeyState(vk) & 0x8000) != 0) return;
            t.Stop();
            if (releaseTimers.TryGetValue(action, out var cur) && cur == t) releaseTimers.Remove(action);
            Released?.Invoke(action, DateTimeOffset.Now - started);
        };
        releaseTimers[action] = t;
        t.Start();
    }

    // MARK: 153 §12.6 modifier-only chords (WH_KEYBOARD_LL)

    private void UpdateHook()
    {
        bool want = modifierOnly.Count > 0;
        if (want && llHook == 0)
        {
            llProc = LowLevelProc;
            llHook = W3Native.SetWindowsHookExW(W3Native.WH_KEYBOARD_LL, llProc, W3Native.GetModuleHandleW(null), 0);
            Log.Write(llHook != 0
                ? $"hotkey: WH_KEYBOARD_LL installed for {string.Join(", ", modifierOnly.Select(kv => $"{kv.Value.RawValue()}={KeyNames.Name(kv.Key, "")}"))} (key codes only)"
                : $"hotkey: WH_KEYBOARD_LL failed {Marshal.GetLastWin32Error()} — modifier-only keys do nothing");
        }
        else if (!want && llHook != 0)
        {
            W3Native.UnhookWindowsHookEx(llHook);
            llHook = 0;
            llProc = null;
            llDown = 0;
            Log.Write("hotkey: WH_KEYBOARD_LL removed (no modifier-only rows)");
        }
    }

    private nint LowLevelProc(int nCode, nuint wParam, nint lParam)
    {
        if (nCode >= 0)
        {
            try
            {
                var k = Marshal.PtrToStructure<W3Native.KBDLLHOOKSTRUCT>(lParam);
                ushort vk = (ushort)k.vkCode;
                uint msg = (uint)wParam;
                bool down = msg is W3Native.WM_KEYDOWN or W3Native.WM_SYSKEYDOWN;
                bool up = msg is W3Native.WM_KEYUP or W3Native.WM_SYSKEYUP;
                if (down)
                {
                    if (llDown == 0 && modifierOnly.TryGetValue(vk, out var action))
                    {
                        llDown = vk;
                        llChorded = false;
                        llStarted = DateTimeOffset.Now;
                        dispatcher.TryEnqueue(() =>
                        {
                            Log.Write($"modifier-only {KeyNames.Name(vk, "")} down ({action.RawValue()})");
                            Pressed?.Invoke(action);
                        });
                    }
                    else if (llDown != 0 && vk != llDown && !llChorded && modifierOnly.TryGetValue(llDown, out var held))
                    {
                        llChorded = true; // chord protection (A:L172): this hold is a shortcut, not the key
                        ushort heldVk = llDown;
                        dispatcher.TryEnqueue(() =>
                        {
                            Log.Write($"modifier-only {KeyNames.Name(heldVk, "")}: another key during the hold — not a release ({held.RawValue()} cancelled)");
                            Chorded?.Invoke(held);
                        });
                    }
                }
                else if (up && llDown != 0 && vk == llDown && modifierOnly.TryGetValue(vk, out var released))
                {
                    bool chorded = llChorded;
                    var heldFor = DateTimeOffset.Now - llStarted;
                    llDown = 0;
                    if (!chorded) dispatcher.TryEnqueue(() => Released?.Invoke(released, heldFor));
                }
            }
            catch (Exception)
            {
                // never let the hook throw into the system's input path
            }
        }
        return W3Native.CallNextHookEx(llHook, nCode, wParam, lParam);
    }

    private void UnregisterAll()
    {
        foreach (var id in registered.Keys) PInvoke.UnregisterHotKey(host.Handle, id);
        registered.Clear();
    }

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        CancelRetry();
        layoutTimer.Stop();
        foreach (var t in releaseTimers.Values) t.Stop();
        releaseTimers.Clear();
        UnregisterAll();
        if (llHook != 0) W3Native.UnhookWindowsHookEx(llHook);
        llHook = 0;
        PInvoke.RemoveWindowSubclass(host.Handle, hostSubclass, HostSubclassId);
        host.Hotkey -= OnHotkey;
        if (Current == this) Current = null;
    }
}
