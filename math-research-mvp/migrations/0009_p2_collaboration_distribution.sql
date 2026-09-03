CREATE TABLE IF NOT EXISTS actors (
    actor_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    role TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS actor_tokens (
    token_id TEXT PRIMARY KEY,
    actor_id TEXT NOT NULL REFERENCES actors(actor_id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    revoked_at TEXT
);

CREATE TABLE IF NOT EXISTS security_audit (
    audit_id TEXT PRIMARY KEY,
    project_id TEXT,
    actor_id TEXT,
    action TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id TEXT NOT NULL,
    decision TEXT NOT NULL,
    reason TEXT,
    request_hash TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_security_audit_project
    ON security_audit(project_id, created_at);

CREATE TABLE IF NOT EXISTS global_fact_catalog (
    content_hash TEXT PRIMARY KEY,
    source_project_id TEXT NOT NULL REFERENCES projects(project_id),
    source_fact_id TEXT NOT NULL REFERENCES facts(fact_id),
    statement TEXT NOT NULL,
    status TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT OR IGNORE INTO global_fact_catalog(content_hash,source_project_id,source_fact_id,statement,status,updated_at)
SELECT content_hash,project_id,fact_id,statement,status,created_at FROM facts;

CREATE TABLE IF NOT EXISTS project_fact_imports (
    import_id TEXT PRIMARY KEY,
    target_project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    source_project_id TEXT NOT NULL REFERENCES projects(project_id),
    source_fact_id TEXT NOT NULL REFERENCES facts(fact_id),
    content_hash TEXT NOT NULL REFERENCES global_fact_catalog(content_hash),
    assurance_snapshot_json TEXT NOT NULL,
    status TEXT NOT NULL,
    imported_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    invalidated_at TEXT,
    UNIQUE(target_project_id, content_hash)
);

CREATE INDEX IF NOT EXISTS idx_project_fact_imports_source
    ON project_fact_imports(source_fact_id, status);

CREATE TABLE IF NOT EXISTS worker_nodes (
    node_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    capabilities_json TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL,
    node_epoch INTEGER NOT NULL,
    registered_at TEXT NOT NULL,
    last_heartbeat TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS task_leases (
    lease_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE CASCADE,
    node_id TEXT NOT NULL REFERENCES worker_nodes(node_id) ON DELETE CASCADE,
    task_revision INTEGER NOT NULL,
    route_epoch INTEGER NOT NULL,
    lease_epoch INTEGER NOT NULL,
    status TEXT NOT NULL,
    leased_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    completed_at TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_task_leases_active_task
    ON task_leases(task_id) WHERE status='active';
CREATE INDEX IF NOT EXISTS idx_task_leases_node_status
    ON task_leases(node_id, status, expires_at);
