-- 210 FIX4-EDGE #18: edge-wide flags that survive a restart (`paused` = `admin pause`).
CREATE TABLE edge_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
