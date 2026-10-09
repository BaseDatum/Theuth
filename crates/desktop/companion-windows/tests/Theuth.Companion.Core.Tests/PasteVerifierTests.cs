namespace Theuth.Companion.Core.Tests;

/// <summary>153 §13.4 (proof 23): the paste verifier — focus moved → unknown, the user acted → userActed (no card), all unchanged →
/// notLanded — the Windows twin of <c>PasteVerifierTests</c>.</summary>
public sealed class PasteVerifierTests
{
    private static readonly FieldRead Before = new(true, "Hello ", "Hello ", "");
    private const string Text = "world, the deploy is done.";

    private static FieldRead After(string value) => new(true, value, value.Length > PasteVerifier.CaretWindowChars ? value[^PasteVerifier.CaretWindowChars..] : value, "");

    [Fact]
    public void OurWordsLandAtTheFirstRead()
    {
        var reads = new[] { After("Hello world, the deploy is done.") };
        Assert.Equal(PasteVerdict.Landed, PasteVerifier.Step(Before, reads, Text, null, VerifyMode.Lease));
    }

    [Fact]
    public void AllThreeUnchangedIsNotLanded()
    {
        var same = new FieldRead(true, "Hello ", "Hello ", "");
        Assert.Null(PasteVerifier.Step(Before, new[] { same }, Text, null, VerifyMode.Lease));
        Assert.Null(PasteVerifier.Step(Before, new[] { same, same }, Text, null, VerifyMode.Lease));
        Assert.Equal(PasteVerdict.NotLanded, PasteVerifier.Step(Before, new[] { same, same, same }, Text, null, VerifyMode.Lease));
        Assert.Equal(PasteVerdict.NotLanded, PasteVerifier.Verdict(Before, new[] { same, same, same }, Text, null, VerifyMode.Lease));
        Assert.Equal(new[] { 150, 350, 1000 }, PasteVerifier.LeaseReadsMs);
    }

    [Fact]
    public void FocusMovedIsUnknown()
    {
        Assert.Equal(PasteVerdict.Unknown, PasteVerifier.Step(Before, new[] { FieldRead.FocusMoved }, Text, null, VerifyMode.Lease));
        var same = new FieldRead(true, "Hello ", "Hello ", "");
        Assert.Equal(PasteVerdict.Unknown, PasteVerifier.Verdict(Before, new[] { same, FieldRead.FocusMoved, same }, Text, null, VerifyMode.Lease));
        Assert.Equal(PasteVerdict.Unknown, PasteVerifier.Step(FieldRead.FocusMoved, Array.Empty<FieldRead>(), Text, null, VerifyMode.Lease));
    }

    [Fact]
    public void AnUnreadableFieldIsUnknown()
    {
        // No TextPattern / ValuePattern value (custom surfaces, secure fields): nothing to verify, no card.
        Assert.Equal(PasteVerdict.Unknown, PasteVerifier.Step(FieldRead.Unreadable, Array.Empty<FieldRead>(), Text, null, VerifyMode.Lease));
        Assert.Equal(PasteVerdict.Unknown, PasteVerifier.Step(Before, new[] { FieldRead.Unreadable }, Text, null, VerifyMode.Lease));
    }

    [Fact]
    public void TheUserTypingMeanwhileIsUserActed()
    {
        var typed = After("Hello there");
        Assert.Null(PasteVerifier.Step(Before, new[] { typed }, Text, null, VerifyMode.Lease)); // may still become ours
        Assert.Equal(PasteVerdict.UserActed, PasteVerifier.Verdict(Before, new[] { typed, typed, typed }, Text, null, VerifyMode.Lease));
    }

    [Fact]
    public void AChangeThatBecomesOursLaterLands()
    {
        var partial = After("Hello wor");
        var full = After("Hello world, the deploy is done.");
        Assert.Equal(PasteVerdict.Landed, PasteVerifier.Verdict(Before, new[] { partial, full }, Text, null, VerifyMode.Lease));
    }

    [Fact]
    public void ALatePasteOfTheRestoredClipboardIsNotLanded()
    {
        // The paste landed after the 500 ms settle: it pasted the user's restored clipboard, not our words (§13.3).
        var late = After("Hello https://example.com/old-copy");
        Assert.Equal(PasteVerdict.NotLanded, PasteVerifier.Step(Before, new[] { late }, Text, "https://example.com/old-copy", VerifyMode.Lease));
    }

    [Fact]
    public void LineEndingsAndAutocorrectedQuotesStillCount()
    {
        var before = new FieldRead(true, "", "", "");
        // A Win32 edit stores CRLF; RichEdit reports CR; autocorrect curls the apostrophe.
        Assert.Equal(PasteVerdict.Landed, PasteVerifier.Verdict(before, new[] { After("line one\r\nit\u2019s line two") }, "line one\nit's line two", null, VerifyMode.Lease));
        Assert.Equal(PasteVerdict.Landed, PasteVerifier.Verdict(before, new[] { After("line one\rline two") }, "line one\nline two", null, VerifyMode.Lease));
        Assert.Equal("a - b ... \"c\"", PasteVerifier.Normalize("a \u2014 b \u2026 \u201Cc\u201D"));
        Assert.Equal("a b\nc", PasteVerifier.Normalize("a\u00A0 b\r\nc"));
    }

    [Fact]
    public void AnInsertionPastTheValueCapIsSeenAtTheCaret()
    {
        // A 100 000-char document: past the 60 000-char cap the value is not read (a cut value would hide the paste); the caret window can.
        var before = new FieldRead(true, null, "end of the document. ", "");
        var after = new FieldRead(true, null, "end of the document. " + Text, "");
        Assert.Equal(PasteVerdict.Landed, PasteVerifier.Verdict(before, new[] { after }, Text, null, VerifyMode.Lease));
    }

    [Fact]
    public void ACaretOnlyMoveIsTheUserActing()
    {
        // H3-D1: the value did not change, only the caret moved → unknown · userActed (never landed, even next to an older copy).
        var before = new FieldRead(true, "x " + Text + " y", "x ", "");
        var after = new FieldRead(true, "x " + Text + " y", "x " + Text, "");
        var reads = new[] { after, after, after };
        Assert.Equal(PasteVerdict.UserActed, PasteVerifier.Verdict(before, reads, Text, null, VerifyMode.Lease));
    }

    [Fact]
    public void LandedNeedsOneMoreOccurrence()
    {
        var before = new FieldRead(true, Text, null, null);
        var same = new FieldRead(true, Text + "!", null, null); // changed, but no extra copy of our words
        Assert.Equal(PasteVerdict.UserActed, PasteVerifier.Verdict(before, new[] { same, same, same }, Text, null, VerifyMode.Lease));
        var more = new FieldRead(true, Text + " " + Text, null, null);
        Assert.Equal(PasteVerdict.Landed, PasteVerifier.Verdict(before, new[] { more }, Text, null, VerifyMode.Lease));
    }

    [Fact]
    public void CountOnlyElementsLandWhenTheCountGrows()
    {
        var before = new FieldRead(true, null, null, null, Count: 70_000);
        Assert.Equal(PasteVerdict.Landed, PasteVerifier.Verdict(before, new[] { new FieldRead(true, null, null, null, Count: 70_000 + Text.Length) }, Text, null, VerifyMode.Lease));
        var still = new FieldRead(true, null, null, null, Count: 70_000);
        Assert.Equal(PasteVerdict.NotLanded, PasteVerifier.Verdict(before, new[] { still, still, still }, Text, null, VerifyMode.Lease));
    }

    [Fact]
    public void ARepeatedPhraseCountsByOccurrence()
    {
        var before = new FieldRead(true, "ok ok ", null, null);
        Assert.Equal(PasteVerdict.Landed, PasteVerifier.Verdict(before, new[] { new FieldRead(true, "ok ok ok", null, null) }, "ok", null, VerifyMode.Lease));
        Assert.Equal(PasteVerdict.NotLanded, PasteVerifier.Verdict(before, new[] { before, before, before }, "ok", null, VerifyMode.Lease));
    }

    [Fact]
    public void ADirectInsertionReadsBackAt150ThenLooksAgainBeforeFallingThrough()
    {
        var same = new FieldRead(true, "Hello ", "Hello ", "");
        Assert.Equal(new[] { 150, 350, 1000 }, PasteVerifier.DirectReadsMs);
        Assert.Equal(PasteVerdict.Landed, PasteVerifier.Step(Before, new[] { After("Hello " + Text) }, Text, null, VerifyMode.Direct)); // the usual case: at 150 ms
        Assert.Null(PasteVerifier.Step(Before, new[] { same }, Text, null, VerifyMode.Direct));
        Assert.Null(PasteVerifier.Step(Before, new[] { same, same }, Text, null, VerifyMode.Direct)); // a slow target: not yet a fall-through
        Assert.Equal(PasteVerdict.NotLanded, PasteVerifier.Step(Before, new[] { same, same, same }, Text, null, VerifyMode.Direct));
        Assert.Equal(PasteVerdict.Landed, PasteVerifier.Verdict(Before, new[] { same, After("Hello " + Text) }, Text, null, VerifyMode.Direct));
    }

    [Fact]
    public void AnAutocompleteSuffixAfterTheCaretStillLands()
    {
        // Browser address bars select a completion after the caret: the text before the caret ends with our words.
        var before = new FieldRead(true, "", "", "");
        var after = new FieldRead(true, "github.com/basedatum", "github.com/base", "datum");
        Assert.Equal(PasteVerdict.Landed, PasteVerifier.Verdict(before, new[] { after }, "github.com/base", null, VerifyMode.Lease));
    }

    [Fact]
    public void TheFooterSaysWhatTheReadsSaw()
    {
        Assert.Equal("field unchanged", PasteVerifier.FieldState(PasteVerdict.NotLanded));
        Assert.Equal("not checked", PasteVerifier.FieldState(PasteVerdict.Unknown));
    }
}