using System.Text;
using System.Text.Json;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>A step row's type, from its tool call's <c>purpose</c> prefix (§10.2: <c>checking: …</c> · <c>executing: …</c> · <c>verifying: …</c>; none = ran).</summary>
public enum StepKind
{
    Checking,
    Executing,
    Verifying,
    Ran,
}

/// <summary>Where a step row is: the first call in flight runs (shimmer), later ones in flight are "next", finished rows are green/red.</summary>
public enum StepStatus
{
    Running,
    Next,
    Ok,
    Failed,

    /// <summary>The approval was denied (by the user, or by the timeout).</summary>
    Denied,

    /// <summary>The turn was stopped (Esc or the cap) while this call ran.</summary>
    Cancelled,
}

/// <summary>One row of the step-typed card (§10.3): <c>checked    ls ~/Desktop/*.png · 14 files</c>.</summary>
public sealed record CommandStep(string CallId, string Tool, StepKind Kind, string Text, StepStatus Status, string? Command = null, string? Note = null)
{
    /// <summary>The row's verb: present while it runs ("checking"), "next" while queued, past once returned ("checked"), "denied" / "stopped".</summary>
    public string Verb => Status switch
    {
        StepStatus.Running => Kind switch { StepKind.Checking => "checking", StepKind.Executing => "executing", StepKind.Verifying => "verifying", _ => "running" },
        StepStatus.Next => "next",
        StepStatus.Denied => "denied",
        StepStatus.Cancelled => "stopped",
        _ => Kind switch { StepKind.Checking => "checked", StepKind.Executing => "executed", StepKind.Verifying => "verified", _ => "ran" },
    };

    public bool InFlight => Status is StepStatus.Running or StepStatus.Next;
}

/// <summary>What a purpose string says: the row type and its text (≤ <see cref="CommandCard.PurposeMax"/> chars, the tool name as fallback).</summary>
public readonly record struct ParsedPurpose(StepKind Kind, string Text);

/// <summary>The final line's mark (§10.3): ✓ done, ✗ failed, none = "replied" (the model ignored the profile; nothing breaks).</summary>
public enum CommandVerdict
{
    Ok,
    Failed,
    Replied,
}

public readonly record struct ParsedVerdict(CommandVerdict Kind, string Line);

/// <summary>The card's state for the current request (§3.4's table).</summary>
public enum CommandPhase
{
    Listening,
    Heard,
    Working,
    NeedsOk,
    Done,
    Stopped,
    Failed,
}

/// <summary>The amber approval box (§10.6): the pending call, its risk, the policy that asked, and the row that gives it context.</summary>
public sealed record CommandApprovalBox(string Id, string Tool, string Target, Risk Risk, ulong AtMs, string? Context = null, Decision? Answered = null, string? Line = null)
{
    /// <summary>"Run `rm -rf ~/Develop/theuth/build`?"</summary>
    public string Question => $"Run `{Target}`?";

    /// <summary>"destructive · policy: ask" — an approval reaches the companion only when the policy said ask.</summary>
    public string RiskLine => $"{Risk.RawValue().Replace('_', ' ')} · policy: ask";
}

/// <summary>One request block of the card (a follow-up appends another under the previous ✓ line, §10.5).</summary>
public sealed record CommandRequest(string Text, IReadOnlyList<CommandStep> Steps, string Reply, CommandPhase Phase, int StepCount = 0, string? Error = null,
    bool CapStopped = false, bool UserStopped = false, ulong StartedAtMs = 0, ulong? FinishedAtMs = null)
{
    public ParsedVerdict Verdict => CommandCard.ParseVerdict(Reply);
}

/// <summary>The whole card: the request blocks and the approval box of the running one.</summary>
public sealed record CommandCardState(IReadOnlyList<CommandRequest> Requests, CommandApprovalBox? Approval = null)
{
    public static CommandCardState Empty { get; } = new(Array.Empty<CommandRequest>());

    public CommandRequest? Current => Requests.Count > 0 ? Requests[^1] : null;

    public CommandPhase Phase => Current?.Phase ?? CommandPhase.Listening;

    /// <summary>The card never auto-hides in a failed or stopped-by-cap state (D13); done hides after 6 s when collapsed.</summary>
    public bool Settled => Phase is CommandPhase.Done or CommandPhase.Stopped or CommandPhase.Failed;
}

/// <summary>What the command-turn stream feeds the reducer (decoded by <see cref="CommandCard.FromEvent"/>).</summary>
public abstract record CommandInput
{
    private CommandInput() { }

    /// <summary>A command gesture started (the pill says "Command · theuth desk"; a follow-up opens a new block).</summary>
    public sealed record Listening(ulong AtMs) : CommandInput;

    /// <summary>The final words were sent to the desk ("heard → working").</summary>
    public sealed record Heard(string Text, ulong AtMs) : CommandInput;

    public sealed record TurnStarted : CommandInput;

    public sealed record ToolStarted(string CallId, string Tool, string? Purpose, string? Command) : CommandInput;

    public sealed record ToolFinished(string CallId, bool IsError, string? Output) : CommandInput;

    public sealed record ApprovalRequested(string Id, string Tool, string Target, Risk Risk, ulong AtMs) : CommandInput;

    public sealed record ApprovalResolved(string Id, Decision Decision, bool TimedOut, ulong AtMs) : CommandInput;

    /// <summary>The user answered the box (keys or voice); <see cref="Via"/> = <c>enter</c> · <c>esc</c> · <c>voice</c> · <c>click</c>.</summary>
    public sealed record Answer(Decision Decision, string Via) : CommandInput;

    /// <summary>A final utterance heard while the box listens that matched nothing.</summary>
    public sealed record Unheard : CommandInput;

    public sealed record TextDelta(string Delta) : CommandInput;

    public sealed record TurnFinished(string? StopReason, ulong AtMs) : CommandInput;

    public sealed record Error(string Message) : CommandInput;

    /// <summary>Esc while working: stop (the card freezes, it does not disappear).</summary>
    public sealed record Stop(ulong AtMs) : CommandInput;
}

/// <summary>What the driver must do after a step.</summary>
public abstract record CommandEffect
{
    private CommandEffect() { }

    /// <summary><c>cancel(desk, "root")</c> — <see cref="Why"/> = <c>cap</c> (§10.4) or <c>esc</c> (§12.4 stop freezes).</summary>
    public sealed record CancelTurn(string Why) : CommandEffect;

    /// <summary>The approval box is up: open the short spoken-approval dictation (§10.6).</summary>
    public sealed record OpenListen(string ApprovalId) : CommandEffect;

    public sealed record CloseListen : CommandEffect;

    /// <summary><c>resolve_approval(id, decision)</c> (the Attention panel's path, 134 D5).</summary>
    public sealed record ResolveApproval(string Id, Decision Decision, string Via) : CommandEffect;
}

/// <summary>
/// 153 §10.3–§10.6 (D10): the command card's reducer — the desk's turn events → step rows, the approval box, the turn cap and the
/// one ✓/✗ line. Rows come from <c>ToolCallStarted</c> (type from the <c>purpose</c> prefix, else "ran"), turn green/red on
/// <c>ToolCallFinished</c>; the text is <c>purpose</c> after the colon (≤ 60 chars; fallback the <c>reason</c> argument, then the tool
/// name — H3-C1). At the <see cref="MaxSteps"/>-th start the card asks the driver to <c>cancel(desk, "root")</c> and freezes with
/// "stopped after 12 steps" (the Mac twin's rule, H3-C1). Pure — the driver feeds events and executes effects.
/// </summary>
public static class CommandCard
{
    /// <summary>§10.4 / W-e: <c>COMMAND_MAX_STEPS</c>, tool calls per request, counted from <c>ToolCallStarted</c>.</summary>
    public const int MaxSteps = 12;

    /// <summary>§10.3: a row's text is at most this many chars.</summary>
    public const int PurposeMax = 60;

    /// <summary>The card's title (§10.3).</summary>
    public const string Title = "Command · theuth desk";

    public static (CommandCardState State, IReadOnlyList<CommandEffect> Effects) Observe(CommandCardState s, CommandInput input)
    {
        var none = Array.Empty<CommandEffect>();
        var cur = s.Current;
        switch (input)
        {
            case CommandInput.Listening l:
                // A gesture on a settled card is a follow-up: a new block under the previous ✓ line (§10.5). A gesture while the
                // previous request still works is the "hold to add" steer: the next Heard opens its own block too.
                if (cur is null || cur.Phase != CommandPhase.Listening)
                    return (s with { Requests = Append(s.Requests, new CommandRequest("", Array.Empty<CommandStep>(), "", CommandPhase.Listening, StartedAtMs: l.AtMs)) }, none);
                return (s, none);
            case CommandInput.Heard h:
                if (cur is { Phase: CommandPhase.Listening })
                    return (s with { Requests = Replace(s.Requests, cur with { Text = h.Text, Phase = CommandPhase.Heard, StartedAtMs = cur.StartedAtMs == 0 ? h.AtMs : cur.StartedAtMs }) }, none);
                return (s with { Requests = Append(s.Requests, new CommandRequest(h.Text, Array.Empty<CommandStep>(), "", CommandPhase.Heard, StartedAtMs: h.AtMs)) }, none);
            case CommandInput.TurnStarted:
                if (cur is { Phase: CommandPhase.Heard }) return (Set(s, cur with { Phase = CommandPhase.Working }), none);
                return (s, none);
            case CommandInput.ToolStarted t:
            {
                if (cur is null || !Live(cur)) return (s, none);
                var p = ParsePurpose(t.Purpose, t.Tool);
                var row = new CommandStep(t.CallId, t.Tool, p.Kind, p.Text, StepStatus.Running, t.Command);
                var steps = Restatus(Append(cur.Steps, row));
                if (cur.StepCount + 1 >= MaxSteps)
                {
                    // H3-C1: the 12th start is the cap — cancel(desk, root) and freeze ("stopped after 12 steps").
                    var frozen = cur with { Steps = Freeze(steps), StepCount = cur.StepCount + 1, Phase = CommandPhase.Stopped, CapStopped = true };
                    var fx = new List<CommandEffect> { new CommandEffect.CancelTurn("cap") };
                    if (s.Approval is not null) fx.Add(new CommandEffect.CloseListen());
                    return (Set(s, frozen) with { Approval = null }, fx);
                }
                var phase = cur.Phase == CommandPhase.NeedsOk ? CommandPhase.NeedsOk : CommandPhase.Working;
                return (Set(s, cur with { Steps = steps, StepCount = cur.StepCount + 1, Phase = phase }), none);
            }
            case CommandInput.ToolFinished f:
            {
                if (cur is null) return (s, none);
                var steps = cur.Steps.Select(r => r.CallId == f.CallId && r.InFlight
                    ? r with { Status = r.Status == StepStatus.Denied ? StepStatus.Denied : f.IsError ? StepStatus.Failed : StepStatus.Ok }
                    : r).ToList();
                return (Set(s, cur with { Steps = Restatus(steps) }), none);
            }
            case CommandInput.ApprovalRequested a:
            {
                if (cur is null || !Live(cur)) return (s, none);
                var context = cur.Steps.LastOrDefault(r => r.Status == StepStatus.Ok && r.Kind == StepKind.Checking)
                    ?? cur.Steps.LastOrDefault(r => r.Status == StepStatus.Ok);
                var box = new CommandApprovalBox(a.Id, a.Tool, Clip(a.Target, PurposeMax), a.Risk, a.AtMs, context is null ? null : $"{context.Verb}  {context.Text}");
                return (Set(s, cur with { Phase = CommandPhase.NeedsOk }) with { Approval = box }, new CommandEffect[] { new CommandEffect.OpenListen(a.Id) });
            }
            case CommandInput.Answer ans:
            {
                if (s.Approval is not { } box || box.Answered is not null) return (s, none);
                return (s with { Approval = box with { Answered = ans.Decision, Line = null } },
                    new CommandEffect[] { new CommandEffect.ResolveApproval(box.Id, ans.Decision, ans.Via), new CommandEffect.CloseListen() });
            }
            case CommandInput.Unheard:
                return s.Approval is { } b && b.Answered is null ? (s with { Approval = b with { Line = SpokenApproval.DidntCatch } }, none) : (s, none);
            case CommandInput.ApprovalResolved r:
            {
                if (s.Approval is not { } box || box.Id != r.Id || cur is null) return (s, none);
                var steps = cur.Steps;
                if (r.Decision == Decision.Deny)
                {
                    // The call that asked: the newest in-flight row of the same tool (ApprovalRequested carries no call id).
                    var target = cur.Steps.LastOrDefault(x => x.InFlight && x.Tool == box.Tool) ?? cur.Steps.LastOrDefault(x => x.InFlight);
                    if (target is not null)
                    {
                        ulong waited = r.AtMs >= box.AtMs ? (r.AtMs - box.AtMs + 500) / 1000 : 0;
                        var note = r.TimedOut ? $"✗ timed out — denied (no answer in {waited} s)" : "✗ denied";
                        steps = Restatus(cur.Steps.Select(x => ReferenceEquals(x, target) ? x with { Status = StepStatus.Denied, Note = note } : x).ToList());
                    }
                }
                var phase = cur.Phase == CommandPhase.NeedsOk ? CommandPhase.Working : cur.Phase;
                IReadOnlyList<CommandEffect> fx = box.Answered is null ? new CommandEffect[] { new CommandEffect.CloseListen() } : none;
                return (Set(s, cur with { Steps = steps, Phase = phase }) with { Approval = null }, fx);
            }
            case CommandInput.TextDelta t:
                if (cur is null || cur.Phase is CommandPhase.Listening or CommandPhase.Stopped) return (s, none);
                return (Set(s, cur with { Reply = cur.Reply + t.Delta, Phase = cur.Phase == CommandPhase.Heard ? CommandPhase.Working : cur.Phase }), none);
            case CommandInput.Error e:
                if (cur is null || cur.Phase is CommandPhase.Listening or CommandPhase.Stopped) return (s, none);
                return (Set(s, cur with { Phase = CommandPhase.Failed, Error = e.Message, Steps = Freeze(cur.Steps) }) with { Approval = null },
                    s.Approval is not null ? new CommandEffect[] { new CommandEffect.CloseListen() } : none);
            case CommandInput.TurnFinished tf:
            {
                if (cur is null || cur.Phase is CommandPhase.Listening or CommandPhase.Done) return (s, none);
                IReadOnlyList<CommandEffect> fx = s.Approval is not null ? new CommandEffect[] { new CommandEffect.CloseListen() } : none;
                if (cur.Phase is CommandPhase.Stopped || cur.CapStopped || cur.UserStopped)
                    return (Set(s, cur with { Phase = CommandPhase.Stopped, Steps = Freeze(cur.Steps), FinishedAtMs = tf.AtMs }) with { Approval = null }, fx);
                if (cur.Phase is CommandPhase.Failed) return (Set(s, cur with { FinishedAtMs = tf.AtMs }) with { Approval = null }, fx);
                if (tf.StopReason is "error" or "cancelled")
                    return (Set(s, cur with { Phase = CommandPhase.Failed, Error = cur.Error ?? tf.StopReason, Steps = Freeze(cur.Steps), FinishedAtMs = tf.AtMs }) with { Approval = null }, fx);
                return (Set(s, cur with { Phase = CommandPhase.Done, Steps = Freeze(cur.Steps), FinishedAtMs = tf.AtMs }) with { Approval = null }, fx);
            }
            case CommandInput.Stop st:
            {
                if (cur is null || !Live(cur)) return (s, none);
                var fx = new List<CommandEffect> { new CommandEffect.CancelTurn("esc") };
                if (s.Approval is not null) fx.Add(new CommandEffect.CloseListen());
                return (Set(s, cur with { Phase = CommandPhase.Stopped, UserStopped = true, Steps = Freeze(cur.Steps), FinishedAtMs = st.AtMs }) with { Approval = null }, fx);
            }
            default:
                return (s, none);
        }
    }

    /// <summary>
    /// §10.2/§10.3: <c>checking: ls ~/Desktop/*.png · 14 files</c> → (Checking, "ls ~/Desktop/*.png · 14 files"); the prefix is
    /// matched loosely (check…/execut…/verif…, any case); no prefix → Ran with the whole purpose; no purpose → Ran with the tool name.
    /// </summary>
    public static ParsedPurpose ParsePurpose(string? purpose, string tool)
    {
        var p = (purpose ?? "").Trim();
        if (p.Length == 0) return new ParsedPurpose(StepKind.Ran, Clip(tool, PurposeMax));
        int colon = p.IndexOf(':');
        if (colon > 0 && colon <= 12)
        {
            var head = p[..colon].Trim().ToLowerInvariant();
            var rest = p[(colon + 1)..].Trim();
            StepKind? kind = head.StartsWith("check", StringComparison.Ordinal) ? StepKind.Checking
                : head.StartsWith("execut", StringComparison.Ordinal) ? StepKind.Executing
                : head.StartsWith("verif", StringComparison.Ordinal) ? StepKind.Verifying
                : null;
            if (kind is { } k) return new ParsedPurpose(k, Clip(rest.Length > 0 ? rest : tool, PurposeMax));
        }
        return new ParsedPurpose(StepKind.Ran, Clip(p, PurposeMax));
    }

    /// <summary>
    /// The reply's mark: ✓ (also ✔ ✅) → Ok, ✗ (also ✘ ❌) → Failed, anything else → Replied ("replied"; shown as is). <see cref="ParsedVerdict.Line"/>
    /// is the trimmed reply.
    /// </summary>
    public static ParsedVerdict ParseVerdict(string? reply)
    {
        var line = (reply ?? "").Trim();
        if (line.Length == 0) return new ParsedVerdict(CommandVerdict.Replied, "");
        if (line.StartsWith('✓') || line.StartsWith('✔') || line.StartsWith("✅", StringComparison.Ordinal)) return new ParsedVerdict(CommandVerdict.Ok, line);
        if (line.StartsWith('✗') || line.StartsWith('✘') || line.StartsWith("❌", StringComparison.Ordinal)) return new ParsedVerdict(CommandVerdict.Failed, line);
        return new ParsedVerdict(CommandVerdict.Replied, line);
    }

    /// <summary>"4 steps  ls · mkdir/mv · ls · applescript": per step the programs of a shell command (≤ 3, joined by /), else the tool.</summary>
    public static string Summary(IReadOnlyList<CommandStep> steps)
    {
        if (steps.Count == 0) return "no steps";
        var labels = steps.Select(Short).ToList();
        return $"{steps.Count} step{(steps.Count == 1 ? "" : "s")}  {string.Join(" · ", labels)}";
    }

    private static string Short(CommandStep step)
    {
        if (step.Command is not { Length: > 0 } cmd) return step.Tool;
        var programs = new List<string>();
        foreach (var part in cmd.Split(new[] { "&&", "||", ";", "|", "\n" }, StringSplitOptions.RemoveEmptyEntries))
        {
            var words = part.Trim().Split(' ', StringSplitOptions.RemoveEmptyEntries);
            var first = words.FirstOrDefault(w => !w.Contains('=') && w is not ("sudo" or "env" or "time" or "nice"));
            if (first is null) continue;
            var name = first.Replace('\\', '/');
            name = name[(name.LastIndexOf('/') + 1)..];
            if (name.Length > 0 && !programs.Contains(name)) programs.Add(name);
            if (programs.Count == 3) break;
        }
        return programs.Count > 0 ? string.Join("/", programs) : step.Tool;
    }

    /// <summary>
    /// The status line under the rows for the current phase (§3.4's copy, Windows key words): "Ctrl+Alt+C held" · "heard → working" ·
    /// "Esc stop · hold Ctrl+Alt+C to add" · the approval question · "O open session · Esc dismiss · hold Ctrl+Alt+C follow up" ·
    /// "stopped after 12 steps — hold Ctrl+Alt+C to continue" · "✗ couldn't finish — ‹message›".
    /// </summary>
    public static string StatusCopy(CommandCardState s, string keyText)
    {
        var cur = s.Current;
        if (s.Approval is { } box)
        {
            if (box.Answered is { } d) return d == Decision.Allow ? "✓ approved — working…" : "✗ denied — waiting for the desk…";
            return $"{box.Question}  {box.RiskLine} · Yes Enter · No Esc · or say “yes” / “no”";
        }
        return cur?.Phase switch
        {
            null or CommandPhase.Listening => $"{keyText} held",
            CommandPhase.Heard => "heard → working",
            CommandPhase.Working or CommandPhase.NeedsOk => $"Esc stop · hold {keyText} to add",
            CommandPhase.Done => $"O open session · Esc dismiss · hold {keyText} follow up",
            CommandPhase.Stopped when cur.CapStopped => $"stopped after {MaxSteps} steps — hold {keyText} to continue",
            CommandPhase.Stopped => $"stopped · hold {keyText} to continue",
            CommandPhase.Failed => FailedLine(cur.Error),
            _ => "",
        };
    }

    /// <summary>"✗ couldn't finish — ‹message›" (§10.4), the message's first line clipped.</summary>
    public static string FailedLine(string? message)
    {
        var first = (message ?? "").Split('\n', StringSplitOptions.RemoveEmptyEntries).Select(l => l.Trim()).FirstOrDefault(l => l.Length > 0) ?? "unknown error";
        return $"✗ couldn't finish — {Clip(first, 120)}";
    }

    /// <summary>The header: "Command · theuth desk", "· done · 6 s" once finished (§10.3).</summary>
    public static string Header(CommandCardState s)
    {
        var cur = s.Current;
        if (cur is { Phase: CommandPhase.Done, FinishedAtMs: { } end }) return $"{Title} · done · {Seconds(cur.StartedAtMs, end)} s";
        if (cur is { Phase: CommandPhase.Stopped }) return $"{Title} · stopped";
        if (cur is { Phase: CommandPhase.Failed }) return $"{Title} · failed";
        return Title;
    }

    /// <summary>§10.2 / W-l: <c>[command, N s] ‹text›</c> — the user line the desk's command profile keys on (at least one second).</summary>
    public static string CommandText(string text, long durationMs) => $"[command, {Math.Max(1, (long)Math.Ceiling(durationMs / 1000.0))} s] {text}";

    /// <summary>The command line plus the engine's <c>[screen …]</c> lines when "Include the screen" is on (as the voice key's <c>composeBody</c>).</summary>
    public static string ComposeBody(string text, long durationMs, IReadOnlyList<string> screenLines)
    {
        var line = CommandText(text.Trim(), durationMs);
        return screenLines.Count == 0 ? line : line + "\n" + string.Join("\n", screenLines);
    }

    /// <summary>The follow-up line after a cap stop says "continue" (§10.4: "the next [command] line says continue").</summary>
    public static string ContinueText(CommandCardState s, string words) =>
        s.Current is { CapStopped: true } ? (words.Trim().Length > 0 ? $"continue — {words.Trim()}" : "continue") : words;

    /// <summary>
    /// The engine event → a reducer input, for the desk session only (<paramref name="session"/>); <c>ApprovalResolved</c> carries no
    /// session and is matched by id in <see cref="Observe"/>. Null for anything else.
    /// </summary>
    public static CommandInput? FromEvent(string type, ReadOnlySpan<byte> json, string session, ulong nowMs)
    {
        JsonDocument doc;
        try { doc = JsonDocument.Parse(json.ToArray()); }
        catch (JsonException) { return null; }
        using (doc)
        {
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object) return null;
            string? Str(JsonElement o, string name) => o.ValueKind == JsonValueKind.Object && o.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;
            if (type != "ApprovalResolved" && Str(root, "session") != session) return null;
            switch (type)
            {
                case "TurnStarted": return new CommandInput.TurnStarted();
                case "TextDelta": return Str(root, "text") is { } t ? new CommandInput.TextDelta(t) : null;
                case "ToolCallStarted":
                {
                    var args = root.TryGetProperty("args", out var a) ? a : default;
                    return new CommandInput.ToolStarted(Str(root, "call_id") ?? "", Str(root, "name") ?? "tool", Str(args, "purpose") ?? Str(args, "reason"), Str(args, "command"));
                }
                case "ToolCallFinished":
                    return new CommandInput.ToolFinished(Str(root, "call_id") ?? "", root.TryGetProperty("is_error", out var e) && e.ValueKind == JsonValueKind.True, Str(root, "output"));
                case "ApprovalRequested":
                {
                    var args = root.TryGetProperty("args", out var a) ? a : default;
                    var risk = Str(root, "risk") switch { "read_only" => Risk.ReadOnly, "mutating" => Risk.Mutating, _ => Risk.Destructive };
                    var tool = Str(root, "tool") ?? "tool";
                    return new CommandInput.ApprovalRequested(Str(root, "id") ?? "", tool, Target(tool, args), risk, nowMs);
                }
                case "ApprovalResolved":
                {
                    var decision = Str(root, "decision") == "allow" ? Decision.Allow : Decision.Deny;
                    bool timedOut = root.TryGetProperty("timed_out", out var to) && to.ValueKind == JsonValueKind.True;
                    return new CommandInput.ApprovalResolved(Str(root, "id") ?? "", decision, timedOut, nowMs);
                }
                case "TurnFinished": return new CommandInput.TurnFinished(Str(root, "stop_reason"), nowMs);
                case "Error": return new CommandInput.Error(Str(root, "message") ?? "the desk failed");
                default: return null;
            }
        }
    }

    /// <summary>What the approval box names: the shell command, else a path, else the purpose, else the compact arguments.</summary>
    public static string Target(string tool, JsonElement args)
    {
        if (args.ValueKind == JsonValueKind.Object)
        {
            foreach (var key in new[] { "command", "script", "path", "url", "purpose" })
            {
                if (args.TryGetProperty(key, out var v) && v.ValueKind == JsonValueKind.String && v.GetString() is { Length: > 0 } s) return s;
            }
            var raw = args.GetRawText();
            return raw.Length > 2 ? $"{tool} {raw}" : tool;
        }
        return tool;
    }

    private static bool Live(CommandRequest r) => r.Phase is CommandPhase.Heard or CommandPhase.Working or CommandPhase.NeedsOk;

    /// <summary>The first in-flight row runs (shimmer); later in-flight rows are "next".</summary>
    private static IReadOnlyList<CommandStep> Restatus(IReadOnlyList<CommandStep> steps)
    {
        bool running = false;
        var outRows = new List<CommandStep>(steps.Count);
        foreach (var r in steps)
        {
            if (!r.InFlight) { outRows.Add(r); continue; }
            outRows.Add(r with { Status = running ? StepStatus.Next : StepStatus.Running });
            running = true;
        }
        return outRows;
    }

    /// <summary>A stopped/finished request: calls still in flight are frozen as stopped.</summary>
    private static IReadOnlyList<CommandStep> Freeze(IReadOnlyList<CommandStep> steps) =>
        steps.Select(r => r.InFlight ? r with { Status = StepStatus.Cancelled } : r).ToList();

    private static CommandCardState Set(CommandCardState s, CommandRequest r) => s with { Requests = Replace(s.Requests, r) };

    private static IReadOnlyList<T> Append<T>(IReadOnlyList<T> list, T item)
    {
        var l = new List<T>(list.Count + 1);
        l.AddRange(list);
        l.Add(item);
        return l;
    }

    private static IReadOnlyList<CommandRequest> Replace(IReadOnlyList<CommandRequest> list, CommandRequest last)
    {
        var l = list.ToList();
        if (l.Count == 0) l.Add(last);
        else l[^1] = last;
        return l;
    }

    private static ulong Seconds(ulong from, ulong to) => to > from ? (to - from + 500) / 1000 : 0;

    /// <summary>At most <paramref name="max"/> chars, an ellipsis when cut, one line.</summary>
    public static string Clip(string s, int max)
    {
        var one = s.Replace("\r", " ").Replace("\n", " ").Trim();
        return one.Length <= max ? one : one[..(max - 1)].TrimEnd() + "…";
    }
}