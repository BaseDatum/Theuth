using System.Text;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core.Tests;

/// <summary>153 wave C (proof 17): the command card reducer — rows from ToolCallStarted with and without purpose, the cap at 12, the ✓/✗ parse, approvals, failure, follow-ups.</summary>
public sealed class CommandCardTests
{
    private static (CommandCardState, List<CommandEffect>) Run(CommandCardState s, params CommandInput[] inputs)
    {
        var fx = new List<CommandEffect>();
        foreach (var i in inputs)
        {
            var (next, e) = CommandCard.Observe(s, i);
            s = next;
            fx.AddRange(e);
        }
        return (s, fx);
    }

    private static CommandCardState Working(string request = "move today's screenshots into Desktop/Shots")
    {
        var (s, _) = Run(CommandCardState.Empty, new CommandInput.Listening(0), new CommandInput.Heard(request, 4000), new CommandInput.TurnStarted());
        return s;
    }

    private static CommandInput.ToolStarted Tool(string id, string purpose, string tool = "shell", string? command = null) => new(id, tool, purpose, command);

    [Fact]
    public void RowsFromToolCallStartedWithAndWithoutPurpose()
    {
        var s = Working();
        Assert.Equal(CommandPhase.Working, s.Phase);
        (s, _) = Run(s,
            Tool("c1", "checking: ls ~/Desktop/*.png · 14 files", command: "ls ~/Desktop/*.png"),
            new CommandInput.ToolFinished("c1", false, "14"),
            Tool("c2", "executing: mkdir -p ~/Desktop/Shots && mv …", command: "mkdir -p ~/Desktop/Shots && mv ~/Desktop/*.png ~/Desktop/Shots/"),
            new CommandInput.ToolFinished("c2", false, ""),
            Tool("c3", "verifying: ls ~/Desktop/Shots | wc -l", command: "ls ~/Desktop/Shots | wc -l"),
            new CommandInput.ToolStarted("c4", "applescript", null, null));
        var rows = s.Current!.Steps;
        Assert.Equal(new[] { "checked", "executed", "verifying", "next" }, rows.Select(r => r.Verb));
        Assert.Equal(new[] { StepStatus.Ok, StepStatus.Ok, StepStatus.Running, StepStatus.Next }, rows.Select(r => r.Status));
        Assert.Equal("ls ~/Desktop/*.png · 14 files", rows[0].Text);
        Assert.Equal(StepKind.Executing, rows[1].Kind);
        // No purpose: type "ran", the tool name as the text.
        Assert.Equal(StepKind.Ran, rows[3].Kind);
        Assert.Equal("applescript", rows[3].Text);
        // The running row turns green/red on ToolCallFinished; the next one starts running.
        (s, _) = Run(s, new CommandInput.ToolFinished("c3", true, "ls: no such file"));
        Assert.Equal(new[] { StepStatus.Ok, StepStatus.Ok, StepStatus.Failed, StepStatus.Running }, s.Current!.Steps.Select(r => r.Status));
        Assert.Equal("verified", s.Current.Steps[2].Verb);
        Assert.Equal("running", s.Current.Steps[3].Verb);
        (s, _) = Run(s, new CommandInput.ToolFinished("c4", false, "ok"));
        Assert.Equal("ran", s.Current!.Steps[3].Verb);
        Assert.Equal("4 steps  ls · mkdir/mv · ls/wc · applescript", CommandCard.Summary(s.Current.Steps));
        Assert.Equal(4, s.Current.StepCount);
    }

    [Fact]
    public void ParsePurposePrefixesAndClip()
    {
        Assert.Equal(new ParsedPurpose(StepKind.Checking, "build/ exists"), CommandCard.ParsePurpose("checking: build/ exists", "shell"));
        Assert.Equal(new ParsedPurpose(StepKind.Executing, "rm -rf build"), CommandCard.ParsePurpose("Executing:rm -rf build", "shell"));
        Assert.Equal(new ParsedPurpose(StepKind.Verifying, "gone"), CommandCard.ParsePurpose("verify: gone", "shell"));
        Assert.Equal(new ParsedPurpose(StepKind.Ran, "list the files"), CommandCard.ParsePurpose("list the files", "shell"));
        Assert.Equal(new ParsedPurpose(StepKind.Ran, "read_file"), CommandCard.ParsePurpose(null, "read_file"));
        Assert.Equal(new ParsedPurpose(StepKind.Ran, "read_file"), CommandCard.ParsePurpose("   ", "read_file"));
        // An unknown prefix is not a type: the whole purpose is the text.
        Assert.Equal(StepKind.Ran, CommandCard.ParsePurpose("note: something", "x").Kind);
        // ≤ 60 chars with an ellipsis; one line.
        var p = CommandCard.ParsePurpose("checking: " + new string('a', 100), "shell");
        Assert.Equal(60, p.Text.Length);
        Assert.EndsWith("…", p.Text);
        Assert.Equal("a b", CommandCard.ParsePurpose("checking: a\nb", "shell").Text);
    }

    [Fact]
    public void CapAt12()
    {
        Assert.Equal(12, CommandCard.MaxSteps);
        var s = Working();
        var fx = new List<CommandEffect>();
        for (int i = 1; i <= CommandCard.MaxSteps; i++)
        {
            List<CommandEffect> e;
            (s, e) = Run(s, Tool($"c{i}", $"checking: step {i}"), new CommandInput.ToolFinished($"c{i}", false, ""));
            fx.AddRange(e);
        }
        Assert.Empty(fx);
        Assert.Equal(CommandPhase.Working, s.Phase);
        Assert.Equal(12, s.Current!.StepCount);
        // The 13th start: cancel(desk, "root") once, freeze, "stopped after 12 steps — hold … to continue".
        (s, fx) = Run(s, Tool("c13", "checking: step 13"));
        Assert.Equal(new CommandEffect[] { new CommandEffect.CancelTurn("cap") }, fx);
        Assert.Equal(CommandPhase.Stopped, s.Phase);
        Assert.True(s.Current!.CapStopped);
        Assert.Equal(12, s.Current.Steps.Count);
        Assert.Equal("stopped after 12 steps — hold Ctrl+Alt+C to continue", CommandCard.StatusCopy(s, "Ctrl+Alt+C"));
        // The cancelled turn's TurnFinished keeps it stopped, not failed; later tool events change nothing.
        (s, fx) = Run(s, Tool("c14", "x"), new CommandInput.TurnFinished("cancelled", 60000));
        Assert.Empty(fx);
        Assert.Equal(CommandPhase.Stopped, s.Phase);
        Assert.Equal(12, s.Current!.Steps.Count);
        Assert.True(s.Settled);
        // A hold continues the thread: the next line says "continue".
        Assert.Equal("continue", CommandCard.ContinueText(s, ""));
        Assert.Equal("continue — and the reminder", CommandCard.ContinueText(s, "and the reminder"));
        (s, _) = Run(s, new CommandInput.Listening(61000), new CommandInput.Heard("continue", 62000), new CommandInput.TurnStarted(), Tool("d1", "checking: again"));
        Assert.Equal(2, s.Requests.Count);
        Assert.Equal(1, s.Current!.StepCount);
        Assert.False(s.Current.CapStopped);
        Assert.Equal("fresh", CommandCard.ContinueText(s, "fresh"));
    }

    [Fact]
    public void VerdictParse()
    {
        Assert.Equal(CommandVerdict.Ok, CommandCard.ParseVerdict("✓ 14 screenshots moved to ~/Desktop/Shots; reminder “Review shots” set for 18:00.").Kind);
        Assert.Equal(CommandVerdict.Failed, CommandCard.ParseVerdict("✗ Reminders refused: no permission").Kind);
        Assert.Equal(CommandVerdict.Ok, CommandCard.ParseVerdict("  ✔ done").Kind);
        Assert.Equal(CommandVerdict.Ok, CommandCard.ParseVerdict("✅ done").Kind);
        Assert.Equal(CommandVerdict.Failed, CommandCard.ParseVerdict("❌ no").Kind);
        Assert.Equal(CommandVerdict.Failed, CommandCard.ParseVerdict("✘ no").Kind);
        var replied = CommandCard.ParseVerdict("Sure! I moved them.");
        Assert.Equal(CommandVerdict.Replied, replied.Kind);
        Assert.Equal("Sure! I moved them.", replied.Line);
        Assert.Equal(CommandVerdict.Replied, CommandCard.ParseVerdict("").Kind);
        Assert.Equal(CommandVerdict.Replied, CommandCard.ParseVerdict(null).Kind);
        // Done: the streamed reply is the ✓ line; the header counts the seconds.
        var (s, _) = Run(Working(), Tool("c1", "checking: x"), new CommandInput.ToolFinished("c1", false, ""),
            new CommandInput.TextDelta("✓ moved "), new CommandInput.TextDelta("14 files"), new CommandInput.TurnFinished("end_turn", 10000));
        Assert.Equal(CommandPhase.Done, s.Phase);
        Assert.Equal(CommandVerdict.Ok, s.Current!.Verdict.Kind);
        Assert.Equal("✓ moved 14 files", s.Current.Verdict.Line);
        Assert.Equal("Command · theuth desk · done · 6 s", CommandCard.Header(s));
        Assert.Equal("O open session · Esc dismiss · hold Ctrl+Alt+C follow up", CommandCard.StatusCopy(s, "Ctrl+Alt+C"));
    }

    [Fact]
    public void ApprovalInTheCardAnsweredAndTimedOut()
    {
        var s = Working();
        (s, _) = Run(s, Tool("c1", "checking: build/ exists · 1.2 GB · 3 days old"), new CommandInput.ToolFinished("c1", false, ""),
            Tool("c2", "executing: rm -rf ~/Develop/theuth/build", command: "rm -rf ~/Develop/theuth/build"));
        var (s2, fx) = Run(s, new CommandInput.ApprovalRequested("ap1", "shell", "rm -rf ~/Develop/theuth/build", Risk.Destructive, 20000));
        Assert.Equal(new CommandEffect[] { new CommandEffect.OpenListen("ap1") }, fx);
        Assert.Equal(CommandPhase.NeedsOk, s2.Phase);
        Assert.Equal("Run `rm -rf ~/Develop/theuth/build`?", s2.Approval!.Question);
        Assert.Equal("destructive · policy: ask", s2.Approval.RiskLine);
        Assert.Equal("checked  build/ exists · 1.2 GB · 3 days old", s2.Approval.Context);
        Assert.StartsWith("Run `rm -rf ~/Develop/theuth/build`?  destructive · policy: ask · Yes Enter · No Esc", CommandCard.StatusCopy(s2, "Ctrl+Alt+C"));
        // An unmatched utterance keeps the box with "didn't catch that".
        var (s3, _) = Run(s2, new CommandInput.Unheard());
        Assert.Equal(SpokenApproval.DidntCatch, s3.Approval!.Line);
        // Voice "yes" → resolve_approval(Allow) once (the box closes its listen); a second answer is ignored.
        (s3, fx) = Run(s3, new CommandInput.Answer(Decision.Allow, "voice"), new CommandInput.Answer(Decision.Deny, "esc"));
        Assert.Equal(new CommandEffect[] { new CommandEffect.ResolveApproval("ap1", Decision.Allow, "voice"), new CommandEffect.CloseListen() }, fx);
        (s3, fx) = Run(s3, new CommandInput.ApprovalResolved("ap1", Decision.Allow, false, 22000));
        Assert.Empty(fx);
        Assert.Null(s3.Approval);
        Assert.Equal(CommandPhase.Working, s3.Phase);
        // Timeout: the row says "denied (no answer in N s)", the box goes, the listen closes; the model's ✗ line follows.
        var (t, tfx) = Run(s2, new CommandInput.ApprovalResolved("ap1", Decision.Deny, true, 140000));
        Assert.Equal(new CommandEffect[] { new CommandEffect.CloseListen() }, tfx);
        Assert.Null(t.Approval);
        var denied = t.Current!.Steps.Single(r => r.CallId == "c2");
        Assert.Equal(StepStatus.Denied, denied.Status);
        Assert.Equal("denied (no answer in 120 s)", denied.Note);
        Assert.Equal("denied", denied.Verb);
        (t, _) = Run(t, new CommandInput.ToolFinished("c2", true, "denied"), new CommandInput.TextDelta("✗ not removed — no answer"), new CommandInput.TurnFinished("end_turn", 141000));
        Assert.Equal(StepStatus.Denied, t.Current!.Steps.Single(r => r.CallId == "c2").Status);
        Assert.Equal(CommandVerdict.Failed, t.Current.Verdict.Kind);
        // An ApprovalResolved for another id is not ours.
        var (o, ofx) = Run(s2, new CommandInput.ApprovalResolved("other", Decision.Allow, false, 1));
        Assert.Same(s2, o);
        Assert.Empty(ofx);
    }

    [Fact]
    public void FailureStopAndFollowUp()
    {
        // error/cancelled stop reasons → "✗ couldn't finish — ‹message›", failed, never auto-hides (Settled but Failed).
        var (f, _) = Run(Working(), new CommandInput.Error("model unavailable: no credential"), new CommandInput.TurnFinished("error", 5000));
        Assert.Equal(CommandPhase.Failed, f.Phase);
        Assert.Equal("✗ couldn't finish — model unavailable: no credential", CommandCard.StatusCopy(f, "Ctrl+Alt+C"));
        var (c, _) = Run(Working(), new CommandInput.TurnFinished("cancelled", 5000));
        Assert.Equal("✗ couldn't finish — cancelled", CommandCard.StatusCopy(c, "Ctrl+Alt+C"));
        // Esc while working: cancel once, freeze with rows stopped; the cancelled TurnFinished keeps it stopped.
        var (s, fx) = Run(Working(), Tool("c1", "executing: long thing"), new CommandInput.Stop(3000));
        Assert.Equal(new CommandEffect[] { new CommandEffect.CancelTurn("esc") }, fx);
        Assert.Equal(StepStatus.Cancelled, s.Current!.Steps[0].Status);
        Assert.Equal("stopped", s.Current.Steps[0].Verb);
        (s, _) = Run(s, new CommandInput.TurnFinished("cancelled", 3500));
        Assert.Equal(CommandPhase.Stopped, s.Phase);
        Assert.True(s.Current!.UserStopped);
        // Heard → working copy; a follow-up on a done card appends a block under the ✓ line.
        var (h, _) = Run(CommandCardState.Empty, new CommandInput.Listening(0));
        Assert.Equal("Ctrl+Alt+C held", CommandCard.StatusCopy(h, "Ctrl+Alt+C"));
        (h, _) = Run(h, new CommandInput.Heard("make a reminder", 2000));
        Assert.Equal("heard → working", CommandCard.StatusCopy(h, "Ctrl+Alt+C"));
        (h, _) = Run(h, new CommandInput.TurnStarted());
        Assert.Equal("Esc stop · hold Ctrl+Alt+C to add", CommandCard.StatusCopy(h, "Ctrl+Alt+C"));
        (h, _) = Run(h, new CommandInput.TextDelta("✓ done"), new CommandInput.TurnFinished("end_turn", 4000),
            new CommandInput.Listening(5000), new CommandInput.Heard("and one for tomorrow", 6000), new CommandInput.TurnStarted());
        Assert.Equal(2, h.Requests.Count);
        Assert.Equal(CommandPhase.Done, h.Requests[0].Phase);
        Assert.Equal("✓ done", h.Requests[0].Reply);
        Assert.Equal("and one for tomorrow", h.Current!.Text);
        Assert.Equal(CommandPhase.Working, h.Phase);
    }

    [Fact]
    public void FromEventDecodesTheDeskStreamOnly()
    {
        CommandInput? E(string type, string json) => CommandCard.FromEvent(type, Encoding.UTF8.GetBytes(json), "desk", 777);
        Assert.Equal(new CommandInput.ToolStarted("call_1", "shell", "checking: ls", "ls -la"),
            E("ToolCallStarted", """{"type":"ToolCallStarted","session":"desk","turn":"t","call_id":"call_1","name":"shell","args":{"command":"ls -la","purpose":"checking: ls"}}"""));
        Assert.Equal(new CommandInput.ToolStarted("call_2", "read_file", null, null),
            E("ToolCallStarted", """{"type":"ToolCallStarted","session":"desk","turn":"t","call_id":"call_2","name":"read_file","args":{"path":"x"}}"""));
        Assert.Null(E("ToolCallStarted", """{"type":"ToolCallStarted","session":"other","call_id":"x","name":"shell","args":{}}"""));
        Assert.Equal(new CommandInput.ToolFinished("call_1", true, "boom"),
            E("ToolCallFinished", """{"type":"ToolCallFinished","session":"desk","turn":"t","call_id":"call_1","output":"boom","is_error":true,"files":[]}"""));
        Assert.Equal(new CommandInput.ApprovalRequested("ap", "shell", "rm -rf build", Risk.Destructive, 777),
            E("ApprovalRequested", """{"type":"ApprovalRequested","id":"ap","session":"desk","tool":"shell","args":{"command":"rm -rf build"},"risk":"destructive"}"""));
        Assert.Equal(new CommandInput.ApprovalResolved("ap", Decision.Deny, true, 777),
            E("ApprovalResolved", """{"type":"ApprovalResolved","id":"ap","decision":"deny","timed_out":true}"""));
        Assert.Equal(new CommandInput.TurnFinished("end_turn", 777), E("TurnFinished", """{"type":"TurnFinished","session":"desk","stop_reason":"end_turn"}"""));
        Assert.Equal(new CommandInput.TextDelta("✓ ok"), E("TextDelta", """{"type":"TextDelta","session":"desk","text":"✓ ok"}"""));
        Assert.Equal(new CommandInput.Error("bad"), E("Error", """{"type":"Error","session":"desk","turn":null,"message":"bad"}"""));
        Assert.IsType<CommandInput.TurnStarted>(E("TurnStarted", """{"type":"TurnStarted","session":"desk","turn":"t","text":"x","model":"m"}"""));
        Assert.Null(E("Weird", """{"session":"desk"}"""));
        Assert.Null(E("TextDelta", "not json"));
    }
}