-- Planning, execution, context, and state-commit reliability V2.
-- This migration is intentionally forward-only; legacy rows receive conservative defaults.

ALTER TABLE routes ADD COLUMN family_id TEXT;
ALTER TABLE routes ADD COLUMN semantic_fingerprint TEXT;
ALTER TABLE routes ADD COLUMN consecutive_no_progress_plans INTEGER NOT NULL DEFAULT 0;
ALTER TABLE routes ADD COLUMN failed_attempt_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE routes ADD COLUMN created_at_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE routes ADD COLUMN last_material_progress_revision INTEGER;
ALTER TABLE routes ADD COLUMN merged_into TEXT;
ALTER TABLE routes ADD COLUMN merge_reason TEXT;
ALTER TABLE routes ADD COLUMN exit_criteria_json TEXT NOT NULL DEFAULT '[]';

UPDATE routes
SET family_id = COALESCE(family_id, 'family_' || route_id),
    semantic_fingerprint = COALESCE(semantic_fingerprint, route_id),
    created_at_revision = CASE
        WHEN created_at_revision = 0 THEN created_in_round
        ELSE created_at_revision
    END;

CREATE UNIQUE INDEX IF NOT EXISTS idx_routes_project_fingerprint_live
ON routes(project_id, semantic_fingerprint)
WHERE status NOT IN ('merged','pruned','human_stopped');
CREATE INDEX IF NOT EXISTS idx_routes_project_family_status
ON routes(project_id, family_id, status);

ALTER TABLE tasks ADD COLUMN plan_revision_id TEXT;
ALTER TABLE tasks ADD COLUMN task_signature TEXT;
ALTER TABLE tasks ADD COLUMN context_packet_id TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_tasks_active_signature
ON tasks(project_id, task_signature)
WHERE task_signature IS NOT NULL
  AND status IN ('queued','offered','leased','running','checkpointed','result_submitted','ingesting');

CREATE TABLE IF NOT EXISTS research_deltas (
    delta_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    from_revision INTEGER NOT NULL,
    to_revision INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    consumed_by_plan_revision_id TEXT,
    consumed_at TEXT,
    UNIQUE(project_id, from_revision, to_revision)
);

CREATE INDEX IF NOT EXISTS idx_research_deltas_project_status
ON research_deltas(project_id, status, to_revision);

CREATE TABLE IF NOT EXISTS fact_impact_records (
    impact_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    fact_id TEXT NOT NULL REFERENCES facts(fact_id) ON DELETE CASCADE,
    source_revision INTEGER NOT NULL,
    closed_goal_ids_json TEXT NOT NULL DEFAULT '[]',
    unblocked_route_ids_json TEXT NOT NULL DEFAULT '[]',
    invalidated_task_ids_json TEXT NOT NULL DEFAULT '[]',
    newly_enabled_task_templates_json TEXT NOT NULL DEFAULT '[]',
    dominated_route_ids_json TEXT NOT NULL DEFAULT '[]',
    resolved_uncertainty_ids_json TEXT NOT NULL DEFAULT '[]',
    planner_disposition TEXT NOT NULL DEFAULT 'pending',
    disposition_reason TEXT,
    created_at TEXT NOT NULL,
    applied_at TEXT,
    UNIQUE(project_id, fact_id, source_revision)
);

CREATE INDEX IF NOT EXISTS idx_fact_impact_project_disposition
ON fact_impact_records(project_id, planner_disposition, source_revision);

CREATE TABLE IF NOT EXISTS bottlenecks (
    bottleneck_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    target_goal_ids_json TEXT NOT NULL,
    kind TEXT NOT NULL,
    precise_statement TEXT NOT NULL,
    completion_contract_json TEXT NOT NULL,
    evidence_ids_json TEXT NOT NULL DEFAULT '[]',
    blocked_route_ids_json TEXT NOT NULL DEFAULT '[]',
    attempted_task_ids_json TEXT NOT NULL DEFAULT '[]',
    failure_pattern_ids_json TEXT NOT NULL DEFAULT '[]',
    repair_action_ids_json TEXT NOT NULL DEFAULT '[]',
    priority REAL NOT NULL,
    status TEXT NOT NULL,
    reopen_condition TEXT,
    created_revision INTEGER NOT NULL,
    updated_revision INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_bottlenecks_project_status_priority
ON bottlenecks(project_id, status, priority DESC);

CREATE TABLE IF NOT EXISTS route_families (
    family_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    semantic_key TEXT NOT NULL,
    canonical_route_id TEXT,
    status TEXT NOT NULL,
    created_revision INTEGER NOT NULL,
    updated_revision INTEGER NOT NULL,
    UNIQUE(project_id, semantic_key)
);

CREATE TABLE IF NOT EXISTS route_progress_entries (
    progress_entry_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    route_id TEXT NOT NULL REFERENCES routes(route_id) ON DELETE CASCADE,
    source_entity_json TEXT NOT NULL,
    progress_kind TEXT NOT NULL,
    material INTEGER NOT NULL,
    summary TEXT NOT NULL,
    evidence_ids_json TEXT NOT NULL DEFAULT '[]',
    project_revision INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_route_progress_route_revision
ON route_progress_entries(route_id, project_revision);

CREATE TABLE IF NOT EXISTS route_tombstones (
    tombstone_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    route_id TEXT NOT NULL REFERENCES routes(route_id) ON DELETE CASCADE,
    semantic_fingerprint TEXT NOT NULL,
    reason TEXT NOT NULL,
    blocking_fact_pattern TEXT,
    revive_only_if_json TEXT NOT NULL,
    created_revision INTEGER NOT NULL,
    revived_revision INTEGER,
    created_at TEXT NOT NULL,
    UNIQUE(project_id, semantic_fingerprint, created_revision)
);

CREATE INDEX IF NOT EXISTS idx_route_tombstones_project_fingerprint
ON route_tombstones(project_id, semantic_fingerprint, revived_revision);

CREATE TABLE IF NOT EXISTS context_packets (
    context_packet_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    task_id TEXT REFERENCES tasks(task_id) ON DELETE CASCADE,
    route_id TEXT REFERENCES routes(route_id) ON DELETE CASCADE,
    packet_kind TEXT NOT NULL,
    source_revision INTEGER NOT NULL,
    problem_contract_excerpt TEXT NOT NULL,
    objective TEXT NOT NULL,
    known_fact_ids_json TEXT NOT NULL DEFAULT '[]',
    bottleneck_id TEXT REFERENCES bottlenecks(bottleneck_id),
    route_progress_json TEXT NOT NULL DEFAULT '{}',
    relevant_failure_pattern_ids_json TEXT NOT NULL DEFAULT '[]',
    relevant_uncertainty_ids_json TEXT NOT NULL DEFAULT '[]',
    source_refs_json TEXT NOT NULL DEFAULT '[]',
    omitted_sections_json TEXT NOT NULL DEFAULT '[]',
    token_estimate INTEGER NOT NULL,
    content_json TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    status TEXT NOT NULL,
    invalidation_reason TEXT,
    created_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_context_packets_content_hash
ON context_packets(project_id, packet_kind, content_hash);
CREATE INDEX IF NOT EXISTS idx_context_packets_task_status
ON context_packets(task_id, status, source_revision);

CREATE TABLE IF NOT EXISTS context_summaries (
    summary_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    summary_kind TEXT NOT NULL,
    scope_kind TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    input_entity_ids_json TEXT NOT NULL,
    input_revision INTEGER NOT NULL,
    summarizer_version TEXT NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    omitted_categories_json TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL,
    rebuild_condition TEXT,
    created_at TEXT NOT NULL,
    invalidated_at TEXT,
    UNIQUE(project_id, summary_kind, scope_kind, scope_id, input_revision)
);

CREATE TABLE IF NOT EXISTS plan_revisions (
    plan_revision_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    round_id TEXT REFERENCES rounds(round_id) ON DELETE SET NULL,
    based_on_project_revision INTEGER NOT NULL,
    consumed_delta_from INTEGER NOT NULL,
    consumed_delta_to INTEGER NOT NULL,
    planner_mode TEXT NOT NULL,
    context_packet_id TEXT REFERENCES context_packets(context_packet_id),
    route_decisions_json TEXT NOT NULL,
    bottleneck_updates_json TEXT NOT NULL,
    task_contract_ids_json TEXT NOT NULL,
    status TEXT NOT NULL,
    rationale TEXT NOT NULL,
    created_at TEXT NOT NULL,
    committed_at TEXT,
    UNIQUE(project_id, ordinal)
);

CREATE INDEX IF NOT EXISTS idx_plan_revisions_project_ordinal
ON plan_revisions(project_id, ordinal DESC);

CREATE TABLE IF NOT EXISTS plan_fact_decisions (
    decision_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    plan_revision_id TEXT NOT NULL REFERENCES plan_revisions(plan_revision_id) ON DELETE CASCADE,
    fact_id TEXT NOT NULL REFERENCES facts(fact_id) ON DELETE CASCADE,
    disposition TEXT NOT NULL,
    reason TEXT NOT NULL,
    affected_entity_ids_json TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    UNIQUE(plan_revision_id, fact_id)
);

CREATE TABLE IF NOT EXISTS task_contracts (
    task_contract_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE CASCADE,
    plan_revision_id TEXT NOT NULL REFERENCES plan_revisions(plan_revision_id) ON DELETE CASCADE,
    contract_version INTEGER NOT NULL,
    contract_json TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(task_id, contract_version),
    UNIQUE(project_id, content_hash)
);

CREATE TABLE IF NOT EXISTS worker_instances (
    worker_instance_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    worker_id TEXT REFERENCES workers(worker_id) ON DELETE SET NULL,
    backend TEXT NOT NULL,
    backend_version TEXT,
    model TEXT,
    status TEXT NOT NULL,
    capability_json TEXT NOT NULL DEFAULT '{}',
    working_directory TEXT,
    handshake_json TEXT,
    quarantine_reason TEXT,
    started_at TEXT NOT NULL,
    ready_at TEXT,
    last_heartbeat_at TEXT,
    exited_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_worker_instances_project_status
ON worker_instances(project_id, status);

CREATE TABLE IF NOT EXISTS task_attempts (
    attempt_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE CASCADE,
    worker_instance_id TEXT REFERENCES worker_instances(worker_instance_id) ON DELETE SET NULL,
    attempt_no INTEGER NOT NULL,
    status TEXT NOT NULL,
    lease_epoch INTEGER NOT NULL,
    plan_revision_id TEXT REFERENCES plan_revisions(plan_revision_id),
    route_cancellation_epoch INTEGER NOT NULL,
    context_packet_id TEXT REFERENCES context_packets(context_packet_id),
    failure_signature TEXT,
    failure_reason TEXT,
    started_at TEXT,
    completed_at TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(task_id, attempt_no)
);

CREATE INDEX IF NOT EXISTS idx_task_attempts_task_status
ON task_attempts(task_id, status, attempt_no DESC);

ALTER TABLE task_leases ADD COLUMN attempt_id TEXT;
ALTER TABLE task_leases ADD COLUMN worker_instance_id TEXT;
ALTER TABLE task_leases ADD COLUMN lease_token_hash TEXT;
ALTER TABLE task_leases ADD COLUMN offered_at TEXT;
ALTER TABLE task_leases ADD COLUMN last_heartbeat_at TEXT;

CREATE TABLE IF NOT EXISTS worker_heartbeats (
    heartbeat_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    worker_instance_id TEXT NOT NULL REFERENCES worker_instances(worker_instance_id) ON DELETE CASCADE,
    attempt_id TEXT REFERENCES task_attempts(attempt_id) ON DELETE SET NULL,
    lease_epoch INTEGER,
    health_json TEXT NOT NULL,
    occurred_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_worker_heartbeats_instance_time
ON worker_heartbeats(worker_instance_id, occurred_at DESC);

CREATE TABLE IF NOT EXISTS task_checkpoints (
    checkpoint_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL REFERENCES task_attempts(attempt_id) ON DELETE CASCADE,
    artifact_id TEXT NOT NULL REFERENCES artifacts(artifact_id),
    lease_epoch INTEGER NOT NULL,
    summary TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_task_checkpoints_task_time
ON task_checkpoints(task_id, created_at DESC);

CREATE TABLE IF NOT EXISTS result_envelopes (
    result_envelope_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL REFERENCES task_attempts(attempt_id) ON DELETE CASCADE,
    lease_id TEXT NOT NULL REFERENCES task_leases(lease_id) ON DELETE CASCADE,
    lease_epoch INTEGER NOT NULL,
    plan_revision_id TEXT,
    route_cancellation_epoch INTEGER NOT NULL,
    outcome TEXT NOT NULL,
    envelope_json TEXT NOT NULL,
    result_artifact_id TEXT REFERENCES artifacts(artifact_id),
    content_hash TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    status TEXT NOT NULL,
    rejection_reason TEXT,
    submitted_at TEXT NOT NULL,
    ingested_at TEXT,
    UNIQUE(project_id, idempotency_key),
    UNIQUE(attempt_id, content_hash)
);

CREATE INDEX IF NOT EXISTS idx_result_envelopes_project_status
ON result_envelopes(project_id, status, submitted_at);

CREATE TABLE IF NOT EXISTS domain_command_dedup (
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    command_kind TEXT NOT NULL,
    status TEXT NOT NULL,
    result_json TEXT,
    created_at TEXT NOT NULL,
    completed_at TEXT,
    PRIMARY KEY(project_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS event_outbox (
    outbox_id TEXT PRIMARY KEY,
    event_id TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    attempt_count INTEGER NOT NULL DEFAULT 0,
    available_at TEXT NOT NULL,
    delivered_at TEXT,
    last_error TEXT,
    UNIQUE(event_id)
);

CREATE INDEX IF NOT EXISTS idx_event_outbox_status_available
ON event_outbox(status, available_at);

INSERT OR IGNORE INTO event_outbox(
    outbox_id,event_id,project_id,event_type,payload_json,status,available_at,delivered_at
)
SELECT
    'outbox_' || event_id,event_id,project_id,type,
    json_object(
        'event_id',event_id,
        'cursor',cursor,
        'project_id',project_id,
        'project_revision',project_revision,
        'type',type,
        'entity',json(entity_json),
        'data',json(data_json),
        'caused_by',CASE WHEN caused_by_json IS NULL THEN NULL ELSE json(caused_by_json) END,
        'occurred_at',occurred_at
    ),
    'delivered',occurred_at,occurred_at
FROM events;

CREATE TABLE IF NOT EXISTS reconciliation_runs (
    reconciliation_run_id TEXT PRIMARY KEY,
    project_id TEXT REFERENCES projects(project_id) ON DELETE CASCADE,
    trigger_kind TEXT NOT NULL,
    status TEXT NOT NULL,
    repairs_json TEXT NOT NULL DEFAULT '[]',
    integrity_errors_json TEXT NOT NULL DEFAULT '[]',
    started_at TEXT NOT NULL,
    completed_at TEXT
);

CREATE TABLE IF NOT EXISTS planner_health (
    project_id TEXT PRIMARY KEY REFERENCES projects(project_id) ON DELETE CASCADE,
    circuit_state TEXT NOT NULL DEFAULT 'closed',
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_failure_reason TEXT,
    opened_at TEXT,
    cooldown_until TEXT,
    last_probe_at TEXT,
    updated_at TEXT NOT NULL
);

INSERT OR IGNORE INTO planner_health(project_id, updated_at)
SELECT project_id, updated_at FROM projects;
