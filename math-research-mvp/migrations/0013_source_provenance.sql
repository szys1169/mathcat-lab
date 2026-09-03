-- Preserve where literature claims came from and make duplicate ingestion auditable.

ALTER TABLE sources ADD COLUMN normalized_url TEXT;
ALTER TABLE sources ADD COLUMN identifier_kind TEXT;
ALTER TABLE sources ADD COLUMN identifier_value TEXT;
ALTER TABLE sources ADD COLUMN origin_task_id TEXT REFERENCES tasks(task_id) ON DELETE SET NULL;
ALTER TABLE sources ADD COLUMN origin_route_id TEXT REFERENCES routes(route_id) ON DELETE SET NULL;
ALTER TABLE sources ADD COLUMN fulltext_artifact_id TEXT REFERENCES artifacts(artifact_id) ON DELETE SET NULL;
ALTER TABLE sources ADD COLUMN provenance_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE sources ADD COLUMN disposition_reason TEXT;

CREATE INDEX IF NOT EXISTS idx_sources_normalized_url
ON sources(project_id, normalized_url);

CREATE INDEX IF NOT EXISTS idx_sources_identifier
ON sources(project_id, identifier_kind, identifier_value);

CREATE TABLE IF NOT EXISTS source_ingestion_records (
    ingestion_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    task_id TEXT REFERENCES tasks(task_id) ON DELETE SET NULL,
    route_id TEXT REFERENCES routes(route_id) ON DELETE SET NULL,
    source_id TEXT REFERENCES sources(source_id) ON DELETE SET NULL,
    disposition TEXT NOT NULL,
    reasons_json TEXT NOT NULL DEFAULT '[]',
    normalized_url TEXT,
    identifier_kind TEXT,
    identifier_value TEXT,
    raw_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_source_ingestion_project
ON source_ingestion_records(project_id, created_at);
