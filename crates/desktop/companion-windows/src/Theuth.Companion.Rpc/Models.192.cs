using System.Text.Json.Serialization;

namespace Theuth.Companion.Rpc;

/// <summary>
/// Design 192w (the engine's <c>fence_snapshot</c> contract): the main app's design-164 renderer drew a picture fence to a PNG.
/// <c>outcome</c> is <c>done</c> · <c>no_renderer</c> · <c>failed</c> · <c>timed_out</c> · <c>cancelled</c>; <c>png</c> is base64 and null
/// unless done; <c>width</c> / <c>height</c> are the PNG's pixels; <c>error</c> says why a draw failed.
/// </summary>
public sealed record FenceSnapshot
{
    [JsonPropertyName("outcome")] public string Outcome { get; init; } = "";

    [JsonPropertyName("png")] public string? Png { get; init; }

    [JsonPropertyName("width")] public double? Width { get; init; }

    [JsonPropertyName("height")] public double? Height { get; init; }

    [JsonPropertyName("error")] public string? Error { get; init; }
}
