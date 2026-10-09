using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using Theuth.Companion.Core;

namespace Theuth.Companion.AnyApp;

// 153 §13.1 / §13.4 / §13.6 / §13.9: UI Automation for any-app dictation — the focused element at the press (the assessment's facts), the
// field reads of the verification, the selection of the rewrite, the focus restore before Replace / Try again. Hand-declared COM vtables
// (only the slots up to the last one called; placeholders keep the order), one dedicated MTA thread with a wall-clock budget per call so
// a hung target can never stall the companion (the 149 WE1 reader's rule: an overrun abandons the stuck thread).

[ComImport, Guid("30CBE57D-D9D0-452A-AB13-7AC5AC4825EE"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IUIAutomation
{
    [return: MarshalAs(UnmanagedType.Bool)] bool CompareElements(IUIAutomationElement el1, IUIAutomationElement el2);
    void _CompareRuntimeIds();
    IUIAutomationElement GetRootElement();
    IUIAutomationElement ElementFromHandle(nint hwnd);
    void _ElementFromPoint();
    IUIAutomationElement GetFocusedElement();
}

[ComImport, Guid("D22108AA-8AC5-49A5-837B-37BBB3D7591E"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IUIAutomationElement
{
    void SetFocus();
    void _GetRuntimeId();
    void _FindFirst();
    void _FindAll();
    void _FindFirstBuildCache();
    void _FindAllBuildCache();
    void _BuildUpdatedCache();
    [return: MarshalAs(UnmanagedType.Struct)] object? GetCurrentPropertyValue(int propertyId);
    void _GetCurrentPropertyValueEx();
    void _GetCachedPropertyValue();
    void _GetCachedPropertyValueEx();
    void _GetCurrentPatternAs();
    void _GetCachedPatternAs();
    [return: MarshalAs(UnmanagedType.IUnknown)] object? GetCurrentPattern(int patternId);
}

[ComImport, Guid("32EBA289-3583-42C9-9C59-3B6D9A1E9B6A"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IUIAutomationTextPattern
{
    void _RangeFromPoint();
    void _RangeFromChild();
    IUIAutomationTextRangeArray GetSelection();
    void _GetVisibleRanges();
    IUIAutomationTextRange GetDocumentRange();
}

[ComImport, Guid("CE4AE76A-E717-4C98-81EA-47371D028EB6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IUIAutomationTextRangeArray
{
    int GetLength();
    IUIAutomationTextRange GetElement(int index);
}

[ComImport, Guid("A543CC6A-F4AE-494B-8239-C814481187A8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IUIAutomationTextRange
{
    IUIAutomationTextRange Clone();
    void _Compare();
    void _CompareEndpoints();
    void _ExpandToEnclosingUnit();
    void _FindAttribute();
    void _FindText();
    [return: MarshalAs(UnmanagedType.Struct)] object? GetAttributeValue(int attributeId);
    void _GetBoundingRectangles();
    void _GetEnclosingElement();
    [return: MarshalAs(UnmanagedType.BStr)] string GetText(int maxLength);
    void _Move();
    int MoveEndpointByUnit(int endpoint, int unit, int count);
    void MoveEndpointByRange(int sourceEndpoint, IUIAutomationTextRange range, int targetEndpoint);
}

/// <summary>The UIA ids this file reads (<c>UIAutomationClient.h</c>).</summary>
internal static class UiaIds
{
    public const int ProcessId = 30002, ControlType = 30003, LocalizedControlType = 30004, Name = 30005, IsEnabled = 30010,
        ClassName = 30012, IsPassword = 30019, NativeWindowHandle = 30020, IsTextPatternAvailable = 30040,
        IsValuePatternAvailable = 30043, ValueValue = 30045, ValueIsReadOnly = 30046;
    public const int TextPattern = 10014;
    public const int IsReadOnlyAttribute = 40015;
    public const int EndpointStart = 0, EndpointEnd = 1;
    public const int UnitCharacter = 0;
}

/// <summary>One dedicated COM thread (MTA) running work items in order, each under a wall-clock budget. An overrun answers the
/// fallback and retires the thread (it finishes its stuck call and exits); the next call starts a fresh one.</summary>
internal sealed class ComWorker
{
    private readonly string name;
    private BlockingCollection<Action>? queue;
    private int generation;

    public ComWorker(string name) => this.name = name;

    public int Restarts { get; private set; }

    public Task<T> Run<T>(Func<T> work, int budgetMs, T fallback, string what)
    {
        var q = Queue();
        var tcs = new TaskCompletionSource<T>(TaskCreationOptions.RunContinuationsAsynchronously);
        q.Add(() =>
        {
            try { tcs.TrySetResult(work()); }
            catch (Exception e)
            {
                Log.Write($"anyapp: {name} {what} failed: {e.GetType().Name} 0x{e.HResult:X8} {e.Message}");
                tcs.TrySetResult(fallback);
            }
        });
        int g = generation;
        return Bounded(tcs.Task, budgetMs, fallback, what, g);
    }

    private async Task<T> Bounded<T>(Task<T> task, int budgetMs, T fallback, string what, int g)
    {
        var first = await Task.WhenAny(task, Task.Delay(budgetMs)).ConfigureAwait(false);
        if (first == task) return await task.ConfigureAwait(false);
        Log.Write($"anyapp: {name} {what} overran {budgetMs} ms — answering the fallback, retiring the thread");
        lock (this)
        {
            if (g == generation)
            {
                queue?.CompleteAdding();
                queue = null;
                generation++;
                Restarts++;
            }
        }
        return fallback;
    }

    private BlockingCollection<Action> Queue()
    {
        lock (this)
        {
            if (queue is { } q) return q;
            var fresh = new BlockingCollection<Action>();
            var t = new Thread(() =>
            {
                foreach (var a in fresh.GetConsumingEnumerable()) a();
            })
            { IsBackground = true, Name = $"theuth {name}" };
            t.SetApartmentState(ApartmentState.MTA);
            t.Start();
            queue = fresh;
            return fresh;
        }
    }
}

/// <summary>What the press captured: the facts for the assessment and the element itself, for every later read and the focus restore.</summary>
internal sealed record FocusCapture(FocusFacts Facts, IUIAutomationElement? Element, nint TopHwnd, int TopPid, string Exe, int ReadMs)
{
    public static FocusCapture None(string why) => new(new FocusFacts { Readable = false, App = why }, null, 0, 0, "", 0);
}

/// <summary>The UIA reads of §13.1 / §13.4 / §13.6, all on the UIA worker.</summary>
internal sealed class FocusReader
{
    private const int CaptureBudgetMs = 400; // §3.2 wants ≤ 150 ms; the first call after start pays the client's warm-up, so the hard stop is wider
    private const int ReadBudgetMs = 600;
    private static readonly HashSet<string> TheuthExes = new(StringComparer.OrdinalIgnoreCase) { "Theuth.Companion", "theuth-desktop", "theuth", "t" };
    private readonly ComWorker worker = new("uia");
    private IUIAutomation? uia;
    private readonly bool weAreElevated = AnyAppNative.IsElevated(Environment.ProcessId);

    /// <summary>Creates the UIA client once on the worker (CUIAutomation8, else CUIAutomation) so the first press does not pay for it.</summary>
    public Task WarmAsync() => worker.Run(() => { _ = Client().GetRootElement(); return true; }, 5000, false, "warm-up");

    private IUIAutomation Client()
    {
        if (uia is { } u) return u;
        foreach (var clsid in new[] { "E22AD333-B25F-460C-83D0-0581107395C9", "FF48DBA4-60EF-4201-AA87-54103EEF594E" })
        {
            try
            {
                if (Type.GetTypeFromCLSID(new Guid(clsid)) is { } t && Activator.CreateInstance(t) is IUIAutomation made)
                {
                    uia = made;
                    return made;
                }
            }
            catch (COMException e) { Log.Write($"anyapp: CoCreate {clsid} failed 0x{e.HResult:X8}"); }
        }
        throw new InvalidOperationException("UI Automation unavailable");
    }

    /// <summary>The focused element now (§13.1), with the foreground window's process for the app name and our-own check (WE1's owner rule).</summary>
    public async Task<FocusCapture> CaptureAsync()
    {
        nint top = AnyAppNative.GetForegroundWindow();
        int topPid = AnyAppNative.ProcessOf(top);
        string path = AnyAppNative.ImagePath(topPid);
        string exe = Path.GetFileName(path);
        bool elevated = !weAreElevated && topPid > 0 && AnyAppNative.IsElevated(topPid);
        bool ours = topPid == Environment.ProcessId || TheuthExes.Contains(Path.GetFileNameWithoutExtension(exe));
        var started = DateTimeOffset.Now;
        var capture = await worker.Run(() => Read(top, topPid, exe, path, elevated, ours), CaptureBudgetMs,
            FocusCapture.None(AppName(path, exe)) with { TopHwnd = top, TopPid = topPid, Exe = exe }, "capture").ConfigureAwait(false);
        int ms = (int)(DateTimeOffset.Now - started).TotalMilliseconds;
        var facts = capture.Facts;
        if (!facts.Readable) facts = facts with { Elevated = elevated, Ours = ours, ProcessId = topPid, Exe = exe, App = AppName(path, exe) };
        return capture with { Facts = facts, ReadMs = ms };
    }

    private FocusCapture Read(nint top, int topPid, string exe, string path, bool elevated, bool ours)
    {
        string app = AppName(path, exe);
        IUIAutomationElement? el;
        try { el = Client().GetFocusedElement(); }
        catch (COMException) { el = null; }
        if (el is null) return new(new FocusFacts { Readable = false, App = app, Exe = exe, ProcessId = topPid, Elevated = elevated, Ours = ours }, null, top, topPid, exe, 0);
        int ct = Int(el, UiaIds.ControlType);
        int elPid = Int(el, UiaIds.ProcessId);
        string? localized = Str(el, UiaIds.LocalizedControlType);
        bool password = Bool(el, UiaIds.IsPassword) == true;
        bool enabled = Bool(el, UiaIds.IsEnabled) != false;
        bool hasValue = Bool(el, UiaIds.IsValuePatternAvailable) == true;
        bool? valueRo = hasValue ? Bool(el, UiaIds.ValueIsReadOnly) : null;
        bool hasText = Bool(el, UiaIds.IsTextPatternAvailable) == true;
        bool? textRo = null;
        int? selected = null;
        bool readBack = false;
        if (hasText && Text(el) is { } tp)
        {
            var sel = FirstSelection(tp);
            var probe = sel ?? TryRef(() => tp.GetDocumentRange());
            if (probe is not null && TryObj(() => probe.GetAttributeValue(UiaIds.IsReadOnlyAttribute)) is bool ro) textRo = ro;
            if (sel is not null && TryRef(() => sel.GetText(DeliveryAssessment.MaxGestureChars + 1)) is { } st) selected = st.Length;
            readBack = TryRef(() => tp.GetDocumentRange().GetText(16)) is not null;
        }
        if (!readBack && hasValue && !password) readBack = Str(el, UiaIds.ValueValue) is not null;
        if (elPid == Environment.ProcessId) ours = true;
        if (Path.GetFileNameWithoutExtension(exe).Equals("ApplicationFrameHost", StringComparison.OrdinalIgnoreCase) && elPid > 0)
        {
            // UWP apps: the frame host owns the top-level window, the element's process is the app.
            var p = AnyAppNative.ImagePath(elPid);
            app = AppName(p, Path.GetFileName(p));
            exe = Path.GetFileName(p);
        }
        var facts = new FocusFacts
        {
            ControlType = ct, LocalizedControlType = localized, IsPassword = password, IsEnabled = enabled, HasValuePattern = hasValue,
            ValueReadOnly = valueRo, HasTextPattern = hasText, TextReadOnly = textRo, CanReadBack = readBack, SelectionLength = selected,
            Ours = ours, Elevated = elevated, ProcessId = elPid > 0 ? elPid : topPid, App = app, Exe = exe,
        };
        return new(facts, el, top, topPid, exe, 0);
    }

    /// <summary>One verification read (§13.4) of the captured element; focus moved when the focused element is another one.</summary>
    public Task<FieldRead> ReadAsync(FocusCapture c) => worker.Run(() => Field(c), ReadBudgetMs, FieldRead.Unreadable, "read");

    private FieldRead Field(FocusCapture c)
    {
        if (c.Element is not { } el) return FieldRead.Unreadable;
        if (!Same(c)) return FieldRead.FocusMoved;
        string? value = null, before = null, selection = null;
        if (c.Facts.HasTextPattern && Text(el) is { } tp)
        {
            value = TryRef(() => tp.GetDocumentRange().GetText(PasteVerifier.MaxValueChars));
            if (value is { Length: >= PasteVerifier.MaxValueChars }) value = null; // past the cap: the caret window decides (a cut value would hide the paste)
            if (FirstSelection(tp) is { } sel)
            {
                selection = TryRef(() => sel.GetText(4096));
                before = TryRef(() =>
                {
                    var r = sel.Clone();
                    r.MoveEndpointByRange(UiaIds.EndpointEnd, r, UiaIds.EndpointStart); // collapse to the selection's start
                    r.MoveEndpointByUnit(UiaIds.EndpointStart, UiaIds.UnitCharacter, -PasteVerifier.CaretWindowChars);
                    return r.GetText(PasteVerifier.CaretWindowChars);
                });
            }
        }
        int? count = null;
        if (value is null && c.Facts.HasValuePattern && !c.Facts.IsPassword && Str(el, UiaIds.ValueValue) is { } v)
        {
            count = v.Length; // H3-D1 count-only: past the cap the character count still says whether something was added
            value = v.Length > PasteVerifier.MaxValueChars ? null : v;
        }
        return new FieldRead(true, value, before, selection, count);
    }

    /// <summary>Is focus still the captured element (and its window still the foreground one)?</summary>
    public Task<bool> FocusSameAsync(FocusCapture c) => worker.Run(() => Same(c), ReadBudgetMs, false, "focus");

    private bool Same(FocusCapture c)
    {
        if (c.Element is not { } el) return AnyAppNative.GetForegroundWindow() == c.TopHwnd && c.TopHwnd != 0;
        if (AnyAppNative.GetForegroundWindow() != c.TopHwnd) return false;
        try
        {
            var now = Client().GetFocusedElement();
            return now is not null && Client().CompareElements(el, now);
        }
        catch (COMException) { return false; }
    }

    /// <summary>§13.6: the selected text (≤ 20 000 chars), or null.</summary>
    public Task<string?> SelectionAsync(FocusCapture c) => worker.Run(() =>
    {
        if (c.Element is not { } el || Text(el) is not { } tp || FirstSelection(tp) is not { } sel) return null;
        return TryRef(() => sel.GetText(DeliveryAssessment.MaxGestureChars));
    }, ReadBudgetMs, (string?)null, "selection");

    /// <summary>
    /// Replace / Try again (§13.5, §13.6): bring the captured window back to the foreground (our card was just clicked, so this process may
    /// hand the foreground on), then <c>SetFocus</c> the captured element — hiding our window alone races the paste (A:L165).
    /// </summary>
    public async Task<bool> RestoreFocusAsync(FocusCapture c)
    {
        if (c.TopHwnd == 0 || !AnyAppNative.IsWindow(c.TopHwnd)) return false;
        if (AnyAppNative.GetForegroundWindow() != c.TopHwnd) AnyAppNative.SetForegroundWindow(c.TopHwnd);
        await worker.Run(() => { c.Element?.SetFocus(); return true; }, ReadBudgetMs, false, "set-focus").ConfigureAwait(false);
        for (int i = 0; i < 6; i++)
        {
            if (await FocusSameAsync(c).ConfigureAwait(false)) return true;
            await Task.Delay(50).ConfigureAwait(false);
        }
        return false;
    }

    private static IUIAutomationTextPattern? Text(IUIAutomationElement el) => TryObj(() => el.GetCurrentPattern(UiaIds.TextPattern)) as IUIAutomationTextPattern;

    private static IUIAutomationTextRange? FirstSelection(IUIAutomationTextPattern tp)
    {
        var arr = TryRef(() => tp.GetSelection());
        if (arr is null) return null;
        int n = TryInt(() => arr.GetLength(), 0);
        return n > 0 ? TryRef(() => arr.GetElement(0)) : null;
    }

    private static int Int(IUIAutomationElement el, int id) => TryObj(() => el.GetCurrentPropertyValue(id)) is int i ? i : 0;

    private static bool? Bool(IUIAutomationElement el, int id) => TryObj(() => el.GetCurrentPropertyValue(id)) is bool b ? b : null;

    private static string? Str(IUIAutomationElement el, int id) => TryObj(() => el.GetCurrentPropertyValue(id)) as string;

    private static bool Swallowed(Exception e) => e is COMException or InvalidCastException or ArgumentException or NotImplementedException or UnauthorizedAccessException;

    private static T? TryRef<T>(Func<T?> f) where T : class
    {
        try { return f(); }
        catch (Exception e) when (Swallowed(e)) { return null; }
    }

    private static object? TryObj(Func<object?> f)
    {
        try { return f(); }
        catch (Exception e) when (Swallowed(e)) { return null; }
    }

    private static int TryInt(Func<int> f, int fallback)
    {
        try { return f(); }
        catch (Exception e) when (Swallowed(e)) { return fallback; }
    }

    /// <summary>The app's name for the pill and the card: the exe's file description ("Notepad"), else its stem.</summary>
    public static string AppName(string path, string exe)
    {
        try
        {
            if (path.Length > 0 && System.Diagnostics.FileVersionInfo.GetVersionInfo(path).FileDescription is { Length: > 0 } d)
            {
                d = d.Trim();
                // Win11 Notepad describes itself as "Notepad.exe": a description that is a file name says the stem.
                if (d.EndsWith(".exe", StringComparison.OrdinalIgnoreCase)) d = d[..^4];
                if (d.Length > 0) return d;
            }
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or ArgumentException) { }
        var stem = Path.GetFileNameWithoutExtension(exe);
        return stem.Length > 0 ? char.ToUpperInvariant(stem[0]) + stem[1..] : "";
    }
}