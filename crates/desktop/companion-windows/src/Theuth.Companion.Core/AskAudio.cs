using System.Buffers.Binary;

namespace Theuth.Companion.Core;

/// <summary>
/// The Ask bar's audio arithmetic (design 134 §4.3, D7): the engine's dictation wants 16 kHz mono little-endian i16 in
/// ~100 ms pieces (<c>dictation_start(16000)</c> → <c>dictation_audio(id, base64)</c>, design 54 §8). Pure, so the framing
/// is tested on the Mac; the app's <c>VoiceRecorder</c> (NAudio.Wasapi) and the probes feed it.
/// </summary>
public static class AskAudio
{
    /// <summary>What <c>dictation_start</c> is told (never null: null opens the ENGINE's microphone — 130 W6 rule).</summary>
    public const int SampleRate = 16_000;

    /// <summary>100 ms at 16 kHz: one <c>dictation_audio</c> call (Swift <c>VoiceRecorder.chunk</c>).</summary>
    public const int ChunkSamples = 1_600;

    /// <summary>
    /// The synthetic probe's tone (Swift <c>AskBarController.probeDictation</c>): <c>sin(2π·hz·n/16000)·amplitude·32767</c>
    /// truncated toward zero, n counted from <paramref name="start"/> so consecutive chunks join without a click.
    /// </summary>
    public static short[] Tone(int start, int count = ChunkSamples, double hz = 440, double amplitude = 0.3)
    {
        var s = new short[count];
        for (int i = 0; i < count; i++)
        {
            s[i] = (short)(Math.Sin(2 * Math.PI * hz * (start + i) / SampleRate) * amplitude * 32767);
        }
        return s;
    }

    /// <summary>Root mean square of i16 samples on a 0–1 scale (full-scale square wave = 1).</summary>
    public static double Rms(ReadOnlySpan<short> pcm)
    {
        if (pcm.Length == 0) return 0;
        double sum = 0;
        foreach (var v in pcm) sum += (double)v * v;
        return Math.Sqrt(sum / pcm.Length) / 32768.0;
    }

    /// <summary>One float sample (−1…1) as i16, rounded and clipped.</summary>
    public static short ToPcm16(float s)
    {
        var v = Math.Round(s * 32767.0);
        return (short)Math.Clamp(v, short.MinValue, short.MaxValue);
    }

    /// <summary>
    /// Interleaved device samples as floats (−1…1): IEEE float 32, or PCM 8 (unsigned) / 16 / 24 / 32 little-endian —
    /// the shapes a WASAPI mix format takes. A trailing partial sample is ignored.
    /// </summary>
    public static float[] Decode(ReadOnlySpan<byte> bytes, bool isFloat, int bitsPerSample)
    {
        int width = bitsPerSample / 8;
        if (width <= 0) return Array.Empty<float>();
        var n = bytes.Length / width;
        var f = new float[n];
        for (int i = 0; i < n; i++)
        {
            var b = bytes.Slice(i * width, width);
            f[i] = (isFloat, bitsPerSample) switch
            {
                (true, 32) => BinaryPrimitives.ReadSingleLittleEndian(b),
                (true, 64) => (float)BinaryPrimitives.ReadDoubleLittleEndian(b),
                (false, 8) => (b[0] - 128) / 128f,
                (false, 16) => BinaryPrimitives.ReadInt16LittleEndian(b) / 32768f,
                (false, 24) => ((b[2] << 24 | b[1] << 16 | b[0] << 8) >> 8) / 8388608f,
                (false, 32) => BinaryPrimitives.ReadInt32LittleEndian(b) / 2147483648f,
                _ => 0f,
            };
        }
        return f;
    }

    /// <summary>A RIFF/WAVE header's format and where its samples are (PCM, IEEE float, or EXTENSIBLE of either).</summary>
    public sealed record Wav(int SampleRate, int Channels, int BitsPerSample, bool IsFloat, int DataOffset, int DataLength);

    /// <summary>Parse a WAV file (e.g. a <c>SpeechSynthesizer</c> stream); null when it is not one this can read.</summary>
    public static Wav? ParseWav(ReadOnlySpan<byte> riff)
    {
        if (riff.Length < 12 || !riff[..4].SequenceEqual("RIFF"u8) || !riff.Slice(8, 4).SequenceEqual("WAVE"u8)) return null;
        int pos = 12;
        int rate = 0, channels = 0, bits = 0;
        bool isFloat = false, haveFormat = false;
        while (pos + 8 <= riff.Length)
        {
            var id = riff.Slice(pos, 4);
            int size = BinaryPrimitives.ReadInt32LittleEndian(riff.Slice(pos + 4, 4));
            int body = pos + 8;
            if (size < 0) return null;
            if (id.SequenceEqual("fmt "u8) && size >= 16 && body + 16 <= riff.Length)
            {
                int tag = BinaryPrimitives.ReadUInt16LittleEndian(riff.Slice(body, 2));
                channels = BinaryPrimitives.ReadUInt16LittleEndian(riff.Slice(body + 2, 2));
                rate = BinaryPrimitives.ReadInt32LittleEndian(riff.Slice(body + 4, 4));
                bits = BinaryPrimitives.ReadUInt16LittleEndian(riff.Slice(body + 14, 2));
                if (tag == 0xFFFE && size >= 40 && body + 26 <= riff.Length) tag = BinaryPrimitives.ReadUInt16LittleEndian(riff.Slice(body + 24, 2));
                if (tag != 1 && tag != 3) return null;
                isFloat = tag == 3;
                haveFormat = true;
            }
            else if (id.SequenceEqual("data"u8))
            {
                if (!haveFormat || channels == 0 || rate == 0) return null;
                return new Wav(rate, channels, bits, isFloat, body, Math.Min(size, riff.Length - body));
            }
            pos = body + size + (size & 1);
        }
        return null;
    }

    /// <summary>A whole WAV file as 16 kHz mono i16 (the synthetic-speech probe's input), or null when unreadable.</summary>
    public static short[]? Pcm16Mono16k(ReadOnlySpan<byte> wavFile)
    {
        if (ParseWav(wavFile) is not { } w) return null;
        var interleaved = Decode(wavFile.Slice(w.DataOffset, w.DataLength), w.IsFloat, w.BitsPerSample);
        var resampler = new MonoResampler(w.SampleRate, w.Channels);
        var mono = new List<float>(interleaved.Length / Math.Max(1, w.Channels) * SampleRate / Math.Max(1, w.SampleRate) + 16);
        resampler.Push(interleaved, mono);
        return mono.Select(ToPcm16).ToArray();
    }
}

/// <summary>
/// Whole <see cref="AskAudio.ChunkSamples"/>-sample chunks out of appends of any size, in order; <see cref="Flush"/> hands
/// out the tail under one chunk (Swift <c>VoiceRecorder</c>: the release sends the tail too). Not thread-safe — the
/// recorder holds its lock around it.
/// </summary>
public sealed class Pcm16Chunker(int chunk = AskAudio.ChunkSamples)
{
    private short[] buffer = new short[chunk];
    private int count;

    public int Chunk { get; } = chunk;

    /// <summary>Samples waiting for the next whole chunk.</summary>
    public int Pending => count;

    public List<short[]> Append(ReadOnlySpan<short> samples)
    {
        var whole = new List<short[]>();
        while (samples.Length > 0)
        {
            int take = Math.Min(Chunk - count, samples.Length);
            samples[..take].CopyTo(buffer.AsSpan(count));
            count += take;
            samples = samples[take..];
            if (count == Chunk)
            {
                whole.Add(buffer);
                buffer = new short[Chunk];
                count = 0;
            }
        }
        return whole;
    }

    /// <summary>The tail (possibly empty); the chunker is empty afterwards.</summary>
    public short[] Flush()
    {
        var tail = buffer.AsSpan(0, count).ToArray();
        count = 0;
        return tail;
    }

    public void Reset() => count = 0;
}

/// <summary>
/// Interleaved frames at any rate and channel count → mono floats at <paramref name="outputRate"/> (16 kHz), streaming:
/// the channels are averaged, a Blackman-windowed sinc low-pass at 0.45 × the output rate runs at the input rate when
/// it is higher (no aliasing from 48 kHz devices), then linear interpolation at the output spacing. The fallback for an
/// endpoint that cannot convert to 16 kHz mono itself (the app first asks WASAPI's <c>AutoConvertPcm</c>).
/// </summary>
public sealed class MonoResampler
{
    private readonly int channels;
    private readonly double step;
    private readonly float[] taps;
    private readonly float[] history;
    private int historyFill;
    private int historyPos;
    private readonly List<float> filtered = new();
    private double position;

    public MonoResampler(int inputRate, int channels, int outputRate = AskAudio.SampleRate)
    {
        if (inputRate <= 0 || outputRate <= 0 || channels <= 0) throw new ArgumentOutOfRangeException(nameof(inputRate));
        InputRate = inputRate;
        OutputRate = outputRate;
        this.channels = channels;
        step = (double)inputRate / outputRate;
        taps = inputRate > outputRate ? LowPass(0.45 * outputRate / inputRate, 8 * (int)Math.Ceiling(step) + 1) : new[] { 1f };
        history = new float[taps.Length];
    }

    public int InputRate { get; }
    public int OutputRate { get; }

    private static float[] LowPass(double cutoff, int n)
    {
        var h = new double[n];
        int m = n - 1;
        double sum = 0;
        for (int i = 0; i < n; i++)
        {
            double x = i - m / 2.0;
            double sinc = x == 0 ? 2 * cutoff : Math.Sin(2 * Math.PI * cutoff * x) / (Math.PI * x);
            double w = 0.42 - 0.5 * Math.Cos(2 * Math.PI * i / m) + 0.08 * Math.Cos(4 * Math.PI * i / m);
            h[i] = sinc * w;
            sum += h[i];
        }
        return h.Select(v => (float)(v / sum)).ToArray();
    }

    /// <summary>Append the output produced by <paramref name="interleaved"/> (whole frames; a partial frame is dropped).</summary>
    public void Push(ReadOnlySpan<float> interleaved, List<float> output)
    {
        int frames = interleaved.Length / channels;
        for (int f = 0; f < frames; f++)
        {
            float mono = 0;
            for (int c = 0; c < channels; c++) mono += interleaved[f * channels + c];
            mono /= channels;
            history[historyPos] = mono;
            historyPos = (historyPos + 1) % history.Length;
            if (historyFill < history.Length) historyFill++;
            float acc = 0;
            for (int k = 0; k < taps.Length; k++)
            {
                // taps[k] weighs the sample k steps back from the newest (missing history counts as silence)
                int idx = historyPos - 1 - k;
                if (idx < 0) idx += history.Length;
                if (k < historyFill) acc += taps[k] * history[idx];
            }
            filtered.Add(acc);
        }
        while (position + 1 < filtered.Count)
        {
            int i = (int)position;
            double frac = position - i;
            output.Add((float)(filtered[i] + (filtered[i + 1] - filtered[i]) * frac));
            position += step;
        }
        int drop = Math.Min((int)position, filtered.Count);
        if (drop > 0)
        {
            filtered.RemoveRange(0, drop);
            position -= drop;
        }
    }
}
