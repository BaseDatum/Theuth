using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core.Tests;

/// <summary>153 W153D — Settings › Dictation's Apps list (the Mac's AppProfilesSettings rules).</summary>
public sealed class AppProfileListTests
{
    [Fact]
    public void AddDedupesByExeAndTakesTheTerminalHeuristic()
    {
        var rows = AppProfileList.Add(Array.Empty<AppProfile>(), @"C:\Windows\notepad.exe", "Notepad");
        rows = AppProfileList.Add(rows, "NOTEPAD", "Notepad again");
        rows = AppProfileList.Add(rows, "WindowsTerminal.exe", "Terminal");
        Assert.Equal(new[] { "notepad.exe", "WindowsTerminal.exe" }, rows.Select(p => p.Id));
        Assert.False(rows[0].Terminal);
        Assert.True(rows[1].Terminal);
        Assert.False(rows[1].Polish);
        Assert.False(rows[1].Learn);
    }

    [Fact]
    public void TheDefaultRowIsOtherAppsAndCannotBeRemoved()
    {
        var rows = AppProfileList.AddDefault(Array.Empty<AppProfile>());
        rows = AppProfileList.AddDefault(rows);
        Assert.Single(rows);
        Assert.True(AppProfileList.HasDefault(rows));
        Assert.Equal("Other apps", AppProfileList.Label(rows[0]));
        Assert.Equal(AppMatch.DefaultRowId, rows[0].Id);
        Assert.False(AppProfileList.Removable(rows[0]));
        Assert.Single(AppProfileList.Remove(rows, AppMatch.DefaultRowId));
    }

    [Fact]
    public void RemoveAndUpdateTouchOneRow()
    {
        var rows = AppProfileList.Add(AppProfileList.Add(Array.Empty<AppProfile>(), "Code.exe", "Visual Studio Code"), "slack.exe", "Slack");
        rows = AppProfileList.Update(rows, "slack.exe", p => p with { Polish = true, Learn = true });
        Assert.True(rows[1].Polish && rows[1].Learn);
        Assert.False(rows[0].Polish);
        Assert.True(rows[0].Terminal); // VS Code is a Monaco-style editor: the lease, terminal spacing
        rows = AppProfileList.Remove(rows, "CODE.EXE");
        Assert.Equal(new[] { "slack.exe" }, rows.Select(p => p.Id));
    }

    [Fact]
    public void TheAddMenuOffersRunningAppsNotListedAndNotOurs()
    {
        var listed = AppProfileList.Add(Array.Empty<AppProfile>(), "notepad.exe", "Notepad");
        var running = new[] { ("notepad", "Notepad"), ("msedge", "Microsoft Edge"), ("Theuth.Companion", "theuth"), ("msedge", "Microsoft Edge"), ("explorer", "") };
        var offer = AppProfileList.Addable(running, listed, "Theuth.Companion.exe");
        Assert.Equal(new[] { ("explorer.exe", "explorer"), ("msedge.exe", "Microsoft Edge") }, offer);
    }

    [Fact]
    public void TheBlobRoundTripsAndABadBlobIsNoRows()
    {
        var rows = AppProfileList.AddDefault(AppProfileList.Add(Array.Empty<AppProfile>(), "WindowsTerminal.exe", "Terminal"));
        rows = AppProfileList.Update(rows, AppMatch.DefaultRowId, p => p with { Polish = true });
        var back = AppProfileList.Parse(AppProfileList.Serialize(rows));
        Assert.Equal(rows, back);
        Assert.Empty(AppProfileList.Parse("not json"));
        Assert.Empty(AppProfileList.Parse(null));
    }
}
