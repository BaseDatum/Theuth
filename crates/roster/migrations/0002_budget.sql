-- 67 §6.4: what each agent spent this month, per engine (replicated, so
-- every engine adds its own row and the sum is the month). `budget_override`
-- records the manager's `agent_pause off` on an over-cap agent: no re-pause
-- that month. replica::attach needs a single-column primary key, so `id` is
-- `<agent>|<month>|<engine>` / `<agent>|<month>`.

CREATE TABLE budget (
    id         TEXT PRIMARY KEY,           -- <agent>|<month>|<engine>
    agent      TEXT    NOT NULL,
    month      TEXT    NOT NULL,           -- YYYY-MM (UTC)
    engine     TEXT    NOT NULL,           -- mesh engine id, else hostname
    spent      REAL    NOT NULL DEFAULT 0,
    currency   TEXT    NOT NULL DEFAULT 'USD',
    updated_ms INTEGER NOT NULL,
    hlc        TEXT
);
CREATE INDEX budget_agent_month ON budget (agent, month);

CREATE TABLE budget_override (
    id         TEXT PRIMARY KEY,           -- <agent>|<month>
    agent      TEXT    NOT NULL,
    month      TEXT    NOT NULL,
    created_ms INTEGER NOT NULL,
    hlc        TEXT
);
