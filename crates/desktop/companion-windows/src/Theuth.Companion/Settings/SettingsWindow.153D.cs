using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Theuth.Companion.AnyApp;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Settings;

/// <summary>
/// 153 W153D — the any-app rows of Settings › Dictation, through W153B's <c>AnyAppSettings(StackPanel)</c> seam; the Mac twin is
/// AppProfilesSettings.swift (H3-CC keys). Companion-local, read live by <see cref="AnyAppPrefs"/> at the next gesture:
/// <c>leaveTextOnClipboard</c>, <c>checkPasteLanded</c>, <c>pauseMediaWhileDictating</c>, <c>directInsertion</c> (Windows only) and the
/// <c>appProfiles</c> JSON blob. The Dictate anywhere switch and its key live on W153C's Keys tab — not repeated here. UIA: <c>settings-anyapp-*</c>.
/// </summary>
internal sealed partial class SettingsWindow
{
    partial void AnyAppSettings(StackPanel page)
    {
        var store = model.Store;
        var prefs = new AnyAppPrefs(store);
        void Write(string key, bool on)
        {
            store.Set(key, on);
            store.Flush();
            Log.Write($"settings: {key} = {(on ? "on" : "off")}");
        }

        SubHeading(page, "Other apps");
        Caption(page, "Hold the Dictate anywhere key (Settings › Keys) to dictate into the app you are in: the words land as each sentence closes, pasted with your clipboard back within half a second. Say “send it” to press Return.");
        Toggle(page, "Leave the dictated text on the clipboard", "settings-anyapp-leave-text", prefs.LeaveTextOnClipboard, on => Write(AnyAppPrefs.LeaveTextOnClipboardKey, on));
        Toggle(page, "Check that the text landed", "settings-anyapp-check-landed", prefs.CheckPasteLanded, on => Write(AnyAppPrefs.CheckPasteLandedKey, on));
        Caption(page, "On (leave): your clipboard keeps the dictated text instead of getting its contents back. The check reads the field at 150, 350 and 1000 ms; a paste that never landed opens a card with the words.");
        Toggle(page, "Pause media while dictating (may un-pause)", "settings-anyapp-pause-media", prefs.PauseMedia, on => Write(AnyAppPrefs.PauseMediaKey, on));
        Caption(page, "One Play/Pause key at the press and again at the end — Windows does not say whether something is playing.");
        Toggle(page, "Type short text directly", "settings-anyapp-direct", prefs.DirectInsertion, on => Write(AnyAppPrefs.DirectInsertionKey, on));
        Caption(page, "Off: every sentence is pasted — typed keystrokes can drop characters in some apps (Notepad). On: up to 60 characters are typed without touching the clipboard.");

        // MARK: apps (§13 / §14.3 appProfiles)
        var bar = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Margin = new Thickness(0, 8, 0, 0) };
        bar.Children.Add(new TextBlock { Text = "Apps", VerticalAlignment = VerticalAlignment.Center, Width = 120 });
        var add = new DropDownButton { Content = "Add app…" };
        AutomationProperties.SetAutomationId(add, "settings-anyapp-add");
        var menu = new MenuFlyout();
        add.Flyout = menu;
        bar.Children.Add(add);
        var addDefault = new Button { Content = "Add a default for other apps" };
        AutomationProperties.SetAutomationId(addDefault, "settings-anyapp-add-default");
        bar.Children.Add(addDefault);
        page.Children.Add(bar);
        var list = new StackPanel { Spacing = 4 };
        AutomationProperties.SetAutomationId(list, "settings-anyapp-apps");
        page.Children.Add(list);
        Caption(page, "terminal: pasted with shell spacing, never typed · polish: a small model tidies each sentence before it lands (theuth › Settings › Voice picks the model) · learn: report what you changed afterwards.");

        void Save(IReadOnlyList<AppProfile> rows)
        {
            store.Set(AnyAppPrefs.AppProfilesKey, AppProfileList.Serialize(rows));
            store.Flush();
            Log.Write($"settings: appProfiles = {rows.Count} row(s)");
            Render();
        }

        void Render()
        {
            list.Children.Clear();
            var rows = prefs.Profiles;
            foreach (var p in rows)
            {
                string id = p.Id;
                var row = new Grid { ColumnSpacing = 10 };
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                for (int k = 0; k < 4; k++) row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                var name = new StackPanel { VerticalAlignment = VerticalAlignment.Center };
                name.Children.Add(new TextBlock { Text = AppProfileList.Label(p), TextTrimming = TextTrimming.CharacterEllipsis });
                if (AppProfileList.Removable(p)) name.Children.Add(new TextBlock { Text = id, FontSize = 11, Opacity = 0.6, FontFamily = new Microsoft.UI.Xaml.Media.FontFamily("Cascadia Mono, Consolas") });
                row.Children.Add(name);
                CheckBox Box(string label, bool value, int col, Func<AppProfile, bool, AppProfile> set)
                {
                    var box = new CheckBox { Content = label, IsChecked = value, MinWidth = 0 };
                    AutomationProperties.SetAutomationId(box, $"settings-anyapp-{label}-{id}");
                    box.Click += (_, _) => Save(AppProfileList.Update(prefs.Profiles, id, x => set(x, box.IsChecked == true)));
                    Grid.SetColumn(box, col);
                    row.Children.Add(box);
                    return box;
                }
                Box("terminal", p.Terminal, 1, (x, v) => x with { Terminal = v });
                Box("polish", p.Polish, 2, (x, v) => x with { Polish = v });
                Box("learn", p.Learn, 3, (x, v) => x with { Learn = v });
                var remove = new Button { Content = "Remove", IsEnabled = AppProfileList.Removable(p) };
                AutomationProperties.SetAutomationId(remove, $"settings-anyapp-remove-{id}");
                AutomationProperties.SetName(remove, $"Remove {AppProfileList.Label(p)}");
                remove.Click += (_, _) => Save(AppProfileList.Remove(prefs.Profiles, id));
                Grid.SetColumn(remove, 4);
                row.Children.Add(remove);
                list.Children.Add(row);
            }
            if (rows.Count == 0) list.Children.Add(new TextBlock { Text = "No apps yet: every app is dictated into with polish and learn off.", Opacity = 0.7 });
            addDefault.Visibility = AppProfileList.HasDefault(rows) ? Visibility.Collapsed : Visibility.Visible;
        }

        menu.Opening += (_, _) =>
        {
            menu.Items.Clear();
            foreach (var (exe, title) in AppProfileList.Addable(RunningApps(), prefs.Profiles, "Theuth.Companion.exe"))
            {
                var item = new MenuFlyoutItem { Text = title };
                AutomationProperties.SetAutomationId(item, $"settings-anyapp-add-{exe}");
                string e = exe, t = title;
                item.Click += (_, _) => Save(AppProfileList.Add(prefs.Profiles, e, t));
                menu.Items.Add(item);
            }
            if (menu.Items.Count == 0) menu.Items.Add(new MenuFlyoutItem { Text = "No other app windows are open", IsEnabled = false });
        };
        addDefault.Click += (_, _) => Save(AppProfileList.AddDefault(prefs.Profiles));
        Render();
    }

    /// <summary>Apps with a top-level window, as (exe, name): the file description when readable, else the process name.</summary>
    private static IEnumerable<(string Exe, string Name)> RunningApps()
    {
        foreach (var p in System.Diagnostics.Process.GetProcesses())
        {
            string exe = "", name = "";
            try
            {
                if (p.MainWindowHandle == IntPtr.Zero) continue;
                exe = p.ProcessName + ".exe";
                name = p.ProcessName;
                try { name = p.MainModule?.FileVersionInfo.FileDescription is { Length: > 0 } d ? d : name; }
                catch (Exception) { /* elevated or exited: the process name */ }
            }
            catch (Exception) { continue; }
            finally { p.Dispose(); }
            yield return (exe, name);
        }
    }
}
