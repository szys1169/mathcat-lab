CREATE TABLE IF NOT EXISTS task_steers (
    steer_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE CASCADE,
    command_id TEXT NOT NULL REFERENCES human_commands(command_id) ON DELETE CASCADE,
    content TEXT NOT NULL,
    expected_task_revision INTEGER NOT NULL,
    expected_route_epoch INTEGER NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    applied_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_task_steers_pending
    ON task_steers(task_id, status, created_at);

CREATE TABLE IF NOT EXISTS budget_overrides (
    budget_override_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    scope_kind TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    limits_json TEXT NOT NULL,
    reason TEXT NOT NULL,
    requested_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(project_id, scope_kind, scope_id)
);

CREATE TABLE IF NOT EXISTS human_questions (
    question_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    question TEXT NOT NULL,
    options_json TEXT NOT NULL,
    blocking_entity_ids_json TEXT NOT NULL,
    status TEXT NOT NULL,
    answer_json TEXT,
    asked_by TEXT NOT NULL,
    answered_by TEXT,
    timeout_at TEXT,
    created_at TEXT NOT NULL,
    answered_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_human_questions_project_status
    ON human_questions(project_id, status, created_at);
