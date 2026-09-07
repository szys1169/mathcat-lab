-- First-class proof coverage graph. Bottlenecks remain the executable planning register;
-- obligations record what is (or is not) covered and link back to those bottlenecks.

CREATE TABLE IF NOT EXISTS proof_obligations (
    obligation_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    goal_id TEXT REFERENCES goals(goal_id) ON DELETE CASCADE,
    parent_obligation_id TEXT REFERENCES proof_obligations(obligation_id) ON DELETE SET NULL,
    source_kind TEXT NOT NULL,
    statement TEXT NOT NULL,
    completion_criteria TEXT NOT NULL,
    necessity TEXT NOT NULL CHECK (necessity IN ('required', 'advisory')),
    status TEXT NOT NULL CHECK (status IN ('open', 'satisfied', 'blocked', 'obsolete')),
    priority REAL NOT NULL,
    source_verification_id TEXT REFERENCES verifications(verification_id) ON DELETE SET NULL,
    source_bottleneck_id TEXT REFERENCES bottlenecks(bottleneck_id) ON DELETE SET NULL,
    source_fingerprint TEXT NOT NULL,
    provenance_json TEXT NOT NULL DEFAULT '{}',
    satisfied_by_fact_id TEXT REFERENCES facts(fact_id) ON DELETE SET NULL,
    created_revision INTEGER NOT NULL,
    updated_revision INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_proof_obligations_root_goal
ON proof_obligations(project_id, goal_id)
WHERE source_kind = 'root_goal' AND status <> 'obsolete';

CREATE UNIQUE INDEX IF NOT EXISTS idx_proof_obligations_report_item
ON proof_obligations(project_id, source_verification_id, source_kind, source_fingerprint)
WHERE source_verification_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_proof_obligations_project_status_priority
ON proof_obligations(project_id, status, necessity, priority DESC, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_proof_obligations_goal_status
ON proof_obligations(project_id, goal_id, status, necessity);

CREATE TABLE IF NOT EXISTS obligation_edges (
    edge_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    source_obligation_id TEXT NOT NULL REFERENCES proof_obligations(obligation_id) ON DELETE CASCADE,
    target_obligation_id TEXT NOT NULL REFERENCES proof_obligations(obligation_id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('depends_on', 'refines')),
    created_at TEXT NOT NULL,
    UNIQUE(project_id, source_obligation_id, target_obligation_id, kind),
    CHECK (source_obligation_id <> target_obligation_id)
);

CREATE INDEX IF NOT EXISTS idx_obligation_edges_project_source
ON obligation_edges(project_id, source_obligation_id, kind);

CREATE TABLE IF NOT EXISTS candidate_obligation_coverage (
    coverage_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    candidate_id TEXT NOT NULL REFERENCES candidates(candidate_id) ON DELETE CASCADE,
    obligation_id TEXT NOT NULL REFERENCES proof_obligations(obligation_id) ON DELETE CASCADE,
    verification_id TEXT NOT NULL REFERENCES verifications(verification_id) ON DELETE CASCADE,
    disposition TEXT NOT NULL CHECK (disposition IN ('supports', 'satisfies', 'insufficient', 'unknown')),
    fact_id TEXT REFERENCES facts(fact_id) ON DELETE SET NULL,
    rationale TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(candidate_id, obligation_id, verification_id)
);

CREATE INDEX IF NOT EXISTS idx_candidate_obligation_coverage_project_obligation
ON candidate_obligation_coverage(project_id, obligation_id, created_at DESC);

-- Preserve the trusted status of projects created before this migration. New projects create
-- their root obligation in the same transaction as their Goal and initial Bottleneck.
INSERT OR IGNORE INTO proof_obligations(
    obligation_id, project_id, goal_id, parent_obligation_id, source_kind, statement,
    completion_criteria, necessity, status, priority, source_bottleneck_id,
    source_fingerprint, provenance_json, satisfied_by_fact_id, created_revision,
    updated_revision, created_at, updated_at
)
SELECT
    'obligation_root_' || g.goal_id,
    g.project_id,
    g.goal_id,
    NULL,
    'root_goal',
    g.statement,
    'A FullyCertified non-counterexample Fact must pass the Goal coverage gate.',
    'required',
    CASE WHEN g.status = 'solved' THEN 'satisfied'
         WHEN g.status = 'refuted' THEN 'obsolete'
         ELSE 'open' END,
    g.priority,
    (SELECT b.bottleneck_id
       FROM bottlenecks b
      WHERE b.project_id = g.project_id
        AND b.kind = 'main_goal_unresolved'
        AND EXISTS (SELECT 1 FROM json_each(b.target_goal_ids_json) j WHERE j.value = g.goal_id)
      ORDER BY b.created_revision, b.bottleneck_id
      LIMIT 1),
    'root:' || g.goal_id,
    json_object('source', 'migration_backfill', 'goal_id', g.goal_id),
    g.solved_by_fact_id,
    COALESCE(NULLIF(g.created_in_round, 0), 1),
    p.revision,
    p.created_at,
    p.updated_at
FROM goals g
JOIN projects p ON p.project_id = g.project_id;
