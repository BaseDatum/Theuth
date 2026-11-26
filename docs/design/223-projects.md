# 223 — Projects: a first-class entity with its own sidebar view and agent tools

Owner request 2026-11-26: Agents › Tasks becomes Projects; Projects moves out of the Agents area into the left sidebar
with its own view; create/delete projects with a description and metadata; agent tools manage projects.
Recon: /tmp/recon-projects.md (state today: a project is only a string label on sessions, tasks, docs, notebooks,
meetings, schedule files; the replicated `projects` table holds name/icon/color only).

## Data
Extend the replicated `projects` table (crates/agent, new migration with the next number):
`description TEXT NULL`, `status TEXT NOT NULL DEFAULT 'active'` (`active` | `archived`), `created_ms INTEGER NULL`,
`metadata TEXT NULL` (JSON object, string → string). `name` stays the PK; icon/color/updated_at stay. Bump the replicated
schema pin and every test that pins it. A project exists when it has a row; labels found on sessions/tasks/docs with no
row are listed as implicit (`implicit: true`) so old data still shows.

## Wire (agent crate, rpc_sessions trait)
```
Project { name, description: Option<String>, status: String, icon: Option<String>, color: Option<String>,
          created_ms: Option<i64>, metadata: BTreeMap<String,String>, implicit: bool }
projects_list() -> Vec<Project>
project_create { name, description?, icon?, color?, metadata? } -> Project      // error if a row exists
project_update { name, description?, status?, icon?, color?, metadata? } -> Project  // creates the row for an implicit label
project_delete { name } -> ProjectDeleted { sessions: u32, tasks: u32, docs: u32, notebooks: u32, meetings: u32 }
```
- Names go through `project::normalize_project`.
- `project_delete` DETACHES: clears the label on sessions, tasks, docs, notebooks and meetings, then deletes the row; it
  tolerates a failing store and still returns the counts it achieved (log a WARN per failure). Schedule files keep
  their `project:` key (v1). Rename is out of scope.
- `set_project_style` / `list_project_styles` stay working (style lives on the same row).
- Topic/event: publish an event the UI can invalidate on (reuse the existing projects/style change event if there is
  one; otherwise add `ProjectsChanged`).

## Agent tools
`project_list` (read_only), `project_create` (mutating), `project_update` (mutating), `project_delete` (destructive),
args mirroring the RPCs. Update descriptions of task_create/task_inbox/doc create/notebook tools that say "project
name as sessions carry it" to point at project_list. 

## UI
- New top-level route `/projects` (list: icon, name, description, status, counts) and `/projects/$name` (detail:
  description editor, metadata key/value editor, icon & colour via the existing ProjectStyleDialog, status
  archive/unarchive, sections linking to its Tasks board `/tasks?project=…`, notes, chats), `New project` dialog,
  `Delete project` dialog that states it detaches (does not delete) items.
- Sidebar: a "Projects" row after Agents and a rail icon; nav section `projects`; `/tasks*` belongs to the projects
  section, no longer to the Agents group (Agents keeps Agents, Routines, Autoresearch). `/tasks` URLs keep working.
- Titles: the board's title becomes "Projects"-scoped where shown in the Agents band; view catalog, view titles and the
  pinned tests updated. ProjectPicker "New project…" calls `project_create`.
