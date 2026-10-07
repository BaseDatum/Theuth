namespace SpikeHost;

internal static class Build
{
#if WASDK2
    public const string WasdkLine = "2.x (-p:WasdkLine=2)";
#else
    public const string WasdkLine = "1.8 (default)";
#endif
}
