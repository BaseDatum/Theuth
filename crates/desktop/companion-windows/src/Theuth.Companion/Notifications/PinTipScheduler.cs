using Microsoft.UI.Dispatching;
using Microsoft.Win32;
using Theuth.Companion.Core;
using Theuth.Companion.Settings;

namespace Theuth.Companion.Notifications;

/// <summary>
/// 134 W6 item 1 (§4.5): shows the first-run "pin theuth to the taskbar corner" tip once per install, and only when the
/// notification-area icon is NOT promoted. The clock starts at the tray's first <c>NIM_ADD</c>; the first check is
/// <see cref="PinTip.MinSinceAdd"/> later (Explorer writes <c>HKCU\Control Panel\NotifyIconSettings\&lt;id&gt;</c> late), then
/// every <see cref="PinTip.Retry"/> while the verdict is <c>NoEntryYet</c> (≤ <see cref="PinTip.MaxNoEntryChecks"/> times) or
/// <c>Paused</c>. The registry is read again just before posting. The decision itself is Core's <see cref="PinTip.Decide"/>;
/// the exe path it was shown or resolved for goes to <see cref="CompanionSettings.PinTipShownFor"/> (settings.json).
/// Every check writes one <c>pin tip:</c> log line.
/// </summary>
internal sealed class PinTipScheduler : IDisposable
{
    private const string Root = @"Control Panel\NotifyIconSettings";

    private readonly Notifier notifier;
    private readonly SettingsModel settings;
    private readonly DispatcherQueue dispatcher;
    private readonly Func<DateTimeOffset?> trayAddedAt;
    private readonly string exePath;
    private DispatcherQueueTimer? timer;
    private int checks;
    private int noEntryChecks;

    public PinTipScheduler(Notifier notifier, SettingsModel settings, DispatcherQueue dispatcher, Func<DateTimeOffset?> trayAddedAt)
    {
        this.notifier = notifier;
        this.settings = settings;
        this.dispatcher = dispatcher;
        this.trayAddedAt = trayAddedAt;
        exePath = Environment.ProcessPath ?? "";
    }

    /// <summary>The last verdict (the probe and the as-built evidence read it).</summary>
    public PinTipVerdict? Last { get; private set; }

    public void Start()
    {
        if (exePath.Length == 0) { Log.Write("pin tip: no process path — off"); return; }
        if (PinTip.IsShownFor(settings.Value.PinTipShownFor, exePath))
        {
            Last = PinTipVerdict.AlreadyShown;
            Log.Write($"pin tip: already shown for this exe ({PinTip.Key} set) — off");
            return;
        }
        if (trayAddedAt() is not { } added)
        {
            Log.Write("pin tip: no tray icon (NIM_ADD failed) — off");
            return;
        }
        var wait = PinTip.MinSinceAdd - (DateTimeOffset.Now - added);
        if (wait < TimeSpan.FromSeconds(1)) wait = TimeSpan.FromSeconds(1);
        Log.Write($"pin tip: armed — first check in {(int)wait.TotalSeconds}s (NIM_ADD at {added:HH:mm:ss.fff})");
        Schedule(wait);
    }

    private void Schedule(TimeSpan after)
    {
        timer?.Stop();
        var t = dispatcher.CreateTimer();
        t.Interval = after;
        t.IsRepeating = false;
        t.Tick += (_, _) => Guard.Run("pin tip check", Check);
        timer = t;
        t.Start();
    }

    private void Check()
    {
        checks++;
        var since = trayAddedAt() is { } added ? DateTimeOffset.Now - added : TimeSpan.Zero;
        var entries = ReadEntries();
        var verdict = PinTip.Decide(settings.Value.PinTipShownFor, exePath, entries, since, notifier.IsPaused, notifier.DeferrableOff);
        Log.Write(PinTip.Describe(verdict, PinTip.EntryFor(entries, exePath), since, checks) + $" entries={entries.Count} paused={(notifier.IsPaused ? "true" : "false")}");
        if (verdict == PinTipVerdict.Show)
        {
            // Again just before showing (Explorer may have promoted it in the meantime; the user may have paused us).
            var again = ReadEntries();
            verdict = PinTip.Decide(settings.Value.PinTipShownFor, exePath, again, since, notifier.IsPaused, notifier.DeferrableOff);
            if (verdict != PinTipVerdict.Show) Log.Write(PinTip.Describe(verdict, PinTip.EntryFor(again, exePath), since, checks) + " (re-read before showing)");
        }
        Last = verdict;
        switch (verdict)
        {
            case PinTipVerdict.Show:
                if (notifier.ShowPinTip()) Persist("shown");
                else Log.Write("pin tip: not posted — will not retry this run");
                return;
            case PinTipVerdict.Promoted:
                Persist("already promoted");
                return;
            case PinTipVerdict.NoEntryYet:
                if (++noEntryChecks >= PinTip.MaxNoEntryChecks)
                {
                    Log.Write($"pin tip: no NotifyIconSettings entry after {noEntryChecks} checks — giving up this run (next launch tries again)");
                    return;
                }
                Schedule(PinTip.Retry);
                return;
            case PinTipVerdict.TooEarly:
            case PinTipVerdict.Paused:
                Schedule(PinTip.Retry);
                return;
            default: // AlreadyShown · Off
                return;
        }
    }

    private void Persist(string why)
    {
        settings.Update(s => s.PinTipShownFor = exePath);
        Log.Write($"pin tip: {why} — {PinTip.Key} = {exePath}");
    }

    /// <summary>Every <c>NotifyIconSettings\&lt;id&gt;</c> row with an <c>ExecutablePath</c>; empty when the key is absent (older Windows).</summary>
    private static List<NotifyIconEntry> ReadEntries()
    {
        var result = new List<NotifyIconEntry>();
        try
        {
            using var root = Registry.CurrentUser.OpenSubKey(Root);
            if (root is null) return result;
            foreach (var name in root.GetSubKeyNames())
            {
                using var k = root.OpenSubKey(name);
                if (k?.GetValue("ExecutablePath") is not string exe || exe.Length == 0) continue;
                int? promoted = k.GetValue("IsPromoted") is int p ? p : null;
                result.Add(new NotifyIconEntry(exe, promoted));
            }
        }
        catch (Exception e) when (e is System.Security.SecurityException or UnauthorizedAccessException or IOException)
        {
            Log.Write($"pin tip: NotifyIconSettings unreadable: {e.Message}");
        }
        return result;
    }

    public void Dispose()
    {
        timer?.Stop();
        timer = null;
    }
}
