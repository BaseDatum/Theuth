using System.Globalization;
using System.Text;
using System.Text.Json;

namespace Theuth.Companion.Core;

/// <summary>
/// The persisted key-value store behind <see cref="CompanionSettings"/> — on Windows a JSON object in
/// <c>%LOCALAPPDATA%\theuth\companion\settings.json</c> (design 134 D12; no <c>UserDefaults</c>). The getters keep
/// <c>UserDefaults</c>' semantics, so the Swift load/save logic ports as is: <see cref="Bool"/> and <see cref="Double"/>
/// read a missing key as false / 0, <see cref="String"/> and <see cref="Object"/> as null.
/// </summary>
public sealed class SettingsStore
{
    private readonly Dictionary<string, object> values = new(StringComparer.Ordinal);

    /// <summary>The backing file; null for an in-memory store (tests).</summary>
    public string? Path { get; }

    public SettingsStore(string? path = null) => Path = path;

    /// <summary><c>%LOCALAPPDATA%\theuth\companion\settings.json</c>.</summary>
    public static string DefaultPath =>
        System.IO.Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "theuth", "companion", "settings.json");

    /// <summary>Reads <paramref name="path"/> when it exists; a missing or unreadable file is an empty store.</summary>
    public static SettingsStore Load(string path)
    {
        var store = new SettingsStore(path);
        try
        {
            if (!File.Exists(path)) return store;
            using var doc = JsonDocument.Parse(File.ReadAllBytes(path));
            if (doc.RootElement.ValueKind != JsonValueKind.Object) return store;
            foreach (var p in doc.RootElement.EnumerateObject())
            {
                switch (p.Value.ValueKind)
                {
                    case JsonValueKind.String: store.values[p.Name] = p.Value.GetString()!; break;
                    case JsonValueKind.True: store.values[p.Name] = true; break;
                    case JsonValueKind.False: store.values[p.Name] = false; break;
                    case JsonValueKind.Number: store.values[p.Name] = p.Value.GetDouble(); break;
                    case JsonValueKind.Null: break;
                    default: store.values[p.Name] = p.Value.Clone(); break; // kept verbatim for a later wave
                }
            }
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonException) { }
        return store;
    }

    /// <summary>Writes the file (atomically: a temp file, then a replace) when the store has one.</summary>
    public void Flush()
    {
        if (Path is null) return;
        var dir = System.IO.Path.GetDirectoryName(Path);
        if (!string.IsNullOrEmpty(dir)) Directory.CreateDirectory(dir);
        var tmp = Path + ".tmp";
        using (var file = File.Create(tmp))
        using (var w = new Utf8JsonWriter(file, new JsonWriterOptions { Indented = true }))
        {
            w.WriteStartObject();
            foreach (var (k, v) in values.OrderBy(kv => kv.Key, StringComparer.Ordinal))
            {
                w.WritePropertyName(k);
                switch (v)
                {
                    case string s: w.WriteStringValue(s); break;
                    case bool b: w.WriteBooleanValue(b); break;
                    case double d: w.WriteNumberValue(d); break;
                    case JsonElement e: e.WriteTo(w); break;
                }
            }
            w.WriteEndObject();
        }
        File.Move(tmp, Path, overwrite: true);
    }

    /// <summary>The stored value (string · bool · double · JsonElement), or null.</summary>
    public object? Object(string key) => values.TryGetValue(key, out var v) ? v : null;

    /// <summary>A string; a number reads as its text (as <c>UserDefaults.string(forKey:)</c>); anything else null.</summary>
    public string? String(string key) => Object(key) switch
    {
        string s => s,
        double d => d.ToString(CultureInfo.InvariantCulture),
        _ => null,
    };

    /// <summary>true for <c>true</c>, a non-zero number, or "yes"/"true"/a non-zero integer string; false otherwise (and when missing).</summary>
    public bool Bool(string key) => Object(key) switch
    {
        bool b => b,
        double d => d != 0,
        string s => s.Equals("yes", StringComparison.OrdinalIgnoreCase) || s.Equals("true", StringComparison.OrdinalIgnoreCase)
                    || (long.TryParse(s, NumberStyles.Integer, CultureInfo.InvariantCulture, out var n) && n != 0),
        _ => false,
    };

    /// <summary>A number, true = 1, a numeric string; 0 otherwise (and when missing).</summary>
    public double Double(string key) => Object(key) switch
    {
        double d => d,
        bool b => b ? 1 : 0,
        string s when double.TryParse(s, NumberStyles.Float, CultureInfo.InvariantCulture, out var d) => d,
        _ => 0,
    };

    public void Set(string key, string value) => values[key] = value;

    public void Set(string key, bool value) => values[key] = value;

    public void Set(string key, double value) => values[key] = value;

    public void Remove(string key) => values.Remove(key);

    public bool Contains(string key) => values.ContainsKey(key);
}

/// <summary>
/// Every Settings-window value (130 §4.6, 134 §4.6), one store key each. W1's <c>undoDelay</c> and <c>railSide</c> keys
/// are kept verbatim so the Store and the rail keep reading them. Port of <c>CompanionSettings</c>
/// (<c>SWIFT/CompanionCore/PolicySettings.swift</c>); W3 owns its Settings window and launch-at-login wiring.
/// </summary>
public sealed class CompanionSettings : IEquatable<CompanionSettings>
{
    public string RailSide { get; set; } = "right";

    /// <summary>The display the rail sits on (its device name); empty = the primary display (Swift: <c>NSScreen.localizedName</c>).</summary>
    public string RailDisplay { get; set; } = "";

    /// <summary>W6 (R2-3): expanded · sliver when quiet (W1's default) · always a sliver.</summary>
    public RailPresence RailPresence { get; set; } = RailPresence.SliverWhenQuiet;

    /// <summary>OFF by default: turning it on installs the low-level keyboard hook the double-tap Alt gesture needs
    /// (design 134 §3, D6; Swift's <c>doubleTapOption</c>, which asks for Accessibility on the Mac).</summary>
    public bool DoubleTapAlt { get; set; }

    /// <summary>Seconds, 0–5 (clamped on save and load).</summary>
    public double UndoDelay { get; set; } = 3;

    public NotificationPrefs Notify { get; set; } = new();

    public bool LaunchAtLogin { get; set; }

    /// <summary>W6 (R2-4): every key binding's override, one table for the grammar, the legend and Settings › Keys.</summary>
    public KeyTable Keys { get; set; } = new();

    /// <summary>134 W6 item 1: the exe path the first-run "pin theuth to the taskbar corner" tip was shown (or resolved as
    /// already promoted) for; empty = never (<see cref="PinTip"/>). Not a Settings-window value.</summary>
    public string PinTipShownFor { get; set; } = "";

    /// <summary>The store keys (Swift's <c>CompanionSettings.Key</c> raw values; <c>doubleTapAlt</c> replaces <c>doubleTapOption</c>).</summary>
    public static class Key
    {
        public const string RailSide = "railSide", RailDisplay = "railDisplay", RailCollapse = "railCollapse";
        public const string RailPresence = "railPresence", DoubleTapAlt = "doubleTapAlt", UndoDelay = "undoDelay";
        public const string NotifyBlocking = "notifyBlocking", NotifyDeferrable = "notifyDeferrable", NotifyError = "notifyError";
        public const string SoundOnErrors = "soundOnErrors", LaunchAtLogin = "launchAtLogin", KeyOverrides = "keyOverrides";

        /// <summary>Windows only (design 134 D8, R2): Settings › Notifications "Errors break through Do not disturb" → <see cref="NotificationPrefs.TimeSensitiveAvailable"/>.</summary>
        public const string ErrorsBreakThroughDnd = "errorsBreakThroughDnd";

        /// <summary>Windows only (134 W6): <see cref="PinTip.Key"/>.</summary>
        public const string PinTipShownFor = PinTip.Key;

        public static readonly IReadOnlyList<string> All = new[]
        {
            RailSide, RailDisplay, RailCollapse, RailPresence, DoubleTapAlt, UndoDelay,
            NotifyBlocking, NotifyDeferrable, NotifyError, SoundOnErrors, LaunchAtLogin, KeyOverrides, ErrorsBreakThroughDnd, PinTipShownFor,
        };
    }

    public static CompanionSettings Load(SettingsStore d)
    {
        var s = new CompanionSettings();
        s.RailSide = d.String(Key.RailSide) ?? s.RailSide;
        s.RailDisplay = d.String(Key.RailDisplay) ?? "";
        // W2's "collapse when quiet" toggle (`railCollapse`) migrates: off → expanded.
        if (RailPresenceText.FromRaw(d.String(Key.RailPresence)) is { } p) s.RailPresence = p;
        else if (d.Object(Key.RailCollapse) is not null && !d.Bool(Key.RailCollapse)) s.RailPresence = RailPresence.Expanded;
        s.DoubleTapAlt = d.Bool(Key.DoubleTapAlt);
        if (d.Object(Key.UndoDelay) is double v) s.UndoDelay = Math.Min(5, Math.Max(0, v));
        var n = s.Notify;
        n.Blocking = NotifyModeText.FromRaw(d.String(Key.NotifyBlocking)) ?? n.Blocking;
        n.Deferrable = NotifyModeText.FromRaw(d.String(Key.NotifyDeferrable)) ?? n.Deferrable;
        n.Error = NotifyModeText.FromRaw(d.String(Key.NotifyError)) ?? n.Error;
        if (d.Object(Key.SoundOnErrors) is not null) n.SoundOnErrors = d.Bool(Key.SoundOnErrors);
        n.TimeSensitiveAvailable = d.Bool(Key.ErrorsBreakThroughDnd); // off by default (R2, Sky)
        s.Notify = n;
        s.LaunchAtLogin = d.Bool(Key.LaunchAtLogin);
        if (d.String(Key.KeyOverrides) is { } j) s.Keys = new KeyTable(j);
        s.PinTipShownFor = d.String(Key.PinTipShownFor) ?? "";
        return s;
    }

    /// <summary>Writes every key and flushes a file-backed store.</summary>
    public void Save(SettingsStore d)
    {
        d.Set(Key.RailSide, RailSide);
        d.Set(Key.RailDisplay, RailDisplay);
        d.Set(Key.RailPresence, RailPresence.RawValue());
        d.Set(Key.DoubleTapAlt, DoubleTapAlt);
        d.Set(Key.UndoDelay, Math.Min(5, Math.Max(0, UndoDelay)));
        d.Set(Key.NotifyBlocking, Notify.Blocking.RawValue());
        d.Set(Key.NotifyDeferrable, Notify.Deferrable.RawValue());
        d.Set(Key.NotifyError, Notify.Error.RawValue());
        d.Set(Key.SoundOnErrors, Notify.SoundOnErrors);
        d.Set(Key.ErrorsBreakThroughDnd, Notify.TimeSensitiveAvailable);
        d.Set(Key.LaunchAtLogin, LaunchAtLogin);
        d.Set(Key.KeyOverrides, Keys.Json);
        if (PinTipShownFor.Length > 0) d.Set(Key.PinTipShownFor, PinTipShownFor);
        else d.Remove(Key.PinTipShownFor);
        d.Flush();
    }

    /// <summary><c>&lt;data_dir&gt;\companion.json</c>, read by the Tauri shell (<c>companion_login_item_status</c>, R1-2; 134 D12).</summary>
    public string LoginItemJson => $"{{ \"launch_at_login\": {(LaunchAtLogin ? "true" : "false")} }}\n";

    public CompanionSettings Clone()
    {
        var c = (CompanionSettings)MemberwiseClone();
        c.Keys = Keys.Clone();
        return c;
    }

    public bool Equals(CompanionSettings? o) =>
        o is not null
        && RailSide == o.RailSide && RailDisplay == o.RailDisplay && RailPresence == o.RailPresence
        && DoubleTapAlt == o.DoubleTapAlt && UndoDelay.Equals(o.UndoDelay) && Notify == o.Notify
        && LaunchAtLogin == o.LaunchAtLogin && Keys == o.Keys && PinTipShownFor == o.PinTipShownFor;

    public override bool Equals(object? obj) => obj is CompanionSettings s && Equals(s);

    public override int GetHashCode() =>
        HashCode.Combine(RailSide, RailDisplay, RailPresence, DoubleTapAlt, UndoDelay, Notify, LaunchAtLogin, HashCode.Combine(Keys, PinTipShownFor));
}

/// <summary>
/// Launch at login on Windows (design 134 D12, §5.5, C7): the companion itself owns
/// <c>HKCU\Software\Microsoft\Windows\CurrentVersion\Run\TheuthCompanion = "&lt;exe&gt;" --background</c> and mirrors the
/// toggle into <c>&lt;data_dir&gt;\companion.json</c> (<see cref="CompanionSettings.LoginItemJson"/>; the Tauri shell's
/// <c>companion_login_item_status</c> reads it, W5's NSIS uninstall hook deletes the value — keep the value name).
/// The pure halves live here (tested on the Mac); the registry calls are the app's (<c>Settings/LoginItem.cs</c>).
/// </summary>
public static class LoginItemRegistration
{
    public const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";
    public const string ValueName = "TheuthCompanion";
    public const string BackgroundFlag = "--background";
    public const string FileName = "companion.json";

    /// <summary>The Run value for this exe: <c>"C:\…\Theuth.Companion.exe" --background</c>.</summary>
    public static string RunValue(string exePath) => $"\"{exePath}\" {BackgroundFlag}";

    /// <summary>The exe a Run value launches (quoted, or the first token); null for an empty value.</summary>
    public static string? ExeOf(string? runValue)
    {
        if (string.IsNullOrWhiteSpace(runValue)) return null;
        var v = runValue.Trim();
        if (v[0] == '"')
        {
            int end = v.IndexOf('"', 1);
            return end > 1 ? v[1..end] : null;
        }
        int space = v.IndexOf(' ');
        return space > 0 ? v[..space] : v;
    }

    /// <summary>True when the value launches <paramref name="exePath"/> (case-insensitive, as NTFS).</summary>
    public static bool PointsAt(string? runValue, string exePath) =>
        ExeOf(runValue) is { } exe && string.Equals(exe.Trim(), exePath.Trim(), StringComparison.OrdinalIgnoreCase);

    /// <summary>What the shell's <c>companion_login_item_status</c> answers: <c>enabled</c> when the value launches this exe.</summary>
    public static string Status(string? runValue, string exePath) => PointsAt(runValue, exePath) ? "enabled" : "not_registered";
}