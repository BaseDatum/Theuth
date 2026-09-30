-- 79 §6.6: a session on any engine asked the user for one or several secrets.
-- Replicated (67 §5.1 rules) so the card reaches the app on whatever engine
-- the user is attached to; `answer_secret` there writes the vault rows and
-- `answered_ms` / `outcome`, and the asking engine's tool call sees both arrive.
CREATE TABLE secret_requests (
    id          TEXT PRIMARY KEY,          -- RequestId (uuid)
    engine      TEXT    NOT NULL,          -- mesh engine id of the asking session
    session     TEXT    NOT NULL,
    title       TEXT,                      -- a form's card title; NULL for secret_request
    fields      TEXT    NOT NULL,          -- json [{name,label,masked,current}]
    created_ms  INTEGER NOT NULL,
    answered_ms INTEGER,
    outcome     TEXT,                      -- stored | declined | timeout
    hlc         TEXT
);
