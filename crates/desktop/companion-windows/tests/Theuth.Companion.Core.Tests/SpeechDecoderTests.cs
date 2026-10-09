using Concentus;
using Concentus.Enums;

namespace Theuth.Companion.Core.Tests;

// 149 WW3 R3: the engine's SpeechAudio packets decode to non-silent PCM.
public sealed class SpeechDecoderTests
{
    /// <summary>
    /// A 440 Hz tone, 24 kHz mono, encoded with Concentus's encoder. Concentus is libopus 1.1.x: its ENCODER refuses 80 ms frames
    /// (OPUS_BAD_ARG), so these are 20 ms packets; the engine's 80 ms packets are code-3 multi-frame packets every Opus decoder
    /// takes (<see cref="DecodesTheEnginesEightyMsPackets"/> covers them with recorded packets).
    /// </summary>
    private static List<byte[]> TonePackets(int packets, double amplitude = 0.4)
    {
        var enc = OpusCodecFactory.CreateEncoder(SpeechDecoder.DefaultSampleRate, 1, OpusApplication.OPUS_APPLICATION_VOIP);
        enc.Bitrate = 32_000;
        const int Frame = 960;
        var pcm = new short[Frame];
        var outBuf = new byte[4000];
        var list = new List<byte[]>();
        int t = 0;
        for (int p = 0; p < packets; p++)
        {
            for (int i = 0; i < pcm.Length; i++, t++) pcm[i] = (short)(Math.Sin(2 * Math.PI * 440 * t / 24000.0) * amplitude * 32767);
            int n = enc.Encode(pcm, Frame, outBuf, outBuf.Length);
            list.Add(outBuf.AsSpan(0, n).ToArray());
        }
        return list;
    }

    [Fact]
    public void DecodesPacketsToNonSilentPcm()
    {
        var packets = TonePackets(12);
        using var dec = new SpeechDecoder();
        double rmsAll = 0; int count = 0;
        for (int i = 0; i < packets.Count; i++)
        {
            var pcm = dec.Decode(Convert.ToBase64String(packets[i]), (uint)i);
            Assert.Equal(1920, pcm.Length); // 20 ms at the 48 kHz output rate
            if (i >= 2) { rmsAll += SpeechDecoder.Rms(pcm); count++; } // the first frames carry the codec's warm-up
        }
        Assert.Equal(12, dec.Packets);
        Assert.Equal(12L * 1920, dec.Samples);
        Assert.Equal(0, dec.Gaps);
        Assert.True(rmsAll / count > 0.05, $"rms {rmsAll / count}");
    }

    [Fact]
    public void GapsAreConcealedNotThrown()
    {
        var packets = TonePackets(4);
        using var dec = new SpeechDecoder();
        dec.Decode(packets[0], 0);
        var pcm = dec.Decode(packets[3], 3); // 1 and 2 lost
        Assert.Equal(1920, pcm.Length);
        Assert.Equal(1, dec.Gaps);
        Assert.Equal(2, dec.Packets);
    }

    [Fact]
    public void SilenceIsZeroRms() => Assert.Equal(0, SpeechDecoder.Rms(new short[1920]));

    [Fact]
    public void ConcentusTwentyFourKilohertzPathIsWhyWeDecodeAtFortyEight()
    {
        // The finding behind OutputRate: the same packet, two decoders.
        var packets = TonePackets(8);
        var d24 = OpusCodecFactory.CreateDecoder(24000, 1); var o24 = new short[960];
        var d48 = OpusCodecFactory.CreateDecoder(48000, 1); var o48 = new short[1920];
        for (int i = 0; i < 6; i++) { d24.Decode(packets[i], o24, 960, false); d48.Decode(packets[i], o48, 1920, false); }
        Assert.True(SpeechDecoder.Rms(o48) > 0.05, $"48k rms {SpeechDecoder.Rms(o48)}");
        Assert.True(SpeechDecoder.Rms(o48) > 20 * SpeechDecoder.Rms(o24), $"24k rms {SpeechDecoder.Rms(o24)} vs 48k {SpeechDecoder.Rms(o48)}");
    }

    [Fact]
    public void WireDecodesTheThreeKinds()
    {
        var a = SpeechWire.Decode("SpeechAudio", System.Text.Encoding.UTF8.GetBytes("""{"type":"SpeechAudio","id":"u1","session":"s1","seq":7,"opus":"AQID"}"""));
        Assert.NotNull(a);
        Assert.Equal(("u1", "s1", 7u, "AQID"), (a!.Id, a.Session, a.Seq!.Value, a.Opus));
        var s = SpeechWire.Decode("SpeechStarted", System.Text.Encoding.UTF8.GetBytes("""{"type":"SpeechStarted","id":"u1","session":"s1","sample_rate":24000,"chars":12,"sink":"client"}"""));
        Assert.Equal(24000, s!.SampleRate);
        var f = SpeechWire.Decode("SpeechFinished", System.Text.Encoding.UTF8.GetBytes("""{"type":"SpeechFinished","id":"u1","session":"s1","reason":"done"}"""));
        Assert.Equal("done", f!.Reason);
        Assert.Null(SpeechWire.Decode("TextDelta", System.Text.Encoding.UTF8.GetBytes("{}")));
        Assert.Null(SpeechWire.Decode("SpeechAudio", System.Text.Encoding.UTF8.GetBytes("not json")));
    }
    /// <summary>
    /// 65 real packets recorded on build-B (2026-10-09 04:17) from <c>speak(desk, "Testing the Windows speaker…")</c> — the engine's
    /// 80 ms code-3 multi-frame packets, advertised at 24 kHz. Decoded at the companion's 48 kHz: 3840 samples each, clearly not silent.
    /// </summary>
    [Fact]
    public void DecodesTheEnginesEightyMsPackets()
    {
        var path = Path.Combine(AppContext.BaseDirectory, "Fixtures", "speech-packets-build-b.json");
        using var doc = System.Text.Json.JsonDocument.Parse(File.ReadAllText(path));
        var packets = doc.RootElement.GetProperty("packets").EnumerateArray().Select(p => p.GetString()!).ToList();
        Assert.Equal(65, packets.Count);
        using var dec = new SpeechDecoder();
        double rmsSum = 0; int loud = 0;
        for (int i = 0; i < packets.Count; i++)
        {
            var pcm = dec.Decode(packets[i], (uint)i);
            Assert.Equal(SpeechDecoder.FrameSamples, pcm.Length); // 80 ms at 48 kHz
            var rms = SpeechDecoder.Rms(pcm);
            rmsSum += rms;
            if (rms > 0.02) loud++;
        }
        Assert.Equal(0, dec.Gaps);
        Assert.Equal(65L * SpeechDecoder.FrameSamples, dec.Samples);
        Assert.True(rmsSum / packets.Count > 0.03, $"mean rms {rmsSum / packets.Count}"); // build-B logged 0.079
        Assert.True(loud >= 20, $"only {loud} packets above 0.02 rms"); // speech with pauses: most frames carry voice
    }
}
