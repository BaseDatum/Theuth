using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>
/// 189w D1: the one writer of this connection's event focus, however many parts of the companion need a session's deltas.
/// The engine keeps ONE focus set per connection and <c>set_event_focus</c> REPLACES it (<c>crates/rpc/src/api.rs:10554-10591</c>,
/// insert :10573): while a set exists, a delta event (<c>TextDelta</c>, <c>ReasoningDelta</c>, <c>ToolCallProgress</c>,
/// <c>VisionProgress</c>) of a session outside it is dropped (:10472-10481) — and an EMPTY set drops every delta. So the set that goes
/// out must always be the UNION of every claim, never one claimant's view of it (the macOS companion lost its Ask reply to a second
/// writer that sent <c>[]</c>). Claimants here: <c>ask</c> (the Ask bar's desk) and <c>point</c> (the voice key's and command mode's
/// desk, which resolves its own <c>desk_open</c> and used to claim nothing). Claims are kept by owner; <see cref="SendAsync"/> sends
/// the union now, FORCED (the engine may have lost it with a connection); sends are serialized — one in flight, requests that arrive
/// meanwhile coalesce into the next one, which reads the claims at its own start, so the LAST claim wins. Waiters resume inline, on the
/// thread that finished the send (the owner thread: async work resumes there). Nothing is sent for an empty
/// union (a release never sends <c>[]</c>). Owner-thread only, like the Store.
/// </summary>
public sealed class EventFocus
{
    private readonly Func<IReadOnlyList<string>, Task> send;
    private readonly Action<string> log;
    private readonly Dictionary<string, string[]> claims = new(StringComparer.Ordinal);
    private readonly List<(long Ticket, TaskCompletionSource Done)> waiters = new();
    private readonly List<string> reasons = new();
    private long requested;
    private long completed;
    private bool pumping;

    public EventFocus(Func<IReadOnlyList<string>, Task> send, Action<string> log)
    {
        this.send = send;
        this.log = log;
    }

    /// <summary>Every claimed session once, sorted — what the next send carries.</summary>
    public IReadOnlyList<string> Union =>
        claims.Values.SelectMany(s => s).Distinct(StringComparer.Ordinal).Order(StringComparer.Ordinal).ToArray();

    /// <summary>The owner's claim (null when it has none).</summary>
    public IReadOnlyList<string>? ClaimOf(string owner) => claims.TryGetValue(owner, out var s) ? s : null;

    /// <summary>The set the engine last accepted (null before the first send).</summary>
    public IReadOnlyList<string>? LastSent { get; private set; }

    /// <summary>How many <c>set_event_focus</c> calls were answered with success.</summary>
    public int Sends { get; private set; }

    /// <summary>The ticket of the latest request: <see cref="EnsureSinceAsync"/> compares against it.</summary>
    public long Requested => requested;

    /// <summary>Replace <paramref name="owner"/>'s claim; sends nothing (see <see cref="SendAsync"/>).</summary>
    public void Claim(string owner, IEnumerable<string> sessions) => claims[owner] = sessions.Distinct(StringComparer.Ordinal).ToArray();

    /// <summary>Drop <paramref name="owner"/>'s claim. Sends nothing: the engine keeps the last set, and an empty one would drop every delta.</summary>
    public bool Release(string owner) => claims.Remove(owner);

    /// <summary><see cref="Claim"/> then <see cref="SendAsync"/>.</summary>
    public Task ClaimAsync(string owner, IEnumerable<string> sessions, string reason)
    {
        Claim(owner, sessions);
        return SendAsync(reason);
    }

    /// <summary>
    /// Send the union now, even when it equals the last one. The task ends when a send that read the claims AFTER this call has been
    /// answered (success or failure — the failure is logged, never thrown: a refused focus must not stop an ask).
    /// </summary>
    public Task SendAsync(string reason)
    {
        long ticket = ++requested;
        if (!reasons.Contains(reason)) reasons.Add(reason);
        var done = new TaskCompletionSource();
        waiters.Add((ticket, done));
        if (!pumping)
        {
            pumping = true;
            _ = Pump();
        }
        return done.Task;
    }

    /// <summary>
    /// "Sent for this ask": waits for a request made after <paramref name="mark"/> (<see cref="Requested"/> taken at the ask's start —
    /// the desk's first resolve makes one), else makes one now. Every ask therefore sends the focus exactly once, first or not.
    /// </summary>
    public Task EnsureSinceAsync(long mark, string reason)
    {
        if (requested <= mark) return SendAsync(reason);
        if (completed >= requested) return Task.CompletedTask;
        var done = new TaskCompletionSource();
        waiters.Add((requested, done));
        return done.Task;
    }

    private async Task Pump()
    {
        try
        {
            while (completed < requested)
            {
                long upTo = requested;
                var set = Union;
                var why = string.Join("+", reasons);
                reasons.Clear();
                if (set.Count == 0)
                {
                    log($"set_event_focus skipped ({why}): no claims");
                }
                else
                {
                    try
                    {
                        await send(set);
                        LastSent = set;
                        Sends++;
                        log($"set_event_focus([{Short(set)}]) ok ({why})");
                    }
                    catch (Exception e)
                    {
                        log($"set_event_focus failed ({why}): {(e is RpcException r ? r.ToString() : e.Message)}");
                    }
                }
                completed = upTo;
                Flush();
            }
        }
        finally
        {
            pumping = false;
            Flush();
        }
    }

    private void Flush()
    {
        for (int i = waiters.Count - 1; i >= 0; i--)
        {
            if (waiters[i].Ticket > completed) continue;
            var done = waiters[i].Done;
            waiters.RemoveAt(i);
            done.TrySetResult();
        }
    }

    private static string Short(IReadOnlyList<string> sessions) =>
        string.Join(",", sessions.Select(s => s.Length > 8 ? s[..8] + "…" : s));
}