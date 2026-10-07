using System.Text;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml.Controls;

namespace Theuth.Companion;

/// <summary>
/// The companion log (design 134 §5.5): <c>[companion HH:mm:ss.fff pid N] message</c> lines — the Swift companion's
/// shape — to <c>%LOCALAPPDATA%\theuth\companion\logs\companion-&lt;date&gt;.log</c> (the newest 7 kept), to the log
/// window, to stderr under <c>--probe</c>/<c>--register-smoke</c>, and during a probe to <c>logs\probe.log</c>
/// (truncated per run, as Swift's <c>/tmp/130-w1-probe.log</c>). Thread-safe; the window is updated on its dispatcher.
/// </summary>
internal static class Log
{
    private const int KeepDays = 7;
    private static readonly object Gate = new();
    private static readonly StringBuilder Buffer = new();
    private static readonly List<TextBox> Sinks = new();
    private static DispatcherQueue? dispatcher;
    private static bool toStderr;
    private static string? probeFile;

    public static string Dir { get; } =
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "theuth", "companion", "logs");

    public static string DailyFile { get; private set; } = "";

    public static string ProbeFile => Path.Combine(Dir, "probe.log");

    public static void Start(DispatcherQueue queue, bool toStderr, bool probeFile)
    {
        dispatcher = queue;
        Log.toStderr = toStderr;
        try
        {
            Directory.CreateDirectory(Dir);
            DailyFile = Path.Combine(Dir, $"companion-{DateTime.Now:yyyy-MM-dd}.log");
            foreach (var old in Directory.GetFiles(Dir, "companion-*.log").OrderByDescending(f => f, StringComparer.Ordinal).Skip(KeepDays))
            {
                File.Delete(old);
            }
            if (probeFile)
            {
                Log.probeFile = ProbeFile;
                File.WriteAllText(Log.probeFile, "");
            }
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            Write($"log directory unusable: {e.Message}");
        }
    }

    public static void Write(string message)
    {
        var line = $"[companion {DateTime.Now:HH:mm:ss.fff} pid {Environment.ProcessId}] {message}";
        lock (Gate)
        {
            Buffer.AppendLine(line);
            Append(DailyFile, line);
            Append(probeFile, line);
            if (toStderr)
            {
                try { Console.Error.WriteLine(line); } catch (IOException) { }
            }
        }
        if (dispatcher is null) return;
        if (dispatcher.HasThreadAccess) Push();
        else dispatcher.TryEnqueue(Push);
    }

    public static void Attach(TextBox box)
    {
        lock (Gate)
        {
            Sinks.Add(box);
            box.Text = Buffer.ToString();
        }
    }

    private static void Append(string? file, string line)
    {
        if (string.IsNullOrEmpty(file)) return;
        try { File.AppendAllText(file, line + Environment.NewLine); }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException) { }
    }

    private static void Push()
    {
        string text;
        TextBox[] sinks;
        lock (Gate)
        {
            text = Buffer.ToString();
            sinks = Sinks.ToArray();
        }
        foreach (var box in sinks)
        {
            box.Text = text;
            box.SelectionStart = text.Length;
        }
    }
}
