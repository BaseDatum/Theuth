# 184 rulings — notes on any machine

## Sky's report (2026-11-09 10:45, verbatim)

 "We have another issue we need to resolve with notes. with meshed nodes, a node that is not the hub is for some reason prevented from creating notes. and then even if it spawns a subagent on the hub to create a note there. if that note has executable cells, they want to run on the hub, and it seems can't be run on the machine they were made on, all directory checks and everything are for the hubs paths, not the nodes path where you want to run the cells. Cells displayed in a note should be executable and handled on the node that they're being viewed on, not the hub, and documents/notes should be able to be created on any node and synced like anything else. I'm not sure why this was done this way, probably for the live document sharing. but there's no reason that nodes that aren't the hub can't connect directly to the relay as well to share a note. Need to get this fixed up so this works properly on meshed nodes."

## Verified causes (recons NotesHubRecon, NoteCellsRecon, DocRelayRecon; main b23f877f/6b8d5084)
- Creation: `CreateNote`, `CreateBoard` and `Create` gate on the lease (document/src/actor.rs:1750, 1773, 1791 → hub.rs:114-127, `NotHub:<id>`).
  - The UI's doc_create forwards to the hub (rpc/src/api.rs doc_forward), so the note is born on the hub.
  - The agent tool doc_create (document/src/tools.rs:877-927) and the plugins never forward: meeting note, task thread, dreaming journal, autoresearch finalize and insights. So off the hub they fail.
  - This is a gap, not a ruled ban. 69 decision 11 (69-devices.md:71) ruled one live Doc on the hub for editing. The contract's blanket "every write is hub-only" (69-contract-docs.md:71) swept creation in.
- Cells: deliberate design 171 W-d (171-cells.md:359). cells/src/note.rs:212-219 route() returns the lease holder, and rpc/src/cells.rs:94-105 forwards every Scope::Doc call to it.
  - So cwd, `~`, interpreters, secrets, the process and cell_runs all belong to the hub. cell_runs is engine-local; cell_latest replicates but is hub-gated (document/src/cells.rs:255).
- Sharing: every Link-on engine registers /h/<engine id> and admits guests. Only the mesh uplink /h/<mesh id> is hub-only (link/src/follow.rs). Links name the mesh id (api.rs doc_link_create).

## Sky's rulings (2026-11-09 11:10, ask_user)
1. **home:** "Machine it was made on". Each note has a home machine that serves its live editing; the others edit through it, as with chats. While the home is offline the hub serves it, and the copies merge when the home returns.
2. **never_hub:** "Yes". A machine set to hub: never can be a note's home; never-hub covers coordination duties, not content.
3. **links:** "Note's machine, mesh as backup". A new link format names the home machine first and the mesh second, and old links keep working.
4. **cell_results:** "Latest run anywhere". Cells run on the machine they are viewed on, and each cell shows its most recent result from any machine, labelled "ran on <machine>". The live terminal and run history stay on the machine that ran it.

## Scope notes for the design (orchestrator, from the recons)
- Knowledge and artifact documents keep the hub, because of derived files, git and deterministic uuid-v5 ids.
- Meeting notes take the recording engine as home; task threads take the task home (182).
- Existing notes have no home, which means the hub, exactly as today.
- Off-hub creation must snapshot at once, so the mirror replicates.
- A cell run must read the block text from the live doc (the home), not the 2 s-stale replica.
- cell_latest needs an engine or host column.
- Link creation checks the home's Link state.
- P2's Reseal must run wherever docs are homed.

## Later rulings (Sky, 2026-11-09 11:47, ask_user, on the writer's §17 open questions)
- **Q1 move_note: "Yes, add Move to".** Notes get a "Move to ▸ machine" action, like tasks (182). It hands the note's home, and its live copy, to the chosen machine. This overrides the writer's default W-n.
- **Q2 unpaired_home: "Re-home them to the hub".** At unpair, every note homed on the removed machine is re-homed permanently to the current hub. If that machine is re-paired later, its notes stay on the hub.
- **Q3 link_off: "Make a mesh link".** If link sharing is off on the note's home, Share mints a mesh link instead (served from the hub, which passes the guest through to the home). Share never refuses for this reason. This overrides the writer's default.
- **Q4 partition: "Both stay editable".** Both sides edit and see a warning, and the edits merge when they reconnect. This is the writer's default (W-q).
- **Q5 stale_engine: "Ignore after 7 days".** A paired machine unseen for 7 days or more stops counting for the mixed-version gate. If it comes back on an old build, notes fall back to the hub until it upgrades.
