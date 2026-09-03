ALTER TABLE candidates ADD COLUMN idempotency_key TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_candidates_project_idempotency
    ON candidates(project_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
