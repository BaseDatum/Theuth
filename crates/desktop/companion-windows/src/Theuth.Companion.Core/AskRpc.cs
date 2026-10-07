using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>The Ask bar's calls (W3/W6 on the Mac; W4 here), positional, in <c>crates/rpc/src/api.rs</c> order.</summary>
public static class AskRpc
{
    /// <summary><c>steer(session, text, null, [], null)</c> → the input id.</summary>
    public static Task<string> SteerAsync(this RpcClient c, string session, string text) =>
        c.CallAsync<string>("steer", session, text, null, Array.Empty<string>(), null);

    public static Task<MeshStatus> MeshStatusAsync(this RpcClient c) => c.CallAsync<MeshStatus>("mesh_status");

    /// <summary><c>desk_open(name)</c> → the desk's session; refused off the hub.</summary>
    public static Task<string> DeskOpenAsync(this RpcClient c, string name) => c.CallAsync<string>("desk_open", name);

    /// <summary><c>activity_list({since_ms, kinds, limit})</c>, newest first.</summary>
    public static Task<ValueList<ActivityEntry>> ActivityListAsync(this RpcClient c, ActivityFilter filter) =>
        c.CallAsync<ValueList<ActivityEntry>>("activity_list", filter);

    /// <summary><c>set_event_focus(sessions)</c>: delta-class events for these sessions only (129 §3.2); pre-129 engines answer -32601.</summary>
    public static Task SetEventFocusAsync(this RpcClient c, IReadOnlyList<string> sessions) =>
        c.CallRawAsync("set_event_focus", sessions.ToArray());

    // MARK: live dictation (W6; design 54 §8)

    /// <summary>Load the streaming model once after connect; never opens a microphone. The reply is the model state.</summary>
    public static Task DictationPrewarmAsync(this RpcClient c) => c.CallRawAsync("dictation_prewarm");

    /// <summary><c>dictation_start(Some(16000))</c>: the companion streams its own mono i16 PCM. <c>None</c> would open the ENGINE's mic — never send null.</summary>
    public static Task<string> DictationStartAsync(this RpcClient c, uint sampleRate = 16_000) =>
        c.CallAsync<string>("dictation_start", sampleRate);

    /// <summary>~100 ms of little-endian mono i16 samples, base64.</summary>
    public static Task DictationAudioAsync(this RpcClient c, string id, ReadOnlySpan<short> pcm16) =>
        c.CallRawAsync("dictation_audio", id, Convert.ToBase64String(Pcm16LittleEndian(pcm16)));

    /// <summary>Flushes the open utterance; <c>DictationFinished</c> follows as an event. Idempotent.</summary>
    public static Task DictationStopAsync(this RpcClient c, string id) => c.CallRawAsync("dictation_stop", id);

    /// <summary>The samples as little-endian bytes (Windows on x64/arm64 is little-endian; written explicitly anyway).</summary>
    public static byte[] Pcm16LittleEndian(ReadOnlySpan<short> pcm16)
    {
        var bytes = new byte[pcm16.Length * 2];
        for (int i = 0; i < pcm16.Length; i++)
        {
            System.Buffers.Binary.BinaryPrimitives.WriteInt16LittleEndian(bytes.AsSpan(i * 2), pcm16[i]);
        }
        return bytes;
    }
}
