-- Design doc 50 §5.3: who answered a peer request. The replying endpoint key
-- (`session:<uuid>` / `peer:<binding>`) when a peer replied, or
-- `owner:<session>` when the human answered on that session's behalf.
ALTER TABLE peer_exchanges ADD COLUMN answered_by TEXT NULL;
