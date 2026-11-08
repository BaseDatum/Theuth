-- 182 TASK-HOME D8: every engine's roster, replicated — one row per
-- (engine, agent), written by that engine from its own agents/*.md files plus
-- theuth (built in on every engine). Read by the move checks and the Agents
-- page. The name-only `agents` mirror (0001) is left as it is.
CREATE TABLE engine_agents (
    id        TEXT PRIMARY KEY,   -- <engine id>/<agent name>
    engine    TEXT NOT NULL,      -- mesh engine id
    name      TEXT NOT NULL,
    file_json TEXT NOT NULL,      -- the AgentFile without `body`
    hlc       TEXT
);
CREATE INDEX engine_agents_name ON engine_agents (name);
