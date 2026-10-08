# Turso v0.8.2 vs SQLite 3.51.0 — SQL compatibility of theuth's real SQL and database files

Facts only; produced 2026-10-08 13:08–13:38 by the `turso-compat` subagent. theuth checkout: /Users/Sky.Moore/Develop/theuth at commit 9468a54f87ea3b27f4973250683fb2f815ff16f7 (Thu Oct 8 12:44:43 2026 +0100, 7 dirty paths, not touched). Control engine: /usr/bin/sqlite3 3.51.0 (and python 3.14's `sqlite3` module = the system libsqlite3) . Nothing was compiled; no cargo was run; all live ~/.theuth files were only read (`.backup` / `cp`). Machine load during the runs: 45–50 (16 cores).

## 1 CLI and invocation

- Archive: https://github.com/tursodatabase/turso/releases/download/v0.8.2/turso_cli-aarch64-apple-darwin.tar.xz (5,472,972 bytes) → `turso_cli-aarch64-apple-darwin/{tursodb,README.md,CHANGELOG.md,LICENSE.md}`; the binary IS called `tursodb` (19,285,832 bytes). No `com.apple.quarantine` xattr was present (curl download), so nothing to strip. Installed at `compat/bin/turso_cli-aarch64-apple-darwin/tursodb`.
- `tursodb --version` → `Turso 0.8.2`. `tursodb --help` → `Usage: tursodb [OPTIONS] [DATABASE] [SQL]`; options: `-m, --output-mode <list|pretty|line>` (default pretty; there is NO json/csv mode), `-o/--output`, `-q/--quiet`, `-e/--echo`, `-v/--vfs <io_uring|experimental_win_iocp|memory|syscall>`, `--readonly`, `--experimental-views`, `--experimental-custom-types`, `-t/--tracing-output`, `--mcp` (MCP server), `--sync-server <addr>`, `--sync-dir`, `--sync-max-databases`, `--experimental-encryption`, `--experimental-index-method`, `--experimental-autovacuum`, `--experimental-vacuum`, `--experimental-attach`, `--experimental-generated-columns`, `--experimental-without-rowid`, `--experimental-multiprocess-wal`, `--experimental-mvcc-passive-checkpoint`, `--unsafe-testing`. No Postgres/pg-wire flag on this binary.
- Dot-commands (cli/commands/mod.rs): .exit .quit .open .schema .output .mode .opcodes .cd .show .nullvalue .echo .tables .databases .import .load .dump .dbconfig .stats .vfslist .indexes .timer .headers .clone .manual .read .parameter .dbtotxt. There is no `-f`/`--file` flag; a script runs via stdin redirect, `.read FILE`, or the positional `[SQL]` argument.
- Observed behaviour (probe script compat/logs/probe1.sh, /tmp/turso_probe*.sql):
  - stdin: `tursodb -q -m list db.sqlite < script.sql` executes statement by statement; after an error it CONTINUES with the next line-separated statement; exit status is 1 if any statement failed, 0 otherwise. Errors are printed to STDOUT (not stderr), as `  × Parse error: no such function: nosuchfunc` followed by a blank line; multi-line errors continue with `│ …` lines; some runtime errors print as `Runtime error: datatype mismatch (19)` without the glyph. Only engine WARN logs go to stderr (e.g. the fts5 warning below).
  - positional `[SQL]`: `tursodb -q -m list :memory: "SELECT 1; SELECT nosuchfunc(2); SELECT 3;"` prints `1`, the error, and STOPS — `3` is never printed (an error aborts the remainder of the same input chunk; the same holds for several statements on one stdin line).
  - `.read /tmp/turso_probe.sql` behaves like stdin. `.headers on`, `.nullvalue NULL` work in list mode; NULL prints as empty by default (same as sqlite3).
  - `.timer on` prints per statement: `Command stats: total: 801 us (this includes parsing/coloring of cli app)` + `query execution stats: Execution: avg=694 us, total=694 us` + `I/O: No samples available`. Not used for the measurements below (wall clock around the process was used for both engines instead).
  - `EXPLAIN QUERY PLAN <stmt>` and `EXPLAIN <stmt>` both work (list mode rows `1|0|0|SCAN t`); EXPLAIN QUERY PLAN resolves tables, columns and functions (`× Parse error: no such function: nosuchfunc`, `× Parse error: no such column: b`), so it is a usable prepare-time check.
  - Opening ANY file (even for a read) creates `<db>-wal` (0 bytes) and `<db>-shm` next to it; `--readonly` does not create them. Opening a C-SQLite file whose schema contains an FTS5 table prints to stderr: `2026-10-08T12:26:40.159760Z  WARN ThreadId(01) turso_core::database: 1643: open warning, failed to load extension: Virtual table module not found: fts5`.
- Invocation settled on for every replay (compat/runner.py): one process per (engine, database, statement list), script on stdin, every statement preceded by a marker `SELECT '@@S<n>';` so output and inline errors are attributed per statement; Turso: `tursodb -q -m list <db>`; control: `/usr/bin/sqlite3 -batch <db>` (errors on stderr carry `near line N`, mapped back to the statement by line ranges). Heavy files were run under `nice -n 10`.

## 2 Corpus statistics (compat/extract.py → compat/corpus/)

- Migrations: 73 files under crates/*/migrations copied unchanged to compat/corpus/migrations/<crate>/ — per crate: agent 40, artifact 3, double 1, link 1, meeting 2, mesh 5, replica 2, roster 6, schedule 3, secrets 2, tasks 8; 194 statements (split with sqlite3.complete_statement): CREATE TABLE 83, ALTER TABLE 58, CREATE INDEX 40, UPDATE 4, INSERT 4, CREATE TRIGGER 3, DROP TABLE 2.
- Rust scan: 663 .rs files under crates/ (target/, vendor/ skipped); files mentioning sqlx/rusqlite/libsqlite3 were lexed (strings with escapes and line continuations, raw strings r#"…"#, comments) and every call of sqlx::query / query_as / query_scalar / query! / query_as! / query_scalar! / raw_sql / QueryBuilder::new / .execute / .execute_batch / .prepare / .prepare_cached / .query_row was inspected. 356 `.execute(`/`.prepare(` calls whose first argument is not SQL (sqlx executors, pools) were skipped.
- Sites: **748** total = **625 static** (literal, `const`, `let`, `include_str!`, arrays of literals iterated at the call site, `if c {A} else {B}` consts) + **123 dynamic** (not replayed). Static sites contain 626 statements. Dynamic reasons: AssertSqlSafe(format!/concat!) 57, format!/concat! 32, QueryBuilder (dynamic) 16, AssertSqlSafe(ident sql 8, first arg not a literal 3, AssertSqlSafe(ident unresolved 3, ident sql 2, AssertSqlSafe(ident update 1, AssertSqlSafe(ident insert 1.
- Per crate (total / static): agent 314/267, codeindex 108/84, replica 70/48, double 42/40, tasks 32/24, knowledge 24/20, document 23/23, roster 23/22, meeting 21/21, secrets 19/18, artifact 17/9, schedule 17/17, mesh 13/13, sessionsync 12/6, link 8/8, dreaming 5/5.
- Per API (total / static): sqlx::query 425/390, sqlx::query_as 247/177, sqlx::query_scalar 42/41, QueryBuilder::new 16/0, .execute 12/12, sqlx::raw_sql 2/1, .execute_batch 2/2, .prepare 2/2. (sqlx 0.9's `AssertSqlSafe(format!(…))` wrapper marks 57 of the dynamic sites; `QueryBuilder` 16; plain `format!` 32.)
- Parameter styles of static sites: qmark 382, none 182, qmark_n 60, qmark,qmark_n 1 (no `$N`, `:name` or `@name` anywhere).
- Static statements by leading keyword: SELECT 251, UPDATE 124, INSERT 102, DELETE 85, CREATE TABLE 19, INSERT OR IGNORE 15, CREATE INDEX 11, INSERT OR REPLACE 7, WITH 4, PRAGMA 3, CREATE TRIGGER 1, DROP TRIGGER 1, DROP INDEX 1, ALTER TABLE 1, DROP TABLE 1.

Feature histogram over the 626 static statements (regex over the SQL with strings/comments blanked; one count per statement per feature):

| feature | statements |
|---|---|
| ? placeholder style | 443 |
| ORDER BY | 100 |
| total()/sum()/avg()/min()/max()/count() | 71 |
| IS NOT / IS | 58 |
| ON CONFLICT | 54 |
| UPSERT DO UPDATE SET | 53 |
| excluded. | 47 |
| coalesce/ifnull/nullif | 36 |
| JOIN | 34 |
| LIMIT | 28 |
| IF NOT EXISTS | 26 |
| OR REPLACE / OR IGNORE | 22 |
| INSERT ... SELECT | 16 |
| GROUP BY | 14 |
| IN (subquery) | 12 |
| LEFT JOIN | 11 |
| CREATE INDEX | 11 |
| LIKE / GLOB | 11 |
| UPDATE ... FROM | 10 |
| RETURNING | 9 |
| UNION | 8 |
| rowid | 6 |
| \|\| concat | 5 |
| CASE | 5 |
| fts5 MATCH | 5 |
| DISTINCT | 5 |
| EXISTS | 4 |
| json_*() | 4 |
| WITH RECURSIVE | 4 |
| WITH (CTE) | 4 |
| FOREIGN KEY / REFERENCES | 4 |
| ON DELETE CASCADE | 4 |
| PRAGMA | 3 |
| length/substr/lower/upper/trim/replace | 3 |
| DROP TABLE/INDEX | 3 |
| CREATE VIRTUAL TABLE | 3 |
| fts5 | 3 |
| TEMP table | 3 |
| json_each/json_tree | 3 |
| CAST | 2 |
| bm25()/highlight()/snippet() | 2 |
| pragma_* table-valued | 1 |
| CREATE TRIGGER | 1 |
| strftime | 1 |
| IIF | 1 |
| ESCAPE | 1 |
| ALTER TABLE ADD COLUMN | 1 |
| sqlite_master | 1 |
| IF EXISTS | 1 |
| DO NOTHING | 1 |
| AUTOINCREMENT | 1 |

Feature histogram over the 194 migration statements:

| feature | statements |
|---|---|
| ALTER TABLE ADD COLUMN | 56 |
| CREATE INDEX | 40 |
| FOREIGN KEY / REFERENCES | 30 |
| CHECK ( | 6 |
| ON DELETE CASCADE | 6 |
| IS NOT / IS | 5 |
| INSERT ... SELECT | 3 |
| \|\| concat | 3 |
| CREATE TRIGGER | 3 |
| BEGIN/COMMIT/ROLLBACK | 3 |
| AUTOINCREMENT | 2 |
| DROP TABLE/INDEX | 2 |
| ALTER TABLE RENAME TO | 2 |
| UPDATE ... FROM | 2 |
| CREATE VIRTUAL TABLE | 2 |
| fts5 | 2 |
| IF NOT EXISTS | 2 |
| WITH RECURSIVE | 1 |
| WITH (CTE) | 1 |
| UNION | 1 |
| JOIN | 1 |
| printf/format() | 1 |
| random()/randomblob() | 1 |
| hex()/unhex() | 1 |
| length/substr/lower/upper/trim/replace | 1 |
| strftime | 1 |
| EXISTS | 1 |

Not present anywhere in theuth's SQL: window functions (`OVER (`), `FILTER (`, `WITHOUT ROWID`, `STRICT`, `GENERATED`, `ATTACH`, `SAVEPOINT`, `VACUUM`, `ALTER TABLE … DROP/RENAME COLUMN`, `COLLATE`, `group_concat`, `datetime()`, `printf`/`format()` in queries (one `printf`/`hex`/`random` in a migration), `last_insert_rowid()`. Present and relevant: UPSERT (`ON CONFLICT … DO UPDATE SET … excluded.`) 54, `RETURNING` 9, `WITH RECURSIVE` 4, `json_each`/`json_*` 4, `IIF` 1, `fts5 MATCH` 3 + `bm25()` 2, `pragma_table_info()` 1, `sqlite_master` 1, `CREATE TEMP TABLE` 3, `AS MATERIALIZED` 1.

## 3 Migration replay per crate (compat/replay_migrations.py → compat/results/migrations.tsv, migrations_summary.json, schema/*.txt)

Mode A = statement by statement (the sqlx bookkeeping statements included, no transaction wrapper, so one failure cannot hide later ones). Mode B = file by file exactly as sqlx-sqlite 0.9.0 sends it: `BEGIN; <whole file>; INSERT INTO _sqlx_migrations …; COMMIT;` then `UPDATE _sqlx_migrations SET execution_time = ?1 WHERE version = ?2`. sqlx's table (quoted from ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sqlx-sqlite-0.9.0/src/migrate.rs lines 91–105):

```sql
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
);
```
plus `SELECT version FROM _sqlx_migrations WHERE success = false ORDER BY version LIMIT 1`, `SELECT version, checksum FROM _sqlx_migrations ORDER BY version`, `INSERT INTO _sqlx_migrations ( version, description, success, checksum, execution_time ) VALUES ( ?1, ?2, TRUE, ?3, -1 )` — all of these passed on Turso for every crate.

| crate | files | migration stmts | stmts incl. sqlx bookkeeping | Turso pass | Turso fail | control-failed | mode B files pass | sqlite_master objects sqlite3 / Turso | objects only in sqlite3 | objects only in Turso |
|---|---|---|---|---|---|---|---|---|---|---|
| agent | 40 | 113 | 196 | 195 | 1 | 0 | 39/40 | 102 / 98 | endless_lines_fts, endless_lines_fts_config, endless_lines_fts_content, endless_lines_fts_data, endless_lines_fts_docsize, endless_lines_fts_idx | __turso_internal_seq___turso_internal_autoincrement_a2a_tasks, __turso_internal_seq___turso_internal_autoincrement_peer_exchanges |
| artifact | 3 | 5 | 14 | 14 | 0 | 0 | 3/3 | 8 / 8 | — | — |
| double | 1 | 11 | 16 | 16 | 0 | 0 | 1/1 | 19 / 19 | — | — |
| link | 1 | 2 | 7 | 7 | 0 | 0 | 1/1 | 6 / 6 | — | — |
| meeting | 2 | 2 | 9 | 8 | 1 | 0 | 1/2 | 10 / 4 | segments, segments_config, segments_content, segments_data, segments_docsize, segments_idx | — |
| mesh | 5 | 16 | 29 | 29 | 0 | 0 | 5/5 | 13 / 13 | — | — |
| replica | 2 | 6 | 13 | 13 | 0 | 0 | 2/2 | 12 / 12 | — | — |
| roster | 6 | 11 | 26 | 26 | 0 | 0 | 6/6 | 22 / 22 | — | — |
| schedule | 3 | 5 | 14 | 14 | 0 | 0 | 3/3 | 6 / 6 | — | — |
| secrets | 2 | 4 | 11 | 11 | 0 | 0 | 2/2 | 11 / 11 | — | — |
| tasks | 8 | 19 | 38 | 38 | 0 | 0 | 8/8 | 23 / 23 | — | — |

Distinct Turso error texts in the migration replay (mode A and mode B agree; the cited line is where the statement starts in the file, including its leading comment):

- `Parse error: no such module: fts5` — 2 statements: crates/agent/migrations/0037_endless.sql:50 (`-- 137 D9: `recall` — FTS5 over the level-0 lines, kept by triggers. CREATE VIRTUAL TABLE …`); crates/meeting/migrations/0002_segments.sql:1 (`-- Transcript segments for `meeting_search` (04 §5): re-indexed whole per meeting -- on ev…`)

The two failing statements are the only `CREATE VIRTUAL TABLE … USING fts5` in the migrations: crates/agent/migrations/0037_endless.sql:50 (`CREATE VIRTUAL TABLE endless_lines_fts USING fts5(text, session_id UNINDEXED, …)`) and crates/meeting/migrations/0002_segments.sql (`segments`). The triggers that maintain `endless_lines_fts` (same file) were CREATED without error on Turso (trigger bodies are not resolved at creation time in either engine). In mode B the whole migration file 0037_endless.sql (agent) and 0002_segments.sql (meeting) fail as a unit, i.e. sqlx's migrator would stop there with `MigrateError::ExecuteMigration` and the crate's store would not open.

Addendum (found while checking the meeting control failures): there is one NESTED migration directory the `crates/*/migrations/*.sql` glob does not cover — crates/meeting/migrations/briefs/0001_briefs.sql (its own sqlx migrator for `briefs.sqlite`, 2 statements). Replayed by hand: `CREATE TABLE briefs (…)` passes on Turso; `CREATE VIRTUAL TABLE briefs_fts USING fts5(id UNINDEXED, aliases, title, tags, bullets, tokenize = 'porter unicode61 remove_diacritics 2')` → `× Parse error: no such module: fts5` (exit 1; sqlite3 exit 0). So the migration total is 75 files / 196 statements, of which 3 fail on Turso, all FTS5. The 13 `control-failed` meeting statements in §4 are the `briefs`/`briefs_fts` queries that ran against the meeting database instead of briefs.sqlite (no ~/.theuth/briefs.sqlite exists yet on this machine); on a briefs database they would split into `briefs` statements that pass and `briefs_fts` statements that fail like the other FTS5 users.

Schema text comparison (`SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY 1,2` read with both engines; compat/schema_diff.py → results/schema_diff.json). Objects present in both engines have the same names; the stored `sql` text differs in form: Turso re-renders CREATE statements on one line, drops SQL comments, drops the `NULL` keyword of nullable columns, moves inline `REFERENCES …` to table-level `FOREIGN KEY (col) REFERENCES …` clauses (ON DELETE CASCADE preserved), keeps `IF NOT EXISTS`, quotes some identifiers (`"key"`). Classes per crate:

- agent: identical text 49; equivalent (identifiers re-quoted) 2; equivalent (whitespace only) 27; equivalent (IF NOT EXISTS kept) 2; DIFFERENT after normalisation 10; equivalent (comments dropped, identifiers re-quoted) 1; equivalent (comments dropped) 5 — the DIFFERENT ones are all `REFERENCES`→`FOREIGN KEY` relocation and/or `NULL` dropped: cost_owners, epochs, jobs, messages, peer_exchanges, projects, questions, remote_turns, resume_reports, sessions
- artifact: equivalent (whitespace only) 5; identical text 3
- double: identical text 11; equivalent (whitespace only) 2; equivalent (comments dropped, identifiers re-quoted) 2; equivalent (comments dropped) 4
- link: identical text 3; equivalent (whitespace only) 1; equivalent (comments dropped) 2
- meeting: identical text 2; equivalent (whitespace only) 2
- mesh: identical text 7; equivalent (whitespace only) 1; equivalent (comments dropped) 4; equivalent (identifiers re-quoted) 1
- replica: identical text 5; equivalent (whitespace only) 7
- roster: identical text 12; equivalent (whitespace only) 1; equivalent (comments dropped) 9
- schedule: identical text 3; equivalent (whitespace only) 2; equivalent (comments dropped) 1
- secrets: identical text 6; equivalent (whitespace only) 4; equivalent (comments dropped) 1
- tasks: identical text 13; equivalent (whitespace only) 4; equivalent (comments dropped) 3; equivalent (comments dropped, identifiers re-quoted) 2; equivalent (identifiers re-quoted) 1

Objects only in Turso: `__turso_internal_seq___turso_internal_autoincrement_<table>` (one per AUTOINCREMENT table, agent: a2a_tasks, peer_exchanges); C SQLite keeps those counters in `sqlite_sequence` instead (present in both). Objects only in sqlite3: the FTS5 table and its five shadow tables. C SQLite (python sqlite3) can open every Turso-written database file + its `-wal` and sees exactly the same objects as the Turso CLI (`csqlite_vs_tursocli_object_mismatch` = [] for all 11 crates).

## 4 Query replay (compat/replay_queries.py → compat/results/queries.tsv, queries_summary.json)

Method: every static statement (626) with every placeholder outside quotes replaced by `NULL`, run against the mode-A migrated database of its crate (crates without migrations: codeindex, document, knowledge got a fresh database bootstrapped from their own static `CREATE …` statements; sessionsync ran on the agent database; dreaming's statements ran on knowledge or agent by table name; replica's statements that target other crates' tables fail in the control too). Order: all `CREATE` first (for real), then `PRAGMA`/`SELECT`/`WITH` (EXPLAIN QUERY PLAN, then executed for real), then `INSERT`/`UPDATE`/`DELETE` and finally `DROP`/`ALTER` (EXPLAIN QUERY PLAN, then `BEGIN; <stmt>; ROLLBACK;`). EXPLAIN QUERY PLAN worked for every statement on Turso; `EXPLAIN` fallback was never needed. Classes: `pass`; `constraint-noise` = the real execution failed with a constraint/datatype error on BOTH engines because of the NULL parameters (74 `NOT NULL constraint failed`, 11 `datatype mismatch` = `LIMIT NULL`); `control-failed` = sqlite3 itself failed at prepare (table of another crate/test fixture); `fail-unsupported`/`fail-missing-object`/`fail-function`/`fail-parse` = Turso failed where sqlite3 passed.

| class | statements |
|---|---|
| pass | 498 |
| constraint-noise | 84 |
| control-failed | 20 |
| fail-missing-object | 20 |
| fail-unsupported | 3 |
| constraint-noise (sqlite3 passed) | 1 |

| crate | pass | constraint-noise | fail-unsupported | fail-missing-object | control-failed | other |
|---|---|---|---|---|---|---|
| agent | 230 | 31 | 0 | 6 | 1 |  |
| artifact | 9 | 0 | 0 | 0 | 0 |  |
| codeindex | 72 | 7 | 2 | 3 | 0 |  |
| document | 15 | 7 | 0 | 0 | 1 |  |
| double | 31 | 8 | 0 | 0 | 0 | constraint-noise (sqlite3 passed) 1 |
| dreaming | 5 | 0 | 0 | 0 | 0 |  |
| knowledge | 12 | 0 | 1 | 7 | 0 |  |
| link | 8 | 0 | 0 | 0 | 0 |  |
| meeting | 2 | 2 | 0 | 4 | 13 |  |
| mesh | 9 | 4 | 0 | 0 | 0 |  |
| replica | 40 | 3 | 0 | 0 | 5 |  |
| roster | 15 | 7 | 0 | 0 | 0 |  |
| schedule | 14 | 3 | 0 | 0 | 0 |  |
| secrets | 14 | 4 | 0 | 0 | 0 |  |
| sessionsync | 4 | 2 | 0 | 0 | 0 |  |
| tasks | 18 | 6 | 0 | 0 | 0 |  |

Gap table (every Turso failure on a statement that sqlite3 accepted):

| class | error (normalised) | statements | crates | examples (path:line) | exact Turso error |
|---|---|---|---|---|---|
| fail-missing-object | Parse error: no such table: <name> | 20 | agent, codeindex, knowledge, meeting | crates/agent/src/store/endless.rs:384; crates/agent/src/store/endless.rs:456 | Parse error: no such table: endless_lines_fts |
| fail-unsupported | Parse error: no such module: <name> | 3 | codeindex, knowledge | crates/codeindex/src/store.rs:149; crates/codeindex/src/store.rs:149 | Parse error: no such module: fts5 |
| constraint-noise (sqlite3 passed) | Runtime error: datatype mismatch (N) | 1 | double | crates/double/src/predict.rs:489 | Runtime error: datatype mismatch (19) |

`fail-missing-object` breakdown: `Parse error: no such table: concepts` ×7, `Parse error: no such table: endless_lines_fts` ×6, `Parse error: no such table: segments` ×4, `Parse error: no such table: symbols_tri` ×2, `Parse error: no such table: symbols_fts` ×1 — all 20 are statements that read or write an FTS5 table that could not be created (`concepts` = knowledge's whole concept store is an FTS5 table; `endless_lines_fts` agent 0037; `segments` meeting; `symbols_fts`/`symbols_tri` codeindex). No other statement class failed: UPSERT/`excluded.`, `RETURNING`, `WITH RECURSIVE`, `json_each`/`json_extract`, `IIF`, `CASE`, `CAST`, `LIKE … ESCAPE`, `pragma_table_info()`, `CREATE TEMP TABLE`, `AS MATERIALIZED`, `UNION`, `EXISTS`, `IN (subquery)`, `UPDATE … FROM`, `INSERT … SELECT`, `COALESCE`, `substr/length/lower`, `PRAGMA temp_store/journal_mode/synchronous`, `CREATE TRIGGER … RAISE(ABORT)` all prepared and executed on Turso.

Single semantic difference seen (counted as noise): crates/double/src/predict.rs:489 — `… AND e.id IN (SELECT id FROM examples WHERE class = ? ORDER BY asked_at DESC LIMIT ?)` with `LIMIT NULL` → sqlite3 returns no rows without error (outer `e.class = NULL` short-circuits), Turso: `Runtime error: datatype mismatch (19)`. Top-level `SELECT … LIMIT NULL` errors with `datatype mismatch` on BOTH engines (sqlite3: `(20)`, Turso: `(19)`).

Control failures (sqlite3 also failed, so not Turso's): crates/agent/src/store.rs:6020 `Parse error: index sessions_desk_owner already exists`; crates/document/src/store.rs:159 `Parse error: duplicate column name: hlc`; crates/meeting/src/cache.rs:160 `Parse error: no such table: briefs`; crates/meeting/src/cache.rs:180 `Parse error: no such table: briefs`; crates/meeting/src/cache.rs:194 `Parse error: no such table: briefs`; crates/meeting/src/cache.rs:211 `Parse error: no such table: briefs`; crates/meeting/src/cache.rs:234 `Parse error: no such table: briefs_fts`; crates/meeting/src/cache.rs:295 `Parse error: no such table: briefs`; crates/meeting/src/cache.rs:300 `Parse error: no such table: briefs`; crates/meeting/src/cache.rs:331 `Parse error: no such table: briefs_fts`; crates/meeting/src/cache.rs:336 `Parse error: no such table: briefs_fts`; crates/meeting/src/cache.rs:353 `Parse error: no such table: briefs`; crates/meeting/src/cache.rs:362 `Parse error: no such table: briefs_fts`; crates/meeting/src/cache.rs:374 `Parse error: no such table: briefs`; crates/meeting/src/cache.rs:395 `Parse error: no such table: briefs`; crates/replica/src/files.rs:495 `Parse error: no such table: agents`; crates/replica/src/tests.rs:91 `Parse error: no such column: n`; crates/replica/tests/sync_loopback.rs:319 `Parse error: no such table: owners`; crates/replica/tests/sync_loopback.rs:335 `Parse error: no such table: owners`; crates/replica/tests/sync_loopback.rs:451 `Parse error: no such table: notes`.

## 5 Real database files (compat/s5_backup.sh, compat/s5_compare.py → compat/results/s5_summary.json, results/s5/*.txt)

Copies: `nice -n 10 /usr/bin/sqlite3 ~/.theuth/<f>.sqlite ".backup compat/data/<f>.sqlite"` — tasks 0.135 s (1,298,432 B), roster 0.065 s (245,760), knowledge 0.177 s (13,709,312), replica 0.432 s (70,283,264), artifacts 0.071 s (135,168), schedule 0.036 s (970,752), codeindex (`~/.theuth/codeindex/Users-Sky.Moore-Develop-theuth-6063174e.sqlite`) 0.267 s (49,197,056), **sessions 4.566 s (1,012,084,736 B)**. The live WAL trio `cp` (sessions.sqlite + -wal 9,422,472 B + -shm) took 0.707 s. Every copy: `PRAGMA journal_mode` = wal and `PRAGMA page_size` = 4096 on both engines.

| file | bytes | tables | Turso opened | table counts equal | integrity_check sqlite3 | integrity_check Turso | tables Turso cannot read (`no such table`) |
|---|---|---|---|---|---|---|---|
| tasks.sqlite | 1,298,432 | 11 | yes (rc 0) | 11 of 11 | ok (0.020 s) | ok (0.022 s) | — |
| roster.sqlite | 245,760 | 12 | yes (rc 0) | 12 of 12 | ok (0.015 s) | ok (0.018 s) | — |
| knowledge.sqlite | 13,709,312 | 7 | yes (rc 1) | 1 of 7 | ok (0.178 s) | *** in database main *** Page 2: never u (0.014 s) | concepts; concepts_config; concepts_content; concepts_data; concepts_docsize; concepts_idx |
| replica.sqlite | 70,283,264 | 7 | yes (rc 0) | 7 of 7 | ok (0.293 s) | ok (0.169 s) | — |
| artifacts.sqlite | 135,168 | 3 | yes (rc 0) | 3 of 3 | ok (0.017 s) | ok (0.018 s) | — |
| schedule.sqlite | 970,752 | 6 | yes (rc 0) | 6 of 6 | ok (0.018 s) | ok (0.027 s) | — |
| codeindex-theuth.sqlite | 49,197,056 | 18 | yes (rc 1) | 3 of 18 | ok (0.922 s) | *** in database main *** Page 3: never u (0.045 s) | cochanges; imports; refs; symbols_fts; symbols_fts_config; symbols_fts_content; symbols_fts_data; symbols_fts_docsize; symbols_fts_idx; symbols_tri; symbols_tri_config; symbols_tri_content; symbols_tri_data; symbols_tri_docsize; symbols_tri_idx |
| sessions.sqlite | 1,012,084,736 | 31 | yes (rc 0) | 31 of 31 | ok (3.871 s) | ok (1.747 s) | — |

Turso never modified a main file on open/read (`main_file_changed` false for all); it created `<f>-wal` (0 B) and `<f>-shm` (32 KiB) beside every copy. For knowledge.sqlite and the codeindex file Turso's `PRAGMA integrity_check` returns `*** in database main ***` + `Page N: never used` for every page of the invisible FTS tables (101 lines for knowledge) while sqlite3 says `ok` — a false positive caused by the same schema-loading stop, not corruption.

Why whole non-FTS tables are unreadable: Turso loads the schema in `sqlite_master` rowid order and stops at the first `CREATE VIRTUAL TABLE … USING fts5`; everything created after it is `no such table` even though `SELECT name FROM sqlite_master` still lists all 29 objects of the codeindex file. Minimal reproduction: `printf 'CREATE TABLE a(x);\nCREATE VIRTUAL TABLE f USING fts5(t);\nCREATE TABLE b(y);\n' | sqlite3 x.db; tursodb -q -m list x.db 'SELECT count(*) FROM a'` → `1`; `… 'SELECT count(*) FROM b'` → `× Parse error: no such table: b`. In the codeindex file this hides `imports` (19,817 rows), `refs` (244,795) and `cochanges` (6,436); in knowledge.sqlite only `links` (created before `concepts`) is readable; `uses` does not exist in the live file yet.

Timed queries (wall clock around one CLI process incl. start-up and file open, best of 3, both engines under `nice -n 10`; outputs written to results/s5/ and compared byte for byte):

| file | query | sqlite3 s | Turso s | rows sqlite3 | rows Turso | output |
|---|---|---|---|---|---|---|
| tasks.sqlite | T1 activity feed (activity.rs:364) | 0.015 | 0.018 | 200 | 200 | identical |
| tasks.sqlite | T2 one task's events (store.rs:319) | 0.014 | 0.015 | 95 | 95 | identical |
| knowledge.sqlite | K1 concepts inventory (store.rs:645) | 0.017 | 0.013 | 1768 | 2 | Turso: × Parse error: no such table: concepts |
| knowledge.sqlite | K2 links with EXISTS over concepts (store.rs:747) | 0.022 | 0.013 | 9 | 2 | Turso: × Parse error: no such table: concepts |
| knowledge.sqlite | K3 links only (no fts table) | 0.011 | 0.016 | 20 | 20 | identical |
| sessions.sqlite | Q1 latest 50 sessions (store.rs:1068 shape) | 0.023 | 0.028 | 50 | 50 | identical |
| sessions.sqlite | Q2 all messages of the largest session (store.rs:2960, params bound) | 0.082 | 0.085 | 4245 | 4245 | identical |
| sessions.sqlite | Q3 model_calls per day (ad hoc; accounting.rs:460 functions) | 0.059 | 0.067 | 18 | 18 | identical |
| sessions.sqlite | Q4 LIKE over message bodies (endless.rs:411 shape) | 0.937 | 3.314 | 11 | 11 | identical |
| sessions.sqlite | Q5 sessions joined to their last message (ad hoc) | 0.363 | 0.411 | 50 | 50 | identical |

Exact SQL of the timed queries:

- T1 activity feed (activity.rs:364) — `SELECT e.id, e.task, e.at_ms, e.actor, e.session, e.field, e.before, e.after, t.title, t.parent FROM task_events e LEFT JOIN tasks t ON t.id = e.task WHERE e.id > 0 ORDER BY e.id LIMIT 200;`
- T2 one task's events (store.rs:319) — `SELECT at_ms, actor, session, field, before, after FROM task_events WHERE task = '01a0e8ba-d1f8-7a22-95e5-66619229e929' ORDER BY id;`
- K1 concepts inventory (store.rs:645) — `SELECT id, path, mtime, size FROM concepts;`
- K2 links with EXISTS over concepts (store.rs:747) — `SELECT l.dst, EXISTS(SELECT 1 FROM concepts c WHERE c.id = l.dst) FROM links l WHERE l.src = 'procedures/dr-desk-code-change-in-worktree-sre-argo' ORDER BY l.dst;`
- K3 links only (no fts table) — `SELECT src, count(*) FROM links GROUP BY src ORDER BY 2 DESC, 1 LIMIT 20;`
- Q1 latest 50 sessions (store.rs:1068 shape) — `SELECT id, title, created_at, updated_at FROM sessions ORDER BY updated_at DESC, id DESC LIMIT 50;`
- Q2 all messages of the largest session (store.rs:2960, params bound) — `SELECT seq, body, excluded, vision, at, thinking_ms, advisory FROM messages WHERE session_id = '01a0d418-9689-7201-bb54-b3c954edb5f6' AND seq >= 0 AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = '01a0d418-9689-7201-bb54-b3c954edb5f6'), 9223372036854775807) ORDER BY seq;`
- Q3 model_calls per day (ad hoc; accounting.rs:460 functions) — `SELECT substr(created_at, 1, 10) AS day, count(*), COALESCE(SUM(estimated_nanos), 0), COALESCE(SUM(IIF(status = 'failed', 1, 0)), 0) FROM model_calls GROUP BY day ORDER BY day;`
- Q4 LIKE over message bodies (endless.rs:411 shape) — `SELECT session_id, seq, length(body) FROM messages WHERE body LIKE '%datatype mismatch%' ESCAPE '\' ORDER BY session_id, seq LIMIT 100;`
- Q5 sessions joined to their last message (ad hoc) — `SELECT s.id, s.updated_at, m.seq, length(m.body) FROM sessions s JOIN messages m ON m.session_id = s.id AND m.seq = (SELECT max(seq) FROM messages WHERE session_id = s.id) ORDER BY s.updated_at DESC LIMIT 50;`

## 6 WAL trio

Run 1 (live trio, `cp` of ~/.theuth/sessions.sqlite + -wal + -shm at 13:25, WAL 9,422,472 B): Turso rw → `191217 805 4244 wal`; Turso `--readonly` → `191217 805 4244 wal`; sqlite3 `-readonly` → `191217 805 4244 wal` (messages, sessions, max seq of the largest session, journal_mode). The `.backup` taken ~1 s later holds 191,221 messages (4 more rows written by the live engine in between). The main file alone (no WAL) also gives 191,217 on both engines, i.e. the copied WAL carried no frames that were not already checkpointed — this run cannot distinguish "read the WAL" from "ignored the WAL". Turso rw TRUNCATED the copied `-wal` to 0 bytes on close (main file size unchanged); `--readonly` left it alone. Run 2 (13:28, fresh trio copy, WAL 4,486,712 B, 27 frames with the current salt): identical counts (191,435 / 806) from the main file alone, sqlite3 -readonly, Turso --readonly and Turso rw — again not decisive.

Run 3, decisive (compat/s5_wal_decisive.py → results/s5_wal_trio_decisive.json): on a copy of the 1 GB `.backup`, C SQLite (python's sqlite3 module) set `PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0`, inserted 3,000 rows (`session_id='wal-test-session'`, ~1 KB bodies) in one transaction and `os._exit`ed without closing → WAL header magic 0x377f0682, version 3007000, checkpoint_seq 0, 4,276,592 B = 1,038 uncheckpointed frames. Then: main file alone (sqlite3) → messages 191221, wal-test rows 0; trio duplicate with sqlite3 `-readonly` → 194221 / 3000; **trio with Turso `--readonly` → 194221 / 3000; Turso rw → 194221 / 3000**; sqlite3 after Turso → 194221 / 3000. Turso reads a C-SQLite WAL with uncheckpointed frames correctly. In rw mode Turso checkpointed and truncated on close: main file 1,012,084,736 → 1,016,283,136 B, `-wal` → 0 B, and sqlite3 then reads the checkpointed file with the same counts. Reverse direction: Turso created a table and inserted 3 rows into a copy of tasks.sqlite (`PRAGMA journal_mode` → wal); sqlite3 then read `3`, `PRAGMA integrity_check` → `ok`, `tasks` count 80.

## 6b Postgres note (S6)

From the clone's README.md (lines 7, 37, 63, 443–449): Turso is "compatible with SQLite, and now with a Postgres frontend too"; "Postgres is a newer and still experimental frontend that speaks the Postgres dialect and wire protocol" sharing storage, concurrency, compiler and VM; feature status is in postgres/COMPAT.md, which says the frontend parses with `pg_query` (libpg_query, the real PostgreSQL grammar) and support is decided by the translator `postgres/parser/translator.rs` plus what the engine executes; components `postgres/parser`, `postgres/frontend` (pg_catalog emulation, COPY, schemas, session handling), `postgres/server` (crate `turso_pg_server`), `postgres/cli` (binary `tursopg`), `postgres/client`, `postgres/regress`, `postgres/tests`. The downloaded `tursodb` 0.8.2 binary exposes no Postgres option (`--help` above; its network modes are `--mcp` and `--sync-server`), so serving the Postgres wire protocol would mean building and running the separate `turso_pg_server`/`tursopg` crates from the workspace, which this spike did not do (no cargo). For theuth, whose SQL is SQLite dialect through sqlx-sqlite, the Postgres frontend is an additional dialect over the same files, not a replacement for the SQLite API.

## 7 Surprises and exact error texts

1. FTS5 is the one hard gap, and it is wider than "no full-text search": `× Parse error: no such module: fts5` for every `CREATE VIRTUAL TABLE … USING fts5` (3 in code: knowledge `concepts`, codeindex `symbols_fts` + `symbols_tri`; 3 in migrations: agent 0037 `endless_lines_fts`, meeting 0002 `segments`, meeting/briefs 0001 `briefs_fts` — six FTS5 tables across five crates: agent, meeting, knowledge, codeindex, and the brief cache); plus `× Parse error: no such table: <name>` for the 20 statements touching them; plus, when Turso opens an EXISTING C-SQLite file that contains an FTS5 table, every table created after it in `sqlite_master` order is invisible (`no such table: imports/refs/cochanges` in the codeindex file) and `PRAGMA integrity_check` reports the FTS pages as `Page N: never used`. turso/COMPAT.md §Full-Text Search: `Turso implements FTS using Tantivy instead of SQLite's FTS3/FTS4/FTS5` — `CREATE INDEX ... USING fts` (Turso-specific syntax), `fts_match()`, `fts_score()`, `fts_highlight()`; row `SQLite FTS3/FTS4/FTS5 | ❌ No | Use Turso FTS instead`. The CLI's `.load` cannot help: there is no fts5 extension in turso/extensions (completion, core, crypto, csv, fuzzy, ipaddr, regexp).
2. Everything else in theuth's static SQL ran: 498 of 626 statements pass outright, 85 fail only because of the NULL stand-in parameters (identically on sqlite3), 20 are control failures, and 23 are FTS5 (3 direct + 20 consequential). 193 of 196 migration statements apply (incl. the nested briefs migration); 11 of 11 `_sqlx_migrations` bookkeeping sequences work.
3. Stored schema text is rewritten by Turso (comments dropped, one line, `REFERENCES` moved to table-level `FOREIGN KEY`, `NULL` dropped, identifiers re-quoted). Any code that reads `sql FROM sqlite_master` and string-matches it would see different text; theuth has one `sqlite_master` statement in code (feature histogram) and sqlx's migrator does not read it.
4. AUTOINCREMENT is implemented with per-table `__turso_internal_seq___turso_internal_autoincrement_<table>` tables that appear in `sqlite_master` (agent: a2a_tasks, peer_exchanges; `sqlite_sequence` is also present).
5. `LIMIT NULL` inside an `IN (…)` subquery: sqlite3 no error / no rows, Turso `Runtime error: datatype mismatch (19)` (crates/double/src/predict.rs:489 shape; irrelevant once a real integer is bound).
6. Timings on the 1 GB sessions copy are within ±15 % of sqlite3 for 4 of 5 queries; the `LIKE '%…%'` scan over 191 k message bodies is 3.5× slower on Turso (3.314 s vs 0.937 s). `PRAGMA integrity_check` on 1 GB: Turso 1.747 s vs sqlite3 3.871 s.
7. The CLI stops at the first error inside one input chunk (positional SQL or several statements on one line) but continues across lines on stdin; errors go to stdout; the exit status is 1 if anything failed. There is no `.bail` equivalent and no json/csv output mode.
8. Turso's rw open of a WAL-mode file checkpoints and truncates the `-wal` on close and creates `-wal`/`-shm` files even for pure reads; `--readonly` avoids both. COMPAT.md lists `sqlite3_backup_*` and `sqlite3_serialize/deserialize` as `❌ No | Stub`, and has no entry at all for the session extension (`sqlite3session_*`/changesets) that crates/agent, crates/sessionsync (libsqlite3-sys `session` feature) and crates/replica (rusqlite `session` + `bundled-sqlcipher`) enable — not tested here (CLI only), recorded as a requirement outside this spike's SQL scope.
9. Not determined: dynamic SQL (123 sites: `AssertSqlSafe(format!(…))` 57, `format!` 32, `QueryBuilder` 16, idents 18) was not replayed; sqlx-level behaviour (prepared-statement caching, `RETURNING` row shapes, type affinities as seen through sqlx's decode) was not exercised because nothing was compiled; the `tasks_schedule` partial index and other objects were compared by name and stored text only, not by query plans.

## 8 Files produced (all under /Users/Sky.Moore/Develop/spike-turso/compat/)

- REPORT.md — this file
- bin/turso_cli-aarch64-apple-darwin/tursodb (+ README.md, CHANGELOG.md, LICENSE.md), bin/turso_cli.tar.xz
- extract.py, extract_lib.py — corpus extraction (S2); corpus/migrations/<crate>/*.sql (73 files), corpus/queries.jsonl (748 sites), corpus/stats.json
- runner.py — marker-based per-statement runner for both CLIs
- replay_migrations.py — S3; results/migrations.tsv (mode A per statement + mode B per file), results/migrations_summary.json, results/schema/<crate>.<engine>.txt, db/<crate>.{sqlite3,turso}[.modeB].sqlite
- schema_diff.py — S3 schema comparison; results/schema_diff.json
- replay_queries.py — S4; results/queries.tsv (626 rows), results/queries_summary.json, db/q_<db>.<engine>.sqlite (working copies)
- s5_backup.sh — S5 copies; data/{tasks,roster,knowledge,replica,artifacts,schedule,codeindex-theuth,sessions}.sqlite, data/wal/ (run 1 trio), data/wal2/ (run 2), data/wal3/ (run 3 incl. B/M duplicates, T.sqlite)
- s5_compare.py — S5; results/s5_summary.json, results/s5/<file>.<Q>.<engine>.txt (query outputs)
- s5_wal_trio.py, s5_wal_decisive.py — S5b; results/s5_wal_trio.json, results/s5_wal_trio_decisive.json
- logs/*.out, logs/probe1.sh, logs/q*.py — run logs and ad-hoc inspection scripts
