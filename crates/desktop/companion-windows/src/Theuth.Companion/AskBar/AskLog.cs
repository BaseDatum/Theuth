namespace Theuth.Companion.AskBar;

/// <summary>
/// The W4 probe log (Swift <c>AskLog</c>): every line goes to the companion log and, during a probe, to
/// <c>%LOCALAPPDATA%\theuth\companion\logs\ask-probe.log</c> (truncated per run) — what ask.ps1 prints after the run.
/// </summary>
internal static class AskLog
{
    private static readonly object Gate = new();
    private static string? file;

    public static string File => Path.Combine(Log.Dir, "ask-probe.log");

    /// <summary>Start the per-run file (a probe); without it lines only reach the companion log.</summary>
    public static void Start()
    {
        lock (Gate)
        {
            try
            {
                Directory.CreateDirectory(Log.Dir);
                System.IO.File.WriteAllText(File, "");
                file = File;
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
                Log.Write($"ask-probe log unusable: {e.Message}");
            }
        }
    }

    public static void Write(string message)
    {
        Log.Write(message);
        lock (Gate)
        {
            if (file is null) return;
            var line = $"[companion {DateTime.Now:HH:mm:ss.fff} pid {Environment.ProcessId}] {message}";
            try { System.IO.File.AppendAllText(file, line + Environment.NewLine); }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException) { }
        }
    }
}
