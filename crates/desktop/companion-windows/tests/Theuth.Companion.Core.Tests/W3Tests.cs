namespace Theuth.Companion.Core.Tests;

// Design 134 W3 (notifications · settings · launch at login): the pure halves the app's Notifier, --probe-notify and
// Run-key code rest on. C# additions — the Swift companion has no replay type (its replay lived inline in
// NotificationsInstaller) and registers its login item through the shell.

public sealed class NotificationReplayTests
{
    private static readonly DateTimeOffset T0 = PolicyFx.T0;

    private static List<AttentionItem> Decisions(int n) =>
        Enumerable.Range(1, n).Select(i => PolicyFx.It($"decision:{i}", AttentionKind.Decision, agent: $"d{i}")).ToList();

    [Fact]
    public void TheFloodSegmentPostsExactlyOneSummary()
    {
        // C6: eleven blocking items at once → flood → one "N agents need you" banner, nothing per item.
        var steps = NotificationReplay.Run(Decisions(11), new NotificationPrefs(), T0);
        var all = steps.Where(s => s.Segment == NotificationReplay.SegmentAll).ToList();
        var posted = all.SelectMany(s => s.Output.Post).ToList();
        var summary = Assert.Single(posted);
        Assert.True(summary.Id == Banner.FloodId && summary.Category == BannerCategory.Batch && summary.Body.StartsWith("11 agents need you", StringComparison.Ordinal));
        // One at a time: the first ten are blocking → ten immediate question banners, all withdrawn when answered elsewhere.
        var one = steps.Where(s => s.Segment == NotificationReplay.SegmentOne).ToList();
        Assert.Equal(10, one.Count(s => s.Label.StartsWith("item ", StringComparison.Ordinal)));
        // Ten immediate question banners, then nine more when items 2–10 are re-added at +70 s (item 1 is still live).
        Assert.Equal(19, one.SelectMany(s => s.Output.Post).Count(b => b.Category == BannerCategory.Question));
        Assert.Equal(9, one.Single(s => s.Label.StartsWith("items re-added", StringComparison.Ordinal)).Output.Post.Count);
        var gone = Assert.Single(one, s => s.Label.StartsWith("answered elsewhere", StringComparison.Ordinal));
        Assert.Equal(10, gone.Output.Withdraw.Count);
        // The engine segment: one error banner, Active + sound without the opt-in.
        var engine = Assert.Single(steps, s => s.Segment == NotificationReplay.SegmentEngine);
        var error = Assert.Single(engine.Output.Post);
        Assert.True(error.Id == Banner.EngineId && error.Category == BannerCategory.Error && error.Level == BannerLevel.Active && error.Sound);
    }

    [Fact]
    public void TheOptInMakesTheReplayedErrorTimeSensitive()
    {
        var prefs = new NotificationPrefs { TimeSensitiveAvailable = true };
        var steps = NotificationReplay.Run(Decisions(1), prefs, T0);
        var engine = Assert.Single(steps, s => s.Segment == NotificationReplay.SegmentEngine);
        Assert.Equal(BannerLevel.TimeSensitive, Assert.Single(engine.Output.Post).Level); // → AppNotificationScenario.Urgent
    }

    [Fact]
    public void TheQuietDayShowsTheHeldBatchAndTheRateWindow()
    {
        // The 130 W2 live shape: nine reviews and a decision.
        var items = Enumerable.Range(1, 9).Select(i => PolicyFx.It($"review:{i}", AttentionKind.Review)).ToList();
        items.Add(PolicyFx.It("decision:x", AttentionKind.Decision));
        var steps = NotificationReplay.Run(items, new NotificationPrefs(), T0);
        var all = Assert.Single(steps, s => s.Segment == NotificationReplay.SegmentAll);
        Assert.Equal("all 10 at once", all.Label);
        var q = Assert.Single(all.Output.Post);
        Assert.True(q.Id == "decision:x" && q.Level == BannerLevel.Active && !q.Sound); // immediate, silent; the reviews are held
        string Posts(string prefix) => string.Join(",", steps.Single(s => s.Label.StartsWith(prefix, StringComparison.Ordinal)).Output.Post.Select(b => b.Category.ToString()));
        Assert.Equal("", Posts("tick +60"));
        Assert.Equal("Batch", Posts("breakpoint +61"));
        Assert.Equal("", Posts("breakpoint +80")); // inside the 2-minute rate window
        Assert.Equal("Batch", Posts("tick +181"));
        Assert.NotEmpty(steps.Single(s => s.Label.StartsWith("answered elsewhere", StringComparison.Ordinal)).Output.Withdraw);
        Assert.Contains("replay all 10 at once: post=[question:decision:x active] withdraw=[]", NotificationReplay.Describe(all));
    }
}

public sealed class LoginItemRegistrationTests
{
    private const string Exe = @"C:\Users\sky\AppData\Local\theuth\companion\Theuth.Companion.exe";

    [Fact]
    public void TheRunValueIsTheQuotedExeWithBackground()
    {
        Assert.Equal("TheuthCompanion", LoginItemRegistration.ValueName); // W5's NSIS uninstall hook deletes exactly this name
        Assert.Equal(@"Software\Microsoft\Windows\CurrentVersion\Run", LoginItemRegistration.RunKey);
        var v = LoginItemRegistration.RunValue(Exe);
        Assert.Equal($"\"{Exe}\" --background", v);
        Assert.Equal(Exe, LoginItemRegistration.ExeOf(v));
        Assert.True(LoginItemRegistration.PointsAt(v, Exe));
        Assert.True(LoginItemRegistration.PointsAt(v.ToUpperInvariant(), Exe)); // NTFS is case-insensitive
        Assert.Equal("enabled", LoginItemRegistration.Status(v, Exe));
    }

    [Fact]
    public void AForeignOrMissingValueIsNotRegistered()
    {
        Assert.Null(LoginItemRegistration.ExeOf(null));
        Assert.Null(LoginItemRegistration.ExeOf("  "));
        Assert.Equal(@"D:\old\Theuth.Companion.exe", LoginItemRegistration.ExeOf(@"D:\old\Theuth.Companion.exe --background"));
        Assert.Equal(@"D:\old\Theuth.Companion.exe", LoginItemRegistration.ExeOf(@"""D:\old\Theuth.Companion.exe"" --background"));
        Assert.False(LoginItemRegistration.PointsAt(@"""D:\old\Theuth.Companion.exe"" --background", Exe)); // moved install → rewritten
        Assert.Equal("not_registered", LoginItemRegistration.Status(null, Exe));
        Assert.Equal("{ \"launch_at_login\": false }\n", new CompanionSettings().LoginItemJson); // what the shell reads
    }
}
