CREATE TABLE IF NOT EXISTS fact_governance_commands (
    governance_command_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    fact_id TEXT NOT NULL REFERENCES facts(fact_id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    action TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    result_json TEXT,
    requested_by TEXT NOT NULL,
    reason TEXT NOT NULL,
    created_at TEXT NOT NULL,
    completed_at TEXT,
    UNIQUE(project_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS fact_challenges (
    challenge_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    fact_id TEXT NOT NULL REFERENCES facts(fact_id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    reason TEXT NOT NULL,
    status TEXT NOT NULL,
    requested_by TEXT NOT NULL,
    verification_id TEXT REFERENCES verifications(verification_id),
    created_at TEXT NOT NULL,
    resolved_at TEXT
);

CREATE TABLE IF NOT EXISTS experiment_capsules (
    capsule_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    task_id TEXT REFERENCES tasks(task_id) ON DELETE SET NULL,
    route_id TEXT REFERENCES routes(route_id) ON DELETE SET NULL,
    language TEXT NOT NULL,
    program_text TEXT NOT NULL,
    input_json TEXT NOT NULL,
    environment_json TEXT NOT NULL,
    stdout TEXT NOT NULL,
    stderr TEXT NOT NULL,
    exit_code INTEGER,
    artifacts_json TEXT NOT NULL,
    conclusion_mapping_json TEXT NOT NULL,
    replay_command_json TEXT NOT NULL,
    status TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    replayed_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_fact_challenges_fact
    ON fact_challenges(fact_id, created_at);
CREATE INDEX IF NOT EXISTS idx_experiment_capsules_project
    ON experiment_capsules(project_id, created_at);
