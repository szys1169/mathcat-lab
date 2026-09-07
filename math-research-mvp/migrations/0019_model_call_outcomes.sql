-- Historical usage rows predate outcome tracking and represent calls that were
-- already finalized by the old runtime. New reservations explicitly start in
-- `running` and are closed as either `succeeded` or `failed`.
ALTER TABLE usage_records ADD COLUMN outcome TEXT NOT NULL DEFAULT 'succeeded';
ALTER TABLE usage_records ADD COLUMN error_kind TEXT;
ALTER TABLE usage_records ADD COLUMN error_message TEXT;

CREATE INDEX IF NOT EXISTS idx_usage_records_project_outcome
ON usage_records(project_id, outcome);
