using System.Text;
using NAudio.CoreAudioApi;
using NAudio.Wave;
using Theuth.Companion.Core;

namespace Theuth.Companion.PointAndSay;

/// <summary>
/// 149 WW3 R3 — plays the engine's client-sink speech on Windows. <c>SpeechStarted</c> for a session this player follows opens
/// WASAPI shared-mode output (<see cref="WasapiOut"/>, 60 ms) over a <see cref="BufferedWaveProvider"/> at the decoder's 48 kHz
/// mono PCM16; every <c>SpeechAudio</c> packet is decoded with Concentus and appended; <c>SpeechFinished</c> lets the buffer
/// drain, then closes the device. <see cref="Stop"/> (Esc, a new gesture) cuts at once and <c>stop_speaking</c> is the caller's.
/// Everything runs on the dispatcher thread the store raises events on; the device is opened lazily so a machine without an
/// output renders nothing and logs why. <c>--speech-dump</c> writes every packet (base64) to
/// <c>%LOCALAPPDATA%\theuth\companion\speech-dump.json</c> for the Core fixture.
/// </summary>
internal sealed class SpeechPlayer : IDisposable
{
    private readonly Func<string?> follow; // the session whose speech we play (the desk)
    private readonly List<string> dump = new();
    private SpeechDecoder? decoder;
    private BufferedWaveProvider? buffer;
    private WasapiOut? output;
    private string? utterance;
    private int packets;
    private double rmsSum;
    private bool finished;
    private System.Threading.Timer? drain;

    public SpeechPlayer(Func<string?> follow) => this.follow = follow;

    public bool Dump { get; init; }
    public string? Utterance => utterance;
    /// <summary>"Speakers (Realtek …)" once the device opened, else null.</summary>
    public string? Device { get; private set; }

    /// <summary>Raw-stream hook: returns true when the event was a speech event for the followed session.</summary>
    public bool Handle(string type, ReadOnlySpan<byte> json)
    {
        if (SpeechWire.Decode(type, json) is not { } w) return false;
        var target = follow();
        if (target is null || w.Session != target) return false;
        switch (w.Type)
        {
            case "SpeechStarted":
                Begin(w.Id ?? "", w.SampleRate ?? SpeechDecoder.DefaultSampleRate);
                return true;
            case "SpeechAudio":
                if (w.Id != utterance || w.Opus is null) return true;
                Append(w.Opus, w.Seq ?? 0);
                return true;
            case "SpeechFinished":
                if (w.Id != utterance) return true;
                Finish(w.Reason ?? "?");
                return true;
        }
        return false;
    }

    private void Begin(string id, int advertisedRate)
    {
        Stop();
        utterance = id;
        packets = 0; rmsSum = 0; finished = false; dump.Clear();
        decoder = new SpeechDecoder();
        buffer = new BufferedWaveProvider(new WaveFormat(SpeechDecoder.OutputRate, 16, 1)) { BufferDuration = TimeSpan.FromSeconds(60), DiscardOnBufferOverflow = true };
        try
        {
            using var en = new MMDeviceEnumerator();
            var dev = en.GetDefaultAudioEndpoint(DataFlow.Render, Role.Multimedia);
            Device = dev.FriendlyName;
            output = new WasapiOut(dev, AudioClientShareMode.Shared, true, 60);
            output.Init(buffer);
            output.Play();
            Log.Write($"speech: {id} started (engine rate {advertisedRate} Hz, decoding at {SpeechDecoder.OutputRate} Hz) → device opened \"{Device}\" state={output.PlaybackState}");
        }
        catch (Exception e)
        {
            output?.Dispose(); output = null; Device = null;
            Log.Write($"speech: {id} started but no output device: {e.GetType().Name}: {e.Message} — decoding only");
        }
    }

    private void Append(string opus, uint seq)
    {
        if (decoder is null || buffer is null) return;
        short[] pcm;
        try { pcm = decoder.Decode(opus, seq); }
        catch (Exception e) { Log.Write($"speech: packet {seq} failed to decode: {e.Message}"); return; }
        if (Dump) dump.Add(opus);
        packets++;
        rmsSum += SpeechDecoder.Rms(pcm);
        var bytes = new byte[pcm.Length * 2];
        Buffer.BlockCopy(pcm, 0, bytes, 0, bytes.Length);
        buffer.AddSamples(bytes, 0, bytes.Length);
        if (packets == 1 || packets % 25 == 0)
            Log.Write($"speech: {packets} packets, {pcm.Length} samples each, rms(last)={SpeechDecoder.Rms(pcm):F3} buffered={buffer.BufferedDuration.TotalMilliseconds:F0} ms");
    }

    private void Finish(string reason)
    {
        finished = true;
        double rms = packets > 0 ? rmsSum / packets : 0;
        Log.Write($"speech: {utterance} finished ({reason}) — {packets} packets decoded, mean rms {rms:F3} ({(rms > 0.005 ? "not silent" : "SILENT")}), device={(Device ?? "none")}, gaps={decoder?.Gaps ?? 0}");
        if (Dump && dump.Count > 0) WriteDump();
        // Let the buffer drain, then close the device.
        var ms = (int)Math.Min(30_000, (buffer?.BufferedDuration.TotalMilliseconds ?? 0) + 250);
        drain?.Dispose();
        drain = new System.Threading.Timer(_ => Close(), null, ms, Timeout.Infinite);
    }

    private void WriteDump()
    {
        try
        {
            var dir = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "theuth", "companion");
            Directory.CreateDirectory(dir);
            var path = Path.Combine(dir, "speech-dump.json");
            var sb = new StringBuilder("{\"utterance\":\"").Append(utterance).Append("\",\"packets\":[");
            for (int i = 0; i < dump.Count; i++) { if (i > 0) sb.Append(','); sb.Append('"').Append(dump[i]).Append('"'); }
            sb.Append("]}");
            File.WriteAllText(path, sb.ToString());
            Log.Write($"speech: dumped {dump.Count} packets to {path}");
        }
        catch (Exception e) { Log.Write($"speech: dump failed: {e.Message}"); }
    }

    /// <summary>Cut playback now (Esc / a new gesture); the engine's <c>stop_speaking</c> is sent by the controller.</summary>
    public void Stop()
    {
        if (output is not null || utterance is not null) Log.Write($"speech: stop ({utterance ?? "-"}, {packets} packets{(finished ? ", finished" : "")})");
        Close();
        utterance = null;
    }

    private void Close()
    {
        drain?.Dispose(); drain = null;
        try { output?.Stop(); output?.Dispose(); } catch (Exception) { }
        output = null;
        buffer = null;
        decoder?.Dispose(); decoder = null;
    }

    public void Dispose() => Close();
}
