using System.Runtime.InteropServices;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.WindowsAndMessaging;

namespace Theuth.Companion.Interop;

/// <summary>
/// The companion's hidden top-level window of class <c>TheuthCompanionRail</c> (design 134 §5.5–5.6): the Tauri shell's
/// quit handshake finds it with <c>FindWindowW(class)</c> and posts <c>WM_CLOSE</c> (W5's <c>companion/windows.rs</c>),
/// and it is the target of the tray callback, <c>WM_HOTKEY</c> and the shell's <c>TaskbarCreated</c> broadcast. A WinUI
/// window's class is fixed (<c>WinUIDesktopWin32WindowClass</c>), so the rail's class name lives on this window, which
/// sits on the same UI thread as the rail (the WinUI message loop dispatches it). Never shown.
/// </summary>
internal sealed class HostWindow : IDisposable
{
    public const string ClassName = "TheuthCompanionRail";

    /// <summary>The tray icon's callback message (<c>uCallbackMessage</c>).</summary>
    public const uint TrayMessage = PInvoke.WM_APP + 1;

    private readonly WNDPROC proc; // referenced for the window's lifetime: the class holds a pointer to it
    private readonly uint taskbarCreated;
    private bool disposed;

    public HWND Handle { get; }

    /// <summary><c>WM_CLOSE</c> (the shell's terminate, or a plain close): the companion quits, exit 0.</summary>
    public event Action? CloseRequested;

    /// <summary><c>WM_HOTKEY</c> with the registration id.</summary>
    public event Action<int>? Hotkey;

    /// <summary>The tray callback (<c>NOTIFYICON_VERSION_4</c>): event (<c>LOWORD(lParam)</c>) and the anchor point (screen px).</summary>
    public event Action<uint, System.Drawing.Point>? Tray;

    /// <summary>Explorer restarted: the tray icon must be added again.</summary>
    public event Action? TaskbarCreated;

    /// <summary><c>WM_SETTINGCHANGE</c> (light/dark taskbar) or <c>WM_DISPLAYCHANGE</c> (the rail re-lays out).</summary>
    public event Action<uint>? SystemChanged;

    public unsafe HostWindow()
    {
        proc = WndProc;
        HINSTANCE instance = PInvoke.GetModuleHandle(default(PCWSTR));
        taskbarCreated = PInvoke.RegisterWindowMessage("TaskbarCreated");
        fixed (char* cls = ClassName)
        fixed (char* title = "theuth companion")
        {
            var wc = new WNDCLASSEXW
            {
                cbSize = (uint)Marshal.SizeOf<WNDCLASSEXW>(),
                lpfnWndProc = proc,
                hInstance = instance,
                lpszClassName = cls,
            };
            if (PInvoke.RegisterClassEx(in wc) == 0) Log.Write($"host: RegisterClassEx({ClassName}) failed {Win32.LastError()}");
            // A plain hidden popup (not message-only: FindWindowW only sees real top-level windows).
            Handle = PInvoke.CreateWindowEx(WINDOW_EX_STYLE.WS_EX_TOOLWINDOW | WINDOW_EX_STYLE.WS_EX_NOACTIVATE, cls, title,
                WINDOW_STYLE.WS_POPUP, 0, 0, 0, 0, default, default, instance, null);
        }
        Log.Write(Handle.IsNull
            ? $"host: CreateWindowEx({ClassName}) failed {Win32.LastError()}"
            : $"host window {Handle} class={ClassName} (the shell's WM_CLOSE target, tray callback, WM_HOTKEY)");
    }

    private LRESULT WndProc(HWND h, uint msg, WPARAM w, LPARAM l)
    {
        try
        {
            switch (msg)
            {
                case PInvoke.WM_HOTKEY:
                    Hotkey?.Invoke((int)w.Value);
                    return default;
                case TrayMessage:
                    // Version 4: LOWORD(lParam) = the event, wParam = the anchor (GET_X/Y_LPARAM).
                    Tray?.Invoke((uint)(l.Value & 0xFFFF), new System.Drawing.Point(Win32.LoWord((nint)w.Value), Win32.HiWord((nint)w.Value)));
                    return default;
                case PInvoke.WM_CLOSE:
                    Log.Write("host: WM_CLOSE — quitting");
                    CloseRequested?.Invoke();
                    return default; // the app quits; DestroyWindow happens in Dispose
                case PInvoke.WM_SETTINGCHANGE:
                case PInvoke.WM_DISPLAYCHANGE:
                    SystemChanged?.Invoke(msg);
                    break;
                default:
                    if (msg == taskbarCreated && taskbarCreated != 0)
                    {
                        Log.Write("host: TaskbarCreated — re-adding the tray icon");
                        TaskbarCreated?.Invoke();
                        return default;
                    }
                    break;
            }
        }
        catch (Exception e)
        {
            // Nothing may unwind through the native frames above a window procedure.
            Log.Write($"host: message 0x{msg:X4} handler failed: {e.GetType().Name}: {e.Message}");
        }
        return PInvoke.DefWindowProc(h, msg, w, l);
    }

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        if (!Handle.IsNull) PInvoke.DestroyWindow(Handle);
    }
}
