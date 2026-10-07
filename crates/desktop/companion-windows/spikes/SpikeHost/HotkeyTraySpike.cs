using System.Runtime.InteropServices;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.System;

namespace SpikeHost;

/// <summary>
/// S3: tray icon (Shell_NotifyIcon, P/Invoke, no library) + global hotkey Ctrl+Shift+Space (RegisterHotKey) on the
/// log window's HWND, whose window procedure is subclassed (comctl32 SetWindowSubclass). Hotkey or tray click shows a
/// panel window (no taskbar button, topmost), brings it to the foreground and focuses its TextBox; Esc hides it and
/// returns the foreground to the previous window.
/// </summary>
internal sealed class HotkeyTraySpike
{
    private const int HotkeyId = 0x0134;
    private const uint WM_TRAY = Native.WM_APP + 1;

    private readonly LogWindow _log;
    private readonly Window _panel;
    private readonly TextBox _input;
    private readonly IntPtr _logHwnd;
    private readonly IntPtr _panelHwnd;
    private readonly Native.SubclassProc _proc;
    private Native.NOTIFYICONDATAW _nid;
    private bool _trayAdded;
    private bool _hotkeyRegistered;
    private IntPtr _prevFg;
    private int _shows;

    public HotkeyTraySpike()
    {
        SpikeLog.Init("hotkey-tray");

        var hint = new TextBlock { Text = "Press Ctrl+Shift+Space or left-click the tray icon to open the panel; Esc hides it.", Margin = new Thickness(12), TextWrapping = TextWrapping.Wrap };
        _log = new LogWindow("SpikeHost S3", hint, exitOnClose: false);
        _log.Closed += (s, e) => { Cleanup(); Application.Current.Exit(); };
        Ui.Place(_log, 900, 620);
        _log.Activate();
        _logHwnd = Ui.Hwnd(_log);

        // Panel (created hidden).
        _panel = new Window { Title = "SpikeHost S3 panel" };
        var stack = new StackPanel { Spacing = 8, Margin = new Thickness(16) };
        stack.Children.Add(new TextBlock { Text = "SpikeHost panel. Type here; Esc hides.", FontSize = 16 });
        _input = new TextBox { PlaceholderText = "focus lands here" };
        AutomationProperties.SetAutomationId(_input, "PanelInput");
        stack.Children.Add(_input);
        var root = new Grid();
        root.Children.Add(stack);
        root.PreviewKeyDown += (s, e) =>
        {
            if (e.Key == VirtualKey.Escape) { e.Handled = true; HidePanel(); }
        };
        _panel.Content = root;
        _panelHwnd = Ui.Hwnd(_panel);
        var aw = _panel.AppWindow;
        var presenter = OverlappedPresenter.Create();
        presenter.SetBorderAndTitleBar(true, false);
        presenter.IsResizable = false;
        presenter.IsMaximizable = false;
        presenter.IsMinimizable = false;
        presenter.IsAlwaysOnTop = true;
        aw.SetPresenter(presenter);
        aw.IsShownInSwitchers = false;
        long ex = (long)Native.GetWindowLongPtr(_panelHwnd, Native.GWL_EXSTYLE);
        Native.SetWindowLongPtr(_panelHwnd, Native.GWL_EXSTYLE, new IntPtr(ex | Native.WS_EX_TOOLWINDOW));
        Ui.Place(_panel, 420, 160, 0, -200);
        SpikeLog.Append($"panel hwnd=0x{_panelHwnd.ToInt64():X} exstyle={Ui.ExStyleBits((long)Native.GetWindowLongPtr(_panelHwnd, Native.GWL_EXSTYLE))} (hidden until hotkey/tray)");

        // Subclass the log window: it receives WM_HOTKEY and the tray callback.
        _proc = WndProc;
        bool sub = Native.SetWindowSubclass(_logHwnd, _proc, new UIntPtr(0x134), UIntPtr.Zero);
        SpikeLog.Append($"SetWindowSubclass(log hwnd 0x{_logHwnd.ToInt64():X}) -> {sub}");

        _hotkeyRegistered = Native.RegisterHotKey(_logHwnd, HotkeyId, Native.MOD_CONTROL | Native.MOD_SHIFT | Native.MOD_NOREPEAT, Native.VK_SPACE);
        int hkErr = Marshal.GetLastWin32Error();
        SpikeLog.Append($"RegisterHotKey(Ctrl+Shift+Space, MOD_NOREPEAT) -> {_hotkeyRegistered} err={hkErr}{(hkErr == 1409 ? " (ERROR_HOTKEY_ALREADY_REGISTERED)" : "")}");

        // Tray icon.
        _nid = new Native.NOTIFYICONDATAW
        {
            cbSize = (uint)Marshal.SizeOf<Native.NOTIFYICONDATAW>(),
            hWnd = _logHwnd,
            uID = 1,
            uFlags = Native.NIF_MESSAGE | Native.NIF_ICON | Native.NIF_TIP | Native.NIF_SHOWTIP,
            uCallbackMessage = WM_TRAY,
            hIcon = Native.LoadIcon(IntPtr.Zero, new IntPtr(Native.IDI_APPLICATION)),
            szTip = "SpikeHost S3 - click to open the panel",
            szInfo = "",
            szInfoTitle = "",
        };
        _trayAdded = Native.Shell_NotifyIconW(Native.NIM_ADD, ref _nid);
        int trayErr = Marshal.GetLastWin32Error();
        SpikeLog.Append($"Shell_NotifyIcon(NIM_ADD) -> {_trayAdded} err={trayErr} cbSize={_nid.cbSize} hIcon=0x{_nid.hIcon.ToInt64():X}");
        _nid.uVersion = Native.NOTIFYICON_VERSION_4;
        bool ver = Native.Shell_NotifyIconW(Native.NIM_SETVERSION, ref _nid);
        SpikeLog.Append($"Shell_NotifyIcon(NIM_SETVERSION 4) -> {ver}");
        SpikeLog.Append($"ready; foreground={Ui.Describe(Native.GetForegroundWindow())}");
    }

    private IntPtr WndProc(IntPtr hWnd, uint uMsg, IntPtr wParam, IntPtr lParam, UIntPtr uIdSubclass, UIntPtr dwRefData)
    {
        if (uMsg == Native.WM_HOTKEY && wParam.ToInt64() == HotkeyId)
        {
            SpikeLog.Append("WM_HOTKEY received");
            ShowPanel("hotkey");
            return IntPtr.Zero;
        }
        if (uMsg == WM_TRAY)
        {
            uint evt = (uint)(lParam.ToInt64() & 0xFFFF);
            if (evt != Native.WM_MOUSEMOVE)
                SpikeLog.Append($"tray callback event=0x{evt:X} at ({Native.LoWord(wParam)},{Native.HiWord(wParam)})");
            if (evt == Native.WM_LBUTTONUP || evt == Native.NIN_SELECT || evt == Native.NIN_KEYSELECT)
                ShowPanel("tray");
            return IntPtr.Zero;
        }
        return Native.DefSubclassProc(hWnd, uMsg, wParam, lParam);
    }

    private void ShowPanel(string source)
    {
        _shows++;
        _prevFg = Native.GetForegroundWindow();
        SpikeLog.Append($"[{source} #{_shows}] foreground before = {Ui.Describe(_prevFg)}");

        _panel.AppWindow.Show(true);
        bool sfw = Native.SetForegroundWindow(_panelHwnd);
        int err = Marshal.GetLastWin32Error();
        var fg1 = Native.GetForegroundWindow();
        SpikeLog.Append($"[{source}] Show(true) + SetForegroundWindow -> {sfw} err={err}; foreground now = {Ui.Describe(fg1)} isPanel={fg1 == _panelHwnd}");

        if (fg1 != _panelHwnd)
        {
            // Fallback 1: the classic ALT tap unlocks SetForegroundWindow for the caller.
            Native.keybd_event(Native.VK_MENU, 0, 0, UIntPtr.Zero);
            Native.keybd_event(Native.VK_MENU, 0, Native.KEYEVENTF_KEYUP, UIntPtr.Zero);
            bool sfw2 = Native.SetForegroundWindow(_panelHwnd);
            var fg2 = Native.GetForegroundWindow();
            SpikeLog.Append($"[{source}] fallback ALT-tap + SetForegroundWindow -> {sfw2}; foreground now = {Ui.Describe(fg2)} isPanel={fg2 == _panelHwnd}");
            if (fg2 != _panelHwnd)
            {
                // Fallback 2: AttachThreadInput to the foreground thread.
                uint fgThread = Native.GetWindowThreadProcessId(fg2, out _);
                uint me = Native.GetCurrentThreadId();
                bool att = Native.AttachThreadInput(me, fgThread, true);
                bool sfw3 = Native.SetForegroundWindow(_panelHwnd);
                Native.BringWindowToTop(_panelHwnd);
                if (att) Native.AttachThreadInput(me, fgThread, false);
                var fg3 = Native.GetForegroundWindow();
                SpikeLog.Append($"[{source}] fallback AttachThreadInput({att}) + SetForegroundWindow -> {sfw3}; foreground now = {Ui.Describe(fg3)} isPanel={fg3 == _panelHwnd}");
            }
        }

        bool focused = _input.Focus(FocusState.Programmatic);
        var t = _panel.DispatcherQueue.CreateTimer();
        t.Interval = TimeSpan.FromMilliseconds(300);
        t.IsRepeating = false;
        t.Tick += (s, e) =>
        {
            var fg = Native.GetForegroundWindow();
            SpikeLog.Append($"[{source}] +300ms foreground = {Ui.Describe(fg)} isPanel={fg == _panelHwnd}; TextBox.Focus()={focused} FocusState={_input.FocusState} GetFocus=0x{Native.GetFocus().ToInt64():X} GetActiveWindow=0x{Native.GetActiveWindow().ToInt64():X}");
        };
        t.Start();
    }

    private void HidePanel()
    {
        SpikeLog.Append($"Esc: hiding panel; text typed='{_input.Text}'");
        _panel.AppWindow.Hide();
        bool ok = Native.SetForegroundWindow(_prevFg);
        var fg = Native.GetForegroundWindow();
        SpikeLog.Append($"Esc: SetForegroundWindow(prev {Ui.Describe(_prevFg)}) -> {ok}; foreground now = {Ui.Describe(fg)} restored={fg == _prevFg}");
    }

    private void Cleanup()
    {
        if (_hotkeyRegistered)
        {
            bool ok = Native.UnregisterHotKey(_logHwnd, HotkeyId);
            SpikeLog.Append($"UnregisterHotKey -> {ok}");
            _hotkeyRegistered = false;
        }
        if (_trayAdded)
        {
            bool ok = Native.Shell_NotifyIconW(Native.NIM_DELETE, ref _nid);
            SpikeLog.Append($"Shell_NotifyIcon(NIM_DELETE) -> {ok}");
            _trayAdded = false;
        }
        Native.RemoveWindowSubclass(_logHwnd, _proc, new UIntPtr(0x134));
        SpikeLog.Append("cleanup done");
    }
}
