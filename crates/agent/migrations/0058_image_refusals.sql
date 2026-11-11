-- read-file-images (52 notes §2): models whose provider refused image input
-- in a session; the session sends them text in place of images from then on,
-- across reloads and restarts. Local: not attached to the session log, so the
-- replicated wire schema (SESSIONS_WIRE_SCHEMA) is unchanged; a replica
-- re-learns with one refused call.
CREATE TABLE image_refusals (
    session TEXT NOT NULL,
    model   TEXT NOT NULL,
    PRIMARY KEY (session, model)
);
