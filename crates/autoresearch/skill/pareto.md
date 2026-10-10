
## Several objectives (Pareto keeps)

Use this kind when the user names two to five numbers that trade against each other and would merge *either* kind of win — faster but larger, smaller but slower, more accurate but slower. The study then keeps a **front** of versions none of which beats another on every objective, and the user lands the one they want. Use a plain **guard** instead when one quantity is a hard limit ("never more than 5 % more memory"): a guard only blocks, it never competes.

Ask once: *"how much of each would you give up for a gain in another?"* The answer is each objective's `max_regression`: the largest loss on that objective a trade may carry and still be kept (empty = any loss, as long as the trade wins somewhere else). `min_effect` stays the smallest gain worth keeping, per objective.

Draft `objectives` instead of `metric`, and set the axes:

```yaml
axes: {objectives: pareto, search: front}
objectives:
  - {name: time_ms, unit: ms, direction: lower, role: objective, min_effect: 1%, max_regression: ""}
  - {name: size_kb, unit: kB, direction: lower, role: objective, min_effect: 1%, max_regression: 10%}
  - {name: peak_kb, unit: kB, direction: lower, role: guard, max_regression: 5%}   # a guard, as before
```

The benchmark prints one `METRIC <name>=` line per objective (and per guard); the first objective is the headline the scalar surfaces show. What changes in the loop:

- A candidate is compared with its **parent** (the front member it was cut from), objective by objective, at a keep threshold corrected for the number of objectives (two objectives: z 2.28 instead of 2.00; three: 2.43; wider still while the noise estimate rests on few samples). It is `improved` when it beats the parent on at least one objective and loses significantly on none — it replaces the parent; `traded` when it beats the parent on one and loses on another within that objective's `max_regression` — it joins the front beside the parent. A loss past `max_regression` is `worse`: nothing is kept. A new member must also beat every existing member on some objective; noise-level clones stay out. Only outright improvements evict members. At most eight members; past that the least distinct one leaves.
- Each experiment is cut from the member with the fewest children, so no trade-off is starved; the study worktree sits at that member when the session starts.
- Bench time grows 10–20 % (the stricter threshold sends more candidates to confirm). More than three objectives raises the bar for every keep; the card warns.
- The Study view shows every experiment on the first two objectives with the front as a staircase; the headline reads `front of 3 · time_ms −21 % · size_kb −12 %`.

Keep `parallel: 1` for a front study. Out of scope: more than five objectives, weighted sums (make that one metric), picking the landing member automatically.
