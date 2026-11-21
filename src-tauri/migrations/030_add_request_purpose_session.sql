-- Request purpose, client session and retry grouping, so the request log can show
-- everything the Live Routing page shows. All nullable: rows written before this
-- migration simply have no purpose / session / trace.
ALTER TABLE requests ADD COLUMN purpose_kind TEXT;
ALTER TABLE requests ADD COLUMN purpose_guessed INTEGER;
ALTER TABLE requests ADD COLUMN purpose_turn INTEGER;
ALTER TABLE requests ADD COLUMN purpose_step INTEGER;
ALTER TABLE requests ADD COLUMN purpose_prev_tools TEXT;
ALTER TABLE requests ADD COLUMN session_client TEXT;
ALTER TABLE requests ADD COLUMN session_id TEXT;
ALTER TABLE requests ADD COLUMN trace_id TEXT;
CREATE INDEX IF NOT EXISTS idx_requests_session ON requests(session_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_requests_trace ON requests(trace_id);
