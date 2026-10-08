namespace Theuth.Companion.Core;

/// <summary>What the first-run tip decided (design 134 §4.5, W6 item 1) — one per check, logged by the app.</summary>
public enum PinTipVerdict
{
    /// <summary>Post the toast now (and persist <see cref="PinTip.Key"/>).</summary>
    Show,
    /// <summary>The icon is already in the taskbar corner (<c>IsPromoted = 1</c>): nothing to say; persist the key, never ask again.</summary>
    Promoted,
    /// <summary>Explorer has not written <c>NotifyIconSettings\&lt;id&gt;</c> for this exe yet (it does so late): check again.</summary>
    NoEntryYet,
    /// <summary>Shown (or resolved) before for this exe path.</summary>
    AlreadyShown,
    /// <summary>Less than <see cref="PinTip.MinSinceAdd"/> after the first <c>NIM_ADD</c>: too early to trust the registry.</summary>
    TooEarly,
    /// <summary>Notifications are paused (tray › Pause notifications): nothing is posted while paused — check again later.</summary>
    Paused,
    /// <summary>Settings › Notifications › Deferrable = Off: the tip is a deferrable informational toast and stays silent.</summary>
    Off,
}

/// <summary>One <c>HKCU\Control Panel\NotifyIconSettings\&lt;id&gt;</c> row: <c>ExecutablePath</c> and <c>IsPromoted</c> (null = absent).</summary>
public sealed record NotifyIconEntry(string ExecutablePath, int? IsPromoted);

/// <summary>
/// The first-run "pin theuth to the taskbar corner" tip (design 134 §4.5; W2 deviation 7 deferred it). Windows 11 hides a
/// new notification-area icon in the overflow and records the user's choice per exe under
/// <c>HKCU\Control Panel\NotifyIconSettings\&lt;id&gt;</c> (<c>ExecutablePath</c>, <c>IsPromoted</c> DWORD; absent = overflow).
/// The decision is pure and lives here; the app reads the registry, the settings store and the Notifier state and posts one
/// toast — title <see cref="Title"/>, body <see cref="Body"/>, one button <see cref="ButtonLabel"/> → <see cref="SettingsUri"/>.
/// It honours the same rules as W3's informational toasts: nothing while paused, nothing when the deferrable class is Off,
/// the Default scenario (Windows' Do not disturb applies), no sound. Shown once per install: the exe path it was shown
/// (or resolved as promoted) for is persisted under <see cref="Key"/>.
/// </summary>
public static class PinTip
{
    /// <summary>settings.json: the exe path the tip was shown or resolved for (empty = never).</summary>
    public const string Key = "pinTipShownFor";

    /// <summary>Explorer writes the <c>NotifyIconSettings</c> entry late: the first check waits this long after the first <c>NIM_ADD</c>.</summary>
    public static readonly TimeSpan MinSinceAdd = TimeSpan.FromSeconds(60);

    /// <summary>How often the app re-checks after <see cref="PinTipVerdict.NoEntryYet"/> / <see cref="PinTipVerdict.Paused"/>.</summary>
    public static readonly TimeSpan Retry = TimeSpan.FromSeconds(60);

    /// <summary>Re-checks per run after which a missing entry is given up (next launch tries again).</summary>
    public const int MaxNoEntryChecks = 5;

    public const string Title = "Keep theuth in view";

    public const string Body =
        "Windows put theuth's icon in the taskbar overflow. Turn it on under Settings › Personalization › Taskbar › Other system tray icons › theuth to keep the count in the corner.";

    public const string ButtonLabel = "Open taskbar settings";

    public const string SettingsUri = "ms-settings:taskbar";

    /// <summary>The toast's tag and group (one tip at a time; a repost replaces it).</summary>
    public const string Tag = "pin-tip", Group = "theuth";

    /// <summary>
    /// The decision for one check. <paramref name="shownFor"/>: the persisted exe path (<see cref="Key"/>); <paramref name="exePath"/>:
    /// this process; <paramref name="entries"/>: the registry rows; <paramref name="sinceAdd"/>: time since the first NIM_ADD;
    /// <paramref name="paused"/>: the Notifier is paused; <paramref name="deferrableOff"/>: Settings › Notifications › Deferrable = Off.
    /// Order: already shown → off → too early → no entry → promoted → paused → show, so a promoted icon resolves the flag even
    /// while paused, and a pause never persists anything.
    /// </summary>
    public static PinTipVerdict Decide(string? shownFor, string exePath, IEnumerable<NotifyIconEntry> entries, TimeSpan sinceAdd, bool paused, bool deferrableOff)
    {
        if (IsShownFor(shownFor, exePath)) return PinTipVerdict.AlreadyShown;
        if (deferrableOff) return PinTipVerdict.Off;
        if (sinceAdd < MinSinceAdd) return PinTipVerdict.TooEarly;
        var entry = EntryFor(entries, exePath);
        if (entry is null) return PinTipVerdict.NoEntryYet;
        if (entry.IsPromoted == 1) return PinTipVerdict.Promoted;
        if (paused) return PinTipVerdict.Paused;
        return PinTipVerdict.Show;
    }

    /// <summary>Whether <see cref="Key"/> names this exe (case-insensitive, trimmed; Windows paths).</summary>
    public static bool IsShownFor(string? shownFor, string exePath) =>
        !string.IsNullOrWhiteSpace(shownFor) && string.Equals(shownFor.Trim(), exePath.Trim(), StringComparison.OrdinalIgnoreCase);

    /// <summary>The row for this exe (Explorer stores the path as launched; compare case-insensitively).</summary>
    public static NotifyIconEntry? EntryFor(IEnumerable<NotifyIconEntry> entries, string exePath) =>
        entries.FirstOrDefault(e => string.Equals(e.ExecutablePath.Trim(), exePath.Trim(), StringComparison.OrdinalIgnoreCase));

    /// <summary>Whether a verdict ends the checks for this run (<c>Show</c> and <c>Promoted</c> also persist the key).</summary>
    public static bool IsFinal(PinTipVerdict v) => v is PinTipVerdict.Show or PinTipVerdict.Promoted or PinTipVerdict.AlreadyShown or PinTipVerdict.Off;

    /// <summary>Whether the verdict persists <see cref="Key"/> = the exe path.</summary>
    public static bool Persists(PinTipVerdict v) => v is PinTipVerdict.Show or PinTipVerdict.Promoted;

    /// <summary>The log line the app writes per check.</summary>
    public static string Describe(PinTipVerdict v, NotifyIconEntry? entry, TimeSpan sinceAdd, int check) =>
        $"pin tip: check {check} verdict={v} sinceAdd={(int)sinceAdd.TotalSeconds}s entry={(entry is null ? "none" : $"IsPromoted={(entry.IsPromoted is { } p ? p.ToString(System.Globalization.CultureInfo.InvariantCulture) : "absent")}")}";
}
