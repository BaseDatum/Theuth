-- A root nobody reads directly (opened by the scheduler): its system prompt
-- gets no visualization instructions. Subagents are recognised by `parent`.
ALTER TABLE sessions ADD COLUMN unattended INTEGER NOT NULL DEFAULT 0;
