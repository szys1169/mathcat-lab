-- Artifact integrity is audited incrementally at service startup.  Every artifact is
-- explicitly unverified until a byte-for-byte SHA-256 check succeeds; absence of a
-- recent startup batch must never be interpreted as successful verification.
CREATE TABLE artifact_integrity_audit_state (
    artifact_id TEXT PRIMARY KEY REFERENCES artifacts(artifact_id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'unverified'
        CHECK(status IN ('unverified', 'verified', 'failed')),
    last_checked_sha256 TEXT,
    last_verified_sha256 TEXT,
    last_checked_at TEXT,
    last_verified_at TEXT,
    last_error TEXT,
    check_count INTEGER NOT NULL DEFAULT 0 CHECK(check_count >= 0),
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_artifact_integrity_audit_priority
    ON artifact_integrity_audit_state(project_id, status, last_checked_at, artifact_id);

CREATE TABLE artifact_integrity_audit_cursors (
    scope_key TEXT PRIMARY KEY,
    project_id TEXT REFERENCES projects(project_id) ON DELETE CASCADE,
    last_artifact_id TEXT,
    completed_cycles INTEGER NOT NULL DEFAULT 0 CHECK(completed_cycles >= 0),
    last_batch_checked_count INTEGER NOT NULL DEFAULT 0
        CHECK(last_batch_checked_count >= 0),
    last_batch_started_at TEXT,
    last_batch_completed_at TEXT,
    updated_at TEXT NOT NULL
);

-- Existing artifacts are intentionally backfilled as unverified.  A later startup
-- batch or explicit full audit must read the bytes before changing this state.
INSERT INTO artifact_integrity_audit_state(
    artifact_id,
    project_id,
    status,
    check_count,
    updated_at
)
SELECT artifact_id, project_id, 'unverified', 0, created_at
FROM artifacts;
