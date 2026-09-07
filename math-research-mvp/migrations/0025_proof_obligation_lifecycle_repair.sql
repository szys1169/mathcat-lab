-- Forward-only lifecycle hardening for the proof-obligation graph and Fact governance.

-- Keep the oldest active challenge and retire duplicate historical work before enforcing
-- the single-active-review invariant.
WITH ranked AS (
    SELECT challenge_id, verification_id,
           ROW_NUMBER() OVER (PARTITION BY fact_id ORDER BY created_at, challenge_id) AS ordinal
    FROM fact_challenges
    WHERE status = 'open'
)
UPDATE verifications
SET status = 'superseded',
    completed_at = COALESCE(completed_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
WHERE verification_id IN (
    SELECT verification_id FROM ranked WHERE ordinal > 1 AND verification_id IS NOT NULL
)
  AND status IN ('submitted', 'verifying');

WITH ranked AS (
    SELECT challenge_id,
           ROW_NUMBER() OVER (PARTITION BY fact_id ORDER BY created_at, challenge_id) AS ordinal
    FROM fact_challenges
    WHERE status = 'open'
)
UPDATE fact_challenges
SET status = 'cancelled',
    resolved_at = COALESCE(resolved_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
WHERE challenge_id IN (SELECT challenge_id FROM ranked WHERE ordinal > 1);

CREATE UNIQUE INDEX IF NOT EXISTS idx_fact_challenges_one_open_per_fact
ON fact_challenges(fact_id)
WHERE status = 'open';

UPDATE proof_obligations
SET completion_criteria = 'A FullyCertified Fact must pass the Goal coverage gate; a proof satisfies this obligation and a counterexample makes it obsolete by refuting the Goal.'
WHERE source_kind = 'root_goal';

-- Releases predating the hard closure gate could mark a Goal solved/refuted with Reviewed
-- evidence. Reopen only those weak legacy closures; later FullyCertified work is preserved.
UPDATE proof_obligations
SET status = 'open',
    satisfied_by_fact_id = NULL,
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE source_kind = 'root_goal'
  AND json_extract(provenance_json, '$.source') = 'migration_backfill'
  AND status IN ('satisfied', 'obsolete')
  AND EXISTS (
      SELECT 1 FROM goals g
      WHERE g.project_id = proof_obligations.project_id
        AND g.goal_id = proof_obligations.goal_id
        AND g.status IN ('solved', 'refuted')
        AND NOT EXISTS (
            SELECT 1 FROM fact_assurances fa
            WHERE fa.fact_id = g.solved_by_fact_id
              AND fa.status = 'active'
              AND fa.acceptance_class = 'fully_certified'
        )
  );

UPDATE projects
SET status = 'needs_human_review',
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE status IN ('success', 'refuted')
  AND EXISTS (
      SELECT 1 FROM goals g
      JOIN proof_obligations po
        ON po.project_id = g.project_id
       AND po.goal_id = g.goal_id
       AND po.source_kind = 'root_goal'
       AND json_extract(po.provenance_json, '$.source') = 'migration_backfill'
      WHERE g.project_id = projects.project_id
        AND g.status IN ('solved', 'refuted')
        AND NOT EXISTS (
            SELECT 1 FROM fact_assurances fa
            WHERE fa.fact_id = g.solved_by_fact_id
              AND fa.status = 'active'
              AND fa.acceptance_class = 'fully_certified'
        )
  );

UPDATE goals
SET status = 'open', solved_by_fact_id = NULL
WHERE status IN ('solved', 'refuted')
  AND EXISTS (
      SELECT 1 FROM proof_obligations po
      WHERE po.project_id = goals.project_id
        AND po.goal_id = goals.goal_id
        AND po.source_kind = 'root_goal'
        AND json_extract(po.provenance_json, '$.source') = 'migration_backfill'
  )
  AND NOT EXISTS (
      SELECT 1 FROM fact_assurances fa
      WHERE fa.fact_id = goals.solved_by_fact_id
        AND fa.status = 'active'
        AND fa.acceptance_class = 'fully_certified'
  );
