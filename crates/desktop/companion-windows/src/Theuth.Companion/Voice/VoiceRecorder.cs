using System.Runtime.InteropServices;
using Microsoft.UI.Dispatching;
using NAudio.CoreAudioApi;
using NAudio.Wave;
using Theuth.Companion.Core;

namespace Theuth.Companion.Voice;

/// <summary>
/// Microphone → 16 kHz mono i16 PCM in ~100 ms chunks on the UI thread (<see cref="OnChunk"/>) for <c>dictation_audio</c>
/// — the port of Swift's <c>VoiceRecorder</c> on WASAPI through NAudio.Wasapi (design 134 D7, R2-4, §4.3). The default
/// capture endpoint is asked for 16 kHz / mono / 16-bit directly (shared mode, <c>AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM</c>
/// via <c>WasapiRecorderBuilder.WithFormat</c>); when the endpoint refuses, or under <c>--mic-mix-format</c>, it is
/// opened in its mix format and <see cref="MonoResampler"/> converts in-process. Windows has no system microphone
/// prompt: Settings › Privacy › Microphone ("Let desktop apps access your microphone") governs the endpoint, and
/// <c>E_ACCESSDENIED</c> there becomes the "microphone blocked" status with the <c>ms-settings:privacy-microphone</c> link.
/// </summary>
internal sealed class VoiceRecorder
{
    public const string BlockedStatus = "microphone blocked in Settings › Privacy › Microphone";
    public const string PrivacySettingsUri = "ms-settings:privacy-microphone";

    private readonly DispatcherQueue dispatcher;
    private readonly object gate = new();
    private readonly Pcm16Chunker chunker = new();
    private readonly List<float> scratch = new();
    private WasapiRecorder? recorder;
    private MonoResampler? resampler;
    private bool direct;
    private bool isFloat;
    private int bits;
    private bool stopping;

    public VoiceRecorder(DispatcherQueue dispatcher) => this.dispatcher = dispatcher;

    /// <summary>One ~100 ms chunk (1600 samples; the tail on stop may be shorter), on the UI thread.</summary>
    public Action<short[]>? OnChunk { get; set; }

    /// <summary>The capture died after it opened (device unplugged, WASAPI error), on the UI thread.</summary>
    public Action<string>? OnFailed { get; set; }

    /// <summary><c>--mic-mix-format</c>: skip the direct 16 kHz request and always resample in-process.</summary>
    public bool PreferMixFormat { get; set; }

    /// <summary>How the endpoint is open, for the log: device, requested and actual formats, conversion path.</summary>
    public string? Description { get; private set; }

    public bool IsCapturing => recorder is not null;

    /// <summary>Open the default capture endpoint and start streaming; null on success, else why not (shown as the bar's status).</summary>
    public Task<string?> StartAsync() => Task.Run(Start);

    private string? Start()
    {
        MMDevice? device = null;
        try
        {
            using var enumerator = new MMDeviceEnumerator();
            if (!enumerator.TryGetDefaultAudioEndpoint(DataFlow.Capture, Role.Communications, out device) || device is null)
            {
                return "no microphone";
            }
            WaveFormat mix;
            using (var client = device.CreateAudioClient()) mix = client.MixFormat;
            string? directError = null;
            WasapiRecorder? rec = null;
            if (!PreferMixFormat)
            {
                try
                {
                    rec = Open(device, new WaveFormat(AskAudio.SampleRate, 16, 1));
                    direct = true;
                }
                catch (Exception e)
                {
                    directError = e.Message;
                    rec = null;
                }
            }
            if (rec is null)
            {
                rec = Open(device, null);
                direct = false;
            }
            var f = rec.WaveFormat;
            isFloat = f.Encoding == WaveFormatEncoding.IeeeFloat
                || (f is WaveFormatExtensible x && x.SubFormat == AudioMediaSubtypes.MEDIASUBTYPE_IEEE_FLOAT);
            bits = f.BitsPerSample;
            resampler = direct ? null : new MonoResampler(f.SampleRate, f.Channels);
            Description = $"device \"{device.FriendlyName}\" mix={Describe(mix)} capture={Describe(f)} path={(direct ? "wasapi-autoconvert" : "mix-format+MonoResampler")}" +
                          (directError is null ? "" : $" (direct 16 kHz refused: {directError})");
            WasapiRecorder? previous;
            lock (gate)
            {
                chunker.Reset();
                scratch.Clear();
                stopping = false;
                previous = recorder; // (c) idempotent start: a capture still open from an earlier gesture is torn down, never leaked
                recorder = rec;
            }
            if (previous is not null)
            {
                try { previous.StopRecording(); } catch (Exception) { }
                previous.DataAvailable -= OnData;
                previous.RecordingStopped -= OnStopped;
                try { previous.Dispose(); } catch (Exception) { }
            }
            return null;
        }
        catch (COMException e) when ((uint)e.HResult == 0x80070005) // E_ACCESSDENIED: the privacy switch
        {
            Description = $"E_ACCESSDENIED on \"{device?.FriendlyName ?? "?"}\"";
            return BlockedStatus;
        }
        catch (Exception e)
        {
            Description = $"{e.GetType().Name} 0x{e.HResult:X8} on \"{device?.FriendlyName ?? "?"}\": {e.Message}";
            return $"microphone failed to start: {e.Message}";
        }
    }

    private WasapiRecorder Open(MMDevice device, WaveFormat? format)
    {
        var builder = new WasapiRecorderBuilder().WithDevice(device).WithSharedMode().WithEventSync().WithBufferLength(100);
        if (format is not null) builder = builder.WithFormat(format);
        var rec = builder.Build();
        rec.DataAvailable += OnData;
        rec.RecordingStopped += OnStopped;
        try
        {
            rec.StartRecording();
        }
        catch
        {
            rec.DataAvailable -= OnData;
            rec.RecordingStopped -= OnStopped;
            rec.Dispose();
            throw;
        }
        return rec;
    }

    /// <summary>The capture thread: device bytes → mono i16 at 16 kHz → whole chunks to the UI thread.</summary>
    private void OnData(ReadOnlySpan<byte> buffer, AudioClientBufferFlags flags, long devicePosition, long qpcPosition)
    {
        List<short[]> chunks;
        lock (gate)
        {
            if (stopping || recorder is null) return;
            if (direct)
            {
                var samples = MemoryMarshal.Cast<byte, short>(buffer[..(buffer.Length & ~1)]);
                if ((flags & AudioClientBufferFlags.Silent) != 0) chunks = chunker.Append(new short[samples.Length]);
                else chunks = chunker.Append(samples);
            }
            else
            {
                var floats = AskAudio.Decode(buffer, isFloat, bits);
                if ((flags & AudioClientBufferFlags.Silent) != 0) Array.Clear(floats);
                scratch.Clear();
                resampler!.Push(floats, scratch);
                var pcm = new short[scratch.Count];
                for (int i = 0; i < pcm.Length; i++) pcm[i] = AskAudio.ToPcm16(scratch[i]);
                chunks = chunker.Append(pcm);
            }
        }
        if (chunks.Count == 0) return;
        dispatcher.TryEnqueue(() =>
        {
            foreach (var c in chunks) OnChunk?.Invoke(c);
        });
    }

    private void OnStopped(object? sender, StoppedEventArgs e)
    {
        if (e.Exception is not { } ex) return;
        bool live;
        lock (gate) live = !stopping && recorder is not null;
        if (!live) return;
        var why = $"microphone stopped: {ex.Message}";
        dispatcher.TryEnqueue(() =>
        {
            TearDown();
            OnFailed?.Invoke(why);
        });
    }

    /// <summary>Stop; the tail under one chunk goes out too (UI thread).</summary>
    public void Stop()
    {
        short[] tail;
        lock (gate)
        {
            stopping = true;
            tail = chunker.Flush();
        }
        TearDown();
        if (tail.Length > 0) OnChunk?.Invoke(tail);
    }

    /// <summary>Stop and drop whatever is buffered (UI thread).</summary>
    public void Cancel()
    {
        lock (gate)
        {
            stopping = true;
            chunker.Reset();
        }
        TearDown();
    }

    private void TearDown()
    {
        WasapiRecorder? rec;
        lock (gate)
        {
            rec = recorder;
            recorder = null;
        }
        if (rec is null) return;
        try { rec.StopRecording(); } catch (Exception) { }
        rec.DataAvailable -= OnData;
        rec.RecordingStopped -= OnStopped;
        try { rec.Dispose(); } catch (Exception) { }
    }

    public static string Describe(WaveFormat f) =>
        $"{f.SampleRate} Hz/{f.Channels} ch/{f.BitsPerSample}-bit {(f is WaveFormatExtensible x ? (x.SubFormat == AudioMediaSubtypes.MEDIASUBTYPE_IEEE_FLOAT ? "float" : x.SubFormat == AudioMediaSubtypes.MEDIASUBTYPE_PCM ? "pcm" : x.SubFormat.ToString()) : f.Encoding.ToString().ToLowerInvariant())}";
}
