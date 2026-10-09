using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core.Tests;

/// <summary>Design 149 WW1 (§10): the key table's new rows per scope, the collision policy over the new scopes, the voice key, E6's fallback, the icon lookup.</summary>
public sealed class KeyTable149Tests
{
    [Fact]
    public void WindowsDefaultsPerScope()
    {
        var t = new KeyTable();
        Assert.Equal("Ctrl+Alt+Space", t.ChordFor(KeyAction.VoiceKey)!.Symbol);
        Assert.Equal("Ctrl+Shift+Space", t.ChordFor(KeyAction.OpenPanel)!.Symbol);
        Assert.Equal("Ctrl+Shift+A", t.ChordFor(KeyAction.OpenAgents)!.Symbol);
        Assert.Equal("Alt+`", t.ChordFor(KeyAction.MeetingPtt)!.Symbol);
        Assert.Equal("Alt+Shift+E", t.ChordFor(KeyAction.MeetingEye)!.Symbol);
        Assert.Equal(new[] { "V", "P", "X" }, new[] { KeyAction.TalkReply, KeyAction.PointAttach, KeyAction.StopAgent }.Select(a => t.ChordFor(a)!.Symbol));
        Assert.Equal("P", t.ChordFor(KeyAction.AskPoint)!.Symbol);
        Assert.Equal(new[] { "J", "K", "Enter", "Ctrl+O", "V", "X", "Esc" },
            t.BindingsIn(KeyScope.Agents).Select(b => b.Chord!.Symbol));
        Assert.Equal(new[] { "M", "Ctrl+M", "E", "D", "J", "K", "Enter", "Esc" }, // island fix §N: J/K walk the briefs
            t.BindingsIn(KeyScope.Island).Select(b => b.Chord!.Symbol));
        Assert.Equal(new[] { "Esc" }, t.BindingsIn(KeyScope.PointAndSay).Select(b => b.Chord!.Symbol));
        Assert.True(t.IsUniquePerScope);
        // Every scope has a title for Settings › Keys and the legend.
        Assert.All(Enum.GetValues<KeyScope>(), s => Assert.NotEqual("", s.Title()));
        // The W1–W6 rows are untouched.
        Assert.Equal("S", t.ChordFor(KeyAction.Snooze)!.Symbol);
        Assert.Equal("E", t.ChordFor(KeyAction.Clear)!.Symbol);
        Assert.Equal("R", t.ChordFor(KeyAction.Reply)!.Symbol);
    }

    [Fact]
    public void FixedRowsAndAliases()
    {
        var t = new KeyTable();
        Assert.True(t.Binding(KeyAction.PointCancel).Fixed);
        Assert.True(t.Binding(KeyAction.AgentsEscape).Fixed);
        Assert.True(t.Binding(KeyAction.IslandEscape).Fixed);
        // Space = focus the reply field (alias of R); Ctrl+P points while typing in the Ask bar; ↓/↑ walk the agents rows.
        Assert.Equal(KeyAction.Reply, t.ActionFor(Chord.Space, KeyScope.Panel));
        Assert.Equal(KeyAction.AskPoint, t.ActionFor(new Chord("p", 'P', ChordModifiers.Control), KeyScope.AskBar));
        Assert.Equal(KeyAction.AgentsNext, t.ActionFor(Chord.Down, KeyScope.Agents));
        Assert.Equal(KeyAction.AgentsPrevious, t.ActionFor(Chord.Up, KeyScope.Agents));
        // Island E is dismiss (scopes are independent, Revision 1 #14); panel E stays clear.
        Assert.Equal(KeyAction.IslandDismiss, t.ActionFor(new Chord("e", 'E'), KeyScope.Island));
        Assert.Equal(KeyAction.Clear, t.ActionFor(new Chord("e", 'E'), KeyScope.Panel));
    }

    [Fact]
    public void CollisionsStayPerScope()
    {
        var t = new KeyTable();
        // Same scope, another row → UsedBy with the owner; across scopes → Ok.
        Assert.Equal(Verdict.UsedBy(KeyAction.OpenPanel), t.Check(new Chord(" ", VirtualKeys.Space, ChordModifiers.Control | ChordModifiers.Shift), KeyAction.VoiceKey));
        Assert.Equal(Verdict.UsedBy(KeyAction.TalkReply), t.Check(new Chord("v", 'V'), KeyAction.PointAttach));
        Assert.Equal(VerdictKind.Ok, t.Check(new Chord("v", 'V'), KeyAction.IslandDeeper).Kind);
        // A global chord without a modifier and a Win chord are refused.
        Assert.Equal(Verdict.ReservedByWindows, t.Check(new Chord(" ", VirtualKeys.Space), KeyAction.VoiceKey));
        Assert.Equal(Verdict.ReservedByWindows, t.Check(new Chord(" ", VirtualKeys.Space, ChordModifiers.Win), KeyAction.VoiceKey));
        // "Use here instead" moves the chord and unassigns the old row; the table round-trips through JSON.
        t.Move(new Chord("x", 'X'), KeyAction.PointAttach);
        Assert.Null(t.ChordFor(KeyAction.StopAgent));
        Assert.Equal("X", t.ChordFor(KeyAction.PointAttach)!.Symbol);
        Assert.Equal(t, new KeyTable(t.Json));
        Assert.Contains("\"stopAgent\":null", t.Json);
        Assert.Contains("\"pointAttach\":", t.Json);
    }

    [Fact]
    public void VoiceKeyIsAGlobalRowWithNoIntent()
    {
        Assert.Equal(KeyScope.Anywhere, KeyTable.ScopeOf(KeyAction.VoiceKey));
        Assert.Null(KeyAction.VoiceKey.Intent());
        Assert.Null(KeyAction.MeetingPtt.Intent());
        Assert.Equal("voiceKey", KeyAction.VoiceKey.RawValue());
        Assert.Equal("pointAndSay", KeyScope.PointAndSay.RawValue());
        Assert.Equal(KeyAction.VoiceKey, KeyText.ActionFromRaw("voiceKey"));
        var r = KeyTable.ResolveGlobal(new KeyTable().ChordFor(KeyAction.VoiceKey), new KeyTable().ChordFor(KeyAction.VoiceKey), _ => false);
        Assert.Null(r.Chord);
        Assert.Contains("held by another app", r.Notice);
    }
}

public sealed class AttentionContext149Tests
{
    private static AttentionItem Item(string? session, ItemContext? context = null) =>
        new() { Id = "ask:1", Kind = AttentionKind.Ask, Title = "T", AtMs = 1, Session = session, Context = context };

    [Fact]
    public void EngineContextWins()
    {
        var r = AttentionContext.Resolve(Item("s1", new ItemContext { Request = "ship it?", LastReply = "tests green" }),
            new[] { new SessionSummary { Id = "s1", Title = "other" } }, new[] { new AgentInfo { Id = "s1", Name = "n", Last = "other" } });
        Assert.Equal(("ship it?", "tests green", true), (r.Request, r.LastReply, r.FromEngine));
    }

    [Fact]
    public void InterimJoinBySession()
    {
        var sessions = new[] { new SessionSummary { Id = "s1", Title = "  fix the build\nsecond line " } };
        var activity = new[] { new AgentInfo { Id = "s1", Name = "n", Last = "done: 3 files" }, new AgentInfo { Id = "s2", Name = "m", Last = "x" } };
        var r = AttentionContext.Resolve(Item("s1"), sessions, activity);
        Assert.Equal(("fix the build", "done: 3 files", false), (r.Request, r.LastReply, r.FromEngine));
        Assert.True(AttentionContext.Resolve(Item(null), sessions, activity).IsEmpty);
        Assert.True(AttentionContext.Resolve(Item("nope"), sessions, activity).IsEmpty);
        // An empty engine context falls through to the join.
        var half = AttentionContext.Resolve(Item("s1", new ItemContext()), sessions, activity);
        Assert.False(half.FromEngine);
        Assert.Equal("fix the build", half.Request);
    }

    [Fact]
    public void ClipsAt160OnATextElementBoundary()
    {
        var s = new string('a', 158) + "é" + "👍🏽" + "tail";
        var c = AttentionContext.Clip(s)!;
        Assert.Equal(AttentionContext.Max, new System.Globalization.StringInfo(c).LengthInTextElements);
        Assert.EndsWith("…", c);
        Assert.Null(AttentionContext.Clip("   \n  "));
        Assert.Null(AttentionContext.Clip(null));
    }
}

public sealed class HarnessIcon149Tests
{
    [Theory]
    [InlineData(null, "theuth")]
    [InlineData("", "theuth")]
    [InlineData("omp", "omp")]
    [InlineData("opencode", "opencode")]
    [InlineData("opencode2", "opencode")]
    [InlineData("Claude-Code", "claude")]
    [InlineData("codex", "codex")]
    [InlineData("gemini", "unknown")]
    public void Lookup(string? kind, string family)
    {
        Assert.Equal(family, HarnessIcon.Family(kind));
        Assert.Equal(family + "-24.png", HarnessIcon.For(kind));
        Assert.Equal(family + "-16.png", HarnessIcon.For(kind, 16));
        Assert.Equal(family + "-24.png", HarnessIcon.For(kind, 17));
    }

    /// <summary>Every family × size the lookup can name exists in the shared asset set the csproj links (149 C).</summary>
    [Fact]
    public void EveryNamedFileShips()
    {
        var dir = FindAssets();
        Assert.NotNull(dir);
        foreach (var family in new[] { HarnessIcon.Omp, HarnessIcon.OpenCode, HarnessIcon.Claude, HarnessIcon.Codex, HarnessIcon.Theuth, HarnessIcon.Unknown })
        {
            foreach (var size in HarnessIcon.Sizes) Assert.True(File.Exists(Path.Combine(dir!, HarnessIcon.For(family, size))), HarnessIcon.For(family, size));
        }
    }

    private static string? FindAssets()
    {
        // tests/…/bin/Debug/net10.0 → the repo's crates/desktop/companion-assets/harness
        var d = new DirectoryInfo(AppContext.BaseDirectory);
        while (d is not null)
        {
            var candidate = Path.Combine(d.FullName, "crates", "desktop", "companion-assets", "harness");
            if (Directory.Exists(candidate)) return candidate;
            d = d.Parent;
        }
        return null;
    }
}
