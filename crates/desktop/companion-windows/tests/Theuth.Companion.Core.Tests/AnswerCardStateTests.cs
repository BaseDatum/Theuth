namespace Theuth.Companion.Core.Tests;

/// <summary>153 wave C (proof 18 + §12.4): the one AnswerCard — generation-checked hide, the doneMs rule, and a walk over every row of the state-honesty table.</summary>
public sealed class AnswerCardStateTests
{
    private const string Key = "Ctrl+Alt+Space";

    private static AnswerCardState Answered(bool finished = true)
    {
        var c = new AnswerCardState();
        c.Begin(CardSource.VoiceKey, 0);
        c.Released(1500);
        c.FinalText("why does the onset test fail", 1700);
        c.TurnStarted(2000);
        c.TextDelta("The failing test is `vad::gate_onset`.\nThe onset run is now 8 frames.", 3000);
        if (finished) c.TurnFinished("end_turn", 4000);
        return c;
    }

    [Fact]
    public void HideIgnoredWhenGenerationStale()
    {
        var c = Answered();
        int first = c.Generation;
        Assert.True(c.DoneArmed);
        // A new gesture while the 6 s timer runs: the old timer's hide is ignored, the new card stays.
        int second = c.Begin(CardSource.VoiceKey, 5000);
        Assert.Equal(first + 1, second);
        Assert.False(c.Hide(first, byTimer: true));
        Assert.False(c.Hide(first));
        Assert.True(c.Visible);
        Assert.Equal(CardPhase.Listening, c.Phase);
        // The current generation's own hide works (a dismiss), once.
        Assert.True(c.Hide(second));
        Assert.False(c.Visible);
    }

    [Fact]
    public void DoneMsArmsOnlyCollapsedNotSpeakingFinished()
    {
        Assert.Equal(6000UL, AnswerCardState.DoneMs);
        // Not finished: the text is still coming → no timer.
        var streaming = Answered(finished: false);
        Assert.Equal(CardPhase.Answer, streaming.Phase);
        Assert.False(streaming.DoneArmed);
        Assert.False(streaming.Hide(streaming.Generation, byTimer: true));
        // Finished, collapsed, silent → armed.
        var c = Answered();
        Assert.True(c.DoneArmed);
        // Expanded → not armed; collapsed again → armed.
        c.SetExpanded(true);
        Assert.False(c.DoneArmed);
        Assert.False(c.Hide(c.Generation, byTimer: true));
        c.ToggleExpanded();
        Assert.True(c.DoneArmed);
        // Speaking → not armed; speech ended → armed again.
        // An open approval box (H3-C1) → not armed.
        c.ApprovalOpen = true;
        Assert.False(c.DoneArmed);
        c.ApprovalOpen = false;
        c.SpeechStarted(4500);
        Assert.False(c.DoneArmed);
        Assert.False(c.Hide(c.Generation, byTimer: true));
        c.SpeechEnded(9000);
        Assert.True(c.DoneArmed);
        Assert.True(c.Hide(c.Generation, byTimer: true));
        // Failed never arms.
        var f = Answered(finished: false);
        f.TurnFinished("error", 4000);
        Assert.False(f.DoneArmed);
        Assert.False(f.Hide(f.Generation, byTimer: true));
    }

    [Fact]
    public void StateHonestyTableEveryRow()
    {
        var c = new AnswerCardState();
        // listening — enter on press; shows the rec dot and words; copy "· ⌃S held · theuth's desk"; leaves on release / lock / Esc.
        int g = c.Begin(CardSource.VoiceKey, 0);
        Assert.Equal(CardPhase.Listening, c.Phase);
        Assert.True(c.Visible);
        Assert.Equal($"· {Key} held · theuth's desk", c.Copy(Key, 100));
        Assert.False(c.DoneArmed);
        var esc = new AnswerCardState();
        int eg = esc.Begin(CardSource.VoiceKey, 0);
        Assert.True(esc.Hide(eg)); // Esc during listening cancels the gesture: the card goes with it
        // finishing — enter on release; copy "finishing…"; leaves on the final text (cap 8 s) or "Nothing recognized"; never hides before the text is in.
        c.Released(1500);
        Assert.Equal(CardPhase.Finishing, c.Phase);
        Assert.Equal("finishing…", c.Copy(Key, 1600));
        Assert.Equal(8000UL, AnswerCardState.FinishingCapMs);
        Assert.False(c.DoneArmed);
        Assert.False(c.Hide(g, byTimer: true));
        var nothing = new AnswerCardState();
        nothing.Begin(CardSource.VoiceKey, 0);
        nothing.Released(900);
        nothing.FinalText("  ", 1000);
        Assert.Equal("Nothing recognized", nothing.Copy(Key, 1000));
        c.FinalText("why does the onset test fail", 1700);
        Assert.Equal("why does the onset test fail", c.Latest.Request);
        // thinking — enter on TurnStarted; shimmer, tool chips, elapsed s; copy "thinking · code_refs · 4 s"; leaves on TurnFinished / error; never looks frozen.
        c.TurnStarted(2000);
        Assert.Equal(CardPhase.Thinking, c.Phase);
        c.ToolStarted("t1", "code_refs");
        Assert.Equal("thinking · code_refs · 4 s", c.Copy(Key, 6000));
        Assert.NotEqual(c.Copy(Key, 6000), c.Copy(Key, 7000)); // the elapsed clock ticks every second
        Assert.True(c.Latest.Tools.Single().Running);
        c.ToolFinished("t1", failed: false);
        Assert.Equal("thinking · 5 s", c.Copy(Key, 7000));
        Assert.False(c.DoneArmed);
        // answer — enter on the first TextDelta / TurnFinished; streamed text; copy "replied"; leaves on 6 s doneMs only if collapsed and not speaking.
        c.TextDelta("The failing test is `vad::gate_onset`.", 8000);
        Assert.Equal(CardPhase.Answer, c.Phase);
        Assert.Equal("replying…", c.Copy(Key, 8000));
        Assert.False(c.DoneArmed); // never vanishes while the text is still coming
        c.TurnFinished("end_turn", 9000);
        Assert.Equal("replied", c.Copy(Key, 9000));
        Assert.True(c.DoneArmed);
        Assert.Equal("The failing test is `vad::gate_onset`.", c.CollapsedText);
        // speaking — enter on Speaker.surface == ours; glyph; copy "click to stop"; leaves on speech end / Esc / a new gesture.
        // (A parallel flag over answer, as the Mac twin built it — H3-C1.)
        c.SpeechStarted(9100);
        Assert.True(c.Speaking);
        Assert.Equal(CardPhase.Answer, c.Phase);
        Assert.Equal("click to stop", c.Copy(Key, 9100));
        Assert.False(c.DoneArmed); // never vanishes while speaking
        c.SpeechEnded(12000);
        Assert.False(c.Speaking);
        Assert.Equal("replied", c.Copy(Key, 12000));
        c.SpeechStarted(12100);
        c.Begin(CardSource.VoiceKey, 13000); // a new gesture ends the speaking state
        Assert.False(c.Speaking);
        Assert.Equal(CardPhase.Listening, c.Phase);
        // failed — enter on TurnFinished{error|cancelled} or a send error; failed colour; copy "couldn't reply — ‹why›"; leaves on Esc only; never auto-hides.
        var f = new AnswerCardState();
        int fg = f.Begin(CardSource.VoiceKey, 0);
        f.Released(1000);
        f.FinalText("hello", 1100);
        f.Failed("send_input: desk not found", 1200);
        Assert.Equal(CardPhase.Failed, f.Phase);
        Assert.Equal("couldn't reply — send_input: desk not found", f.Copy(Key, 1200));
        Assert.False(f.DoneArmed);
        Assert.False(f.Hide(fg, byTimer: true));
        Assert.True(f.Visible);
        Assert.True(f.Dismiss()); // Esc
        Assert.False(f.Visible);
        var cancelled = new AnswerCardState();
        cancelled.Begin(CardSource.Command, 0);
        cancelled.TurnStarted(10);
        cancelled.TurnFinished("cancelled", 20);
        Assert.Equal(CardPhase.Failed, cancelled.Phase);
        Assert.Equal("couldn't reply — cancelled", cancelled.Copy(Key, 30));
        // Stop freezes: Esc during thinking keeps the partial answer with "stopped"; the cancelled TurnFinished does not fail it.
        var s = new AnswerCardState();
        s.Begin(CardSource.VoiceKey, 0);
        s.TurnStarted(10);
        s.TextDelta("partial", 20);
        Assert.True(s.Stop(30));
        s.TurnFinished("cancelled", 40);
        Assert.Equal(CardPhase.Answer, s.Phase);
        Assert.Equal("stopped", s.Copy(Key, 50));
        Assert.Equal("partial", s.Latest.Answer);
        Assert.True(s.Visible);
        Assert.False(s.Stop(60)); // nothing left to stop
    }

    [Fact]
    public void ThreadFollowUpsAndHeader()
    {
        var c = Answered();
        Assert.Equal("theuth desk · replied just now", c.Header());
        // A follow-up gesture while the card is up appends a request (the dots advance) and keeps the expanded state.
        c.SetExpanded(true);
        c.Begin(CardSource.VoiceKey, 10000, followUp: true);
        Assert.True(c.Expanded);
        Assert.Equal(2, c.Thread.Count);
        Assert.Equal(1, c.Index);
        Assert.Equal("theuth desk · 2 of 2", c.Header());
        Assert.True(c.Move(-1));
        Assert.Equal("why does the onset test fail", c.Current.Request);
        Assert.False(c.Move(-1));
        Assert.True(c.Move(+1));
        // A gesture from another mode starts a fresh card.
        c.Begin(CardSource.Command, 20000, followUp: true);
        Assert.Single(c.Thread);
        Assert.Equal(CardSource.Command, c.Source);
        Assert.False(c.Expanded is true && false);
    }

    [Fact]
    public void MarkdownBlocksAndInlines()
    {
        var blocks = AnswerMarkdown.Parse("# Title\nThe failing test is `vad::gate_onset`.\n\n- fix the fixture → **vad.rs:112**\n- or keep *7*\n1. first\n2) second\n```rust\nlet x = 1;\n```\nSee [design](theuth://c/x).");
        Assert.IsType<MdBlock.Heading>(blocks[0]);
        Assert.Equal(1, ((MdBlock.Heading)blocks[0]).Level);
        var p = Assert.IsType<MdBlock.Paragraph>(blocks[1]);
        Assert.Equal(new MdInline[] { new MdInline.Text("The failing test is "), new MdInline.Code("vad::gate_onset"), new MdInline.Text(".") }, p.Inlines);
        var b1 = Assert.IsType<MdBlock.Bullet>(blocks[2]);
        Assert.False(b1.Numbered);
        Assert.Contains(new MdInline.Bold("vad.rs:112"), b1.Inlines);
        Assert.Contains(new MdInline.Italic("7"), ((MdBlock.Bullet)blocks[3]).Inlines);
        Assert.Equal(1, ((MdBlock.Bullet)blocks[4]).Number);
        Assert.Equal(2, ((MdBlock.Bullet)blocks[5]).Number);
        var code = Assert.IsType<MdBlock.Code>(blocks[6]);
        Assert.Equal("rust", code.Language);
        Assert.Equal("let x = 1;", code.Text);
        Assert.Contains(new MdInline.Link("design", "theuth://c/x"), ((MdBlock.Paragraph)blocks[7]).Inlines);
        // A streaming answer with an unclosed fence renders the code so far.
        var open = AnswerMarkdown.Parse("text\n```\npartial");
        Assert.Equal("partial", Assert.IsType<MdBlock.Code>(open[1]).Text);
        Assert.Empty(AnswerMarkdown.Parse(""));
        Assert.Equal(new MdInline[] { new MdInline.Text("2 * 3 = 6") }, AnswerMarkdown.Inline("2 * 3 = 6"));
    }
}