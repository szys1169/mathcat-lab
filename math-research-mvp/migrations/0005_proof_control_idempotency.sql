CREATE TABLE IF NOT EXISTS proof_control_commands (
    command_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    action TEXT NOT NULL,
    target_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    result_json TEXT,
    created_at TEXT NOT NULL,
    completed_at TEXT,
    UNIQUE(project_id, idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_proof_control_target
    ON proof_control_commands(project_id, target_id, created_at);
