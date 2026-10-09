using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core.Tests;

/// <summary>153 wave C (proof 22, test part): the two new anywhere rows, the persisted-table uniqueness, the recorder's gesture-family
/// refusal, modifier-only chords, the AltGr decision, the per-mode settings and the card's fixed keys.</summary>
public sealed class KeyTableTests153
{
    private static Chord Ctrl(string c, char k, ChordModifiers extra = ChordModifiers.None) => new(c, k, ChordModifiers.Control | ChordModifiers.Alt | extra);

    [Fact]
    public void TwoNewAnywhereRowsWithWindowsDefaults()
    {
        var t = new KeyTable();
        Assert.Equal("Ctrl+Alt+D", t.ChordFor(KeyAction.DictateAnywhere)!.Symbol);
        Assert.Equal("Ctrl+Alt+C", t.ChordFor(KeyAction.Command)!.Symbol);
        Assert.Equal(KeyScope.Anywhere, KeyTable.ScopeOf(KeyAction.DictateAnywhere));
        Assert.Equal(KeyScope.Anywhere, KeyTable.ScopeOf(KeyAction.Command));
        Assert.Null(KeyAction.DictateAnywhere.Intent());
        Assert.Null(KeyAction.Command.Intent());
        // The Swift twin's persisted ids (H3-CC: .dictateAnywhere, .command).
        Assert.Equal("dictateAnywhere", KeyAction.DictateAnywhere.RawValue());
        Assert.Equal("command", KeyAction.Command.RawValue());
        Assert.Equal(KeyAction.Command, KeyText.ActionFromRaw("command"));
        // The family and its modes.
        Assert.Equal(new[] { KeyAction.VoiceKey, KeyAction.DictateAnywhere, KeyAction.Command }, KeyTable.ModeKeys);
        Assert.Equal(PointMode.Ask, KeyTable.ModeOf(KeyAction.VoiceKey));
        Assert.Equal(PointMode.Dictate, KeyTable.ModeOf(KeyAction.DictateAnywhere));
        Assert.Equal(PointMode.Command, KeyTable.ModeOf(KeyAction.Command));
        Assert.Null(KeyTable.ModeOf(KeyAction.OpenPanel));
        Assert.All(Enum.GetValues<PointMode>(), m => Assert.Equal(m, KeyTable.ModeOf(KeyTable.ActionOf(m))));
        // The voice key, anywhere's other rows and their order are unchanged; the new rows follow the voice key.
        var anywhere = t.BindingsIn(KeyScope.Anywhere).Select(b => b.Action).ToList();
        Assert.Equal(anywhere.IndexOf(KeyAction.VoiceKey) + 1, anywhere.IndexOf(KeyAction.DictateAnywhere));
        Assert.Equal(anywhere.IndexOf(KeyAction.VoiceKey) + 2, anywhere.IndexOf(KeyAction.Command));
        Assert.Equal("Ctrl+Alt+Space", t.ChordFor(KeyAction.VoiceKey)!.Symbol);
    }

    [Fact]
    public void PersistedTableUnique()
    {
        // The defaults, and a persisted table with overrides on the new rows, stay unique per scope and round-trip.
        var t = new KeyTable();
        Assert.True(t.IsUniquePerScope);
        t.Set(Ctrl("j", 'J'), KeyAction.DictateAnywhere);
        t.Set(null, KeyAction.Command);
        var json = t.Json;
        Assert.Contains("\"dictateAnywhere\":{", json);
        Assert.Contains("\"command\":null", json);
        var back = new KeyTable(json);
        Assert.Equal(t, back);
        Assert.True(back.IsUniquePerScope);
        Assert.Equal("Ctrl+Alt+J", back.ChordFor(KeyAction.DictateAnywhere)!.Symbol);
        Assert.Null(back.ChordFor(KeyAction.Command));
        // A table saved before 153 (no new ids) loads the new rows' defaults.
        var old = new KeyTable("""{"voiceKey":{"chars":"k","keyCode":75,"modifiers":3}}""");
        Assert.Equal("Ctrl+Alt+K", old.ChordFor(KeyAction.VoiceKey)!.Symbol);
        Assert.Equal("Ctrl+Alt+D", old.ChordFor(KeyAction.DictateAnywhere)!.Symbol);
        Assert.True(old.IsUniquePerScope);
        // Every check-respecting edit keeps the table unique: try every family chord on every family row.
        foreach (var row in KeyTable.ModeKeys)
        {
            foreach (var other in KeyTable.ModeKeys)
            {
                var u = new KeyTable();
                var chord = u.ChordFor(other)!;
                var v = u.Check(chord, row);
                if (v.Kind == VerdictKind.Ok) u.Set(chord, row);
                u.Move(chord, row);
                Assert.True(u.IsUniquePerScope);
            }
        }
    }

    [Fact]
    public void RecorderRefusesTheVoiceKeysChordForTheNewRowsAndViceVersa()
    {
        var t = new KeyTable();
        var voice = t.ChordFor(KeyAction.VoiceKey)!;
        Assert.Equal(Verdict.ModeKeyClash(KeyAction.VoiceKey), t.Check(voice, KeyAction.DictateAnywhere));
        Assert.Equal(Verdict.ModeKeyClash(KeyAction.VoiceKey), t.Check(voice, KeyAction.Command));
        Assert.Equal(Verdict.ModeKeyClash(KeyAction.DictateAnywhere), t.Check(Ctrl("d", 'D'), KeyAction.VoiceKey));
        Assert.Equal(Verdict.ModeKeyClash(KeyAction.Command), t.Check(Ctrl("c", 'C'), KeyAction.VoiceKey));
        Assert.Equal(Verdict.ModeKeyClash(KeyAction.Command), t.Check(Ctrl("c", 'C'), KeyAction.DictateAnywhere));
        // Refused means refused: Move does nothing across the family (no "Use here instead").
        t.Move(voice, KeyAction.Command);
        Assert.Equal("Ctrl+Alt+Space", t.ChordFor(KeyAction.VoiceKey)!.Symbol);
        Assert.Equal("Ctrl+Alt+C", t.ChordFor(KeyAction.Command)!.Symbol);
        // Outside the family the 149 policy is unchanged: the panel key is "used by", movable.
        var panel = t.ChordFor(KeyAction.OpenPanel)!;
        Assert.Equal(Verdict.UsedBy(KeyAction.OpenPanel), t.Check(panel, KeyAction.Command));
        Assert.Equal(Verdict.UsedBy(KeyAction.Command), t.Check(Ctrl("c", 'C'), KeyAction.OpenAgents));
        // The usual refusals hold for the new rows: no modifier, a Win chord, a reserved chord.
        Assert.Equal(Verdict.ReservedByWindows, t.Check(new Chord("d", 'D'), KeyAction.DictateAnywhere));
        Assert.Equal(Verdict.ReservedByWindows, t.Check(new Chord("d", 'D', ChordModifiers.Win | ChordModifiers.Control), KeyAction.DictateAnywhere));
        Assert.Equal(Verdict.ReservedByWindows, t.Check(Ctrl("q", 'Q'), KeyAction.Command, new[] { Ctrl("q", 'Q') }));
        Assert.Equal(Verdict.Unchanged, t.Check(Ctrl("d", 'D'), KeyAction.DictateAnywhere));
        Assert.Equal(VerdictKind.Ok, t.Check(Ctrl("x", 'X'), KeyAction.Command).Kind);
    }

    [Fact]
    public void ModifierOnlyChordsNeedTheOptInAndAFamilyRow()
    {
        var t = new KeyTable();
        var rctrl = new Chord("", VirtualKeys.RControl);
        Assert.True(rctrl.IsModifierOnly);
        Assert.Equal("Right Ctrl", rctrl.Symbol);
        Assert.Equal(Verdict.NeedsModifierOnly, t.Check(rctrl, KeyAction.Command));
        Assert.Equal(VerdictKind.Ok, t.Check(rctrl, KeyAction.Command, modifierOnly: true).Kind);
        // Only the family rows; never a Win key (Start).
        Assert.Equal(Verdict.ReservedByWindows, t.Check(rctrl, KeyAction.OpenPanel, modifierOnly: true));
        Assert.Equal(Verdict.ReservedByWindows, t.Check(new Chord("", VirtualKeys.LWin), KeyAction.VoiceKey, modifierOnly: true));
        t.Set(rctrl, KeyAction.Command);
        Assert.Equal(Verdict.ModeKeyClash(KeyAction.Command), t.Check(rctrl, KeyAction.DictateAnywhere, modifierOnly: true));
        Assert.Equal(t, new KeyTable(t.Json));
        Assert.False(AltGrRule.Applies(rctrl));
    }

    [Fact]
    public void AltGrDecisionFromTheLayoutProbe()
    {
        var d = Ctrl("d", 'D');
        var c = Ctrl("c", 'C');
        // US: Ctrl+Alt+D types nothing → registered as it is, no notice.
        var us = AltGrRule.Decide(d, null);
        Assert.Equal(d, us.Register);
        Assert.Null(us.Notice);
        Assert.False(us.Substituted);
        Assert.Equal(d, AltGrRule.Decide(d, "").Register);
        // A control character or white space is not a glyph (AltGr+Space types a space; the voice key stays).
        Assert.False(AltGrRule.Decide(d, "\u0004").Substituted);
        Assert.False(AltGrRule.Decide(d, " ").Substituted);
        // US-International: AltGr+D = ð, AltGr+C = © → Ctrl+Shift+Alt+D / +C and the Keys-tab notice in the design's words.
        var intl = AltGrRule.Decide(d, "ð");
        Assert.True(intl.Substituted);
        Assert.Equal(ChordModifiers.Control | ChordModifiers.Alt | ChordModifiers.Shift, intl.Register.Modifiers);
        Assert.Equal('D', intl.Register.KeyCode);
        Assert.Equal("AltGr on this layout — using Ctrl+Shift+Alt+D", intl.Notice);
        Assert.Equal("AltGr on this layout — using Ctrl+Shift+Alt+C", AltGrRule.Decide(c, "©").Notice);
        Assert.Equal("Ctrl+Shift+Alt+C", AltGrRule.FallbackText(c));
        // Polish ć, Czech &: any visible character.
        Assert.True(AltGrRule.Decide(c, "ć").Substituted);
        Assert.True(AltGrRule.Decide(c, "&").Substituted);
        // Only exact Ctrl+Alt on a typing key is probed.
        var voice = new KeyTable().ChordFor(KeyAction.VoiceKey)!;
        Assert.False(AltGrRule.Applies(voice));
        Assert.False(AltGrRule.Decide(voice, "x").Substituted);
        Assert.False(AltGrRule.Applies(Ctrl("d", 'D', ChordModifiers.Shift)));
        Assert.False(AltGrRule.Applies(new Chord("d", 'D', ChordModifiers.Control)));
        Assert.False(AltGrRule.Applies(new Chord("", VirtualKeys.F1 + 4, ChordModifiers.Control | ChordModifiers.Alt)));
        Assert.True(AltGrRule.Applies(new Chord("7", '7', ChordModifiers.Control | ChordModifiers.Alt)));
        Assert.False(AltGrRule.Applies(null));
    }

    [Fact]
    public void ModeKeySettingsRoundTripAndDefaults()
    {
        var store = new SettingsStore();
        var s = ModeKeySettings.Load(store);
        Assert.True(s.DictateAnywhereEnabled && s.CommandKeyEnabled);
        Assert.False(s.ModifierOnlyKeys);
        Assert.All(Enum.GetValues<PointMode>(), m => Assert.Equal(Gesture.HoldLock, s.GestureFor(m))); // W-i: Automatic is opt-in
        s.SetGesture(PointMode.Command, Gesture.Automatic);
        s.CommandKeyEnabled = false;
        s.ModifierOnlyKeys = true;
        s.Save(store);
        Assert.Equal("automatic", store.String("gesture.command"));
        Assert.Equal("holdLock", store.String("gesture.ask"));
        Assert.Equal("holdLock", store.String("gesture.dictate"));
        Assert.False(store.Bool("commandKeyEnabled"));
        Assert.True(store.Bool("modifierOnlyKeys"));
        var back = ModeKeySettings.Load(store);
        Assert.Equal(s, back);
        Assert.Equal(s, s.Clone());
        Assert.False(back.Enabled(KeyAction.Command));
        Assert.True(back.Enabled(KeyAction.DictateAnywhere));
        Assert.True(back.Enabled(KeyAction.VoiceKey));
        Assert.Equal(Gesture.Automatic, PointModeText.GestureFromRaw("automatic"));
        Assert.Equal(Gesture.HoldLock, PointModeText.GestureFromRaw("bogus"));
    }

    [Fact]
    public void CardKeysApprovalBoxTakesPrecedence()
    {
        var enter = new KeyPress("\r", VirtualKeys.Return, ChordModifiers.None);
        var esc = new KeyPress("\u001b", VirtualKeys.Escape, ChordModifiers.None);
        var o = new KeyPress("o", 'O', ChordModifiers.None);
        // Approval box up: Enter approves, Esc denies — over expand, stop and dismiss.
        Assert.Equal(CardKeyIntent.Approve, CardKeys.IntentFor(enter, new CardKeyContext(ApprovalShown: true, Working: true)));
        Assert.Equal(CardKeyIntent.Deny, CardKeys.IntentFor(esc, new CardKeyContext(ApprovalShown: true, Working: true, Expanded: true)));
        Assert.Equal(CardKeyIntent.Approve, CardKeys.IntentFor(new KeyPress("\r", VirtualKeys.NumpadEnter, ChordModifiers.None), new CardKeyContext(ApprovalShown: true)));
        // Esc: speech first, then stop while working, then collapse, then dismiss.
        Assert.Equal(CardKeyIntent.StopSpeaking, CardKeys.IntentFor(esc, new CardKeyContext(Speaking: true, Expanded: true)));
        Assert.Equal(CardKeyIntent.Stop, CardKeys.IntentFor(esc, new CardKeyContext(Working: true)));
        Assert.Equal(CardKeyIntent.Collapse, CardKeys.IntentFor(esc, new CardKeyContext(Expanded: true)));
        Assert.Equal(CardKeyIntent.Dismiss, CardKeys.IntentFor(esc, new CardKeyContext()));
        // Enter expands/collapses; O opens; J/K and the arrows walk the thread; Ctrl+L lists.
        Assert.Equal(CardKeyIntent.ToggleExpand, CardKeys.IntentFor(enter, new CardKeyContext()));
        Assert.Equal(CardKeyIntent.Open, CardKeys.IntentFor(o, new CardKeyContext()));
        Assert.Equal(CardKeyIntent.Next, CardKeys.IntentFor(new KeyPress("j", 'J', ChordModifiers.None), new CardKeyContext()));
        Assert.Equal(CardKeyIntent.Previous, CardKeys.IntentFor(new KeyPress("k", 'K', ChordModifiers.None), new CardKeyContext()));
        Assert.Equal(CardKeyIntent.Next, CardKeys.IntentFor(new KeyPress("", VirtualKeys.Right, ChordModifiers.None), new CardKeyContext()));
        Assert.Equal(CardKeyIntent.Previous, CardKeys.IntentFor(new KeyPress("", VirtualKeys.Left, ChordModifiers.None), new CardKeyContext()));
        Assert.Equal(CardKeyIntent.List, CardKeys.IntentFor(new KeyPress("l", 'L', ChordModifiers.Control), new CardKeyContext()));
        Assert.Equal(CardKeyIntent.None, CardKeys.IntentFor(new KeyPress("o", 'O', ChordModifiers.Control), new CardKeyContext()));
        // In the follow-up field letters are text: only Enter (send) and Esc act.
        var field = new CardKeyContext(Expanded: true, FollowUpFocused: true);
        Assert.Equal(CardKeyIntent.SendFollowUp, CardKeys.IntentFor(enter, field));
        Assert.Equal(CardKeyIntent.None, CardKeys.IntentFor(o, field));
        Assert.Equal(CardKeyIntent.Collapse, CardKeys.IntentFor(esc, field));
        Assert.Contains("Yes Enter", CardKeys.Footer(new CardKeyContext(ApprovalShown: true), false));
        Assert.Equal("Enter expand · O open · Esc", CardKeys.Footer(new CardKeyContext(), true));
    }
}
