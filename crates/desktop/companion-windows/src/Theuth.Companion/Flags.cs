using System.Globalization;

namespace Theuth.Companion;

/// <summary>
/// Command-line switches (design 134 §5.5; the Swift companion's flags so the wave gates are the same scripts).
/// W1: <c>--probe N</c> (connect, log counts, exit 0 after N s; never takes the foreground), <c>--shareable</c> (W2: no
/// capture exclusion), <c>--background</c> (the Run key's launch, W3), <c>--register-smoke</c> (build.ps1).
/// Later waves add <c>--probe-notify</c>, <c>--probe-legend</c>, <c>--ask-probe N --synthetic-dictation</c>.
/// </summary>
internal static class Flags
{
    private static readonly string[] Args = Environment.GetCommandLineArgs().Skip(1).ToArray();

    public static double? Probe { get; } = Seconds("--probe");
    public static bool Shareable { get; } = Args.Contains("--shareable");
    public static bool Background { get; } = Args.Contains("--background");
    public static bool RegisterSmoke { get; } = Args.Contains("--register-smoke");

    private static double? Seconds(string flag)
    {
        int i = Array.IndexOf(Args, flag);
        if (i < 0 || i + 1 >= Args.Length) return null;
        return double.TryParse(Args[i + 1], NumberStyles.Float, CultureInfo.InvariantCulture, out var s) && s >= 0 ? s : null;
    }

    /// <summary>Seconds as Swift prints a <c>TimeInterval</c>: <c>15.0</c>, <c>1.5</c>; <c>no</c> when absent.</summary>
    public static string Describe(double? seconds) =>
        seconds is not { } s ? "no" : s == Math.Floor(s) ? s.ToString("0.0", CultureInfo.InvariantCulture) : s.ToString("R", CultureInfo.InvariantCulture);
}
