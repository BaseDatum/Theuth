using Microsoft.UI.Dispatching;
using Windows.Win32;

namespace Theuth.Companion.PointAndSay;

/// <summary>One pointer sample (96 §4.2, <c>computer::pointer::Sample</c>): ms since the press, virtual-screen physical px, pen down.</summary>
internal readonly record struct PointSample(uint TMs, int X, int Y, bool Pen);

/// <summary>
/// The 60 Hz pointer sampler (149 §7.4 #4): a dispatcher timer at 1000/<see cref="Core.PointMachine.SampleHz"/> ms over
/// <c>GetCursorPos</c> (physical virtual-screen pixels under PerMonitorV2) and <c>GetAsyncKeyState(VK_LBUTTON)</c> for
/// <c>annotate = mouse</c>. Samples are kept for <c>screen_context</c>; each tick is handed to the overlays.
/// </summary>
internal sealed class PointerSampler : IDisposable
{
    private readonly DispatcherQueueTimer timer;
    private readonly List<PointSample> samples = new();
    private DateTimeOffset started;
    private bool penMode;

    public PointerSampler(DispatcherQueue dispatcher)
    {
        timer = dispatcher.CreateTimer();
        timer.Interval = TimeSpan.FromMilliseconds(1000.0 / Core.PointMachine.SampleHz);
        timer.IsRepeating = true;
        timer.Tick += (_, _) => Tick();
    }

    /// <summary>A sample landed (also the first one at start).</summary>
    public event Action<PointSample>? Sampled;

    public IReadOnlyList<PointSample> Samples => samples;

    public bool Running => timer.IsRunning;

    /// <summary><paramref name="mouse"/>: the pen is the left button; otherwise the whole hold draws.</summary>
    public void Start(bool mouse)
    {
        samples.Clear();
        penMode = mouse;
        started = DateTimeOffset.Now;
        Tick();
        timer.Start();
    }

    public void Stop() => timer.Stop();

    private void Tick()
    {
        if (!PInvoke.GetCursorPos(out var p)) return;
        bool pen = !penMode || (PInvoke.GetAsyncKeyState(Gdi.VkLButton) & 0x8000) != 0;
        var s = new PointSample((uint)Math.Max(0, (DateTimeOffset.Now - started).TotalMilliseconds), p.X, p.Y, pen);
        samples.Add(s);
        Sampled?.Invoke(s);
    }

    public void Dispose() => timer.Stop();
}
