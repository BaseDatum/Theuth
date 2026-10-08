using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.AskBar;

/// <summary><see cref="IDictationCalls"/> over the Store's engine connection (<see cref="AskRpc"/>, design 54 §8).</summary>
internal sealed class DictationCalls(RpcClient client) : IDictationCalls
{
    public Task PrewarmAsync() => client.DictationPrewarmAsync();

    /// <summary>Always 16 000: <c>null</c> would open the engine host's own microphone (130 W6 rule).</summary>
    public Task<string> StartAsync() => client.DictationStartAsync(AskAudio.SampleRate);

    public Task AudioAsync(string id, short[] pcm16) => client.DictationAudioAsync(id, pcm16);

    public Task StopAsync(string id) => client.DictationStopAsync(id);
}
