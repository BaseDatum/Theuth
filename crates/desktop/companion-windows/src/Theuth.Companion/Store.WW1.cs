using Theuth.Companion.Core;

namespace Theuth.Companion;

/// <summary>
/// 149 WW1's app-side members of the Store (the data and the thin actions are in Core's <c>StoreModel.WW1.cs</c>, so they
/// are Mac-tested; this partial only owns what needs the WinUI process): the global-hotkey forwarding and the engine
/// connection the surfaces call directly for anything the Store does not wrap. Thread: everything here runs on
/// <see cref="Store.Dispatcher"/>, like the rest of the Store.
///
/// What WW2 / WW3 read (all on <c>StoreModel</c>, raised as <c>PropertyChanged</c> on the UI thread):
///   agents · <c>Activity</c>, <c>Agents</c> (W1), <c>Sessions</c>, <c>SessionsStatus</c>, <c>Mesh</c>, <c>SelfEngineId</c>, <c>HostNameOf</c>, <c>SessionOf</c>, <c>ChildrenOf</c>, <c>ContextFor</c>;
///   meeting · <c>Meeting</c>, <c>Topics</c>, <c>Briefs</c>, <c>MeetingCursor</c>, <c>Live</c>, <c>MeetingLive</c>, <c>MeetingMicLevel</c>, <c>MeetingSystemLevel</c>, <c>MeetingListening</c>;
///   events · <c>EventReceived(type, json)</c> (raw, last), <c>MeetingEventReceived</c>, <c>SubagentSpawnedReceived</c>, <c>ActivityAppendedReceived</c>, <c>DictationEventReceived</c>, <c>SessionsRefetched</c>, <c>MeetingRefetched</c>;
///   keys · <c>GlobalKeyPressed(KeyAction)</c> / <c>GlobalKeyReleased(KeyAction, held)</c> for <c>VoiceKey</c>, <c>OpenAgents</c>, <c>MeetingPtt</c>, <c>MeetingEye</c> (and the W1–W6 rows);
///   actions · <c>CancelTreeAsync</c>, <c>SendInputAsync(session, text, files)</c>, <c>SteerAsync(session, text, files)</c>, <c>ScreenContextAsync</c>, <c>PutAttachmentAsync</c>, <c>Meeting*Async</c>, <c>BriefGet/SearchAsync</c>, <c>SpeakAsync</c>, <c>StopSpeakingAsync</c>, <c>AgentPauseAsync</c>, <c>Refetch149Async</c>, <c>RefetchMeetingAsync</c>.
/// </summary>
public sealed partial class Store
{
    private bool hotkeysInstalled;

    /// <summary>
    /// Forwards <see cref="Hotkeys.Pressed"/> / <see cref="Hotkeys.Released"/> to <c>GlobalKeyPressed</c> / <c>GlobalKeyReleased</c>
    /// (once; the surfaces call it right after <c>Hotkeys.Apply</c>). Every registered row is forwarded; consumers filter by action.
    /// Hold ≥ 200 ms / double-tap / Esc-between-press-and-send are WW3's <c>PointMachine</c>, not decided here.
    /// </summary>
    internal void InstallHotkeys(Hotkeys hotkeys)
    {
        if (hotkeysInstalled) return;
        hotkeysInstalled = true;
        hotkeys.Pressed += GlobalKeyDown;
        hotkeys.Released += GlobalKeyUp;
        // §8: the meeting push-to-talk and eye rows are registered only while a meeting is live — re-applied on every change.
        hotkeys.Gate = a => a is not (KeyAction.MeetingPtt or KeyAction.MeetingEye) || MeetingLive;
        bool live = MeetingLive;
        PropertyChanged += (_, e) =>
        {
            if (e.PropertyName != nameof(MeetingLive) || MeetingLive == live) return;
            live = MeetingLive;
            Log.Write($"149: meeting live={live.ToString().ToLowerInvariant()} — re-registering the global rows");
            hotkeys.Apply(hotkeys.Applied ?? Keys);
        };
        hotkeys.Apply(hotkeys.Applied ?? Keys);
        Log.Write("149: global keys forwarded to Store (voiceKey, openAgents; meetingPtt/meetingEye while a meeting is live)");
    }
}
