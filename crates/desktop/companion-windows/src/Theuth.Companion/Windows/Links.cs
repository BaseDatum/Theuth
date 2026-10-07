using System.Diagnostics;
using Windows.Win32;

namespace Theuth.Companion.Ui;

/// <summary>
/// <c>theuth://</c> links into the main window (design 134 §4.2: D · O · Ctrl+O, Done-zone sessions, peek-card sessions)
/// through <c>Launcher.LaunchUriAsync</c>, ShellExecute as the fallback; the scheme handler (the Tauri shell's deep-link
/// plugin, W5) may take the foreground, which the companion grants first.
/// </summary>
internal static class Links
{
    public static async Task<bool> Open(Uri uri)
    {
        PInvoke.AllowSetForegroundWindow(PInvoke.ASFW_ANY);
        try
        {
            bool ok = await global::Windows.System.Launcher.LaunchUriAsync(uri);
            Log.Write($"open {uri.OriginalString} (Launcher.LaunchUriAsync) -> {(ok ? "true" : "false")}");
            if (ok) return true;
        }
        catch (Exception e)
        {
            Log.Write($"open {uri.OriginalString}: Launcher.LaunchUriAsync failed: {e.GetType().Name}: {e.Message}");
        }
        try
        {
            Process.Start(new ProcessStartInfo(uri.OriginalString) { UseShellExecute = true });
            Log.Write($"open {uri.OriginalString} (ShellExecute)");
            return true;
        }
        catch (Exception e)
        {
            Log.Write($"open {uri.OriginalString}: ShellExecute failed: {e.GetType().Name}: {e.Message}");
            return false;
        }
    }

    /// <summary>
    /// Tray › Open theuth (Swift W7): the main app shows the view it had open last — start <c>theuth-desktop.exe</c> beside
    /// the <c>companion\</c> folder (the shell's single-instance plugin brings a running one forward); <c>theuth://new</c>
    /// only when it cannot be found.
    /// </summary>
    public static void OpenTheuth()
    {
        var main = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "..", "theuth-desktop.exe"));
        if (File.Exists(main))
        {
            PInvoke.AllowSetForegroundWindow(PInvoke.ASFW_ANY);
            try
            {
                Process.Start(new ProcessStartInfo(main) { UseShellExecute = true });
                Log.Write($"open theuth: started {main}");
                return;
            }
            catch (Exception e)
            {
                Log.Write($"open theuth: {main} failed: {e.Message}");
            }
        }
        _ = Open(Core.DeepLink.New);
    }
}
