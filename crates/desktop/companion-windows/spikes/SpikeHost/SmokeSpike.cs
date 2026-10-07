using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace SpikeHost;

/// <summary>S0: a normal window with a button and a text block, to prove the toolchain and UI Automation.</summary>
internal sealed class SmokeSpike
{
    private int _clicks;
    private readonly LogWindow _win;

    public SmokeSpike()
    {
        SpikeLog.Init("smoke");

        var text = new TextBlock { Text = "not clicked", VerticalAlignment = VerticalAlignment.Center };
        AutomationProperties.SetAutomationId(text, "SmokeText");

        var button = new Button { Content = "Click me" };
        AutomationProperties.SetAutomationId(button, "SmokeButton");
        button.Click += (s, e) =>
        {
            _clicks++;
            text.Text = $"clicked {_clicks}";
            SpikeLog.Append($"SmokeButton clicked {_clicks}; foreground={Ui.Describe(Native.GetForegroundWindow())}");
        };

        var header = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12, Margin = new Thickness(12) };
        header.Children.Add(button);
        header.Children.Add(text);

        _win = new LogWindow("SpikeHost S0", header);
        Ui.Place(_win, 760, 520);
        _win.Activate();

        var hwnd = Ui.Hwnd(_win);
        SpikeLog.Append($"S0 window shown hwnd=0x{hwnd.ToInt64():X} dpi={Native.GetDpiForWindow(hwnd)} scale={Ui.Scale(hwnd):F2} rect={Ui.Rect(hwnd)} exstyle={Ui.ExStyleBits((long)Native.GetWindowLongPtr(hwnd, Native.GWL_EXSTYLE))}");
        SpikeLog.Append($"WinAppSDK runtime: {typeof(Window).Assembly.GetName().Name} {typeof(Window).Assembly.GetName().Version}; WASDK line={Build.WasdkLine}; .NET {Environment.Version}; AUMID={Native.ProcessAumid()}");
        try
        {
            var dir = System.IO.Path.GetDirectoryName(Environment.ProcessPath) ?? ".";
            var files = Directory.GetFiles(dir, "*", SearchOption.AllDirectories);
            long total = files.Sum(f => new FileInfo(f).Length);
            var big = files.Select(f => new FileInfo(f)).OrderByDescending(f => f.Length).Take(6).Select(f => $"{f.Name} {f.Length / 1048576.0:F1}MB");
            SpikeLog.Append($"output dir: {files.Length} files, {total / 1048576.0:F1} MB; largest: {string.Join(", ", big)}; ML present={File.Exists(System.IO.Path.Combine(dir, "onnxruntime.dll"))}");
        }
        catch (Exception ex) { SpikeLog.Append("output dir scan failed: " + ex.Message); }
    }
}
