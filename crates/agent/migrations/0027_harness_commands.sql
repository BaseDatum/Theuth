-- Design doc 75 §3: the command manifest an external harness (ACP child)
-- announces in `available_commands_update` — its loaded skills
-- (`skill:<name>` on omp) and slash commands — as a JSON array of
-- `{name, description, hint}`, so a reloaded session lists the same skills
-- the harness had. Written once per distinct value. NULL for native sessions.
ALTER TABLE sessions ADD COLUMN harness_commands TEXT;
