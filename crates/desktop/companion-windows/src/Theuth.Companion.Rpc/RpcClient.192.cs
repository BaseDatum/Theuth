namespace Theuth.Companion.Rpc;

/// <summary>
/// Design 192w: the one picture call. <c>fence_snapshot(lang, source, width, appearance, scale)</c> asks the engine to have a connected
/// renderer client draw a <c>json-render</c> / <c>mermaid</c> / <c>openui</c> fence to a PNG: <paramref name="width"/> in CSS px (the engine
/// clamps 240..1200), <paramref name="appearance"/> <c>dark</c> | <c>light</c>, <paramref name="scale"/> the device pixel ratio (1..3).
/// A draw takes 2–6 s and at most about 20 s; an engine without the method answers <c>-32601</c>.
/// </summary>
public sealed partial class RpcClient
{
    public Task<FenceSnapshot> FenceSnapshotAsync(string lang, string source, int width, string appearance, double scale) =>
        CallAsync<FenceSnapshot>("fence_snapshot", lang, source, width, appearance, scale);
}
