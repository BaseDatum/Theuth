using System.Text.Json;

namespace Theuth.Companion.Rpc;

/// <summary>
/// Where the local engine is, the way <c>DESK/launch.rs</c> probes it: <c>&lt;data_dir&gt;/engine.json</c> → <c>listen</c>,
/// else <c>127.0.0.1:7777</c>; the token at <c>&lt;data_dir&gt;/token</c>; <c>THEUTH_DATA_DIR</c> overrides <c>~/.theuth</c>
/// (on Windows <c>%USERPROFILE%\.theuth</c> — the profile folder, never <c>C:\Users\&lt;login&gt;</c>; design 134 §2 T1).
/// Port of <c>SWIFT/TheuthKit/Discovery.swift</c>: the same semantics as the macOS companion (D2).
/// </summary>
public sealed record EngineLocation(Uri Url, string Token)
{
    /// <summary><c>$THEUTH_DATA_DIR</c> when set and non-empty, else <c>&lt;home&gt;/.theuth</c>.</summary>
    public static string DataDir(IReadOnlyDictionary<string, string?>? env = null)
    {
        string? d = env is null
            ? Environment.GetEnvironmentVariable("THEUTH_DATA_DIR")
            : env.TryGetValue("THEUTH_DATA_DIR", out var v) ? v : null;
        if (!string.IsNullOrEmpty(d)) return d;
        return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".theuth");
    }

    /// <summary>No usable token in the data dir (the engine has not run there yet).</summary>
    public sealed class NoTokenException(string dataDir) : Exception($"no token in {dataDir}")
    {
        public string DataDirectory { get; } = dataDir;
    }

    /// <summary>Reads the token (required) and <c>engine.json</c>'s <c>listen</c> (optional) from <paramref name="dataDir"/>.</summary>
    public static EngineLocation Discover(string? dataDir = null)
    {
        dataDir ??= DataDir();
        string? token = null;
        try { token = File.ReadAllText(Path.Combine(dataDir, "token")); }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
        token = token?.Trim();
        if (string.IsNullOrEmpty(token)) throw new NoTokenException(dataDir);

        string? listen = null;
        try
        {
            using var doc = JsonDocument.Parse(File.ReadAllBytes(Path.Combine(dataDir, "engine.json")));
            if (doc.RootElement.ValueKind == JsonValueKind.Object
                && doc.RootElement.TryGetProperty("listen", out var l)
                && l.ValueKind == JsonValueKind.String)
            {
                listen = l.GetString();
            }
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonException) { }
        listen ??= "127.0.0.1:7777";
        return new EngineLocation(new Uri($"ws://{listen}/"), token);
    }
}
