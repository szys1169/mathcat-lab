CREATE TABLE IF NOT EXISTS sources (
    source_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    authors_json TEXT NOT NULL DEFAULT '[]',
    url TEXT,
    citation_key TEXT,
    theorem_reference TEXT,
    statement_excerpt TEXT,
    assumptions_json TEXT NOT NULL DEFAULT '[]',
    applicability TEXT NOT NULL,
    status TEXT NOT NULL,
    retrieved_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sources_project ON sources(project_id, retrieved_at);
