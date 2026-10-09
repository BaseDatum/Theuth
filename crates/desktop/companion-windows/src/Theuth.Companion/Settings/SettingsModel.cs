using Theuth.Companion.Core;

namespace Theuth.Companion.Settings;

/// <summary>The recorder's verdict under a Settings › Keys row (Swift <c>RowNotice</c>): a collision ("Already used by …"
/// + Use here instead), a reserved chord, or a chord another app holds.</summary>
internal sealed record RowNotice(string Text, Chord? MoveChord = null);

/// <summary>
/// The persisted <see cref="CompanionSettings"/> as one observable the Settings window edits and the Store, the rail,
/// the policy and the Run key read live (130 §4.6, design 134 D12: <c>%LOCALAPPDATA%\theuth\companion\settings.json</c>
/// through Core's <see cref="SettingsStore"/>). Port of <c>SettingsModel</c> (SettingsStore.swift). UI thread only.
/// Edit through <see cref="Update"/>: it saves, then raises <see cref="Changed"/> with the old and the new value.
/// </summary>
internal sealed class SettingsModel
{
    public static SettingsModel Shared { get; } = new(SettingsStore.Load(SettingsStore.DefaultPath));

    private readonly SettingsStore store;

    /// <summary>The current value. Never mutate it directly — go through <see cref="Update"/>.</summary>
    public CompanionSettings Value { get; private set; }

    /// <summary>(old, new) after every saved change.</summary>
    public event Action<CompanionSettings, CompanionSettings>? Changed;

    /// <summary>W6: "Ctrl+Shift+X is held by another app — using Ctrl+Shift+Space"; shown in Settings › Keys and the legend, never a dialog.</summary>
    public string? HotkeyNotice { get; private set; }

    /// <summary>W6: the recorder's verdict on the last chord typed into a row.</summary>
    public Dictionary<KeyAction, RowNotice> RowNotices { get; } = new();

    /// <summary>Raised when <see cref="HotkeyNotice"/> or <see cref="RowNotices"/> changed (the Keys page redraws).</summary>
    public event Action? NoticesChanged;

    public SettingsModel(SettingsStore store)
    {
        this.store = store;
        Value = CompanionSettings.Load(store);
        Dictation = DictationSettings.Load(store);
    }

    /// <summary>153 W153B: Settings › Dictation's companion-local values (own keys in the same store, the PointSettings pattern).</summary>
    public DictationSettings Dictation { get; private set; }

    /// <summary>Raised after <see cref="UpdateDictation"/> saved a change.</summary>
    public event Action? DictationChanged;

    /// <summary>Apply <paramref name="edit"/> to a copy of <see cref="Dictation"/>; when it changed anything, persist and notify.</summary>
    public void UpdateDictation(Action<DictationSettings> edit)
    {
        var next = Dictation.Clone();
        edit(next);
        if (next.Equals(Dictation)) return;
        Dictation = next;
        try { next.Save(store); }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException) { Log.Write($"settings: dictation save failed: {e.Message}"); }
        DictationChanged?.Invoke();
    }

    public KeyTable Keys => Value.Keys;

    /// <summary>149 WW3: the one backing store, shared with the island / voice-key settings so no flush drops the other's keys.</summary>
    internal SettingsStore Store => store;

    /// <summary>Apply <paramref name="edit"/> to a copy; when it changed anything, persist and notify.</summary>
    public void Update(Action<CompanionSettings> edit)
    {
        var old = Value.Clone();
        var next = Value.Clone();
        edit(next);
        if (next.Equals(old)) return;
        Value = next;
        try { next.Save(store); }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException) { Log.Write($"settings: save failed: {e.Message}"); }
        Changed?.Invoke(old, next);
    }

    public void SetHotkeyNotice(string? notice)
    {
        HotkeyNotice = notice;
        NoticesChanged?.Invoke();
    }

    public void SetRowNotice(KeyAction action, RowNotice? notice)
    {
        if (notice is null) RowNotices.Remove(action);
        else RowNotices[action] = notice;
        NoticesChanged?.Invoke();
    }

    public void ClearRowNotices()
    {
        RowNotices.Clear();
        NoticesChanged?.Invoke();
    }
}
