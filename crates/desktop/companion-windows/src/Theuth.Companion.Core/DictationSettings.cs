using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>
/// Settings › Dictation's companion-local values (153 §14.3, W153B), in the companion's settings store under their own keys —
/// the <see cref="PointSettings"/> pattern; the shared <see cref="CompanionSettings"/> is untouched:
/// <list type="bullet">
/// <item><c>micOnKeyDown</c> (§8.5 "Open the mic on key-down", default on: the hold-to-talk recorder starts at the press);</item>
/// <item><c>shapePreset.&lt;surface&gt;</c> and <c>endPhrases</c> — the last value THIS companion wrote with <c>set_shape</c> /
/// <c>set_end_phrases</c>. The engine owns the settings (<c>voice.shape.*</c>, <c>voice.end_commands.*</c>) and the contract has
/// no getter (H3-B1: "remember the last value locally"), so these only label the controls; null = never set here (the engine
/// default, or whatever another client chose).</item>
/// </list>
/// </summary>
public sealed class DictationSettings : IEquatable<DictationSettings>
{
    public bool MicOnKeyDown { get; set; } = true;

    /// <summary>Surface word → preset word, as last written by this companion.</summary>
    public Dictionary<string, string> ShapePresets { get; set; } = new(StringComparer.Ordinal);

    /// <summary>The end phrases this companion last wrote; null = never.</summary>
    public EndPhrases? EndPhrases { get; set; }

    public static class Key
    {
        public const string MicOnKeyDown = "micOnKeyDown", ShapePresetPrefix = "shapePreset.", EndPhrases = "endPhrases";
    }

    /// <summary>The preset this companion last set for <paramref name="surface"/>, or null.</summary>
    public ShapePreset? PresetFor(Surface surface) => ShapePresets.TryGetValue(surface.Word(), out var w) ? ParsePreset(w) : null;

    /// <summary>A preset wire word, or null.</summary>
    public static ShapePreset? ParsePreset(string? word) => word switch
    {
        "full" => ShapePreset.Full,
        "plain" => ShapePreset.Plain,
        "off" => ShapePreset.Off,
        _ => null,
    };

    public static DictationSettings Load(SettingsStore d)
    {
        var s = new DictationSettings();
        if (d.Contains(Key.MicOnKeyDown)) s.MicOnKeyDown = d.Bool(Key.MicOnKeyDown);
        foreach (var surface in Enum.GetValues<Surface>())
        {
            if (ParsePreset(d.String(Key.ShapePresetPrefix + surface.Word())) is { } p) s.ShapePresets[surface.Word()] = p.Word();
        }
        if (d.String(Key.EndPhrases) is { Length: > 0 } json)
        {
            try { s.EndPhrases = TheuthJson.Deserialize<EndPhrases>(json); }
            catch (System.Text.Json.JsonException) { s.EndPhrases = null; }
        }
        return s;
    }

    public void Save(SettingsStore d)
    {
        d.Set(Key.MicOnKeyDown, MicOnKeyDown);
        foreach (var surface in Enum.GetValues<Surface>())
        {
            var key = Key.ShapePresetPrefix + surface.Word();
            if (ShapePresets.TryGetValue(surface.Word(), out var w)) d.Set(key, w);
            else d.Remove(key);
        }
        if (EndPhrases is { } p) d.Set(Key.EndPhrases, TheuthJson.Serialize(p));
        else d.Remove(Key.EndPhrases);
        d.Flush();
    }

    public DictationSettings Clone() => new()
    {
        MicOnKeyDown = MicOnKeyDown,
        ShapePresets = new Dictionary<string, string>(ShapePresets, StringComparer.Ordinal),
        EndPhrases = EndPhrases,
    };

    public bool Equals(DictationSettings? o) =>
        o is not null && MicOnKeyDown == o.MicOnKeyDown && Equals(EndPhrases, o.EndPhrases)
        && ShapePresets.Count == o.ShapePresets.Count && ShapePresets.All(kv => o.ShapePresets.TryGetValue(kv.Key, out var v) && v == kv.Value);

    public override bool Equals(object? obj) => Equals(obj as DictationSettings);

    public override int GetHashCode() => HashCode.Combine(MicOnKeyDown, ShapePresets.Count, EndPhrases);

    /// <summary>Settings › Dictation's phrase field: comma- or newline-separated, trimmed, empties dropped, case kept.</summary>
    public static ValueList<string> ParsePhrases(string text) =>
        text.Split(new[] { ',', '\n', ';' }, StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries).Distinct(StringComparer.OrdinalIgnoreCase).ToValueList();

    /// <summary>The field text for a phrase list.</summary>
    public static string JoinPhrases(IEnumerable<string> phrases) => string.Join(", ", phrases);

    /// <summary>
    /// The "Last dictation" line (153 §8.7, the Settings › Voice debug row's words): "Companion · mic 42 ms · first words 310 ms ·
    /// final 890 ms · stop → final 420 ms"; a stage that never happened is left out; null when there is no last dictation.
    /// </summary>
    public static string? TimingsLine(DictationTimings? t)
    {
        if (t is null) return null;
        var parts = new List<string> { t.Surface is { } s ? SurfaceLabel(s) : "Unknown surface" };
        if (t.PressToFirstPcmMs is { } pcm) parts.Add($"mic {pcm} ms");
        if (t.PressToFirstPartialMs is { } partial) parts.Add($"first words {partial} ms");
        if (t.PressToFirstFinalMs is { } fin) parts.Add($"final {fin} ms");
        if (t.StopToFinalMs is { } stop) parts.Add($"stop → final {stop} ms");
        if (t.PolishMs is { } polish) parts.Add($"polish {polish} ms");
        if (t.Device is { Length: > 0 } dev) parts.Add(dev);
        return string.Join(" · ", parts);
    }

    /// <summary>A surface as Settings names it.</summary>
    public static string SurfaceLabel(Surface s) => s switch
    {
        Surface.Composer => "Composer",
        Surface.Tui => "Terminal UI",
        Surface.Companion => "Companion",
        Surface.Phone => "Phone",
        _ => "Other apps",
    };

    /// <summary>A preset as Settings names it.</summary>
    public static string PresetLabel(ShapePreset p) => p switch
    {
        ShapePreset.Plain => "Plain — punctuation, numbers, literals",
        ShapePreset.Off => "Off — the model's words as heard",
        _ => "Full — plus dictionary and end commands",
    };

    // MARK: 153 F1 — what Settings › Dictation says once voice_settings() answered (or not)

    /// <summary>The caption under the shape pickers: the engine's values (F1), or the last choice made here when the engine predates F1
    /// (<paramref name="older"/>: it answered -32601) or could not answer; <paramref name="unknown"/> names presets this build does not know.</summary>
    public static string ShapeCaption(bool fromEngine, bool older, IReadOnlyList<string>? unknown = null)
    {
        var line = fromEngine
            ? "Shown: the engine's presets, shared with the desktop, the terminal UI and the phone."
            : older
                ? "Shown: the last choice made here — this engine is older and cannot report its presets; \"Full\" is its default."
                : "Shown: the last choice made here — the engine did not answer; \"Full\" is its default.";
        return unknown is { Count: > 0 } ? $"{line} Not known to this companion: {string.Join(", ", unknown)}." : line;
    }

    /// <summary>The polish model line from <c>voice_settings().polish_model</c> (an id, or null = the chain).</summary>
    public static string PolishModelLine(string? id) =>
        string.IsNullOrWhiteSpace(id) ? "Polish model: Chain (title model, then session)" : $"Polish model: {id}";

    /// <summary>The history caption from <c>voice.history</c>; null (an engine before F1) keeps the engine's documented defaults.</summary>
    public static string HistoryCaption(HistoryConfig? h)
    {
        const string tail = " The agent never reads it, and no channel or other machine can reach it. The switches are in theuth's Settings › Voice.";
        if (h is null)
            return "Kept on this computer only — by default the last 200 dictations and up to 64 MiB of their audio, so a correction can be learned and replayed." + tail;
        if (!h.Enabled)
            return "History is off: no dictation or audio is kept, and corrections are not learned." + tail;
        var learn = h.Learn ? "Corrections are learned." : "Learning from corrections is off.";
        return $"Kept on this computer only — the last {h.Rows} dictations and up to {h.AudioMib} MiB of their audio, so a correction can be replayed. {learn}" + tail;
    }

    /// <summary>The microphone preference list from <c>voice_settings().input_devices</c> when the engine has F1, else from
    /// <c>voice_status().input_devices</c> (wave A) — the same <c>voice.input_devices</c> key either way.</summary>
    public static IReadOnlyList<string> MicPreferences(VoiceSettings? settings, IReadOnlyList<string>? fromStatus) =>
        settings is not null ? settings.InputDevices : fromStatus ?? Array.Empty<string>();
}

/// <summary>
/// The suggestion card's words (153 §6.4, mock frame D) — pure, so the Mac tests pin the copy: header "Dictionary" · "2 times this
/// week"; "You changed ‹heard› → ‹typed› after dictating."; "Heard the same way twice (Companion, 7 days). Add it as a replacement?".
/// </summary>
public static class SuggestionCopy
{
    /// <summary>The card auto-dismisses after this long; it counts as "Not now" without the cooldown — nothing is sent (§6.4).</summary>
    public static readonly TimeSpan AutoDismiss = TimeSpan.FromSeconds(20);

    public const string Header = "Dictionary";

    public const string Play = "▶ play what you said";

    /// <summary>"2 times this week".</summary>
    public static string CountLabel(DictionarySuggestion s) => $"{Times(s.Occurrences)} this week";

    /// <summary>"You changed “parakeet tee dee tee” → “Parakeet TDT” after dictating." (the card draws the heard form struck through).</summary>
    public static string Sentence(DictionarySuggestion s) => $"You changed “{s.Heard}” → “{s.Typed}” after dictating.";

    /// <summary>"Heard the same way twice (Companion, 7 days). Add it as a replacement?".</summary>
    public static string Detail(DictionarySuggestion s) =>
        $"Heard the same way {Times(s.Occurrences, spelled: true)} ({(s.Surface is { } surface ? DictationSettings.SurfaceLabel(surface) : "dictation")}, 7 days). Add it as a replacement?";

    private static string Times(uint n, bool spelled = false) => n switch
    {
        1 => spelled ? "once" : "1 time",
        2 when spelled => "twice",
        _ => $"{n} times",
    };
}

/// <summary>
/// 153 §8.1 for the companion's own recorder: <c>voice.input_devices</c> (read from <c>voice_status.input_devices</c>) is an
/// ordered list of host device names; the first one that is connected wins, else the system default — "one choice serves both
/// paths". The engine's names on Windows are the endpoints' friendly names (cpal over WASAPI), matched case-insensitively.
/// </summary>
public static class MicPreference
{
    /// <summary>The live preference (the app refreshes it at connect and when Settings changes it); empty = the system default.</summary>
    public static IReadOnlyList<string> Names { get; set; } = Array.Empty<string>();

    /// <summary>The index into <paramref name="endpoints"/> (active capture endpoints' friendly names) to open, or null for the default.</summary>
    public static int? Choose(IReadOnlyList<string> preferences, IReadOnlyList<string> endpoints)
    {
        foreach (var want in preferences)
        {
            for (int i = 0; i < endpoints.Count; i++)
            {
                if (string.Equals(endpoints[i], want, StringComparison.OrdinalIgnoreCase)) return i;
            }
        }
        return null;
    }
}

/// <summary>
/// H-A (153-as-built-A): <c>dictionary_list</c> / <c>dictionary_put</c> / <c>dictionary_delete</c> / <c>dictionary_suggestion_answer</c>
/// answer the error "dictionary loading" for ~4 s after the engine starts — retried quietly, never shown as a failure.
/// </summary>
public static class DictionaryRetry
{
    public const string LoadingMessage = "dictionary loading";

    public static bool IsLoading(Exception e) => e is RpcException r && r.Message.Contains(LoadingMessage, StringComparison.OrdinalIgnoreCase);

    /// <summary>Runs <paramref name="call"/>, retrying while the engine says the dictionary is loading (every <paramref name="every"/>, up to <paramref name="attempts"/>).</summary>
    public static async Task<T> Run<T>(Func<Task<T>> call, Func<TimeSpan, Task>? delay = null, int attempts = 8, TimeSpan? every = null)
    {
        var wait = delay ?? (t => Task.Delay(t));
        for (int i = 1; ; i++)
        {
            try { return await call(); }
            catch (Exception e) when (IsLoading(e) && i < attempts) { await wait(every ?? TimeSpan.FromMilliseconds(750)); }
        }
    }

    public static Task Run(Func<Task> call, Func<TimeSpan, Task>? delay = null, int attempts = 8, TimeSpan? every = null) =>
        Run(async () => { await call(); return true; }, delay, attempts, every);
}