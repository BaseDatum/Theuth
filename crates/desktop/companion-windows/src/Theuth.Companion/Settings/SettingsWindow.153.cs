#pragma warning disable CS0618 // NAudio 3.1: WasapiOut is obsolete in favour of WasapiPlayerBuilder; SpeechPlayer pairs the same classic API.
using System.Globalization;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;
using Theuth.Companion.Ui;

namespace Theuth.Companion.Settings;

/// <summary>
/// 153 W153B — Settings › Dictation (W-n: the companion's tab; the desktop's is Settings › Voice). Every engine value goes through
/// the §14.2 RPCs and lives in the engine's <c>config.yaml</c> (§14.3) — microphone (<c>list_input_devices</c> /
/// <c>set_input_devices</c>, read back from <c>voice_status.input_devices</c>), shaping per surface (<c>set_shape</c>), spoken
/// commands (<c>set_end_phrases</c>), the polish model (<c>polish_model</c>; changed in theuth's Settings, no second picker, §9.2),
/// the dictionary (<c>dictionary_list</c> / <c>dictionary_put</c> / <c>dictionary_delete</c>), history and privacy
/// (<c>dictation_history(50)</c>, ▶ <c>dictation_audio_get</c>, <c>dictation_history_forget</c>) and the "Last dictation" line
/// (<c>dictation_status().last</c>). The contract has no getter for the shape presets or the end phrases, so the controls show
/// the last value THIS companion wrote (H3-B1), remembered in <see cref="DictationSettings"/>; "Open the mic on key-down" is
/// companion-local (§8.5). UIA: <c>settings-dictation-*</c>. W153D's any-app rows plug in through <see cref="AnyAppSettings"/>.
/// </summary>
internal sealed partial class SettingsWindow
{
    /// <summary>Bumped on every build of the page: an RPC answer for an older page is dropped.</summary>
    private int dictationPage;

    /// <summary>
    /// W153D's seam (153 §13 / §14.3: app profiles with the per-app polish opt-in, "leave the text on the clipboard", "check that the
    /// paste landed"): implement this partial method in a <c>SettingsWindow.153D.cs</c> partial — no line in this file changes.
    /// </summary>
    partial void AnyAppSettings(StackPanel page);

    private void Dictation(StackPanel page)
    {
        int gen = ++dictationPage;
        bool Current() => gen == dictationPage && CurrentPage == "dictation";
        var prefs = model.Dictation;
        Heading(page, "Dictation");
        Caption(page, "How spoken words become text in the Ask bar, the panel's row mics and other apps. The engine keeps these settings for every client — the desktop, the terminal UI, the phone and this companion.");
        var status = Caption(page, "");
        AutomationProperties.SetAutomationId(status, "settings-dictation-status");
        AutomationProperties.SetLiveSetting(status, Microsoft.UI.Xaml.Automation.Peers.AutomationLiveSetting.Polite);
        void Say(string text) { if (Current()) status.Text = text; }

        // MARK: microphone (§8.1, §8.5)
        SubHeading(page, "Microphone");
        var mic = Combo(page, "Input device", "settings-dictation-mic", new[] { ("", "System default") }, "", _ => { });
        mic.IsEnabled = false;
        var micNote = Caption(page, "Loading the engine's devices…");
        AutomationProperties.SetAutomationId(micNote, "settings-dictation-mic-note");
        _ = LoadMicrophones(mic, micNote, Current, Say);
        Toggle(page, "Open the mic on key-down", "settings-dictation-mic-early", prefs.MicOnKeyDown, on => model.UpdateDictation(d => d.MicOnKeyDown = on));
        Caption(page, "The recorder starts at the press of the Ask-bar or panel key, so the first word is never cut; a tap still never starts a dictation. Windows shows the microphone in use for a moment on every tap.");

        // MARK: shaping per surface (§4.5)
        SubHeading(page, "Shaping");
        Caption(page, "Full: spoken punctuation, numbers, literals, your dictionary and the spoken commands. Plain: punctuation, numbers and literals only. Off: the model's words as heard.");
        var presets = Enum.GetValues<ShapePreset>().Select(p => (p.Word(), DictationSettings.PresetLabel(p))).ToList();
        foreach (var surface in Enum.GetValues<Surface>())
        {
            var s = surface;
            Combo(page, DictationSettings.SurfaceLabel(s), $"settings-dictation-shape-{s.Word()}", presets, prefs.PresetFor(s)?.Word() ?? ShapePreset.Full.Word(),
                v => _ = SetShape(s, DictationSettings.ParsePreset(v) ?? ShapePreset.Full, Say));
        }
        Caption(page, "Shown: the last choice made here (theuth has no reader for these yet); \"Full\" is the engine's default.");

        // MARK: spoken commands (§7.2)
        SubHeading(page, "Spoken commands");
        var phrases = prefs.EndPhrases ?? new EndPhrases();
        var send = Field(page, "Send phrases", "settings-dictation-send", DictationSettings.JoinPhrases(phrases.Send));
        var scratch = Field(page, "Scratch phrases", "settings-dictation-scratch", DictationSettings.JoinPhrases(phrases.Scratch));
        var literal = Field(page, "Literal word", "settings-dictation-literal", phrases.Literal);
        var save = new Button { Content = "Save spoken commands" };
        AutomationProperties.SetAutomationId(save, "settings-dictation-phrases-save");
        save.Click += (_, _) => _ = SetPhrases(new EndPhrases
        {
            Send = DictationSettings.ParsePhrases(send.Text),
            Scratch = DictationSettings.ParsePhrases(scratch.Text),
            Literal = literal.Text.Trim().Length == 0 ? "literal" : literal.Text.Trim(),
        }, Say);
        page.Children.Add(save);
        Caption(page, "Say a send phrase last and the field is sent at once — no 3 s hold. A scratch phrase drops what you said just before it. Say the literal word first to write a phrase as words. Comma-separated.");

        // MARK: polish (§9.1–9.2)
        SubHeading(page, "Polish");
        var polish = Caption(page, "Polish model: …");
        AutomationProperties.SetAutomationId(polish, "settings-dictation-polish-model");
        _ = LoadPolishModel(polish, Current);
        var change = new HyperlinkButton { Content = "Change in theuth's Settings › Voice…", Padding = new Thickness(0, 2, 0, 2) };
        AutomationProperties.SetAutomationId(change, "settings-dictation-polish-change");
        change.Click += (_, _) => _ = Links.Open(new Uri("theuth://settings/voice"));
        page.Children.Add(change);
        Caption(page, "Agent prompts are never polished: the Ask bar, the panel and the row mics always send the shaped text. Polish applies only to text for people — documents and messages in theuth, and other apps you turn it on for.");
        Caption(page, "Other apps: polish, terminal spacing and slash commands are set per app, with dictation into any app.");
        AnyAppSettings(page);

        // MARK: dictionary (§5.3, §5.6)
        SubHeading(page, "Dictionary");
        Caption(page, "Your words and the ones learned from your corrections. The packaged technical terms are always on.");
        var dict = new StackPanel { Spacing = 4 };
        AutomationProperties.SetAutomationId(dict, "settings-dictation-dictionary");
        page.Children.Add(dict);
        var addRow = new Grid { ColumnSpacing = 8 };
        addRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        addRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        addRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var canonical = new TextBox { PlaceholderText = "Write it as (Parakeet TDT)" };
        AutomationProperties.SetAutomationId(canonical, "settings-dictation-dict-canonical");
        AutomationProperties.SetName(canonical, "Write it as");
        var spoken = new TextBox { PlaceholderText = "Said as (parakeet tee dee tee)" };
        AutomationProperties.SetAutomationId(spoken, "settings-dictation-dict-spoken");
        AutomationProperties.SetName(spoken, "Said as");
        var add = new Button { Content = "Add" };
        AutomationProperties.SetAutomationId(add, "settings-dictation-dict-add");
        Grid.SetColumn(spoken, 1);
        Grid.SetColumn(add, 2);
        addRow.Children.Add(canonical);
        addRow.Children.Add(spoken);
        addRow.Children.Add(add);
        page.Children.Add(addRow);
        add.Click += (_, _) =>
        {
            var c = canonical.Text.Trim();
            if (c.Length == 0) { Say("Type how the word is written first."); return; }
            var entry = new Entry { Canonical = c, Spoken = DictationSettings.ParsePhrases(spoken.Text), Source = EntrySource.User };
            canonical.Text = "";
            spoken.Text = "";
            _ = PutEntry(entry, dict, Current, Say);
        };
        _ = LoadDictionary(dict, Current, Say);

        // MARK: history and privacy (§6.5, §6.7, §8.7)
        SubHeading(page, "History");
        var last = Caption(page, "Last dictation: …");
        AutomationProperties.SetAutomationId(last, "settings-dictation-last");
        _ = LoadLast(last, Current);
        var history = new StackPanel { Spacing = 4 };
        AutomationProperties.SetAutomationId(history, "settings-dictation-history");
        page.Children.Add(history);
        var forgetAll = new Button { Content = "Forget all" };
        AutomationProperties.SetAutomationId(forgetAll, "settings-dictation-history-forget-all");
        forgetAll.Click += (_, _) => _ = Forget(null, history, Current, Say);
        page.Children.Add(forgetAll);
        Caption(page, "Kept on this computer only — the last 200 dictations and up to 64 MiB of their audio, so a correction can be learned and replayed. The agent never reads it, and no channel or other machine can reach it. Learn from my corrections, Keep audio and the history switch itself are in theuth's Settings › Voice.");
        var voice = new HyperlinkButton { Content = "Open theuth's Settings › Voice…", Padding = new Thickness(0, 2, 0, 2) };
        AutomationProperties.SetAutomationId(voice, "settings-dictation-history-settings");
        voice.Click += (_, _) => _ = Links.Open(new Uri("theuth://settings/voice"));
        page.Children.Add(voice);
        _ = LoadHistory(history, Current, Say);
    }

    // MARK: loaders and actions (UI thread; every answer checks the page is still the one that asked)

    private async Task LoadMicrophones(ComboBox mic, TextBlock note, Func<bool> current, Action<string> say)
    {
        ValueList<InputDevice> devices;
        ValueList<string> prefs;
        try
        {
            devices = await store.Client.ListInputDevicesAsync();
            prefs = (await store.Client.VoiceStatusAsync()).InputDevices;
        }
        catch (Exception e)
        {
            if (current()) note.Text = $"The engine did not list its devices ({Describe(e)}).";
            return;
        }
        MicPreference.Names = prefs;
        if (!current()) return;
        var selected = prefs.FirstOrDefault() ?? "";
        bool filling = true;
        mic.Items.Clear();
        void Add(string value, string title)
        {
            var item = new ComboBoxItem { Content = title, Tag = value };
            AutomationProperties.SetAutomationId(item, $"settings-dictation-mic-{(value.Length == 0 ? "default" : Slug(value))}");
            mic.Items.Add(item);
            if (value == selected) mic.SelectedItem = item;
        }
        Add("", "System default");
        foreach (var d in devices) Add(d.Name, d.Default ? $"{d.Name} (default)" : d.Name);
        if (selected.Length > 0 && mic.SelectedItem is null) Add(selected, $"{selected} (not connected)");
        if (mic.SelectedItem is null) mic.SelectedIndex = 0;
        mic.IsEnabled = true;
        filling = false;
        note.Text = prefs.Count > 1
            ? $"The engine tries {string.Join(" → ", prefs)} in order, then the system default. Picking one here makes it the only choice."
            : "The engine's microphone and this companion's own recorder use the same choice.";
        mic.SelectionChanged += async (_, _) =>
        {
            if (filling || mic.SelectedItem is not ComboBoxItem { Tag: string v }) return;
            try
            {
                await store.Client.SetInputDevicesAsync(v.Length == 0 ? Array.Empty<string>() : new[] { v });
                MicPreference.Names = v.Length == 0 ? Array.Empty<string>() : new[] { v };
                say(v.Length == 0 ? "Microphone: the system default." : $"Microphone: {v}.");
                Log.Write($"settings: set_input_devices([{v}]) ok");
            }
            catch (Exception e)
            {
                say($"The microphone did not change ({Describe(e)}).");
            }
        };
    }

    private async Task SetShape(Surface surface, ShapePreset preset, Action<string> say)
    {
        try
        {
            await store.Client.SetShapeAsync(surface, preset);
            model.UpdateDictation(d => d.ShapePresets[surface.Word()] = preset.Word());
            say($"{DictationSettings.SurfaceLabel(surface)}: {preset.Word()}.");
            Log.Write($"settings: set_shape({surface.Word()}, {preset.Word()}) ok");
        }
        catch (Exception e)
        {
            say($"Shaping did not change ({Describe(e)}).");
        }
    }

    private async Task SetPhrases(EndPhrases phrases, Action<string> say)
    {
        try
        {
            await store.Client.SetEndPhrasesAsync(phrases);
            model.UpdateDictation(d => d.EndPhrases = phrases);
            say($"Spoken commands saved: send \"{DictationSettings.JoinPhrases(phrases.Send)}\" · scratch \"{DictationSettings.JoinPhrases(phrases.Scratch)}\" · literal \"{phrases.Literal}\".");
            Log.Write($"settings: set_end_phrases({TheuthJson.Serialize(phrases)}) ok");
        }
        catch (Exception e)
        {
            say($"The spoken commands did not change ({Describe(e)}).");
        }
    }

    private async Task LoadPolishModel(TextBlock line, Func<bool> current)
    {
        string text;
        try { text = await store.Client.PolishModelAsync() is { } m ? $"Polish model: {(m.Name.Length > 0 ? m.Name : m.Id)}{(m.Provider.Length > 0 ? $" · {m.Provider}" : "")}" : "Polish model: Chain (title model, then session)"; }
        catch (Exception e) { text = $"Polish model: unknown ({Describe(e)})"; }
        if (current()) line.Text = text;
    }

    private async Task LoadDictionary(StackPanel list, Func<bool> current, Action<string> say)
    {
        ValueList<Entry> entries;
        try { entries = await store.Client.DictionaryListAsync(); }
        catch (Exception e)
        {
            if (current()) { list.Children.Clear(); list.Children.Add(Muted($"The engine did not list the dictionary ({Describe(e)}).")); }
            return;
        }
        if (!current()) return;
        list.Children.Clear();
        if (entries.Count == 0) list.Children.Add(Muted("No words of yours yet — add one below, or correct a dictation twice and accept the card."));
        int n = 0;
        foreach (var e in entries.OrderBy(e => e.Canonical, StringComparer.OrdinalIgnoreCase))
        {
            var entry = e;
            var heard = entry.Spoken.Concat(entry.Variants.Select(v => v.Heard)).Distinct(StringComparer.OrdinalIgnoreCase).ToList();
            var row = Row($"{entry.Canonical}{(heard.Count > 0 ? "  ←  " + string.Join(", ", heard) : "")}", $"{entry.Source} · {entry.Kind}{(entry.Scope.Kind == "global" ? "" : $" · {entry.Scope.SurfaceWord ?? entry.Scope.App ?? entry.Scope.Kind}")}",
                "Delete", $"settings-dictation-dict-{n++}", async () =>
                {
                    try
                    {
                        bool removed = await store.Client.DictionaryDeleteAsync(entry.Id);
                        say(removed ? $"Removed {entry.Canonical}." : $"{entry.Canonical} was already gone.");
                    }
                    catch (Exception ex) { say($"Not removed ({Describe(ex)})."); }
                    await LoadDictionary(list, current, say);
                });
            list.Children.Add(row);
        }
    }

    private async Task PutEntry(Entry entry, StackPanel list, Func<bool> current, Action<string> say)
    {
        try
        {
            var stored = await store.Client.DictionaryPutAsync(entry);
            say($"Added {stored.Canonical}.");
            Log.Write($"settings: dictionary_put({stored.Canonical}) → {stored.Id}");
        }
        catch (Exception e) { say($"Not added ({Describe(e)})."); }
        await LoadDictionary(list, current, say);
    }

    private async Task LoadLast(TextBlock line, Func<bool> current)
    {
        string text;
        try { text = DictationSettings.TimingsLine((await store.Client.DictationStatusAsync()).Last) is { } t ? $"Last dictation: {t}" : "Last dictation: none since the engine started."; }
        catch (Exception e) { text = $"Last dictation: unknown ({Describe(e)})"; }
        if (current()) line.Text = text;
    }

    private async Task LoadHistory(StackPanel list, Func<bool> current, Action<string> say)
    {
        ValueList<HistoryRow> rows;
        try { rows = await store.Client.DictationHistoryAsync(50); }
        catch (Exception e)
        {
            if (current()) { list.Children.Clear(); list.Children.Add(Muted($"No history from the engine ({Describe(e)}).")); }
            return;
        }
        if (!current()) return;
        list.Children.Clear();
        if (rows.Count == 0) list.Children.Add(Muted("Nothing dictated yet."));
        int n = 0;
        foreach (var r in rows)
        {
            var row = r;
            var when = DateTimeOffset.FromUnixTimeMilliseconds((long)row.StartedMs).ToLocalTime().ToString("HH:mm", CultureInfo.InvariantCulture);
            var words = row.Polished is { Length: > 0 } p && p != row.Shaped ? $"{row.Shaped}  →  {p} (polished)" : row.Shaped.Length > 0 ? row.Shaped : row.Raw;
            var meta = $"{when} · {(row.Surface is { } s ? DictationSettings.SurfaceLabel(s) : "?")} → {row.Destination}{(row.Reason.StartsWith("error", StringComparison.Ordinal) ? " · " + row.Reason : "")}";
            var grid = Row(words.Length == 0 ? "(nothing recognized)" : words, meta, "Forget", $"settings-dictation-history-{n}", () => _ = Forget(row.Id, list, current, say));
            ToolTipService.SetToolTip(grid, row.Raw.Length > 0 ? $"Heard: {row.Raw}" : "Heard: nothing");
            if (row.HasAudio)
            {
                var play = new HyperlinkButton { Content = "▶", Padding = new Thickness(6, 0, 6, 0), VerticalAlignment = VerticalAlignment.Center };
                AutomationProperties.SetAutomationId(play, $"settings-dictation-history-{n}-play");
                AutomationProperties.SetName(play, "Play what you said");
                play.Click += (_, _) => _ = PlayHistory(row.Id, say);
                Grid.SetColumn(play, 1);
                grid.Children.Add(play);
            }
            n++;
            list.Children.Add(grid);
        }
    }

    private async Task Forget(string? id, StackPanel list, Func<bool> current, Action<string> say)
    {
        try
        {
            await store.Client.DictationHistoryForgetAsync(id);
            say(id is null ? "History cleared." : "Forgotten.");
            Log.Write($"settings: dictation_history_forget({id ?? "null"}) ok");
        }
        catch (Exception e) { say($"Not forgotten ({Describe(e)})."); }
        await LoadHistory(list, current, say);
    }

    private NAudio.Wave.WasapiOut? historyOutput;

    private async Task PlayHistory(string id, Action<string> say)
    {
        try
        {
            var bytes = Convert.FromBase64String(await store.Client.DictationAudioGetAsync(id, null));
            historyOutput?.Dispose();
            var buffer = new NAudio.Wave.BufferedWaveProvider(new NAudio.Wave.WaveFormat(16_000, 16, 1), TimeSpan.FromSeconds(Math.Max(5, bytes.Length / 32_000.0 + 1))) { DiscardOnBufferOverflow = true };
            buffer.AddSamples(bytes, 0, bytes.Length);
            using var en = new NAudio.CoreAudioApi.MMDeviceEnumerator();
            historyOutput = new NAudio.Wave.WasapiOut(en.GetDefaultAudioEndpoint(NAudio.CoreAudioApi.DataFlow.Render, NAudio.CoreAudioApi.Role.Multimedia), NAudio.CoreAudioApi.AudioClientShareMode.Shared, true, 60);
            historyOutput.Init(buffer);
            historyOutput.Play();
            say($"Playing {bytes.Length / 32_000.0:0.0} s.");
        }
        catch (Exception e) { say($"No audio ({Describe(e)})."); }
    }

    // MARK: building blocks of this page

    private static void SubHeading(StackPanel page, string text) =>
        page.Children.Add(new TextBlock { Text = text, FontSize = 14, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Margin = new Thickness(0, 10, 0, 0) });

    private static TextBox Field(StackPanel page, string label, string id, string text)
    {
        var box = new TextBox { Header = label, Text = text, MinWidth = 320, HorizontalAlignment = HorizontalAlignment.Left };
        AutomationProperties.SetAutomationId(box, id);
        AutomationProperties.SetName(box, label);
        page.Children.Add(box);
        return box;
    }

    private static TextBlock Muted(string text) =>
        new() { Text = text, FontSize = 12, TextWrapping = TextWrapping.Wrap, Foreground = Theme.Brush(NativeLook.TextTertiary, ElementTheme.Default) };

    /// <summary>A list row: the words (wrapping), a meta line under them, and one trailing action button.</summary>
    private static Grid Row(string words, string meta, string action, string id, Action act)
    {
        var grid = new Grid { ColumnSpacing = 8, Padding = new Thickness(0, 2, 0, 2) };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var text = new StackPanel();
        text.Children.Add(new TextBlock { Text = words, TextWrapping = TextWrapping.Wrap, MaxLines = 3, TextTrimming = TextTrimming.CharacterEllipsis });
        text.Children.Add(new TextBlock { Text = meta, FontSize = 12, Foreground = Theme.Brush(NativeLook.TextSecondary, ElementTheme.Default), TextTrimming = TextTrimming.CharacterEllipsis });
        grid.Children.Add(text);
        var button = new Button { Content = action, VerticalAlignment = VerticalAlignment.Center };
        AutomationProperties.SetAutomationId(button, $"{id}-{action.ToLowerInvariant()}");
        AutomationProperties.SetName(button, $"{action} {words}");
        button.Click += (_, _) => act();
        Grid.SetColumn(button, 2);
        grid.Children.Add(button);
        AutomationProperties.SetAutomationId(grid, id);
        AutomationProperties.SetName(grid, $"{words} — {meta}");
        return grid;
    }

    private static string Slug(string s) => new string(s.Select(c => char.IsLetterOrDigit(c) ? char.ToLowerInvariant(c) : '-').ToArray()).Trim('-');

    private static string Describe(Exception e) => e is RpcException r ? r.ToString() : e.Message;
}