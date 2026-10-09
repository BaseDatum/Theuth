using Theuth.Companion.Core;
using static Theuth.Companion.AnyApp.AnyAppNative;

namespace Theuth.Companion.AnyApp;

/// <summary>The user's clipboard as the lease took it (§13.3 step 1): every copyable format's bytes, in clipboard order.</summary>
internal sealed record ClipSnapshot(IReadOnlyList<(uint Id, string Name, byte[] Data)> Items, string? Text, int Skipped)
{
    public static ClipSnapshot Empty { get; } = new(Array.Empty<(uint, string, byte[])>(), null, 0);

    public long Bytes => Items.Sum(i => (long)i.Data.Length);

    public string Describe() => $"{Items.Count} format(s), {Bytes} bytes{(Skipped > 0 ? $", {Skipped} skipped" : "")}{(Text is { } t ? $", text {t.Length} chars" : "")}";
}

/// <summary>
/// 153 §13.3 / §13.9 on Win32 — the lease's clipboard steps. Runs on its own worker thread (a delay-rendering owner that hangs in
/// <c>GetClipboardData</c> must not hang the UI), the hidden <see cref="SessionWindow"/> as the clipboard owner. Our item is ONE write:
/// CF_UNICODETEXT + <c>ExcludeClipboardContentFromMonitorProcessing</c> + <c>CanIncludeInClipboardHistory</c> = 0 +
/// <c>CanUploadToCloudClipboard</c> = 0 (Win+V history and cloud sync skip it) + the registered <c>theuth.dictation-session</c> format
/// holding the lease's marker, which is how the settle knows the clipboard is still ours.
/// </summary>
internal sealed class ClipboardIo
{
    private readonly nint owner;
    private readonly ComWorker worker = new("clipboard");
    private readonly uint sessionFormat = RegisterClipboardFormatW(ClipboardLease.SessionFormat);
    private readonly uint excludeFormat = RegisterClipboardFormatW(ClipboardLease.ExcludeFromMonitors);
    private readonly uint historyFormat = RegisterClipboardFormatW(ClipboardLease.CanIncludeInHistory);
    private readonly uint cloudFormat = RegisterClipboardFormatW(ClipboardLease.CanUploadToCloud);

    public ClipboardIo(nint owner) => this.owner = owner;

    public Task<ClipSnapshot?> SnapshotAsync() => worker.Run(Snapshot, 1500, (ClipSnapshot?)null, "snapshot");

    public Task<bool> WriteAsync(string text, string marker) => worker.Run(() => Write(text, marker), 800, false, "write");

    public Task<string?> MarkerAsync() => worker.Run(Marker, 500, (string?)null, "marker");

    public Task<bool> RestoreAsync(ClipSnapshot s) => worker.Run(() => Restore(s), 1500, false, "restore");

    /// <summary>The card's Copy (§13.5): the shaped text alone, no markers — an ordinary copy the user owns.</summary>
    public Task<bool> CopyPlainAsync(string text) => worker.Run(() => Put(new[] { (CfUnicodeText, Utf16z(text)) }), 800, false, "copy");

    /// <summary>CF_UNICODETEXT now (the probes' before/after proof), or null.</summary>
    public Task<string?> TextAsync() => worker.Run(() =>
    {
        if (!Open()) return null;
        try { return GetClipboardData(CfUnicodeText) is var h && h != 0 && Bytes(h) is { } b ? FromUtf16z(b) : null; }
        finally { CloseClipboard(); }
    }, 800, (string?)null, "text");

    /// <summary>OpenClipboard with a short retry: another app may hold it for a few milliseconds.</summary>
    private bool Open()
    {
        for (int i = 0; i < 10; i++)
        {
            if (OpenClipboard(owner)) return true;
            Thread.Sleep(15);
        }
        Log.Write($"anyapp: OpenClipboard failed (err {System.Runtime.InteropServices.Marshal.GetLastWin32Error()})");
        return false;
    }

    private ClipSnapshot? Snapshot()
    {
        if (!Open()) return null;
        try
        {
            var seen = new List<ClipFormat>();
            for (uint f = EnumClipboardFormats(0); f != 0; f = EnumClipboardFormats(f))
            {
                string name = Name(f);
                long size = ClipboardSnapshotPolicy.IsHandleFormat(f) || ClipboardSnapshotPolicy.IsOurs(name) ? 0 : SizeOf(f);
                seen.Add(new ClipFormat(f, name, size));
            }
            var plan = ClipboardSnapshotPolicy.Plan(seen);
            var items = new List<(uint, string, byte[])>();
            string? text = null;
            foreach (var id in plan)
            {
                nint h = GetClipboardData(id);
                if (Bytes(h) is not { } data) continue;
                items.Add((id, seen.First(s => s.Id == id).Name, data));
                if (id == CfUnicodeText) text = FromUtf16z(data);
            }
            return new ClipSnapshot(items, text, seen.Count - items.Count);
        }
        finally { CloseClipboard(); }
    }

    private static long SizeOf(uint f)
    {
        nint h = GetClipboardData(f);
        return h == 0 ? 0 : (long)GlobalSize(h);
    }

    private bool Write(string text, string marker)
    {
        var zero = BitConverter.GetBytes(0u);
        return Put(new[]
        {
            (CfUnicodeText, Utf16z(text)),
            (excludeFormat, zero),
            (historyFormat, zero),
            (cloudFormat, zero),
            (sessionFormat, Utf16z(marker)),
        });
    }

    private string? Marker()
    {
        if (!IsClipboardFormatAvailable(sessionFormat)) return null;
        if (!Open()) return null;
        try { return GetClipboardData(sessionFormat) is var h && h != 0 && Bytes(h) is { } b ? FromUtf16z(b) : null; }
        finally { CloseClipboard(); }
    }

    private bool Restore(ClipSnapshot s) => Put(s.Items.Select(i => (i.Id >= 0xC000 && i.Name.Length > 0 ? RegisterClipboardFormatW(i.Name) : i.Id, i.Data)).ToArray());

    /// <summary>EmptyClipboard, then each format; the clipboard owns every HGLOBAL SetClipboardData accepted, we free the rest.</summary>
    private bool Put(IReadOnlyList<(uint Id, byte[] Data)> formats)
    {
        if (!Open()) return false;
        try
        {
            if (!EmptyClipboard()) return false;
            bool ok = true;
            foreach (var (id, data) in formats)
            {
                if (id == 0) { ok = false; continue; }
                nint mem = Global(data);
                if (mem == 0) { ok = false; continue; }
                if (SetClipboardData(id, mem) == 0)
                {
                    GlobalFree(mem);
                    ok = false;
                }
            }
            return ok;
        }
        finally { CloseClipboard(); }
    }

    private static readonly Dictionary<uint, string> Standard = new()
    {
        [1] = "CF_TEXT", [2] = "CF_BITMAP", [3] = "CF_METAFILEPICT", [4] = "CF_SYLK", [5] = "CF_DIF", [6] = "CF_TIFF", [7] = "CF_OEMTEXT",
        [8] = "CF_DIB", [9] = "CF_PALETTE", [10] = "CF_PENDATA", [11] = "CF_RIFF", [12] = "CF_WAVE", [13] = "CF_UNICODETEXT",
        [14] = "CF_ENHMETAFILE", [15] = "CF_HDROP", [16] = "CF_LOCALE", [17] = "CF_DIBV5",
    };

    private static string Name(uint f)
    {
        if (Standard.TryGetValue(f, out var s)) return s;
        var b = new char[256];
        int n = GetClipboardFormatNameW(f, b, b.Length);
        return n > 0 ? new string(b, 0, n) : $"0x{f:X4}";
    }
}

/// <summary>
/// The any-app hidden window (never shown): <c>WTSRegisterSessionNotification</c> target for §13.8's lock refusal and the lease's clipboard
/// owner. The lock is never tested by locking build-B: <see cref="Synthesize"/> posts a <c>WM_WTSSESSION_CHANGE</c> to this window, which
/// takes exactly the path a real WTS_SESSION_LOCK takes.
/// </summary>
internal sealed class SessionWindow : IDisposable
{
    private const string ClassName = "TheuthCompanionAnyApp";
    private static WndProc? proc; // the delegate the class points at, alive for the process
    private static SessionWindow? current;
    private bool registered;

    public SessionWindow()
    {
        current = this;
        nint instance = GetModuleHandleW(null);
        if (proc is null)
        {
            proc = Proc;
            var cls = new WndClassEx
            {
                Size = (uint)System.Runtime.InteropServices.Marshal.SizeOf<WndClassEx>(),
                WndProc = System.Runtime.InteropServices.Marshal.GetFunctionPointerForDelegate(proc),
                Instance = instance,
                ClassName = ClassName,
            };
            if (RegisterClassExW(ref cls) == 0) Log.Write($"anyapp: RegisterClassEx failed (err {System.Runtime.InteropServices.Marshal.GetLastWin32Error()})");
        }
        // WS_POPUP, WS_EX_TOOLWINDOW: a real (not message-only) top-level window, so session notifications reach it; never shown.
        Handle = CreateWindowExW(0x00000080, ClassName, "theuth any-app", 0x80000000, 0, 0, 0, 0, 0, 0, instance, 0);
        if (Handle == 0)
        {
            Log.Write($"anyapp: hidden window failed (err {System.Runtime.InteropServices.Marshal.GetLastWin32Error()}) — no lock notifications, clipboard owner = none");
            return;
        }
        registered = RegisterSessionNotification(Handle);
        Log.Write($"anyapp: hidden window 0x{Handle:X} · WTSRegisterSessionNotification={(registered ? "ok" : $"FAILED err {System.Runtime.InteropServices.Marshal.GetLastWin32Error()}")}");
    }

    public nint Handle { get; }

    /// <summary>A <c>WM_WTSSESSION_CHANGE</c> wParam arrived (on the UI thread).</summary>
    public event Action<int, bool>? SessionChange;

    /// <summary>Post a synthetic <c>WM_WTSSESSION_CHANGE</c> (the probe's lock proof): the same message path as the real notification.</summary>
    public bool Synthesize(int wtsCode) => Handle != 0 && PostMessageW(Handle, WmWtsSessionChange, wtsCode, 0x5357 /* "SW" marks it synthetic in the log */);

    private static nint Proc(nint hwnd, uint msg, nint w, nint l)
    {
        if (msg == WmWtsSessionChange && current is { } me && hwnd == me.Handle)
        {
            try { me.SessionChange?.Invoke((int)w, l == 0x5357); }
            catch (Exception e) { Log.Write($"anyapp: session change handler failed: {e.GetType().Name}: {e.Message}"); }
            return 0;
        }
        return DefWindowProcW(hwnd, msg, w, l);
    }

    public void Dispose()
    {
        if (Handle == 0) return;
        if (registered) WTSUnRegisterSessionNotification(Handle);
        DestroyWindow(Handle);
    }
}
