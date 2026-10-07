using System.Runtime.InteropServices;
using System.Text.Json;
using Microsoft.UI.Xaml;

namespace Theuth.Companion;

/// <summary>
/// Crash reporting (design 134 §5.5): an unhandled exception writes <c>companion-&lt;time&gt;.crash.json</c> (exception,
/// stack, version, engine version) beside the log. No Windows Error Reporting LocalDumps (per-machine registry), no telemetry.
/// </summary>
internal static class CrashReport
{
    public static void Install(Application app)
    {
        AppDomain.CurrentDomain.UnhandledException += (_, e) => Write(e.ExceptionObject as Exception, "AppDomain.UnhandledException");
        app.UnhandledException += (_, e) => Write(e.Exception, "Application.UnhandledException");
        TaskScheduler.UnobservedTaskException += (_, e) =>
        {
            Log.Write($"unobserved task exception: {e.Exception.GetBaseException().Message}");
            e.SetObserved();
        };
    }

    private static void Write(Exception? ex, string source)
    {
        try
        {
            Directory.CreateDirectory(Log.Dir);
            var path = Path.Combine(Log.Dir, $"companion-{DateTime.Now:yyyyMMdd-HHmmss}.crash.json");
            using (var file = File.Create(path))
            using (var w = new Utf8JsonWriter(file, new JsonWriterOptions { Indented = true }))
            {
                w.WriteStartObject();
                w.WriteString("source", source);
                w.WriteString("time", DateTimeOffset.Now.ToString("O"));
                w.WriteString("version", AppInfo.Version);
                w.WriteString("engine_version", AppInfo.EngineVersion());
                w.WriteString("os", RuntimeInformation.OSDescription);
                w.WriteString("type", ex?.GetType().FullName);
                w.WriteString("message", ex?.Message);
                w.WriteString("hresult", ex is null ? null : $"0x{ex.HResult:X8}");
                w.WriteString("stack", ex?.ToString());
                w.WriteEndObject();
            }
            Log.Write($"crash ({source}): {ex?.GetType().Name}: {ex?.Message} — {path}");
        }
        catch (Exception)
        {
            // Nothing left to report to.
        }
    }
}
