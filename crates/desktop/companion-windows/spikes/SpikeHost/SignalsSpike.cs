using System.Runtime.InteropServices;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace SpikeHost;

/// <summary>
/// S4: SetWinEventHook(EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT) logging every foreground change
/// (process + title) and GetLastInputInfo idle seconds every 2 s, for 60 s.
/// </summary>
internal sealed class SignalsSpike
{
    private readonly LogWindow _log;
    private readonly Window _second;
    private readonly Native.WinEventProc _proc;
    private IntPtr _hook;
    private readonly DispatcherQueueTimer _timer;
    private readonly DateTime _start = DateTime.Now;
    private int _events;

    public SignalsSpike()
    {
        SpikeLog.Init("signals");
        _log = new LogWindow("SpikeHost S4");
        Ui.Place(_log, 900, 620, -60, 40);
        _log.Activate();

        _second = new Window { Title = "SpikeHost S4 second" };
        _second.Content = new TextBlock { Text = "Second window: activate me and the log window alternately.", Margin = new Thickness(16), TextWrapping = TextWrapping.Wrap };
        Ui.Place(_second, 420, 200, 300, -260);
        _second.Activate();

        _proc = OnWinEvent;
        _hook = Native.SetWinEventHook(Native.EVENT_SYSTEM_FOREGROUND, Native.EVENT_SYSTEM_FOREGROUND, IntPtr.Zero, _proc, 0, 0, Native.WINEVENT_OUTOFCONTEXT);
        SpikeLog.Append($"SetWinEventHook(EVENT_SYSTEM_FOREGROUND, OUTOFCONTEXT) -> 0x{_hook.ToInt64():X} err={Marshal.GetLastWin32Error()} thread={Native.GetCurrentThreadId()}");
        SpikeLog.Append($"initial foreground = {Ui.Describe(Native.GetForegroundWindow())}");

        _timer = _log.DispatcherQueue.CreateTimer();
        _timer.Interval = TimeSpan.FromSeconds(2);
        _timer.IsRepeating = true;
        _timer.Tick += (s, e) => OnTick();
        _timer.Start();
        OnTick();
    }

    private void OnTick()
    {
        var lii = new Native.LASTINPUTINFO { cbSize = (uint)Marshal.SizeOf<Native.LASTINPUTINFO>() };
        bool ok = Native.GetLastInputInfo(ref lii);
        uint now = Native.GetTickCount();
        double idle = unchecked(now - lii.dwTime) / 1000.0;
        double elapsed = (DateTime.Now - _start).TotalSeconds;
        SpikeLog.Append($"idle {idle,6:F1}s (GetLastInputInfo ok={ok} tick={lii.dwTime}) elapsed {elapsed:F0}s foregroundEvents={_events}");
        if (elapsed >= 60)
        {
            _timer.Stop();
            bool un = Native.UnhookWinEvent(_hook);
            _hook = IntPtr.Zero;
            SpikeLog.Append($"60 s over: UnhookWinEvent -> {un}; done ({_events} foreground events)");
        }
    }

    private void OnWinEvent(IntPtr hWinEventHook, uint eventType, IntPtr hwnd, int idObject, int idChild, uint idEventThread, uint dwmsEventTime)
    {
        if (idObject != Native.OBJID_WINDOW) return;
        _events++;
        SpikeLog.Append($"foreground -> {Ui.Describe(hwnd)} (event thread={idEventThread}, callback thread={Native.GetCurrentThreadId()}, t={dwmsEventTime})");
    }
}
