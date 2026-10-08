namespace Theuth.Companion.Notifications;

/// <summary>
/// A managed exception that escapes a WinRT callback (a <c>DispatcherQueueTimer.Tick</c>, a hook, an event raised from
/// native code) never reaches <c>Application.UnhandledException</c>: CsWinRT hands the HRESULT back to CoreMessaging, which
/// fails fast with 0xC000027B (STATUS_STOWED_EXCEPTION) and nothing is logged. Every W3 callback runs through here instead:
/// the exception is logged with its stack and the companion stays up.
/// </summary>
internal static class Guard
{
    public static void Run(string what, Action action)
    {
        try { action(); }
        catch (Exception e) { Log.Write($"EXCEPTION in {what}: {e.GetType().Name}: {e.Message}\n{e.StackTrace}"); }
    }

    public static T Run<T>(string what, Func<T> action, T fallback)
    {
        try { return action(); }
        catch (Exception e)
        {
            Log.Write($"EXCEPTION in {what}: {e.GetType().Name}: {e.Message}\n{e.StackTrace}");
            return fallback;
        }
    }
}
