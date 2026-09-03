-- Distinguish a soft planning budget from the backend hard timeout and retain attempt history.

CREATE TABLE IF NOT EXISTS planning_stage_attempts (
    attempt_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    round_id TEXT NOT NULL REFERENCES rounds(round_id) ON DELETE CASCADE,
    stage TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    attempt_number INTEGER NOT NULL,
    status TEXT NOT NULL,
    soft_deadline_at TEXT NOT NULL,
    hard_deadline_at TEXT NOT NULL,
    last_heartbeat_at TEXT NOT NULL,
    soft_budget_exceeded INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    UNIQUE(round_id, stage, input_hash, attempt_number)
);

CREATE INDEX IF NOT EXISTS idx_planning_attempts_round_stage
ON planning_stage_attempts(project_id, round_id, stage, attempt_number);
