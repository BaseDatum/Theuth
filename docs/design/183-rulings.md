# 183 — Sky's asks and rulings (2026-11-08)

## Asks, verbatim (11:17–11:31, typed while the 2.18.12 Linux build ran)
1. **A2A setup UI.** "If I wanted a better UI for setting up network A2A on theuth, what would that require? Essentially I want to make it where with mdns (the one that doesn't require a dns server and works over lan) as well as through the relay, agents can communicate if they're paired or something… how should that work?"
2. **Messaging across the mesh.** "and if two nodes are in the same mesh, can those sessions communicate freely with each other now? I believe that was the intention. if both harnesses have session communication on, and are part of the same mesh, they should be able to message each other across the mesh as well as different sessions on the same machine."
3. **Live state on other nodes.** "also, when I continue a session on another machine, why doesn't the state of that session stream to other reachable nodes in the mesh and update live like when I pair a device to a mesh."
4. **Starting a session on a node.** "and what happens with a device on a mesh, do I have UI to start a session on a specific node in the mesh?"
5. **Telling the model about a move.** "oh and for the continue here is there any kind of harness message to the agent/model that says, your session has moved to node: x, os: linux, etc. so the agent is aware that this has occurred? that message should have custom UI just like all the other messages and not be just a blob of xml or whatever visible to the user."
6. **Operating a session without moving it.** "Also if I send a message from another machine, that should not cause the session to move, I should be able to use node A's UI to operate node B's sessions just like I'm running them from a paired pure UI device that is not a node like ipad or iphone."
7. **Remote approvals.** "Oh and I should be able to control approvals from the three dots for a remote session, so if I use a cloud node session from a different node I can turn on approvals there without moving the session."

## What Sky was shown before ruling
- A table of each ask against what exists today and what it needs. It is in the three recon sheets (183-recon-*.md).
- A three-phase plan: Phase 1 remote sessions, Phase 2 network setup, Phase 3 contacts.
- The mock docs/design/assets/183-network-and-remote-sessions.html, with five panels:
  - Settings › Network (This machine, Nearby, Machines in this mesh, Contacts);
  - the pair dialog with a 6-digit code;
  - a remote session with an "on cloud-1 · live" chip, a relayed approval card, and a ⋯ menu holding Approvals, Agent messaging, Model, Continue here and Move to;
  - the "Session moved to xeus · Arch Linux · ~/Develop/theuth" harness card;
  - New chat on [machine ▾] with that machine's folders.
- A warning that pairing today means full trust (vault key, sessions, tasks, full RPC).

## Rulings (ask_user, 11:56)
| # | Question | Sky's answer | Changed from recommendation? |
|---|---|---|---|
| D1 | When you type, approve or change a setting in another machine's chat… | **Run on the owner.** This reverses design 68 ruling 5 ("user sessions follow you"). A session moves only via Continue here or Move to. | no |
| D2 | When the owning machine is offline and you type… | **Ask: Continue here?** A card offers to move the session here. Nothing runs until accepted. | no |
| D3 | Should pairing let agents on both machines message each other? | Freeform, verbatim: "basically agents/sessions on nodes in the same mesh should be able to message each other by default, this is for separate harnesses that are not in a mesh. we should use some kind of a2a standard for pairing if one exists." | YES. The recommendation was a checkbox in the pair dialog. Ruled instead: messaging is on by default inside a mesh; the pairing ceremony is for separate harnesses and uses an A2A standard if one exists. |
| D4 | When a session moves, its approvals and agent-messaging settings… | **Go with the session.** They are saved on the session row. | no |
| D5 | Other people's machines (the contacts tier, limited trust)… | **Design it now too.** All three phases go in one design. | YES. The recommendation was to design it later. |

Sky has NOT said go for the build. This design is the contract the build will follow when he does. (As of 11:56; D10 below, 12:33–12:36, says go.)

## Later rulings (12:33–12:36)

| # | Question | Sky's answer |
|---|---|---|
| D6 | §17 Q1: with mesh messaging on by default, spawn_subagent is also on, because both use one switch | First answer: "This question is not clear, clarify". After clarification: **Yes, one setting.** |
| D7 | §17 Q2: unpair does not rotate the vault key | First answer: "Unpairing A2A over network or mesh? Clarify". The clarification: this is the mesh, where secrets and the key are shared. Answer: **Also rotate the key now.** On unpair the remaining machines re-encrypt with a new key, AND unpair warns and lists the secrets to replace at their providers |
| D8 | §17 Q3: a remote session with approvals on, nobody answers | **Deny after 5 min.** Same as local; W-6 stands |
| D9 | §17 Q4: what contacts can reach | First answer: "Also chosen chats". Clarified: **Their agents message it.** Another harness's agents can send into a chat you pick, as the A2A plugin's exact-session exports already allow. There is NO live view for the other person, and no typing from their app |
| D10 | Build? | **Go: all phases in order** |
| D11 | §17 Q6: the removed machine keeps `mesh/device_key`, so it could pose as this mesh to paired phones and iPads | **Offer a checkbox** (12:59): the Unpair dialog gets "Also reset phone and iPad pairing", off by default. When ticked, the device key is rotated after the vault key and every phone and iPad must re-pair. |
