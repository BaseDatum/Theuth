using System.Text.Json;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.AnyApp;

/// <summary>
/// 153 §14.3's companion-local keys that any-app delivery reads (H3-CC names): <c>dictateAnywhereEnabled</c> (default on),
/// <c>leaveTextOnClipboard</c> (default off — §13.3 step 5 is skipped when on), <c>checkPasteLanded</c> (default on — §13.4's triple),
/// <c>appProfiles</c> (JSON <c>[AppProfile]</c>, the wire shape). Settings › Dictation (W153B's page, W153D's rows in SettingsWindow.153D.cs) writes them; this reads them live from the ONE
/// settings store (SettingsModel.Shared's), so a change applies at the next gesture.
/// </summary>
internal sealed class AnyAppPrefs
{
    public const string DictateAnywhereEnabledKey = "dictateAnywhereEnabled";
    public const string LeaveTextOnClipboardKey = "leaveTextOnClipboard";
    public const string CheckPasteLandedKey = "checkPasteLanded";
    public const string AppProfilesKey = "appProfiles";
    /// <summary>§8.6 (H3-D2, the Mac's key): toggle Play/Pause at a non-refused press and again at the finish; opt-in, default off.</summary>
    public const string PauseMediaKey = "pauseMediaWhileDictating";
    /// <summary>Windows: injected keystrokes are lossy in Win11 Notepad (live rounds 1, 4, 5), so the lease is the default delivery;
    /// paced direct typing (§13.2) only when this is on.</summary>
    public const string DirectInsertionKey = "directInsertion";

    private readonly SettingsStore store;

    public AnyAppPrefs(SettingsStore store) => this.store = store;

    public bool Enabled => !store.Contains(DictateAnywhereEnabledKey) || store.Bool(DictateAnywhereEnabledKey);

    public bool LeaveTextOnClipboard => store.Contains(LeaveTextOnClipboardKey) && store.Bool(LeaveTextOnClipboardKey);

    public bool CheckPasteLanded => !store.Contains(CheckPasteLandedKey) || store.Bool(CheckPasteLandedKey);
    public bool PauseMedia => store.Contains(PauseMediaKey) && store.Bool(PauseMediaKey);
    public bool DirectInsertion => store.Contains(DirectInsertionKey) && store.Bool(DirectInsertionKey);

    /// <summary>The user's per-app rows (an unreadable value is no rows, never an exception).</summary>
    public IReadOnlyList<AppProfile> Profiles
    {
        get
        {
            string? json = store.Object(AppProfilesKey) switch
            {
                JsonElement e when e.ValueKind == JsonValueKind.Array => e.GetRawText(),
                string s => s,
                _ => null,
            };
            if (json is null) return Array.Empty<AppProfile>();
            try
            {
                return TheuthJson.Deserialize<List<AppProfile>>(json);
            }
            catch (Exception e) when (e is JsonException or NotSupportedException or InvalidOperationException)
            {
                Log.Write($"anyapp: {AppProfilesKey} unreadable ({e.Message}) — no per-app rows");
                return Array.Empty<AppProfile>();
            }
        }
    }

    /// <summary>The user's row for <paramref name="exe"/> (id = the exe name, with or without ".exe"), or null.</summary>
    public AppProfile? Find(string exe) => Profiles.FirstOrDefault(p => AppMatch.Same(p.Id, exe));

    /// <summary>
    /// The row sent with <c>dictation_start</c> / <c>rewrite</c> (H3-D1): the app's own row; else the any_app default row re-labelled with
    /// the app's id and name; else (no rows) a synthesized row with polish and learn off. <c>terminal ||= heuristics</c> in every case.
    /// </summary>
    public AppProfile ProfileFor(string exe, string app)
    {
        bool heuristic = AppMatch.IsTerminal(exe, null);
        var rows = Profiles;
        if (rows.FirstOrDefault(p => AppMatch.Same(p.Id, exe)) is { } own) return own with { Terminal = own.Terminal || heuristic };
        if (rows.FirstOrDefault(p => string.Equals(p.Id, AppMatch.DefaultRowId, StringComparison.OrdinalIgnoreCase)) is { } d)
            return d with { Id = exe, Name = app, Terminal = d.Terminal || heuristic };
        return new AppProfile { Id = exe, Name = app, Terminal = heuristic, Polish = false, Learn = false };
    }

    public string Describe() =>
        $"enabled={(Enabled ? "on" : "off")} leaveTextOnClipboard={(LeaveTextOnClipboard ? "on" : "off")} checkPasteLanded={(CheckPasteLanded ? "on" : "off")} pauseMedia={(PauseMedia ? "on" : "off")} directInsertion={(DirectInsertion ? "on" : "off")} profiles={Profiles.Count}";
}