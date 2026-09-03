-- Bring verification model workers under the same packet/contract/lease/envelope discipline.

ALTER TABLE verification_attempts ADD COLUMN worker_instance_id TEXT REFERENCES worker_instances(worker_instance_id) ON DELETE SET NULL;
ALTER TABLE verification_attempts ADD COLUMN context_packet_id TEXT REFERENCES context_packets(context_packet_id) ON DELETE SET NULL;
ALTER TABLE verification_attempts ADD COLUMN lease_epoch INTEGER NOT NULL DEFAULT 0;
ALTER TABLE verification_attempts ADD COLUMN failure_signature TEXT;

CREATE TABLE IF NOT EXISTS verification_task_contracts (
    verification_task_contract_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL UNIQUE REFERENCES verification_attempts(attempt_id) ON DELETE CASCADE,
    contract_json TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS verification_task_leases (
    verification_lease_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL UNIQUE REFERENCES verification_attempts(attempt_id) ON DELETE CASCADE,
    worker_instance_id TEXT NOT NULL REFERENCES worker_instances(worker_instance_id) ON DELETE CASCADE,
    lease_epoch INTEGER NOT NULL,
    cancellation_epoch INTEGER NOT NULL,
    lease_token_hash TEXT NOT NULL,
    status TEXT NOT NULL,
    offered_at TEXT NOT NULL,
    leased_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    last_heartbeat_at TEXT NOT NULL,
    completed_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_verification_task_leases_active
ON verification_task_leases(project_id, status, expires_at);

CREATE TABLE IF NOT EXISTS verification_result_envelopes (
    verification_result_envelope_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL REFERENCES verification_attempts(attempt_id) ON DELETE CASCADE,
    verification_lease_id TEXT NOT NULL REFERENCES verification_task_leases(verification_lease_id) ON DELETE CASCADE,
    lease_epoch INTEGER NOT NULL,
    cancellation_epoch INTEGER NOT NULL,
    outcome TEXT NOT NULL,
    envelope_json TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    status TEXT NOT NULL,
    rejection_reason TEXT,
    submitted_at TEXT NOT NULL,
    ingested_at TEXT,
    UNIQUE(project_id, idempotency_key),
    UNIQUE(attempt_id, content_hash)
);

CREATE INDEX IF NOT EXISTS idx_verification_result_pending
ON verification_result_envelopes(project_id, status, submitted_at);
