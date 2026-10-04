-- 91 THOUGHT-TIME: ms the model thought before this assistant row's reply
-- or tool calls (first reasoning chunk to first non-reasoning event of the
-- same model call). NULL for rows written before this migration and for
-- replies with no streamed reasoning.
ALTER TABLE messages ADD COLUMN thinking_ms INTEGER NULL;
