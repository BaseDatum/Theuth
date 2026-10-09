namespace Theuth.Companion.Core.Tests;

/// <summary>153 §13.1 (proof 23): every role of the assessment table, the delivery choice (§13.2), the gesture cap and the Return
/// rule (§7.4), the session-lock gate (§13.8), the app profiles' terminal rule and the copy — the Windows twin of
/// <c>DeliveryAssessmentTests</c>.</summary>
public sealed class DeliveryAssessmentTests
{
    private static FocusFacts F(int ct, bool value = false, bool? valueRo = null, bool text = false, bool? textRo = null) => new()
    {
        ControlType = ct, HasValuePattern = value, ValueReadOnly = value ? valueRo : null, HasTextPattern = text, TextReadOnly = text ? textRo : null,
        CanReadBack = value || text, App = "Notepad", Exe = "notepad.exe",
    };

    // MARK: editable (§13.1 row 1: Edit/Document with ValuePattern not read-only, or TextPattern)

    [Fact]
    public void EditWithAWritableValueIsEditableAndTypedDirectly()
    {
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: false));
        Assert.Equal(TargetVerdict.Editable, a.Verdict);
        Assert.False(a.Secure);
        Assert.True(a.CanDirect);
        Assert.Equal("field", a.Role);
        Assert.Equal(DeliveryMethod.Direct, DeliveryAssessment.Method(a, "hello world", fellBack: false));
    }

    [Fact]
    public void DocumentWithATextPatternIsEditable()
    {
        // Win11 Notepad's RichEditD2DPT, Word: Document + TextPattern, the read-only attribute false or unsupported.
        Assert.Equal(TargetVerdict.Editable, DeliveryAssessment.Assess(F(UiaControlType.Document, text: true, textRo: false)).Verdict);
        Assert.Equal(TargetVerdict.Editable, DeliveryAssessment.Assess(F(UiaControlType.Document, text: true, textRo: null)).Verdict);
    }

    [Fact]
    public void EditWithOnlyATextPatternIsEditable() =>
        Assert.Equal(TargetVerdict.Editable, DeliveryAssessment.Assess(F(UiaControlType.Edit, text: true)).Verdict);

    [Fact]
    public void AnEditableComboBoxIsEditable() =>
        Assert.Equal(TargetVerdict.Editable, DeliveryAssessment.Assess(F(UiaControlType.ComboBox, value: true, valueRo: false)).Verdict);

    [Fact]
    public void AWritableValueIsEditableOutsideTheCertainlyNotRows()
    {
        // The macOS "value settable" row: an editable grid cell (DataItem, e.g. a spreadsheet cell) with a settable value takes text…
        var a = DeliveryAssessment.Assess(F(UiaControlType.DataItem, value: true, valueRo: false));
        Assert.Equal(TargetVerdict.Editable, a.Verdict);
        Assert.Equal("cell", a.Role);
        // …but a settable value never un-refuses a certainly-not row (H3-D1: a slider stays refused).
        var s = DeliveryAssessment.Assess(F(UiaControlType.Slider, value: true, valueRo: false));
        Assert.Equal(RefusalReason.NotATextTarget, s.Refusal);
        Assert.Equal("Nothing to type into — slider", AnyAppCopy.Pill(s));
        Assert.Equal(RefusalReason.NotATextTarget, DeliveryAssessment.Assess(F(UiaControlType.CheckBox, value: true, valueRo: false)).Refusal);
    }

    [Fact]
    public void AnEditableTargetWithoutReadBackTakesTheLease()
    {
        // W-m: direct insertion only where the read-back can confirm it.
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: false) with { CanReadBack = false });
        Assert.Equal(TargetVerdict.Editable, a.Verdict);
        Assert.False(a.CanDirect);
        Assert.Equal(DeliveryMethod.Lease, DeliveryAssessment.Method(a, "hi", false));
    }

    // MARK: secure (§13.1: IsPassword → editable, by the lease, never Return)

    [Fact]
    public void APasswordFieldIsEditableSecureAndLeasedNeverReturned()
    {
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: false) with { IsPassword = true, SelectionLength = 4 });
        Assert.Equal(TargetVerdict.Editable, a.Verdict);
        Assert.True(a.Secure);
        Assert.False(a.CanDirect);
        Assert.False(a.Rewrite); // a secure field never offers the rewrite panel
        Assert.Equal(DeliveryMethod.Lease, DeliveryAssessment.Method(a, "hunter2", false));
        Assert.Equal("not sent — secure field", DeliveryAssessment.ReturnWithheld(a, focusSame: true, modifierDown: false));
        Assert.Equal("Dictating into Notepad · secure field", AnyAppCopy.Pill(a));
    }

    [Fact]
    public void AReadOnlyPasswordFieldIsUnknownAndSecure()
    {
        // H3-D1: a text role that is not editable is not certain → unknown; secure stays secure (the lease, never Return).
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: true) with { IsPassword = true });
        Assert.Equal(TargetVerdict.Unknown, a.Verdict);
        Assert.True(a.Secure);
        Assert.Equal(DeliveryMethod.Lease, DeliveryAssessment.Method(a, "hunter2", false));
        Assert.Equal("not sent — secure field", DeliveryAssessment.ReturnWithheld(a, true, false));
    }

        // MARK: unknown (§13.1 row 2: Pane, Custom, Document without patterns, elevated windows; refuse only when certain)

    [Theory]
    [InlineData(UiaControlType.Pane, "pane")]
    [InlineData(UiaControlType.Custom, "element")]
    [InlineData(UiaControlType.Window, "window")]
    [InlineData(UiaControlType.Group, "group")]
    [InlineData(UiaControlType.List, "list")]
    [InlineData(UiaControlType.Tree, "tree")]
    [InlineData(UiaControlType.DataGrid, "table")]
    [InlineData(UiaControlType.Spinner, "spinner")]
    [InlineData(UiaControlType.DataItem, "cell")]
    [InlineData(0, "element")]
    public void UncertainRolesAreUnknownAndLeased(int ct, string role)
    {
        var a = DeliveryAssessment.Assess(F(ct));
        Assert.Equal(TargetVerdict.Unknown, a.Verdict);
        Assert.Equal(role, a.Role);
        Assert.Equal(DeliveryMethod.Lease, DeliveryAssessment.Method(a, "ls -la", false));
        Assert.Equal("Notepad · unknown field — paste", AnyAppCopy.Pill(a));
    }

    [Fact]
    public void ADocumentWithoutPatternsIsUnknown() =>
        Assert.Equal(TargetVerdict.Unknown, DeliveryAssessment.Assess(F(UiaControlType.Document)).Verdict);

    [Fact]
    public void AnEditWithoutPatternsIsUnknown() =>
        Assert.Equal(TargetVerdict.Unknown, DeliveryAssessment.Assess(F(UiaControlType.Edit)).Verdict);

    [Fact]
    public void AnElevatedTargetIsUnknown()
    {
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: false) with { Elevated = true });
        Assert.Equal(TargetVerdict.Unknown, a.Verdict);
        Assert.Equal("elevated window", a.Role);
        Assert.Equal("Notepad · elevated window — paste", AnyAppCopy.Pill(a));
        // UIPI may refuse even reading the element: still unknown, never a refusal.
        Assert.Equal(TargetVerdict.Unknown, DeliveryAssessment.Assess(new FocusFacts { Readable = false, Elevated = true, App = "regedit" }).Verdict);
    }

    [Fact]
    public void NoReadableFocusedElementIsUnknown()
    {
        // GPU terminals, VMs, games: nothing readable → the lease and the verification decide.
        var a = DeliveryAssessment.Assess(new FocusFacts { Readable = false, App = "Terminal" });
        Assert.Equal(TargetVerdict.Unknown, a.Verdict);
        Assert.Equal("Terminal · unknown field — paste", AnyAppCopy.Pill(a));
    }

    [Fact]
    public void TextWithATextPatternIsUnknownNotRefused()
    {
        // Windows Terminal's control reports ControlType.Text with a TextPattern: refusing it would refuse the terminal.
        var a = DeliveryAssessment.Assess(F(UiaControlType.Text, text: true) with { App = "Terminal", Exe = "WindowsTerminal.exe" });
        Assert.Equal(TargetVerdict.Unknown, a.Verdict);
    }

    [Fact]
    public void ATerminalProfileTakesTheLeaseEvenOnAnEditableSurface()
    {
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: false) with { App = "Terminal" }, terminal: true);
        Assert.Equal(TargetVerdict.Unknown, a.Verdict);
        Assert.Equal(DeliveryMethod.Lease, DeliveryAssessment.Method(a, "git status", false));
        Assert.Equal("Terminal · unknown field — paste", AnyAppCopy.Pill(a));
    }

    // MARK: certainly not (§13.1 row 3) — refuse with the hint, nothing recorded

    [Theory]
    [InlineData(UiaControlType.Button, "button")]
    [InlineData(UiaControlType.MenuItem, "menuitem")]
    [InlineData(UiaControlType.Text, "text")]
    [InlineData(UiaControlType.Slider, "slider")]
    [InlineData(UiaControlType.CheckBox, "checkbox")]
    [InlineData(UiaControlType.Image, "image")]
    [InlineData(UiaControlType.ListItem, "listitem")]
    // the Windows forms of the macOS column (AXRadioButton, AXMenu*, AXToolbar, AXTable row) and their plain kin
    [InlineData(UiaControlType.RadioButton, "radio")]
    [InlineData(UiaControlType.Menu, "menu")]
    [InlineData(UiaControlType.MenuBar, "menubar")]
    [InlineData(UiaControlType.ToolBar, "toolbar")]
    [InlineData(UiaControlType.SplitButton, "button")]
    [InlineData(UiaControlType.Hyperlink, "link")]
    [InlineData(UiaControlType.TreeItem, "treeitem")]
    [InlineData(UiaControlType.TabItem, "tab")]
    [InlineData(UiaControlType.ScrollBar, "scrollbar")]
    [InlineData(UiaControlType.ProgressBar, "progress")]
    [InlineData(UiaControlType.TitleBar, "titlebar")]
    public void CertainlyNotRolesAreRefusedWithTheirName(int ct, string role)
    {
        var a = DeliveryAssessment.Assess(F(ct));
        Assert.Equal(TargetVerdict.CertainlyNot, a.Verdict);
        Assert.True(a.Refused);
        Assert.Equal(RefusalReason.NotATextTarget, a.Refusal);
        Assert.Equal(DeliveryMethod.Refuse, DeliveryAssessment.Method(a, "anything", false));
        Assert.Equal($"Nothing to type into — {role}", AnyAppCopy.Pill(a));
        Assert.True(DeliveryAssessment.IsCertainlyNotRole(ct));
    }

    [Fact]
    public void AReadOnlyEditIsUnknownAndLeased()
    {
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: true));
        Assert.Equal(TargetVerdict.Unknown, a.Verdict); // H3-D1: a text role that is not editable → unknown, the verification decides
        Assert.Equal(RefusalReason.None, a.Refusal);
        Assert.Equal(DeliveryMethod.Lease, DeliveryAssessment.Method(a, "words", false));
        Assert.Equal("Notepad · unknown field — paste", AnyAppCopy.Pill(a));
    }

    [Fact]
    public void AReadOnlyDocumentIsUnknown()
    {
        // A browser page with no field focused: Document, ValuePattern read-only (the URL), text read-only — not certain, so unknown.
        var a = DeliveryAssessment.Assess(F(UiaControlType.Document, value: true, valueRo: true, text: true, textRo: true));
        Assert.Equal(TargetVerdict.Unknown, a.Verdict);
        Assert.False(a.CanDirect);
    }

    [Fact]
    public void ATerminalDowngradesEditableToUnknownAndNeverUnRefuses()
    {
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: false, text: true), terminal: true);
        Assert.Equal(TargetVerdict.Unknown, a.Verdict);
        Assert.False(a.CanDirect);
        Assert.Equal(DeliveryMethod.Lease, DeliveryAssessment.Method(a, "ls", false));
        Assert.Equal(RefusalReason.NotATextTarget, DeliveryAssessment.Assess(F(UiaControlType.Button), terminal: true).Refusal);
    }

    [Fact]
    public void AWritableSignalWinsOverAReadOnlyOne()
    {
        // Chromium's root Document reports the URL read-only through ValuePattern but its design-mode text is writable.
        var a = DeliveryAssessment.Assess(F(UiaControlType.Document, value: true, valueRo: true, text: true, textRo: false));
        Assert.Equal(TargetVerdict.Editable, a.Verdict);
    }

    [Fact]
    public void ADisabledTextFieldIsUnknownButADisabledButtonIsRefused()
    {
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: false) with { IsEnabled = false });
        Assert.Equal(TargetVerdict.Unknown, a.Verdict); // H3-D1: refuse only when certain — the lease and the verification decide
        Assert.Equal("Notepad · unknown field — paste", AnyAppCopy.Pill(a));
        var b = DeliveryAssessment.Assess(F(UiaControlType.Button) with { IsEnabled = false });
        Assert.Equal(RefusalReason.NotATextTarget, b.Refusal);
        Assert.Equal(TargetVerdict.Unknown, DeliveryAssessment.Assess(F(UiaControlType.Pane) with { IsEnabled = false }).Verdict);
    }

    [Fact]
    public void OurOwnWindowsAreRefusedWithTheComposerHint()
    {
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: false) with { Ours = true });
        Assert.Equal(RefusalReason.OwnWindow, a.Refusal);
        Assert.Equal("Use Ctrl+D here", AnyAppCopy.Pill(a));
    }

    // MARK: lock refusal (§13.8, §13.9 — never tested by locking build-B: the decision is tested here)

    [Fact]
    public void ALockedSessionRefusesEveryTarget()
    {
        var gate = new SessionLockGate();
        Assert.True(gate.MayDeliver);
        Assert.True(gate.OnSessionChange(SessionLockGate.WtsSessionLock));
        Assert.True(gate.Locked);
        Assert.False(gate.MayDeliver);
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: false), locked: gate.Locked);
        Assert.Equal(RefusalReason.Locked, a.Refusal);
        Assert.Equal(DeliveryMethod.Refuse, DeliveryAssessment.Method(a, "hi", false));
        Assert.Equal("Screen locked — nothing typed", AnyAppCopy.Pill(a));
        Assert.False(gate.OnSessionChange(SessionLockGate.WtsSessionLock)); // repeated lock: no change
        Assert.True(gate.OnSessionChange(SessionLockGate.WtsSessionUnlock));
        Assert.True(gate.MayDeliver);
        Assert.False(gate.OnSessionChange(0x5)); // WTS_SESSION_LOGON and friends do not move the gate
        Assert.False(new SessionLockGate(lockedAtStart: true).MayDeliver);
    }

    // MARK: rewrite entry (§13.6)

    [Fact]
    public void ASelectionInAnEditableFieldOffersRewrite()
    {
        var a = DeliveryAssessment.Assess(F(UiaControlType.Document, text: true, textRo: false) with { SelectionLength = 312 });
        Assert.True(a.Rewrite);
        Assert.Equal("Rewrite in Notepad", AnyAppCopy.Pill(a));
        Assert.Equal("Rewrite · 312 chars selected · say what to change", AnyAppCopy.RewritePill(312));
        Assert.False(DeliveryAssessment.Assess(F(UiaControlType.Document, text: true) with { SelectionLength = 0 }).Rewrite);
        Assert.False(DeliveryAssessment.Assess(F(UiaControlType.Pane) with { SelectionLength = 10 }).Rewrite);
    }

    // MARK: delivery choice (§13.2)

    [Fact]
    public void DirectIsAtMost200SingleLineCharacters()
    {
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: false));
        Assert.Equal(DeliveryMethod.Direct, DeliveryAssessment.Method(a, new string('a', DeliveryAssessment.DirectMaxChars), false));
        Assert.Equal(DeliveryMethod.Lease, DeliveryAssessment.Method(a, new string('a', DeliveryAssessment.DirectMaxChars + 1), false));
        Assert.Equal(DeliveryMethod.Lease, DeliveryAssessment.Method(a, "first line\nsecond line", false)); // Enter would send in a chat app
        Assert.Equal(DeliveryMethod.Lease, DeliveryAssessment.Method(a, "a\tb", false)); // Tab would move focus
        Assert.Equal(DeliveryMethod.Lease, DeliveryAssessment.Method(a, "", false));
        Assert.Equal(DeliveryMethod.Direct, DeliveryAssessment.Method(a, "naïve café — 😀", false));
    }

    [Fact]
    public void HeldKeysTakeTheLease()
    {
        // During a hold of Ctrl+Alt+D paced typing would interleave with the chord's auto-repeat: one atomic Ctrl+V instead.
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: false));
        Assert.Equal(DeliveryMethod.Lease, DeliveryAssessment.Method(a, "short", fellBack: false, keysHeld: true));
        Assert.Equal(DeliveryMethod.Direct, DeliveryAssessment.Method(a, "short", fellBack: false, keysHeld: false));
    }

    [Fact]
    public void AFailedReadBackSendsThisAndEveryLaterUtteranceToTheLease()
    {
        var a = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: false));
        Assert.Equal(DeliveryMethod.Lease, DeliveryAssessment.Method(a, "short", fellBack: true));
    }

    [Fact]
    public void TheGestureCapIs20000Characters()
    {
        Assert.True(DeliveryAssessment.FitsGesture(19_990, "0123456789"));
        Assert.False(DeliveryAssessment.FitsGesture(19_991, "0123456789"));
    }

    // MARK: "send it" (§7.4)

    [Fact]
    public void ReturnOnlyIntoAnEditableUnchangedFocusWithNoModifierDown()
    {
        var editable = DeliveryAssessment.Assess(F(UiaControlType.Edit, value: true, valueRo: false));
        Assert.Null(DeliveryAssessment.ReturnWithheld(editable, focusSame: true, modifierDown: false));
        Assert.Equal("not sent — focus moved", DeliveryAssessment.ReturnWithheld(editable, focusSame: false, modifierDown: false));
        Assert.Equal("not sent — a key is held", DeliveryAssessment.ReturnWithheld(editable, focusSame: true, modifierDown: true));
        var unknown = DeliveryAssessment.Assess(F(UiaControlType.Pane));
        Assert.Null(DeliveryAssessment.ReturnWithheld(unknown, focusSame: true, modifierDown: false)); // H3-D2: unknown targets take Return too
        Assert.Equal(AnyAppCopy.ScratchHint, "Nothing of ours to scratch");
    }

    // MARK: app profiles (AppProfile.id = the exe name on Windows)

    [Fact]
    public void ProfilesMatchTheExeWithOrWithoutTheExtension()
    {
        Assert.Equal("windowsterminal", AppMatch.Normalize(@"C:\Program Files\WindowsApps\WindowsTerminal.exe"));
        Assert.True(AppMatch.Same("WindowsTerminal.exe", "windowsterminal"));
        Assert.True(AppMatch.Same("notepad", "NOTEPAD.EXE"));
        Assert.False(AppMatch.Same("", "notepad.exe"));
        Assert.True(AppMatch.IsTerminal("WindowsTerminal.exe", profileTerminal: null));
        Assert.True(AppMatch.IsTerminal("pwsh.exe", null));
        Assert.False(AppMatch.IsTerminal("notepad.exe", null));
        Assert.True(AppMatch.IsTerminal("WindowsTerminal.exe", profileTerminal: false)); // terminal ||= heuristics (H3-D1)
        Assert.True(AppMatch.IsTerminal("myshell.exe", profileTerminal: true));
        Assert.True(AppMatch.IsTerminal("Code.exe", null)); // Monaco-style editors take the lease too
        Assert.True(AppMatch.IsTerminal("Cursor.exe", null));
        Assert.True(AppMatch.IsTerminal("Zed.exe", null));
        Assert.True(AppMatch.IsTerminal("conhost.exe", null));
        Assert.True(AppMatch.IsTerminal("cmd.exe", null));
        Assert.True(AppMatch.IsTerminal("alacritty.exe", null));
        Assert.True(AppMatch.IsTerminal("wezterm-gui.exe", null));
        Assert.Equal("any_app", AppMatch.DefaultRowId);
    }

    // MARK: names (the ax.rs uia_role vocabulary) and copy

    [Fact]
    public void RolesUseTheScreenLineVocabulary()
    {
        Assert.Equal("link", UiaControlType.Role(UiaControlType.Hyperlink, "hyperlink"));
        Assert.Equal("text", UiaControlType.Role(UiaControlType.Text, "text"));
        Assert.Equal("field", UiaControlType.Role(UiaControlType.Edit, "edit"));
        Assert.Equal("button", UiaControlType.Role(UiaControlType.Button, "button"));
        Assert.Equal("window", UiaControlType.Role(UiaControlType.Window, null));
        Assert.Equal("document", UiaControlType.Role(UiaControlType.Document, null));
        Assert.Equal("tabitem", UiaControlType.Role(UiaControlType.Custom, "Tab Item"));
        Assert.Equal("schaltfläche", UiaControlType.Role(UiaControlType.Custom, "Schaltfläche"));
        Assert.Equal("element", UiaControlType.Role(0, null));
        Assert.Equal("element", UiaControlType.Role(0, "   "));
    }

    [Fact]
    public void TheCopyMatchesTheDesignTable()
    {
        var mail = DeliveryAssessment.Assess(F(UiaControlType.Document, text: true) with { App = "Mail" });
        Assert.Equal("Dictating into Mail", AnyAppCopy.Pill(mail));
        Assert.Equal("Ctrl+Alt+D held · shape on", AnyAppCopy.Held("Ctrl+Alt+D", locked: false, shape: true));
        Assert.Equal("✓ 3 lines into Mail", AnyAppCopy.Done(3, "Mail"));
        Assert.Equal("✓ 1 line into Mail", AnyAppCopy.Done(1, "Mail"));
        Assert.Equal("Come back to Mail to continue · Esc", AnyAppCopy.Paused("Mail"));
        Assert.Equal("Couldn't type into Terminal · paste not landed", AnyAppCopy.CardTitle("Terminal", DeliveryFailure.PasteNotLanded));
        Assert.Equal("checked at 150 · 350 · 1000 ms — field unchanged · clipboard restored",
            AnyAppCopy.CardFooter(PasteVerifier.LeaseReadsMs, PasteVerifier.FieldState(PasteVerdict.NotLanded), clipboardRestored: true));
        Assert.Equal("nothing to type into", AnyAppCopy.Kind(DeliveryFailure.NoEditableTarget));
        foreach (var k in Enum.GetValues<DeliveryFailure>()) Assert.NotEqual("failed", AnyAppCopy.Kind(k));
    }

    // MARK: spacing between utterances (the splice rule)

    [Fact]
    public void UtterancesAreJoinedWithALeadSpaceLikeTheSplice()
    {
        Assert.Equal("Hi Dana.", AnyAppSpacing.Lead("Hi Dana.", beforeCaret: "", previous: null));
        Assert.Equal("Hi Dana.", AnyAppSpacing.Lead("Hi Dana.", beforeCaret: null, previous: null)); // unknown field: nothing added
        Assert.Equal(" the deploy is done.", AnyAppSpacing.Lead("the deploy is done.", beforeCaret: "Hi Dana.", previous: null));
        Assert.Equal("the deploy", AnyAppSpacing.Lead("the deploy", beforeCaret: "Hi Dana, ", previous: null));
        Assert.Equal(" Next.", AnyAppSpacing.Lead("Next.", beforeCaret: null, previous: "Hi Dana."));
        Assert.Equal(", then", AnyAppSpacing.Lead(", then", beforeCaret: "Hi", previous: null)); // punctuation attaches
        Assert.Equal("\nline", AnyAppSpacing.Lead("\nline", beforeCaret: "Hi", previous: null)); // "new line" carries its own break
        Assert.Equal("next", AnyAppSpacing.Lead("next", beforeCaret: null, previous: "line one\n"));
        Assert.Equal(" more", AnyAppSpacing.Lead("more", beforeCaret: "/compact", previous: "/compact")); // the space comes with the next words
    }

    [Fact]
    public void TheAnyAppPillSaysTheChordWithoutTheShapeSegment()
    {
        Assert.Equal("Ctrl+Alt+D held", AnyAppCopy.Chord("Ctrl+Alt+D", false));
        Assert.Equal("Ctrl+Alt+D locked", AnyAppCopy.Chord("Ctrl+Alt+D", true));
    }
}
