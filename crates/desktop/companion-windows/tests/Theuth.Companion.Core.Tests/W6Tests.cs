namespace Theuth.Companion.Core.Tests;

// W6: the key table (item 4), the rail presences (item 3), the live dictation reducer (item 2). Key table cases in the
// Windows form of design 134 D6: Win32 virtual-key codes, ⌘ → Ctrl, ⌥ → Alt, Win chords refused, Windows key caps.

public sealed class KeyTableTests
{
    private static Chord K(string c, ushort code, ChordModifiers m = ChordModifiers.None) => new(c, code, m);

    [Fact]
    public void DefaultsAreUniquePerScopeAndRoundTrip()
    {
        var t = new KeyTable();
        Assert.True(t.IsUniquePerScope);
        Assert.Equal(Enum.GetValues<KeyAction>().Length, t.Bindings.Count); // every action has exactly one row
        Assert.Equal(Enum.GetValues<KeyAction>().Length, t.Bindings.Select(b => b.Action).Distinct().Count());
        Assert.Equal(t, new KeyTable(t.Json));
        Assert.Equal("{}", t.Json);
        // Same chord across scopes is fine: Ctrl+O in the panel and the Ask bar.
        Assert.Equal(t.ChordFor(KeyAction.OpenSession), t.ChordFor(KeyAction.AskOpenSession));
    }

    [Fact]
    public void LookupMatchesCharsForLettersAndCodesForSpecials()
    {
        var t = new KeyTable();
        Assert.Equal(KeyAction.Next, t.ActionFor(K("J", 0, ChordModifiers.Shift), KeyScope.Panel)); // Shift+J is J in the panel
        Assert.Equal(KeyAction.OpenSession, t.ActionFor(K("o", 'O', ChordModifiers.Control), KeyScope.Panel));
        Assert.Equal(KeyAction.Next, t.ActionFor(K("", VirtualKeys.Down), KeyScope.Panel)); // ↓ alias
        Assert.Equal(KeyAction.AnswerRecommended, t.ActionFor(K("\r", VirtualKeys.NumpadEnter), KeyScope.Panel)); // keypad Enter alias
        Assert.Equal(KeyAction.Legend, t.ActionFor(K("?", VirtualKeys.Oem2Slash, ChordModifiers.Shift), KeyScope.Panel)); // ? alias
        Assert.Equal(KeyAction.AskLegend, t.ActionFor(K("?", VirtualKeys.Oem2Slash, ChordModifiers.Shift), KeyScope.AskBar));
        Assert.Null(t.ActionFor(K("j", 'J'), KeyScope.AskBar)); // J means nothing in the Ask bar
        Assert.Equal(KeyAction.OpenPanel, t.ActionFor(K(" ", VirtualKeys.Space, ChordModifiers.Control | ChordModifiers.Shift), KeyScope.Anywhere));
        Assert.Null(t.ActionFor(K("j", 'J', ChordModifiers.Alt), KeyScope.Panel)); // Alt+J is not J
    }

    [Fact]
    public void OverrideWinsThenResetRestores()
    {
        var t = new KeyTable();
        t.Set(K("n", 'N'), KeyAction.Next);
        Assert.Equal("N", t.ChordFor(KeyAction.Next)?.Symbol);
        Assert.Equal(KeyAction.Next, t.ActionFor(new Chord("n"), KeyScope.Panel));
        Assert.Null(t.ActionFor(new Chord("j"), KeyScope.Panel)); // the old chord is free
        Assert.Equal(Intent.Next, KeyGrammar.IntentFor(new KeyPress("n"), table: t));
        Assert.Equal(t, new KeyTable(t.Json)); // overrides persist
        Assert.False(t.Binding(KeyAction.Next).IsDefault);
        t.Reset(KeyAction.Next);
        Assert.Equal(new KeyTable(), t);
        t.Set(K("x", 'X'), KeyAction.Snooze);
        t.Set(null, KeyAction.Clear);
        Assert.Null(t.ChordFor(KeyAction.Clear)); // unassigned persists as null
        Assert.Null(new KeyTable(t.Json).ChordFor(KeyAction.Clear));
        t.ResetAll();
        Assert.Equal(new KeyTable(), t);
        t.Set(K("J", 'J', ChordModifiers.Shift), KeyAction.Previous);
        Assert.Equal(K("j", 'J'), t.Overrides[KeyAction.Previous]); // local chords are stored lower-cased without Shift
    }

    [Fact]
    public void CollisionPolicy()
    {
        var t = new KeyTable();
        var j = K("j", 'J');
        Assert.Equal(Verdict.Unchanged, t.Check(j, KeyAction.Next));
        Assert.Equal(Verdict.UsedBy(KeyAction.Next), t.Check(j, KeyAction.Previous)); // same scope: refused with the owner
        Assert.Equal(Verdict.Ok, t.Check(j, KeyAction.AskOpenSession)); // across scopes: allowed
        t.Move(j, KeyAction.Previous);
        Assert.Equal(j, t.ChordFor(KeyAction.Previous));
        Assert.Null(t.ChordFor(KeyAction.Next)); // "Use here instead": the other row goes unassigned
        Assert.True(t.IsUniquePerScope);
        // Global scope: system chords and bare keys are reserved; F-keys alone are fine.
        var windowMenu = K(" ", VirtualKeys.Space, ChordModifiers.Alt); // Alt+Space, the window menu (the Mac case: ⌘Space)
        Assert.Equal(Verdict.ReservedByWindows, t.Check(windowMenu, KeyAction.OpenAskBar, reserved: new[] { windowMenu }));
        Assert.Equal(Verdict.ReservedByWindows, t.Check(K("a", 'A'), KeyAction.OpenAskBar)); // no modifier
        Assert.Equal(Verdict.ReservedByWindows, t.Check(K("A", 'A', ChordModifiers.Shift), KeyAction.OpenAskBar)); // Shift alone does not count
        Assert.Equal(Verdict.Ok, t.Check(K("", VirtualKeys.F1 + 4), KeyAction.OpenAskBar)); // F5
        Assert.Equal(Verdict.UsedBy(KeyAction.OpenPanel), t.Check(K(" ", VirtualKeys.Space, ChordModifiers.Control | ChordModifiers.Shift), KeyAction.OpenAskBar));
        Assert.Equal(Verdict.Ok, t.Check(K("a", 'A'), KeyAction.Snooze)); // local rows take bare keys
    }

    [Fact]
    public void RegistrationFailureFallsBackToTheDefaultWithANotice()
    {
        var def = K(" ", VirtualKeys.Space, ChordModifiers.Control | ChordModifiers.Shift);
        var saved = K("k", 'K', ChordModifiers.Control | ChordModifiers.Alt);
        Assert.Equal(new GlobalResolution(saved, null), KeyTable.ResolveGlobal(saved, def, _ => true));
        var r = KeyTable.ResolveGlobal(saved, def, c => c != saved);
        Assert.Equal(def, r.Chord);
        Assert.Equal("Ctrl+Alt+K is held by another app — using Ctrl+Shift+Space", r.Notice);
        var none = KeyTable.ResolveGlobal(saved, def, _ => false);
        Assert.Null(none.Chord);
        Assert.Contains("no hotkey", none.Notice);
        Assert.Equal(new GlobalResolution(null, null), KeyTable.ResolveGlobal(null, def, _ => false)); // unassigned registers nothing
    }

    [Fact]
    public void ChordSymbols()
    {
        Assert.Equal("Ctrl+Shift+Space", K(" ", VirtualKeys.Space, ChordModifiers.Control | ChordModifiers.Shift).Symbol);
        Assert.Equal("Ctrl+O", K("o", 'O', ChordModifiers.Control).Symbol);
        Assert.Equal("Enter", K("\r", VirtualKeys.Return).Symbol);
        Assert.Equal("Esc", K("\u001b", VirtualKeys.Escape).Symbol);
        Assert.Equal("Win+F1", K("", VirtualKeys.F1, ChordModifiers.Win).Symbol); // the Mac case: fnF1
        Assert.Equal("Ctrl+Alt+/", K("/", VirtualKeys.Oem2Slash, ChordModifiers.Control | ChordModifiers.Alt).Symbol);
        Assert.Equal("↑", K("", VirtualKeys.Up).Symbol);
    }

    [Fact]
    public void TextFieldsTypeInsteadOfActing()
    {
        Assert.Equal(Intent.None, KeyGrammar.IntentFor(new KeyPress("j"), textFieldFocused: true));
        Assert.Equal(Intent.None, KeyGrammar.IntentFor(new KeyPress("1"), textFieldFocused: true));
        Assert.Equal(Intent.Escape, KeyGrammar.IntentFor(new KeyPress("\u001b", VirtualKeys.Escape), textFieldFocused: true));
        Assert.Equal(Intent.Send, KeyGrammar.IntentFor(new KeyPress("\r", VirtualKeys.Return), scope: KeyScope.AskBar));
        Assert.Equal(Intent.Legend, KeyGrammar.IntentFor(new KeyPress("/", VirtualKeys.Oem2Slash, ctrl: true), scope: KeyScope.AskBar));
        Assert.Equal(Intent.Settings, KeyGrammar.IntentFor(new KeyPress(",", VirtualKeys.OemComma, ctrl: true)));
    }

    // MARK: C# additions — the Windows-only rules of design 134 D6 / §4.6.

    [Fact]
    public void WinKeyChordsBelongToTheOperatingSystemInEveryScope()
    {
        var t = new KeyTable();
        Assert.Equal(Verdict.ReservedByWindows, t.Check(K("d", 'D', ChordModifiers.Win | ChordModifiers.Control), KeyAction.OpenAskBar));
        Assert.Equal(Verdict.ReservedByWindows, t.Check(K("j", 'J', ChordModifiers.Win), KeyAction.Next));
        Assert.Equal(Verdict.ReservedByWindows, t.Check(K("", VirtualKeys.F1 + 4, ChordModifiers.Win), KeyAction.OpenAskBar));
    }

    [Fact]
    public void ModifiersAreWin32ModFlagsAndKeyCapsAreWindowsNames()
    {
        // RegisterHotKey takes MOD_ALT 0x1 · MOD_CONTROL 0x2 · MOD_SHIFT 0x4 · MOD_WIN 0x8 as they are.
        Assert.Equal(0x1, (int)ChordModifiers.Alt);
        Assert.Equal(0x2, (int)ChordModifiers.Control);
        Assert.Equal(0x4, (int)ChordModifiers.Shift);
        Assert.Equal(0x8, (int)ChordModifiers.Win);
        var open = new KeyTable().ChordFor(KeyAction.OpenPanel)!;
        Assert.Equal((ushort)0x20, open.KeyCode); // VK_SPACE
        Assert.Equal(ChordModifiers.Control | ChordModifiers.Shift, open.Modifiers);
        Assert.Equal("Num Enter", Chord.NumpadEnter.Symbol);
        Assert.Equal("Win+Ctrl+Alt+Shift+PgDn", K("", VirtualKeys.PageDown, ChordModifiers.Win | ChordModifiers.Control | ChordModifiers.Alt | ChordModifiers.Shift).Symbol);
        Assert.Equal("key 255", K("", 0xFF).Symbol);
        Assert.Equal("Ctrl+L", new KeyTable().ChordFor(KeyAction.AskBar)?.Symbol);
        Assert.Equal("Ctrl+/", new KeyTable().ChordFor(KeyAction.Legend)?.Symbol);
    }
}

public sealed class RailPresenceTests
{
    [Fact]
    public void Widths()
    {
        Assert.Equal(4, RailCollapse.WidthFor(collapsed: true));
        Assert.Equal(26, RailCollapse.WidthFor(collapsed: false));
    }

    [Fact]
    public void SliverWhenQuietIsW1()
    {
        Assert.Equal(8, RailCollapse.CollapseDelay(RailPresence.SliverWhenQuiet, quiet: true, hovering: false, afterHover: false));
        Assert.Null(RailCollapse.CollapseDelay(RailPresence.SliverWhenQuiet, quiet: false, hovering: false, afterHover: false));
        Assert.Null(RailCollapse.CollapseDelay(RailPresence.SliverWhenQuiet, quiet: true, hovering: true, afterHover: false));
        Assert.True(RailCollapse.ExpandsOnActivity(RailPresence.SliverWhenQuiet));
    }

    [Fact]
    public void ExpandedNeverCollapses()
    {
        Assert.Null(RailCollapse.CollapseDelay(RailPresence.Expanded, quiet: true, hovering: false, afterHover: true));
        Assert.False(RailCollapse.ExpandsOnActivity(RailPresence.Expanded));
    }

    [Fact]
    public void AlwaysSliverIgnoresNeedsYou()
    {
        Assert.Equal(0, RailCollapse.CollapseDelay(RailPresence.AlwaysSliver, quiet: false, hovering: false, afterHover: false)); // needs-you or working: still a sliver
        Assert.Equal(0, RailCollapse.CollapseDelay(RailPresence.AlwaysSliver, quiet: true, hovering: false, afterHover: false));
        Assert.Null(RailCollapse.CollapseDelay(RailPresence.AlwaysSliver, quiet: false, hovering: true, afterHover: false)); // hover expands it
        Assert.Equal(1, RailCollapse.CollapseDelay(RailPresence.AlwaysSliver, quiet: false, hovering: false, afterHover: true)); // pointer left: 1 s linger
        Assert.False(RailCollapse.ExpandsOnActivity(RailPresence.AlwaysSliver)); // a new needs-you item does not expand it
    }
}

public sealed class AskDictationTests
{
    private static AskDictation.Event Text(string id, int u, string text, bool final) => new AskDictation.Event.Text(id, u, text, final);

    [Fact]
    public void StartLiveStopIdle()
    {
        var d = new AskDictation();
        Assert.False(d.IsActive);
        Assert.True(d.Handle(AskDictation.Event.Start.Instance));
        Assert.Equal(AskDictation.DictationPhase.Starting, d.Phase);
        Assert.False(d.Handle(Text("x", 0, "early", false))); // not ours yet
        _ = d.Handle(new AskDictation.Event.Started("x"));
        Assert.Equal(AskDictation.DictationPhase.Live("x"), d.Phase);
        Assert.True(d.Handle(Text("x", 0, "ship", false)));
        Assert.True(d.LiveText == "ship" && d.PartialText == "ship" && d.FinalText == "");
        _ = d.Handle(Text("x", 0, "ship it", true));
        _ = d.Handle(Text("x", 1, "to", false));
        Assert.True(d.FinalText == "ship it" && d.PartialText == "to" && d.LiveText == "ship it to");
        _ = d.Handle(Text("x", 1, "", true));
        Assert.Equal("ship it", d.LiveText); // an empty closed utterance is dropped
        Assert.False(d.Handle(Text("other", 0, "no", true))); // another frontend's dictation
        Assert.False(d.Handle(new AskDictation.Event.Level("x", ValueList.Of(0.1f, 0.5f))));
        Assert.Equal(new[] { 0.1f, 0.5f }, d.Levels);
        _ = d.Handle(AskDictation.Event.Stop.Instance);
        Assert.Equal(AskDictation.DictationPhase.Stopping("x"), d.Phase);
        Assert.True(d.Handle(new AskDictation.Event.Finished("x", null)));
        Assert.True(d.Phase == AskDictation.DictationPhase.Idle && d.Error is null && d.Levels.Count == 0);
    }

    [Fact]
    public void TypedEditsWinOverLatePartials()
    {
        var d = new AskDictation();
        _ = d.Handle(AskDictation.Event.Start.Instance);
        _ = d.Handle(new AskDictation.Event.Started("x"));
        _ = d.Handle(Text("x", 0, "ship", false));
        _ = d.Handle(AskDictation.Event.Stop.Instance);
        _ = d.Handle(AskDictation.Event.Typed.Instance);
        Assert.False(d.Handle(Text("x", 0, "ship it", true))); // the field keeps the user's text
        Assert.False(d.Handle(new AskDictation.Event.Finished("x", null)));
        Assert.Equal("ship it", d.LiveText); // the reducer still tracks it
        Assert.True(d.Handle(AskDictation.Event.Start.Instance)); // a new hold resets
        Assert.Empty(d.Utterances);
    }

    [Fact]
    public void ErrorsSurfaceInTheHintRow()
    {
        var d = new AskDictation();
        _ = d.Handle(AskDictation.Event.Start.Instance);
        _ = d.Handle(new AskDictation.Event.StartFailed("already dictating"));
        Assert.True(d.Error == "dictating elsewhere" && d.Phase == AskDictation.DictationPhase.Idle);
        _ = d.Handle(AskDictation.Event.Start.Instance);
        _ = d.Handle(new AskDictation.Event.StartFailed("voice is disabled"));
        Assert.Equal("voice is disabled", d.Error);
        _ = d.Handle(AskDictation.Event.Start.Instance);
        _ = d.Handle(new AskDictation.Event.Started("y"));
        Assert.Null(d.Error); // a new start clears it
        _ = d.Handle(new AskDictation.Event.Finished("y", "model failed"));
        Assert.True(d.Error == "model failed" && !d.IsActive);
        Assert.False(d.Handle(new AskDictation.Event.Started("z"))); // a stray start event while idle is ignored
    }
}
