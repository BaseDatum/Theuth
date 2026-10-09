namespace Theuth.Companion.Core;

/// <summary>
/// 149 voice-key fix R3: "Esc cancels" during the hold. While the voice key is held, the user's Esc reaches Windows as
/// &lt;the chord's modifiers&gt;+Esc (Ctrl+Alt+Esc for the default Ctrl+Alt+Space), so the plain-Esc hotkey never matches. The
/// controller therefore registers a second Esc hotkey carrying the bound voice key's modifiers — derived here, so a rebound key
/// works too — except the one combination Windows owns: Ctrl+Shift+Esc is Task Manager and is never registered.
/// </summary>
public static class EscDuringHold
{
    /// <summary>Windows opens Task Manager on this chord; registering it would swallow it system-wide.</summary>
    public const ChordModifiers TaskManager = ChordModifiers.Control | ChordModifiers.Shift;

    /// <summary>
    /// The modifiers to register Esc with while <paramref name="voiceKey"/> is held: null when there is nothing to add (no voice key,
    /// no modifiers — plain Esc already matches) or when the combination is reserved (<see cref="TaskManager"/>, or any Win chord).
    /// </summary>
    public static ChordModifiers? Modifiers(Chord? voiceKey)
    {
        if (voiceKey is null) return null;
        var m = voiceKey.Modifiers & ~ChordModifiers.Win; // a Win chord is the OS's (KeyTable.Check refuses them) and would be refused here too
        if (m == ChordModifiers.None) return null;
        if (m == TaskManager) return null;
        return m;
    }

    /// <summary>Why <see cref="Modifiers"/> answered null for a chord with modifiers, for the log; null when it did not.</summary>
    public static string? SkipReason(Chord? voiceKey)
    {
        if (voiceKey is null || voiceKey.Modifiers == ChordModifiers.None) return null;
        var m = voiceKey.Modifiers & ~ChordModifiers.Win;
        if (m == TaskManager) return "Ctrl+Shift+Esc is Task Manager — not registered; Esc cancels once the modifiers are released";
        if (m == ChordModifiers.None) return "a Win chord's Esc is Windows' own — not registered";
        return null;
    }
}
