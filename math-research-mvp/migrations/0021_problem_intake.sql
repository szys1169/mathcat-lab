CREATE TABLE IF NOT EXISTS problem_drafts (
    draft_id TEXT PRIMARY KEY,
    requested_by TEXT NOT NULL,
    creation_idempotency_key TEXT NOT NULL,
    creation_request_hash TEXT NOT NULL,
    prompt TEXT NOT NULL,
    material_directory TEXT NOT NULL,
    materials_json TEXT NOT NULL,
    material_manifest_hash TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN (
        'generating',
        'awaiting_confirmation',
        'confirmed',
        'failed'
    )),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    document_json TEXT,
    document_hash TEXT,
    model TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    elapsed_ms INTEGER NOT NULL DEFAULT 0 CHECK (elapsed_ms >= 0),
    error_kind TEXT,
    error_message TEXT,
    confirmation_idempotency_key TEXT,
    confirmation_request_hash TEXT,
    confirmed_project_id TEXT UNIQUE REFERENCES projects(project_id),
    start_command_id TEXT UNIQUE REFERENCES human_commands(command_id),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    generation_completed_at TEXT,
    confirmed_at TEXT,
    UNIQUE(requested_by, creation_idempotency_key),
    CHECK ((status = 'awaiting_confirmation' OR status = 'confirmed') =
           (document_json IS NOT NULL AND document_hash IS NOT NULL)),
    CHECK ((confirmation_idempotency_key IS NULL) =
           (confirmation_request_hash IS NULL)),
    CHECK (status != 'confirmed' OR
           (confirmation_idempotency_key IS NOT NULL AND
            confirmation_request_hash IS NOT NULL AND
            confirmed_project_id IS NOT NULL AND
            confirmed_at IS NOT NULL)),
    CHECK (status = 'confirmed' OR
           (confirmed_project_id IS NULL AND start_command_id IS NULL AND confirmed_at IS NULL))
);

CREATE TABLE IF NOT EXISTS problem_draft_attempts (
    attempt_id TEXT PRIMARY KEY,
    draft_id TEXT NOT NULL REFERENCES problem_drafts(draft_id) ON DELETE CASCADE,
    attempt_number INTEGER NOT NULL CHECK (attempt_number > 0),
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed')),
    model TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    elapsed_ms INTEGER NOT NULL DEFAULT 0 CHECK (elapsed_ms >= 0),
    error_kind TEXT,
    error_message TEXT,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    UNIQUE(draft_id, attempt_number)
);

CREATE INDEX IF NOT EXISTS idx_problem_drafts_status_updated
    ON problem_drafts(status, updated_at, draft_id);

CREATE INDEX IF NOT EXISTS idx_problem_draft_attempts_draft_status
    ON problem_draft_attempts(draft_id, status, attempt_number);
