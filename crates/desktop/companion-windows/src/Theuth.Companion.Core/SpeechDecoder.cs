using Concentus;

namespace Theuth.Companion.Core;

/// <summary>
/// 149 WW3 R3 — the engine's <c>speak</c> is client-sink (api.rs <c>speak</c>: <c>Sink::Client</c>): the audio arrives as
/// <c>SpeechStarted {id, session, sample_rate: 24000, chars}</c> · <c>SpeechAudio {id, session, seq, opus}</c> (one RFC 6716
/// packet, mono, 80 ms = 1920 samples at 24 kHz, base64) · <c>SpeechFinished {id, session, reason}</c> (voice/lib.rs:325–359),
/// and nothing on Windows played it. This decodes the packets with Concentus (pure managed Opus) into 16-bit PCM for the
/// app's NAudio output; <see cref="Rms"/> is the test's and the log's "not silent" check.
/// </summary>
public sealed class SpeechDecoder : IDisposable
{
    /// <summary>What <c>ptts</c> produces (43 §0.5) and <c>SpeechStarted.sample_rate</c> advertises — the ENCODER's rate.</summary>
    public const int DefaultSampleRate = 24_000;

    /// <summary>
    /// The rate this decodes at. Opus packets carry no rate; a decoder may pick any of 8/12/16/24/48 kHz. Concentus 2.2.2's 24 kHz
    /// path returned near-silence for packets its own 24 kHz encoder made (max |s| ≈ 9 of 32767; the same packets decoded at
    /// 48 kHz gave 7 944), so the companion decodes at Opus's native 48 kHz and hands NAudio 48 kHz PCM — the device mixes at 48
    /// kHz on build-B anyway.
    /// </summary>
    public const int OutputRate = 48_000;

    /// <summary>80 ms at <see cref="OutputRate"/>; the decoder is given room for Opus's 120 ms maximum so an odd packet never truncates.</summary>
    public const int FrameSamples = 3840;
    private const int MaxFrameSamples = 5760;

    private readonly IOpusDecoder decoder;
    private readonly short[] scratch = new short[MaxFrameSamples];
    private uint expectedSeq;

    public int SampleRate { get; }
    public int Packets { get; private set; }
    public long Samples { get; private set; }
    /// <summary>Packets that arrived with a gap in <c>seq</c> (concealed by the decoder as silence; logged, never thrown).</summary>
    public int Gaps { get; private set; }

    public SpeechDecoder()
    {
        SampleRate = OutputRate;
        decoder = OpusCodecFactory.CreateDecoder(OutputRate, 1);
    }

    /// <summary>One <c>SpeechAudio.opus</c> packet (base64) → PCM16 mono samples (a copy, sized to what the packet held).</summary>
    public short[] Decode(string opusBase64, uint seq) => Decode(Convert.FromBase64String(opusBase64), seq);

    public short[] Decode(ReadOnlySpan<byte> packet, uint seq)
    {
        if (seq != expectedSeq)
        {
            Gaps++;
            // Packet loss concealment for the missing ones, so the timeline stays 80 ms per seq.
            for (uint s = expectedSeq; s < seq && s - expectedSeq < 8; s++) decoder.Decode(ReadOnlySpan<byte>.Empty, scratch.AsSpan(0, FrameSamples), FrameSamples, false);
        }
        expectedSeq = seq + 1;
        int n = decoder.Decode(packet, scratch, MaxFrameSamples, false);
        Packets++;
        Samples += n;
        return scratch.AsSpan(0, n).ToArray();
    }

    /// <summary>Root-mean-square of the samples, 0…1 of full scale; speech sits well above 0.005, silence at 0.</summary>
    public static double Rms(ReadOnlySpan<short> pcm)
    {
        if (pcm.Length == 0) return 0;
        double acc = 0;
        foreach (var s in pcm) { double v = s / 32768.0; acc += v * v; }
        return Math.Sqrt(acc / pcm.Length);
    }

    public void Dispose() { }
}

/// <summary>The three speech events as the companion reads them off the raw stream (every field optional, never throws).</summary>
public sealed record SpeechWire(string Type, string? Id, string? Session, uint? Seq, string? Opus, int? SampleRate, string? Reason)
{
    public static SpeechWire? Decode(string type, ReadOnlySpan<byte> json)
    {
        if (type is not ("SpeechStarted" or "SpeechAudio" or "SpeechFinished")) return null;
        try
        {
            using var doc = System.Text.Json.JsonDocument.Parse(json.ToArray());
            var r = doc.RootElement;
            string? S(string k) => r.TryGetProperty(k, out var v) && v.ValueKind == System.Text.Json.JsonValueKind.String ? v.GetString() : null;
            uint? U(string k) => r.TryGetProperty(k, out var v) && v.ValueKind == System.Text.Json.JsonValueKind.Number && v.TryGetUInt32(out var u) ? u : null;
            int? I(string k) => r.TryGetProperty(k, out var v) && v.ValueKind == System.Text.Json.JsonValueKind.Number && v.TryGetInt32(out var i) ? i : null;
            return new SpeechWire(type, S("id"), S("session"), U("seq"), S("opus"), I("sample_rate"), S("reason"));
        }
        catch (Exception) { return null; }
    }
}
