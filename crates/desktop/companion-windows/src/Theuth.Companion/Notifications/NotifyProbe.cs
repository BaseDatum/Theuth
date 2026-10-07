using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;
using Theuth.Companion.Settings;

namespace Theuth.Companion.Notifications;

/// <summary>
/// <c>--probe-notify N</c> (130 W2 <c>NotificationsInstaller.runProbe</c>; design 134 §7 W3 gate): run the real policy
/// against the live engine for N s, show Settings › Notifications for the last 3 s without activating, then replay the
/// live items through fresh policies (<see cref="NotificationReplay"/>) — <b>posting every step for real</b>, one toast
/// group per segment (<c>probe-all</c> · <c>probe-one</c> · <c>probe-engine</c>) — count what is in the centre with
/// <c>AppNotificationManager.GetAllAsync</c> (C6: exactly one summary in the flood segment), log everything to
/// <see cref="Notifier.ProbeLog"/>, and quit 0 (or stay with <c>--stay</c>, so a toast button can be pressed on the live
/// process). Windows difference from the Mac's dry run: the gate counts toasts, so the replay posts them; the toasts
/// are left in the centre as evidence and removed by the next launch's reconcile.
/// </summary>
internal sealed class NotifyProbe
{
    private readonly Store store;
    private readonly Notifier notifier;
    private readonly SettingsController settings;
    private readonly BreakpointSource breakpoints;
    private readonly DispatcherQueue dispatcher;
    private readonly Action<int> quit;

    public NotifyProbe(Store store, Notifier notifier, SettingsController settings, BreakpointSource breakpoints, DispatcherQueue dispatcher, Action<int> quit)
    {
        this.store = store;
        this.notifier = notifier;
        this.settings = settings;
        this.breakpoints = breakpoints;
        this.dispatcher = dispatcher;
        this.quit = quit;
    }

    public void Run(double seconds, bool stay)
    {
        try { File.WriteAllText(Notifier.ProbeLog, ""); } catch (Exception e) when (e is IOException or UnauthorizedAccessException) { }
        var prefs = SettingsModel.Shared.Value.Notify;
        notifier.ProbeLine($"start probe-notify {Flags.Describe(seconds)} s exe={Environment.ProcessPath} prefs={Notifier.Describe(prefs)} registered={(Notifier.Registered ? "true" : "false")} setting={notifier.SettingText} urgentSupported={(Notifier.UrgentSupported ? "true" : "false")} stay={(stay ? "true" : "false")}");
        _ = ClearAsync();
        After(Math.Max(1, seconds - 3), () =>
        {
            settings.Show(activate: false, page: "notifications");
            notifier.ProbeLine($"settings window shown page={settings.CurrentPage} active=false");
        });
        After(seconds, async () =>
        {
            var items = store.Items.ToList();
            string Count<T>(IEnumerable<T> xs, Func<T, string> key) =>
                string.Join(" ", xs.GroupBy(key).Select(g => $"{g.Key}×{g.Count()}").OrderBy(s => s, StringComparer.Ordinal));
            var (sw, idle) = breakpoints.Counts;
            notifier.ProbeLine($"live items={items.Count} [{Count(items, i => i.Kind.RawValue)}] classes=[{Count(items, NotificationReplay.ClassName)}] " +
                $"new-while-running→posted={{{string.Join(", ", notifier.PostedByCategory.Select(kv => $"{kv.Key}: {kv.Value}"))}}} flood={(notifier.InFlood ? "true" : "false")} held={notifier.HeldCount} " +
                $"paused={(notifier.PausedUntil is not null ? "true" : "false")} connection={store.Connection.ToString().ToLowerInvariant()} breakpoints: switches={sw} idle={idle}");
            await ReplayAsync(items, prefs);
            notifier.ProbeLine($"done after {Flags.Describe(seconds)} s — {(stay ? "staying (--stay)" : "quitting 0")}");
            if (!stay) quit(0);
        });
    }

    private async Task ClearAsync()
    {
        var before = await notifier.AllAsync();
        if (before.Count > 0) await notifier.RemoveAllAsync($"probe start: {before.Count} toasts of ours were in the centre");
    }

    /// <summary>Every step posted for real; the live policy's groups are the agents, the replay's are its segments.</summary>
    private async Task ReplayAsync(IReadOnlyList<AttentionItem> items, NotificationPrefs prefs)
    {
        var t0 = DateTimeOffset.Now;
        var steps = NotificationReplay.Run(items, prefs, t0);
        var tags = new Dictionary<string, Dictionary<string, string>>(StringComparer.Ordinal); // segment → banner id → tag
        var postedBySegment = new Dictionary<string, int>(StringComparer.Ordinal);
        foreach (var step in steps)
        {
            string group = "probe-" + step.Segment;
            var inSegment = tags.TryGetValue(step.Segment, out var d) ? d : tags[step.Segment] = new Dictionary<string, string>(StringComparer.Ordinal);
            foreach (var id in step.Output.Withdraw)
            {
                if (inSegment.Remove(id, out var tag)) await notifier.Remove(tag, group, id);
            }
            foreach (var b in step.Output.Post)
            {
                var tag = ToastBuilder.TagFor(b.Id);
                if (notifier.Show(b, notifier.OptionLabels(b), tag, group, group))
                {
                    inSegment[b.Id] = tag;
                    postedBySegment[step.Segment] = postedBySegment.GetValueOrDefault(step.Segment) + 1;
                }
            }
            notifier.ProbeLine(NotificationReplay.Describe(step));
        }
        // Windows lets a Show settle asynchronously; give the centre a moment before counting.
        await Task.Delay(1500);
        var all = await notifier.AllAsync();
        var byGroup = all.GroupBy(n => n.Group).OrderBy(g => g.Key, StringComparer.Ordinal).Select(g => $"{g.Key}×{g.Count()}");
        int flood = all.Count(n => n.Group == "probe-" + NotificationReplay.SegmentAll);
        int floodPosted = postedBySegment.GetValueOrDefault(NotificationReplay.SegmentAll);
        notifier.ProbeLine($"GetAllAsync total={all.Count} by-group=[{string.Join(" ", byGroup)}] posted-by-segment=[{string.Join(" ", postedBySegment.OrderBy(kv => kv.Key, StringComparer.Ordinal).Select(kv => $"{kv.Key}×{kv.Value}"))}]");
        notifier.ProbeLine($"flood segment ({items.Count} items at once): posted={floodPosted} in-centre={flood} {(items.Count > NotificationPolicy.FloodThreshold ? (flood == 1 ? "→ exactly one summary (C6 ok)" : "→ C6 FAILED") : "(no flood: ≤ 10 items)")}");
    }

    private void After(double seconds, Action action)
    {
        var t = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromSeconds(seconds);
        t.IsRepeating = false;
        t.Tick += (_, _) => action();
        t.Start();
        keep.Add(t);
    }

    private readonly List<DispatcherQueueTimer> keep = new();
}
