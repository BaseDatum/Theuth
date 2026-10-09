using System.Globalization;

namespace Theuth.Companion.AskBar;

/// <summary>
/// W4's switches (design 134 §5.5; the Swift companion's <c>AskFlags</c>): <c>--ask-probe N</c> (connect, show the Ask
/// bar, resolve theuth's desk without sending, quit after N s), <c>--synthetic-dictation</c> (2 s of a 440 Hz tone
/// through <c>dictation_start/audio/stop</c>), <c>--synthetic-speech "text"</c> (the same path with speech from the
/// system synthesizer), <c>--ask</c> (open the Ask bar at launch), <c>--mic-probe N</c> (capture the default endpoint
/// for N s and log the RMS — C8), <c>--mic-mix-format</c> (capture in the device's mix format and resample in-process),
/// <c>--ask-replay</c> (render the receipt row from a recorded <c>TextDelta</c>/<c>TurnFinished</c> stream).
/// 153 W153B: <c>--ask-fixture shaped|send-it|suggestion|all</c> (with <c>--ask-probe N</c>): recorded <c>Dictation*</c> /
/// <c>DictionarySuggestion</c> items through the Store as if dictated (<see cref="AskFixtures"/>).
/// </summary>
internal static class AskFlags
{
    private static readonly string[] Args = Environment.GetCommandLineArgs().Skip(1).ToArray();

    public static double? Probe { get; } = Seconds("--ask-probe");
    public static bool SyntheticDictation { get; } = Args.Contains("--synthetic-dictation");
    public static string? SyntheticSpeech { get; } = Value("--synthetic-speech");
    public static bool Ask { get; } = Args.Contains("--ask");
    public static double? MicProbe { get; } = Seconds("--mic-probe");
    public static bool MicMixFormat { get; } = Args.Contains("--mic-mix-format");
    public static bool Replay { get; } = Args.Contains("--ask-replay");
    public static string? Fixture { get; } = Value("--ask-fixture");

    /// <summary>153 W153B: with <c>--settings dictation</c>, render the Settings › Dictation page to <c>%TEMP%\153-w153b-settings-dictation.png</c> once it loaded.</summary>
    public static bool DictationShot { get; } = Args.Contains("--dictation-shot");

    /// <summary>Where the 153 probes put their PNGs (<c>w153c-gate.ps1 pngs</c> / <c>b64</c> read them there).</summary>
    public static string Png(string name) => Path.Combine(Path.GetTempPath(), $"153-w153b-{name}.png");

    /// <summary>A probe run: the hotkey and the microphone are inert (Swift <c>AskFlags.automated</c>).</summary>
    public static bool Automated => Probe is not null || MicProbe is not null || Replay;

    private static string? Value(string flag)
    {
        int i = Array.IndexOf(Args, flag);
        return i < 0 || i + 1 >= Args.Length ? null : Args[i + 1];
    }

    private static double? Seconds(string flag)
    {
        var v = Value(flag);
        if (v is null) return null;
        return double.TryParse(v, NumberStyles.Float, CultureInfo.InvariantCulture, out var s) && s >= 0 ? s : null;
    }
}
