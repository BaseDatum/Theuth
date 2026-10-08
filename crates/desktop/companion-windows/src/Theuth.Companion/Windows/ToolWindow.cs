using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Theuth.Companion.Interop;
using Windows.Graphics;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.Shell;
using Windows.Win32.UI.WindowsAndMessaging;

namespace Theuth.Companion.Ui;

/// <summary>
/// A borderless, topmost, no-taskbar WinUI window — the rail, the Attention panel, the legend, the peek card and the
/// tray menu's host (design 134 §4.1–4.2, spike S1): an <see cref="OverlappedPresenter"/> without border and title bar,
/// <c>IsShownInSwitchers = false</c>, <c>WS_EX_TOOLWINDOW | WS_EX_TOPMOST</c> (+ the caller's ex-styles) set before the first
/// show, a <c>SetWindowSubclass</c> on the top-level HWND installed before <c>SetPresenter</c>/<c>MoveAndResize</c> that answers
/// <c>WM_GETMINMAXINFO</c> with the requested size (else Windows clamps a frameless top-level to 136 px, A6) and reports
/// <c>WM_ACTIVATE</c>; <c>WDA_EXCLUDEFROMCAPTURE</c> unless <c>--shareable</c> (C11). Closing hides it (Alt+F4 must not
/// destroy a window the companion reuses) until the app quits.
/// </summary>
internal abstract partial class ToolWindow : Window
{
    private const nuint SubclassId = 0x134;
    private readonly SUBCLASSPROC subclass; // referenced for the window's lifetime
    private readonly OverlappedPresenter presenter;
    private int minWidth = 1;
    private int minHeight = 1;
    private bool minimumLogged;

    protected ToolWindow(string title, WINDOW_EX_STYLE extra, bool excludeFromCapture)
    {
        Title = title;
        Hwnd = Win32.Hwnd(this);
        subclass = SubclassProc;
        if (!PInvoke.SetWindowSubclass(Hwnd, subclass, SubclassId, 0)) Log.Write($"{title}: SetWindowSubclass failed {Win32.LastError()}");
        presenter = OverlappedPresenter.Create();
        presenter.SetBorderAndTitleBar(false, false);
        presenter.IsResizable = false;
        presenter.IsMaximizable = false;
        presenter.IsMinimizable = false;
        presenter.IsAlwaysOnTop = true;
        AppWindow.SetPresenter(presenter);
        AppWindow.IsShownInSwitchers = false;
        Win32.SetExStyle(Hwnd, Win32.ExStyle(Hwnd) | WINDOW_EX_STYLE.WS_EX_TOOLWINDOW | WINDOW_EX_STYLE.WS_EX_TOPMOST | extra);
        if (extra.HasFlag(WINDOW_EX_STYLE.WS_EX_LAYERED))
        {
            PInvoke.SetLayeredWindowAttributes(Hwnd, default, 255, LAYERED_WINDOW_ATTRIBUTES_FLAGS.LWA_ALPHA);
        }
        Win32.FrameChanged(Hwnd);
        ExcludedFromCapture = excludeFromCapture;
        if (excludeFromCapture && !PInvoke.SetWindowDisplayAffinity(Hwnd, WINDOW_DISPLAY_AFFINITY.WDA_EXCLUDEFROMCAPTURE))
        {
            Log.Write($"{title}: SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) failed {Win32.LastError()}");
        }
        AppWindow.Closing += (_, e) =>
        {
            if (AllowClose) return;
            e.Cancel = true;
            OnCloseRequested();
        };
    }

    public HWND Hwnd { get; }

    public bool ExcludedFromCapture { get; }

    /// <summary>Set when the app quits: closing then really closes.</summary>
    public bool AllowClose { get; set; }

    public bool IsShown => AppWindow.IsVisible;

    public double Scale => Win32.Scale(Hwnd);

    public RECT ScreenRect => Win32.Rect(Hwnd);

    /// <summary>Alt+F4 or a close request: hide by default.</summary>
    protected virtual void OnCloseRequested() => AppWindow.Hide();

    /// <summary><c>WM_ACTIVATE</c>, after the default handling. <paramref name="other"/>: the window losing or gaining activation.</summary>
    protected virtual void OnActivate(bool active, HWND other) { }

    /// <summary>Moves and sizes in physical pixels (<c>MoveAndResize</c> is physical, A6), lowering the minimum track size first.</summary>
    public void Place(RectInt32 r)
    {
        minWidth = Math.Max(1, r.Width);
        minHeight = Math.Max(1, r.Height);
        try
        {
            presenter.PreferredMinimumWidth = minWidth;
            presenter.PreferredMinimumHeight = minHeight;
        }
        catch (Exception e) when (!minimumLogged)
        {
            minimumLogged = true;
            Log.Write($"{Title}: PreferredMinimumWidth/Height failed hr=0x{e.HResult:X8} {e.Message}");
        }
        AppWindow.MoveAndResize(r);
    }

    /// <summary>On screen without taking the foreground (<c>AppWindow.Show(false)</c>, never <c>Activate()</c>).</summary>
    public void ShowPassive() => AppWindow.Show(false);

    /// <summary>Shown and activated (<c>AppWindow.Show(true)</c> + <c>SetForegroundWindow</c>, spike S3). False when Windows refused the foreground.</summary>
    public bool ShowActive()
    {
        AppWindow.Show(true);
        bool ok = PInvoke.SetForegroundWindow(Hwnd);
        return ok || PInvoke.GetForegroundWindow() == Hwnd;
    }

    public void HideWindow() => AppWindow.Hide();

    public bool IsForeground => PInvoke.GetForegroundWindow() == Hwnd;

    private LRESULT SubclassProc(HWND h, uint msg, WPARAM w, LPARAM l, nuint id, nuint data)
    {
        if (msg == PInvoke.WM_GETMINMAXINFO)
        {
            var r = PInvoke.DefSubclassProc(h, msg, w, l);
            unsafe
            {
                var mmi = (MINMAXINFO*)l.Value;
                mmi->ptMinTrackSize = new System.Drawing.Point(minWidth, minHeight);
            }
            return r;
        }
        if (msg == PInvoke.WM_ACTIVATE)
        {
            var r = PInvoke.DefSubclassProc(h, msg, w, l);
            try
            {
                OnActivate((w.Value & 0xFFFF) != PInvoke.WA_INACTIVE, (HWND)l.Value);
            }
            catch (Exception e)
            {
                Log.Write($"{Title}: WM_ACTIVATE handler failed: {e.GetType().Name}: {e.Message}");
            }
            return r;
        }
        return PInvoke.DefSubclassProc(h, msg, w, l);
    }

    /// <summary>Before the window is destroyed at quit: detach what must not outlive it (W6: the system backdrop controller).</summary>
    protected virtual void OnClosingForGood() { }

    /// <summary>The app quits: really close (removes the subclass first).</summary>
    public void CloseForGood()
    {
        AllowClose = true;
        try { OnClosingForGood(); }
        catch (Exception e) { Log.Write($"{Title}: OnClosingForGood failed: {e.GetType().Name}: {e.Message}"); }
        PInvoke.RemoveWindowSubclass(Hwnd, subclass, SubclassId);
        Close();
    }
}
