using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Windows.Win32;
using Windows.Win32.UI.Input.KeyboardAndMouse;

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
/// </summary>
internal sealed class Hotkeys : IDisposable
{
    private const int FirstId = 0x0134;
    private const int ProbeId = 0x7FFF;
    private readonly HostWindow host;
    private readonly DispatcherQueue dispatcher;
    private readonly Dictionary<int, (KeyAction Action, Chord Chord)> registered = new();
    private readonly Dictionary<KeyAction, int> idOf = new();
    private DispatcherQueueTimer? releaseTimer;
    private HotkeyRetry? retry;
    private DispatcherQueueTimer? retryTimer;

    public Hotkeys(HostWindow host, DispatcherQueue dispatcher)
    {
        this.host = host;
        this.dispatcher = dispatcher;
        host.Hotkey += OnHotkey;
    }

    /// <summary>A global chord went down (once per press: MOD_NOREPEAT).</summary>
    public event Action<KeyAction>? Pressed;

    /// <summary>The chord's key came up, with how long it was held (hold-to-talk, W4).</summary>
    public event Action<KeyAction, TimeSpan>? Released;

    /// <summary><see cref="Notice"/> changed after <see cref="Apply"/> returned (a retry registered a row or gave up).</summary>
    public event Action? NoticeChanged;

    /// <summary>"Ctrl+Shift+Space is held by another app — …"; null when every global row registered as saved.</summary>
    public string? Notice { get; private set; }

    public IReadOnlyDictionary<KeyAction, Chord> Registered => registered.Values.ToDictionary(v => v.Action, v => v.Chord);

    /// <summary>149 WW1: rows this answers false for are skipped by <see cref="Apply"/> (the meeting PTT / eye rows only while a meeting is live, §8); null = every row.</summary>
    public Func<KeyAction, bool>? Gate { get; set; }

    /// <summary>The table last applied, for a re-<see cref="Apply"/> when the gate's answer changes.</summary>
    public KeyTable? Applied { get; private set; }

    /// <summary>The running startup retry (tests and the probe log); null when every row registered or the window closed.</summary>
    public HotkeyRetry? Retry => retry is { Active: true } r ? r : null;

    /// <summary>(Re-)registers the table's global rows; W3 calls it again when Settings › Keys changes. A running retry stops first.</summary>
    public void Apply(KeyTable table)
    {
        CancelRetry();
        UnregisterAll();
        Applied = table;
        var notices = new List<string>();
        var failures = new List<HotkeyFailure>();
        int id = FirstId;
        foreach (var b in table.BindingsIn(KeyScope.Anywhere))
        {
            int rowId = id++; // one id per row, so a retry can take the row's slot later
            if (Gate is { } gate && !gate(b.Action))
            {
                Log.Write($"hotkey {b.Action.RawValue()}: not registered now (gated)");
                continue;
            }
            idOf[b.Action] = rowId;
            var r = KeyTable.ResolveGlobal(b.Chord, b.DefaultChord, CanRegister);
            if (r.Notice is { } n)
            {
                notices.Add(n);
                failures.Add(new HotkeyFailure(b.Action, b.Chord!, n));
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
        if (failures.Count > 0)
        {
            retry = new HotkeyRetry(failures, DateTimeOffset.Now);
            Notice = retry.Notice;
            Log.Write($"hotkey: retrying {string.Join(", ", failures.Select(f => $"{f.Action.RawValue()}={f.Chord.Symbol}"))} every {HotkeyRetry.Interval.TotalSeconds:0} s for up to {HotkeyRetry.Window.TotalSeconds:0} s (the shell lets go on its next poll)");
            ScheduleRetry();
        }
        else
        {
            Notice = notices.Count == 0 ? null : string.Join(" · ", notices);
        }
    }

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
        var notice = r.Notice;
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

    private void WatchRelease(KeyAction action, Chord chord)
    {
        releaseTimer?.Stop();
        var started = DateTimeOffset.Now;
        int vk = chord.KeyCode & 0xFF;
        var t = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromMilliseconds(50);
        t.IsRepeating = true;
        t.Tick += (_, _) =>
        {
            if ((PInvoke.GetAsyncKeyState(vk) & 0x8000) != 0) return;
            t.Stop();
            if (releaseTimer == t) releaseTimer = null;
            Released?.Invoke(action, DateTimeOffset.Now - started);
        };
        releaseTimer = t;
        t.Start();
    }

    private void UnregisterAll()
    {
        foreach (var id in registered.Keys) PInvoke.UnregisterHotKey(host.Handle, id);
        registered.Clear();
    }

    public void Dispose()
    {
        CancelRetry();
        releaseTimer?.Stop();
        UnregisterAll();
        host.Hotkey -= OnHotkey;
    }
}
