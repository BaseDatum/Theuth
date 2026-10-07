using System.Reflection;
using System.Text.Json;
using Theuth.Companion.Rpc;

namespace Theuth.Companion;

internal static class AppInfo
{
    /// <summary>The named mutex the Tauri shell's alive check opens (design 134 §5.5, D10).</summary>
    public const string AliveMutexName = @"Local\io.theuth.companion";

    /// <summary>The workspace version (Directory.Build.props reads Cargo.toml), with the commit when the build had one.</summary>
    public static string Version { get; } =
        typeof(AppInfo).Assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion
        ?? typeof(AppInfo).Assembly.GetName().Version?.ToString() ?? "?";

    /// <summary>"Microsoft.WinUI 3.0.0.0" — the runtime line the spikes logged.</summary>
    public static string WinUI { get; } =
        $"{typeof(Microsoft.UI.Xaml.Window).Assembly.GetName().Name} {typeof(Microsoft.UI.Xaml.Window).Assembly.GetName().Version}";

    /// <summary><c>&lt;data_dir&gt;\engine.json</c>'s <c>version</c>, for crash reports; null when unknown.</summary>
    public static string? EngineVersion()
    {
        try
        {
            using var doc = JsonDocument.Parse(File.ReadAllBytes(Path.Combine(EngineLocation.DataDir(), "engine.json")));
            return doc.RootElement.TryGetProperty("version", out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonException)
        {
            return null;
        }
    }
}
