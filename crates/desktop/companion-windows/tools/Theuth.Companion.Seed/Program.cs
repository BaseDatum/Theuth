using System.Globalization;
using System.Text.Json;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Seed;

/// <summary>
/// <c>seed decision|blocked|hire|all [--count N] [--title TEXT] [--name AGENT]</c> · <c>seed list</c> · <c>seed clear</c> — real
/// attention items on the local engine (engine.json + token, as the companion finds it) through the engine's own RPCs,
/// for UI tests on a scratch engine that has no model provider. Seeded tasks carry project <c>companion-seed</c> and are
/// owned and reviewed by the user (no desk is woken); seeded hires are agent files named <c>seed-…</c>.
/// <c>clear</c> cancels every <c>companion-seed</c> task, removes every <c>seed-</c> proposal and dismisses what is left of them.
/// </summary>
public static class Program
{
    private const string Project = "companion-seed";

    private static readonly Dictionary<string, string> NeedsAModel = new(StringComparer.Ordinal)
    {
        ["approval"] = "an approval is a live tool call waiting in a session — it needs a model turn",
        ["ask"] = "an ask is an ask_user call from a live session — it needs a model turn",
        ["show"] = "a show item is an artifact a session showed — it needs a model turn",
        ["finished"] = "a finished item is an agent's ended turn (with ## Next) — it needs a model turn",
        ["secret"] = "a secret request comes from a session's secret_request — it needs a model turn",
        ["handoff"] = "a handoff is a session asking for the user at its desktop — it needs a model turn",
        ["resume"] = "a resume item is a turn interrupted by a restart — it needs a model turn",
        ["budget"] = "a budget item needs an agent's monthly spend at 80 % of its cap — it needs model spend",
        ["review"] = "a review is a task its owner moved doing → review; doing is task_checkout by a session (the user's task_update refuses backlog/todo → review) — it needs a desk turn",
        ["node"] = "a node item needs a mesh peer that went unreachable — it needs a second engine",
    };

    public static async Task<int> Main(string[] args)
    {
        if (args.Length == 0 || args[0] is "help" or "--help" or "-h")
        {
            Console.WriteLine("usage: seed decision|blocked|hire|all [--count N] [--title TEXT] [--name AGENT] | list | clear");
            Console.WriteLine("  decision  a user task blocked on a decision:<id> (no packet: only an agent's escalate tool writes the options)");
            Console.WriteLine("  blocked   a user task blocked on a non-decision blocker");
            Console.WriteLine("  hire      a proposed agent seed-<n> (agent_put with proposed_by)");
            Console.WriteLine("  not seedable without a model: " + string.Join(", ", NeedsAModel.Keys));
            return 0;
        }
        int count = 1;
        string? title = null;
        string? agentName = null; // 149 NATIVE proof: `hire --name <a-z0-9-, ≤32>` seeds a hire whose card header name is long
        var kinds = new List<string>();
        for (int i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--count" when i + 1 < args.Length: count = int.Parse(args[++i], CultureInfo.InvariantCulture); break;
                case "--title" when i + 1 < args.Length: title = args[++i]; break;
                case "--name" when i + 1 < args.Length: agentName = args[++i]; break;
                default: kinds.Add(args[i]); break;
            }
        }

        var location = EngineLocation.Discover();
        Console.WriteLine($"engine {location.Url.AbsoluteUri}");
        var opened = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        await using var client = new RpcClient(location, s => { if (s == RpcClient.State.Open) opened.TrySetResult(); }, _ => { });
        client.Start();
        if (await Task.WhenAny(opened.Task, Task.Delay(TimeSpan.FromSeconds(10))) != opened.Task)
        {
            Console.Error.WriteLine("seed: the engine did not answer within 10 s");
            return 2;
        }

        int failures = 0;
        if (kinds.Contains("clear")) await Clear(client);
        else if (!kinds.Contains("list"))
        {
            if (kinds.Remove("all")) kinds.AddRange(new[] { "decision", "blocked", "hire", "review" });
            foreach (var kind in kinds)
            {
                if (NeedsAModel.TryGetValue(kind, out var why))
                {
                    Console.WriteLine($"cannot seed {kind}: {why}");
                    continue;
                }
                for (int n = 1; n <= count; n++)
                {
                    try { Console.WriteLine(await SeedOne(client, kind, title, n, agentName)); }
                    catch (Exception e)
                    {
                        failures++;
                        Console.Error.WriteLine($"seed {kind} failed: {(e is RpcException r ? r.ToString() : e.Message)}");
                    }
                }
            }
        }
        await PrintAttention(client);
        return failures == 0 ? 0 : 1;
    }

    private static async Task<string> SeedOne(RpcClient c, string kind, string? title, int n, string? agentName = null)
    {
        var stamp = DateTimeOffset.Now.ToString("HHmmss", CultureInfo.InvariantCulture);
        switch (kind)
        {
            case "decision":
            {
                var id = await CreateTask(c, title ?? $"Seed decision {n}: which layout for the pricing page?");
                var decision = $"decision:{Guid.NewGuid()}";
                await c.CallRawAsync("task_update", id, new { state = "blocked", blockers = new[] { decision } });
                return $"seeded decision task {id} blocked on {decision}";
            }
            case "blocked":
            {
                var id = await CreateTask(c, title ?? $"Seed blocked {n}: waiting on the staging bucket");
                await c.CallRawAsync("task_update", id, new { state = "blocked", blockers = new[] { "waiting on the staging bucket" } });
                return $"seeded blocked task {id}";
            }
            case "hire":
            {
                var name = agentName ?? $"seed-{stamp}-{n}";
                await c.CallRawAsync("agent_put", new
                {
                    name,
                    title = title ?? $"Seed hire {n}",
                    reports_to = "theuth",
                    budget = new { type = "none" },
                    paused = true,
                    proposed_by = "agent:theuth",
                    runs_in = "any",
                    body = "# seed\nA proposal made by the companion seeding tool.\n",
                });
                return $"seeded hire {name}";
            }
            default:
                throw new ArgumentException($"unknown kind {kind} (decision, blocked, hire, all, list, clear)");
        }
    }

    private static async Task<string> CreateTask(RpcClient c, string title)
    {
        var task = await c.CallRawAsync("task_create", new
        {
            state = "todo", // backlog → blocked is refused; todo → blocked is the user's move (ledger guard)
            title,
            brief = "Created by the companion seeding tool for a UI test; `seed clear` removes it.",
            owner = new { type = "user" },
            reviewer = new { type = "user" },
            project = Project,
        });
        using var doc = JsonDocument.Parse(task);
        return doc.RootElement.GetProperty("id").GetString()!;
    }

    private static async Task Clear(RpcClient c)
    {
        var tasks = await c.CallRawAsync("tasks_list", new { project = Project });
        using (var doc = JsonDocument.Parse(tasks))
        {
            foreach (var t in doc.RootElement.EnumerateArray())
            {
                var id = t.GetProperty("id").GetString()!;
                var state = t.GetProperty("state").GetString();
                if (state is "cancelled" or "done") continue;
                try
                {
                    await c.CallRawAsync("task_update", id, new { state = "cancelled" });
                    Console.WriteLine($"cancelled task {id} (was {state})");
                }
                catch (RpcException e)
                {
                    Console.Error.WriteLine($"cancel {id} failed: {e}");
                }
            }
        }
        var agents = await c.CallRawAsync("agents_list");
        using (var doc = JsonDocument.Parse(agents))
        {
            foreach (var a in doc.RootElement.EnumerateArray())
            {
                var name = a.GetProperty("file").GetProperty("name").GetString()!;
                if (!name.StartsWith("seed-", StringComparison.Ordinal)) continue;
                await c.CallRawAsync("agent_remove", name, "removed by seed clear");
                Console.WriteLine($"removed proposal {name}");
            }
        }
        // Anything of ours still listed (an item the engine keeps after the task moved) is dismissed.
        foreach (var item in await c.AttentionListAsync())
        {
            bool ours = (item.Task is { } t && await IsSeedTask(c, t)) || (item.Kind == AttentionKind.Hire && item.Agent?.StartsWith("seed-", StringComparison.Ordinal) == true);
            if (!ours) continue;
            await c.AttentionDismissAsync(item.Id);
            Console.WriteLine($"dismissed {item.Id}");
        }
    }

    private static async Task<bool> IsSeedTask(RpcClient c, string id)
    {
        try
        {
            using var doc = JsonDocument.Parse(await c.CallRawAsync("task_get", id));
            return doc.RootElement.TryGetProperty("project", out var p) && p.ValueKind == JsonValueKind.String && p.GetString() == Project;
        }
        catch (RpcException)
        {
            return false;
        }
    }

    private static async Task PrintAttention(RpcClient c)
    {
        var items = await c.AttentionListAsync();
        Console.WriteLine($"attention_list={items.Count} [{string.Join(",", items.Select(i => i.Kind.RawValue))}]");
        foreach (var i in items) Console.WriteLine($"  {i.Kind.RawValue,-9} {i.Id}  {i.Title}");
    }
}