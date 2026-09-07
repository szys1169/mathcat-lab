PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS projects (
    project_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    problem_contract_json TEXT NOT NULL,
    status TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0,
    current_round INTEGER NOT NULL DEFAULT 0,
    budget_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS rounds (
    round_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    number INTEGER NOT NULL,
    status TEXT NOT NULL,
    based_on_revision INTEGER NOT NULL,
    started_at TEXT,
    completed_at TEXT,
    summary TEXT,
    UNIQUE(project_id, number)
);

CREATE TABLE IF NOT EXISTS routes (
    route_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    method_summary TEXT NOT NULL,
    target_goal_ids_json TEXT NOT NULL,
    required_fact_ids_json TEXT NOT NULL,
    status TEXT NOT NULL,
    score REAL NOT NULL,
    priority REAL NOT NULL,
    cancellation_epoch INTEGER NOT NULL DEFAULT 0,
    created_in_round INTEGER NOT NULL,
    attributes_json TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS workers (
    worker_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    backend TEXT NOT NULL,
    status TEXT NOT NULL,
    current_task_id TEXT,
    current_route_id TEXT,
    session_id TEXT,
    last_heartbeat TEXT
);

CREATE TABLE IF NOT EXISTS tasks (
    task_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    route_id TEXT NOT NULL REFERENCES routes(route_id),
    worker_id TEXT REFERENCES workers(worker_id),
    worker_role TEXT NOT NULL,
    goal_ids_json TEXT NOT NULL,
    objective TEXT NOT NULL,
    completion_contract TEXT NOT NULL,
    status TEXT NOT NULL,
    priority REAL NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    route_cancellation_epoch INTEGER NOT NULL DEFAULT 0,
    round INTEGER NOT NULL,
    result_summary TEXT
);

CREATE TABLE IF NOT EXISTS goals (
    goal_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    statement TEXT NOT NULL,
    parent_goal_ids_json TEXT NOT NULL,
    status TEXT NOT NULL,
    priority REAL NOT NULL,
    blocked_by_json TEXT NOT NULL,
    solved_by_fact_id TEXT,
    created_in_round INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS goal_edges (
    edge_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    UNIQUE(project_id, source_id, target_id, kind)
);

CREATE TABLE IF NOT EXISTS hypotheses (
    hypothesis_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    statement TEXT NOT NULL,
    status TEXT NOT NULL,
    route_id TEXT,
    promoted_to_fact_id TEXT,
    created_in_round INTEGER NOT NULL,
    attributes_json TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS hypothesis_edges (
    edge_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    UNIQUE(project_id, source_id, target_id, kind)
);

CREATE TABLE IF NOT EXISTS facts (
    fact_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    statement TEXT NOT NULL,
    assumptions_json TEXT NOT NULL,
    proof_markdown TEXT NOT NULL,
    dependency_fact_ids_json TEXT NOT NULL,
    definitions_introduced_json TEXT NOT NULL,
    external_source_ids_json TEXT NOT NULL,
    verification_ids_json TEXT NOT NULL,
    evidence_level TEXT NOT NULL,
    created_by TEXT NOT NULL,
    status TEXT NOT NULL,
    content_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS fact_edges (
    edge_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    UNIQUE(project_id, source_id, target_id, kind)
);

CREATE TABLE IF NOT EXISTS candidates (
    candidate_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    submission_json TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS verifications (
    verification_id TEXT PRIMARY KEY,
    candidate_id TEXT NOT NULL REFERENCES candidates(candidate_id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    report_json TEXT,
    started_at TEXT,
    completed_at TEXT
);

CREATE TABLE IF NOT EXISTS uncertainties (
    uncertainty_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    description TEXT NOT NULL,
    type TEXT NOT NULL,
    severity TEXT NOT NULL,
    affects_goal_ids_json TEXT NOT NULL,
    affects_route_ids_json TEXT NOT NULL,
    introduced_by TEXT NOT NULL,
    resolution_methods_json TEXT NOT NULL,
    status TEXT NOT NULL,
    resolved_by TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS failures (
    failure_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    route_id TEXT,
    task_id TEXT,
    failure_type TEXT NOT NULL,
    summary TEXT NOT NULL,
    repairable INTEGER NOT NULL,
    raw_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS failure_patterns (
    pattern_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    pattern_json TEXT NOT NULL,
    confidence REAL NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS human_commands (
    command_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    type TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id TEXT NOT NULL,
    mode TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    expected_project_revision INTEGER NOT NULL,
    idempotency_key TEXT NOT NULL,
    reason TEXT NOT NULL,
    requested_by TEXT NOT NULL,
    status TEXT NOT NULL,
    before_revision INTEGER,
    after_revision INTEGER,
    affected_entities_json TEXT NOT NULL DEFAULT '[]',
    error TEXT,
    created_at TEXT NOT NULL,
    applied_at TEXT,
    UNIQUE(project_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS suggestions (
    suggestion_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    command_id TEXT NOT NULL REFERENCES human_commands(command_id) ON DELETE CASCADE,
    content TEXT NOT NULL,
    target_route_id TEXT,
    status TEXT NOT NULL,
    decision TEXT,
    created_in_round INTEGER NOT NULL,
    effective_round INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    cursor INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    project_revision INTEGER NOT NULL,
    type TEXT NOT NULL,
    entity_json TEXT NOT NULL,
    data_json TEXT NOT NULL,
    caused_by_json TEXT,
    occurred_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS artifacts (
    artifact_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    media_type TEXT NOT NULL,
    filename TEXT NOT NULL,
    size INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    created_in_round INTEGER NOT NULL,
    related_entity_ids_json TEXT NOT NULL,
    storage_path TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS usage_records (
    usage_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    round INTEGER,
    worker_id TEXT,
    task_id TEXT,
    model TEXT,
    model_calls INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    elapsed_ms INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_events_project_cursor ON events(project_id, cursor);
CREATE INDEX IF NOT EXISTS idx_tasks_project_status ON tasks(project_id, status);
CREATE INDEX IF NOT EXISTS idx_routes_project_status ON routes(project_id, status);
CREATE INDEX IF NOT EXISTS idx_verifications_project_status ON verifications(project_id, status);
CREATE INDEX IF NOT EXISTS idx_uncertainties_project_status ON uncertainties(project_id, status);
CREATE INDEX IF NOT EXISTS idx_artifacts_project_round ON artifacts(project_id, created_in_round);

