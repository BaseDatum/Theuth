using System.Security.Cryptography;
using System.Text;
using Microsoft.Windows.AppNotifications;
using Microsoft.Windows.AppNotifications.Builder;
using Theuth.Companion.Core;

namespace Theuth.Companion.Notifications;

/// <summary>
/// One <see cref="Banner"/> → one <see cref="AppNotification"/> (design 134 §4.4, D8). Categories become argument sets
/// (<c>action=open|opt-1|opt-2|opt-3|reply|deny-note|send-back|clear|later</c>, <c>banner=&lt;id&gt;</c>, <c>item=&lt;id&gt;</c>);
/// per-agent stacking is <c>SetGroup(agent)</c>, replacement/withdrawal is <c>SetTag</c>; buttons carry the card's ≤ 3
/// option labels, a text box carries Reply / Deny with note / Send back; blocking kinds are silent (<c>MuteAudio</c>),
/// errors sound; <see cref="BannerLevel.TimeSensitive"/> is <c>AppNotificationScenario.Urgent</c> (the Do-not-disturb
/// opt-in; Windows asks once), everything else <c>Default</c> — the companion respects Do not disturb (R2, Sky).
/// </summary>
internal static class ToastBuilder
{
    public const string ArgAction = "action", ArgBanner = "banner", ArgItem = "item", ArgCategory = "kind";
    public const string InputText = "text";
    public static readonly string[] OptionActions = { "opt-1", "opt-2", "opt-3" };

    /// <summary>Tags and groups are capped at 64 chars by the platform; long ids keep a readable head plus a stable hash.</summary>
    public static string TagFor(string id)
    {
        if (id.Length <= 64) return id;
        var hash = Convert.ToHexString(SHA1.HashData(Encoding.UTF8.GetBytes(id)))[..16].ToLowerInvariant();
        return id[..40] + "~" + hash;
    }

    public static string GroupFor(string thread) => TagFor(thread.Length == 0 ? "theuth" : thread);

    /// <summary>
    /// <paramref name="options"/>: the card's option labels that need no note (≤ 3, in key order); <paramref name="reply"/>:
    /// the text-box action for the category (null = none); <paramref name="urgentSupported"/>: <c>IsUrgentScenarioSupported()</c>.
    /// </summary>
    public static AppNotification Build(Banner b, IReadOnlyList<string> options, string tag, string group, bool urgentSupported)
    {
        bool receipt = b.Id.StartsWith("receipt-", StringComparison.Ordinal);
        var builder = new AppNotificationBuilder()
            .AddArgument(ArgAction, receipt ? "later" : "open")
            .AddArgument(ArgBanner, Safe(b.Id))
            .AddArgument(ArgCategory, NotificationReplay.Camel(b.Category))
            .AddText(b.Title)
            .AddText(b.Body)
            .SetTag(tag)
            .SetGroup(group);
        if (b.ItemIds.Count == 1) builder.AddArgument(ArgItem, Safe(b.ItemIds[0]));
        if (!b.Sound) builder.MuteAudio();
        if (b.Level == BannerLevel.TimeSensitive && urgentSupported) builder.SetScenario(AppNotificationScenario.Urgent);
        if (!receipt) AddButtons(builder, b, options);
        return builder.BuildNotification();
    }

    /// <summary>Swift's category table (Notifier.category), per toast: option buttons from the card, then the category's own actions.</summary>
    private static void AddButtons(AppNotificationBuilder builder, Banner b, IReadOnlyList<string> options)
    {
        int n = 0;
        foreach (var label in options.Take(3))
        {
            builder.AddButton(Button(label, OptionActions[n], b.Id));
            n++;
        }
        switch (b.Category)
        {
            case BannerCategory.Question:
                builder.AddTextBox(InputText, "Your answer", "Reply");
                builder.AddButton(Button("Reply", "reply", b.Id).SetInputId(InputText));
                break;
            case BannerCategory.Approval:
                builder.AddTextBox(InputText, "Why not", "Deny with note");
                builder.AddButton(Button("Deny with note", "deny-note", b.Id).SetInputId(InputText));
                break;
            case BannerCategory.Review:
                builder.AddTextBox(InputText, "What to change", "Send back");
                builder.AddButton(Button("Send back", "send-back", b.Id).SetInputId(InputText));
                builder.AddButton(Button("Open", "open", b.Id));
                break;
            case BannerCategory.Finished:
                builder.AddButton(Button("Clear", "clear", b.Id));
                break;
            case BannerCategory.Batch:
                builder.AddButton(Button("Open panel", "open", b.Id));
                builder.AddButton(Button("Later", "later", b.Id));
                break;
            default: // Error
                builder.AddButton(Button("Open", "open", b.Id));
                break;
        }
    }

    private static AppNotificationButton Button(string label, string action, string bannerId) =>
        new AppNotificationButton(label).AddArgument(ArgAction, action).AddArgument(ArgBanner, Safe(bannerId));

    /// <summary>Argument values cannot carry the separators <c>;</c> and <c>=</c>.</summary>
    private static string Safe(string s) => s.Replace(';', ',').Replace('=', ':');

    /// <summary>The Swift log line: <c>post &lt;category&gt; level=… sound=… thread=… id=… title="…" body="…" items=N</c>.</summary>
    public static string Describe(Banner b, string tag, string group) =>
        $"post {NotificationReplay.Camel(b.Category)} level={NotificationReplay.Camel(b.Level)} sound={(b.Sound ? "true" : "false")} thread={b.Thread} id={b.Id} tag={tag} group={group} title=\"{b.Title}\" body=\"{b.Body}\" items={b.ItemIds.Count}";
}
