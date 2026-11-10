# 188 as built: W-NET (Network category)

Branch `feat/188-net`, base `d02f4959`. Scope: `categories/network.tsx`, `Network.tsx`, `Link.tsx`, `Contacts.tsx`, `Communication.tsx`, `lib/contacts.ts`, `Network.test.ts`, `Contacts.test.ts`. `PairDialog`, `UnpairDialog`, `SandboxVms`, `SandboxSheets` and `sandbox-queries` are unchanged.

## Tabs

Every tab except Devices has `visible: hasLocal`. A phone or web client with no local node sees only Devices.

| Tab | Component | Holds |
|---|---|---|
| This machine | `MachineTab` (`Network.tsx`) | `RotationBanner`, then This machine: the editable name (`mesh_rename`), the id with Copy, the LAN and relay chips, "Managing the mesh of …", and "Let nearby machines find this one while pairing". Then a **Mesh** section: Name, Prefer this node as hub, Sessions, Replication log, Rotate vault key. |
| Machines | `MachinesTab` (`Network.tsx`) | Machines in this mesh (Agents can message, Rename, Unpair… with `UnpairDialog`, the quarantine rows), Nearby with Pair… (`usePairFlow`), **Pair with a string** (Show pairing string, QR, Copy, the paste form), **On another computer over SSH** (`OverSsh`, only where `list_nodes` answers). This tab holds the `mesh_nearby_watch` lease: re-armed every 10 s and on reconnect, released on unmount. |
| Devices | `DevicesTab` (`Link.tsx`) | On a node: "Managing Link of …", Devices paired with this mesh, Relay (switch, address, This node, This mesh, the relay paragraph), **Legacy devices** with **Node name** (only when legacy devices exist), then `Reachable` ("Other meshes this app can reach"). On a bare device: Pair with a mesh, then `Reachable` ("Meshes this device can reach"). |
| Contacts | `ContactsTab` (`Contacts.tsx`) | The Contacts list, then **Access** (`ContactAccess`). `?focus=access:<name>` scrolls that contact's Access row into view. |
| Messaging | `MessagingTab` (`Communication.tsx`) | **Messaging**: Let sessions message each other. **A2A** (`A2aSection`): `ConfigFlagRow a2a.enabled` "A2A over HTTP", the listener row, Problem, Reload peers and exports, the HTTP peers that are not contacts, and the exports with their `WaitingRow`s. |
| Sandbox VMs | `SandboxVms` | Unchanged. |

## Removals
- Network › **Agent messaging** (`MessagingStatus`) and `messagingWhy`; nothing else imported them. Its `mesh` wording moved into the Messaging switch's source line (see Decisions).
- Network's collapsed **Advanced** `<details>`: its rows moved, so the block is gone.
- Communication's per-chat parts: "This chat may communicate", the Effective row with Open ancestor, Root scope (scope select, alias, Open parent), the `stray` waiting rows and `<Peers/>`, plus the `session`/`focus` props. `SCOPES`, `routed`, `useSessions`, `Peers`, `Input` and `Select` left the file with them. W-CHAT rebuilds these from C0's version.
- Communication › A2A: the **contact rows** and **"No peers or exports yet · Contacts…"**.
- Access: the **Contacts…** header button.
- The Contacts list and `ContactAccess` no longer render inside Network or Communication.

## Moves
- `Reachable`, `Name` and `EngineLine` moved from `Network.tsx` to `Link.tsx`, which is now the Devices tab.
- Network's `Contacts` and `SandboxVms` renders were dropped: each has its own tab now.
- Link › Relay › Node name moved into Legacy devices.
- The listener row's "Contacts do not need it" sentence is now the `ConfigFlagRow` description, worded as in the brief.

## Retargeted links
- "Choose what X may reach…": `navigate(/settings/contacts?focus=access:<name>)` (was `/settings/communication`).
- The add-contact flow's "Choose what X may reach": `AddContact.onAccess` now gets the ceremony's peer endpoint id. The list maps it to the contact's name through `contact_list` (`Contact.endpoint`), because the ceremony's display name ("Bob") need not equal the stored contact name ("bob"). If the contact is not found yet, it goes to `/settings/contacts` with no focus.
- `ShareDialog`'s Contacts… goes to `/settings/contacts` (was `/settings/network`).
- `toolNote`: "pin its risk on Agents › Tools".

## Decisions
- The tab components are named `MachineTab`, `MachinesTab`, `DevicesTab`, `ContactsTab` and `MessagingTab`. The files keep their names, so no importer outside settings changes. Every export used by `ContactAllowStrip`, `SessionMenu`, the HUD, `VmDesktop`, `peer-identity` and the other importers is unchanged.
- Focus helpers live in `lib/contacts.ts`: `accessFocus(name)` and `focusedContact(focus)`. A name may contain `:`; anything other than `access:<non-empty>` focuses nothing.
- The focus scroll runs when the row mounts or its `focused` turns true. Picking "Choose what X may reach…" twice in a row for the same contact does not scroll again, because the URL does not change. Kept simple on purpose.
- The Mesh section's first row is labelled **Name** (it was "Mesh"), so it does not repeat the section title.
- Sessions and Replication log stay two rows, as today.
- The Messaging switch's source line: `env` → the brief's environment sentence; `command` → "Changed with /communication."; `config` → nothing; `mesh` → "On by default because this machine has paired machines." That text came from the removed `messagingWhy`; before this change, Communication showed "Not configured: off." for `mesh`, which was wrong. `default` → "Not configured: off." as today.
- When `a2a_status.enabled` is false, the listener row reads "Off" with no description.
- Section titles: "Messaging" and "A2A". Kept short; the row "A2A over HTTP" carries the detail.
- Pointer texts that named old tabs now name the new ones: Nearby off ("… on This machine"), the empty Machines list ("… pairing string below"), the pairing-string hint ("Settings › Network › Machines there"), the SSH node's Remove confirm ("Settings › Network › Devices there"), and the bare device's Pair with a mesh text. `Reachable`'s bare empty text is now "No mesh paired yet." because Pair with a mesh sits right above it. Access's empty text: "Add a contact above to choose what it may reach."
- The relay paragraph sits inside the Relay section, so Legacy devices follow the relay.

## Deviations
None from the brief's structure. The A2A contact filter is the existing inline `a.peers.filter(...)`, not new logic. I did not extract a helper. It is tested by rendering `A2aSection` (`Contacts.test.ts`, "188 Messaging › A2A lists only the HTTP peers").

## Hand-offs
- W-SHELL: hides a tab through `visible` and redirects a hidden tab, so a bare device opening `/settings/machine` is sent to `devices`.
- W-CHAT: owns the per-chat rows that left `Communication.tsx`. The old `stray` test ("a held message of this chat whose export has no row still shows") left `Contacts.test.ts` with them.
