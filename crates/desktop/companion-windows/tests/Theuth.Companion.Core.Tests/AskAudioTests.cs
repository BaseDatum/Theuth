using System.Buffers.Binary;

namespace Theuth.Companion.Core.Tests;

/// <summary>The Ask bar's audio arithmetic (design 134 §4.3, D7): the probe tone, the 100 ms chunker, device formats → 16 kHz mono i16.</summary>
public sealed class AskAudioTests
{
    [Fact]
    public void TheProbeToneIsTheSwiftOne()
    {
        var c = AskAudio.Tone(0);
        Assert.Equal(AskAudio.ChunkSamples, c.Length);
        Assert.Equal(0, c[0]);
        Assert.InRange(c.Max(), 9820, 9831); // 0.3 × 32767
        Assert.InRange(AskAudio.Rms(c), 0.2115, 0.2127); // 0.3 / √2 — the engine's DictationLevel reading on the Mac (130 W6)
        Assert.Equal(AskAudio.Tone(0, 3200)[1600], AskAudio.Tone(1600)[0]); // chunks join without a click
        Assert.Equal(0, AskAudio.Rms(ReadOnlySpan<short>.Empty));
    }

    [Fact]
    public void TheChunkerHandsOutWholeChunksInOrderAndFlushesTheTail()
    {
        var chunker = new Pcm16Chunker();
        var ramp = Enumerable.Range(0, 5000).Select(i => (short)i).ToArray();
        Assert.Empty(chunker.Append(ramp.AsSpan(0, 1000)));
        Assert.Equal(1000, chunker.Pending);
        var one = chunker.Append(ramp.AsSpan(1000, 1000));
        Assert.Single(one);
        Assert.Equal(ramp.AsSpan(0, 1600).ToArray(), one[0]);
        Assert.Equal(400, chunker.Pending);
        var two = chunker.Append(ramp.AsSpan(2000, 3000));
        Assert.Equal(2, two.Count);
        Assert.Equal(ramp.AsSpan(1600, 1600).ToArray(), two[0]);
        Assert.Equal(ramp.AsSpan(3200, 1600).ToArray(), two[1]);
        Assert.Equal(200, chunker.Pending);
        Assert.Equal(ramp.AsSpan(4800, 200).ToArray(), chunker.Flush());
        Assert.Equal(0, chunker.Pending);
        Assert.Empty(chunker.Flush());
    }

    [Fact]
    public void DeviceFormatsDecodeToFloats()
    {
        var pcm16 = new byte[4];
        BinaryPrimitives.WriteInt16LittleEndian(pcm16, 16384);
        BinaryPrimitives.WriteInt16LittleEndian(pcm16.AsSpan(2), -32768);
        Assert.Equal(new[] { 0.5f, -1f }, AskAudio.Decode(pcm16, isFloat: false, bitsPerSample: 16));

        var f32 = new byte[8];
        BinaryPrimitives.WriteSingleLittleEndian(f32, 0.25f);
        BinaryPrimitives.WriteSingleLittleEndian(f32.AsSpan(4), -0.75f);
        Assert.Equal(new[] { 0.25f, -0.75f }, AskAudio.Decode(f32, isFloat: true, bitsPerSample: 32));

        Assert.Equal(new[] { 0f, -1f, 127 / 128f }, AskAudio.Decode(new byte[] { 128, 0, 255 }, isFloat: false, bitsPerSample: 8));

        var pcm24 = new byte[] { 0x00, 0x00, 0x40, 0x00, 0x00, 0x80 }; // 0x400000 = half scale, 0x800000 = -1
        Assert.Equal(new[] { 0.5f, -1f }, AskAudio.Decode(pcm24, isFloat: false, bitsPerSample: 24));

        var pcm32 = new byte[4];
        BinaryPrimitives.WriteInt32LittleEndian(pcm32, int.MinValue / 2);
        Assert.Equal(new[] { -0.5f }, AskAudio.Decode(pcm32, isFloat: false, bitsPerSample: 32));

        Assert.Equal(new[] { 0.5f }, AskAudio.Decode(new byte[] { 0, 64, 1 }, isFloat: false, bitsPerSample: 16)); // a trailing partial sample is ignored
        Assert.Equal(short.MaxValue, AskAudio.ToPcm16(2f));
        Assert.Equal(short.MinValue, AskAudio.ToPcm16(-1.5f));
        Assert.Equal(16384, AskAudio.ToPcm16(0.5f));
    }

    /// <summary>A RIFF/WAVE file: PCM 16 or IEEE float 32, any rate and channel count.</summary>
    private static byte[] Wav(float[] interleaved, int rate, int channels, bool isFloat)
    {
        int bits = isFloat ? 32 : 16;
        int block = channels * bits / 8;
        var data = new byte[interleaved.Length * bits / 8];
        for (int i = 0; i < interleaved.Length; i++)
        {
            if (isFloat) BinaryPrimitives.WriteSingleLittleEndian(data.AsSpan(i * 4), interleaved[i]);
            else BinaryPrimitives.WriteInt16LittleEndian(data.AsSpan(i * 2), AskAudio.ToPcm16(interleaved[i]));
        }
        var ms = new MemoryStream();
        var w = new BinaryWriter(ms);
        w.Write("RIFF"u8);
        w.Write(36 + data.Length);
        w.Write("WAVE"u8);
        w.Write("fmt "u8);
        w.Write(16);
        w.Write((ushort)(isFloat ? 3 : 1));
        w.Write((ushort)channels);
        w.Write(rate);
        w.Write(rate * block);
        w.Write((ushort)block);
        w.Write((ushort)bits);
        w.Write("data"u8);
        w.Write(data.Length);
        w.Write(data);
        return ms.ToArray();
    }

    private static int ZeroCrossings(short[] s)
    {
        int n = 0;
        for (int i = 1; i < s.Length; i++) if ((s[i - 1] < 0) != (s[i] < 0)) n++;
        return n;
    }

    [Fact]
    public void AStereo48kWavBecomes16kMonoWithTheToneIntact()
    {
        const int rate = 48_000;
        var frames = rate / 2; // 0.5 s
        var interleaved = new float[frames * 2];
        for (int i = 0; i < frames; i++)
        {
            var v = (float)(Math.Sin(2 * Math.PI * 440 * i / rate) * 0.3);
            interleaved[2 * i] = v;
            interleaved[2 * i + 1] = v;
        }
        var wav = Wav(interleaved, rate, 2, isFloat: false);
        var parsed = AskAudio.ParseWav(wav);
        Assert.NotNull(parsed);
        Assert.Equal((48_000, 2, 16, false), (parsed!.SampleRate, parsed.Channels, parsed.BitsPerSample, parsed.IsFloat));
        var mono = AskAudio.Pcm16Mono16k(wav);
        Assert.NotNull(mono);
        Assert.InRange(mono!.Length, 7_990, 8_000);
        Assert.InRange(AskAudio.Rms(mono.AsSpan(800)), 0.20, 0.22); // after the filter warms up
        Assert.InRange(ZeroCrossings(mono.AsSpan(800).ToArray()), 390, 400); // 440 Hz × 0.45 s × 2
    }

    [Fact]
    public void AFloat22kMonoWavResamplesUpAndAnUnknownFileIsRefused()
    {
        const int rate = 22_050;
        var samples = Enumerable.Range(0, rate / 10).Select(i => (float)(Math.Sin(2 * Math.PI * 440 * i / rate) * 0.5)).ToArray();
        var mono = AskAudio.Pcm16Mono16k(Wav(samples, rate, 1, isFloat: true));
        Assert.NotNull(mono);
        Assert.InRange(mono!.Length, 1_590, 1_600);
        Assert.InRange(AskAudio.Rms(mono.AsSpan(100)), 0.34, 0.37); // 0.5 / √2
        Assert.Null(AskAudio.Pcm16Mono16k("not a wav at all"u8));
        Assert.Null(AskAudio.ParseWav(new byte[8]));
    }

    [Fact]
    public void TheResamplerPassesSixteenKThroughAndAveragesChannels()
    {
        var same = new MonoResampler(16_000, 2);
        var output = new List<float>();
        same.Push(new float[] { 0.2f, 0.4f, -0.5f, -0.5f, 1f, 0f }, output);
        Assert.Equal(new[] { 0.3f, -0.5f }, output.Select(v => (float)Math.Round(v, 5)).Take(2)); // one input frame is still interpolating
        Assert.Throws<ArgumentOutOfRangeException>(() => new MonoResampler(0, 1));

        var down = new MonoResampler(48_000, 1);
        var dc = new List<float>();
        down.Push(Enumerable.Repeat(0.5f, 4800).ToArray(), dc);
        Assert.InRange(dc.Count, 1_590, 1_600);
        Assert.InRange(dc[^1], 0.49f, 0.51f); // a constant survives the low-pass
    }
}
