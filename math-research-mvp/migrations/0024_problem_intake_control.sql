-- Idempotent, auditable control commands for the pre-project problem intake lifecycle.
-- Attempt rows remain the durable history of every initial generation and explicit retry.
CREATE TABLE IF NOT EXISTS problem_draft_commands (
    command_id TEXT PRIMARY KEY,
    draft_id TEXT NOT NULL REFERENCES problem_drafts(draft_id) ON DELETE CASCADE,
    command_type TEXT NOT NULL CHECK (command_type IN ('cancel', 'retry')),
    requested_by TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    result_revision INTEGER NOT NULL CHECK (result_revision > 0),
    created_at TEXT NOT NULL,
    UNIQUE(requested_by, idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_problem_draft_commands_draft_created
    ON problem_draft_commands(draft_id, created_at, command_id);
