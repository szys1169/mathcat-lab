-- PostgreSQL production State Committer schema for multi-process leasing and result ingestion.

CREATE TABLE IF NOT EXISTS projects (
    project_id TEXT PRIMARY KEY,
    revision BIGINT NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    contract_json JSONB NOT NULL,
    budget_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS routes (
    route_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    cancellation_epoch BIGINT NOT NULL DEFAULT 0,
    family_id TEXT,
    semantic_fingerprint TEXT,
    payload_json JSONB NOT NULL,
    UNIQUE(project_id, semantic_fingerprint)
);

CREATE TABLE IF NOT EXISTS plan_revisions (
    plan_revision_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    ordinal BIGINT NOT NULL,
    based_on_project_revision BIGINT NOT NULL,
    consumed_delta_from BIGINT NOT NULL,
    consumed_delta_to BIGINT NOT NULL,
    planner_mode TEXT NOT NULL,
    payload_json JSONB NOT NULL,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    committed_at TIMESTAMPTZ,
    UNIQUE(project_id, ordinal)
);

CREATE TABLE IF NOT EXISTS tasks (
    task_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    route_id TEXT NOT NULL REFERENCES routes(route_id) ON DELETE CASCADE,
    plan_revision_id TEXT REFERENCES plan_revisions(plan_revision_id),
    task_signature TEXT NOT NULL,
    status TEXT NOT NULL,
    priority DOUBLE PRECISION NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1,
    route_cancellation_epoch BIGINT NOT NULL,
    context_packet_id TEXT,
    contract_json JSONB NOT NULL,
    payload_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS tasks_live_signature_unique
ON tasks(project_id, task_signature)
WHERE status IN ('queued','offered','leased','running','checkpointed','result_submitted','ingesting');

CREATE TABLE IF NOT EXISTS task_attempts (
    attempt_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE CASCADE,
    worker_instance_id TEXT NOT NULL,
    attempt_no BIGINT NOT NULL,
    lease_epoch BIGINT NOT NULL,
    status TEXT NOT NULL,
    failure_signature TEXT,
    failure_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    UNIQUE(task_id, attempt_no)
);

CREATE TABLE IF NOT EXISTS task_leases (
    lease_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL REFERENCES task_attempts(attempt_id) ON DELETE CASCADE,
    worker_instance_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    lease_epoch BIGINT NOT NULL,
    task_revision BIGINT NOT NULL,
    route_epoch BIGINT NOT NULL,
    token_hash TEXT NOT NULL,
    status TEXT NOT NULL,
    leased_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    last_heartbeat_at TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS task_leases_one_active_attempt
ON task_leases(task_id) WHERE status='active';

CREATE TABLE IF NOT EXISTS result_envelopes (
    result_envelope_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL REFERENCES task_attempts(attempt_id) ON DELETE CASCADE,
    lease_id TEXT NOT NULL REFERENCES task_leases(lease_id) ON DELETE CASCADE,
    lease_epoch BIGINT NOT NULL,
    route_cancellation_epoch BIGINT NOT NULL,
    content_hash TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    outcome TEXT NOT NULL,
    envelope_json JSONB NOT NULL,
    status TEXT NOT NULL,
    rejection_reason TEXT,
    submitted_at TIMESTAMPTZ NOT NULL,
    ingested_at TIMESTAMPTZ,
    UNIQUE(project_id, idempotency_key),
    UNIQUE(attempt_id, content_hash)
);

CREATE TABLE IF NOT EXISTS events (
    event_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    project_revision BIGINT NOT NULL,
    event_type TEXT NOT NULL,
    entity_json JSONB NOT NULL,
    data_json JSONB NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS event_outbox (
    outbox_id TEXT PRIMARY KEY,
    event_id TEXT NOT NULL UNIQUE REFERENCES events(event_id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    payload_json JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    attempt_count BIGINT NOT NULL DEFAULT 0,
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at TIMESTAMPTZ,
    last_error TEXT
);

CREATE INDEX IF NOT EXISTS postgres_tasks_claim
ON tasks(project_id, status, priority DESC, created_at);
CREATE INDEX IF NOT EXISTS postgres_outbox_pending
ON event_outbox(status, available_at);
