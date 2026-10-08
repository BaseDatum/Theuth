namespace Theuth.Companion.Core;

/// <summary>What a toast activation asked for (design 134 §4.4, W3's argument sets; W6 item 8 made the fresh-start path read them).</summary>
public sealed record ToastActivation(string Action, string? Banner, string? Item, string? Text)
{
    /// <summary>Argument keys the toasts carry (ToastBuilder writes them; here so Core can parse what Core named).</summary>
    public const string ArgAction = "action", ArgBanner = "banner", ArgItem = "item", InputText = "text";

    /// <summary>Button actions that answer with the card's n-th note-free option.</summary>
    public static readonly string[] OptionActions = { "opt-1", "opt-2", "opt-3" };

    /// <summary>
    /// The toast's <c>Arguments</c> (body or button) and the text box, as the Notifier reads them: no <c>action</c> means the
    /// body was tapped (open); <c>text</c> is trimmed and null when empty.
    /// </summary>
    public static ToastActivation Parse(IEnumerable<KeyValuePair<string, string>> arguments, IEnumerable<KeyValuePair<string, string>>? input)
    {
        // WinRT's IMap projects as IDictionary, not IReadOnlyDictionary: take any pair sequence.
        var args = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var kv in arguments) args[kv.Key] = kv.Value;
        var ins = new Dictionary<string, string>(StringComparer.Ordinal);
        if (input is not null) foreach (var kv in input) ins[kv.Key] = kv.Value;
        string action = args.TryGetValue(ArgAction, out var a) && a.Length > 0 ? a : "open";
        string? banner = args.TryGetValue(ArgBanner, out var b) && b.Length > 0 ? b : null;
        string? item = args.TryGetValue(ArgItem, out var i) && i.Length > 0 ? i : null;
        string? text = ins.TryGetValue(InputText, out var t) && t.Trim().Length > 0 ? t.Trim() : null;
        return new ToastActivation(action, banner, item, text);
    }

    /// <summary>"later" does nothing; everything else acts on the Store.</summary>
    public bool IsNoop => Action == "later";

    /// <summary>Which option (0-based) an <c>opt-n</c> action names; null for every other action.</summary>
    public int? OptionIndex => Array.IndexOf(OptionActions, Action) is var n && n >= 0 ? n : null;

    /// <summary>
    /// Whether to hold the action until the Store's first refetch. A process the click LAUNCHED (<paramref name="launched"/>:
    /// the activation came from <c>GetActivatedEventArgs</c>, not <c>NotificationInvoked</c>) has no items yet and holds everything
    /// but "later", so the panel opens on the right item; a running instance holds only answers while the Store is still empty.
    /// </summary>
    public bool HoldUntilRefetch(bool launched, bool refetchedOnce, int itemCount)
    {
        if (IsNoop) return false;
        if (launched) return !refetchedOnce;
        return !refetchedOnce && itemCount == 0 && Action != "open";
    }
}

/// <summary>Tray › Pause notifications ▸ — which row carries the check mark (134 W6 item 9; the Mac menu's rule in
/// <c>NotificationsInstaller.swift isUntilTomorrow</c>: paused into tomorrow and more than an hour away = "Until tomorrow").</summary>
public enum PauseMenuState { Off, Hour, Tomorrow }

public static class PauseMenu
{
    public static PauseMenuState State(DateTimeOffset? pausedUntil, DateTimeOffset now, DateTimeOffset tomorrow)
    {
        if (pausedUntil is not { } until || until <= now) return PauseMenuState.Off;
        bool sameDayAsTomorrow = until.LocalDateTime.Date == tomorrow.LocalDateTime.Date;
        return sameDayAsTomorrow && (until - now) > TimeSpan.FromHours(1) ? PauseMenuState.Tomorrow : PauseMenuState.Hour;
    }

    /// <summary>The parent row's title while paused: "Pause notifications (until 05:23)"; plain when not paused.</summary>
    public static string Title(DateTimeOffset? pausedUntil, DateTimeOffset now) =>
        pausedUntil is { } u && u > now ? $"Pause notifications (until {u.LocalDateTime:HH:mm})" : "Pause notifications";
}
