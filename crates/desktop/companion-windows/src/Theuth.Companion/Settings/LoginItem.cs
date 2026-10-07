using Microsoft.Win32;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Settings;

/// <summary>
/// Launch at login (design 134 D12, §5.5, C7; R2, Sky): the companion writes and removes
/// <c>HKCU\Software\Microsoft\Windows\CurrentVersion\Run\TheuthCompanion = "&lt;exe&gt;" --background</c> itself and
/// mirrors the toggle into <c>&lt;data_dir&gt;\companion.json</c> (<see cref="CompanionSettings.LoginItemJson"/>), which
/// the Tauri shell's <c>companion_login_item_status</c> reads; W5's NSIS uninstall hook deletes the value. The string
/// halves are Core's <see cref="LoginItemRegistration"/> (tested on the Mac); this file is the registry and the file.
/// </summary>
internal static class LoginItem
{
    public static string Exe => Environment.ProcessPath ?? "Theuth.Companion.exe";

    public static string FilePath => Path.Combine(EngineLocation.DataDir(), LoginItemRegistration.FileName);

    /// <summary>The Run value as stored, or null.</summary>
    public static string? ReadRunValue()
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(LoginItemRegistration.RunKey, writable: false);
            return key?.GetValue(LoginItemRegistration.ValueName) as string;
        }
        catch (Exception e) when (e is System.Security.SecurityException or IOException or UnauthorizedAccessException)
        {
            Log.Write($"login item: reading the Run key failed: {e.Message}");
            return null;
        }
    }

    /// <summary><c>enabled</c> when the Run value launches this exe, else <c>not_registered</c> (the shell's words).</summary>
    public static string Status() => LoginItemRegistration.Status(ReadRunValue(), Exe);

    /// <summary>Write (true) or delete (false) the Run value, then mirror the state into companion.json.</summary>
    public static void Apply(bool enabled, CompanionSettings settings)
    {
        try
        {
            using var key = Registry.CurrentUser.CreateSubKey(LoginItemRegistration.RunKey, writable: true);
            if (key is null) throw new IOException("CreateSubKey returned null");
            if (enabled)
            {
                var value = LoginItemRegistration.RunValue(Exe);
                key.SetValue(LoginItemRegistration.ValueName, value, RegistryValueKind.String);
                Log.Write($"login item: wrote HKCU\\{LoginItemRegistration.RunKey}\\{LoginItemRegistration.ValueName} = {value}");
            }
            else
            {
                key.DeleteValue(LoginItemRegistration.ValueName, throwOnMissingValue: false);
                Log.Write($"login item: removed HKCU\\{LoginItemRegistration.RunKey}\\{LoginItemRegistration.ValueName}");
            }
        }
        catch (Exception e) when (e is System.Security.SecurityException or IOException or UnauthorizedAccessException)
        {
            Log.Write($"login item: Run key write failed: {e.Message}");
        }
        Mirror(settings);
    }

    /// <summary><c>&lt;data_dir&gt;\companion.json</c> = <c>{ "launch_at_login": bool }</c>, written atomically.</summary>
    public static void Mirror(CompanionSettings settings)
    {
        var path = FilePath;
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            var tmp = path + ".tmp";
            File.WriteAllText(tmp, settings.LoginItemJson);
            File.Move(tmp, path, overwrite: true);
            Log.Write($"settings: wrote {path} launch_at_login={(settings.LaunchAtLogin ? "true" : "false")}");
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            Log.Write($"settings: companion.json write failed: {e.Message}");
        }
    }

    /// <summary>
    /// At every start (§5.5): a Run value whose exe no longer exists is deleted (the uninstall fallback); with the toggle
    /// on, a missing value or one pointing at another install path is rewritten for this exe; companion.json is written
    /// when it is missing or disagrees with the toggle.
    /// </summary>
    public static void Reconcile(CompanionSettings settings)
    {
        var value = ReadRunValue();
        var exe = LoginItemRegistration.ExeOf(value);
        if (exe is not null && !LoginItemRegistration.PointsAt(value, Exe) && !File.Exists(exe))
        {
            Log.Write($"login item: Run value points at a missing exe ({exe}) — deleting it");
            Apply(false, settings);
            value = null;
        }
        if (settings.LaunchAtLogin && !LoginItemRegistration.PointsAt(value, Exe))
        {
            Log.Write(value is null ? "login item: toggle on but no Run value — writing it" : "login item: Run value is another install — rewriting for this exe");
            Apply(true, settings);
            return;
        }
        string? current = null;
        try { if (File.Exists(FilePath)) current = File.ReadAllText(FilePath); }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException) { }
        if (current != settings.LoginItemJson) Mirror(settings);
        Log.Write($"login item: status={Status()} run={(value is null ? "absent" : "present")} file={FilePath}");
    }
}
