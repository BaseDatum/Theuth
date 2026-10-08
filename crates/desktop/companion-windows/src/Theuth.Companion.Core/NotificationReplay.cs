using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>
/// The <c>--probe-notify</c> replay (130 W2 <c>NotificationsInstaller.replay</c>; design 134 §7 W3, C6), Windows form:
/// the live items fed to fresh policies as if they had just arrived — once as a whole (flood when &gt; 10), then the
/// first ≤ 10 one at a time (per-class verdicts, the held batch at a breakpoint, the 2-minute cap and rate window,
/// withdrawal), then the engine lost (the error class, Urgent under the Do-not-disturb opt-in). Pure, so the Mac tests
/// pin the C6 reading — exactly one summary banner in the flood segment; the app posts every step's output for real
/// (one toast group per segment) and counts them with <c>AppNotificationManager.GetAllAsync</c>.
/// </summary>
public static class NotificationReplay
{
    public const string SegmentAll = "all";
    public const string SegmentOne = "one";
    public const string SegmentEngine = "engine";

    /// <summary>One policy call of the replay: which segment it belongs to, the Swift log label, what came out.</summary>
    public sealed record Step(string Segment, string Label, PolicyOutput Output);

    public static IReadOnlyList<Step> Run(IReadOnlyList<AttentionItem> items, NotificationPrefs prefs, DateTimeOffset t0)
    {
        var steps = new List<Step>();
        var whole = new NotificationPolicy(prefs);
        _ = whole.Update(Array.Empty<AttentionItem>(), t0);
        steps.Add(new Step(SegmentAll, $"all {items.Count} at once", whole.Update(items, t0)));

        var one = new NotificationPolicy(prefs);
        _ = one.Update(Array.Empty<AttentionItem>(), t0);
        var few = items.Take(NotificationPolicy.FloodThreshold).ToList();
        for (int i = 0; i < few.Count; i++)
        {
            var item = few[i];
            steps.Add(new Step(SegmentOne, $"item {i + 1} {item.Kind.RawValue} ({ClassName(item)})",
                one.Update(few.Take(i + 1).ToList(), t0.AddSeconds(i))));
        }
        steps.Add(new Step(SegmentOne, $"tick +60 s (cap not elapsed, held={one.HeldCount})", one.Tick(t0.AddSeconds(60))));
        steps.Add(new Step(SegmentOne, $"breakpoint +61 s (held={one.HeldCount})", one.Breakpoint(t0.AddSeconds(61))));
        _ = one.Update(few.Take(1).ToList(), t0.AddSeconds(65));
        steps.Add(new Step(SegmentOne, $"items re-added +70 s (held={one.HeldCount})", one.Update(few, t0.AddSeconds(70))));
        steps.Add(new Step(SegmentOne, $"breakpoint +80 s inside the 2-min rate window (held={one.HeldCount})", one.Breakpoint(t0.AddSeconds(80))));
        steps.Add(new Step(SegmentOne, "tick +181 s (rate window passed)", one.Tick(t0.AddSeconds(181))));
        steps.Add(new Step(SegmentOne, "answered elsewhere (all gone)", one.Update(Array.Empty<AttentionItem>(), t0.AddSeconds(182))));
        // Windows addition (134 D8 gate): the error class for real — Default scenario + sound, or Urgent under the opt-in.
        // W6 item 10: the loss is held for NotificationPolicy.EngineGrace; the banner comes from the tick after it.
        var lost = one.Engine(false, t0.AddSeconds(183));
        lost.Merge(one.Tick(t0.AddSeconds(183) + NotificationPolicy.EngineGrace));
        steps.Add(new Step(SegmentEngine, $"engine lost +183 s (+{(int)NotificationPolicy.EngineGrace.TotalSeconds} s grace)", lost));
        return steps;
    }

    /// <summary>Swift's <c>NotificationClass.rawValue</c>: <c>blocking</c> · <c>deferrable</c> · <c>error</c> · <c>never</c>.</summary>
    public static string ClassName(AttentionItem item) => NotificationPolicy.Classify(item).ToString().ToLowerInvariant();

    /// <summary>The Swift replay line for one step: <c>replay &lt;label&gt;: post=[category:id level[ sound], …] withdraw=[…]</c>.</summary>
    public static string Describe(Step s) =>
        $"replay {s.Label}: post=[{string.Join(", ", s.Output.Post.Select(b => $"{Camel(b.Category)}:{b.Id} {Camel(b.Level)}{(b.Sound ? " sound" : "")}"))}]" +
        $" withdraw=[{string.Join(", ", s.Output.Withdraw.Select(w => $"\"{w}\""))}]";

    /// <summary>Swift raw values: <c>timeSensitive</c>, <c>question</c> … (the enum name with a lower-case first letter).</summary>
    public static string Camel<T>(T value) where T : struct, Enum
    {
        var s = value.ToString();
        return s.Length == 0 ? s : char.ToLowerInvariant(s[0]) + s[1..];
    }
}
