using System.Globalization;

namespace Theuth.Companion.Notifications;

/// <summary>
/// W3's switches (the Swift companion's <c>extension Flags</c> in SettingsStore.swift; design 134 §5.5): kept out of
/// W1's <c>Flags</c> so sibling waves never edit one file. <c>--probe-notify N</c>: run the policy live for N s, then
/// replay the live items for real and count the toasts (<see cref="NotifyProbe"/>); <c>--stay</c> keeps the app running
/// after the replay so a toast button can be pressed on the live process. <c>--request-notifications</c>: the one non-UI
/// place that may raise Windows' one-time urgent consent. <c>--settings [page]</c>: open the Settings window at launch.
/// </summary>
internal static class W3Flags
{
    private static readonly string[] Args = Environment.GetCommandLineArgs().Skip(1).ToArray();

    public static double? ProbeNotify { get; } = Seconds("--probe-notify");
    public static bool Stay { get; } = Args.Contains("--stay");
    public static bool RequestNotifications { get; } = Args.Contains("--request-notifications");

    /// <summary>null = not asked; "" = the first page; else a page key (rail · keys · notifications · undo · login · engine).</summary>
    public static string? SettingsPage { get; } = Word("--settings");

    /// <summary>Any probe: the surfaces must not take the foreground and nothing is answered on its own.</summary>
    public static bool AnyProbe => Flags.Probe is not null || ProbeNotify is not null;

    private static double? Seconds(string flag)
    {
        int i = Array.IndexOf(Args, flag);
        if (i < 0 || i + 1 >= Args.Length) return null;
        return double.TryParse(Args[i + 1], NumberStyles.Float, CultureInfo.InvariantCulture, out var s) && s >= 0 ? s : null;
    }

    private static string? Word(string flag)
    {
        int i = Array.IndexOf(Args, flag);
        if (i < 0) return null;
        return i + 1 < Args.Length && !Args[i + 1].StartsWith("--", StringComparison.Ordinal) ? Args[i + 1] : "";
    }
}
