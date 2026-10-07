using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml.Controls;

namespace SpikeHost;

/// <summary>Timestamped log: appended to %LOCALAPPDATA%\theuth-spikes\&lt;mode&gt;.log and mirrored into every attached TextBox.</summary>
internal static class SpikeLog
{
    private static readonly object Gate = new();
    private static readonly List<TextBox> Sinks = new();
    private static readonly System.Text.StringBuilder Buffer = new();
    private static string _file = "";
    private static DispatcherQueue? _dq;

    public static string FilePath => _file;

    public static void Init(string mode)
    {
        var dir = System.IO.Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "theuth-spikes");
        try { Directory.CreateDirectory(dir); } catch { }
        _file = System.IO.Path.Combine(dir, mode + ".log");
        _dq = DispatcherQueue.GetForCurrentThread();
        Append($"=== {mode} start pid={Environment.ProcessId} exe={Environment.ProcessPath} args=[{string.Join(" ", Environment.GetCommandLineArgs().Skip(1))}] os={Environment.OSVersion.VersionString} log={_file} ===");
    }

    public static void Attach(TextBox box)
    {
        lock (Gate)
        {
            Sinks.Add(box);
            box.Text = Buffer.ToString();
        }
    }

    public static void Append(string line)
    {
        var stamped = $"{DateTime.Now:HH:mm:ss.fff} {line}";
        lock (Gate)
        {
            Buffer.AppendLine(stamped);
            if (_file.Length > 0)
            {
                try { File.AppendAllText(_file, stamped + Environment.NewLine); } catch { }
            }
        }
        if (_dq is null) return;
        if (_dq.HasThreadAccess) Push(); else _dq.TryEnqueue(Push);
    }

    private static void Push()
    {
        string text;
        TextBox[] sinks;
        lock (Gate) { text = Buffer.ToString(); sinks = Sinks.ToArray(); }
        foreach (var b in sinks)
        {
            b.Text = text;
            try { b.SelectionStart = text.Length; } catch { }
        }
    }
}
