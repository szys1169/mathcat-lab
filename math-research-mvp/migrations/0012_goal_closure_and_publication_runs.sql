-- Auditable goal-closure evidence and idempotent publication lifecycle.

CREATE TABLE IF NOT EXISTS goal_closure_records (
    closure_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    goal_id TEXT NOT NULL REFERENCES goals(goal_id) ON DELETE CASCADE,
    fact_id TEXT NOT NULL REFERENCES facts(fact_id) ON DELETE CASCADE,
    verification_id TEXT NOT NULL REFERENCES verifications(verification_id) ON DELETE CASCADE,
    check_id TEXT NOT NULL REFERENCES verification_checks(check_id) ON DELETE RESTRICT,
    outcome TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(goal_id, fact_id, verification_id)
);

CREATE INDEX IF NOT EXISTS idx_goal_closure_project_goal
ON goal_closure_records(project_id, goal_id, created_at);

CREATE TABLE IF NOT EXISTS publication_runs (
    publication_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    allow_partial INTEGER NOT NULL,
    source_revision INTEGER NOT NULL,
    source_packet_hash TEXT NOT NULL,
    status TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 1,
    result_json TEXT,
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    UNIQUE(project_id, idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_publication_runs_project_status
ON publication_runs(project_id, status, created_at);
