CREATE TABLE IF NOT EXISTS planning_stage_runs (
    stage_run_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    round_id TEXT NOT NULL REFERENCES rounds(round_id) ON DELETE CASCADE,
    stage TEXT NOT NULL,
    status TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    output_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(round_id, stage)
);

CREATE INDEX IF NOT EXISTS idx_planning_stage_project_round
    ON planning_stage_runs(project_id, round_id, stage);
