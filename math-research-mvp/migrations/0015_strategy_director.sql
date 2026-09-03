CREATE TABLE IF NOT EXISTS strategy_states (
    strategy_state_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    round_id TEXT NOT NULL REFERENCES rounds(round_id) ON DELETE CASCADE,
    input_hash TEXT NOT NULL,
    audit_kind TEXT NOT NULL CHECK(audit_kind IN ('initial', 'control', 'macro')),
    fixed_goal TEXT NOT NULL,
    verdict_summary TEXT NOT NULL,
    proof_skeleton_json TEXT NOT NULL,
    route_portfolio_json TEXT NOT NULL,
    interface_debts_json TEXT NOT NULL,
    central_missing_bridge TEXT NOT NULL,
    method_vs_proposition_failure TEXT NOT NULL CHECK(method_vs_proposition_failure IN ('method_failure', 'proposition_failure', 'undetermined')),
    dangerous_shortcuts_json TEXT NOT NULL,
    strategy_directives_json TEXT NOT NULL,
    literature_priorities_json TEXT NOT NULL,
    macro_replan_required INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    UNIQUE(project_id, round_id, input_hash)
);

CREATE INDEX IF NOT EXISTS idx_strategy_states_project_created
ON strategy_states(project_id, created_at, strategy_state_id);
