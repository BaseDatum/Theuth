using Microsoft.Windows.AppLifecycle;
using Windows.Win32;

namespace Theuth.Companion.Ui;

/// <summary>
/// Single instance (design 134 §5.5, A9): the first process owns <c>AppInstance</c> key <c>io.theuth.companion</c>; a second
/// launch lets the first take the foreground (<c>AllowSetForegroundWindow</c>), redirects its activation there (a launch, a
/// toast click) and exits 0 — the first instance hears it on <c>AppInstance.Activated</c> and opens the panel
/// (<see cref="Surfaces"/>). Called from <c>Program.Main</c> before any XAML. <c>--register-smoke</c> (build.ps1's self-test)
/// never redirects. The named mutex <c>Local\io.theuth.companion</c> stays the shell's alive check (W1, D10).
/// </summary>
internal static class SingleInstance
{
    public const string Key = "io.theuth.companion";


    /// <summary>True: this process is the companion. False: the activation went to the running one — return 0.</summary>
    public static bool Claim(string[] args)
    {
        if (args.Contains("--register-smoke")) return true;
        AppInstance me;
        try
        {
            me = AppInstance.FindOrRegisterForKey(Key);
        }
        catch (Exception e)
        {
            Note($"single instance: FindOrRegisterForKey failed ({e.GetType().Name}: {e.Message}) — running anyway");
            return true;
        }
        // The current instance's own activation (a toast click that LAUNCHED this process) is read by the Notifier right after
        // AppNotificationManager.Register(): read before it, a toast-launched process fail-fasts in Microsoft.WindowsAppRuntime.dll
        // (0xc0000409, build-B 2026-10-08 05:01–05:03). The redirect branch below is a different process and may read it.
        if (me.IsCurrent) return true;
        try
        {
            var activation = AppInstance.GetCurrent().GetActivatedEventArgs();
            PInvoke.AllowSetForegroundWindow(me.ProcessId);
            // Off the STA: RedirectActivationToAsync can deadlock a thread that is not pumping (Microsoft's sample does the same).
            bool done = Task.Run(() => me.RedirectActivationToAsync(activation).AsTask().Wait(TimeSpan.FromSeconds(10))).Wait(TimeSpan.FromSeconds(12));
            Note($"single instance: another companion runs (pid {me.ProcessId}) — activation ({activation.Kind}) redirected={(done ? "true" : "false")}, exit 0");
        }
        catch (Exception e)
        {
            Note($"single instance: redirect to pid {me.ProcessId} failed ({e.GetType().Name}: {e.Message}) — exit 0");
        }
        return false;
    }

    /// <summary>The log isn't started in a process that exits before XAML: append the line to the day's file directly.</summary>
    private static void Note(string message)
    {
        try
        {
            Directory.CreateDirectory(Log.Dir);
            File.AppendAllText(Path.Combine(Log.Dir, $"companion-{DateTime.Now:yyyy-MM-dd}.log"),
                $"[companion {DateTime.Now:HH:mm:ss.fff} pid {Environment.ProcessId}] {message}{Environment.NewLine}");
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException) { }
    }
}
