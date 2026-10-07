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
/// </summary>
internal sealed class Hotkeys : IDisposable
{
    private const int FirstId = 0x0134;
    private const int ProbeId = 0x7FFF;
    private readonly HostWindow host;
    private readonly DispatcherQueue dispatcher;
    private readonly Dictionary<int, (KeyAction Action, Chord Chord)> registered = new();
    private DispatcherQueueTimer? releaseTimer;

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

    /// <summary>"Ctrl+Shift+Space is held by another app — …"; null when every global row registered as saved.</summary>
    public string? Notice { get; private set; }

    public IReadOnlyDictionary<KeyAction, Chord> Registered => registered.Values.ToDictionary(v => v.Action, v => v.Chord);

    /// <summary>(Re-)registers the table's global rows; W3 calls it again when Settings › Keys changes.</summary>
    public void Apply(KeyTable table)
    {
        UnregisterAll();
        var notices = new List<string>();
        int id = FirstId;
        foreach (var b in table.BindingsIn(KeyScope.Anywhere))
        {
            var r = KeyTable.ResolveGlobal(b.Chord, b.DefaultChord, CanRegister);
            if (r.Notice is { } n)
            {
                notices.Add(n);
                Log.Write($"hotkey: {n}");
            }
            if (r.Chord is not { } c)
            {
                if (b.Chord is null) Log.Write($"hotkey {b.Action.RawValue()}: unassigned");
                continue;
            }
            if (Register(id, c))
            {
                registered[id] = (b.Action, c);
                Log.Write($"hotkey {b.Action.RawValue()} = {c.Symbol} registered (RegisterHotKey, MOD_NOREPEAT) id=0x{id:X}");
            }
            else
            {
                Log.Write($"hotkey {b.Action.RawValue()} = {c.Symbol}: RegisterHotKey failed {Win32.LastError()}");
            }
            id++;
        }
        Notice = notices.Count == 0 ? null : string.Join(" · ", notices);
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
        releaseTimer?.Stop();
        UnregisterAll();
        host.Hotkey -= OnHotkey;
    }
}
