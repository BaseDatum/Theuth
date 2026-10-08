using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

// 130 §4.4 (the policy table) and §5.5 (the state machine). Pure: items in, banners out; the app posts them
// (W3's Notifier: AppNotificationManager, design 134 D8). Times are DateTimeOffset, intervals seconds.

/// <summary>The four rows of the §4.4 table.</summary>
public enum NotificationClass { Blocking, Deferrable, Error, Never }

/// <summary>Per-class user preference (Settings › Notifications).</summary>
public enum NotifyMode { Immediate, AtBreak, Off }

public static class NotifyModeText
{
    /// <summary>The persisted word (Swift's raw value): <c>immediate</c> · <c>atBreak</c> · <c>off</c>.</summary>
    public static string RawValue(this NotifyMode m) => m switch
    {
        NotifyMode.Immediate => "immediate",
        NotifyMode.AtBreak => "atBreak",
        _ => "off",
    };

    public static NotifyMode? FromRaw(string? raw) => raw switch
    {
        "immediate" => NotifyMode.Immediate,
        "atBreak" => NotifyMode.AtBreak,
        "off" => NotifyMode.Off,
        _ => null,
    };
}

/// <summary>Settings › Notifications. A value type, as in Swift: a policy keeps its own copy.</summary>
public record struct NotificationPrefs()
{
    public NotifyMode Blocking { get; set; } = NotifyMode.Immediate;
    public NotifyMode Deferrable { get; set; } = NotifyMode.AtBreak;
    public NotifyMode Error { get; set; } = NotifyMode.Immediate;
    public bool SoundOnErrors { get; set; } = true;

    /// <summary>
    /// Errors may break through Do-not-disturb (<see cref="BannerLevel.TimeSensitive"/>). On the Mac: the restricted
    /// time-sensitive entitlement is available. On Windows (design 134 D8, R2): Settings › Notifications "Errors break
    /// through Do not disturb" is on (off by default) → the Notifier posts the level as <c>AppNotificationScenario.Urgent</c>.
    /// </summary>
    public bool TimeSensitiveAvailable { get; set; } = false;

    public readonly NotifyMode ModeFor(NotificationClass c) => c switch
    {
        NotificationClass.Blocking => Blocking,
        NotificationClass.Deferrable => Deferrable,
        NotificationClass.Error => Error,
        _ => NotifyMode.Off,
    };
}

/// <summary>Registered categories (D8); <c>Error</c> and the batch's reuse for plain kinds are W2 additions.</summary>
public enum BannerCategory { Question, Approval, Review, Finished, Batch, Error }

/// <summary>Active: a normal banner. Passive: quiet, deferrable. TimeSensitive: breaks through Do-not-disturb (Windows: Urgent).</summary>
public enum BannerLevel { Active, Passive, TimeSensitive }

/// <summary>One notification the app should post (Swift: one <c>UNNotificationRequest</c>; Windows: one <c>AppNotification</c>).</summary>
public sealed record Banner(
    string Id,
    BannerCategory Category,
    string Title,
    string Body,
    string Thread,
    BannerLevel Level,
    bool Sound,
    ValueList<string> ItemIds)
{
    /// <summary><see cref="Thread"/>: the agent, so an agent's banners stack (Windows: <c>SetGroup</c>).
    /// <see cref="ItemIds"/>: the items it covers (one, or the batch); withdrawn when they are all gone.</summary>
    public const string FloodId = "flood";

    public const string EngineId = "engine";
}

public sealed class PolicyOutput
{
    public List<Banner> Post { get; } = new();
    public List<string> Withdraw { get; } = new();

    public bool IsEmpty => Post.Count == 0 && Withdraw.Count == 0;

    internal void Merge(PolicyOutput o)
    {
        Post.AddRange(o.Post);
        Withdraw.AddRange(o.Withdraw);
    }
}

public sealed class NotificationPolicy
{
    /// <summary>Held deferrables go out at the latest this long after the first one (seconds).</summary>
    public const double Cap = 120;

    /// <summary>Steady state: at most one deferrable banner per window (seconds).</summary>
    public const double Rate = 120;

    /// <summary>More than this many needs-you items → flood mode.</summary>
    public const int FloodThreshold = 10;

    public NotificationPrefs Prefs { get; set; }
    public DateTimeOffset? PausedUntil { get; private set; }

    private bool primed;
    private HashSet<string> known = new(StringComparer.Ordinal);
    private List<AttentionItem> held = new();
    private DateTimeOffset? holdingSince;
    private bool breakpointPending;
    private DateTimeOffset? lastDeferrableAt;
    private bool flood;
    private int floodShown;
    private int waitingNow;
    private bool engineLost;
    /// <summary>A lost connection waiting out <see cref="EngineGrace"/> (134 W6 item 10); null when none is pending.</summary>
    private DateTimeOffset? engineLostAt;
    /// <summary>The companion is quitting on purpose (its own <c>theuth://quit</c>): the engine going away is expected, never an error.</summary>
    private bool quitting;

    /// <summary>
    /// How long a lost connection is held before the error toast (134 W6 item 10). The shell's quit handshake stops the engine
    /// ~60 ms before it closes the companion (WM_CLOSE), an update kills the engine seconds before the companion, and an engine
    /// restart reconnects within a few seconds — each of those posted "theuth engine unreachable" into Notification Center for
    /// nothing. 5 s is longer than the shell's 2 s TerminateProcess grace and an installer's kill sequence, and the rail already
    /// dims the wordmark and shows the carnelian dot at once, so only the toast waits. A lasting loss notifies exactly as before.
    /// </summary>
    public static readonly TimeSpan EngineGrace = TimeSpan.FromSeconds(5);

    /// <summary>Whether a lost connection is waiting out the grace (the app's probe reads it).</summary>
    public bool EngineLossPending => engineLostAt is not null;

    /// <summary>Posted banner id → item ids still waiting.</summary>
    private readonly Dictionary<string, HashSet<string>> live = new(StringComparer.Ordinal);

    public NotificationPolicy() : this(new NotificationPrefs()) { }

    public NotificationPolicy(NotificationPrefs prefs) => Prefs = prefs;

    public int HeldCount => held.Count;
    public bool InFlood => flood;

    public bool IsPaused(DateTimeOffset now) => PausedUntil is { } until && now < until;

    // MARK: classification (§4.4)

    public static NotificationClass Classify(AttentionItem item) => item.Kind.Tag switch
    {
        AttentionKindTag.Approval or AttentionKindTag.Ask or AttentionKindTag.Decision or AttentionKindTag.Secret or AttentionKindTag.Handoff
            => NotificationClass.Blocking,
        AttentionKindTag.Review or AttentionKindTag.Hire or AttentionKindTag.Finished or AttentionKindTag.Blocked
            => NotificationClass.Deferrable,
        // attention.rs: "paused at the cap; …" vs "at 80 %: the cap pauses it".
        AttentionKindTag.Budget => item.Detail?.StartsWith("paused", StringComparison.Ordinal) == true ? NotificationClass.Error : NotificationClass.Deferrable,
        AttentionKindTag.Node => NotificationClass.Error,
        _ => NotificationClass.Never, // show · resume · other
    };

    public static BannerCategory CategoryFor(AttentionItem item) => item.Kind.Tag switch
    {
        AttentionKindTag.Ask or AttentionKindTag.Decision => BannerCategory.Question,
        AttentionKindTag.Approval => BannerCategory.Approval,
        AttentionKindTag.Review => BannerCategory.Review,
        AttentionKindTag.Finished => BannerCategory.Finished,
        AttentionKindTag.Node => BannerCategory.Error,
        AttentionKindTag.Budget => Classify(item) == NotificationClass.Error ? BannerCategory.Error : BannerCategory.Batch,
        _ => BannerCategory.Batch,
    };

    // MARK: inputs

    /// <summary>
    /// Items from <c>attention_list</c>; the first call seeds the known set without posting (those were waiting before
    /// the companion started) and only takes the flood reading.
    /// </summary>
    public PolicyOutput Update(IReadOnlyList<AttentionItem> items, DateTimeOffset now)
    {
        var output = new PolicyOutput();
        var ids = new HashSet<string>(items.Select(i => i.Id), StringComparer.Ordinal);
        foreach (var (bid, set) in live.ToList())
        {
            if (set.Count == 0) continue;
            var left = new HashSet<string>(set.Where(ids.Contains), StringComparer.Ordinal);
            if (left.Count == 0)
            {
                output.Withdraw.Add(bid);
                live.Remove(bid);
            }
            else
            {
                live[bid] = left;
            }
        }
        held.RemoveAll(i => !ids.Contains(i.Id));
        if (held.Count == 0)
        {
            holdingSince = null;
            breakpointPending = false;
        }
        var fresh = items.Where(i => !known.Contains(i.Id)).ToList();
        known = ids;
        int waiting = items.Count(i => i.Kind.NeedsYou);
        waitingNow = waiting;

        if (!primed)
        {
            primed = true;
            flood = waiting > FloodThreshold;
            floodShown = flood ? waiting : 0;
            return output;
        }
        if (IsPaused(now)) return output;

        if (waiting > FloodThreshold)
        {
            if (!flood)
            {
                flood = true;
                held = new();
                holdingSince = null;
                breakpointPending = false;
                output.Post.Add(FloodBanner(waiting));
            }
        }
        else if (flood)
        {
            flood = false;
            floodShown = 0;
            if (live.Remove(Banner.FloodId)) output.Withdraw.Add(Banner.FloodId);
        }

        foreach (var item in fresh)
        {
            var cls = Classify(item);
            // Flood: no per-item banners except errors.
            if (flood && cls != NotificationClass.Error) continue;
            switch (Prefs.ModeFor(cls))
            {
                case NotifyMode.Off:
                    continue;
                case NotifyMode.Immediate:
                    output.Post.Add(MakeBanner(item, cls));
                    break;
                case NotifyMode.AtBreak:
                    held.Add(item);
                    holdingSince ??= now;
                    break;
            }
        }
        output.Merge(Release(now, breakpoint: false));
        return output;
    }

    /// <summary>An app switch or a ≥ 20 s typing pause (<see cref="BreakpointDetector"/>).</summary>
    public PolicyOutput Breakpoint(DateTimeOffset now)
    {
        var output = new PolicyOutput();
        if (IsPaused(now)) return output;
        if (flood && FloodWaiting is int waiting && waiting != floodShown)
        {
            output.Post.Add(FloodBanner(waiting));
        }
        output.Merge(Release(now, breakpoint: true));
        return output;
    }

    /// <summary>The 1 Hz timer: the 2-minute cap, the rate window, the end of a pause, the engine-loss grace.</summary>
    public PolicyOutput Tick(DateTimeOffset now)
    {
        if (PausedUntil is { } until && now >= until) PausedUntil = null;
        var output = EngineDue(now);
        if (IsPaused(now)) return output;
        output.Merge(Release(now, breakpoint: false));
        return output;
    }

    /// <summary>The companion is quitting on purpose: drop a pending engine loss and never post one from here on.</summary>
    public void Quitting()
    {
        quitting = true;
        engineLostAt = null;
    }

    /// <summary>A pending loss whose grace has passed becomes the error banner (unless paused, Off or quitting).</summary>
    private PolicyOutput EngineDue(DateTimeOffset now)
    {
        var output = new PolicyOutput();
        if (engineLostAt is not { } at || quitting || now - at < EngineGrace) return output;
        engineLostAt = null;
        engineLost = true;
        if (IsPaused(now) || Prefs.Error == NotifyMode.Off) return output;
        var b = new Banner(Banner.EngineId, BannerCategory.Error, "theuth engine unreachable",
            "The companion lost the engine; reconnecting.", "theuth", ErrorLevel, Prefs.SoundOnErrors, new ValueList<string>());
        live[b.Id] = new HashSet<string>(StringComparer.Ordinal);
        output.Post.Add(b);
        return output;
    }

    /// <summary>Menu: Pause 1 h / until tomorrow; null resumes. Nothing is posted while paused; the rail is live.</summary>
    public void Pause(DateTimeOffset? until) => PausedUntil = until;

    /// <summary>
    /// The RPC connection dropped / came back. A drop is held for <see cref="EngineGrace"/> (the banner comes from
    /// <see cref="Tick"/> when the grace has passed and the engine is still gone); a reconnect inside the grace drops it
    /// silently; a reconnect after the banner withdraws it.
    /// </summary>
    public PolicyOutput Engine(bool connected, DateTimeOffset now)
    {
        var output = new PolicyOutput();
        if (connected)
        {
            engineLostAt = null;
            engineLost = false;
            if (live.Remove(Banner.EngineId)) output.Withdraw.Add(Banner.EngineId);
            return output;
        }
        if (engineLost || engineLostAt is not null || quitting) return output;
        engineLostAt = now;
        return output;
    }

    // MARK: internals

    private int? FloodWaiting => flood ? waitingNow : null;

    private BannerLevel ErrorLevel => Prefs.TimeSensitiveAvailable ? BannerLevel.TimeSensitive : BannerLevel.Active;

    private PolicyOutput Release(DateTimeOffset now, bool breakpoint)
    {
        var output = new PolicyOutput();
        if (breakpoint) breakpointPending = true;
        if (held.Count == 0 || holdingSince is not { } since) return output;
        if (!breakpointPending && (now - since).TotalSeconds < Cap) return output;
        // §5.5 last-but-one row: hold even at a breakpoint until the rate window passes.
        if (lastDeferrableAt is { } last && (now - last).TotalSeconds < Rate) return output;
        var b = held.Count == 1 ? MakeBanner(held[0], Classify(held[0])) : BatchBanner(held, now);
        output.Post.Add(b);
        held = new();
        holdingSince = null;
        breakpointPending = false;
        lastDeferrableAt = now;
        return output;
    }

    private Banner MakeBanner(AttentionItem item, NotificationClass cls)
    {
        var who = item.Agent ?? Card.ShortSession(item.Session) ?? "theuth";
        BannerLevel level;
        bool sound;
        switch (cls)
        {
            case NotificationClass.Error: level = ErrorLevel; sound = Prefs.SoundOnErrors; break;
            case NotificationClass.Deferrable: level = BannerLevel.Passive; sound = false; break;
            default: level = BannerLevel.Active; sound = false; break;
        }
        var b = new Banner(item.Id, CategoryFor(item), $"{who} · {Label(item)}", item.Title, who, level, sound, ValueList.Of(item.Id));
        live[b.Id] = new HashSet<string>(StringComparer.Ordinal) { item.Id };
        return b;
    }

    private Banner BatchBanner(List<AttentionItem> items, DateTimeOffset now)
    {
        int finished = items.Count(i => i.Kind == AttentionKind.Finished);
        int later = items.Count - finished;
        var parts = new List<string>();
        if (later > 0) parts.Add($"{later} for later");
        if (finished > 0) parts.Add($"{finished} finished");
        var b = new Banner($"batch-{(ulong)now.ToUnixTimeMilliseconds()}", BannerCategory.Batch, "theuth at your break",
            string.Join(" · ", parts), "theuth", BannerLevel.Passive, false, items.Select(i => i.Id).ToValueList());
        live[b.Id] = new HashSet<string>(b.ItemIds, StringComparer.Ordinal);
        return b;
    }

    private Banner FloodBanner(int waiting)
    {
        floodShown = waiting;
        var b = new Banner(Banner.FloodId, BannerCategory.Batch, "theuth", $"{waiting} agents need you — open the panel",
            "theuth", BannerLevel.Active, false, new ValueList<string>());
        live[b.Id] = new HashSet<string>(StringComparer.Ordinal);
        return b;
    }

    internal static string Label(AttentionItem item) => item.Kind.Tag switch
    {
        AttentionKindTag.Approval => "approval",
        AttentionKindTag.Ask => "question",
        AttentionKindTag.Decision => "decision",
        AttentionKindTag.Secret => "secret",
        AttentionKindTag.Handoff => "needs you at its desktop",
        AttentionKindTag.Review => "review",
        AttentionKindTag.Hire => "hire",
        AttentionKindTag.Finished => "finished",
        AttentionKindTag.Blocked => "blocked",
        AttentionKindTag.Budget => Classify(item) == NotificationClass.Error ? "paused · budget" : "budget 80 %",
        AttentionKindTag.Node => "unreachable",
        AttentionKindTag.Show => "show",
        AttentionKindTag.Resume => "resume",
        _ => item.Kind.RawValue,
    };
}
