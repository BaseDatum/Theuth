using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.AskBar;

/// <summary><see cref="IDictationCalls"/> over the Store's engine connection (<see cref="AskRpc"/>, design 54 §8; 153 §14.2).</summary>
internal sealed class DictationCalls(RpcClient client) : IDictationCalls
{
    public Task PrewarmAsync() => client.DictationPrewarmAsync();

    /// <summary>Always 16 000: <c>null</c> would open the engine host's own microphone (130 W6 rule).</summary>
    public Task<string> StartAsync() => client.DictationStartAsync(AskAudio.SampleRate);

    /// <summary>
    /// 153 §14.1: <c>dictation_start(16000, opts)</c> — the companion surface, the agent destination, the press offset. An engine
    /// that refuses the trailing param (<c>-32602</c>, before 153) gets the 149 call, so dictation never breaks on an old engine.
    /// </summary>
    public async Task<string> StartAsync(DictationOpts opts)
    {
        try
        {
            return await client.DictationStartAsync(AskAudio.SampleRate, opts);
        }
        catch (RpcException e) when (e.Code == -32602)
        {
            Log.Write($"dictation_start: the engine refused the 153 opts ({e}) — starting without them");
            return await client.DictationStartAsync(AskAudio.SampleRate);
        }
    }

    public Task AudioAsync(string id, short[] pcm16) => client.DictationAudioAsync(id, pcm16);

    public Task StopAsync(string id) => client.DictationStopAsync(id);

    /// <summary>153 §6.1: the text actually sent after dictation <paramref name="id"/>; an engine before 153 (<c>-32601</c>) learns nothing.</summary>
    public async Task CorrectionAsync(string id, string sentText)
    {
        try
        {
            await client.DictationCorrectionAsync(id, sentText);
            Log.Write($"dictation_correction({id}, {sentText.Length} chars) ok");
        }
        catch (RpcException e) when (e.Code == -32601)
        {
            Log.Write("dictation_correction: the engine has no 153 history (-32601) — nothing reported");
        }
    }
}
