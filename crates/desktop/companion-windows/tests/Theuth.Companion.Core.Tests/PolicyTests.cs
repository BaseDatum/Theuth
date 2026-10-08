namespace Theuth.Companion.Core.Tests;

// Port of PolicyTests.swift. Swift's `#expect` captured operands as values, so every reducer call was hoisted into a
// `let`; the C# asserts read the same way.

internal static class PolicyFx
{
    public static readonly DateTimeOffset T0 = DateTimeOffset.FromUnixTimeSeconds(1_791_448_800);

    public static AttentionItem It(string id, AttentionKind kind, string agent = "w2", string? detail = null) => new()
    {
        Id = id,
        Kind = kind,
        Title = $"title {id}",
        Detail = detail,
        AtMs = 1_791_448_000_000,
        Agent = agent,
        Approval = kind == AttentionKind.Approval ? $"ap-{id}" : null,
    };

    public static DateTimeOffset At(double s) => T0.AddSeconds(s);

    /// <summary>A policy primed with an empty list so every later item is "new".</summary>
    public static NotificationPolicy Primed(NotificationPrefs? prefs = null)
    {
        var p = new NotificationPolicy(prefs ?? new NotificationPrefs());
        _ = p.Update(Array.Empty<AttentionItem>(), At(-1));
        return p;
    }
}

public sealed class PolicyClassTests
{
    private static readonly DateTimeOffset T0 = PolicyFx.T0;
    private static AttentionItem It(string id, AttentionKind kind, string agent = "w2", string? detail = null) => PolicyFx.It(id, kind, agent, detail);

    [Fact]
    public void EveryKindHasItsRow()
    {
        Assert.All(new[] { AttentionKind.Approval, AttentionKind.Ask, AttentionKind.Decision, AttentionKind.Secret, AttentionKind.Handoff },
            k => Assert.Equal(NotificationClass.Blocking, NotificationPolicy.Classify(It("x", k))));
        Assert.All(new[] { AttentionKind.Review, AttentionKind.Hire, AttentionKind.Finished, AttentionKind.Blocked },
            k => Assert.Equal(NotificationClass.Deferrable, NotificationPolicy.Classify(It("x", k))));
        Assert.Equal(NotificationClass.Deferrable, NotificationPolicy.Classify(It("b", AttentionKind.Budget, detail: "at 80 %: the cap pauses it")));
        Assert.Equal(NotificationClass.Error, NotificationPolicy.Classify(It("b", AttentionKind.Budget, detail: "paused at the cap; un-pause to override for the month")));
        Assert.Equal(NotificationClass.Error, NotificationPolicy.Classify(It("n", AttentionKind.Node)));
        Assert.All(new[] { AttentionKind.Show, AttentionKind.Resume, AttentionKind.Other("later") },
            k => Assert.Equal(NotificationClass.Never, NotificationPolicy.Classify(It("x", k))));
    }

    [Fact]
    public void BlockingIsImmediateActiveSilent()
    {
        var p = PolicyFx.Primed();
        var output = p.Update(new[] { It("ask:1", AttentionKind.Ask), It("show:1", AttentionKind.Show) }, T0);
        var b = Assert.Single(output.Post);
        Assert.True(b.Id == "ask:1" && b.Category == BannerCategory.Question && b.Level == BannerLevel.Active && !b.Sound && b.Thread == "w2");
        Assert.Empty(output.Withdraw);
    }

    [Fact]
    public void ErrorIsActiveWithSoundAndTimeSensitiveOnlyWhenAvailable()
    {
        var p = PolicyFx.Primed();
        var plain = p.Update(new[] { It("node:1", AttentionKind.Node) }, T0);
        Assert.Equal(new[] { BannerLevel.Active }, plain.Post.Select(b => b.Level));
        Assert.Equal(new[] { true }, plain.Post.Select(b => b.Sound));
        var prefs = new NotificationPrefs { TimeSensitiveAvailable = true };
        p = PolicyFx.Primed(prefs);
        var ts = p.Update(new[] { It("budget:ops", AttentionKind.Budget, detail: "paused at the cap") }, T0);
        Assert.Equal(new[] { BannerLevel.TimeSensitive }, ts.Post.Select(b => b.Level));
        Assert.Equal(new[] { BannerCategory.Error }, ts.Post.Select(b => b.Category));
        prefs.SoundOnErrors = false;
        p = PolicyFx.Primed(prefs);
        var quiet = p.Update(new[] { It("node:2", AttentionKind.Node) }, T0);
        Assert.Equal(new[] { false }, quiet.Post.Select(b => b.Sound));
    }

    [Fact]
    public void FirstSnapshotIsSeededNotPosted()
    {
        var p = new NotificationPolicy();
        var seed = p.Update(new[] { It("ask:old", AttentionKind.Ask), It("review:old", AttentionKind.Review) }, T0);
        Assert.True(seed.IsEmpty);
        var output = p.Update(new[] { It("ask:old", AttentionKind.Ask), It("review:old", AttentionKind.Review), It("ask:new", AttentionKind.Ask) }, PolicyFx.At(1));
        Assert.Equal(new[] { "ask:new" }, output.Post.Select(b => b.Id)); // only the genuinely new item posts
    }
}

public sealed class PolicyDeferralTests
{
    private static readonly DateTimeOffset T0 = PolicyFx.T0;
    private static AttentionItem It(string id, AttentionKind kind, string agent = "w2") => PolicyFx.It(id, kind, agent);
    private static DateTimeOffset At(double s) => PolicyFx.At(s);

    [Fact]
    public void HeldUntilBreakpointThenOneBatch()
    {
        var p = PolicyFx.Primed();
        var a = p.Update(new[] { It("review:1", AttentionKind.Review) }, T0);
        var b2 = p.Update(new[] { It("review:1", AttentionKind.Review), It("finished:1", AttentionKind.Finished, agent: "w1") }, At(10));
        Assert.True(a.IsEmpty && b2.IsEmpty && p.HeldCount == 2);
        var early = p.Tick(At(60));
        Assert.True(early.IsEmpty); // the cap has not elapsed
        var output = p.Breakpoint(At(70));
        var b = Assert.Single(output.Post);
        Assert.True(b.Category == BannerCategory.Batch && b.Level == BannerLevel.Passive && b.Body == "1 for later · 1 finished");
        Assert.Equal(new HashSet<string> { "review:1", "finished:1" }, b.ItemIds.ToHashSet());
        Assert.Equal(0, p.HeldCount);
    }

    [Fact]
    public void SingleHeldItemKeepsItsOwnCategory()
    {
        var p = PolicyFx.Primed();
        _ = p.Update(new[] { It("review:1", AttentionKind.Review) }, T0);
        var output = p.Breakpoint(At(5));
        Assert.Equal(new[] { BannerCategory.Review }, output.Post.Select(b => b.Category));
        Assert.Equal(new[] { BannerLevel.Passive }, output.Post.Select(b => b.Level));
    }

    [Fact]
    public void TwoMinuteCapReleasesWithoutBreakpoint()
    {
        var p = PolicyFx.Primed();
        _ = p.Update(new[] { It("review:1", AttentionKind.Review) }, T0);
        var before = p.Tick(At(119));
        var after = p.Tick(At(120));
        Assert.True(before.IsEmpty);
        Assert.Equal(new[] { "review:1" }, after.Post.Select(b => b.Id));
    }

    [Fact]
    public void SteadyStateOneDeferrablePerTwoMinutes()
    {
        var p = PolicyFx.Primed();
        _ = p.Update(new[] { It("review:1", AttentionKind.Review) }, T0);
        var first = p.Breakpoint(At(1));
        Assert.Single(first.Post);
        _ = p.Update(new[] { It("review:1", AttentionKind.Review), It("review:2", AttentionKind.Review) }, At(30));
        var inWindow = p.Breakpoint(At(40));
        Assert.True(inWindow.IsEmpty); // held even at a breakpoint inside the rate window
        var edge = p.Tick(At(120));
        var past = p.Tick(At(121));
        Assert.True(edge.IsEmpty);
        Assert.Equal(new[] { "review:2" }, past.Post.Select(b => b.Id)); // the pending breakpoint releases once the window passes
    }

    [Fact]
    public void PerClassModesOverrideTheTable()
    {
        var prefs = new NotificationPrefs { Blocking = NotifyMode.AtBreak, Deferrable = NotifyMode.Immediate, Error = NotifyMode.Off };
        var p = PolicyFx.Primed(prefs);
        var output = p.Update(new[] { It("ask:1", AttentionKind.Ask), It("review:1", AttentionKind.Review), It("node:1", AttentionKind.Node) }, T0);
        Assert.Equal(new[] { "review:1" }, output.Post.Select(b => b.Id));
        Assert.Equal(new[] { BannerLevel.Passive }, output.Post.Select(b => b.Level));
        Assert.Equal(1, p.HeldCount);
        var released = p.Breakpoint(At(1));
        Assert.Equal(new[] { "ask:1" }, released.Post.Select(b => b.Id));
    }
}

public sealed class PolicyFloodPauseWithdrawTests
{
    private static readonly DateTimeOffset T0 = PolicyFx.T0;
    private static AttentionItem It(string id, AttentionKind kind, string agent = "w2") => PolicyFx.It(id, kind, agent);
    private static DateTimeOffset At(double s) => PolicyFx.At(s);

    [Fact]
    public void FloodPostsOneSummaryAndNoPerItemBanners()
    {
        var p = PolicyFx.Primed();
        var items = Enumerable.Range(1, 11).Select(i => It($"ask:{i}", AttentionKind.Ask, agent: $"w{i}")).ToList();
        var enter = p.Update(items, T0);
        Assert.Equal(new[] { Banner.FloodId }, enter.Post.Select(b => b.Id));
        Assert.Equal("11 agents need you — open the panel", enter.Post.First().Body);
        Assert.True(p.InFlood);
        items.Add(It("ask:12", AttentionKind.Ask));
        var more = p.Update(items, At(1));
        Assert.True(more.IsEmpty); // no per-item banners while flooded
        var refreshed = p.Breakpoint(At(2));
        Assert.Equal(new[] { "12 agents need you — open the panel" }, refreshed.Post.Select(b => b.Body)); // the count refreshes at a breakpoint
        items.Add(It("node:1", AttentionKind.Node));
        var err = p.Update(items, At(3));
        Assert.Equal(new[] { "node:1" }, err.Post.Select(b => b.Id)); // errors still get through
        var drained = p.Update(items.Take(5).ToList(), At(4));
        Assert.Contains(Banner.FloodId, drained.Withdraw);
        Assert.Contains("node:1", drained.Withdraw);
        Assert.False(p.InFlood);
    }

    [Fact]
    public void AnsweredElsewhereWithdrawsItsBanner()
    {
        var p = PolicyFx.Primed();
        _ = p.Update(new[] { It("ask:1", AttentionKind.Ask), It("ask:2", AttentionKind.Ask) }, T0);
        var output = p.Update(new[] { It("ask:2", AttentionKind.Ask) }, At(1));
        Assert.Equal(new[] { "ask:1" }, output.Withdraw);
        Assert.Empty(output.Post);
        // A held item that disappears is dropped silently; a batch is withdrawn once every item is gone.
        _ = p.Update(new[] { It("ask:2", AttentionKind.Ask), It("review:1", AttentionKind.Review), It("review:2", AttentionKind.Review) }, At(2));
        var batch = p.Breakpoint(At(3)).Post.First();
        var partial = p.Update(new[] { It("ask:2", AttentionKind.Ask), It("review:2", AttentionKind.Review) }, At(4));
        var gone = p.Update(new[] { It("ask:2", AttentionKind.Ask) }, At(5));
        Assert.Empty(partial.Withdraw);
        Assert.Equal(new[] { batch.Id }, gone.Withdraw);
    }

    [Fact]
    public void PauseDeliversNothingAndEndsOnTick()
    {
        var p = PolicyFx.Primed();
        p.Pause(At(3600));
        var a = p.Update(new[] { It("ask:1", AttentionKind.Ask), It("review:1", AttentionKind.Review) }, T0);
        var b = p.Breakpoint(At(1));
        var c = p.Engine(connected: false, At(2));
        Assert.True(a.IsEmpty && b.IsEmpty && c.IsEmpty);
        Assert.True(p.IsPaused(At(3599)) && !p.IsPaused(At(3600)));
        _ = p.Tick(At(3600));
        Assert.Null(p.PausedUntil);
        var after = p.Update(new[] { It("ask:1", AttentionKind.Ask), It("ask:2", AttentionKind.Ask) }, At(3601));
        Assert.Equal(new[] { "ask:2" }, after.Post.Select(x => x.Id)); // the item that arrived during the pause was seen
    }

    [Fact]
    public void EngineLostIsOneErrorBannerWithdrawnOnReconnect()
    {
        var p = PolicyFx.Primed();
        // W6 item 10: the drop is held for EngineGrace; the banner comes from the tick after it (EngineLossGraceTests cover the grace).
        Assert.True(p.Engine(connected: false, T0).IsEmpty);
        var lost = p.Tick(T0 + NotificationPolicy.EngineGrace);
        Assert.Equal(new[] { Banner.EngineId }, lost.Post.Select(b => b.Id));
        Assert.Equal(new[] { true }, lost.Post.Select(b => b.Sound));
        var again = p.Engine(connected: false, T0 + NotificationPolicy.EngineGrace + TimeSpan.FromSeconds(1));
        var refresh = p.Update(Array.Empty<AttentionItem>(), T0 + NotificationPolicy.EngineGrace + TimeSpan.FromSeconds(2));
        Assert.True(again.IsEmpty && refresh.Withdraw.Count == 0); // not repeated; an item refresh does not withdraw it
        var back = p.Engine(connected: true, T0 + NotificationPolicy.EngineGrace + TimeSpan.FromSeconds(3));
        Assert.Equal(new[] { Banner.EngineId }, back.Withdraw);
    }
}

public sealed class BreakpointDetectorTests
{
    private static BreakpointDetector.Signal App(string? id) => new BreakpointDetector.Signal.AppActivated(id);
    private static BreakpointDetector.Signal Idle(double s) => new BreakpointDetector.Signal.Idle(s);

    [Fact]
    public void AppSwitchFiresOncePerChangeAndIgnoresSelf()
    {
        var own = @"C:\Users\user\AppData\Local\theuth\companion\Theuth.Companion.exe";
        var d = new BreakpointDetector(own);
        var r = new[]
        {
            d.Feed(App(@"C:\Program Files\theuth\theuth-desktop.exe")), d.Feed(App(@"C:\Program Files\Mozilla Firefox\firefox.exe")),
            d.Feed(App(@"C:\Program Files\Mozilla Firefox\firefox.exe")), d.Feed(App(own)),
            d.Feed(App(@"C:\Program Files\theuth\theuth-desktop.exe")),
        };
        Assert.Equal(new[] { false, true, false, false, true }, r); // seed · switch · same app · our own panel · switch
    }

    [Fact]
    public void IdleFiresOncePerStretch()
    {
        var d = new BreakpointDetector(null);
        var r = new[] { d.Feed(Idle(19.9)), d.Feed(Idle(20)), d.Feed(Idle(21)), d.Feed(Idle(90)), d.Feed(Idle(0.3)), d.Feed(Idle(20.5)) };
        Assert.Equal(new[] { false, true, false, false, false, true }, r); // once per pause; typing resumed → a new pause fires again
    }
}

public sealed class DoubleTapDetectorTests
{
    private const Modifiers O = Modifiers.Alt;
    private const Modifiers N = Modifiers.None;

    private static bool[] Run((Modifiers Mods, double T)[] steps, int? cancelBefore = null)
    {
        var d = new DoubleTapDetector();
        return steps.Select((s, i) =>
        {
            if (i == cancelBefore) d.Cancel();
            return d.Feed(s.Mods, s.T);
        }).ToArray();
    }

    [Fact]
    public void DoubleTap() => Assert.Equal(new[] { false, false, false, true }, Run(new[] { (O, 0.0), (N, 0.08), (O, 0.18), (N, 0.26) }));

    [Fact]
    public void TapThenHold() => Assert.DoesNotContain(true, Run(new[] { (O, 0.0), (N, 0.08), (O, 0.18), (N, 1.0) }));

    [Fact]
    public void LongHoldThenTap() => Assert.DoesNotContain(true, Run(new[] { (O, 0.0), (N, 0.60), (O, 0.70), (N, 0.75) }));

    [Fact]
    public void ChordReleaseThenTap() =>
        Assert.DoesNotContain(true, Run(new[] { (O, 0.0), (Modifiers.Alt | Modifiers.Win, 0.05), (O, 0.10), (N, 0.15), (O, 0.20), (N, 0.25) }));

    [Fact]
    public void TwoTapsTooFarApart() => Assert.DoesNotContain(true, Run(new[] { (O, 0.0), (N, 0.05), (O, 0.40), (N, 0.45) }));

    [Fact]
    public void ThirdTapCompletesWithTheSecond() =>
        Assert.True(Run(new[] { (O, 0.0), (N, 0.05), (O, 0.40), (N, 0.45), (O, 0.55), (N, 0.60) })[^1]);

    [Fact]
    public void AutoRepeatDuplicateIgnored() => Assert.True(Run(new[] { (O, 0.0), (O, 0.03), (N, 0.08), (O, 0.18), (N, 0.26) })[^1]);

    [Fact]
    public void KeyBetweenTapsCancels() =>
        Assert.DoesNotContain(true, Run(new[] { (O, 0.0), (N, 0.05), (O, 0.15), (N, 0.20) }, cancelBefore: 2));

    [Fact]
    public void ShiftBetweenTapsCancels() =>
        Assert.DoesNotContain(true, Run(new[] { (O, 0.0), (N, 0.05), (Modifiers.Shift, 0.10), (N, 0.15), (O, 0.20), (N, 0.25) }));

    [Fact]
    public void CapsLockIsMaskedByTheSource()
    {
        // The keyboard-hook source drops Caps Lock before feeding; the detector only ever sees the modifier bits.
        Assert.True(Run(new[] { (O, 0.0), (N, 0.08), (O, 0.18), (N, 0.26) })[^1]);
    }
}

public sealed class SettingsPersistenceTests
{
    [Fact]
    public void RoundTripAndDefaults()
    {
        var d = new SettingsStore();
        Assert.Equal(new CompanionSettings(), CompanionSettings.Load(d)); // empty defaults = the design's defaults
        var s = new CompanionSettings { RailSide = "left", RailPresence = RailPresence.AlwaysSliver, DoubleTapAlt = true, UndoDelay = 7 };
        s.Keys.Set(new Chord("n", 'N'), KeyAction.Next);
        s.Notify = s.Notify with { Blocking = NotifyMode.AtBreak, Deferrable = NotifyMode.Off, SoundOnErrors = false };
        s.LaunchAtLogin = true;
        s.Save(d);
        var back = CompanionSettings.Load(d);
        Assert.Equal(5, back.UndoDelay); // clamped 0–5
        back.UndoDelay = 7;
        Assert.Equal(s, back);
        // W1's keys are the ones the Store and the rail already read.
        Assert.True(d.String("railSide") == "left" && d.Double("undoDelay") == 5);
        Assert.Equal("{ \"launch_at_login\": true }\n", s.LoginItemJson);
    }

    [Fact]
    public void W2CollapseToggleMigratesToPresence()
    {
        var d = new SettingsStore();
        d.Set("railCollapse", false);
        Assert.Equal(RailPresence.Expanded, CompanionSettings.Load(d).RailPresence);
        d.Set("railCollapse", true);
        Assert.Equal(RailPresence.SliverWhenQuiet, CompanionSettings.Load(d).RailPresence);
        d.Set("railPresence", "alwaysSliver");
        Assert.Equal(RailPresence.AlwaysSliver, CompanionSettings.Load(d).RailPresence); // the new key wins
    }

    // MARK: C# addition — the Windows store is a file (design 134 D12).

    [Fact]
    public void TheSettingsFileRoundTripsAndToleratesGarbage()
    {
        var dir = Path.Combine(Path.GetTempPath(), "theuth-companion-" + Guid.NewGuid().ToString("N"));
        var path = Path.Combine(dir, "companion", "settings.json");
        try
        {
            var s = new CompanionSettings { RailSide = "left", UndoDelay = 1.5, LaunchAtLogin = true };
            s.Keys.Set(null, KeyAction.Clear);
            s.Save(SettingsStore.Load(path));
            Assert.True(File.Exists(path));
            Assert.Equal(s, CompanionSettings.Load(SettingsStore.Load(path)));
            File.WriteAllText(path, "{ not json");
            Assert.Equal(new CompanionSettings(), CompanionSettings.Load(SettingsStore.Load(path)));
            Assert.EndsWith(Path.Combine("theuth", "companion", "settings.json"), SettingsStore.DefaultPath);
        }
        finally
        {
            if (Directory.Exists(dir)) Directory.Delete(dir, recursive: true);
        }
    }
}
