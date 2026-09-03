ALTER TABLE projects ADD COLUMN human_route_approval INTEGER NOT NULL DEFAULT 0;
ALTER TABLE routes ADD COLUMN human_review TEXT NOT NULL DEFAULT 'not_required'
    CHECK (human_review IN ('not_required','pending','approved','rejected'));

CREATE TABLE IF NOT EXISTS problem_contract_versions (
    problem_contract_version_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    contract_version INTEGER NOT NULL,
    contract_json TEXT NOT NULL,
    previous_contract_json TEXT NOT NULL,
    change_reason TEXT NOT NULL,
    changed_by TEXT NOT NULL,
    command_id TEXT NOT NULL REFERENCES human_commands(command_id),
    created_revision INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(project_id, contract_version)
);

CREATE TABLE IF NOT EXISTS human_route_proposals (
    proposal_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    method_summary TEXT NOT NULL,
    target_goal_ids_json TEXT NOT NULL,
    required_fact_ids_json TEXT NOT NULL,
    known_risks_json TEXT NOT NULL,
    reason TEXT NOT NULL,
    proposed_by TEXT NOT NULL,
    command_id TEXT NOT NULL REFERENCES human_commands(command_id),
    status TEXT NOT NULL CHECK (status IN ('queued','accepted','rejected')),
    route_id TEXT REFERENCES routes(route_id),
    decision_reason TEXT,
    created_revision INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    decided_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_human_route_proposals_project_status
    ON human_route_proposals(project_id, status, created_at);

CREATE TABLE IF NOT EXISTS board_write_requests (
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    action TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    command_id TEXT NOT NULL REFERENCES human_commands(command_id),
    result_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(project_id, idempotency_key)
);
