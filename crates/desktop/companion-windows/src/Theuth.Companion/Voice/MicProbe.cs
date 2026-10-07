using System.Globalization;
using Microsoft.UI.Dispatching;
using NAudio.CoreAudioApi;
using Theuth.Companion.AskBar;
using Theuth.Companion.Core;

namespace Theuth.Companion.Voice;

/// <summary>
/// <c>--mic-probe N [--mic-mix-format]</c> (design 134 §7 W4, C8: "a real mic capture in the RDP session logs non-zero
/// RMS"): list the capture endpoints, open the default one through <see cref="VoiceRecorder"/> for N s, log the format
/// path and the RMS per second, then exit 0. Needs no engine.
/// </summary>
internal static class MicProbe
{
    public static void Run(DispatcherQueue dispatcher, double seconds, bool mixFormat, Action<int> quit)
    {
        AskLog.Start();
        AskLog.Write($"mic-probe start seconds={Flags.Describe(seconds)} mix-format={(mixFormat ? "true" : "false")} user={Environment.UserName} interactive={(Environment.UserInteractive ? "true" : "false")} session={System.Diagnostics.Process.GetCurrentProcess().SessionId}");
        try
        {
            using var enumerator = new MMDeviceEnumerator();
            using var all = enumerator.EnumerateAudioEndPoints(DataFlow.Capture, DeviceState.All);
            AskLog.Write($"mic-probe capture endpoints: {all.Count}");
            foreach (var d in all)
            {
                string mix;
                try { mix = d.State == DeviceState.Active ? VoiceRecorder.Describe(d.AudioClient.MixFormat) : "-"; }
                catch (Exception e) { mix = $"(mix format unavailable: {e.Message})"; }
                AskLog.Write($"mic-probe endpoint {d.State} \"{d.FriendlyName}\" id={d.ID} mix={mix}");
            }
            if (enumerator.TryGetDefaultAudioEndpoint(DataFlow.Capture, Role.Communications, out var def) && def is not null)
            {
                AskLog.Write($"mic-probe default (communications) = \"{def.FriendlyName}\"");
            }
            else
            {
                AskLog.Write("mic-probe default (communications) = none");
            }
        }
        catch (Exception e)
        {
            AskLog.Write($"mic-probe enumeration failed: {e.GetType().Name}: {e.Message}");
        }

        var recorder = new VoiceRecorder(dispatcher) { PreferMixFormat = mixFormat };
        long chunks = 0, samples = 0, secondChunks = 0, secondSamples = 0;
        double sumSq = 0, secondSumSq = 0, maxRms = 0, peak = 0;
        int second = 0;
        recorder.OnChunk = c =>
        {
            chunks++;
            secondChunks++;
            samples += c.Length;
            secondSamples += c.Length;
            foreach (var v in c)
            {
                sumSq += (double)v * v;
                secondSumSq += (double)v * v;
                peak = Math.Max(peak, Math.Abs(v) / 32768.0);
            }
        };
        recorder.OnFailed = why => AskLog.Write($"mic-probe failed: {why}");
        _ = recorder.StartAsync().ContinueWith(t =>
        {
            AskLog.Write($"mic-probe open: {(t.Result ?? "ok")} — {recorder.Description ?? "?"}");
        }, TaskScheduler.FromCurrentSynchronizationContext());

        var tick = dispatcher.CreateTimer();
        tick.Interval = TimeSpan.FromSeconds(1);
        tick.IsRepeating = true;
        tick.Tick += (_, _) =>
        {
            second++;
            var rms = secondSamples == 0 ? 0 : Math.Sqrt(secondSumSq / secondSamples) / 32768.0;
            maxRms = Math.Max(maxRms, rms);
            AskLog.Write($"mic-probe t={second}s chunks={secondChunks} samples={secondSamples} rms={F(rms)} peak={F(peak)}");
            secondChunks = 0;
            secondSamples = 0;
            secondSumSq = 0;
        };
        tick.Start();

        var done = dispatcher.CreateTimer();
        done.Interval = TimeSpan.FromSeconds(seconds);
        done.IsRepeating = false;
        done.Tick += (_, _) =>
        {
            tick.Stop();
            recorder.Stop();
            var rms = samples == 0 ? 0 : Math.Sqrt(sumSq / samples) / 32768.0;
            AskLog.Write($"mic-probe done after {Flags.Describe(seconds)} s: chunks={chunks} samples={samples} ({samples / 16000.0:F2} s of 16 kHz audio) rms={F(rms)} max-rms(1 s)={F(maxRms)} peak={F(peak)} non-zero={(rms > 0 ? "true" : "false")} path={recorder.Description ?? "not opened"} exit 0");
            var linger = dispatcher.CreateTimer();
            linger.Interval = TimeSpan.FromSeconds(1);
            linger.IsRepeating = false;
            linger.Tick += (_, _) => quit(0);
            linger.Start();
            Keep.Add(linger);
        };
        done.Start();
        Keep.Add(tick);
        Keep.Add(done);
    }

    private static readonly List<DispatcherQueueTimer> Keep = new();

    private static string F(double v) => v.ToString("F4", CultureInfo.InvariantCulture);
}
