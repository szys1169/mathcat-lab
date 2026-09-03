CREATE TABLE IF NOT EXISTS verification_policies (
    policy_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    profile TEXT NOT NULL,
    required_acceptance TEXT NOT NULL,
    required_checks_json TEXT NOT NULL,
    independent_reviewer_count INTEGER NOT NULL,
    require_citation_review INTEGER NOT NULL,
    require_adversarial_review INTEGER NOT NULL,
    require_alignment_review INTEGER NOT NULL,
    require_fresh_replay INTEGER NOT NULL,
    max_attempts INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS verification_cases (
    case_id TEXT PRIMARY KEY,
    verification_id TEXT NOT NULL UNIQUE REFERENCES verifications(verification_id) ON DELETE CASCADE,
    candidate_id TEXT NOT NULL REFERENCES candidates(candidate_id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    policy_id TEXT NOT NULL REFERENCES verification_policies(policy_id),
    snapshot_id TEXT,
    profile TEXT NOT NULL,
    required_acceptance TEXT NOT NULL,
    achieved_acceptance TEXT,
    stage TEXT NOT NULL,
    risk_score REAL NOT NULL,
    risk_reasons_json TEXT NOT NULL,
    cancellation_epoch INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT
);

CREATE TABLE IF NOT EXISTS verification_snapshots (
    snapshot_id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL UNIQUE REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    candidate_hash TEXT NOT NULL,
    project_revision INTEGER NOT NULL,
    contract_version INTEGER NOT NULL,
    dependency_hashes_json TEXT NOT NULL,
    source_hashes_json TEXT NOT NULL,
    policy_hash TEXT NOT NULL,
    toolchain_hash TEXT,
    content_hash TEXT NOT NULL UNIQUE,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS verification_attempts (
    attempt_id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    backend TEXT,
    cancellation_epoch INTEGER NOT NULL DEFAULT 0,
    input_hash TEXT NOT NULL,
    output_hash TEXT,
    error_kind TEXT,
    error_message TEXT,
    started_at TEXT,
    completed_at TEXT,
    UNIQUE(case_id, sequence)
);

CREATE TABLE IF NOT EXISTS verification_checks (
    check_id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    attempt_id TEXT REFERENCES verification_attempts(attempt_id) ON DELETE SET NULL,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    mandatory INTEGER NOT NULL,
    summary TEXT NOT NULL,
    details_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    completed_at TEXT,
    UNIQUE(case_id, attempt_id, kind)
);

CREATE TABLE IF NOT EXISTS verification_findings (
    finding_id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    check_id TEXT REFERENCES verification_checks(check_id) ON DELETE SET NULL,
    reviewer_kind TEXT NOT NULL,
    severity TEXT NOT NULL,
    category TEXT NOT NULL,
    location TEXT,
    claim TEXT NOT NULL,
    rationale TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS verification_evidence (
    evidence_id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    check_id TEXT REFERENCES verification_checks(check_id) ON DELETE SET NULL,
    dimension TEXT NOT NULL,
    kind TEXT NOT NULL,
    uri TEXT,
    sha256 TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS semantic_contracts (
    contract_id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    natural_language_statement TEXT NOT NULL,
    variables_json TEXT NOT NULL,
    assumptions_json TEXT NOT NULL,
    conclusion TEXT NOT NULL,
    definitions_json TEXT NOT NULL,
    boundary_conditions_json TEXT NOT NULL,
    ambiguity_notes_json TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(case_id, version),
    UNIQUE(case_id, content_hash)
);

CREATE TABLE IF NOT EXISTS formalizations (
    formalization_id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    semantic_contract_id TEXT NOT NULL REFERENCES semantic_contracts(contract_id),
    theorem_name TEXT NOT NULL,
    lean_statement TEXT NOT NULL,
    lean_source TEXT NOT NULL,
    mapping_json TEXT NOT NULL,
    source_hash TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(case_id, source_hash)
);

CREATE TABLE IF NOT EXISTS alignment_reviews (
    alignment_id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    formalization_id TEXT NOT NULL REFERENCES formalizations(formalization_id) ON DELETE CASCADE,
    relation TEXT NOT NULL,
    reviewer_kind TEXT NOT NULL,
    rationale TEXT NOT NULL,
    missing_assumptions_json TEXT NOT NULL,
    extra_assumptions_json TEXT NOT NULL,
    confidence REAL NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS backend_runs (
    backend_run_id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    attempt_id TEXT REFERENCES verification_attempts(attempt_id) ON DELETE SET NULL,
    backend TEXT NOT NULL,
    backend_version TEXT NOT NULL,
    status TEXT NOT NULL,
    command_json TEXT NOT NULL,
    working_directory TEXT NOT NULL,
    exit_code INTEGER,
    stdout TEXT NOT NULL,
    stderr TEXT NOT NULL,
    diagnostics_json TEXT NOT NULL,
    axioms_json TEXT NOT NULL,
    elapsed_ms INTEGER NOT NULL,
    input_hash TEXT NOT NULL,
    output_hash TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS verification_packages (
    package_id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    manifest_json TEXT NOT NULL,
    manifest_hash TEXT NOT NULL UNIQUE,
    storage_path TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS verification_replays (
    replay_id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    package_id TEXT NOT NULL REFERENCES verification_packages(package_id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    fresh_process INTEGER NOT NULL,
    manifest_hash TEXT NOT NULL,
    observed_hash TEXT NOT NULL,
    backend_run_id TEXT REFERENCES backend_runs(backend_run_id),
    summary TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS proof_searches (
    search_id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id) ON DELETE CASCADE,
    formalization_id TEXT NOT NULL REFERENCES formalizations(formalization_id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    strategy TEXT NOT NULL,
    budget_json TEXT NOT NULL,
    nodes_created INTEGER NOT NULL DEFAULT 0,
    nodes_expanded INTEGER NOT NULL DEFAULT 0,
    model_calls INTEGER NOT NULL DEFAULT 0,
    cancellation_epoch INTEGER NOT NULL DEFAULT 0,
    root_node_id TEXT,
    solution_node_id TEXT,
    started_at TEXT NOT NULL,
    completed_at TEXT
);

CREATE TABLE IF NOT EXISTS proof_nodes (
    node_id TEXT PRIMARY KEY,
    search_id TEXT NOT NULL REFERENCES proof_searches(search_id) ON DELETE CASCADE,
    parent_node_id TEXT REFERENCES proof_nodes(node_id) ON DELETE CASCADE,
    depth INTEGER NOT NULL,
    state_id INTEGER,
    goal TEXT NOT NULL,
    local_context_json TEXT NOT NULL,
    tactic TEXT,
    score REAL NOT NULL,
    status TEXT NOT NULL,
    diagnostic TEXT,
    cancellation_epoch INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS proof_edges (
    edge_id TEXT PRIMARY KEY,
    search_id TEXT NOT NULL REFERENCES proof_searches(search_id) ON DELETE CASCADE,
    source_node_id TEXT NOT NULL REFERENCES proof_nodes(node_id) ON DELETE CASCADE,
    target_node_id TEXT NOT NULL REFERENCES proof_nodes(node_id) ON DELETE CASCADE,
    tactic TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(search_id, source_node_id, target_node_id)
);

CREATE TABLE IF NOT EXISTS proof_hints (
    hint_id TEXT PRIMARY KEY,
    search_id TEXT NOT NULL REFERENCES proof_searches(search_id) ON DELETE CASCADE,
    node_id TEXT REFERENCES proof_nodes(node_id) ON DELETE SET NULL,
    content TEXT NOT NULL,
    requested_by TEXT NOT NULL,
    effective_after_expansion INTEGER NOT NULL,
    consumed_at TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS fact_assurances (
    assurance_id TEXT PRIMARY KEY,
    fact_id TEXT NOT NULL REFERENCES facts(fact_id) ON DELETE CASCADE,
    case_id TEXT NOT NULL REFERENCES verification_cases(case_id),
    acceptance_class TEXT NOT NULL,
    snapshot_hash TEXT NOT NULL,
    package_id TEXT REFERENCES verification_packages(package_id),
    replay_id TEXT REFERENCES verification_replays(replay_id),
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    invalidated_at TEXT,
    UNIQUE(fact_id, case_id)
);

CREATE INDEX IF NOT EXISTS idx_verification_cases_project_stage
    ON verification_cases(project_id, stage);
CREATE INDEX IF NOT EXISTS idx_verification_checks_case_kind
    ON verification_checks(case_id, kind);
CREATE INDEX IF NOT EXISTS idx_verification_findings_case
    ON verification_findings(case_id, severity);
CREATE INDEX IF NOT EXISTS idx_backend_runs_case_backend
    ON backend_runs(case_id, backend);
CREATE INDEX IF NOT EXISTS idx_proof_nodes_search_status
    ON proof_nodes(search_id, status, score DESC);
CREATE INDEX IF NOT EXISTS idx_fact_assurances_fact_status
    ON fact_assurances(fact_id, status);
